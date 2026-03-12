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

Use the `playwright-cli` skill to visually verify rendered walkthrough HTML.

**Important:** `playwright-cli open` does not support `file://` URLs. Serve files over HTTP instead.

### Typical workflow

```bash
# 1. Build and render a walkthrough
cargo run -- collect -- HEAD~3..HEAD
cargo run -- render walkthrough.md -o /tmp/walkthrough.html

# 2. Serve the HTML over HTTP (playwright blocks file:// URLs)
python3 -m http.server 8765 --directory /tmp &

# 3. Open in browser and navigate
playwright-cli open http://localhost:8765/walkthrough.html

# 4. Take a snapshot (required before screenshot or element interaction)
playwright-cli snapshot

# 5. Screenshot the full page (use --filename, not a positional arg)
playwright-cli screenshot --full-page --filename /tmp/walkthrough-full.png

# 6. Screenshot a specific element by ref from the snapshot
playwright-cli screenshot e7 --filename /tmp/first-diff.png

# 7. Run JS to query the DOM (expression must be a single-expression string)
playwright-cli eval "document.querySelectorAll('.diff-block').length"
playwright-cli eval "document.querySelector('h1').textContent"

# 8. Resize viewport for responsive testing
playwright-cli resize 1200 900

# 9. Close browser and stop server
playwright-cli close
kill %1
```

### Key gotchas

- **Snapshot before screenshot:** `playwright-cli screenshot` requires a recent snapshot. Always run `snapshot` first, and again after `resize` or navigation.
- **screenshot syntax:** Use `--filename <path>` for the output path. A bare path arg is interpreted as an element ref. Use `--full-page` for the entire scrollable page.
- **eval limitations:** Only single JS expressions work. Array spread/map expressions like `[...nodeList].map(...)` fail. Use simpler queries or string concatenation in the eval.
- **No `file://`:** The browser blocks `file://` protocol. Use a local HTTP server.
