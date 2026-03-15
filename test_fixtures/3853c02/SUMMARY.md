# Call workspace/create RPC instead of /api/ready for bootstrap

Today, when cortex needs to bootstrap a sandbox (set up git repos, agent templates, plugins, credentials), it sends a flat bag of environment variables to sboxd's `/api/ready` HTTP endpoint. This is untyped, only runs for reused sandboxes, and requires nimbus as a middleman for newly provisioned ones.

This change introduces an alternative path: cortex calls sboxd's `workspace/create` WebSocket RPC with a structured `BootstrapConfig`. A Statsig feature flag (`cortex_workspace_create_bootstrap`) switches between the two paths, so we can roll out incrementally. The old `/api/ready` path is preserved as the default fallback.

## 1. New types: BootstrapConfig on the wire protocol

The Go sboxd proto already defines `BootstrapConfig` and `GitConfig` (added in a prior sboxd PR). The TypeScript stream-protocol types need to mirror them so the TS client can send them over the wire.

`GitConfig` holds ugit server credentials. `BootstrapConfig` carries everything sboxd needs to set up a workspace: which workload to run, who owns it, git access, and an opaque `env` map for feature flags that sboxd passes through to foundry controller hooks.

The key addition is `bootstrap?: BootstrapConfig` on `WorkspaceCreateRequest`. When present, sboxd runs bootstrap inline after creating the workspace directory. When absent, the existing behavior (create an empty directory) is unchanged.

```difft services/agentplat/stream-protocol/src/types.ts chunks=all
        workingDirectory?: string
      }
      
   1 +// --- Bootstrap Config (proto/workspace.go) ---
   3 +export interface GitConfig {
   4 +  baseUrl: string
   5 +  username: string
   6 +  password: string
   7 +}
   9 +export interface BootstrapConfig {
  10 +  workloadName: string
  11 +  workloadOwnerType: string
  12 +  workloadOwnerID: string
  13 +  agentType?: string
  14 +  agentConfigID?: string
  15 +  sandboxGitEnabled?: boolean
  16 +  git: GitConfig
  17 +}
   2 +
   8 +
  18 +
  19  export interface WorkspaceCreateRequest {
  20    rootPath?: string
  22    args?: Record<string, string>
  23 +  bootstrap?: BootstrapConfig
  24  }
  25  
  26  export interface WorkspaceCreateResponse {
```

## 2. Client: accept BootstrapConfig in createWorkspace

The TS client's `createWorkspace` method needs to accept the new `bootstrap` field in its options. This widens the options type to include `bootstrap?: BootstrapConfig` alongside the existing `workspaceId`.

```difft services/agentplat/ts-client/src/client.ts chunks=all
        RepositoryStatus,
        RepoSaveResult,
        GitCredentials,
   1 +  BootstrapConfig,
   2    FileMetadata,
   3    FileExistsResponse,
   4    FileWriteRequest,
  ...
 227    createWorkspace(
 228      rootPath: string,
 229      cbOrOpts?:
 230 +      | Callback<WorkspaceMetadata | undefined>
 231 +      | { workspaceId?: WorkspaceID; bootstrap?: BootstrapConfig },
 234 -    let opts: { workspaceId?: WorkspaceID } = {}
 234 +    let opts: { workspaceId?: WorkspaceID; bootstrap?: BootstrapConfig } = {}
 235      let callback: Callback<WorkspaceMetadata | undefined>
 236      if (typeof cbOrOpts === 'function') {
 237        callback = cbOrOpts
```

## 3. New bootstrap function and call site

This is the core change. Two new functions are added to `foundry_api.ts`, and the existing bootstrap call site is restructured.

First, the new imports bring in `AgentPlatClient` (the WebSocket client) and the `BootstrapConfig` type, plus the default workspace constants:

```difft services/cortex/lib/support/foundry_api.ts chunks=0
      import { maybeRewriteSandboxUrl } from '#lib/foundry/maybe_rewrite_sandbox_url.js'
      import statsD from './statsd.js'
      import { generateSinatraJwt, generateUgitJwt } from '#lib/util/jwt.js'
   1 +import { AgentPlatClient } from '@figma/agentplat-client'
   2 +import { DEFAULT_WORKSPACE_ID, DEFAULT_WORKSPACE_PATH } from '@figma/agentplat-stream-protocol'
   3 +import type { BootstrapConfig } from '@figma/agentplat-stream-protocol'
   4  
   5  let antiabuseClient: AntiabuseClient | null = null
   6  
```

Next, `bootstrapViaWorkspaceCreate` is the new function that constructs a `BootstrapConfig` from the request context and calls sboxd's `workspace/create` RPC over WebSocket. It includes a 30-second timeout and proper cleanup via `finally`:

```difft services/cortex/lib/support/foundry_api.ts chunks=1
        }
      }
      
   1 +/**
   2 + * Calls sboxd workspace/create with structured BootstrapConfig via WebSocket.
   3 + * Replaces the env-var-based /api/ready bootstrap path. Bootstrap is idempotent,
   4 + * so this is safe to call on every sandbox access (new, warm, or reused).
   5 + */
   6 +async function bootstrapViaWorkspaceCreate(
   7 +  sboxdUrl: string,
   8 +  context: RequestContext,
   9 +  workloadConfig: WorkloadConfig,
  10 +): Promise<void> {
  11 +  const apiUrl = getSettings().ugit_base_url || hardcodedApiUrls[getEnv()]
  13 +  const bootstrap: BootstrapConfig = {
  14 +    workloadName: workloadConfig.workloadName,
  15 +    workloadOwnerType: 'file',
  16 +    workloadOwnerID: context.fileId!,
  17 +    git: {
  18 +      baseUrl: `${apiUrl}/git`,
  19 +      username: context.fileId!,
  20 +      password: await generateUgitJwt(context, workloadConfig.workloadName),
  21 +    },
  22 +    agentType: 'claude',
  23 +    sandboxGitEnabled: CortexStatsig.checkGate(context, 'make_sandbox_git_persistence', false),
  24 +  }
  26 +  logger.info('Bootstrapping workspace via workspace/create', { sboxd_url: sboxdUrl })
  28 +  const client = new AgentPlatClient(sboxdUrl)
  30 +  try {
  31 +    await new Promise<void>((resolve, reject) => {
  32 +      const timeout = setTimeout(() => {
  33 +        reject(new Error('sboxd workspace/create timed out after 30s'))
  34 +      }, 30_000)
  36 +      client.connect(undefined, (connectErr) => {
  37 +        if (connectErr) {
  38 +          clearTimeout(timeout)
  39 +          return reject(connectErr)
  40 +        }
  42 +        client.createWorkspace(
  43 +          DEFAULT_WORKSPACE_PATH,
  44 +          { workspaceId: DEFAULT_WORKSPACE_ID, bootstrap },
  45 +          (createErr) => {
  46 +            clearTimeout(timeout)
  47 +            if (createErr) return reject(createErr)
  48 +            resolve()
  49 +          },
  50 +        )
  51 +      })
  52 +    })
  53 +  } finally {
  54 +    client.close()
  55 +  }
  56 +}
  12 +
  25 +
  27 +
  29 +
  35 +
  41 +
  57 +
  58  /**
  59   * Client for interacting with the Foundry controller API
```

Finally, the existing bootstrap call site is restructured. Previously it only called `reconcileSboxdReady` for reused, non-warm sandboxes. Now it checks the `cortex_workspace_create_bootstrap` Statsig gate first: if enabled, it calls the new `bootstrapViaWorkspaceCreate` for all workload-configured sandboxes; otherwise it falls back to the old `reconcileSboxdReady` path with the same conditions as before:

```difft services/cortex/lib/support/foundry_api.ts chunks=2
      
            const rewrittenSboxdUrl = sboxdUrl ? maybeRewriteSandboxUrl(sboxdUrl, context) : undefined
      
   1 -      // When a sandbox is reused (not newly provisioned, not from warm pool) and
   1 +      // Bootstrap the workspace on sboxd. With the feature flag enabled, we call
   2 -      // a workload is configured, Nimbus skips the sboxd ready call. We call it
   2 +      // workspace/create with structured BootstrapConfig for all workload-configured
   3 -      // directly to bootstrap the workspace (git repos, agent templates, plugins,
   3 +      // sandboxes (new, warm, reused) since bootstrap is idempotent. Without the
   4 -      // credentials). Errors are propagated because a failed reconciliation means
   4 +      // flag, we fall back to the legacy /api/ready path for reused sandboxes only.
   1 -      // the sboxd session will definitely fail.
   5 -      if (
   5 +      if (workloadConfig && context.fileId && rewrittenSboxdUrl) {
   6 -        workloadConfig &&
   6 +        const useWorkspaceCreate = CortexStatsig.checkGate(
   7 -        rewrittenSboxdUrl &&
   7 +          context,
   8 +          'cortex_workspace_create_bootstrap',
   9 +          false,
  10 +        )
  12 +        if (useWorkspaceCreate) {
  13 +          await bootstrapViaWorkspaceCreate(rewrittenSboxdUrl, context, workloadConfig)
  14 +          statsD.increment('foundry_api_client.workspace_bootstrapped', 1, tags)
  15 -        !sandboxResponse.newlyProvisioned &&
  15 +        } else if (!sandboxResponse.newlyProvisioned && !sandboxResponse.warm) {
   1 -        !sandboxResponse.warm
   1 -      ) {
  19 +      }
  11 +
  16            await reconcileSboxdReady(rewrittenSboxdUrl, controllerToken, envVars)
  17            statsD.increment('foundry_api_client.sboxd_reconciled', 1, tags)
  18 +        }
```

## 4. Feature flag and dependency

A new Statsig flag `cortex_workspace_create_bootstrap` gates the new path. This lets us roll out incrementally and kill-switch back to `/api/ready` if needed:

```difft services/cortex/lib/statsig/flags.ts chunks=all
        'sandboxed_assistant',
        'foundry_cached_preview_sandbox_trigger',
        'make_sandbox_git_persistence',
   1 +  'cortex_workspace_create_bootstrap',
   2    'voice_to_text',
   3  ] as const
   4  
```

The `@figma/agentplat-client` package is added as a workspace dependency for the WebSocket client:

```difft services/cortex/package.json chunks=all
          "@figma/ai-agent-adapters-claude-cli": "workspace:*",
          "@figma/agent-sandbox-modal-direct": "workspace:*",
          "@figma/ai-agent-adapters-types": "workspace:*",
   1 +    "@figma/agentplat-client": "workspace:*",
   2      "@figma/agentplat-stream-protocol": "workspace:*",
   3      "modal": "catalog:",
   4      "@google-cloud/monitoring": "^4.1.0",
```
