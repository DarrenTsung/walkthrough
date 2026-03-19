# Walkthrough

Rust CLI that generates narrative walkthroughs of code changes, rendered as standalone HTML with side-by-side difftastic diffs, syntax highlighting, and interactive features. Also works as a general-purpose markdown renderer.

## Install

```bash
cargo install --path .
```

Requires [difftastic](https://github.com/Wilfred/difftastic) (`brew install difftastic`).

## Quick start

### 1. Install the CLI and skill

```bash
# Install the walkthrough binary
cargo install --path .

# Symlink the skill into your project so Claude Code can use it
ln -s /path/to/walkthrough/.claude/skills/walkthrough ~/.claude/skills/walkthrough
```

### 2. Generate a walkthrough with Claude Code

Open Claude Code in your repo and use the `/walkthrough` slash command:

```
/walkthrough HEAD~3..HEAD
```

Claude will:
1. Collect difft data for the changes
2. Write a narrative walkthrough with prose, diffs, and diagrams
3. Run adversarial reviewers to verify accuracy and completeness
4. Open the rendered HTML in your browser

Other examples:
```
/walkthrough                    # working tree changes
/walkthrough staged             # staged changes
/walkthrough abc123             # a specific commit
/walkthrough abc123..def456     # a range
```

### 3. Manual usage (without Claude Code)

```bash
walkthrough collect -- HEAD~3..HEAD
walkthrough render walkthrough.md -o walkthrough.html

# Or render plain markdown (no diff data needed)
walkthrough render doc.md --no-diff-data -o doc.html
```

## Commands

| Command | Description |
|---|---|
| `walkthrough collect -- <git-diff-args>` | Collect difft JSON for changed files |
| `walkthrough render <file.md> -o <file.html>` | Render markdown to HTML |
| `walkthrough verify <file.md>` | Check all diff chunks are referenced |
| `walkthrough publish <file.html>` | Publish to GitHub Pages |

## Markdown syntax

### Diff blocks

Reference collected diff data by file path and chunk indices:

````markdown
```difft src/foo.rs chunks=0,1,3
```

```difft src/bar.rs chunks=all
```

```difft src/baz.rs chunks=1 lines=164-200
```
````

### Source blocks

Show existing source code with syntax highlighting:

````markdown
```src path/to/file.rs:10-30
```

```src path/to/file.rs:10-30 old
```
````

Adding `old` shows the pre-change version of the file.

### Code folds

Collapse code ranges into clickable pseudocode summaries. Single-line:

````markdown
```folds
5-15: setup_test(mock_api)
```
````

Multi-line (leave first line empty, control indentation yourself):

````markdown
```folds
15-44:
    mock_api.expect_checkpoint(|bytes, activities| {
        assert(bytes.has(rounded_rect(10, 10)));
        connection.send(frame(10, 12));
        Err("Failed!")
    })
```
````

Pseudocode is syntax-highlighted in the same language as the file. Folds support browser find (Ctrl+F) and auto-expand when a match is found inside.

### Notes

Annotate specific lines with hover tooltips:

````markdown
```notes
1: This initializes the connection
5-8: Race condition window
```
````

### Frontmatter

Optional YAML metadata renders as a subtitle in the sticky header:

```markdown
---
pr: https://github.com/org/repo/pull/123
author: Jane Smith
---

# My Walkthrough
```

### Other features

- **Mermaid diagrams**: ` ```mermaid ` blocks are pre-rendered to inline SVG
- **Markdown tables**: standard pipe-delimited tables
- **Service badges**: `` `@sboxd` `` renders as a styled service name badge
- **Collapsible diff blocks**: auto-collapsed for deleted/generated files
- **Scroll-pinned diffs**: diff blocks capture scroll when reaching the viewport top
- **Table of contents**: auto-generated from headings, fixed in the left margin

## Build

```bash
cargo build
cargo test
cargo clippy
```

## Acknowledgements

Inspired by [Showboat](https://github.com/simonw/showboat) by Simon Willison.
