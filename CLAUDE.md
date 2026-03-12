# Walkthrough

Rust CLI that generates narrative walkthroughs of code changes with difftastic diffs, rendered as GitHub-style side-by-side HTML.

## Architecture

- `src/main.rs` - CLI entry point with three subcommands via clap
- `src/collect.rs` - Runs `difft --display json` via `GIT_EXTERNAL_DIFF` on each changed file, enriches the JSON with file contents and unified diff hunks, writes per-file JSON to an output directory
- `src/difft_json.rs` - Serde types for difftastic's JSON output (`DifftOutput`, `LineEntry`, `LineSide`, `ChangeSpan`, `DiffHunk`)
- `src/render.rs` - Parses walkthrough markdown, replaces `` ```difft `` code blocks with rendered HTML diff tables, converts the rest via pulldown-cmark
- `src/verify.rs` - Checks that every chunk in the collected data is referenced by at least one difft code block in the walkthrough

## Commands

```
# Collect difft data for changes between commits (args after -- are passed to git diff)
cargo run -- collect -- HEAD~3..HEAD

# Verify all chunks are covered in a walkthrough markdown file
cargo run -- verify walkthrough.md

# Render walkthrough markdown to HTML
cargo run -- render walkthrough.md -o walkthrough.html
```

Default data directory for all commands: `/tmp/walkthrough_data`

## Walkthrough markdown syntax

Difft code blocks reference collected data by file path and chunk indices:

````markdown
```difft src/foo.rs chunks=0,1,3
```

```difft src/bar.rs chunks=all
```
````

## External dependencies

- **difftastic** (`difft`) must be installed and on PATH. Used with `--display json --color never` and `DFT_UNSTABLE=yes`.

## Build and check

```
cargo build
cargo check
cargo clippy
```

## Testing rendered HTML with playwright-cli

Use the `playwright-cli` skill to visually verify rendered walkthrough HTML. Typical workflow:

```bash
# 1. Build and render a walkthrough
cargo run -- collect -- HEAD~3..HEAD
cargo run -- render walkthrough.md -o /tmp/walkthrough.html

# 2. Open the rendered HTML in a browser
playwright-cli open file:///tmp/walkthrough.html

# 3. Take a snapshot to inspect the page structure
playwright-cli snapshot

# 4. Screenshot the full page
playwright-cli screenshot /tmp/walkthrough-screenshot.png

# 5. Interact with the page (scroll, inspect elements, etc.)
playwright-cli eval "document.querySelectorAll('.diff-block').length"
playwright-cli click <ref>           # click an element from the snapshot
playwright-cli mousewheel 0 500      # scroll down

# 6. Close the browser
playwright-cli close
```

Key commands for testing:
- `playwright-cli open <url>` - open a browser (supports `file://` URLs)
- `playwright-cli snapshot` - capture page state as YAML, returns element refs for interaction
- `playwright-cli screenshot <path>` - save a PNG screenshot
- `playwright-cli eval <js>` - run JavaScript in the page (e.g. count diff blocks, check styles)
- `playwright-cli resize <width> <height>` - test responsive layout
- `playwright-cli -s=<name> open` - use named sessions to compare multiple renders side by side
