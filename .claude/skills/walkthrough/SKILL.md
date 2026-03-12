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
- `--output <path>`: output path for the walkthrough markdown (default: `/tmp/walkthrough-{timestamp}.md`)

Derive two values from this:
- `DIFF_ARGS`: the arguments to pass after `git diff` (e.g. `--cached`, `HEAD~1 HEAD`, etc.)
- `OUTPUT_PATH`: where to write the walkthrough markdown

## Step 1: Check difft

Run `which difft`. If not found, ask the user if they want to install via `brew install difftastic`.
If they decline, stop with: "difft is required for walkthrough generation."

Verify the version supports JSON output (0.67.0+): `difft --version`.

## Step 2: Collect difft JSON

The walkthrough CLI is at `~/Documents/walkthrough`. Build and run the collect command:
```bash
cargo run --manifest-path ~/Documents/walkthrough/Cargo.toml -- collect -o /tmp/walkthrough_data/ -- $DIFF_ARGS
```

This produces one JSON file per changed file in `/tmp/walkthrough_data/`.

After collecting, read each JSON file to understand the chunks. For each file, note:
- How many chunks it has
- What kind of changes each chunk contains (look at the change spans)
- Whether the file is added, deleted, or modified

## Step 3: Plan the narrative

Analyze the chunks and decide how to organize the walkthrough. Group by narrative theme,
not by file. Consider:

- **Core logic changes**: chunks with substantive behavior changes. Lead with these.
- **New modules/files**: introduce new concepts early, before their usage sites.
- **Refactoring/renames**: mechanical changes grouped together.
- **Import/config updates**: boilerplate changes, put these last.
- **Test changes**: group test updates near the code they test, or in a separate section.

Plan the section titles and which (file, chunk) pairs go in each section.

## Step 4: Write the walkthrough markdown

Write the markdown file at `OUTPUT_PATH`. Use this structure:

````markdown
# Walkthrough: <concise title describing the change>

<1-2 sentence overview of what this change does and why.>

## 1. <Section title>

<Narrative explaining what this group of changes does and why.>

```difft path/to/file.rs chunks=0,1
<paste the actual code from those chunks here, so you stay grounded in what you're narrating>
```

## 2. <Next section>

...
````

Rules for writing the markdown:

1. **Every code block** that references diffs uses the info string format:
   `difft <file-path> chunks=<spec>` where spec is comma-separated indices or `all`.

2. **The block body** must contain the actual changed code from those chunks. Reconstruct
   readable code from the change spans in the JSON. This is for YOUR context while narrating.
   The `render` step replaces it with properly formatted side-by-side HTML.

3. **Group by narrative**, not by file. A single section can reference chunks from multiple
   files. A single file's chunks can appear across multiple sections.

4. **The same file can appear multiple times** in different sections with different chunk
   selections.

5. Use `chunks=all` for new files, deleted files, or files with only one or two chunks.

6. **Narrative style**: explain WHY the change was made, not just what changed. Be concise.
   Use present tense ("This extracts..." not "This extracted...").

## Step 5: Verify coverage

Run the verify command:
```bash
cargo run --manifest-path ~/Documents/walkthrough/Cargo.toml -- verify "$OUTPUT_PATH" --data-dir /tmp/walkthrough_data/
```

This checks that every chunk in every JSON file is referenced by at least one
`difft <file> chunks=...` annotation in the walkthrough.

If verification fails, it prints the uncovered chunks. Add sections referencing them and
re-verify. Repeat up to 3 times.

## Step 6: Render HTML

Run the render command:
```bash
cargo run --manifest-path ~/Documents/walkthrough/Cargo.toml -- render "$OUTPUT_PATH" --data-dir /tmp/walkthrough_data/ -o "${OUTPUT_PATH%.md}.html"
```

## Step 7: Present

Open the HTML file:
```bash
open "${OUTPUT_PATH%.md}.html"
```

Print a summary:
- Number of files covered
- Number of chunks covered
- Output file path
