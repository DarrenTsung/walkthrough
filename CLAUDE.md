# Walkthrough

Rust CLI that generates narrative walkthroughs of code changes with side-by-side diffs, rendered as GitHub-style HTML.

## Architecture

- `src/main.rs` - CLI entry point with three subcommands via clap
- `src/collect.rs` - Runs `git diff -U0` to get unified diff hunks, reads file contents via `git cat-file`, generates chunks by positionally pairing old/new lines within each hunk, writes per-file JSON to an output directory
- `src/difft_json.rs` - Serde types for the collected JSON output (`DifftOutput`, `LineEntry`, `LineSide`, `ChangeSpan`, `DiffHunk`)
- `src/render.rs` - Parses walkthrough markdown, replaces `` ```diff `` code blocks with rendered HTML diff tables, converts the rest via pulldown-cmark. Also writes enriched markdown back to the input file with text diffs in the code block bodies.
- `src/verify.rs` - Checks that every chunk in the collected data is referenced by at least one diff code block in the walkthrough

## Commands

```
# Collect diff data for the current branch (auto-detects merge-base with origin/master or origin/main)
cargo run -- collect

# Collect with explicit diff range (only needed for non-branch cases)
cargo run -- collect -- HEAD~3..HEAD
cargo run -- collect -- --cached

# Verify all chunks are covered in a walkthrough markdown file
cargo run -- verify walkthrough.md

# Render walkthrough markdown to HTML
cargo run -- render walkthrough.md -o walkthrough.html
```

When on a feature branch, prefer `collect` with no args. It uses `origin/master...HEAD`
(three-dot syntax) to diff only the branch's changes against the merge-base, which is
reliable even when the local master ref is stale or the branch has been rebased.

Default data directory for all commands: `.walkthrough_data` (in the current directory)

## Walkthrough markdown syntax

Diff code blocks reference collected data by file path and chunk indices (`difft` is also accepted for backwards compatibility):

````markdown
```diff src/foo.rs chunks=0,1,3
```

```diff src/bar.rs chunks=all
```

```diff src/baz.rs chunks=1 lines=164-200
```
````

The optional `lines=START-END` parameter (1-based, inclusive, relative to the chunk) filters a chunk to only show a portion. Line 1 is the first changed line in the chunk. This lets you split a large chunk across multiple sections with interleaved prose.

### Code folds

A `folds` block placed immediately after a diff block collapses line ranges into clickable pseudocode summaries. Line numbers are 1-based, relative to the first line on the targeted side of the chunk:

````markdown
```diff src/foo.rs chunks=0
```

```folds
5-15: Set up test fixtures and mock data
20-30: Assert expected results
```
````

Each fold line can optionally specify which side of a side-by-side diff it targets:

- `5-15: ...` or `new 5-15: ...` targets new-file (rhs) line numbers (default)
- `old 5-15: ...` targets old-file (lhs) line numbers

Old-side folds are needed for deleted-only chunks where there are no new-file lines. In side-by-side diffs, the fold summary renders on the targeted side's columns with the other side left empty.

Collapsed folds show italic pseudocode text with a yellow background and left yellow border. Clicking expands to reveal the original code, with the yellow left border continuing along the expanded lines.

## External dependencies

- **mermaid-cli** (`mmdc`) for pre-rendering mermaid diagrams to inline SVG. Install with `npm install -g @mermaid-js/mermaid-cli`. Uses system Chrome via a puppeteer config. If not installed, mermaid blocks fall back to showing source code.

## Build and check

```
cargo build
cargo check
cargo clippy
```

## Rendering details

### Scroll-pinned diff blocks

Diff blocks have `max-height: 80vh` with `overflow: hidden` (no scrollbar). A JS script intercepts `wheel` events at the window level: when a diff block's top reaches near the viewport top, page scroll is captured and redirected to scroll the block internally. The page's `scrollY` is pinned via a `scroll` event listener to prevent trackpad momentum from pushing surrounding text off-screen. The sticky `.diff-header` stays pinned at the top of the block. When the diff content reaches its end, the pin releases and normal page scrolling resumes.

### Markdown enrichment

The render command writes back to the input markdown file, replacing each diff code block body with a unified-diff-style text representation (` ` context, `-` removed, `+` added). This uses the same chunk processing logic as the HTML renderer (context lines, consolidation). The enriched markdown is idempotent: re-running render produces identical HTML and re-populates the same text diffs.

### Expression-aware context expansion

Context lines normally show 3 lines before and after each chunk (`CONTEXT_LINES`). When a changed line sits inside a multi-line expression (e.g. a function call) whose opener would be cut off by the 3-line limit, the renderer expands context backward to include the enclosing expression's opener. Similarly, if changed lines contain unclosed openers, context expands forward.

The expansion uses bracket counting (`({[` vs `)}]`) and is scoped to the **nearest enclosing expression**: only the first bracket dip below 0 (scanning forward from the changed line) triggers expansion. This prevents expansion from bleeding into unrelated outer expressions. Capped at `MAX_EXPRESSION_CONTEXT` (8) extra lines, and if the cap is hit without balancing the brackets, the expansion is abandoned entirely (falls back to normal 3-line context).

### Syntax-highlighted code blocks

Fenced code blocks with a language tag (e.g. `` ```typescript ``, `` ```rust ``) are syntax-highlighted using arborium (tree-sitter). The language tag is mapped to a file extension for `arborium::detect_language`. Blocks without a language tag render as plain text.

### Line mapping

`new_to_old_line` and `old_to_new_line` map between old/new file lines using unified diff hunk boundaries. Used to compute context line correspondence and resolve one-sided chunks (e.g. added-only) to the correct old-file position.

## Fixture-based rendering tests

The `test_fixtures/` directory holds fixture data (JSON from `walkthrough collect`) that `cargo test` uses to verify the rendering pipeline. Each subdirectory contains `*.json` files.

### What the tests check

For every chunk in every fixture JSON, `fixture_rendering_matches` verifies:

1. **Row layout** matches `consolidate_chunk` + sort (same line pairings, same order)
2. **Old-side line numbers** are non-decreasing
3. **No duplicate line numbers** on either side
4. **Added-only rows** have no `hl-add` token highlights (full-row background is sufficient)
5. **Removed-only rows** have no `hl-del` token highlights
6. **Text rendering** changed-line count matches HTML rendering

### Adding a new fixture

```bash
# 1. Capture fixture data from a commit (or range)
mkdir -p test_fixtures/<commit>
cargo run -- collect -o test_fixtures/<commit> -- <commit>~1..<commit>

# 2. Remove internal files (keep SUMMARY.md for rendering)
rm -f test_fixtures/<commit>/.gitignore test_fixtures/<commit>/.meta.json

# 3. Run tests
cargo test fixture_rendering_matches

# 4. Render the fixture walkthrough (uses SUMMARY.md from collect)
walkthrough render test_fixtures/<commit>/SUMMARY.md \
  --data-dir test_fixtures/<commit>/ -o walkthrough-<commit>.html
```

Each fixture directory contains:
- `*.json` - collected diff JSON (from `walkthrough collect`)
- `SUMMARY.md` - walkthrough markdown referencing all chunks (renderable)

### When to add fixtures

Add a fixture when you find a rendering bug. The fixture captures the exact JSON that triggered the issue, ensuring the fix is regression-tested.

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
