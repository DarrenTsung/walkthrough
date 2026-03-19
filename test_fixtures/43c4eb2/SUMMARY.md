# Session manager: auto-create + validation

Test fixture for per-side fold behavior on mixed diffs.

## Paired changes (comment rewrite + signature change)

Both sides have content. The `new` fold targets new-file line numbers.

```difft services/agentplat/sbox/sboxd/internal/session/manager.go chunks=5
      	return SessionStatusRunning
      }
      
   1 -// createSessionLockedWithOpts creates a new session. When isDefault is true, the
   1 +// createSessionLockedWithOpts creates a new session. If id is empty, a UUID is
   2 -// session uses the shared config dir at {sessionPath}/{agentType}/ (backward compat).
   2 +// generated. When isDefault is true, the session uses the shared config dir at
   3 +// {sessionPath}/{agentType}/ (backward compat). Otherwise, the session gets a
   4 -// Otherwise, the session gets a per-session directory at {sessionPath}/sessions/{id}/{agentType}/.
   4 +// per-session directory at {sessionPath}/sessions/{id}/{agentType}/.
   5 -func (m *Manager) createSessionLockedWithOpts(agentType, workspaceID, wsPath string, isDefault bool) (*session, error) {
   5 +func (m *Manager) createSessionLockedWithOpts(agentType, workspaceID, wsPath string, isDefault bool, id string) (*session, error) {
   6  	if workspaceID == "" {
   7  		workspaceID = m.defaultWorkspaceID
   8  	}
```

```folds
new 1-5: doc comment + signature change (added `id string` param)
```

## Added-only block (auto-create logic)

Mostly added lines with one paired changed line in the middle.

```difft services/agentplat/sbox/sboxd/internal/session/manager.go chunks=4
      		var ok bool
      		s, ok = m.getSessionLocked(sessionID)
      		if !ok {
   1 +			// Auto-create: client sent an unknown session ID (e.g. a random UUID).
   2 +			// Validate before touching the filesystem.
   3 +			if err := validateSessionID(sessionID); err != nil {
   5 +				return nil, err
   6 +			}
   7 +			// Create the session so the semantics match empty/default.
   8 +			isNewSession = true
   9 +			var err error
  10 +			s, err = m.createSessionLockedWithOpts(agentType, req.WorkspaceID, wsPath, false, sessionID)
  11 +			if err != nil {
  12 +				m.mu.Unlock()
  13 -			return nil, ErrSessionNotFound
  13 +				return nil, err
  14 +			}
  15 +			slog.InfoContext(ctx, "auto-created session for client-provided ID",
  16 +				"session_id", sessionID,
  17 +				"agent_type", agentType,
  18 +				"workspace_id", req.WorkspaceID,
  19 +			)
  20  		}
  21  		// Check if session is being deleted (subplan 01)
  22  		if s.deleting {
```

```folds
1-19:
    if !ok {
        validate(sessionID)
        create session with auto-create flag
        log("auto-created session for client-provided ID")
    }
```

## Deleted-only block (removed config dir creation)

All lines are old-side only. The `old` fold targets old-file line numbers.

```difft services/agentplat/sbox/sboxd/internal/session/manager.go chunks=7
1053  		m.metrics.RecordSessionsActive(len(m.sessions))
1054  		return nil, fmt.Errorf("create session streams dir: %w", err)
1055  	}
1056 -	// For per-session dirs, create the config directory
1056 -	if !isDefault {
1056 -		if err := os.MkdirAll(configDir, 0755); err != nil {
1056 -			delete(m.sessions, id)
1056 -			m.metrics.RecordSessionsActive(len(m.sessions))
1056 -			return nil, fmt.Errorf("create per-session config dir: %w", err)
1056 -		}
1056 -	}
1056 -
1057  	// Caller handles persistence (subplan 02)
1058  	return s, nil
```

```folds
old 1-8:
    // For per-session dirs, create the config directory
    if !isDefault { os.MkdirAll(configDir) }
```

## Added-only block (new validateSessionID function)

```difft services/agentplat/sbox/sboxd/internal/session/manager.go chunks=8
      	return s, nil
      }
      
   1 +// validateSessionID returns an error if id is unsafe for use in filesystem paths.
   2 +// Allowed characters are alphanumeric and hyphens (UUID-safe set).
   3 +func validateSessionID(id string) error {
   4 +	const maxLen = 128
   5 +	if len(id) == 0 {
   6 +		return fmt.Errorf("invalid session ID: must not be empty")
   7 +	}
   8 +	if len(id) > maxLen {
   9 +		return fmt.Errorf("invalid session ID: exceeds maximum length of %d", maxLen)
  10 +	}
  11 +	// First character must be alphanumeric — leading hyphens can confuse CLI
  12 +	// tools that interpret '-' as a flag prefix.
  13 +	first := rune(id[0])
  14 +	if !((first >= 'a' && first <= 'z') || (first >= 'A' && first <= 'Z') || (first >= '0' && first <= '9')) {
  15 +		return fmt.Errorf("invalid session ID: must start with an alphanumeric character")
  16 +	}
  17 +	for _, c := range id {
  18 +		if !((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '-') {
  19 +			return fmt.Errorf("invalid session ID: contains disallowed character %q", c)
  20 +		}
  21 +	}
  22 +	return nil
  23 +}
  24 +
  25  func (m *Manager) allocatePromptID(s *session) int {
  26  	id := s.info.NextPromptID
```

```folds
1-23:
    func validateSessionID(id) error {
        check length (0, >128)
        check first char is alphanumeric
        check all chars are [a-zA-Z0-9-]
    }
```
