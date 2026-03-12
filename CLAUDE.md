# Walkthrough

Rust CLI that generates narrative walkthroughs of code changes with difftastic diffs, rendered as GitHub-style side-by-side HTML.

## Architecture

- `src/main.rs` - CLI entry point with three subcommands via clap
- `src/collect.rs` - Runs `difft --display json` via `GIT_EXTERNAL_DIFF` on each changed file, enriches the JSON with file contents and unified diff hunks, writes per-file JSON to an output directory
- `src/difft_json.rs` - Serde types for difftastic's JSON output (`DifftOutput`, `LineEntry`, `LineSide`, `ChangeSpan`, `DiffHunk`)
- `src/render.rs` - Parses walkthrough markdown, replaces `` ```difft `` code blocks with rendered HTML diff tables, converts the rest via pulldown-cmark. Also writes enriched markdown back to the input file with text diffs in the code block bodies.
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

Default data directory for all commands: `.walkthrough_data` (in the current directory)

## Walkthrough markdown syntax

Difft code blocks reference collected data by file path and chunk indices:

````markdown
```difft src/foo.rs chunks=0,1,3
```

```difft src/bar.rs chunks=all
```

```difft src/baz.rs chunks=1 lines=164-200
```
````

The optional `lines=START-END` parameter (1-based, inclusive, relative to the chunk) filters a chunk to only show a portion. Line 1 is the first changed line in the chunk. This lets you split a large chunk across multiple sections with interleaved prose.

## External dependencies

- **difftastic** (`difft`) must be installed and on PATH. Used with `--display json --color never` and `DFT_UNSTABLE=yes`.

## Build and check

```
cargo build
cargo check
cargo clippy
```

## Rendering details

### Highlight span merging

Adjacent difft highlight spans that are directly adjacent or separated only by whitespace are merged into single `<span>` regions (`merge_whitespace_spans` in render.rs). Without this, difft's structural matching produces separate spans for each token (e.g. `meta:`, `{`, `level`, `},`), leaving unhighlighted gaps between them.

### Hunk gap filling

Difft's structural matching can miss lines that `git diff` considers changed. The renderer cross-references unified diff hunks with difft's output to find these gaps and renders them as extra removed/added/paired lines. Hunk analysis runs before context line computation to ensure context lines don't overlap with changed lines.

### Scroll-pinned diff blocks

Diff blocks have `max-height: 80vh` with `overflow: hidden` (no scrollbar). A JS script intercepts `wheel` events at the window level: when a diff block's top reaches near the viewport top, page scroll is captured and redirected to scroll the block internally. The page's `scrollY` is pinned via a `scroll` event listener to prevent trackpad momentum from pushing surrounding text off-screen. The sticky `.diff-header` stays pinned at the top of the block. When the diff content reaches its end, the pin releases and normal page scrolling resumes.

### Markdown enrichment

The render command writes back to the input markdown file, replacing each difft code block body with a unified-diff-style text representation (` ` context, `-` removed, `+` added). This uses the same chunk processing logic as the HTML renderer (context lines, hunk gap filling, consolidation). The enriched markdown is idempotent: re-running render produces identical HTML and re-populates the same text diffs. This enables an LLM workflow where the narrative is written first, then refined after seeing the actual diffs inline.

### Line mapping

`new_to_old_line` and `old_to_new_line` map between old/new file lines using unified diff hunk boundaries. Used to compute context line correspondence and resolve one-sided chunks (e.g. added-only) to the correct old-file position.

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
