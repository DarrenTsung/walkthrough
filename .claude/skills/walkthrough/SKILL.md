---
name: walkthrough
description: "Generate a narrative walkthrough of code changes with difftastic diffs and verified complete coverage"
allowed-tools: "Bash, Read, Write, Glob, Grep"
argument-hint: "[diff-source] [--output path]"
---

# Walkthrough Generator

Generate a narrative walkthrough of code changes, rendered as an HTML document with
side-by-side difftastic diffs. Every change chunk must be referenced in the walkthrough,
ensuring complete coverage.

## Arguments

Parse `$ARGUMENTS` to determine the diff source and output path:

- **No args**: working tree changes (`git diff`)
- `staged` or `--cached`: staged changes (`git diff --cached`)
- A commit SHA (e.g. `abc123`): that commit (`git diff abc123~1 abc123`)
- A range `A..B`: that range (`git diff A..B`)
- `HEAD~N`: last N commits (`git diff HEAD~N HEAD`)
- `--output <path>`: output path for the walkthrough markdown (default: `walkthrough-{timestamp}.md`)

Derive two values from this:
- `DIFF_ARGS`: the arguments to pass after `git diff` (e.g. `--cached`, `HEAD~1 HEAD`, etc.)
- `OUTPUT_PATH`: where to write the walkthrough markdown

## Step 1: Check prerequisites

Run `which difft`. If not found, ask the user if they want to install via `brew install difftastic`.
If they decline, stop with: "difft is required for walkthrough generation."

Verify the version supports JSON output (0.67.0+): `difft --version`.

Run `which walkthrough`. If not found, install it:
```bash
cargo install --path ~/Documents/walkthrough
```

## Step 2: Collect diffs

Run the collect command:
```bash
walkthrough collect -o .walkthrough_data/ -- $DIFF_ARGS
```

This produces JSON files for the renderer and a `SUMMARY.md` with human-readable diffs.
Read `.walkthrough_data/SUMMARY.md` to understand all the changes. It lists each file
with its chunks as text diffs (` ` context, `-` removed, `+` added), including chunk
indices and new-file line ranges you can reference in the walkthrough.

**Do not read the JSON files.** They contain machine-readable token spans meant for the
renderer.

## Step 3: Plan the narrative

Using the summary, decide how to organize the walkthrough. Group by narrative theme,
not by file. Consider:

- **Core logic changes**: chunks with substantive behavior changes. Lead with these.
- **New modules/files**: introduce new concepts early, before their usage sites.
- **Refactoring/renames**: mechanical changes grouped together.
- **Import/config updates**: boilerplate changes, put these last.
- **Test changes**: group test updates near the code they test, or in a separate section.

Plan the section titles and which (file, chunk) pairs go in each section. If a chunk
contains multiple logical changes, use `lines=START-END` to split it.

## Step 4: Write the walkthrough narrative

Rewrite the markdown file at `OUTPUT_PATH` with the planned structure. The difft code block
bodies will be repopulated by the next render, so don't worry about their contents.

````markdown
# <concise title describing the change>

<1-2 sentence overview of what this change does and why.>

## 1. <Section title>

<Narrative explaining what this group of changes does and why.>

```difft path/to/file.rs chunks=0,1
```

## 2. <Next section>

...
````

Rules for writing the markdown:

1. **Every code block** that references diffs uses the info string format:
   `difft <file-path> chunks=<spec>` where spec is comma-separated indices or `all`.
   Optionally add `lines=START-END` (1-based, inclusive) to show only a portion of the
   chunk. This is useful for splitting a large chunk (e.g. two new functions in one chunk)
   across multiple sections with prose in between.

2. **The block body** will be populated by the render step with a unified-diff-style text
   representation (` ` context, `-` removed, `+` added). You do not need to reconstruct
   code manually from the JSON.

3. **Group by narrative**, not by file. A single section can reference chunks from multiple
   files. A single file's chunks can appear across multiple sections.

4. **The same file can appear multiple times** in different sections with different chunk
   selections.

5. Use `chunks=all` for new files, deleted files, or files with only one or two chunks.

6. **Narrative style**: explain WHY the change was made, not just what changed. Be concise.
   Use present tense ("This extracts..." not "This extracted...").

7. **Explain terms inline, not upfront.** Do not create a glossary or "key terms" section.
   Instead, define domain-specific terms, service names, and jargon the first time they
   naturally appear in the narrative. Anchor explanations with concrete use-cases or examples
   when possible (e.g. "a **sandbox** is an isolated container where user code runs; each
   Figma file gets its own"). Assume the reader may be unfamiliar with the codebase.

8. **Interleave prose and diffs.** When a section has multiple diffs, place explanatory text
   between them rather than grouping all prose at the top and all diffs at the bottom. Each
   diff block should be immediately preceded by the prose that explains it. For example, if
   a section covers a feature flag addition and a new dependency, explain the flag, show the
   flag diff, explain the dependency, show the dependency diff.

## Step 5: Render and enrich

Run the render command:
```bash
walkthrough render "$OUTPUT_PATH" --data-dir .walkthrough_data/ -o "${OUTPUT_PATH%.md}.html"
```

This does three things:
1. Produces the HTML file with side-by-side diffs
2. Writes text diff representations back into each difft code block in the markdown file
3. Verifies coverage and adds a badge below the title showing whether all chunks are covered

If the render output reports uncovered chunks, add sections referencing them and re-render.

## Step 6: Review and revise the narrative

Re-read the markdown file (`OUTPUT_PATH`). The difft code blocks now contain the actual text
diffs that correspond to what the HTML renders. For each section:

1. Read the diff in the code block carefully
2. Check that the surrounding narrative accurately describes what the diff shows
3. Look for mismatches: narrative claims that don't match the code, missing context about
   why a change matters, or sections that would be clearer in a different order

If revisions are needed, edit the narrative text (not the code block bodies) and re-run the
render command. The code block bodies will be repopulated, so edits there are overwritten.
Repeat until the narrative is coherent.

## Step 7: Present

Open the HTML file:
```bash
open "${OUTPUT_PATH%.md}.html"
```

Print a summary:
- Number of files covered
- Number of chunks covered
- Output file path
