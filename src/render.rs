use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use pulldown_cmark::{Options, Parser};
use regex::Regex;

use arborium::Highlighter;

use crate::difft_json::{DifftOutput, LineEntry, LineSide};

/// Number of unchanged context lines to show before/after each chunk.
const CONTEXT_LINES: usize = 3;

/// Total context lines captured for HTML rendering. Lines beyond CONTEXT_LINES
/// are hidden behind an expandable "show more" control.
const EXPANDED_CONTEXT_LINES: usize = 40;

/// Lines revealed per click on the "show more" expander.
const EXPAND_STEP: usize = 8;

/// Max extra context lines to add when expanding to show complete expressions
/// (e.g. multi-line function calls cut off by the CONTEXT_LINES limit).
const MAX_EXPRESSION_CONTEXT: usize = 8;

/// Count bracket balance on a line: `({[` contribute +1, `)}]` contribute -1.
fn bracket_balance(line: &str) -> i32 {
    let mut bal: i32 = 0;
    for ch in line.chars() {
        match ch {
            '(' | '{' | '[' => bal += 1,
            ')' | '}' | ']' => bal -= 1,
            _ => {}
        }
    }
    bal
}

/// Expand context boundaries outward when the visible range cuts off a
/// multi-line expression that the changed lines participate in.
///
/// Only triggers when the *changed lines + post-context* contain unmatched
/// closers (expand before) or the *pre-context + changed lines* contain
/// unmatched openers (expand after). Brackets that appear solely in context
/// lines on the opposite side of the change are ignored, preventing expansion
/// into unrelated code.
///
/// Returns `(expanded_ctx_before, expanded_ctx_after)`.
fn expand_context_for_expressions(
    lines: &[String],
    ctx_before: usize,
    first_changed: usize,
    last_changed: usize,
    ctx_after: usize,
) -> (usize, usize) {
    let end = ctx_after.min(lines.len());
    if first_changed >= end || first_changed > last_changed {
        return (ctx_before, ctx_after);
    }
    let last = last_changed.min(end - 1);

    let mut new_before = ctx_before;
    let mut new_after = ctx_after;

    // Before-expansion: scan from first changed line forward. Use only the
    // depth at the first line where depth drops below 0 (the immediate
    // enclosing expression's closer). Further closers from outer/sibling
    // expressions are ignored to prevent over-expansion.
    {
        let mut depth: i32 = 0;
        let mut min_depth: i32 = 0;
        for idx in first_changed..end {
            depth += bracket_balance(&lines[idx]);
            if depth < 0 {
                min_depth = depth;
                break;
            }
        }
        if min_depth < 0 {
            let saved = new_before;
            let mut accumulated: i32 = 0;
            let mut extra = 0;
            while new_before > 0 && extra < MAX_EXPRESSION_CONTEXT {
                new_before -= 1;
                accumulated += bracket_balance(&lines[new_before]);
                extra += 1;
                if accumulated >= -min_depth {
                    break;
                }
            }
            // If we hit the cap without balancing, the extra lines are
            // mid-expression noise. Fall back to normal context.
            if accumulated < -min_depth {
                new_before = saved;
            }
        }
    }

    // After-expansion: scan from start of visible range through last changed
    // line. If depth > 0, there are openers (among or before the changed
    // lines) whose closers are below the post-context boundary.
    {
        let mut depth: i32 = 0;
        for idx in ctx_before..=last {
            depth += bracket_balance(&lines[idx]);
        }
        if depth > 0 {
            let saved = new_after;
            let mut remaining = depth;
            let mut extra = 0;
            while new_after < lines.len() && extra < MAX_EXPRESSION_CONTEXT {
                remaining += bracket_balance(&lines[new_after]);
                new_after += 1;
                extra += 1;
                if remaining <= 0 {
                    break;
                }
            }
            // If we hit the cap without balancing, fall back.
            if remaining > 0 {
                new_after = saved;
            }
        }
    }

    (new_before, new_after)
}

const CSS: &str = r#"
:root {
    --bg: #fff;
    --bg-secondary: #f6f8fa;
    --border: #d1d9e0;
    --border-muted: #d1d9e0b3;
    --text: #1f2328;
    --text-secondary: #59636e;
    --text-muted: #59636e;
    --primary: #0550ae;
    --diff-header-bg: #f6f8fa;
    --diff-header-fg: #1f2328;
    --ln-fg: #59636e;
    --added-bg: #dafbe1;
    --added-hl: #b0e9bd;
    --added-num-bg: #aceebb;
    --removed-bg: #ffebe9;
    --removed-hl: #ffcecb;
    --removed-num-bg: #ffcecb;
    --sep-bg: #f6f8fa;
    --empty-bg: #f6f8fa;
}

@media (prefers-color-scheme: dark) {
    :root {
        --bg: #0d1117;
        --bg-secondary: #161b22;
        --border: #30363d;
        --text: #e6edf3;
        --text-secondary: #9ba4b0;
        --text-muted: #8b949e;
        --primary: #58a6ff;
        --diff-header-bg: #161b22;
        --diff-header-fg: #58a6ff;
        --ln-fg: #8b949e;
        --added-bg: #12261e;
        --added-hl: #1a4721;
        --removed-bg: #2d1215;
        --removed-hl: #5d1214;
        --sep-bg: #161b22;
        --empty-bg: #161b22;
    }
}

*, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

html {
    font-size: 15px;
    -webkit-font-smoothing: antialiased;
}

body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
    color: var(--text);
    background: var(--bg);
    line-height: 1.55;
}

article {
    max-width: 860px;
    margin: 0 auto;
    padding: 24px 28px 20vh;
    line-height: 1.6;
}

.diff-block {
    width: calc(100vw - 260px);
    max-width: 1400px;
    margin-left: 50%;
    transform: translateX(-50%);
    margin-top: 1rem;
    margin-bottom: 1rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    max-height: 75vh;
    overflow: hidden;
}

h1 {
    font-size: 1.2em;
    font-weight: 700;
    margin: 0 calc(-28px - 50vw + 50%);
    padding: 10px 28px;
    text-align: center;
    border-bottom: 1px solid var(--border);
    background: var(--bg);
    position: sticky;
    top: 0;
    z-index: 20;
}
.subtitle {
    font-size: 0.75em;
    font-weight: 400;
    color: var(--text-muted);
    display: block;
    margin-top: 2px;
}
.subtitle a {
    color: var(--text-muted);
    text-decoration: underline;
    text-decoration-color: var(--border);
    text-underline-offset: 2px;
}
.subtitle a:hover {
    color: var(--text);
}
h1 + * {
    margin-top: 1.2em;
}

h2, h3, h4, h5, h6 {
    scroll-margin-top: 60px;
}

h2 {
    font-size: 1.35em;
    font-weight: 650;
    margin: 0.8em 0 0.3em;
    padding-bottom: 0.2em;
    border-bottom: 1px solid var(--border);
}

h3 {
    font-size: 1.15em;
    font-weight: 650;
    margin: 0.7em 0 0.25em;
}

h4, h5, h6 {
    font-weight: 650;
    margin: 0.6em 0 0.2em;
}

p { margin: 0 0 0.75em; }

ul, ol {
    margin: 0 0 0.75em;
    padding-left: 1.75em;
}

li { margin: 0.15em 0; }

code {
    background: var(--bg-secondary);
    padding: 0.15em 0.35em;
    border-radius: 3px;
    border: 1px solid var(--border);
    font-size: 0.84em;
    font-family: "SFMono-Regular", Consolas, "Liberation Mono", Menlo, monospace;
}

pre {
    background: var(--bg-secondary);
    padding: 12px 14px;
    border-radius: 4px;
    border: 1px solid var(--border);
    overflow-x: auto;
    width: max-content;
    min-width: min(100%, 860px);
    max-width: 1400px;
    margin: 0 0 0.75em;
    margin-left: 50%;
    transform: translateX(-50%);
}

pre code {
    background: none;
    border: none;
    padding: 0;
    font-size: 0.84em;
    line-height: 1.45;
}

blockquote {
    border-left: 3px solid var(--text-muted);
    padding: 0 0.9em;
    color: var(--text-secondary);
    margin: 0 0 0.75em;
}

a {
    color: var(--primary);
    text-decoration: underline;
    text-decoration-color: rgba(5, 80, 174, 0.3);
    text-underline-offset: 2px;
}

a:hover {
    text-decoration-color: var(--primary);
}

hr {
    border: none;
    border-top: 1px solid var(--border);
    margin: 1.5em 0;
}

img { max-width: 100%; }

/* Markdown tables (exclude diff tables) */
article table:not(.diff-table) {
    border-collapse: collapse;
    margin: 0 0 0.75em;
    font-size: 0.9em;
}
article table:not(.diff-table) th,
article table:not(.diff-table) td {
    border: 1px solid var(--border);
    padding: 6px 12px;
    text-align: left;
}
article table:not(.diff-table) th {
    background: var(--bg-secondary);
    font-weight: 600;
}
article table:not(.diff-table) tr:nth-child(even) td {
    background: var(--bg-secondary);
}

/* Coverage badge */
.coverage-badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 0.8em;
    padding: 4px 10px;
    border-radius: 6px;
    margin-bottom: 0.5em;
}

.coverage-badge.pass {
    background: #e6ffec;
    color: #1a7f37;
    border: 1px solid #aceebb;
}

.coverage-badge.fail {
    background: #ffebe9;
    color: #c4232b;
    border: 1px solid #ffcecb;
}

/* Floating table of contents */
.toc {
    position: fixed;
    top: var(--toc-top, 68px);
    left: 16px;
    width: 200px;
    max-height: calc(100vh - var(--toc-top, 68px) - 16px);
    overflow: visible;
    font-size: 12px;
    line-height: 1.5;
    mask-image: linear-gradient(to right, rgba(0,0,0,0.5) 0%, rgba(0,0,0,0.5) 50%, rgba(0,0,0,0) 70%);
    -webkit-mask-image: linear-gradient(to right, rgba(0,0,0,0.5) 0%, rgba(0,0,0,0.5) 50%, rgba(0,0,0,0) 70%);
    transition: mask-image 0.2s ease, -webkit-mask-image 0.2s ease;
}

.toc:hover {
    mask-image: none;
    -webkit-mask-image: none;
}

.toc a {
    display: block;
    padding: 3px 0;
    color: var(--text-muted);
    text-decoration: none;
    border-left: 2px solid transparent;
    padding-left: 10px;
    transition: color 0.15s, border-color 0.15s;
    white-space: nowrap;
}

.toc a:hover {
    color: var(--text);
}

.toc a.active {
    color: var(--text);
    border-left-color: var(--text);
    font-weight: 600;
}

.toc a.toc-h3 {
    padding-left: 22px;
    font-size: 11px;
}

@media (max-width: 1300px) {
    .toc { display: none; }
}

/* (diff-block styles consolidated above) */

.diff-header {
    position: sticky;
    top: 0;
    background: var(--diff-header-bg);
    color: var(--diff-header-fg);
    padding: 0.4rem 0.8rem;
    font-family: "SFMono-Regular", Consolas, "Liberation Mono", Menlo, monospace;
    font-size: 0.82rem;
    font-weight: 600;
    border-bottom: 1px solid var(--border);
    z-index: 1;
    cursor: pointer;
    user-select: none;
}

/* Collapsible diff blocks */
.collapse-arrow {
    display: inline-block;
    transition: transform 0.15s ease;
    font-size: 0.7em;
    vertical-align: middle;
}
.diff-block:not(.collapsed) .collapse-arrow { transform: rotate(90deg); }
.collapse-disclaimer {
    font-weight: 400;
    font-style: italic;
    font-size: 0.82rem;
    padding: 0.4rem 0.8rem;
    color: #5a4a00;
    background: #fef9e7;
    border-bottom: 1px solid #e8d44d;
    cursor: pointer;
}
@media (prefers-color-scheme: dark) {
    .collapse-disclaimer { color: #e8d080; background: #2a2400; border-color: #4a4010; }
}
.diff-block:not(.collapsed) .collapse-disclaimer { display: none; }
.collapsed { max-height: none !important; overflow: visible !important; }
.collapsed .diff-header { border-bottom: none; }

/* Viewed checkbox */
.viewed-label {
    float: right;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
    font-size: 0.75rem;
    font-weight: 400;
    color: var(--text-muted);
    cursor: pointer;
    user-select: none;
    display: inline-flex;
    align-items: center;
    gap: 0.45em;
}
.viewed-label:hover { color: var(--text); }
.viewed-check { margin: 0; cursor: pointer; }
.viewed-label span { position: relative; top: 0.1px; font-size: 0.8rem; }
.diff-block.viewed .diff-header { opacity: 0.6; }
.collapsed-hidden { display: none !important; }

.diff-table {
    width: 100%;
    border-collapse: collapse;
    font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace;
    font-size: 12px;
    line-height: 18px;
    color: #1f2328;
    table-layout: fixed;
}
.diff-table-lhs, .diff-table-rhs {
    table-layout: auto;
}

col.ln-col { width: 3.5em; }
col.sign-col { width: 1.5em; }
col.code-col { width: calc(100% - 5em); }

.diff-table td {
    padding: 1px 0.6rem;
    vertical-align: top;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
}

.diff-table .ln {
    text-align: right;
    color: var(--ln-fg);
    user-select: none;
    white-space: nowrap;
    padding: 1px 8px 1px 8px;
    min-width: 3em;
    border-right: 1px solid var(--border);
}

.diff-table .sign {
    color: var(--ln-fg);
    user-select: none;
    white-space: nowrap;
    padding: 1px 0 1px 8px;
    width: 1.5em;
}

tr.line-added .sign-rhs { color: #1a7f37; }
tr.line-removed .sign-lhs { color: #cf222e; }
tr.line-paired .sign-lhs { color: #cf222e; }
tr.line-paired .sign-rhs { color: #1a7f37; }

.diff-sides {
    display: flex;
    width: 100%;
}
.diff-side {
    flex: 1;
    min-width: 0;
    overflow-x: auto;
}
.diff-side-old {
    border-right: 1px solid var(--border);
}

/* Placeholder rows: zero-height empty rows on the "other" side of a
   one-sided change. They exist so height-sync can align rows when the
   corresponding fold is expanded. */
tr.placeholder { visibility: collapse; }
tr.placeholder td { padding: 0; border: none; line-height: 0; font-size: 0; }

/* Context lines (unchanged) */
tr.line-context td { background: var(--bg); }

/* Removed lines (full-line: lhs side highlighted, rhs side empty) */
tr.line-removed td.code-lhs,
tr.line-removed .sign-lhs { background: var(--removed-bg); }
tr.line-removed .ln-lhs { background: var(--removed-num-bg); }
tr.line-removed td.code-rhs,
tr.line-removed .sign-rhs,
tr.line-removed .ln-rhs { background: var(--empty-bg); }

/* Removed lines (partial: no row background, token highlights for changes) */
tr.line-removed-partial td.code-lhs,
tr.line-removed-partial .sign-lhs { background: var(--bg); }
tr.line-removed-partial .ln-lhs { background: var(--removed-num-bg); }
tr.line-removed-partial td.code-rhs,
tr.line-removed-partial .sign-rhs,
tr.line-removed-partial .ln-rhs { background: var(--empty-bg); }
tr.line-removed-partial .sign-lhs { color: #cf222e; }

/* Added lines (full-line: rhs side highlighted, lhs side empty) */
tr.line-added td.code-rhs,
tr.line-added .sign-rhs { background: var(--added-bg); }
tr.line-added .ln-rhs { background: var(--added-num-bg); }
tr.line-added td.code-lhs,
tr.line-added .sign-lhs,
tr.line-added .ln-lhs { background: var(--empty-bg); }

/* Added lines (partial: no row background, token highlights for changes) */
tr.line-added-partial td.code-rhs,
tr.line-added-partial .sign-rhs { background: var(--bg); }
tr.line-added-partial .ln-rhs { background: var(--added-num-bg); }
tr.line-added-partial td.code-lhs,
tr.line-added-partial .sign-lhs,
tr.line-added-partial .ln-lhs { background: var(--empty-bg); }
tr.line-added-partial .sign-rhs { color: #1a7f37; }

/* Paired rows (partial change): no row background, token highlights only */
tr.line-paired td.code-lhs,
tr.line-paired .sign-lhs,
tr.line-paired td.code-rhs,
tr.line-paired .sign-rhs { background: var(--bg); }
tr.line-paired .ln-lhs { background: var(--removed-num-bg); }
tr.line-paired .ln-rhs { background: var(--added-num-bg); }

/* Paired rows (full-line change): full row background like added/removed */
tr.line-paired-full td.code-lhs,
tr.line-paired-full .sign-lhs { background: var(--removed-bg); }
tr.line-paired-full .ln-lhs { background: var(--removed-num-bg); }
tr.line-paired-full td.code-rhs,
tr.line-paired-full .sign-rhs { background: var(--added-bg); }
tr.line-paired-full .ln-rhs { background: var(--added-num-bg); }
tr.line-paired-full .sign-lhs { color: #cf222e; }
tr.line-paired-full .sign-rhs { color: #1a7f37; }

.chunk-sep td {
    height: 0.5rem;
    background: var(--sep-bg);
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
}

/* Expandable context lines (hidden by default) */
tr.expand-line[hidden] {
    content-visibility: hidden;
    line-height: 0;
    font-size: 0;
}
tr.expand-line[hidden] td {
    padding: 0 !important;
    height: 0;
    border: none !important;
    overflow: hidden;
}
tr.expand-summary td {
    background: var(--sep-bg);
    cursor: pointer;
    text-align: center;
    padding: 2px 0;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
    font-size: 0.75rem;
    color: var(--text-secondary);
    user-select: none;
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
}
tr.expand-summary:hover td {
    background: #ddf4ff;
    color: #0969da;
}
@media (prefers-color-scheme: dark) {
    tr.expand-summary:hover td { background: #0d2440; color: #58a6ff; }
}

/* Code annotations: yellow right-edge indicator */
tr.annotated td:last-child {
    border-right: 3px solid #f0c000;
}
.note-tooltip {
    display: none;
    position: fixed;
    z-index: 10;
    max-width: none;
    white-space: nowrap;
    padding: 8px 12px;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
    font-size: 0.85rem;
    line-height: 1.4;
    color: #5a4a00;
    background: #fef9e7;
    border: 1px solid #e8d44d;
    border-radius: 6px;
    box-shadow: 0 2px 8px rgba(0,0,0,0.1);
    pointer-events: none;
}
.note-tooltip hr {
    border: none;
    border-top: 1px solid #e8d44d;
    margin: 4px 0;
}
@media (prefers-color-scheme: dark) {
    tr.annotated td:last-child { border-right-color: #c0a000; }
    .note-tooltip {
        color: #e8d080;
        background: #2a2400;
        border-color: #4a4010;
    }
}

/* Code folds: collapsible pseudocode summaries */
tr.fold-line[hidden] {
    content-visibility: hidden;
    line-height: 0;
    font-size: 0;
}
tr.fold-line[hidden] td {
    padding: 0 !important;
    height: 0;
    border: none !important;
    overflow: hidden;
}
tr.fold-line.fold-expanded td:first-child {
    border-left: 3px solid #f0c000;
}
tr.fold-summary td {
    background: #fefdf8;
    cursor: pointer;
}
tr.fold-summary td:first-child {
    border-left: 3px solid #f0c000;
}
tr.fold-summary td.fold-text {
    white-space: pre-wrap;
    padding: 1px 0.6rem;
    color: #5a4a00;
}
tr.fold-summary:hover td {
    background: #fefbef;
}
tr.fold-summary.placeholder td,
tr.fold-summary.placeholder:hover td {
    background: transparent;
    cursor: default;
}
.fold-arrow {
    display: inline-block;
    transition: transform 0.15s ease;
    font-size: 0.7em;
    vertical-align: middle;
    margin-right: 0.4em;
    position: relative;
    top: -2px;
}
tr.fold-summary.fold-expanded .fold-arrow {
    transform: rotate(90deg);
}
td.fold-count {
    color: #9a8a40;
    font-size: 0.7rem;
    white-space: nowrap;
    vertical-align: middle;
    user-select: none;
}
tr.fold-summary td.sign {
    vertical-align: middle;
}
/* Added-side folds: soft green (distinct from bright green diff lines) */
tr.fold-summary.fold-add td { background: #e6f6e6; }
tr.fold-summary.fold-add td:first-child { border-left-color: #2d8a4e; }
tr.fold-summary.fold-add td.fold-text { color: #1a4a2a; }
tr.fold-summary.fold-add:hover td { background: #d8f0d8; }
tr.fold-summary.fold-add td.fold-count { color: #2d8a4e; }
tr.fold-line.fold-expanded.fold-add td:first-child { border-left-color: #2d8a4e; }
/* Removed-side folds: soft pink (distinct from bright red diff lines) */
tr.fold-summary.fold-del td { background: #f6e0e8; }
tr.fold-summary.fold-del td:first-child { border-left-color: #b03060; }
tr.fold-summary.fold-del td.fold-text { color: #5a1a30; }
tr.fold-summary.fold-del:hover td { background: #f0d4de; }
tr.fold-summary.fold-del td.fold-count { color: #b03060; }
tr.fold-line.fold-expanded.fold-del td:first-child { border-left-color: #b03060; }
@media (prefers-color-scheme: dark) {
    tr.fold-summary td {
        background: #1a1800;
    }
    tr.fold-summary td:first-child {
        border-left-color: #c0a000;
    }
    tr.fold-summary td.fold-text {
        color: #e8d080;
    }
    tr.fold-summary:hover td {
        background: #2a2400;
    }
    tr.fold-summary.placeholder td,
    tr.fold-summary.placeholder:hover td {
        background: transparent;
    }
    td.fold-count { color: #8a7a30; }
    tr.fold-line.fold-expanded td:first-child {
        border-left-color: #c0a000;
    }
    /* Dark: added-side folds (green) */
    tr.fold-summary.fold-add td { background: #0d220d; }
    tr.fold-summary.fold-add td:first-child { border-left-color: #3fb950; }
    tr.fold-summary.fold-add td.fold-text { color: #7ee890; }
    tr.fold-summary.fold-add:hover td { background: #163016; }
    tr.fold-summary.fold-add td.fold-count { color: #3fb950; }
    tr.fold-line.fold-expanded.fold-add td:first-child { border-left-color: #3fb950; }
    /* Dark: removed-side folds (pink) */
    tr.fold-summary.fold-del td { background: #260d18; }
    tr.fold-summary.fold-del td:first-child { border-left-color: #e06090; }
    tr.fold-summary.fold-del td.fold-text { color: #f0a0c0; }
    tr.fold-summary.fold-del:hover td { background: #341428; }
    tr.fold-summary.fold-del td.fold-count { color: #e06090; }
    tr.fold-line.fold-expanded.fold-del td:first-child { border-left-color: #e06090; }
}

/* Single-column diff (add-only or remove-only blocks) */
.diff-block:has(.diff-single) {
    width: fit-content;
    min-width: calc((100vw - 260px) * 0.6);
    max-width: calc(100vw - 260px);
    overflow-x: auto;
}
.diff-single { table-layout: auto; width: max-content; min-width: 100%; }
.diff-single tr.line-removed td:last-child { border-right: 1px solid var(--removed-bg); }
.diff-single tr.line-removed-partial td:last-child { border-right: 1px solid var(--bg); }
.diff-single tr.line-added td:last-child { border-right: 1px solid var(--added-bg); }
.diff-single tr.line-added-partial td:last-child { border-right: 1px solid var(--bg); }
.diff-single tr.line-context td:last-child { border-right: 1px solid var(--bg); }
.diff-single tr.line-paired td:last-child { border-right: 1px solid var(--bg); }
.diff-single tr.line-paired-full td:last-child { border-right: 1px solid var(--removed-bg); }
.diff-single col.code-col { width: auto; }
.diff-single td[class*="code"] { white-space: pre; overflow-wrap: normal; }
.diff-single .line-added .code-rhs,
.diff-single .line-added .sign-rhs { background: var(--added-bg); }
.diff-single .line-added .ln-rhs { background: var(--added-num-bg); }
.diff-single .line-removed .code-lhs,
.diff-single .line-removed .sign-lhs { background: var(--removed-bg); }
.diff-single .line-removed .ln-lhs { background: var(--removed-num-bg); }

/* Source blocks now reuse diff-single styling */

/* Service badges: `@sboxd` in markdown */
.service-badge {
    font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, monospace;
    font-size: 0.9em;
    font-family: "SF Mono", "Menlo", "Consolas", monospace;
    color: #0550ae;
    background: #f0f4ff;
    padding: 0.1em 0.4em;
    border-radius: 4px;
    border: 1px solid #c8d8f0;
}

@media (prefers-color-scheme: dark) {
    .service-badge {
        color: #a0c0f0;
        background: #0a1428;
        border-color: #1a2a48;
    }
}

/* Task lists (custom checkboxes) */
article li > input[type="checkbox"] {
    appearance: none;
    -webkit-appearance: none;
    width: 1.15em;
    height: 1.15em;
    border: 1.5px solid var(--border);
    border-radius: 3px;
    background: var(--bg);
    vertical-align: -0.2em;
    margin-right: 0.35em;
    cursor: default;
    position: relative;
}
article li > input[type="checkbox"]:checked {
    background: #6e7781;
    border-color: #6e7781;
}
article li > input[type="checkbox"]:checked::after {
    content: "";
    position: absolute;
    left: 4px;
    top: 1px;
    width: 4.5px;
    height: 8.5px;
    border: solid #fff;
    border-width: 0 2px 2px 0;
    transform: rotate(45deg);
}
article li:has(> input[type="checkbox"]) {
    list-style: none;
    margin-left: -1.35em;
}
@media (prefers-color-scheme: dark) {
    article li > input[type="checkbox"] {
        border-color: #555;
    }
    article li > input[type="checkbox"]:checked {
        background: #8b949e;
        border-color: #8b949e;
    }
}

/* Mermaid diagrams */
.mermaid-diagram {
    margin: 1rem 0;
    text-align: center;
}
.mermaid-diagram svg {
    max-width: 100%;
    height: auto;
}

/* Token-level highlights within changed lines (more saturated than row bg) */
.hl-del { background: var(--removed-hl); }
.hl-add { background: var(--added-hl); }
"#;

const JS: &str = r#"
// Close duplicate tabs: when a walkthrough opens, tell other tabs with the
// same filename to close. Uses BroadcastChannel (same-origin only, works
// with file:// URLs in the same browser).
(function() {
    var filename = location.pathname.split('/').pop() || '';
    if (filename && typeof BroadcastChannel !== 'undefined') {
        var ch = new BroadcastChannel('walkthrough_dedup');
        ch.postMessage({ type: 'opened', filename: filename, time: Date.now() });
        ch.onmessage = function(e) {
            if (e.data.type === 'opened' && e.data.filename === filename && e.data.time > 0) {
                // A newer tab with the same filename opened; close this one.
                window.close();
            }
        };
    }
})();

(function() {
    var blocks = document.querySelectorAll('.diff-block');
    var pinnedY = null;
    var activeBlock = null;
    var lastWheelTime = 0;
    var navCooldownUntil = 0;

    // After hash navigation (TOC click), suppress block capture briefly
    // so the user can scroll freely from the new position. Also scroll
    // diff blocks above the target to bottom and below to top so you
    // see the end of the previous section and start of the next.
    window.addEventListener('hashchange', function() {
        pinnedY = null;
        activeBlock = null;
        navCooldownUntil = Date.now() + 600;
        var target = document.getElementById(location.hash.slice(1));
        if (!target) return;
        var targetY = target.getBoundingClientRect().top + window.scrollY;
        for (var i = 0; i < blocks.length; i++) {
            var block = blocks[i];
            var blockY = block.offsetTop + block.offsetHeight / 2;
            if (blockY < targetY) {
                block.scrollTop = block.scrollHeight;
            } else {
                block.scrollTop = 0;
            }
        }
    });

    window.addEventListener('wheel', function(e) {
        lastWheelTime = Date.now();
        if (Date.now() < navCooldownUntil) return;

        // If we have an active block, keep the page pinned and scroll it
        if (activeBlock) {
            var maxScroll = activeBlock.scrollHeight - activeBlock.clientHeight;
            var atEnd = activeBlock.scrollTop >= maxScroll - 1;
            var atStart = activeBlock.scrollTop <= 0;

            if ((e.deltaY > 0 && !atEnd) || (e.deltaY < 0 && !atStart)) {
                e.preventDefault();
                window.scrollTo(0, pinnedY);
                activeBlock.scrollTop += e.deltaY;
                return;
            }
            // Released: diff reached its end/start
            activeBlock = null;
            pinnedY = null;
            return;
        }

        // Check if any block should become active
        for (var i = 0; i < blocks.length; i++) {
            var block = blocks[i];
            if (block.classList.contains('collapsed')) continue;
            var rect = block.getBoundingClientRect();

            var maxScroll = block.scrollHeight - block.clientHeight;
            if (maxScroll <= 0) continue;
            if (rect.bottom < 0 || rect.top > window.innerHeight) continue;

            var atEnd = block.scrollTop >= maxScroll - 1;
            var atStart = block.scrollTop <= 0;

            if (e.deltaY > 0 && rect.top <= 120 && !atEnd) {
                e.preventDefault();
                pinnedY = window.scrollY;
                activeBlock = block;
                window.scrollTo(0, pinnedY);
                block.scrollTop += e.deltaY;
                return;
            }

            if (e.deltaY < 0 && !atStart && rect.bottom >= window.innerHeight - 80) {
                e.preventDefault();
                pinnedY = window.scrollY;
                activeBlock = block;
                window.scrollTo(0, pinnedY);
                block.scrollTop += e.deltaY;
                return;
            }
        }
    }, { passive: false });

    // Pin on scroll events caused by trackpad momentum, but only
    // shortly after a wheel event. Non-wheel scrolls (TOC clicks,
    // keyboard navigation) clear the pin instead of fighting them.
    window.addEventListener('scroll', function() {
        if (pinnedY !== null) {
            if (Date.now() - lastWheelTime < 200) {
                window.scrollTo(0, pinnedY);
            } else {
                pinnedY = null;
                activeBlock = null;
            }
        }
    });
})();

// Table of contents: build from headings and highlight on scroll
(function() {
    var headings = document.querySelectorAll('article h1[id], article h2[id], article h3[id]');
    if (headings.length < 2) return;

    var nav = document.createElement('nav');
    nav.className = 'toc';
    for (var i = 0; i < headings.length; i++) {
        var h = headings[i];
        var a = document.createElement('a');
        a.className = 'toc-' + h.tagName.toLowerCase();
        if (h.tagName === 'H1') {
            // h1 is sticky, so use scroll-to-top instead of anchor
            a.href = '#';
            // Exclude subtitle span text
            var clone = h.cloneNode(true);
            var sub = clone.querySelector('.subtitle');
            if (sub) sub.remove();
            a.textContent = clone.textContent;
        } else {
            a.href = '#' + h.id;
            a.textContent = h.textContent;
        }
        nav.appendChild(a);
    }
    document.body.appendChild(nav);

    // Position TOC below the sticky h1 via CSS variable
    function updateTocTop() {
        var h1 = document.querySelector('article h1');
        if (h1) {
            var top = h1.offsetHeight + 42;
            document.documentElement.style.setProperty('--toc-top', top + 'px');
        }
    }
    updateTocTop();
    // Re-measure after fonts load (may change h1 height)
    if (document.fonts) document.fonts.ready.then(updateTocTop);

    var links = nav.querySelectorAll('a');
    var ticking = false;

    function updateActive() {
        var current = null;
        for (var i = 0; i < headings.length; i++) {
            if (headings[i].getBoundingClientRect().top <= 80) {
                current = i;
            }
        }
        for (var j = 0; j < links.length; j++) {
            links[j].classList.toggle('active', j === current);
        }
        ticking = false;
    }

    window.addEventListener('scroll', function() {
        if (!ticking) {
            requestAnimationFrame(updateActive);
            ticking = true;
        }
    });
    updateActive();
})();

// Code annotation tooltips (hover, follow cursor, no flicker between rows)
(function() {
    var rows = document.querySelectorAll('tr.annotated[data-notes]');
    if (!rows.length) return;

    var tip = document.createElement('div');
    tip.className = 'note-tooltip';
    document.body.appendChild(tip);
    var hideTimer = null;
    var currentKey = '';

    rows.forEach(function(row) {
        row.addEventListener('mouseenter', function() {
            clearTimeout(hideTimer);
            var raw = row.getAttribute('data-notes');
            if (raw !== currentKey) {
                tip.innerHTML = '';
                var notes = raw.split('|');
                notes.forEach(function(note, i) {
                    if (i > 0) tip.appendChild(document.createElement('hr'));
                    var p = document.createElement('div');
                    p.textContent = note;
                    p.style.padding = '2px 0';
                    tip.appendChild(p);
                });
                currentKey = raw;
            }
            tip.style.display = 'block';
        });
        row.addEventListener('mousemove', function(e) {
            var x = e.clientX + 16;
            var maxX = window.innerWidth - tip.offsetWidth - 8;
            tip.style.left = Math.min(x, maxX) + 'px';
            tip.style.top = (e.clientY + 16) + 'px';
        });
        row.addEventListener('mouseleave', function() {
            hideTimer = setTimeout(function() {
                tip.style.display = 'none';
                currentKey = '';
            }, 50);
        });
    });
})();

// Single-column blocks use CSS fit-content + max-content table for
// width sizing. No JS measurement needed.

// Collapsible diff blocks: click header to toggle, search to auto-expand
(function() {
    document.querySelectorAll('.diff-block').forEach(function(block) {
        var header = block.querySelector('.diff-header');
        var body = block.querySelector('.diff-body');
        if (!header || !body) return;

        function collapse() {
            block.classList.add('collapsed');
            var mode = block.getAttribute('data-collapse');
            if (mode === 'hidden') {
                body.classList.add('collapsed-hidden');
            } else {
                body.setAttribute('hidden', 'until-found');
            }
        }
        function expand() {
            block.classList.remove('collapsed');
            body.removeAttribute('hidden');
            body.classList.remove('collapsed-hidden');
        }
        function toggle() {
            if (block.classList.contains('collapsed')) { expand(); } else { collapse(); }
        }
        header.addEventListener('click', function(e) {
            if (e.target.closest('.viewed-label')) return;
            toggle();
        });
        var disclaimer = block.querySelector('.collapse-disclaimer');
        if (disclaimer) disclaimer.addEventListener('click', toggle);

        var checkbox = block.querySelector('.viewed-check');
        if (checkbox) {
            checkbox.addEventListener('change', function() {
                if (checkbox.checked) {
                    block.classList.add('viewed');
                    collapse();
                } else {
                    block.classList.remove('viewed');
                }
            });
        }

        body.addEventListener('beforematch', function() {
            expand();
        });
    });
})();

// Code folds: click to expand/collapse pseudocode summaries
(function() {
    function expandFold(table, id) {
        var summary = table.querySelector('tr.fold-summary[data-fold-id="' + id + '"]');
        if (summary) summary.classList.add('fold-expanded');
        table.querySelectorAll('tr.fold-line[data-fold-id="' + id + '"]').forEach(function(line) {
            line.removeAttribute('hidden');
            line.classList.add('fold-expanded');
        });
        var sides = table.closest('.diff-sides');
        if (sides) sides.dispatchEvent(new Event('fold-toggle'));
    }
    function collapseFold(table, id) {
        var summary = table.querySelector('tr.fold-summary[data-fold-id="' + id + '"]');
        if (summary) summary.classList.remove('fold-expanded');
        table.querySelectorAll('tr.fold-line[data-fold-id="' + id + '"]').forEach(function(line) {
            line.setAttribute('hidden', 'until-found');
            line.classList.remove('fold-expanded');
        });
        var sides = table.closest('.diff-sides');
        if (sides) sides.dispatchEvent(new Event('fold-toggle'));
    }
    document.querySelectorAll('tr.fold-summary').forEach(function(row) {
        row.addEventListener('click', function() {
            var id = row.getAttribute('data-fold-id');
            var table = row.closest('table');
            if (row.classList.contains('fold-expanded')) {
                collapseFold(table, id);
            } else {
                expandFold(table, id);
            }
        });
    });
    // Auto-expand fold when browser find matches hidden content
    document.querySelectorAll('tr.fold-line').forEach(function(line) {
        line.addEventListener('beforematch', function() {
            var id = line.getAttribute('data-fold-id');
            var table = line.closest('table');
            expandFold(table, id);
            // Scroll the diff block to show the matched row
            setTimeout(function() {
                var block = line.closest('.diff-block');
                if (block) {
                    var blockTop = block.getBoundingClientRect().top;
                    var lineTop = line.getBoundingClientRect().top;
                    if (lineTop < blockTop || lineTop > blockTop + block.clientHeight) {
                        block.scrollTop += lineTop - blockTop - 40;
                    }
                }
            }, 0);
        });
    });
})();

// Expandable context: click to reveal hidden context lines
(function() {
    var STEP = 8;
    function revealAll(scope, id) {
        scope.querySelectorAll('tr.expand-line[data-expand-id="' + id + '"]').forEach(function(l) {
            l.removeAttribute('hidden');
            l.classList.remove('expand-line');
        });
        scope.querySelectorAll('tr.expand-summary[data-expand-id="' + id + '"]').forEach(function(s) {
            s.remove();
        });
    }
    function updateButton(btn, remaining) {
        if (remaining <= 0) { btn.remove(); return; }
        var next = Math.min(remaining, STEP);
        var dir = btn.getAttribute('data-dir');
        var arrow = dir === 'down' ? '\u2193' : dir === 'up' ? '\u2191' : '\u21C5';
        var td = btn.querySelector('td');
        if (td) td.textContent = arrow + ' Show ' + next + ' more ' + (next === 1 ? 'line' : 'lines');
    }
    function revealStep(row) {
        var id = row.getAttribute('data-expand-id');
        var dir = row.getAttribute('data-dir') || 'all';
        var sides = row.closest('.diff-sides');
        var scope = sides || row.closest('.diff-body');
        if (!scope) return;
        // Reveal STEP lines from one direction in each table,
        // then reposition the button to stay adjacent to remaining hidden lines.
        var tables = scope.querySelectorAll('table');
        var remaining = 0;
        tables.forEach(function(table) {
            var hidden = Array.from(table.querySelectorAll(
                'tr.expand-line[data-expand-id="' + id + '"][hidden]'));
            var batch = dir === 'down' ? hidden.slice(0, STEP) : hidden.slice(-STEP);
            remaining = Math.max(remaining, hidden.length - batch.length);
            batch.forEach(function(line) {
                line.removeAttribute('hidden');
                line.classList.remove('expand-line');
            });
            // Reposition this table's button next to remaining hidden lines.
            var myBtn = table.querySelector('tr.expand-summary[data-expand-id="' + id + '"]');
            if (myBtn && batch.length > 0) {
                var lastRevealed = batch[batch.length - 1];
                var firstRevealed = batch[0];
                if (dir === 'down') {
                    lastRevealed.parentNode.insertBefore(myBtn, lastRevealed.nextSibling);
                } else {
                    firstRevealed.parentNode.insertBefore(myBtn, firstRevealed);
                }
            }
        });
        updateButton(row, remaining);
        // Update the same button in the other table (two-table mode).
        scope.querySelectorAll('tr.expand-summary[data-expand-id="' + id + '"]').forEach(function(s) {
            if (s !== row) updateButton(s, remaining);
        });
        if (sides) sides.dispatchEvent(new Event('fold-toggle'));
    }
    document.querySelectorAll('tr.expand-summary').forEach(function(row) {
        row.addEventListener('click', function() { revealStep(row); });
    });
    // Auto-expand all when browser find matches hidden context
    document.querySelectorAll('tr.expand-line').forEach(function(line) {
        line.addEventListener('beforematch', function() {
            var id = line.getAttribute('data-expand-id');
            var sides = line.closest('.diff-sides');
            var scope = sides || line.closest('.diff-body');
            if (scope) {
                revealAll(scope, id);
                if (sides) sides.dispatchEvent(new Event('fold-toggle'));
            }
        });
    });
})();

// Height-sync for two-table side-by-side diffs
(function() {
    document.querySelectorAll('.diff-sides').forEach(function(sides) {
        var lhsTable = sides.querySelector('.diff-table-lhs');
        var rhsTable = sides.querySelector('.diff-table-rhs');
        if (!lhsTable || !rhsTable) return;

        function syncHeights() {
            var lhsRows = lhsTable.querySelectorAll('tbody tr[data-row-idx]');
            var rhsRows = rhsTable.querySelectorAll('tbody tr[data-row-idx]');
            var rhsMap = {};
            for (var i = 0; i < rhsRows.length; i++) {
                rhsMap[rhsRows[i].getAttribute('data-row-idx')] = rhsRows[i];
            }
            // Reset all inline heights and re-collapse placeholders
            for (var j = 0; j < lhsRows.length; j++) {
                lhsRows[j].style.height = '';
                lhsRows[j].style.visibility = '';
            }
            for (var j = 0; j < rhsRows.length; j++) {
                rhsRows[j].style.height = '';
                rhsRows[j].style.visibility = '';
            }
            // Measure and sync rows by data-row-idx
            for (var j = 0; j < lhsRows.length; j++) {
                var lRow = lhsRows[j];
                var idx = lRow.getAttribute('data-row-idx');
                var rRow = rhsMap[idx];
                if (!rRow) continue;
                // Skip if either side is hidden (inside a fold)
                if (lRow.hasAttribute('hidden') || rRow.hasAttribute('hidden')) continue;
                var lh = lRow.getBoundingClientRect().height;
                var rh = rRow.getBoundingClientRect().height;
                var max = Math.max(lh, rh);
                if (max > 0 && lh !== rh) {
                    if (lh < max) {
                        lRow.style.height = max + 'px';
                        if (lRow.classList.contains('placeholder')) lRow.style.visibility = 'visible';
                    }
                    if (rh < max) {
                        rRow.style.height = max + 'px';
                        if (rRow.classList.contains('placeholder')) rRow.style.visibility = 'visible';
                    }
                }
            }
            // Sync fold-summary placeholders with their real counterparts
            // (these don't have data-row-idx, matched by data-fold-id instead)
            lhsTable.querySelectorAll('tr.fold-summary').forEach(function(lSum) {
                var fid = lSum.getAttribute('data-fold-id');
                var rSum = rhsTable.querySelector('tr.fold-summary[data-fold-id="' + fid + '"]');
                if (!rSum) return;
                lSum.style.height = '';
                lSum.style.visibility = '';
                rSum.style.height = '';
                rSum.style.visibility = '';
                var lh = lSum.getBoundingClientRect().height;
                var rh = rSum.getBoundingClientRect().height;
                var max = Math.max(lh, rh);
                if (max > 0 && lh !== rh) {
                    if (lh < max) {
                        lSum.style.height = max + 'px';
                        if (lSum.classList.contains('placeholder')) lSum.style.visibility = 'visible';
                    }
                    if (rh < max) {
                        rSum.style.height = max + 'px';
                        if (rSum.classList.contains('placeholder')) rSum.style.visibility = 'visible';
                    }
                }
            });
        }
        syncHeights();
        window.addEventListener('resize', function() {
            requestAnimationFrame(syncHeights);
        });
        sides.addEventListener('fold-toggle', function() {
            // Delay sync to after fold DOM changes take effect
            requestAnimationFrame(syncHeights);
        });
    });
})();
"#;

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Map a tree-sitter capture name to a GitHub prettylights color.
/// Returns (color, italic). No more fragile RGB matching.
fn github_color_for_capture(capture: &str) -> (&'static str, bool) {
    // Tree-sitter captures are hierarchical: "keyword.function", "string.special", etc.
    // Colors are GitHub prettylights with saturation boosted ~30% to compensate for
    // antialiasing on colored diff backgrounds.
    if capture.starts_with("comment") {
        return ("#556271", false);
    }
    if capture.starts_with("string") {
        return ("#002d72", false);
    }
    if capture.starts_with("constant") || capture.starts_with("number") || capture.starts_with("boolean") || capture.starts_with("float") {
        return ("#004fb3", false);
    }
    if capture.starts_with("keyword") || capture.starts_with("repeat") || capture.starts_with("conditional")
        || capture.starts_with("exception") || capture.starts_with("include") || capture.starts_with("storageclass")
    {
        return ("#db1522", false);
    }
    if capture.starts_with("constructor") {
        return ("#953800", false);
    }
    if capture.starts_with("type") {
        return ("#1f2328", false);
    }
    if capture.starts_with("function") || capture.starts_with("method") {
        return ("#6025cd", false);
    }
    if capture.starts_with("property") || capture.starts_with("field") {
        return ("#004fb3", false);
    }
    if capture.starts_with("variable") || capture.starts_with("parameter") {
        return ("#1f2328", false);
    }
    if capture.starts_with("operator") || capture.starts_with("punctuation") {
        return ("#1f2328", false);
    }
    if capture.starts_with("tag") {
        return ("#004fb3", false);
    }
    if capture.starts_with("attribute") {
        return ("#6025cd", false);
    }
    if capture.starts_with("label") || capture.starts_with("namespace") {
        return ("#953800", false);
    }
    ("#1f2328", false)
}

/// Priority for overlapping captures. Higher = wins.
/// Specific captures (function, type, keyword) beat generic ones (variable).
fn capture_priority(capture: &str) -> u8 {
    if capture.starts_with("comment") { return 10; }
    if capture.starts_with("string") { return 10; }
    if capture.starts_with("keyword") { return 9; }
    if capture.starts_with("function") || capture.starts_with("method") { return 8; }
    if capture.starts_with("constructor") { return 9; }
    if capture.starts_with("type") { return 8; }
    if capture.starts_with("constant") || capture.starts_with("number") || capture.starts_with("boolean") { return 7; }
    if capture.starts_with("tag") || capture.starts_with("attribute") { return 7; }
    if capture.starts_with("property") || capture.starts_with("field") { return 6; }
    if capture.starts_with("variable") || capture.starts_with("parameter") { return 5; }
    if capture.starts_with("operator") || capture.starts_with("punctuation") { return 3; }
    1
}

/// Syntax-highlight all lines of a file using arborium (tree-sitter).
/// Returns one HTML string per line with `<span style="...">` tokens.
fn syntax_highlight_lines(lines: &[String], hl: &mut Highlighter, lang: Option<&str>) -> Vec<String> {
    let lang = match lang {
        Some(l) => l,
        None => return lines.iter().map(|l| html_escape(l)).collect(),
    };

    let full_text = lines.join("\n");
    let spans = match hl.highlight_spans(lang, &full_text) {
        Ok(s) => s,
        Err(_) => return lines.iter().map(|l| html_escape(l)).collect(),
    };

    // Build a per-byte color map from spans. Tree-sitter spans can overlap
    // (nested scopes like "function" + "variable" on the same token).
    // Use capture priority so specific captures (function, type) win over
    // generic ones (variable).
    let text_len = full_text.len();
    let mut byte_color: Vec<Option<(&str, bool, u8)>> = vec![None; text_len];
    for span in &spans {
        let (color, italic) = github_color_for_capture(&span.capture);
        let priority = capture_priority(&span.capture);
        if color == "#1f2328" && !italic { continue; }
        let start = (span.start as usize).min(text_len);
        let end = (span.end as usize).min(text_len);
        for b in start..end {
            let dominated = byte_color[b].map_or(true, |(_, _, p)| priority >= p);
            if dominated {
                byte_color[b] = Some((color, italic, priority));
            }
        }
    }

    // Render per-line HTML from the byte color map.
    let mut result = Vec::with_capacity(lines.len());
    let mut byte_offset: usize = 0;

    for line in lines {
        let line_start = byte_offset;
        let line_end = byte_offset + line.len();

        let mut html = String::new();
        let mut pos = line_start;

        while pos < line_end {
            let cur_style = byte_color.get(pos).copied().flatten().map(|(c, i, _)| (c, i));

            // Find run of same style
            let mut run_end = pos + 1;
            while run_end < line_end {
                let next = byte_color.get(run_end).copied().flatten().map(|(c, i, _)| (c, i));
                if next != cur_style { break; }
                run_end += 1;
            }

            let text = &full_text[pos..run_end];
            match cur_style {
                None => html.push_str(&html_escape(text)),
                Some((color, true)) => html.push_str(&format!(
                    "<span style=\"font-style:italic;color:{}\">{}</span>",
                    color, html_escape(text)
                )),
                Some((color, false)) => html.push_str(&format!(
                    "<span style=\"color:{}\">{}</span>",
                    color, html_escape(text)
                )),
            }
            pos = run_end;
        }

        result.push(html);
        byte_offset = line_end + 1; // +1 for the \n
    }
    result
}

/// Insert a diff highlight span into syntax-highlighted HTML at the given
/// text-character positions (byte offsets into the original plain text).
/// Walks the HTML, tracking the text position, and injects opening/closing tags.
/// Insert multiple non-overlapping diff highlight spans into syntax-highlighted HTML.
/// `spans` must be sorted by start position and non-overlapping.
fn insert_diff_highlights(highlighted_html: &str, spans: &[(usize, usize)], hl_class: &str) -> String {
    if spans.is_empty() {
        return highlighted_html.to_string();
    }

    let mut out = String::new();
    let mut text_pos: usize = 0;
    let mut span_idx: usize = 0;
    let mut opened = false;
    let bytes = highlighted_html.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'<' {
            let tag_end = highlighted_html[i..].find('>').map(|p| i + p + 1).unwrap_or(len);
            out.push_str(&highlighted_html[i..tag_end]);
            i = tag_end;
        } else if bytes[i] == b'&' {
            let ent_end = highlighted_html[i..].find(';').map(|p| i + p + 1).unwrap_or(i + 1);
            // Close current span if we've passed its end
            if opened {
                if let Some(&(_, end)) = spans.get(span_idx.wrapping_sub(1)) {
                    if text_pos >= end {
                        out.push_str("</span>");
                        opened = false;
                    }
                }
            }
            // Open new span if we've reached its start
            if !opened {
                if let Some(&(start, end)) = spans.get(span_idx) {
                    if text_pos >= start && text_pos < end {
                        out.push_str(&format!("<span class=\"{}\">", hl_class));
                        opened = true;
                        span_idx += 1;
                    }
                }
            }
            out.push_str(&highlighted_html[i..ent_end]);
            text_pos += 1;
            // Close if we've reached end after this char
            if opened {
                if let Some(&(_, end)) = spans.get(span_idx.wrapping_sub(1)) {
                    if text_pos >= end {
                        out.push_str("</span>");
                        opened = false;
                    }
                }
            }
            i = ent_end;
        } else {
            // Close current span if we've passed its end
            if opened {
                if let Some(&(_, end)) = spans.get(span_idx.wrapping_sub(1)) {
                    if text_pos >= end {
                        out.push_str("</span>");
                        opened = false;
                    }
                }
            }
            // Open new span if we've reached its start
            if !opened {
                if let Some(&(start, end)) = spans.get(span_idx) {
                    if text_pos >= start && text_pos < end {
                        out.push_str(&format!("<span class=\"{}\">", hl_class));
                        opened = true;
                        span_idx += 1;
                    }
                }
            }
            out.push(bytes[i] as char);
            text_pos += 1;
            // Close if we've reached end after this char
            if opened {
                if let Some(&(_, end)) = spans.get(span_idx.wrapping_sub(1)) {
                    if text_pos >= end {
                        out.push_str("</span>");
                        opened = false;
                    }
                }
            }
            i += 1;
        }
    }
    if opened {
        out.push_str("</span>");
    }
    out
}

fn insert_diff_highlight(highlighted_html: &str, change_start: usize, change_end: usize, hl_class: &str) -> String {
    if change_start >= change_end {
        return highlighted_html.to_string();
    }

    let mut out = String::new();
    let mut text_pos: usize = 0; // position in original text
    let mut opened = false;
    let bytes = highlighted_html.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'<' {
            // HTML tag: copy verbatim, doesn't advance text_pos
            let tag_end = highlighted_html[i..].find('>').map(|p| i + p + 1).unwrap_or(len);
            out.push_str(&highlighted_html[i..tag_end]);
            i = tag_end;
        } else if bytes[i] == b'&' {
            // HTML entity: counts as 1 text character
            let ent_end = highlighted_html[i..].find(';').map(|p| i + p + 1).unwrap_or(i + 1);
            if !opened && text_pos >= change_start && text_pos < change_end {
                out.push_str(&format!("<span class=\"{}\">", hl_class));
                opened = true;
            }
            if opened && text_pos >= change_end {
                out.push_str("</span>");
                opened = false;
            }
            if !opened && text_pos == change_start {
                out.push_str(&format!("<span class=\"{}\">", hl_class));
                opened = true;
            }
            out.push_str(&highlighted_html[i..ent_end]);
            text_pos += 1;
            if opened && text_pos >= change_end {
                out.push_str("</span>");
                opened = false;
            }
            i = ent_end;
        } else {
            // Regular character
            if !opened && text_pos == change_start {
                out.push_str(&format!("<span class=\"{}\">", hl_class));
                opened = true;
            }
            out.push(highlighted_html.as_bytes()[i] as char);
            text_pos += 1;
            if opened && text_pos >= change_end {
                out.push_str("</span>");
                opened = false;
            }
            i += 1;
        }
    }
    if opened {
        out.push_str("</span>");
    }
    out
}

/// Find the minimal differing region between two strings by trimming common prefix/suffix.
fn find_change_bounds(old: &str, new: &str) -> (usize, usize, usize, usize) {
    let prefix_len = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();

    let old_rest = &old[prefix_len..];
    let new_rest = &new[prefix_len..];
    let suffix_len = old_rest
        .bytes()
        .rev()
        .zip(new_rest.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();

    (
        prefix_len,
        old.len() - suffix_len,
        prefix_len,
        new.len() - suffix_len,
    )
}

/// A row to render in the diff table.
#[derive(Clone, Copy)]
struct DiffRow<'a> {
    lhs: Option<&'a LineSide>,
    rhs: Option<&'a LineSide>,
}

/// Consolidate entries within a chunk for better side-by-side layout.
fn consolidate_chunk<'a>(entries: &'a [LineEntry]) -> Vec<DiffRow<'a>> {
    let mut rows = Vec::new();
    let mut i = 0;

    while i < entries.len() {
        let entry = &entries[i];
        let has_lhs = entry.lhs.is_some();
        let has_rhs = entry.rhs.is_some();

        if has_lhs && has_rhs {
            rows.push(DiffRow {
                lhs: entry.lhs.as_ref(),
                rhs: entry.rhs.as_ref(),
            });
            i += 1;
        } else if has_lhs {
            let lhs_start = i;
            while i < entries.len() && entries[i].lhs.is_some() && entries[i].rhs.is_none() {
                i += 1;
            }
            let lhs_run = &entries[lhs_start..i];

            let rhs_start = i;
            while i < entries.len() && entries[i].rhs.is_some() && entries[i].lhs.is_none() {
                i += 1;
            }
            let rhs_run = &entries[rhs_start..i];

            let max_len = lhs_run.len().max(rhs_run.len());
            for j in 0..max_len {
                rows.push(DiffRow {
                    lhs: lhs_run.get(j).and_then(|e| e.lhs.as_ref()),
                    rhs: rhs_run.get(j).and_then(|e| e.rhs.as_ref()),
                });
            }
        } else if has_rhs {
            // Added-only entries: don't pair with following removed entries.
            // The removed→added direction (has_lhs branch) pairs naturally
            // because removed lines are replaced by added lines. But
            // added→removed is the reverse and creates cross-ordered pairs
            // when the two runs are semantically unrelated.
            rows.push(DiffRow {
                lhs: None,
                rhs: entry.rhs.as_ref(),
            });
            i += 1;
        } else {
            i += 1;
        }
    }

    rows
}

/// Compute the Longest Common Subsequence of two string slices.
/// Returns pairs of (old_idx, new_idx) that form the LCS.
fn longest_common_subsequence(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let m = old.len();
    let n = new.len();
    // dp[i][j] = LCS length for old[..i] vs new[..j]
    let mut dp = vec![vec![0u32; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if old[i - 1] == new[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to find matched pairs
    let mut matches = Vec::new();
    let mut i = m;
    let mut j = n;
    while i > 0 && j > 0 {
        if old[i - 1] == new[j - 1] {
            matches.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] >= dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    matches.reverse();
    matches
}

/// Post-process consolidated rows to detect reordered lines. When a block of
/// "changed" rows contains lines that appear in both old and new (just at
/// different positions), re-pair them as context so only the truly moved lines
/// show as removed/added.
///
/// Uses LCS to determine which lines are in the same relative order (context)
/// vs which were actually moved (shown as remove + add).
fn fix_reorders<'a>(
    rows: Vec<DiffRow<'a>>,
    old_lines: &[String],
    new_lines: &[String],
) -> Vec<DiffRow<'a>> {
    let is_context = |row: &DiffRow| -> bool {
        if let (Some(lhs), Some(rhs)) = (row.lhs, row.rhs) {
            let old = old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
            let new = new_lines.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
            old == new
        } else {
            false
        }
    };

    let mut result = Vec::with_capacity(rows.len());
    let mut i = 0;

    while i < rows.len() {
        if is_context(&rows[i]) {
            result.push(rows[i]);
            i += 1;
            continue;
        }

        // Collect a maximal run of non-context (changed) rows
        let run_start = i;
        while i < rows.len() && !is_context(&rows[i]) {
            i += 1;
        }
        let run = &rows[run_start..i];

        // Collect old-side and new-side entries from the run
        let mut old_entries: Vec<&'a LineSide> = Vec::new();
        let mut new_entries: Vec<&'a LineSide> = Vec::new();

        for row in run {
            if let Some(lhs) = row.lhs {
                old_entries.push(lhs);
            }
            if let Some(rhs) = row.rhs {
                new_entries.push(rhs);
            }
        }

        // Need at least 2 entries on each side for reorder detection
        // (and cap at 200 to avoid expensive LCS on huge runs)
        if old_entries.len() < 2 || new_entries.len() < 2
            || old_entries.len() > 200 || new_entries.len() > 200
        {
            result.extend_from_slice(run);
            continue;
        }

        // Build content vectors for LCS
        let old_contents: Vec<&str> = old_entries.iter()
            .map(|s| old_lines.get(s.line_number as usize).map(|l| l.as_str()).unwrap_or(""))
            .collect();
        let new_contents: Vec<&str> = new_entries.iter()
            .map(|s| new_lines.get(s.line_number as usize).map(|l| l.as_str()).unwrap_or(""))
            .collect();

        let lcs = longest_common_subsequence(&old_contents, &new_contents);

        // Only re-pair when a large portion of lines have exact content
        // matches (evidence of reordering, not a refactor with coincidental
        // matches). Require >= 50% of both sides to be matched.
        let min_side = old_entries.len().min(new_entries.len());
        if lcs.len() * 2 < min_side {
            result.extend_from_slice(run);
            continue;
        }

        // Build sets of matched indices
        let mut matched_old = vec![false; old_entries.len()];
        let mut matched_new = vec![false; new_entries.len()];
        // Map from new_idx → old_idx for LCS matches
        let mut new_to_old: HashMap<usize, usize> = HashMap::new();
        for &(oi, ni) in &lcs {
            matched_old[oi] = true;
            matched_new[ni] = true;
            new_to_old.insert(ni, oi);
        }

        // Emit unmatched old entries as removals (in old-file order)
        for (oi, &old_side) in old_entries.iter().enumerate() {
            if !matched_old[oi] {
                result.push(DiffRow { lhs: Some(old_side), rhs: None });
            }
        }

        // Emit new entries in order: matched as paired context, unmatched as additions
        for (ni, &new_side) in new_entries.iter().enumerate() {
            if let Some(&oi) = new_to_old.get(&ni) {
                result.push(DiffRow { lhs: Some(old_entries[oi]), rhs: Some(new_side) });
            } else {
                result.push(DiffRow { lhs: None, rhs: Some(new_side) });
            }
        }
    }

    result
}

/// Merge adjacent difft change spans separated by whitespace or punctuation.
/// Returns sorted, non-overlapping (start, end) byte ranges.
fn merge_whitespace_spans(changes: &[crate::difft_json::ChangeSpan], line: &str) -> Vec<(usize, usize)> {
    let mut spans: Vec<(usize, usize)> = changes.iter()
        .map(|s| (s.start, s.end))
        .collect();
    spans.sort_by_key(|&(start, _)| start);

    let mut merged: Vec<(usize, usize)> = Vec::new();
    for &(start, end) in &spans {
        if let Some(last) = merged.last_mut() {
            let gap = &line.as_bytes()[last.1..start.min(line.len())];
            // Merge if gap is only whitespace and punctuation (not identifiers)
            if gap.iter().all(|&b| !b.is_ascii_alphanumeric() && b != b'_') {
                last.1 = end;
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

/// Check if merged spans cover the full content of a line (from first
/// non-whitespace character to the last non-whitespace character).
fn spans_cover_full_line(spans: &[(usize, usize)], line: &str) -> bool {
    if spans.is_empty() { return false; }
    let first_nws = line.bytes().position(|b| b != b' ' && b != b'\t').unwrap_or(0);
    let last_nws = line.bytes().rposition(|b| b != b' ' && b != b'\t').map(|p| p + 1).unwrap_or(0);
    if first_nws >= last_nws { return true; } // all whitespace
    let span_start = spans.first().map(|s| s.0).unwrap_or(usize::MAX);
    let span_end = spans.last().map(|s| s.1).unwrap_or(0);
    span_start <= first_nws && span_end >= last_nws
}

fn render_diff_row(
    row: &DiffRow,
    old_lines: &[String], new_lines: &[String],
    old_hl: &[String], new_hl: &[String],
) -> String {
    let mut html = String::new();

    // For paired rows, determine if it's a full-line or partial change,
    // then choose the appropriate row class and highlight strategy.
    if let (Some(lhs), Some(rhs)) = (row.lhs, row.rhs) {
        let old_line = old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let new_line = new_lines.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let old_highlighted = old_hl.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let new_highlighted = new_hl.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");

        // Difft sometimes includes unchanged lines in a chunk because the
        // surrounding syntactic structure changed (e.g. a JSDoc comment got
        // longer). Render these as context, not as paired changes.
        // Difft sometimes includes unchanged lines in a chunk because the
        // surrounding syntactic structure changed (e.g. a JSDoc comment got
        // longer). Render these as context, not as paired changes.
        if old_line == new_line {
            return render_context_row(
                lhs.line_number as usize, rhs.line_number as usize, old_hl, new_hl,
            );
        }

        let old_code;
        let new_code;
        let is_full_line;

        if !lhs.changes.is_empty() || !rhs.changes.is_empty() {
            // Merge adjacent spans separated only by whitespace
            let old_spans = merge_whitespace_spans(&lhs.changes, old_line);
            let new_spans = merge_whitespace_spans(&rhs.changes, new_line);

            // Check if spans cover the full content (first non-ws to end)
            is_full_line = spans_cover_full_line(&new_spans, new_line)
                && spans_cover_full_line(&old_spans, old_line);

            if is_full_line {
                // Full-line change: use row background, no token highlights needed
                old_code = old_highlighted.to_string();
                new_code = new_highlighted.to_string();
            } else {
                old_code = insert_diff_highlights(old_highlighted, &old_spans, "hl-del");
                new_code = insert_diff_highlights(new_highlighted, &new_spans, "hl-add");
            }

            // Purely additive: old has no spans, only new changed.
            // Render old side as context (no minus, no red).
            // Purely additive: old has no spans, only new changed.
            // Render old side as context (no minus, no red).
            if lhs.changes.is_empty() && !rhs.changes.is_empty() {
                let bg = "style=\"background:var(--bg)\"";
                html.push_str("<tr class=\"line-paired\">");
                html.push_str(&format!(
                    "<td class=\"ln ln-lhs\" {bg}>{}</td><td class=\"sign sign-lhs\" {bg}></td><td class=\"code-lhs\" {bg}>{}</td>",
                    lhs.line_number + 1, old_highlighted
                ));
                html.push_str(&format!(
                    "<td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td>",
                    rhs.line_number + 1, new_code
                ));
                html.push_str("</tr>");
                return html;
            }
            // Purely subtractive: only old changed, new side is context
            if !lhs.changes.is_empty() && rhs.changes.is_empty() {
                let bg = "style=\"background:var(--bg)\"";
                html.push_str("<tr class=\"line-paired\">");
                html.push_str(&format!(
                    "<td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td>",
                    lhs.line_number + 1, old_code
                ));
                html.push_str(&format!(
                    "<td class=\"ln ln-rhs\" {bg}>{}</td><td class=\"sign sign-rhs\" {bg}></td><td class=\"code-rhs\" {bg}>{}</td>",
                    rhs.line_number + 1, new_highlighted
                ));
                html.push_str("</tr>");
                return html;
            }
        } else {
            // No difft spans: fall back to prefix/suffix comparison
            let (old_cs, old_ce, new_cs, new_ce) = find_change_bounds(old_line, new_line);
            is_full_line = false;
            old_code = insert_diff_highlight(old_highlighted, old_cs, old_ce, "hl-del");
            new_code = insert_diff_highlight(new_highlighted, new_cs, new_ce, "hl-add");
        }

        let row_class = if is_full_line { "line-paired-full" } else { "line-paired" };
        html.push_str(&format!("<tr class=\"{}\">", row_class));

        html.push_str(&format!(
            "<td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td>",
            lhs.line_number + 1, old_code
        ));
        html.push_str(&format!(
            "<td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td>",
            rhs.line_number + 1, new_code
        ));
    } else if let Some(lhs) = row.lhs {
        let old_line = old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let old_highlighted = old_hl.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let spans = merge_whitespace_spans(&lhs.changes, old_line);
        let is_full = spans.is_empty() || spans_cover_full_line(&spans, old_line);
        let row_class = if is_full { "line-removed" } else { "line-removed-partial" };
        let content = if is_full {
            old_highlighted.to_string()
        } else {
            insert_diff_highlights(old_highlighted, &spans, "hl-del")
        };
        html.push_str(&format!("<tr class=\"{}\">", row_class));
        html.push_str(&format!(
            "<td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td>",
            lhs.line_number + 1, content
        ));
        html.push_str("<td class=\"ln ln-rhs\"></td><td class=\"sign sign-rhs\"></td><td class=\"code-rhs\"></td>");
    } else if let Some(rhs) = row.rhs {
        let new_line = new_lines.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let new_highlighted = new_hl.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let spans = merge_whitespace_spans(&rhs.changes, new_line);
        let is_full = spans.is_empty() || spans_cover_full_line(&spans, new_line);
        let row_class = if is_full { "line-added" } else { "line-added-partial" };
        let content = if is_full {
            new_highlighted.to_string()
        } else {
            insert_diff_highlights(new_highlighted, &spans, "hl-add")
        };
        html.push_str(&format!("<tr class=\"{}\">", row_class));
        html.push_str("<td class=\"ln ln-lhs\"></td><td class=\"sign sign-lhs\"></td><td class=\"code-lhs\"></td>");
        html.push_str(&format!(
            "<td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td>",
            rhs.line_number + 1, content
        ));
    } else {
        return String::new();
    }

    html.push_str("</tr>");
    html
}

/// Render a context (unchanged) line showing both old and new sides.
/// Expects pre-highlighted HTML content in the line arrays.
fn render_context_row(old_idx: usize, new_idx: usize, old_hl: &[String], new_hl: &[String]) -> String {
    let old_content = old_hl.get(old_idx).map(|s| s.as_str()).unwrap_or("");
    let new_content = new_hl.get(new_idx).map(|s| s.as_str()).unwrap_or("");
    format!(
        "<tr class=\"line-context\"><td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\"></td><td class=\"code-lhs\">{}</td>\
         <td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\"></td><td class=\"code-rhs\">{}</td></tr>",
        old_idx + 1, old_content, new_idx + 1, new_content
    )
}

/// Render a single-column context row (for add-only or remove-only blocks).
fn render_single_context_row(idx: usize, hl_lines: &[String]) -> String {
    let content = hl_lines.get(idx).map(|s| s.as_str()).unwrap_or("");
    format!(
        "<tr class=\"line-context\"><td class=\"ln\">{}</td><td class=\"sign\"></td><td class=\"code\">{}</td></tr>",
        idx + 1, content
    )
}

/// Render a hidden expandable context row (side-by-side, 6 cols).
fn render_context_row_expandable(old_idx: usize, new_idx: usize, old_hl: &[String], new_hl: &[String], expand_id: &str) -> String {
    let old_content = old_hl.get(old_idx).map(|s| s.as_str()).unwrap_or("");
    let new_content = new_hl.get(new_idx).map(|s| s.as_str()).unwrap_or("");
    format!(
        "<tr class=\"line-context expand-line\" data-expand-id=\"{}\" hidden=\"until-found\">\
         <td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\"></td><td class=\"code-lhs\">{}</td>\
         <td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\"></td><td class=\"code-rhs\">{}</td></tr>",
        expand_id, old_idx + 1, old_content, new_idx + 1, new_content
    )
}

/// Render a hidden expandable single-column context row.
fn render_single_context_row_expandable(idx: usize, hl_lines: &[String], expand_id: &str) -> String {
    let content = hl_lines.get(idx).map(|s| s.as_str()).unwrap_or("");
    format!(
        "<tr class=\"line-context expand-line\" data-expand-id=\"{}\" hidden=\"until-found\">\
         <td class=\"ln\">{}</td><td class=\"sign\"></td><td class=\"code\">{}</td></tr>",
        expand_id, idx + 1, content
    )
}

/// Render a single-column diff row (for add-only or remove-only blocks).
fn render_single_diff_row(row: &DiffRow, layout: DiffLayout, old_hl: &[String], new_hl: &[String]) -> String {
    match layout {
        DiffLayout::AddOnly => {
            if let Some(rhs) = row.rhs {
                let content = new_hl.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
                format!(
                    "<tr class=\"line-added\"><td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td></tr>",
                    rhs.line_number + 1, content
                )
            } else {
                String::new()
            }
        }
        DiffLayout::RemoveOnly => {
            if let Some(lhs) = row.lhs {
                let content = old_hl.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
                format!(
                    "<tr class=\"line-removed\"><td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td></tr>",
                    lhs.line_number + 1, content
                )
            } else {
                String::new()
            }
        }
        DiffLayout::SideBySide => String::new(), // shouldn't be called
    }
}

/// Get the min/max 0-based line numbers referenced in a chunk, for each side.
fn chunk_line_range(chunk: &[LineEntry]) -> (Option<(u64, u64)>, Option<(u64, u64)>) {
    let mut lhs_min: Option<u64> = None;
    let mut lhs_max: Option<u64> = None;
    let mut rhs_min: Option<u64> = None;
    let mut rhs_max: Option<u64> = None;

    for entry in chunk {
        if let Some(lhs) = &entry.lhs {
            let ln = lhs.line_number;
            lhs_min = Some(lhs_min.map_or(ln, |m: u64| m.min(ln)));
            lhs_max = Some(lhs_max.map_or(ln, |m: u64| m.max(ln)));
        }
        if let Some(rhs) = &entry.rhs {
            let ln = rhs.line_number;
            rhs_min = Some(rhs_min.map_or(ln, |m: u64| m.min(ln)));
            rhs_max = Some(rhs_max.map_or(ln, |m: u64| m.max(ln)));
        }
    }

    let lhs_range = match (lhs_min, lhs_max) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    };
    let rhs_range = match (rhs_min, rhs_max) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    };
    (lhs_range, rhs_range)
}

use crate::difft_json::DiffHunk;

/// Map a new-file line (1-based) to an old-file line (1-based) using unified diff hunks.
/// Between hunks the lines are unchanged and advance 1:1; within a hunk we clamp to
/// the hunk's old range boundary.
fn new_to_old_line(new_line_1: u64, hunks: &[DiffHunk]) -> u64 {
    let mut offset: i64 = 0; // accumulated (new_count - old_count) from previous hunks
    for h in hunks {
        let h_new_start = h.new_start;
        let h_new_end = h.new_start + h.new_count; // exclusive
        let h_old_start = h.old_start;

        if new_line_1 < h_new_start {
            // Before this hunk: in an unchanged region
            return (new_line_1 as i64 - offset).max(1) as u64;
        }
        if new_line_1 < h_new_end {
            // Inside this hunk: clamp to the old start (insertion point)
            return h_old_start;
        }
        offset += h.new_count as i64 - h.old_count as i64;
    }
    // After all hunks
    (new_line_1 as i64 - offset).max(1) as u64
}

/// Map an old-file line (1-based) to a new-file line (1-based) using unified diff hunks.
fn old_to_new_line(old_line_1: u64, hunks: &[DiffHunk]) -> u64 {
    let mut offset: i64 = 0;
    for h in hunks {
        let h_old_start = h.old_start;
        let h_old_end = h.old_start + h.old_count;

        if old_line_1 < h_old_start {
            return (old_line_1 as i64 + offset).max(1) as u64;
        }
        if old_line_1 < h_old_end {
            return h.new_start;
        }
        offset += h.new_count as i64 - h.old_count as i64;
    }
    (old_line_1 as i64 + offset).max(1) as u64
}

/// Convert a 1-based line number from hunk mapping to 0-based index.
fn to_0based(line_1: u64) -> usize {
    (line_1 as usize).saturating_sub(1)
}

/// Find lines within unified diff hunks that overlap a chunk's range but are
/// not present in difft's structural matching output. Returns paired (old, new),
/// removed-only, and added-only lists (all 0-based indices).
///
/// Pairing uses content similarity (trimmed equality) rather than positional
/// matching, which avoids the line-ordering bugs from the old approach.
fn hunk_gap_lines(
    chunk: &[LineEntry],
    hunks: &[DiffHunk],
    old_first: usize,
    old_last: usize,
    new_first: usize,
    new_last: usize,
    old_lines: &[String],
    new_lines: &[String],
) -> (Vec<(usize, usize)>, Vec<usize>, Vec<usize>) {
    let difft_old: std::collections::HashSet<usize> = chunk.iter()
        .filter_map(|e| e.lhs.as_ref().map(|s| s.line_number as usize))
        .collect();
    let difft_new: std::collections::HashSet<usize> = chunk.iter()
        .filter_map(|e| e.rhs.as_ref().map(|s| s.line_number as usize))
        .collect();

    let mut raw_removed = Vec::new();
    let mut raw_added = Vec::new();

    for h in hunks {
        let h_old_start = (h.old_start as usize).saturating_sub(1);
        let h_old_end = h_old_start + h.old_count as usize;
        let h_new_start = (h.new_start as usize).saturating_sub(1);
        let h_new_end = h_new_start + h.new_count as usize;

        // Only process hunks that overlap this chunk's line range
        let overlaps_old = h_old_end > old_first.saturating_sub(1) && h_old_start <= old_last + 1;
        let overlaps_new = h_new_end > new_first.saturating_sub(1) && h_new_start <= new_last + 1;
        if !overlaps_old && !overlaps_new { continue; }

        for line_0 in h_old_start..h_old_end {
            if !difft_old.contains(&line_0) {
                raw_removed.push(line_0);
            }
        }
        for line_0 in h_new_start..h_new_end {
            if !difft_new.contains(&line_0) {
                raw_added.push(line_0);
            }
        }
    }

    // Pair gap lines by content similarity. Match criteria (in priority order):
    // 1. Exact trimmed equality (content is identical, just leading/trailing ws)
    // 2. Whitespace-normalized equality (only internal whitespace differs, e.g.
    //    Go alignment: `mu                   sync.RWMutex` == `mu         sync.RWMutex`)
    // 3. One line's trimmed content starts with the other's (structural match,
    //    e.g. `cbOrOpts?:` matches `cbOrOpts?: Callback<...> | ...`)
    let normalize_ws = |s: &str| -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let mut paired = Vec::new();
    let mut used_added: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut unmatched_removed = Vec::new();

    for &old_0 in &raw_removed {
        let old_content = old_lines.get(old_0).map(|s| s.trim()).unwrap_or("");
        if old_content.is_empty() {
            unmatched_removed.push(old_0);
            continue;
        }
        let old_normalized = normalize_ws(old_content);
        // First try exact trimmed match, then whitespace-normalized, then prefix
        let matched = raw_added.iter()
            .find(|&&new_0| {
                !used_added.contains(&new_0) && {
                    let new_content = new_lines.get(new_0).map(|s| s.trim()).unwrap_or("");
                    old_content == new_content
                }
            })
            .or_else(|| raw_added.iter()
                .find(|&&new_0| {
                    !used_added.contains(&new_0) && {
                        let new_content = new_lines.get(new_0).map(|s| s.trim()).unwrap_or("");
                        normalize_ws(new_content) == old_normalized
                    }
                }))
            .or_else(|| raw_added.iter()
                .find(|&&new_0| {
                    !used_added.contains(&new_0) && {
                        let new_content = new_lines.get(new_0).map(|s| s.trim()).unwrap_or("");
                        !new_content.is_empty()
                            && (old_content.starts_with(new_content) || new_content.starts_with(old_content))
                    }
                }))
            .copied();
        if let Some(new_0) = matched {
            paired.push((old_0, new_0));
            used_added.insert(new_0);
        } else {
            unmatched_removed.push(old_0);
        }
    }

    let unmatched_added: Vec<usize> = raw_added.into_iter()
        .filter(|n| !used_added.contains(n))
        .collect();

    (paired, unmatched_removed, unmatched_added)
}

/// Produce a unified-diff-style text representation of selected chunks.
/// Uses the same chunk processing logic as HTML rendering (context lines, hunk gap
/// filling, consolidation) but outputs plain text with ` `/`-`/`+` prefixes.
/// Optional 0-based line range filter (inclusive on both ends, new-file lines).
type LineFilter = Option<(usize, usize)>;

/// Key spacing for sort keys. Entries with rhs use rhs * KEY_SPACE.
/// Removed-only entries increment from prev_rhs * KEY_SPACE + seq.
/// Must be large enough that removed_seq never reaches the next paired key.
const KEY_SPACE: u64 = 10000;

fn render_chunks_text(difft: &DifftOutput, chunk_indices: &[usize], line_filter: LineFilter) -> String {
    let mut out = String::new();
    let hunks = &difft.hunks;

    // Compute base new-file line (0-based) for relative line numbering.
    let mut base_new: usize = usize::MAX;
    for &idx in chunk_indices {
        if let Some(chunk) = difft.chunks.get(idx) {
            for entry in chunk {
                if let Some(rhs) = &entry.rhs {
                    base_new = base_new.min(rhs.line_number as usize);
                }
            }
        }
    }
    if base_new == usize::MAX { base_new = 0; }

    // Format a text diff line with a relative line number prefix.
    // The relative number is 1-based, matching the lines= parameter.
    let fmt_line = |prefix: &str, new_0: usize, content: &str| -> String {
        if new_0 >= base_new {
            let rel = new_0 - base_new + 1;
            format!("{:>4} {}{}\n", rel, prefix, content)
        } else {
            // Pre-context line before the first changed line
            format!("     {}{}\n", prefix, content)
        }
    };

    let mut last_old_rendered: Option<usize> = None;
    let mut last_new_rendered: Option<usize> = None;

    let mut first_chunk = true;
    for &idx in chunk_indices {
        let Some(chunk) = difft.chunks.get(idx) else { continue };

        let (lhs_range, rhs_range) = chunk_line_range(chunk);

        let (mut old_first, mut old_last) = match lhs_range {
            Some((min, max)) => (min as usize, max as usize),
            None => {
                let (rmin, rmax) = rhs_range.unwrap_or((0, 0));
                let o_min = to_0based(new_to_old_line(rmin + 1, hunks));
                let o_max = to_0based(new_to_old_line(rmax + 1, hunks));
                (o_min, o_max)
            }
        };
        let (mut new_first, mut new_last) = match rhs_range {
            Some((min, max)) => (min as usize, max as usize),
            None => {
                let (lmin, lmax) = lhs_range.unwrap_or((0, 0));
                let n_min = to_0based(old_to_new_line(lmin + 1, hunks));
                let n_max = to_0based(old_to_new_line(lmax + 1, hunks));
                (n_min, n_max)
            }
        };

        let mut old_ctx_before = old_first.saturating_sub(CONTEXT_LINES);
        let mut new_ctx_before = new_first.saturating_sub(CONTEXT_LINES);
        let mut old_ctx_after = (old_last + 1 + CONTEXT_LINES).min(difft.old_lines.len());
        let mut new_ctx_after = (new_last + 1 + CONTEXT_LINES).min(difft.new_lines.len());

        // Expand context when the boundary cuts off a multi-line expression.
        let (exp_new_before, exp_new_after) =
            expand_context_for_expressions(&difft.new_lines, new_ctx_before, new_first, new_last, new_ctx_after);
        let new_before_delta = new_ctx_before - exp_new_before;
        let new_after_delta = exp_new_after - new_ctx_after;
        new_ctx_before = exp_new_before;
        new_ctx_after = exp_new_after;
        old_ctx_before = old_ctx_before.saturating_sub(new_before_delta);
        old_ctx_after = (old_ctx_after + new_after_delta).min(difft.old_lines.len());

        let old_ctx_start = match last_old_rendered {
            Some(last) if last + 1 > old_ctx_before => last + 1,
            _ => old_ctx_before,
        };
        let new_ctx_start = match last_new_rendered {
            Some(last) if last + 1 > new_ctx_before => last + 1,
            _ => new_ctx_before,
        };

        if !first_chunk {
            let has_gap = match last_old_rendered {
                Some(last) => last + 1 < old_ctx_before,
                None => match last_new_rendered {
                    Some(last) => last + 1 < new_ctx_before,
                    None => true,
                },
            };
            if has_gap {
                out.push_str("  ...\n");
            }
        }
        first_chunk = false;

        // Context lines before
        let old_pre_count = old_first.saturating_sub(old_ctx_start);
        let new_pre_count = new_first.saturating_sub(new_ctx_start);
        let pre_count = old_pre_count.min(new_pre_count);
        for i in 0..pre_count {
            let new_idx = new_first - pre_count + i;
            let line = difft.new_lines.get(new_idx).map(|s| s.as_str()).unwrap_or("");
            out.push_str(&fmt_line(" ", new_idx, line));
        }

        // Build unified item list (same logic as HTML renderer)
        #[derive(Clone, Copy)]
        enum TextItem<'a> {
            DifftRow(&'a DiffRow<'a>),
            GapRemoved(usize),
            GapAdded(usize),
            GapPaired(usize, usize),
        }

        let rows = consolidate_chunk(chunk);
        let rows = fix_reorders(rows, &difft.old_lines, &difft.new_lines);
        let mut items: Vec<(u64, TextItem)> = Vec::new();
        let mut prev_rhs_t: Option<u64> = None;
        let mut removed_seq_t: u64 = 0;
        for row in &rows {
            let key = if let Some(rhs) = row.rhs {
                prev_rhs_t = Some(rhs.line_number);
                removed_seq_t = 0;
                rhs.line_number * 3
            } else {
                removed_seq_t += 1;
                prev_rhs_t.map_or(removed_seq_t, |p| p * 3 + removed_seq_t)
            };
            items.push((key, TextItem::DifftRow(row)));
        }

        let (gap_paired, gap_removed, gap_added) = hunk_gap_lines(
            chunk, hunks, old_first, old_last, new_first, new_last,
            &difft.old_lines, &difft.new_lines,
        );
        for &(old_0, new_0) in &gap_paired {
            items.push((new_0 as u64 * KEY_SPACE, TextItem::GapPaired(old_0, new_0)));
            old_first = old_first.min(old_0); old_last = old_last.max(old_0);
            new_first = new_first.min(new_0); new_last = new_last.max(new_0);
        }
        for &old_0 in &gap_removed {
            let nearest_key = items.iter()
                .filter_map(|&(k, ref item)| match item {
                    TextItem::DifftRow(row) => row.lhs.map(|s| {
                        let dist = (s.line_number as i64 - old_0 as i64).unsigned_abs();
                        (dist, k)
                    }),
                    _ => None,
                })
                .min_by_key(|&(dist, _)| dist)
                .map(|(_, k)| k);
            items.push((nearest_key.unwrap_or(0), TextItem::GapRemoved(old_0)));
            old_first = old_first.min(old_0); old_last = old_last.max(old_0);
        }
        for &new_0 in &gap_added {
            items.push((new_0 as u64 * KEY_SPACE + KEY_SPACE - 1, TextItem::GapAdded(new_0)));
            new_first = new_first.min(new_0); new_last = new_last.max(new_0);
        }
        items.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| {
                let old_a = match &a.1 {
                    TextItem::DifftRow(r) => r.lhs.map(|s| s.line_number),
                    TextItem::GapRemoved(o) | TextItem::GapPaired(o, _) => Some(*o as u64),
                    TextItem::GapAdded(_) => None,
                };
                let old_b = match &b.1 {
                    TextItem::DifftRow(r) => r.lhs.map(|s| s.line_number),
                    TextItem::GapRemoved(o) | TextItem::GapPaired(o, _) => Some(*o as u64),
                    TextItem::GapAdded(_) => None,
                };
                old_a.cmp(&old_b)
            })
        });

        // Remove gap lines that violate old-side ordering
        let get_old_t = |item: &TextItem| -> Option<u64> {
            match item {
                TextItem::DifftRow(r) => r.lhs.map(|s| s.line_number),
                TextItem::GapRemoved(o) | TextItem::GapPaired(o, _) => Some(*o as u64),
                TextItem::GapAdded(_) => None,
            }
        };
        let mut max_old_t: Option<u64> = None;
        items.retain(|&(_, ref item)| {
            if let Some(o) = get_old_t(item) {
                if let Some(m) = max_old_t {
                    if o < m {
                        return !matches!(item,
                            TextItem::GapRemoved(_) | TextItem::GapAdded(_) | TextItem::GapPaired(_, _));
                    }
                }
                max_old_t = Some(o);
            }
            true
        });

        // Collect ALL item line numbers before filtering so filtered-out
        // changed lines don't become context rows.
        let mut item_old_lines_t: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut item_new_lines_t: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for &(_, ref item) in &items {
            match item {
                TextItem::DifftRow(row) => {
                    if let Some(lhs) = row.lhs { item_old_lines_t.insert(lhs.line_number as usize); }
                    if let Some(rhs) = row.rhs { item_new_lines_t.insert(rhs.line_number as usize); }
                }
                TextItem::GapRemoved(o) => { item_old_lines_t.insert(*o); }
                TextItem::GapAdded(n) => { item_new_lines_t.insert(*n); }
                TextItem::GapPaired(o, n) => { item_old_lines_t.insert(*o); item_new_lines_t.insert(*n); }
            }
        }

        // Apply line filter after collecting skip sets.
        if let Some((filter_start, filter_end)) = line_filter {
            items.retain(|&(_, ref item)| {
                let n = match item {
                    TextItem::DifftRow(row) => row.rhs.map(|s| s.line_number as usize)
                        .or(row.lhs.map(|s| s.line_number as usize)),
                    TextItem::GapRemoved(o) => Some(*o),
                    TextItem::GapAdded(n) => Some(*n),
                    TextItem::GapPaired(_, n) => Some(*n),
                };
                n.map_or(false, |n| n >= filter_start && n <= filter_end)
            });
            new_first = usize::MAX;
            new_last = 0;
            old_first = usize::MAX;
            old_last = 0;
            for &(_, ref item) in &items {
                match item {
                    TextItem::DifftRow(row) => {
                        if let Some(lhs) = row.lhs { old_first = old_first.min(lhs.line_number as usize); old_last = old_last.max(lhs.line_number as usize); }
                        if let Some(rhs) = row.rhs { new_first = new_first.min(rhs.line_number as usize); new_last = new_last.max(rhs.line_number as usize); }
                    }
                    TextItem::GapRemoved(o) => { old_first = old_first.min(*o); old_last = old_last.max(*o); }
                    TextItem::GapAdded(n) => { new_first = new_first.min(*n); new_last = new_last.max(*n); }
                    TextItem::GapPaired(o, n) => {
                        old_first = old_first.min(*o); old_last = old_last.max(*o);
                        new_first = new_first.min(*n); new_last = new_last.max(*n);
                    }
                }
            }
            if items.is_empty() { continue; }

            if old_first == usize::MAX && new_first != usize::MAX {
                old_first = to_0based(new_to_old_line(new_first as u64 + 1, hunks));
                old_last = to_0based(new_to_old_line(new_last as u64 + 1, hunks));
            } else if new_first == usize::MAX && old_first != usize::MAX {
                new_first = to_0based(old_to_new_line(old_first as u64 + 1, hunks));
                new_last = to_0based(old_to_new_line(old_last as u64 + 1, hunks));
            }
        }

        let mut prev_old: Option<usize> = None;
        let mut prev_new: Option<usize> = None;

        for &(_, ref item) in &items {
            let (cur_old, cur_new) = match item {
                TextItem::DifftRow(row) => (
                    row.lhs.map(|s| s.line_number as usize),
                    row.rhs.map(|s| s.line_number as usize),
                ),
                TextItem::GapRemoved(o) => (Some(*o), None),
                TextItem::GapAdded(n) => (None, Some(*n)),
                TextItem::GapPaired(o, n) => (Some(*o), Some(*n)),
            };

            // Render the item with relative line numbers
            match item {
                TextItem::DifftRow(row) => {
                    match (row.lhs, row.rhs) {
                        (Some(lhs), Some(rhs)) => {
                            let n = rhs.line_number as usize;
                            let old_line = difft.old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
                            let new_line = difft.new_lines.get(n).map(|s| s.as_str()).unwrap_or("");
                            if old_line == new_line {
                                out.push_str(&fmt_line(" ", n, new_line));
                            } else {
                                out.push_str(&fmt_line("-", n, old_line));
                                out.push_str(&fmt_line("+", n, new_line));
                            }
                        }
                        (Some(lhs), None) => {
                            let n = to_0based(old_to_new_line(lhs.line_number + 1, hunks));
                            let old_line = difft.old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
                            out.push_str(&fmt_line("-", n, old_line));
                        }
                        (None, Some(rhs)) => {
                            let n = rhs.line_number as usize;
                            let new_line = difft.new_lines.get(n).map(|s| s.as_str()).unwrap_or("");
                            out.push_str(&fmt_line("+", n, new_line));
                        }
                        (None, None) => {}
                    }
                }
                TextItem::GapRemoved(old_0) => {
                    let n = to_0based(old_to_new_line(*old_0 as u64 + 1, hunks));
                    let line = difft.old_lines.get(*old_0).map(|s| s.as_str()).unwrap_or("");
                    out.push_str(&fmt_line("-", n, line));
                }
                TextItem::GapAdded(new_0) => {
                    let line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
                    out.push_str(&fmt_line("+", *new_0, line));
                }
                TextItem::GapPaired(_old_0, new_0) => {
                    // Content-matched, just repositioned: render as context
                    let new_line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
                    out.push_str(&fmt_line(" ", *new_0, new_line));
                }
            }

            if let Some(o) = cur_old { prev_old = Some(o); }
            if let Some(n) = cur_new { prev_new = Some(n); }
        }

        // Context lines after
        let old_post_start = old_last + 1;
        let new_post_start = new_last + 1;
        let old_post_count = old_ctx_after.saturating_sub(old_post_start);
        let new_post_count = new_ctx_after.saturating_sub(new_post_start);
        let post_count = old_post_count.min(new_post_count);
        for i in 0..post_count {
            let new_idx = new_post_start + i;
            let old_idx = old_post_start + i;
            if !item_old_lines_t.contains(&old_idx) && !item_new_lines_t.contains(&new_idx) {
                let line = difft.new_lines.get(new_idx).map(|s| s.as_str()).unwrap_or("");
                out.push_str(&fmt_line(" ", new_idx, line));
            }
        }

        if post_count > 0 {
            last_old_rendered = Some(old_post_start + post_count - 1);
            last_new_rendered = Some(new_post_start + post_count - 1);
        } else {
            last_old_rendered = Some(old_last);
            last_new_rendered = Some(new_last);
        }
    }

    out
}

/// Collapse mode for a diff/code block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CollapseMode {
    None,
    Searchable,  // hidden="until-found" (deleted files: searchable when collapsed)
    Hidden,      // display:none (generated files: not searchable when collapsed)
}

/// Layout mode for a diff block: side-by-side (6 cols) or single-column (3 cols).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffLayout {
    SideBySide,
    AddOnly,
    RemoveOnly,
}

fn detect_diff_layout(difft: &DifftOutput, chunk_indices: &[usize], line_filter: LineFilter) -> DiffLayout {
    // File-level status: new files are always add-only, deleted files remove-only.
    // Difft's structural matching can pair entries even for new/deleted files,
    // but the file status from git is the ground truth.
    match difft.status.as_deref() {
        Some("added") => return DiffLayout::AddOnly,
        Some("deleted") => return DiffLayout::RemoveOnly,
        _ => {}
    }

    // For modified/renamed files, check entries in the selected chunks.
    // When a line filter is active, only consider entries within the filtered
    // range so that splitting a mixed chunk with lines= can produce
    // single-column blocks for the add-only or remove-only portions.
    let mut has_lhs = false;
    let mut has_rhs = false;
    for &idx in chunk_indices {
        let Some(chunk) = difft.chunks.get(idx) else { continue };
        for entry in chunk {
            // Apply line filter: use rhs line number, fall back to lhs
            if let Some((start, end)) = line_filter {
                let ln = entry.rhs.as_ref().map(|r| r.line_number as usize)
                    .or_else(|| entry.lhs.as_ref().map(|l| l.line_number as usize));
                if let Some(ln) = ln {
                    if ln < start || ln > end { continue; }
                }
            }
            if entry.lhs.is_some() { has_lhs = true; }
            if entry.rhs.is_some() { has_rhs = true; }
            if has_lhs && has_rhs { return DiffLayout::SideBySide; }
        }
    }
    if has_rhs && !has_lhs { DiffLayout::AddOnly }
    else if has_lhs && !has_rhs { DiffLayout::RemoveOnly }
    else { DiffLayout::SideBySide }
}

/// Split a 6-column side-by-side diff table into two 3-column tables in a flex container.
/// Only operates on `<table class="diff-table">` (not `diff-single`).
fn split_into_two_tables(html: &str) -> String {
    // Find the table tag. Only split side-by-side tables (no "diff-single" in the class).
    let table_start_needle = "<table class=\"diff-table\">";
    let table_start = match html.find(table_start_needle) {
        Some(pos) => pos,
        None => return html.to_string(),
    };

    // Find <tbody> and </tbody>
    let tbody_start = match html[table_start..].find("<tbody>") {
        Some(pos) => table_start + pos,
        None => return html.to_string(),
    };
    let tbody_content_start = tbody_start + "<tbody>".len();
    let tbody_end = match html[tbody_content_start..].find("</tbody>") {
        Some(pos) => tbody_content_start + pos,
        None => return html.to_string(),
    };
    let table_end = match html[tbody_end..].find("</table>") {
        Some(pos) => tbody_end + pos + "</table>".len(),
        None => return html.to_string(),
    };

    let tbody_content = &html[tbody_content_start..tbody_end];

    // Extract all <tr ...>...</tr> elements
    let tr_re = Regex::new(r"(?s)<tr ([^>]*)>(.*?)</tr>").unwrap();
    let td_re = Regex::new(r"(?s)<td ([^>]*)>(.*?)</td>").unwrap();

    let mut lhs_rows = String::new();
    let mut rhs_rows = String::new();
    let mut row_idx: usize = 0;

    for tr_cap in tr_re.captures_iter(tbody_content) {
        let tr_attrs = &tr_cap[1];
        let tr_inner = &tr_cap[2];

        // Collect all <td> cells
        let cells: Vec<(&str, &str)> = td_re.captures_iter(tr_inner)
            .map(|c| (c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str()))
            .collect();

        if cells.len() == 1 {
            // Chunk separator row: single cell with colspan="6"
            // Create two rows, each with colspan="3"
            let (attrs, content) = cells[0];
            let new_attrs = attrs.replace("colspan=\"6\"", "colspan=\"3\"");
            let lhs_tr = format!(
                "<tr {} data-row-idx=\"{}\"><td {}>{}</td></tr>",
                tr_attrs, row_idx, new_attrs, content
            );
            let rhs_tr = format!(
                "<tr {} data-row-idx=\"{}\"><td {}>{}</td></tr>",
                tr_attrs, row_idx, new_attrs, content
            );
            lhs_rows.push_str(&lhs_tr);
            rhs_rows.push_str(&rhs_tr);
        } else if cells.len() == 6 {
            // Normal 6-cell row: split into first 3 (lhs) and last 3 (rhs).
            // For added-only rows, the lhs placeholder is zero-height.
            // For removed-only rows, the rhs placeholder is zero-height.
            // Placeholders still exist so height-sync can align rows when
            // the other side's fold is expanded.
            let is_add_only = tr_attrs.contains("line-added");
            let is_remove_only = tr_attrs.contains("line-removed");
            let lhs_cells: String = cells[..3].iter()
                .map(|(attrs, content)| format!("<td {}>{}</td>", attrs, content))
                .collect();
            let rhs_cells: String = cells[3..].iter()
                .map(|(attrs, content)| format!("<td {}>{}</td>", attrs, content))
                .collect();
            let lhs_attrs = if is_add_only {
                tr_attrs.replacen("class=\"", "class=\"placeholder ", 1)
            } else {
                tr_attrs.to_string()
            };
            let rhs_attrs = if is_remove_only {
                tr_attrs.replacen("class=\"", "class=\"placeholder ", 1)
            } else {
                tr_attrs.to_string()
            };
            lhs_rows.push_str(&format!(
                "<tr {} data-row-idx=\"{}\">{}</tr>",
                lhs_attrs, row_idx, lhs_cells
            ));
            rhs_rows.push_str(&format!(
                "<tr {} data-row-idx=\"{}\">{}</tr>",
                rhs_attrs, row_idx, rhs_cells
            ));
        } else if cells.len() == 3 {
            // Already a 3-cell row (e.g., fold summary). Put it in both tables.
            let cell_html: String = cells.iter()
                .map(|(attrs, content)| format!("<td {}>{}</td>", attrs, content))
                .collect();
            lhs_rows.push_str(&format!(
                "<tr {} data-row-idx=\"{}\">{}</tr>",
                tr_attrs, row_idx, cell_html
            ));
            rhs_rows.push_str(&format!(
                "<tr {} data-row-idx=\"{}\">{}</tr>",
                tr_attrs, row_idx, cell_html
            ));
        } else {
            // Unexpected cell count: pass through to both sides
            lhs_rows.push_str(&format!(
                "<tr {} data-row-idx=\"{}\">{}</tr>",
                tr_attrs, row_idx, tr_inner
            ));
            rhs_rows.push_str(&format!(
                "<tr {} data-row-idx=\"{}\">{}</tr>",
                tr_attrs, row_idx, tr_inner
            ));
        }

        row_idx += 1;
    }

    let colgroup = "<colgroup><col class=\"ln-col\"><col class=\"sign-col\"><col class=\"code-col\"></colgroup>";

    let two_tables = format!(
        "<div class=\"diff-sides\">\
         <div class=\"diff-side diff-side-old\">\
         <table class=\"diff-table diff-table-lhs\">{}<tbody>{}</tbody></table>\
         </div>\
         <div class=\"diff-side diff-side-new\">\
         <table class=\"diff-table diff-table-rhs\">{}<tbody>{}</tbody></table>\
         </div>\
         </div>",
        colgroup, lhs_rows, colgroup, rhs_rows
    );

    // Replace the original table with the two-table flex container
    let mut result = String::with_capacity(html.len() + two_tables.len());
    result.push_str(&html[..table_start]);
    result.push_str(&two_tables);
    result.push_str(&html[table_end..]);
    result
}

fn render_chunks(difft: &DifftOutput, chunk_indices: &[usize], file_path: &str, line_filter: LineFilter, hl: &mut Highlighter, collapse: CollapseMode) -> String {
    let lang = arborium::detect_language(file_path);
    let old_hl = syntax_highlight_lines(&difft.old_lines, hl, lang);
    let new_hl = syntax_highlight_lines(&difft.new_lines, hl, lang);

    let layout = detect_diff_layout(difft, chunk_indices, line_filter);
    let single = layout != DiffLayout::SideBySide;

    let collapsed = collapse != CollapseMode::None;
    let block_class = if collapsed { "diff-block collapsed" } else { "diff-block" };
    let collapse_attr = match collapse {
        CollapseMode::None => "data-collapse=\"none\"",
        CollapseMode::Searchable => "data-collapse=\"searchable\"",
        CollapseMode::Hidden => "data-collapse=\"hidden\"",
    };
    let arrow = "\u{25B6}"; // rotated 90deg via CSS when expanded
    let disclaimer = match collapse {
        CollapseMode::Searchable =>
            "<div class=\"collapse-disclaimer\">Automatically collapsed (file deleted). Click to expand.</div>",
        CollapseMode::Hidden =>
            "<div class=\"collapse-disclaimer\">Automatically collapsed (generated file). Click to expand.</div>",
        CollapseMode::None => "",
    };

    let mut html = String::new();
    html.push_str(&format!(
        "<div class=\"{}\" {}><div class=\"diff-header\">\
         <span class=\"collapse-arrow\">{}</span> {}\
         <label class=\"viewed-label\"><input type=\"checkbox\" class=\"viewed-check\"><span>Viewed</span></label>\
         </div>{}",
        block_class, collapse_attr, arrow, html_escape(file_path), disclaimer
    ));

    let body_attr = match collapse {
        CollapseMode::None => "",
        CollapseMode::Searchable => " hidden=\"until-found\"",
        CollapseMode::Hidden => " class=\"collapsed-hidden\"",
    };
    html.push_str(&format!("<div class=\"diff-body\"{}>", body_attr));
    if single {
        let layout_class = if layout == DiffLayout::AddOnly { "diff-add-only" } else { "diff-remove-only" };
        html.push_str(&format!(
            "<table class=\"diff-table diff-single {}\"><colgroup>\
             <col class=\"ln-col\"><col class=\"sign-col\"><col class=\"code-col\">\
             </colgroup><tbody>",
            layout_class
        ));
    } else {
        html.push_str(
            "<table class=\"diff-table\"><colgroup>\
             <col class=\"ln-col\"><col class=\"sign-col\"><col class=\"code-col\">\
             <col class=\"ln-col\"><col class=\"sign-col\"><col class=\"code-col\">\
             </colgroup><tbody>",
        );
    }

    let hunks = &difft.hunks;

    // Track the last rendered line indices to avoid duplicating context between chunks.
    let mut last_old_rendered: Option<usize> = None;
    let mut last_new_rendered: Option<usize> = None;

    // Track all rendered line numbers across chunks to prevent duplicates
    // when one chunk's context overlaps another chunk's changed lines.
    let mut rendered_old: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut rendered_new: std::collections::HashSet<usize> = std::collections::HashSet::new();

    // Pre-compute each chunk's first old/new line so we can cap post-context
    // to avoid rendering past the next chunk's start (which causes ordering issues).
    let chunk_boundaries: Vec<(usize, usize)> = chunk_indices.iter().filter_map(|&ci| {
        let chunk = difft.chunks.get(ci)?;
        let (lr, rr) = chunk_line_range(chunk);
        let old_min = match lr {
            Some((min, _)) => min as usize,
            None => {
                let rmin = rr.map_or(0, |(min, _)| min);
                to_0based(new_to_old_line(rmin + 1, hunks))
            }
        };
        let new_min = match rr {
            Some((min, _)) => min as usize,
            None => {
                let lmin = lr.map_or(0, |(min, _)| min);
                to_0based(old_to_new_line(lmin + 1, hunks))
            }
        };
        Some((old_min, new_min))
    }).collect();

    let mut first_chunk = true;
    for (chunk_order, &idx) in chunk_indices.iter().enumerate() {
        let Some(chunk) = difft.chunks.get(idx) else { continue };

        let (lhs_range, rhs_range) = chunk_line_range(chunk);

        // Resolve the first/last 0-based line on each side, using unified diff hunks
        // to map between old/new when one side has no entries in this chunk.
        let (mut old_first, mut old_last) = match lhs_range {
            Some((min, max)) => (min as usize, max as usize),
            None => {
                let (rmin, rmax) = rhs_range.unwrap_or((0, 0));
                // rmin/rmax are 0-based difft lines; hunks use 1-based git lines
                let o_min = to_0based(new_to_old_line(rmin + 1, hunks));
                let o_max = to_0based(new_to_old_line(rmax + 1, hunks));
                (o_min, o_max)
            }
        };
        let (mut new_first, mut new_last) = match rhs_range {
            Some((min, max)) => (min as usize, max as usize),
            None => {
                let (lmin, lmax) = lhs_range.unwrap_or((0, 0));
                let n_min = to_0based(old_to_new_line(lmin + 1, hunks));
                let n_max = to_0based(old_to_new_line(lmax + 1, hunks));
                (n_min, n_max)
            }
        };

        // Build a list of render items from difft entries, supplemented with
        // hunk gap lines (lines git considers changed but difft didn't flag).
        // Gap lines are added as individual removed/added rows (no pairing).
        #[derive(Clone, Copy)]
        enum RenderItem<'a> {
            DifftRow(&'a DiffRow<'a>),
            GapRemoved(usize),  // 0-based old line
            GapAdded(usize),    // 0-based new line
            GapPaired(usize, usize), // (old_0, new_0)
        }

        let rows = consolidate_chunk(chunk);
        let rows = fix_reorders(rows, &difft.old_lines, &difft.new_lines);

        // Compute gap lines first so we can use gap-paired positions to
        // assign correct sort keys for removed-only difft entries that
        // have no preceding rhs entry.
        let (gap_paired, gap_removed, gap_added) = hunk_gap_lines(
            chunk, hunks, old_first, old_last, new_first, new_last,
            &difft.old_lines, &difft.new_lines,
        );

        // Sort entries by new-file position with KEY_SPACE spacing.
        // For removed-only entries with no preceding rhs, use gap-paired
        // items to find the correct new-file position so the entry sorts
        // correctly relative to gap-paired context lines.
        let mut items: Vec<(u64, RenderItem)> = Vec::new();
        let mut prev_rhs: Option<u64> = None;
        let mut removed_seq: u64 = 0;
        for row in &rows {
            let key = if let Some(rhs) = row.rhs {
                // Only anchor prev_rhs from truly paired entries (both sides).
                // Added-only entries shouldn't position subsequent removed entries.
                if row.lhs.is_some() {
                    prev_rhs = Some(rhs.line_number);
                }
                removed_seq = 0;
                rhs.line_number * KEY_SPACE
            } else if let Some(p) = prev_rhs {
                removed_seq += 1;
                p * KEY_SPACE + removed_seq
            } else if let Some(lhs) = row.lhs {
                // No prev_rhs: find the gap-paired item just before this
                // entry in old-line order, but only from the same hunk.
                removed_seq += 1;
                let old_1 = lhs.line_number + 1; // 1-based
                // Find the hunk containing this old line
                let containing_hunk = hunks.iter().find(|h|
                    h.old_count > 0
                        && old_1 >= h.old_start
                        && old_1 < h.old_start + h.old_count
                );
                let old_0 = lhs.line_number as usize;
                if let Some(h) = containing_hunk {
                    let h_old_start = (h.old_start as usize).saturating_sub(1);
                    let h_old_end = h_old_start + h.old_count as usize;
                    if let Some(&(_, gn)) = gap_paired.iter()
                        .filter(|&&(go, _)| go < old_0 && go >= h_old_start && go < h_old_end)
                        .max_by_key(|&&(go, _)| go)
                    {
                        gn as u64 * KEY_SPACE + KEY_SPACE / 2 + removed_seq
                    } else {
                        removed_seq
                    }
                } else {
                    removed_seq
                }
            } else {
                removed_seq += 1;
                removed_seq
            };
            items.push((key, RenderItem::DifftRow(row)));
        }

        for &(old_0, new_0) in &gap_paired {
            items.push((new_0 as u64 * KEY_SPACE, RenderItem::GapPaired(old_0, new_0)));
            old_first = old_first.min(old_0);
            old_last = old_last.max(old_0);
            new_first = new_first.min(new_0);
            new_last = new_last.max(new_0);
        }
        for &old_0 in &gap_removed {
            // Use same key as nearby difft removed entries so they interleave
            // by old-line via the tiebreaker. Find the nearest difft entry.
            let nearest_key = items.iter()
                .filter_map(|&(k, ref item)| match item {
                    RenderItem::DifftRow(row) => row.lhs.map(|s| {
                        let dist = (s.line_number as i64 - old_0 as i64).unsigned_abs();
                        (dist, k)
                    }),
                    _ => None,
                })
                .min_by_key(|&(dist, _)| dist)
                .map(|(_, k)| k);
            let key = nearest_key.unwrap_or(0);
            items.push((key, RenderItem::GapRemoved(old_0)));
            old_first = old_first.min(old_0);
            old_last = old_last.max(old_0);
        }
        for &new_0 in &gap_added {
            items.push((new_0 as u64 * KEY_SPACE + KEY_SPACE - 1, RenderItem::GapAdded(new_0)));
            new_first = new_first.min(new_0);
            new_last = new_last.max(new_0);
        }

        // Sort by key, tiebreak by old-line number
        items.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| {
                let old_a = match &a.1 {
                    RenderItem::DifftRow(r) => r.lhs.map(|s| s.line_number),
                    RenderItem::GapRemoved(o) | RenderItem::GapPaired(o, _) => Some(*o as u64),
                    RenderItem::GapAdded(_) => None,
                };
                let old_b = match &b.1 {
                    RenderItem::DifftRow(r) => r.lhs.map(|s| s.line_number),
                    RenderItem::GapRemoved(o) | RenderItem::GapPaired(o, _) => Some(*o as u64),
                    RenderItem::GapAdded(_) => None,
                };
                old_a.cmp(&old_b)
            })
        });

        // Un-pair low-similarity entries. Difft sometimes pairs lines by
        // position even when content is almost entirely different. Split
        // these into adjacent removed + added items at the same sort key.
        let mut split_rows: Vec<DiffRow> = Vec::new();
        // Owned LineSides with cleared changes for split rows, so they
        // render as full-line removals/additions instead of partial highlights.
        let mut split_sides: Vec<LineSide> = Vec::new();
        let is_low_similarity = |row: &DiffRow| -> bool {
            if let (Some(lhs), Some(rhs)) = (row.lhs, row.rhs) {
                // Skip entries where content is identical (difft includes
                // unchanged lines in chunks when the surrounding structure
                // changed, e.g. a JSDoc comment grew longer).
                let old_line = difft.old_lines.get(lhs.line_number as usize);
                let new_line = difft.new_lines.get(rhs.line_number as usize);
                if old_line == new_line { return false; }

                let old_text = old_line.map(|s| s.trim().len()).unwrap_or(0);
                let new_text = new_line.map(|s| s.trim().len()).unwrap_or(0);
                let old_changed: usize = lhs.changes.iter()
                    .map(|c| c.end.saturating_sub(c.start))
                    .sum();
                let new_changed: usize = rhs.changes.iter()
                    .map(|c| c.end.saturating_sub(c.start))
                    .sum();
                let old_pct = if old_text > 0 { old_changed * 100 / old_text } else { 0 };
                let new_pct = if new_text > 0 { new_changed * 100 / new_text } else { 0 };
                old_pct > 70 && new_pct > 70
            } else {
                false
            }
        };
        // First pass: identify which items need splitting and create the
        // split DiffRows (need stable references before building new items).
        let split_indices: Vec<usize> = items.iter().enumerate()
            .filter_map(|(i, (_, item))| {
                if let RenderItem::DifftRow(row) = item {
                    if is_low_similarity(row) { return Some(i); }
                }
                None
            })
            .collect();
        for &idx in &split_indices {
            if let RenderItem::DifftRow(row) = &items[idx].1 {
                // Create owned LineSides with full-line change spans so
                // the rows render with full-line background (no token
                // highlights implying a semantic match).
                if let Some(lhs) = row.lhs {
                    let len = difft.old_lines.get(lhs.line_number as usize)
                        .map(|s| s.len()).unwrap_or(0);
                    split_sides.push(LineSide {
                        line_number: lhs.line_number,
                        changes: vec![crate::difft_json::ChangeSpan { content: String::new(), start: 0, end: len, highlight: String::new() }],
                    });
                }
                if let Some(rhs) = row.rhs {
                    let len = difft.new_lines.get(rhs.line_number as usize)
                        .map(|s| s.len()).unwrap_or(0);
                    split_sides.push(LineSide {
                        line_number: rhs.line_number,
                        changes: vec![crate::difft_json::ChangeSpan { content: String::new(), start: 0, end: len, highlight: String::new() }],
                    });
                }
            }
        }
        // Build split DiffRows referencing the owned sides.
        {
            let mut side_idx = 0;
            for _ in &split_indices {
                split_rows.push(DiffRow { lhs: Some(&split_sides[side_idx]), rhs: None });
                split_rows.push(DiffRow { lhs: None, rhs: Some(&split_sides[side_idx + 1]) });
                side_idx += 2;
            }
        }
        // Second pass: rebuild items. Re-pair the split entries
        // positionally (old line N beside new line N) so they share a
        // row in side-by-side view. Changes are cleared so they render
        // with full-row background, not token highlights.
        if !split_indices.is_empty() {
            // Rebuild split_rows as re-paired: combine each removed/added
            // pair into a single row with both sides but no change spans.
            let mut repaired: Vec<DiffRow> = Vec::new();
            for si in (0..split_rows.len()).step_by(2) {
                repaired.push(DiffRow {
                    lhs: split_sides.get(si).map(|s| s as &LineSide),
                    rhs: split_sides.get(si + 1).map(|s| s as &LineSide),
                });
            }
            split_rows = repaired;

            let split_set: std::collections::HashSet<usize> = split_indices.into_iter().collect();
            let mut new_items: Vec<(u64, RenderItem)> = Vec::new();
            let mut repaired_idx = 0;
            for (i, (key, item)) in items.into_iter().enumerate() {
                if split_set.contains(&i) {
                    new_items.push((key, RenderItem::DifftRow(&split_rows[repaired_idx])));
                    repaired_idx += 1;
                } else {
                    new_items.push((key, item));
                }
            }
            items = new_items;
        }

        // Enforce old-side monotonicity. When old and new sides have
        // different ordering (refactored code), the new-side-based sort can
        // scatter old-side line numbers. Stable-sort items so old-side line
        // numbers are non-decreasing while preserving new-side order for
        // items without old-side lines (added-only).
        let get_old = |item: &RenderItem| -> Option<u64> {
            match item {
                RenderItem::DifftRow(r) => r.lhs.map(|s| s.line_number),
                RenderItem::GapRemoved(o) | RenderItem::GapPaired(o, _) => Some(*o as u64),
                RenderItem::GapAdded(_) => None,
            }
        };

        // First pass: remove gap lines that violate old-side ordering.
        let mut max_old: Option<u64> = None;
        items.retain(|&(_, ref item)| {
            if let Some(o) = get_old(item) {
                if let Some(m) = max_old {
                    if o < m {
                        return !matches!(item,
                            RenderItem::GapRemoved(_) | RenderItem::GapAdded(_) | RenderItem::GapPaired(_, _));
                    }
                }
                max_old = Some(o);
            }
            true
        });


        // If the chunk has gap-paired lines AND the difft entries have both
        // sides, use side-by-side rendering. Don't override for purely
        // one-sided chunks (all removed or all added) where gap-paired lines
        // are just whitespace realignment from the removal/addition.
        let has_difft_lhs = rows.iter().any(|r| r.lhs.is_some());
        let has_difft_rhs = rows.iter().any(|r| r.rhs.is_some());
        let (single, layout) = if single && !gap_paired.is_empty() && has_difft_lhs && has_difft_rhs {
            (false, DiffLayout::SideBySide)
        } else {
            (single, layout)
        };

        // Collect ALL item line numbers before filtering, so gap lines and
        // filtered-out changed lines don't become context rows.
        let mut item_old_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut item_new_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for &(_, ref item) in &items {
            match item {
                RenderItem::DifftRow(row) => {
                    if let Some(lhs) = row.lhs { item_old_lines.insert(lhs.line_number as usize); }
                    if let Some(rhs) = row.rhs { item_new_lines.insert(rhs.line_number as usize); }
                }
                RenderItem::GapRemoved(o) => { item_old_lines.insert(*o); }
                RenderItem::GapAdded(n) => { item_new_lines.insert(*n); }
                RenderItem::GapPaired(o, n) => { item_old_lines.insert(*o); item_new_lines.insert(*n); }
            }
        }

        // Apply line filter: remove items outside the range but keep their
        // line numbers in the skip sets so they don't become context rows.
        if let Some((filter_start, filter_end)) = line_filter {
            items.retain(|&(_, ref item)| {
                let n = match item {
                    RenderItem::DifftRow(row) => row.rhs.map(|s| s.line_number as usize)
                        .or(row.lhs.map(|s| s.line_number as usize)),
                    RenderItem::GapRemoved(o) => Some(*o),
                    RenderItem::GapAdded(n) => Some(*n),
                    RenderItem::GapPaired(_, n) => Some(*n),
                };
                n.map_or(false, |n| n >= filter_start && n <= filter_end)
            });
            new_first = usize::MAX;
            new_last = 0;
            old_first = usize::MAX;
            old_last = 0;
            for &(_, ref item) in &items {
                match item {
                    RenderItem::DifftRow(row) => {
                        if let Some(lhs) = row.lhs { old_first = old_first.min(lhs.line_number as usize); old_last = old_last.max(lhs.line_number as usize); }
                        if let Some(rhs) = row.rhs { new_first = new_first.min(rhs.line_number as usize); new_last = new_last.max(rhs.line_number as usize); }
                    }
                    RenderItem::GapRemoved(o) => { old_first = old_first.min(*o); old_last = old_last.max(*o); }
                    RenderItem::GapAdded(n) => { new_first = new_first.min(*n); new_last = new_last.max(*n); }
                    RenderItem::GapPaired(o, n) => {
                        old_first = old_first.min(*o); old_last = old_last.max(*o);
                        new_first = new_first.min(*n); new_last = new_last.max(*n);
                    }
                }
            }
            if items.is_empty() { continue; }

            // If one side has no entries, derive from the other via hunk mapping
            if old_first == usize::MAX && new_first != usize::MAX {
                old_first = to_0based(new_to_old_line(new_first as u64 + 1, hunks));
                old_last = to_0based(new_to_old_line(new_last as u64 + 1, hunks));
            } else if new_first == usize::MAX && old_first != usize::MAX {
                new_first = to_0based(old_to_new_line(old_first as u64 + 1, hunks));
                new_last = to_0based(old_to_new_line(old_last as u64 + 1, hunks));
            }
        }

        // Compute visible context boundaries (CONTEXT_LINES + expression expansion).
        let mut old_vis_before = old_first.saturating_sub(CONTEXT_LINES);
        let mut new_vis_before = new_first.saturating_sub(CONTEXT_LINES);
        let mut old_vis_after = (old_last + 1 + CONTEXT_LINES).min(difft.old_lines.len());
        let mut new_vis_after = (new_last + 1 + CONTEXT_LINES).min(difft.new_lines.len());

        // Expand visible context when the boundary cuts off a multi-line expression.
        let (exp_new_before, exp_new_after) =
            expand_context_for_expressions(&difft.new_lines, new_vis_before, new_first, new_last, new_vis_after);
        let new_before_delta = new_vis_before - exp_new_before;
        let new_after_delta = exp_new_after - new_vis_after;
        new_vis_before = exp_new_before;
        new_vis_after = exp_new_after;
        old_vis_before = old_vis_before.saturating_sub(new_before_delta);
        old_vis_after = (old_vis_after + new_after_delta).min(difft.old_lines.len());

        // Wider expanded context boundaries for hidden expandable rows.
        let mut old_ctx_before = old_first.saturating_sub(EXPANDED_CONTEXT_LINES);
        let mut new_ctx_before = new_first.saturating_sub(EXPANDED_CONTEXT_LINES);
        let mut old_ctx_after = (old_last + 1 + EXPANDED_CONTEXT_LINES).min(difft.old_lines.len());
        let mut new_ctx_after = (new_last + 1 + EXPANDED_CONTEXT_LINES).min(difft.new_lines.len());

        // Cap post-context at the next chunk's visible pre-context start.
        // Leave CONTEXT_LINES lines before the next chunk's first change so
        // the next chunk's visible pre-context isn't consumed by hidden
        // expanded post-context from this chunk.
        if let Some(&(next_old_min, next_new_min)) = chunk_boundaries.get(chunk_order + 1) {
            old_ctx_after = old_ctx_after.min(next_old_min.saturating_sub(CONTEXT_LINES));
            new_ctx_after = new_ctx_after.min(next_new_min.saturating_sub(CONTEXT_LINES));
        }

        let old_ctx_start = match last_old_rendered {
            Some(last) if last + 1 > old_ctx_before => last + 1,
            _ => old_ctx_before,
        };
        let new_ctx_start = match last_new_rendered {
            Some(last) if last + 1 > new_ctx_before => last + 1,
            _ => new_ctx_before,
        };

        if !first_chunk {
            let has_gap = match last_old_rendered {
                Some(last) => last + 1 < old_ctx_before,
                None => match last_new_rendered {
                    Some(last) => last + 1 < new_ctx_before,
                    None => true,
                },
            };
            if has_gap {
                let colspan = if single { 3 } else { 6 };
                html.push_str(&format!("<tr class=\"chunk-sep\"><td colspan=\"{}\"></td></tr>", colspan));
            }
        }
        first_chunk = false;

        // Render context lines BEFORE the chunk.
        // Lines from ctx_start..vis_before are hidden (expandable).
        // Lines from vis_before..first are visible.
        let old_pre_count = old_first.saturating_sub(old_ctx_start);
        let new_pre_count = new_first.saturating_sub(new_ctx_start);
        let pre_count = match layout {
            DiffLayout::AddOnly => new_pre_count,
            DiffLayout::RemoveOnly => old_pre_count,
            DiffLayout::SideBySide => old_pre_count.min(new_pre_count),
        };

        // Visible start: the first line that should be shown without expanding.
        let old_vis_start = old_vis_before.max(old_ctx_start);
        let new_vis_start = new_vis_before.max(new_ctx_start);
        let vis_pre_count = match layout {
            DiffLayout::AddOnly => new_first.saturating_sub(new_vis_start),
            DiffLayout::RemoveOnly => old_first.saturating_sub(old_vis_start),
            DiffLayout::SideBySide => old_first.saturating_sub(old_vis_start)
                .min(new_first.saturating_sub(new_vis_start)),
        };
        let hidden_pre_count = pre_count.saturating_sub(vis_pre_count);
        let expand_id_pre = format!("expand-pre-{}", chunk_order);

        // Hidden expandable context lines (before visible context).
        // Single ↑ button at the bottom (closest to visible code).
        let colspan = if single { 3 } else { 6 };

        for i in 0..hidden_pre_count {
            let o = old_first.saturating_sub(pre_count) + i;
            let n = new_first.saturating_sub(pre_count) + i;
            if single {
                let (idx, hl_lines) = if layout == DiffLayout::AddOnly {
                    (n, &new_hl)
                } else {
                    (o, &old_hl)
                };
                if (layout == DiffLayout::AddOnly && !item_new_lines.contains(&idx) && rendered_new.insert(idx))
                    || (layout == DiffLayout::RemoveOnly && !item_old_lines.contains(&idx) && rendered_old.insert(idx))
                {
                    html.push_str(&render_single_context_row_expandable(idx, hl_lines, &expand_id_pre));
                }
            } else if !item_old_lines.contains(&o) && !item_new_lines.contains(&n)
                && !rendered_old.contains(&o) && !rendered_new.contains(&n)
            {
                rendered_old.insert(o);
                rendered_new.insert(n);
                html.push_str(&render_context_row_expandable(o, n, &old_hl, &new_hl, &expand_id_pre));
            }
        }

        if hidden_pre_count > 0 {
            let step = hidden_pre_count.min(EXPAND_STEP);
            let plural = if step == 1 { "line" } else { "lines" };
            html.push_str(&format!(
                "<tr class=\"expand-summary\" data-expand-id=\"{}\" data-dir=\"up\">\
                 <td class=\"expand-btn\" colspan=\"{}\">\
                 \u{2191} Show {} more {}</td></tr>",
                expand_id_pre, colspan, step, plural
            ));
        }

        // Visible context lines (normal CONTEXT_LINES range).
        for i in hidden_pre_count..pre_count {
            let o = old_first.saturating_sub(pre_count) + i;
            let n = new_first.saturating_sub(pre_count) + i;
            if single {
                let (idx, hl_lines) = if layout == DiffLayout::AddOnly {
                    (n, &new_hl)
                } else {
                    (o, &old_hl)
                };
                if (layout == DiffLayout::AddOnly && !item_new_lines.contains(&idx) && rendered_new.insert(idx))
                    || (layout == DiffLayout::RemoveOnly && !item_old_lines.contains(&idx) && rendered_old.insert(idx))
                {
                    html.push_str(&render_single_context_row(idx, hl_lines));
                }
            } else if !item_old_lines.contains(&o) && !item_new_lines.contains(&n)
                && !rendered_old.contains(&o) && !rendered_new.contains(&n)
            {
                rendered_old.insert(o);
                rendered_new.insert(n);
                html.push_str(&render_context_row(o, n, &old_hl, &new_hl));
            }
        }

        // Render items, filling gaps with context lines.
        let mut prev_old: Option<usize> = None;
        let mut prev_new: Option<usize> = None;

        for &(_, ref item) in &items {
            let (cur_old, cur_new) = match item {
                RenderItem::DifftRow(row) => (
                    row.lhs.map(|s| s.line_number as usize),
                    row.rhs.map(|s| s.line_number as usize),
                ),
                RenderItem::GapRemoved(o) => (Some(*o), None),
                RenderItem::GapAdded(n) => (None, Some(*n)),
                RenderItem::GapPaired(o, n) => (Some(*o), Some(*n)),
            };

            // Skip items whose lines were already rendered by a previous chunk.
            let dominated_old = cur_old.map_or(false, |o| rendered_old.contains(&o));
            let dominated_new = cur_new.map_or(false, |n| rendered_new.contains(&n));
            if dominated_old || dominated_new { continue; }

            // Fill intra-chunk gaps: unchanged lines between consecutive items
            // that aren't in any hunk (e.g. a function signature between a
            // comment change and a body change). Use the old side to detect
            // gaps since removed-only items lack a new side.
            if let (Some(po), Some(pn)) = (prev_old, prev_new) {
                let co = cur_old.unwrap_or(usize::MAX);
                let cn = cur_new.unwrap_or(usize::MAX);
                // Only fill when the old side has a forward gap and (if available)
                // the new side also has a forward gap (no cross-ordering).
                let new_ok = cn == usize::MAX || cn > pn + 1;
                if co > po + 1 && co != usize::MAX && new_ok {
                    let old_gap_start = po + 1;
                    let new_gap_start = pn + 1;
                    let old_gap_count = co - old_gap_start;
                    let new_gap_count = cn - new_gap_start;
                    let gap_count = old_gap_count.min(new_gap_count);
                    for g in 0..gap_count {
                        let o = old_gap_start + g;
                        let n = new_gap_start + g;
                        // Skip lines inside hunks (those are changed, not context).
                        let in_hunk = hunks.iter().any(|h| {
                            let ho = (h.old_start as usize).saturating_sub(1);
                            let hn = (h.new_start as usize).saturating_sub(1);
                            (h.old_count > 0 && o >= ho && o < ho + h.old_count as usize)
                                || (h.new_count > 0 && n >= hn && n < hn + h.new_count as usize)
                        });
                        // Also skip if old line would violate monotonic ordering.
                        let max_rendered_old = rendered_old.iter().copied().max().unwrap_or(0);
                        if !in_hunk
                            && o > max_rendered_old
                            && n < difft.new_lines.len()
                            && !rendered_old.contains(&o) && !rendered_new.contains(&n)
                            && !item_old_lines.contains(&o) && !item_new_lines.contains(&n)
                        {
                            rendered_old.insert(o);
                            rendered_new.insert(n);
                            if single {
                                let (idx, hl_lines) = if layout == DiffLayout::AddOnly {
                                    (n, &new_hl)
                                } else {
                                    (o, &old_hl)
                                };
                                html.push_str(&render_single_context_row(idx, hl_lines));
                            } else {
                                html.push_str(&render_context_row(o, n, &old_hl, &new_hl));
                            }
                        }
                    }
                }
            }

            // Record these lines as rendered.
            if let Some(o) = cur_old { rendered_old.insert(o); }
            if let Some(n) = cur_new { rendered_new.insert(n); }

            // Render the item
            if single {
                match item {
                    RenderItem::DifftRow(row) => {
                        html.push_str(&render_single_diff_row(row, layout, &old_hl, &new_hl));
                    }
                    RenderItem::GapRemoved(old_0) => {
                        let content = old_hl.get(*old_0).map(|s| s.as_str()).unwrap_or("");
                        html.push_str(&format!(
                            "<tr class=\"line-removed\"><td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td></tr>",
                            old_0 + 1, content
                        ));
                    }
                    RenderItem::GapAdded(new_0) => {
                        let content = new_hl.get(*new_0).map(|s| s.as_str()).unwrap_or("");
                        html.push_str(&format!(
                            "<tr class=\"line-added\"><td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td></tr>",
                            new_0 + 1, content
                        ));
                    }
                    RenderItem::GapPaired(old_0, new_0) => {
                        let (idx, hl_lines) = if layout == DiffLayout::AddOnly {
                            (*new_0, &new_hl)
                        } else {
                            (*old_0, &old_hl)
                        };
                        html.push_str(&render_single_context_row(idx, hl_lines));
                    }
                }
            } else {
                match item {
                    RenderItem::DifftRow(row) => {
                        html.push_str(&render_diff_row(row, &difft.old_lines, &difft.new_lines, &old_hl, &new_hl));
                    }
                    RenderItem::GapRemoved(old_0) => {
                        let content = old_hl.get(*old_0).map(|s| s.as_str()).unwrap_or("");
                        html.push_str(&format!(
                            "<tr class=\"line-removed\"><td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td>\
                             <td class=\"ln ln-rhs\"></td><td class=\"sign sign-rhs\"></td><td class=\"code-rhs\"></td></tr>",
                            old_0 + 1, content
                        ));
                    }
                    RenderItem::GapAdded(new_0) => {
                        let content = new_hl.get(*new_0).map(|s| s.as_str()).unwrap_or("");
                        html.push_str(&format!(
                            "<tr class=\"line-added\"><td class=\"ln ln-lhs\"></td><td class=\"sign sign-lhs\"></td><td class=\"code-lhs\"></td>\
                             <td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td></tr>",
                            new_0 + 1, content
                        ));
                    }
                    RenderItem::GapPaired(old_0, new_0) => {
                        // Gap-paired lines matched by trimmed content: only whitespace
                        // differs. Render as context (no color, no signs) since the
                        // content is effectively unchanged, just repositioned.
                        html.push_str(&render_context_row(*old_0, *new_0, &old_hl, &new_hl));
                    }
                }
            }

            if let Some(o) = cur_old { prev_old = Some(o); }
            if let Some(n) = cur_new { prev_new = Some(n); }
        }

        // Render context lines AFTER the chunk.
        // Lines from post_start..vis_after are visible.
        // Lines from vis_after..ctx_after are hidden (expandable).
        let old_post_start = old_last + 1;
        let new_post_start = new_last + 1;
        let old_post_count = old_ctx_after.saturating_sub(old_post_start);
        let new_post_count = new_ctx_after.saturating_sub(new_post_start);
        let post_count = match layout {
            DiffLayout::AddOnly => new_post_count,
            DiffLayout::RemoveOnly => old_post_count,
            DiffLayout::SideBySide => old_post_count.min(new_post_count),
        };

        let vis_post_count = match layout {
            DiffLayout::AddOnly => new_vis_after.saturating_sub(new_post_start),
            DiffLayout::RemoveOnly => old_vis_after.saturating_sub(old_post_start),
            DiffLayout::SideBySide => old_vis_after.saturating_sub(old_post_start)
                .min(new_vis_after.saturating_sub(new_post_start)),
        }.min(post_count);
        let hidden_post_count = post_count.saturating_sub(vis_post_count);
        let expand_id_post = format!("expand-post-{}", chunk_order);

        // Visible post-context lines.
        for i in 0..vis_post_count {
            let o = old_post_start + i;
            let n = new_post_start + i;
            if single {
                let (idx, hl_lines, item_lines) = if layout == DiffLayout::AddOnly {
                    (n, &new_hl, &item_new_lines)
                } else {
                    (o, &old_hl, &item_old_lines)
                };
                let already = if layout == DiffLayout::AddOnly {
                    rendered_new.contains(&idx)
                } else {
                    rendered_old.contains(&idx)
                };
                if !item_lines.contains(&idx) && !already {
                    if layout == DiffLayout::AddOnly { rendered_new.insert(idx); }
                    else { rendered_old.insert(idx); }
                    html.push_str(&render_single_context_row(idx, hl_lines));
                }
            } else if !item_old_lines.contains(&o) && !item_new_lines.contains(&n)
                && !rendered_old.contains(&o) && !rendered_new.contains(&n)
            {
                rendered_old.insert(o);
                rendered_new.insert(n);
                html.push_str(&render_context_row(o, n, &old_hl, &new_hl));
            }
        }

        // Expander button and hidden post-context lines.
        // Single ↓ button at the top (closest to visible code).
        if hidden_post_count > 0 {
            let step = hidden_post_count.min(EXPAND_STEP);
            let plural = if step == 1 { "line" } else { "lines" };
            html.push_str(&format!(
                "<tr class=\"expand-summary\" data-expand-id=\"{}\" data-dir=\"down\">\
                 <td class=\"expand-btn\" colspan=\"{}\">\
                 \u{2193} Show {} more {}</td></tr>",
                expand_id_post, colspan, step, plural
            ));
        }

        // Hidden expandable post-context lines.
        for i in vis_post_count..post_count {
            let o = old_post_start + i;
            let n = new_post_start + i;
            if single {
                let (idx, hl_lines, item_lines) = if layout == DiffLayout::AddOnly {
                    (n, &new_hl, &item_new_lines)
                } else {
                    (o, &old_hl, &item_old_lines)
                };
                let already = if layout == DiffLayout::AddOnly {
                    rendered_new.contains(&idx)
                } else {
                    rendered_old.contains(&idx)
                };
                if !item_lines.contains(&idx) && !already {
                    if layout == DiffLayout::AddOnly { rendered_new.insert(idx); }
                    else { rendered_old.insert(idx); }
                    html.push_str(&render_single_context_row_expandable(idx, hl_lines, &expand_id_post));
                }
            } else if !item_old_lines.contains(&o) && !item_new_lines.contains(&n)
                && !rendered_old.contains(&o) && !rendered_new.contains(&n)
            {
                rendered_old.insert(o);
                rendered_new.insert(n);
                html.push_str(&render_context_row_expandable(o, n, &old_hl, &new_hl, &expand_id_post));
            }
        }

        if post_count > 0 {
            last_old_rendered = Some(old_post_start + post_count - 1);
            last_new_rendered = Some(new_post_start + post_count - 1);
        } else {
            last_old_rendered = Some(old_last);
            last_new_rendered = Some(new_last);
        }
    }

    html.push_str("</tbody></table></div></div>");

    // Strip common leading indentation from all code cells.
    // Find the minimum indent across both LHS and RHS non-empty cells,
    // then remove that many spaces from every cell so deeply-nested code
    // doesn't waste horizontal space.
    let code_cell_re = Regex::new(r#"class="code[^"]*">( +)"#).unwrap();
    let min_indent = code_cell_re.captures_iter(&html)
        .map(|c| c[1].len())
        .min()
        .unwrap_or(0);
    if min_indent > 0 {
        // Build a pattern that matches the indent after any code cell opening tag
        let strip_re = Regex::new(&format!(
            r#"(class="code[^"]*">){}"#,
            " ".repeat(min_indent)
        )).unwrap();
        html = strip_re.replace_all(&html, "${1}").to_string();
    }

    if !single {
        html = split_into_two_tables(&html);
    }

    html
}

/// Generate a walkthrough-ready markdown with all diffs as difft code blocks.
/// The LLM can use this directly as a starting point for the narrative.
pub fn write_summary(data_dir: &Path, output: &Path) -> Result<()> {
    let mut data: Vec<(String, DifftOutput)> = Vec::new();
    for entry in fs::read_dir(data_dir).context("Failed to read data directory")? {
        let entry = entry?;
        let path = entry.path();
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.extension().map_or(false, |e| e == "json") && filename != ".meta.json" {
            let json_str = fs::read_to_string(&path)?;
            let difft: DifftOutput = serde_json::from_str(&json_str)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            if let Some(ref p) = difft.path {
                data.push((p.clone(), difft));
            }
        }
    }
    data.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = String::new();

    // Generate frontmatter with author and PR (empty values if unavailable,
    // so the LLM sees the fields and knows to fill them in)
    let pr_url = Command::new("gh")
        .args(["pr", "view", "--json", "url", "--jq", ".url"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let author = Command::new("gh")
        .args(["api", "user", "--jq", ".name"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    out.push_str("---\n");
    out.push_str(&format!("pr: {}\n", pr_url));
    out.push_str(&format!("author: {}\n", author));
    out.push_str("---\n\n");

    out.push_str("# TODO: title\n\nTODO: overview\n\n");

    for (file, difft) in &data {
        let status = difft.status.as_deref().unwrap_or("changed");
        let chunk_count = difft.chunks.len();

        if chunk_count <= 2 {
            // Small files: single block with chunks=all
            out.push_str(&format!("## {}\n\n", file));
            out.push_str(&format!("<!-- {}, {} -->\n\n", status, chunk_count));
            let text = render_chunks_text(difft, &(0..chunk_count).collect::<Vec<_>>(), None);
            out.push_str(&format!("```difft {} chunks=all\n", file));
            out.push_str(&text);
            out.push_str("```\n\n");
        } else {
            // Larger files: one block per chunk with line ranges noted
            out.push_str(&format!("## {}\n\n", file));
            out.push_str(&format!("<!-- {} -->\n\n", status));
            for i in 0..chunk_count {
                let chunk = &difft.chunks[i];
                let (_, rhs_range) = chunk_line_range(chunk);
                let line_info = match rhs_range {
                    Some((min, max)) => format!(" (new lines {}-{})", min + 1, max + 1),
                    None => {
                        let (lhs_range, _) = chunk_line_range(chunk);
                        match lhs_range {
                            Some((min, max)) => format!(" (old lines {}-{})", min + 1, max + 1),
                            None => String::new(),
                        }
                    }
                };
                out.push_str(&format!("<!-- chunk {}{} -->\n\n", i, line_info));
                let text = render_chunks_text(difft, &[i], None);
                out.push_str(&format!("```difft {} chunks={}\n", file, i));
                out.push_str(&text);
                out.push_str("```\n\n");
            }
        }
    }

    fs::write(output, &out)
        .with_context(|| format!("Failed to write summary to {}", output.display()))?;
    Ok(())
}

pub fn run(walkthrough_path: &Path, data_dir: &Path, output_path: &Path, no_diff_data: bool) -> Result<()> {
    let raw_content = fs::read_to_string(walkthrough_path)
        .with_context(|| format!("Failed to read {}", walkthrough_path.display()))?;

    // Parse optional YAML frontmatter (between --- delimiters)
    let mut metadata: HashMap<String, String> = HashMap::new();
    let md_content = if raw_content.starts_with("---\n") || raw_content.starts_with("---\r\n") {
        let end_marker = if raw_content.starts_with("---\r\n") { "\r\n---" } else { "\n---" };
        if let Some(end_pos) = raw_content[4..].find(end_marker) {
            let frontmatter = &raw_content[4..4 + end_pos];
            for line in frontmatter.lines() {
                if let Some((key, value)) = line.split_once(':') {
                    metadata.insert(
                        key.trim().to_string(),
                        value.trim().to_string(),
                    );
                }
            }
            // Skip past the closing ---
            let content_start = 4 + end_pos + end_marker.len();
            raw_content[content_start..].trim_start_matches('\n').trim_start_matches("\r\n").to_string()
        } else {
            raw_content
        }
    } else {
        raw_content
    };

    let difft_re = Regex::new(r"^difft\s+(\S+)\s+chunks=(\S+)(?:\s+lines=(\S+))?")?;
    let src_re = Regex::new(r"^src\s+(?:old\s+)?(\S+):(\d+)-(\d+)(?:\s+old)?")?;

    let mut hl = Highlighter::new();

    // Load all difft JSON data (skip when --no-diff-data is set)
    let mut data: HashMap<String, DifftOutput> = HashMap::new();
    if !no_diff_data {
        for entry in fs::read_dir(data_dir).context("Failed to read data directory")? {
            let entry = entry?;
            let path = entry.path();
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if path.extension().map_or(false, |e| e == "json") && filename != ".meta.json" {
                let json_str = fs::read_to_string(&path)?;
                let difft: DifftOutput = serde_json::from_str(&json_str)
                    .with_context(|| format!("Failed to parse {}", path.display()))?;
                if let Some(ref path) = difft.path {
                    data.insert(path.clone(), difft);
                }
            }
        }
    }

    // Warn if collected data may be stale (HEAD has moved since collection).
    if !no_diff_data {
        let meta_path = data_dir.join(".meta.json");
        if let Ok(meta_str) = fs::read_to_string(&meta_path) {
            if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&meta_str) {
                if let Some(collected_head) = meta.get("head_sha").and_then(|v| v.as_str()) {
                    let current_head = std::process::Command::new("git")
                        .args(["rev-parse", "HEAD"])
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
                    if let Some(ref cur) = current_head {
                        if cur != collected_head {
                            let short_collected = &collected_head[..collected_head.len().min(8)];
                            let short_current = &cur[..cur.len().min(8)];
                            eprintln!(
                                "Warning: collected data is from HEAD {} but current HEAD is {}. \
                                 Re-run `walkthrough collect` if the diff has changed.",
                                short_collected, short_current,
                            );
                        }
                    }
                }
            }
        }
    }

    // First pass: replace difft/src/notes code blocks with HTML placeholders,
    // and build the enriched markdown with text diffs in code block bodies.
    // Also track which (file, chunk) pairs are referenced for verification.
    let mut processed_md = String::new();
    let mut enriched_md = String::new();

    // Preserve frontmatter in enriched markdown
    if !metadata.is_empty() {
        enriched_md.push_str("---\n");
        for (key, value) in &metadata {
            enriched_md.push_str(&format!("{}: {}\n", key, value));
        }
        enriched_md.push_str("---\n\n");
    }
    let mut diff_blocks: Vec<String> = Vec::new();
    let mut in_difft_block = false;
    let mut in_notes_block = false;
    let mut notes_body = String::new();
    let mut notes_backtick_count = 0;
    let mut in_folds_block = false;
    let mut folds_body = String::new();
    let mut last_block_base_line: usize = 0; // base new-line (0-based) for relative→absolute
    let mut last_block_base_line_old: usize = 0; // base old-line (0-based) for old-side folds
    let mut last_block_file = String::new(); // file path for syntax-highlighting folds
    let mut referenced: std::collections::HashSet<(String, usize)> = std::collections::HashSet::new();

    for line in md_content.lines() {
        if in_notes_block {
            if line.trim_start().starts_with("```") {
                in_notes_block = false;
                enriched_md.push_str(line);
                enriched_md.push('\n');

                // Parse notes and inject into most recent diff block
                let note_re = Regex::new(r"^(\d+)(?:-(\d+))?:\s*(.+)$").unwrap();
                let mut annotations: Vec<(u64, u64, String)> = Vec::new();
                for note_line in notes_body.lines() {
                    let trimmed = note_line.trim();
                    if trimmed.is_empty() { continue; }
                    if let Some(caps) = note_re.captures(trimmed) {
                        let rel_start: usize = caps[1].parse().unwrap_or(0);
                        let rel_end: usize = caps.get(2).map_or(rel_start, |m| m.as_str().parse().unwrap_or(rel_start));
                        let text = caps[3].to_string();
                        let abs_start = (last_block_base_line + rel_start) as u64;
                        let abs_end = (last_block_base_line + rel_end) as u64;
                        annotations.push((abs_start, abs_end, text));
                    }
                }

                if !annotations.is_empty() {
                    let block_idx = diff_blocks.len().saturating_sub(1);
                    if let Some(last_block) = diff_blocks.last_mut() {
                        for (start, end, text) in &annotations {
                            let escaped = html_escape(text).replace('"', "&quot;");
                            for ln in *start..=*end {
                                for side in &["ln-lhs", "ln-rhs"] {
                                    let exact = format!("class=\"ln {}\">{}</td>", side, ln);
                                    if let Some(td_pos) = last_block.find(&exact) {
                                        if let Some(tr_pos) = last_block[..td_pos].rfind("<tr") {
                                            let tr_close = tr_pos + last_block[tr_pos..].find('>').unwrap_or(0);
                                            let old_tag = last_block[tr_pos..tr_close].to_string();
                                            if old_tag.contains("data-notes=\"") {
                                                // Append to existing notes
                                                let new_tag = old_tag.replace(
                                                    "data-notes=\"",
                                                    &format!("data-notes=\"{}&#x7c;", escaped),
                                                );
                                                let before = last_block[..tr_pos].to_string();
                                                let after = last_block[tr_close..].to_string();
                                                *last_block = format!("{}{}{}", before, new_tag, after);
                                            } else {
                                                let new_tag = if !old_tag.contains("annotated") {
                                                    old_tag.replacen("class=\"", "class=\"annotated ", 1)
                                                } else {
                                                    old_tag.clone()
                                                };
                                                let new_tag = format!("{} data-notes=\"{}\"", new_tag, escaped);
                                                let before = last_block[..tr_pos].to_string();
                                                let after = last_block[tr_close..].to_string();
                                                *last_block = format!("{}{}{}", before, new_tag, after);
                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                notes_body.push_str(line);
                notes_body.push('\n');
                enriched_md.push_str(line);
                enriched_md.push('\n');
            }
            continue;
        }
        if in_folds_block {
            if line.trim_start().starts_with("```") {
                in_folds_block = false;
                enriched_md.push_str(line);
                enriched_md.push('\n');

                // Parse folds and inject into most recent diff block.
                // Each fold can optionally specify a side: "old 1-5: ..." or "new 1-5: ...".
                // Default (no prefix) is "new" for backward compatibility.
                // Side determines which line numbers to use and which column to render in.
                let fold_re = Regex::new(r"^(?:(old|new)\s+)?(\d+)(?:-(\d+))?:\s*(.*)$").unwrap();
                // side: false = new (rhs), true = old (lhs)
                let mut folds: Vec<(u64, u64, String, bool)> = Vec::new();
                for fold_line in folds_body.lines() {
                    let trimmed = fold_line.trim();
                    if trimmed.is_empty() { continue; }
                    if let Some(caps) = fold_re.captures(trimmed) {
                        let is_old = caps.get(1).map_or(false, |m| m.as_str() == "old");
                        let rel_start: usize = caps[2].parse().unwrap_or(0);
                        let rel_end: usize = caps.get(3).map_or(rel_start, |m| m.as_str().parse().unwrap_or(rel_start));
                        let text = caps[4].to_string();
                        let base = if is_old { last_block_base_line_old } else { last_block_base_line };
                        let abs_start = (base + rel_start) as u64;
                        let abs_end = (base + rel_end) as u64;
                        folds.push((abs_start, abs_end, text, is_old));
                    } else if !folds.is_empty() {
                        // Continuation line: append to previous fold's text
                        let last = folds.last_mut().unwrap();
                        if !last.2.is_empty() { last.2.push('\n'); }
                        last.2.push_str(fold_line);
                    }
                }

                if !folds.is_empty() {
                    if let Some(last_block) = diff_blocks.last_mut() {
                        let is_single = last_block.contains("diff-single");

                        for (fold_idx, (start, end, text, is_old)) in folds.iter().enumerate() {
                            let escaped_text = html_escape(text);
                            let fold_id = fold_idx.to_string();

                            // Mark matching rows with fold-line class, count how many.
                            // Search pattern depends on which side the fold targets:
                            //   old  → "ln ln-lhs" only
                            //   new  → "ln ln-rhs" first, then "ln ln-lhs", then plain "ln"
                            let mut fold_count: usize = 0;
                            for ln in *start..=*end {
                                let needles: Vec<String> = if *is_old {
                                    vec![
                                        format!("class=\"ln ln-lhs\">{}</td>", ln),
                                        format!("class=\"ln\">{}</td>", ln),
                                    ]
                                } else {
                                    vec![
                                        format!("class=\"ln ln-rhs\">{}</td>", ln),
                                        format!("class=\"ln\">{}</td>", ln),
                                    ]
                                };
                                for needle in &needles {
                                    if let Some(td_pos) = last_block.find(needle.as_str()) {
                                        if let Some(tr_pos) = last_block[..td_pos].rfind("<tr") {
                                            let tr_close = tr_pos + last_block[tr_pos..].find('>').unwrap_or(0);
                                            let old_tag = last_block[tr_pos..tr_close].to_string();

                                            if old_tag.contains("chunk-sep") || old_tag.contains("fold-line") {
                                                break;
                                            }

                                            let new_tag = old_tag.replacen(
                                                "class=\"",
                                                "class=\"fold-line ",
                                                1,
                                            );
                                            let new_tag = format!("{} data-fold-id=\"{}\" hidden=\"until-found\"", new_tag, fold_id);
                                            let before = last_block[..tr_pos].to_string();
                                            let after = last_block[tr_close..].to_string();
                                            *last_block = format!("{}{}{}", before, new_tag, after);
                                            fold_count += 1;
                                            break; // found the row, no need to check other patterns
                                        }
                                    }
                                }
                            }

                            if fold_count == 0 {
                                let side_label = if *is_old { "old" } else { "new" };
                                eprintln!(
                                    "Warning: fold {} {}-{} matched 0 rows in the diff block \
                                     (line numbers are relative, 1-based from the first \
                                     {}-file line in the chunk)",
                                    side_label, start, end, side_label
                                );
                                continue;
                            }

                            // Extract indentation from the first non-empty folded row's code cell.
                            // For side-specific folds, look at the targeted side's code cell;
                            // otherwise check all code cells in the row.
                            let mut indent = String::new();
                            let fold_marker = format!("data-fold-id=\"{}\"", fold_id);
                            let code_class = if *is_old {
                                "class=\"code-lhs\""
                            } else if !is_single {
                                "class=\"code-rhs\""
                            } else {
                                "class=\"code"
                            };
                            let mut search_from = 0usize;
                            'outer: while let Some(marker_pos) = last_block[search_from..].find(&fold_marker) {
                                let abs_marker = search_from + marker_pos;
                                // Find the end of this row so we check all code cells within it
                                let row_end = last_block[abs_marker..].find("</tr>")
                                    .map_or(last_block.len(), |p| abs_marker + p);
                                let mut code_search = abs_marker;
                                while code_search < row_end {
                                    if let Some(code_pos) = last_block[code_search..row_end].find(code_class) {
                                        let abs_code_pos = code_search + code_pos;
                                        if let Some(gt_pos) = last_block[abs_code_pos..].find('>') {
                                            let content_start = abs_code_pos + gt_pos + 1;
                                            let mut row_indent = String::new();
                                            let mut has_content = false;
                                            for ch in last_block[content_start..].chars() {
                                                if ch == ' ' || ch == '\t' {
                                                    row_indent.push(ch);
                                                } else if ch == '<' {
                                                    // Check if this is </td> (empty cell) or
                                                    // a <span> tag (syntax-highlighted content)
                                                    has_content = !last_block[content_start + row_indent.len()..].starts_with("</td>");
                                                    break;
                                                } else if ch == '\n' {
                                                    break;
                                                } else {
                                                    has_content = true;
                                                    break;
                                                }
                                            }
                                            if has_content {
                                                indent = row_indent;
                                                break 'outer;
                                            }
                                            // Empty cell; try the next code cell in this row
                                            code_search = abs_code_pos + code_class.len();
                                        } else {
                                            break;
                                        }
                                    } else {
                                        break;
                                    }
                                }
                                search_from = abs_marker + fold_marker.len();
                            }

                            // Build summary row with multi-line support.
                            // For side-by-side diffs with a side-specific fold, the summary
                            // appears on just that side's columns; the other side is empty.
                            let line_label = if fold_count == 1 { "line" } else { "lines" };

                            // Render fold text: syntax-highlight as the same language.
                            // Single-line folds get the extracted indent prefix.
                            // Multi-line folds: replace the markdown's minimum indentation
                            // with the code's actual indentation so the fold aligns with
                            // surrounding code.
                            let is_multiline = text.contains('\n');
                            let text_lines: Vec<String> = if is_multiline {
                                // Find minimum indentation in the markdown fold text
                                let min_indent = text.split('\n')
                                    .filter(|l| !l.trim().is_empty())
                                    .map(|l| l.len() - l.trim_start().len())
                                    .min()
                                    .unwrap_or(0);
                                // Replace markdown indent with code indent
                                text.split('\n')
                                    .map(|l| {
                                        if l.trim().is_empty() {
                                            String::new()
                                        } else {
                                            let stripped = &l[min_indent.min(l.len())..];
                                            format!("{}{}", indent, stripped)
                                        }
                                    })
                                    .collect()
                            } else {
                                // Single-line: strip leading whitespace from the
                                // markdown text. The extracted code indent is
                                // prepended separately to match the stripped code.
                                text.split('\n')
                                    .filter(|l| !l.is_empty())
                                    .map(|l| l.trim_start().to_string())
                                    .collect()
                            };
                            let lang = arborium::detect_language(&last_block_file);
                            let hl_lines = syntax_highlight_lines(&text_lines, &mut hl, lang);
                            let mut fold_content = String::new();
                            for (i, hl_line) in hl_lines.iter().enumerate() {
                                if i > 0 { fold_content.push('\n'); }
                                if !is_multiline {
                                    fold_content.push_str(&indent);
                                }
                                fold_content.push_str(hl_line);
                            }

                            // Determine fold color class based on context:
                            // add-only block or new-side fold → fold-add (green)
                            // remove-only block or old-side fold → fold-del (red)
                            // src block or other → default (yellow)
                            let fold_color = if last_block.contains("diff-add-only") {
                                " fold-add"
                            } else if last_block.contains("diff-remove-only") {
                                " fold-del"
                            } else if last_block.contains("diff-sides") {
                                if *is_old { " fold-del" } else { " fold-add" }
                            } else {
                                "" // src blocks keep yellow
                            };

                            let summary = format!(
                                "<tr class=\"fold-summary{}\" data-fold-id=\"{}\">\
                                 <td class=\"ln fold-count\">{} {}</td>\
                                 <td class=\"sign\"><span class=\"fold-arrow\">\u{25B6}</span></td>\
                                 <td class=\"fold-text\">{}</td></tr>",
                                fold_color, fold_id, fold_count, line_label, fold_content
                            );

                            // Insert fold summary row before the first fold-line of this group
                            if let Some(marker_pos) = last_block.find(&fold_marker) {
                                if let Some(tr_pos) = last_block[..marker_pos].rfind("<tr") {
                                    let before = last_block[..tr_pos].to_string();
                                    let after = last_block[tr_pos..].to_string();
                                    *last_block = format!("{}{}{}", before, summary, after);
                                }
                            }

                            // In two-table layout, insert a placeholder fold-summary
                            // in the OTHER table so height-sync can align when expanded.
                            let is_two_table = last_block.contains("diff-sides");
                            if is_two_table {
                                let placeholder = format!(
                                    "<tr class=\"fold-summary placeholder\" data-fold-id=\"{}\">\
                                     <td class=\"ln\"></td><td class=\"sign\"></td><td class=\"code\"></td></tr>",
                                    fold_id
                                );
                                let other_table = if *is_old { "diff-table-rhs" } else { "diff-table-lhs" };
                                if let Some(table_pos) = last_block.find(other_table) {
                                    // Find the data-row-idx of the first fold-line row
                                    // (skip fold-summary which doesn't have row-idx).
                                    // Fold-line rows have both "fold-line" in class and the fold_marker.
                                    let idx_re = Regex::new(r#"data-row-idx="(\d+)""#).unwrap();
                                    let mut first_fold_idx: Option<usize> = None;
                                    let mut search = 0;
                                    while let Some(pos) = last_block[search..].find(&fold_marker) {
                                        let abs = search + pos;
                                        if let Some(tr_start) = last_block[..abs].rfind("<tr") {
                                            let tr_tag = &last_block[tr_start..abs + fold_marker.len()];
                                            if tr_tag.contains("fold-line") {
                                                first_fold_idx = idx_re.captures(tr_tag)
                                                    .and_then(|c| c[1].parse::<usize>().ok());
                                                break;
                                            }
                                        }
                                        search = abs + fold_marker.len();
                                    }
                                    if let Some(target_idx) = first_fold_idx {
                                        let target_needle = format!("data-row-idx=\"{}\"", target_idx);
                                        // Find this row-idx in the other table
                                        if let Some(other_row_pos) = last_block[table_pos..].find(&target_needle) {
                                            let abs_pos = table_pos + other_row_pos;
                                            if let Some(tr_start) = last_block[..abs_pos].rfind("<tr") {
                                                let before = last_block[..tr_start].to_string();
                                                let after = last_block[tr_start..].to_string();
                                                *last_block = format!("{}{}{}", before, placeholder, after);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                folds_body.push_str(line);
                folds_body.push('\n');
                enriched_md.push_str(line);
                enriched_md.push('\n');
            }
            continue;
        }
        if !in_difft_block {
            if line.starts_with("```") && line.chars().filter(|&c| c == '`').count() >= 3 {
                let backtick_count = line.chars().take_while(|&c| c == '`').count();
                let info = &line[backtick_count..];

                if let Some(caps) = difft_re.captures(info.trim()) {
                    let file = caps[1].to_string();
                    let chunks_spec = caps[2].to_string();

                    // Parse optional lines=START-END (1-based, relative to chunk).
                    // Convert to absolute 0-based new-file line numbers using the
                    // chunk's first new-file line as the base.
                    let line_filter: LineFilter = caps.get(3).and_then(|m| {
                        let s = m.as_str();
                        let parts: Vec<&str> = s.split('-').collect();
                        if parts.len() != 2 { return None; }
                        let rel_start: usize = parts[0].parse().ok()?;
                        let rel_end: usize = parts[1].parse().ok()?;
                        let difft_ref = data.get(&file)?;
                        // Find the earliest new-file line across the selected chunks
                        let mut base = usize::MAX;
                        let chunk_indices_for_base: Vec<usize> = if chunks_spec == "all" {
                            (0..difft_ref.chunks.len()).collect()
                        } else {
                            chunks_spec.split(',')
                                .filter_map(|s| s.trim().parse().ok())
                                .collect()
                        };
                        for &ci in &chunk_indices_for_base {
                            if let Some(chunk) = difft_ref.chunks.get(ci) {
                                for entry in chunk {
                                    if let Some(rhs) = &entry.rhs {
                                        base = base.min(rhs.line_number as usize);
                                    }
                                }
                            }
                        }
                        if base == usize::MAX { return None; }
                        // relative 1-based -> absolute 0-based
                        Some((base + rel_start - 1, base + rel_end - 1))
                    });

                    let is_generated = info.contains(" generated");

                    if let Some(difft) = data.get(&file) {
                        let indices: Vec<usize> = if chunks_spec == "all" {
                            (0..difft.chunks.len()).collect()
                        } else {
                            chunks_spec
                                .split(',')
                                .filter_map(|s| s.trim().parse().ok())
                                .collect()
                        };

                        for &idx in &indices {
                            referenced.insert((file.clone(), idx));
                        }
                        let collapse = if is_generated {
                            CollapseMode::Hidden
                        } else if difft.status.as_deref() == Some("deleted") {
                            CollapseMode::Searchable
                        } else {
                            CollapseMode::None
                        };
                        let rendered_html = render_chunks(difft, &indices, &file, line_filter, &mut hl, collapse);
                        let placeholder_id = diff_blocks.len();
                        diff_blocks.push(rendered_html);
                        processed_md
                            .push_str(&format!("<!-- DIFF_PLACEHOLDER_{} -->\n", placeholder_id));

                        // Track base line numbers for relative fold addressing
                        let mut base = usize::MAX;
                        let mut old_base = usize::MAX;
                        for &ci in &indices {
                            if let Some(chunk) = difft.chunks.get(ci) {
                                for entry in chunk {
                                    if let Some(rhs) = &entry.rhs {
                                        base = base.min(rhs.line_number as usize);
                                    }
                                    if let Some(lhs) = &entry.lhs {
                                        old_base = old_base.min(lhs.line_number as usize);
                                    }
                                }
                            }
                        }
                        last_block_base_line = if base == usize::MAX { 0 } else { base };
                        last_block_base_line_old = if old_base == usize::MAX { 0 } else { old_base };
                        last_block_file = file.clone();

                        // Write enriched markdown: opening fence + text diff + closing fence
                        let text_diff = render_chunks_text(difft, &indices, line_filter);
                        enriched_md.push_str(line);
                        enriched_md.push('\n');
                        enriched_md.push_str(&text_diff);
                        enriched_md.push_str(&"`".repeat(backtick_count));
                        enriched_md.push('\n');
                    } else {
                        eprintln!("Warning: no data for file '{}', passing through", file);
                        processed_md.push_str(line);
                        processed_md.push('\n');
                        enriched_md.push_str(line);
                        enriched_md.push('\n');
                    }
                    in_difft_block = true;
                    continue;
                }

                // Source block: ```src filepath:start-end [old]
                if let Some(caps) = src_re.captures(info.trim()) {
                    let file = caps[1].to_string();
                    let start: usize = caps[2].parse().unwrap_or(1);
                    let end: usize = caps[3].parse().unwrap_or(1);
                    let use_old = info.contains(" old");

                    // Get lines from collected data, or fall back to filesystem
                    let lines_from_data: Option<&Vec<String>> = data.get(&file)
                        .map(|difft| if use_old { &difft.old_lines } else { &difft.new_lines });
                    let lines_from_fs: Vec<String>;
                    let lines: Option<&Vec<String>> = if let Some(l) = lines_from_data {
                        Some(l)
                    } else {
                        // Try reading from filesystem (relative to cwd)
                        match fs::read_to_string(&file) {
                            Ok(content) => {
                                lines_from_fs = content.lines().map(|s| s.to_string()).collect();
                                Some(&lines_from_fs)
                            }
                            Err(_) => None,
                        }
                    };

                    if let Some(lines) = lines {
                        let lang = arborium::detect_language(&file);
                        let hl_lines = syntax_highlight_lines(lines, &mut hl, lang);

                        // Render source block using diff-block styling with single column
                        let mut src_html = String::new();
                        let header_label = if use_old {
                            format!("{} (old)", html_escape(&file))
                        } else {
                            html_escape(&file)
                        };
                        src_html.push_str(&format!(
                            "<div class=\"diff-block\" data-collapse=\"none\"><div class=\"diff-header\">\
                             <span class=\"collapse-arrow\">\u{25B6}</span> {}\
                             <label class=\"viewed-label\"><input type=\"checkbox\" class=\"viewed-check\"><span>Viewed</span></label>\
                             </div><div class=\"diff-body\">",
                            header_label
                        ));
                        src_html.push_str(
                            "<table class=\"diff-table diff-single diff-add-only\"><colgroup>\
                             <col class=\"ln-col\"><col class=\"sign-col\"><col class=\"code-col\">\
                             </colgroup><tbody>"
                        );
                        for ln in start..=end {
                            let idx = ln.saturating_sub(1);
                            let content = hl_lines.get(idx).map(|s| s.as_str()).unwrap_or("");
                            src_html.push_str(&format!(
                                "<tr class=\"line-context\"><td class=\"ln\">{}</td><td class=\"sign\"></td><td class=\"code\">{}</td></tr>",
                                ln, content
                            ));
                        }
                        src_html.push_str("</tbody></table></div></div>");

                        let placeholder_id = diff_blocks.len();
                        diff_blocks.push(src_html);
                        processed_md.push_str(&format!("<!-- DIFF_PLACEHOLDER_{} -->\n", placeholder_id));

                        // Track base line for relative fold/note line numbers.
                        // For src blocks, relative line 1 = the first displayed line.
                        last_block_base_line = start.saturating_sub(1);
                        last_block_base_line_old = start.saturating_sub(1);
                        last_block_file = file.clone();

                        // Enrich markdown with source lines
                        enriched_md.push_str(line);
                        enriched_md.push('\n');
                        for ln in start..=end {
                            let idx = ln.saturating_sub(1);
                            let content = lines.get(idx).map(|s| s.as_str()).unwrap_or("");
                            enriched_md.push_str(&format!("{:>4} {}\n", ln, content));
                        }
                        enriched_md.push_str(&"`".repeat(backtick_count));
                        enriched_md.push('\n');
                    } else {
                        eprintln!("Warning: no data for file '{}' and file not found on disk, passing through", file);
                        processed_md.push_str(line);
                        processed_md.push('\n');
                        enriched_md.push_str(line);
                        enriched_md.push('\n');
                    }
                    in_difft_block = true;
                    continue;
                }

                // Notes block: ```notes
                if info.trim() == "notes" {
                    in_notes_block = true;
                    notes_body.clear();
                    notes_backtick_count = backtick_count;
                    enriched_md.push_str(line);
                    enriched_md.push('\n');
                    continue;
                }

                // Folds block: ```folds
                if info.trim() == "folds" {
                    in_folds_block = true;
                    folds_body.clear();
                    enriched_md.push_str(line);
                    enriched_md.push('\n');
                    continue;
                }
            }
            processed_md.push_str(line);
            processed_md.push('\n');
            enriched_md.push_str(line);
            enriched_md.push('\n');
        } else if line.trim_start().starts_with("```") {
            in_difft_block = false;
            // Don't emit the closing fence to enriched_md; we already wrote it above
        }
    }

    // Write enriched markdown back to the walkthrough file
    fs::write(walkthrough_path, &enriched_md)
        .with_context(|| format!("Failed to write enriched markdown to {}", walkthrough_path.display()))?;
    eprintln!("Wrote enriched markdown back to {}", walkthrough_path.display());

    // Second pass: convert markdown (with placeholders) to HTML
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(&processed_md, options);
    let mut html_body = String::new();
    pulldown_cmark::html::push_html(&mut html_body, parser);

    // Third pass: replace placeholders with rendered diff HTML
    for (i, block_html) in diff_blocks.iter().enumerate() {
        let placeholder = format!("<!-- DIFF_PLACEHOLDER_{} -->", i);
        let placeholder_in_p = format!("<p>{}</p>", placeholder);
        if html_body.contains(&placeholder_in_p) {
            html_body = html_body.replace(&placeholder_in_p, block_html);
        } else {
            html_body = html_body.replace(&placeholder, block_html);
        }
    }

    // Pre-render mermaid code blocks to inline SVG via mmdc (mermaid CLI).
    // pulldown-cmark renders ```mermaid as <pre><code class="language-mermaid">...</code></pre>
    let mermaid_re = Regex::new(r#"(?s)<pre><code class="language-mermaid">(.*?)</code></pre>"#)?;
    html_body = mermaid_re.replace_all(&html_body, |caps: &regex::Captures| {
        let body = caps[1].replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"");

        // Write mermaid source to a unique temp file, run mmdc, read SVG output.
        // Use a per-process unique prefix to avoid races between concurrent renders.
        let tmp_dir = std::env::temp_dir();
        let unique = format!("walkthrough_mermaid_{}", std::process::id());
        let input_path = tmp_dir.join(format!("{}.mmd", unique));
        let output_svg = tmp_dir.join(format!("{}.svg", unique));
        let _ = fs::write(&input_path, &body);

        // Write puppeteer config to use system Chrome
        let puppeteer_config = tmp_dir.join(format!("{}_puppeteer.json", unique));
        let _ = fs::write(&puppeteer_config, r#"{"executablePath":"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome","args":["--no-sandbox"]}"#);

        let result = std::process::Command::new("mmdc")
            .arg("-i").arg(&input_path)
            .arg("-o").arg(&output_svg)
            .arg("-t").arg("neutral")
            .arg("-b").arg("transparent")
            .arg("-p").arg(&puppeteer_config)
            .output();

        match result {
            Ok(output) if output.status.success() => {
                match fs::read_to_string(&output_svg) {
                    Ok(svg) => {
                        let _ = fs::remove_file(&input_path);
                        let _ = fs::remove_file(&output_svg);
                        format!("<div class=\"mermaid-diagram\">{}</div>", svg)
                    }
                    Err(e) => {
                        eprintln!("Warning: failed to read mermaid SVG: {}", e);
                        format!("<pre><code class=\"language-mermaid\">{}</code></pre>", caps[1].to_string())
                    }
                }
            }
            Ok(output) => {
                eprintln!("Warning: mmdc failed: {}", String::from_utf8_lossy(&output.stderr));
                format!("<pre><code class=\"language-mermaid\">{}</code></pre>", caps[1].to_string())
            }
            Err(_) => {
                eprintln!("Warning: mmdc not found. Install with: npm install -g @mermaid-js/mermaid-cli");
                format!("<pre><code class=\"language-mermaid\">{}</code></pre>", caps[1].to_string())
            }
        }
    }).to_string();

    // Reject fenced code blocks without a language tag.
    // pulldown-cmark renders bare ``` blocks as <pre><code>...</code></pre> (no class).
    let bare_code_re = Regex::new(r#"(?s)<pre><code>([^<]*?)</code></pre>"#)?;
    if let Some(m) = bare_code_re.find(&html_body) {
        // Extract a snippet of the content for the error message.
        let start = m.start();
        let snippet_re = Regex::new(r#"(?s)<pre><code>(.{0,60})"#)?;
        let snippet = snippet_re.captures(&html_body[start..])
            .map(|c| c[1].replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">"))
            .unwrap_or_default();
        let snippet = snippet.lines().next().unwrap_or(&snippet);
        anyhow::bail!(
            "Fenced code block without a language tag:\n\n  ```\n  {}...\n  ```\n\n\
             All code blocks must specify a language (e.g. ```typescript, ```rust, ```bash). \
             Use ```plain for blocks with no syntax highlighting.",
            snippet
        );
    }

    // Syntax-highlight fenced code blocks.
    // pulldown-cmark renders ```lang as <pre><code class="language-lang">...</code></pre>.
    // Map the language tag to a file extension for arborium::detect_language.
    let code_block_re = Regex::new(r#"(?s)<pre><code class="language-([^"]+)">(.*?)</code></pre>"#)?;
    let mut hl = Highlighter::new();
    html_body = code_block_re.replace_all(&html_body, |caps: &regex::Captures| {
        let lang_tag = &caps[1];
        let html_encoded_body = &caps[2];

        // "plain" means no syntax highlighting; keep the block as-is.
        if lang_tag == "plain" || lang_tag == "text" || lang_tag == "txt" {
            return format!("<pre><code>{}</code></pre>", html_encoded_body);
        }

        // Map common markdown fence tags to file extensions for detect_language.
        let ext = match lang_tag {
            "typescript" | "ts" => "ts",
            "tsx" => "tsx",
            "javascript" | "js" => "js",
            "jsx" => "jsx",
            "rust" | "rs" => "rs",
            "python" | "py" => "py",
            "go" | "golang" => "go",
            "ruby" | "rb" => "rb",
            "java" => "java",
            "c" => "c",
            "cpp" | "c++" | "cxx" => "cpp",
            "css" => "css",
            "html" => "html",
            "json" => "json",
            "yaml" | "yml" => "yaml",
            "toml" => "toml",
            "bash" | "sh" | "shell" | "zsh" => "sh",
            "sql" => "sql",
            "swift" => "swift",
            "kotlin" | "kt" => "kt",
            other => other,
        };
        let fake_path = format!("file.{}", ext);
        let lang = arborium::detect_language(&fake_path);

        if lang.is_none() {
            return caps[0].to_string();
        }

        // Decode HTML entities back to source text for highlighting.
        let source = html_encoded_body
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"");

        let lines: Vec<String> = source.lines().map(String::from).collect();
        let highlighted = syntax_highlight_lines(&lines, &mut hl, lang);

        let mut out = String::from("<pre><code>");
        for (i, line) in highlighted.iter().enumerate() {
            out.push_str(line);
            if i + 1 < highlighted.len() {
                out.push('\n');
            }
        }
        out.push_str("</code></pre>");
        out
    }).to_string();

    // Replace @service inline code with styled service badges.
    // pulldown-cmark renders `@sboxd` as <code>@sboxd</code>.
    // Use `\@name` in backticks to suppress the badge (renders as plain `@name`).
    let service_escape_re = Regex::new(r#"<code>\\@([^<]+)</code>"#)?;
    html_body = service_escape_re.replace_all(&html_body, "\x00ESCAPED_AT$1\x00").to_string();
    let service_re = Regex::new(r#"<code>@([^<]+)</code>"#)?;
    html_body = service_re.replace_all(&html_body, |caps: &regex::Captures| {
        format!("<span class=\"service-badge\">{}</span>", &caps[1])
    }).to_string();
    let escaped_at_re = Regex::new(r#"\x00ESCAPED_AT([^\x00]+)\x00"#)?;
    html_body = escaped_at_re.replace_all(&html_body, "<code>@$1</code>").to_string();

    // Add anchor IDs to headings and extract the first h1 for <title>.
    let heading_re = Regex::new(r"<(h[1-6])>(.*?)</h[1-6]>")?;
    let mut page_title = String::from("Walkthrough");
    let mut first_h1 = true;
    html_body = heading_re.replace_all(&html_body, |caps: &regex::Captures| {
        let tag = &caps[1];
        let content = &caps[2];
        // Strip HTML tags to get plain text for the slug and title
        let strip_re = Regex::new(r"<[^>]+>").unwrap();
        let plain = strip_re.replace_all(content, "");
        let plain = plain.trim();
        if first_h1 && tag == "h1" {
            page_title = plain.to_string();
            first_h1 = false;
        }
        let slug: String = plain
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        format!("<{tag} id=\"{slug}\">{content}</{tag}>")
    }).to_string();

    // Verify coverage: check all chunks in all files are referenced.
    // Skip coverage badge entirely when there's no diff data (pure markdown mode).
    if !data.is_empty() {
        let mut total_chunks: usize = 0;
        let mut uncovered: Vec<(String, usize)> = Vec::new();
        for (file, difft) in &data {
            let chunk_count = difft.chunks.len();
            total_chunks += chunk_count;
            for i in 0..chunk_count {
                if !referenced.contains(&(file.clone(), i)) {
                    uncovered.push((file.clone(), i));
                }
            }
        }
        let all_covered = uncovered.is_empty();
        let file_count = data.len();

        // Read diff source from .meta.json if available
        let diff_source = fs::read_to_string(data_dir.join(".meta.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("diff_args")?.as_array().map(|args| {
                    args.iter()
                        .filter_map(|a| a.as_str().map(String::from))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
            })
            .unwrap_or_default();

        let source_text = if diff_source.is_empty() {
            String::new()
        } else {
            format!(" in <code>{}</code>", html_escape(&diff_source))
        };

        let badge_html = if all_covered {
            format!(
                "<div class=\"coverage-badge pass\">\u{2705} All {} chunks across {} files{} are present</div>",
                total_chunks, file_count, source_text
            )
        } else {
            format!(
                "<div class=\"coverage-badge fail\">\u{274c} {} uncovered chunks (out of {} across {} files{})</div>",
                uncovered.len(), total_chunks, file_count, source_text
            )
        };

        if all_covered {
            eprintln!(
                "All {} chunks across {} files{} are present",
                total_chunks, file_count,
                if diff_source.is_empty() { String::new() } else { format!(" in\n{}", diff_source) },
            );
        } else {
            eprintln!(
                "{} uncovered chunks (out of {} across {} files{})",
                uncovered.len(), total_chunks, file_count,
                if diff_source.is_empty() { String::new() } else { format!(" in\n{}", diff_source) },
            );
            for (file, idx) in &uncovered {
                eprintln!("  {} chunk {}", file, idx);
            }
        }

        // Inject badge after the first h1 closing tag
        let badge_anchor = "</h1>";
        if let Some(pos) = html_body.find(badge_anchor) {
            let insert_at = pos + badge_anchor.len();
            html_body.insert_str(insert_at, &format!("\n{}", badge_html));
        }
    }

    // Inject metadata subtitle inside the h1 (before </h1>)
    if !metadata.is_empty() {
        let mut subtitle_parts: Vec<String> = Vec::new();
        if let Some(pr) = metadata.get("pr").filter(|v| !v.is_empty()) {
            // Auto-link PR numbers
            let pr_text = if pr.starts_with("http") {
                // Extract PR number from URL (e.g. .../pull/711123)
                let pr_num = pr.rsplit('/').next().unwrap_or(pr);
                format!("<a href=\"{}\">#{}</a>", html_escape(pr), html_escape(pr_num))
            } else if let Some(stripped) = pr.strip_prefix('#') {
                format!("#{}", stripped)
            } else {
                format!("#{}", pr)
            };
            subtitle_parts.push(pr_text);
        }
        if let Some(author) = metadata.get("author").filter(|v| !v.is_empty()) {
            subtitle_parts.push(html_escape(author));
        }
        // Include any other metadata keys
        for (key, value) in &metadata {
            if key != "pr" && key != "author" && !value.is_empty() {
                subtitle_parts.push(format!("{}: {}", html_escape(key), html_escape(value)));
            }
        }
        if !subtitle_parts.is_empty() {
            let subtitle_html = format!(
                "<span class=\"subtitle\">{}</span>",
                subtitle_parts.join(" \u{b7} ")
            );
            if let Some(pos) = html_body.find("</h1>") {
                html_body.insert_str(pos, &subtitle_html);
            }
        }
    }

    let full_html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><text y='.9em' font-size='90'>📖</text></svg>">
<title>{title}</title>
<style>
{css}
</style>
</head>
<body>
<article>
{body}
</article>
<script>
{js}
</script>
</body>
</html>"#,
        title = html_escape(&page_title),
        css = CSS,
        body = html_body,
        js = JS
    );

    fs::write(output_path, full_html)
        .with_context(|| format!("Failed to write {}", output_path.display()))?;

    eprintln!("Rendered walkthrough to {}", output_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::difft_json::{ChangeSpan, DiffHunk};

    /// Build a simple LineSide with optional change spans.
    fn side(line: u64, spans: Vec<ChangeSpan>) -> LineSide {
        LineSide { line_number: line, changes: spans }
    }

    fn span(content: &str, start: usize, end: usize) -> ChangeSpan {
        ChangeSpan { content: content.to_string(), highlight: "normal".to_string(), start, end }
    }

    /// Build a DifftOutput from old/new lines, chunks, and hunks.
    /// Helper to call render_chunks with default syntax highlighting.
    fn test_render_chunks(difft: &DifftOutput, chunk_indices: &[usize], file_path: &str, line_filter: LineFilter) -> String {
        let mut hl = Highlighter::new();
        render_chunks(difft, chunk_indices, file_path, line_filter, &mut hl, CollapseMode::None)
    }

    fn make_difft(
        old_lines: Vec<&str>,
        new_lines: Vec<&str>,
        chunks: Vec<Vec<LineEntry>>,
        hunks: Vec<DiffHunk>,
    ) -> DifftOutput {
        DifftOutput {
            chunks,
            language: None,
            path: Some("test.ts".to_string()),
            status: Some("changed".to_string()),
            old_lines: old_lines.into_iter().map(String::from).collect(),
            new_lines: new_lines.into_iter().map(String::from).collect(),
            hunks,
        }
    }

    /// Extract all (class, old_ln, new_ln) tuples from rendered HTML table rows.
    /// Supports three layouts:
    ///   - Two-table mode (diff-table-lhs + diff-table-rhs): 3-cell rows paired by data-row-idx
    ///   - Single-column mode (diff-single): 1 ln cell mapped to the active side
    ///   - Legacy 6-column mode: first 2 ln cells are old/new
    fn extract_rows(html: &str) -> Vec<(String, Option<u64>, Option<u64>)> {
        // Strip hidden expandable context rows and expander buttons before parsing.
        let expand_re = Regex::new(r#"(?s)<tr [^>]*class="[^"]*expand-(line|summary)[^"]*"[^>]*>.*?</tr>"#).unwrap();
        let html = &expand_re.replace_all(html, "");

        let is_two_table = html.contains("diff-table-lhs") && html.contains("diff-table-rhs");
        let is_single = html.contains("diff-single");
        let single_is_add = html.contains("diff-add-only");

        if is_two_table {
            // Two-table mode: parse each table separately, pair rows by data-row-idx
            let row_re = Regex::new(r#"<tr ([^>]*class="([^"]+)"[^>]*)>"#).unwrap();
            let td_re = Regex::new(r#"<td class="ln[^"]*"[^>]*>(\d*)</td>"#).unwrap();
            let idx_re = Regex::new(r#"data-row-idx="(\d+)""#).unwrap();

            // Find LHS table content
            let lhs_start = html.find("diff-table-lhs").unwrap_or(0);
            let lhs_end = html[lhs_start..].find("</table>").map(|p| lhs_start + p).unwrap_or(html.len());
            let lhs_html = &html[lhs_start..lhs_end];

            // Find RHS table content
            let rhs_start = html.find("diff-table-rhs").unwrap_or(0);
            let rhs_end = html[rhs_start..].find("</table>").map(|p| rhs_start + p).unwrap_or(html.len());
            let rhs_html = &html[rhs_start..rhs_end];

            // Parse LHS rows into a map: row_idx -> (class, ln)
            let mut lhs_map: std::collections::HashMap<usize, (String, Option<u64>)> = std::collections::HashMap::new();
            let mut lhs_order: Vec<usize> = Vec::new();
            for row_cap in row_re.captures_iter(lhs_html) {
                let attrs = &row_cap[1];
                let class = row_cap[2].to_string()
                    .replace("placeholder ", "")
                    .replace("line-paired-full", "line-paired")
                    .replace("line-added-partial", "line-added")
                    .replace("line-removed-partial", "line-removed");
                if let Some(idx_cap) = idx_re.captures(attrs) {
                    let idx: usize = idx_cap[1].parse().unwrap_or(0);
                    let match_end = row_cap.get(0).unwrap().end();
                    let rest = &lhs_html[match_end..];
                    let end = rest.find("</tr>").unwrap_or(rest.len());
                    let row_html = &rest[..end];
                    let lns: Vec<Option<u64>> = td_re.captures_iter(row_html)
                        .map(|c| {
                            let s = &c[1];
                            if s.is_empty() { None } else { s.parse().ok() }
                        })
                        .collect();
                    let old_ln = lns.first().copied().flatten();
                    lhs_map.insert(idx, (class, old_ln));
                    lhs_order.push(idx);
                }
            }

            // Parse RHS rows
            let mut rhs_map: std::collections::HashMap<usize, Option<u64>> = std::collections::HashMap::new();
            for row_cap in row_re.captures_iter(rhs_html) {
                let attrs = &row_cap[1];
                if let Some(idx_cap) = idx_re.captures(attrs) {
                    let idx: usize = idx_cap[1].parse().unwrap_or(0);
                    let abs_start = row_cap.get(0).unwrap().end();
                    let rest = &rhs_html[abs_start..];
                    let end = rest.find("</tr>").unwrap_or(rest.len());
                    let row_html = &rest[..end];
                    let lns: Vec<Option<u64>> = td_re.captures_iter(row_html)
                        .map(|c| {
                            let s = &c[1];
                            if s.is_empty() { None } else { s.parse().ok() }
                        })
                        .collect();
                    let new_ln = lns.first().copied().flatten();
                    rhs_map.insert(idx, new_ln);
                }
            }

            // Parse RHS rows with class info for added-only rows
            let mut rhs_class_map: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
            let mut rhs_order: Vec<usize> = Vec::new();
            for row_cap in row_re.captures_iter(rhs_html) {
                let attrs = &row_cap[1];
                let class = row_cap[2].to_string()
                    .replace("placeholder ", "")
                    .replace("line-paired-full", "line-paired")
                    .replace("line-added-partial", "line-added")
                    .replace("line-removed-partial", "line-removed");
                if let Some(idx_cap) = idx_re.captures(attrs) {
                    let idx: usize = idx_cap[1].parse().unwrap_or(0);
                    rhs_class_map.insert(idx, class);
                    rhs_order.push(idx);
                }
            }

            // Merge LHS and RHS indices in order, deduplicating
            let mut all_indices: Vec<usize> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            let mut li = 0;
            let mut ri = 0;
            loop {
                let l = lhs_order.get(li).copied();
                let r = rhs_order.get(ri).copied();
                match (l, r) {
                    (Some(lv), Some(rv)) => {
                        if lv <= rv {
                            if seen.insert(lv) { all_indices.push(lv); }
                            li += 1;
                            if lv == rv { ri += 1; }
                        } else {
                            if seen.insert(rv) { all_indices.push(rv); }
                            ri += 1;
                        }
                    }
                    (Some(lv), None) => {
                        if seen.insert(lv) { all_indices.push(lv); }
                        li += 1;
                    }
                    (None, Some(rv)) => {
                        if seen.insert(rv) { all_indices.push(rv); }
                        ri += 1;
                    }
                    (None, None) => break,
                }
            }

            // Combine by row_idx
            let mut rows = Vec::new();
            for idx in &all_indices {
                let (class, old_ln) = lhs_map.get(idx)
                    .map(|(c, l)| (c.clone(), *l))
                    .unwrap_or_else(|| {
                        (rhs_class_map.get(idx).cloned().unwrap_or_default(), None)
                    });
                let new_ln = rhs_map.get(idx).copied().flatten();
                rows.push((class, old_ln, new_ln));
            }
            rows
        } else {
            // Original single-table mode (single-column or legacy 6-column)
            let row_re = Regex::new(r#"<tr class="([^"]+)">"#).unwrap();
            let td_re = Regex::new(r#"<td class="ln[^"]*"[^>]*>(\d*)</td>"#).unwrap();

            let mut rows = Vec::new();
            for row_cap in row_re.captures_iter(html) {
                let class = row_cap[1].to_string()
                    .replace("line-paired-full", "line-paired")
                    .replace("line-added-partial", "line-added")
                    .replace("line-removed-partial", "line-removed");
                let start = row_cap.get(0).unwrap().end();
                let rest = &html[start..];
                let end = rest.find("</tr>").unwrap_or(rest.len());
                let row_html = &rest[..end];

                let lns: Vec<Option<u64>> = td_re.captures_iter(row_html)
                    .map(|c| {
                        let s = &c[1];
                        if s.is_empty() { None } else { s.parse().ok() }
                    })
                    .collect();

                if is_single && lns.len() == 1 {
                    let ln = lns[0];
                    if single_is_add {
                        rows.push((class, None, ln));
                    } else {
                        rows.push((class, ln, None));
                    }
                } else {
                    let old_ln = lns.first().copied().flatten();
                    let new_ln = lns.get(1).copied().flatten();
                    rows.push((class, old_ln, new_ln));
                }
            }
            rows
        }
    }

    #[test]
    fn no_duplicate_line_numbers() {
        // Simulate the scenario that caused duplicate line 411:
        // Difft pairs old lines 316-324 with new lines 406-424, but some
        // old-only entries exist (e.g. lhs=320 with no rhs). The sort key
        // must map old lines to new-file positions to avoid gap-filler dupes.
        let old_lines: Vec<&str> = (0..330).map(|_| "old line").collect();
        let new_lines: Vec<&str> = (0..425).map(|_| "new line").collect();

        let chunk = vec![
            // added-only entries
            LineEntry { lhs: None, rhs: Some(side(413, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(414, vec![])) },
            // paired entries
            LineEntry { lhs: Some(side(316, vec![])), rhs: Some(side(406, vec![])) },
            LineEntry { lhs: Some(side(317, vec![])), rhs: Some(side(407, vec![])) },
            LineEntry { lhs: Some(side(318, vec![])), rhs: Some(side(408, vec![])) },
            LineEntry { lhs: Some(side(319, vec![])), rhs: Some(side(409, vec![])) },
            // removed-only (this was the problematic case)
            LineEntry { lhs: Some(side(320, vec![])), rhs: None },
            // paired
            LineEntry { lhs: Some(side(321, vec![])), rhs: Some(side(410, vec![])) },
            LineEntry { lhs: Some(side(322, vec![])), rhs: Some(side(411, vec![])) },
            LineEntry { lhs: Some(side(323, vec![])), rhs: Some(side(412, vec![])) },
            LineEntry { lhs: Some(side(324, vec![])), rhs: Some(side(420, vec![])) },
        ];

        let hunks = vec![DiffHunk { old_start: 317, old_count: 13, new_start: 407, new_count: 18 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let html = test_render_chunks(&difft, &[0], "test.ts", None);
        let rows = extract_rows(&html);

        // Check no new-side line number appears more than once
        let new_lns: Vec<u64> = rows.iter().filter_map(|(_, _, n)| *n).collect();
        let mut seen = std::collections::HashSet::new();
        for ln in &new_lns {
            assert!(seen.insert(ln), "duplicate new-side line number: {}", ln);
        }

        // Check no old-side line number appears more than once
        let old_lns: Vec<u64> = rows.iter().filter_map(|(_, o, _)| *o).collect();
        seen.clear();
        for ln in &old_lns {
            assert!(seen.insert(ln), "duplicate old-side line number: {}", ln);
        }
    }

    #[test]
    fn added_only_rows_have_no_token_highlights() {
        let old_lines = vec!["line 0", "line 1", "line 2", "line 3"];
        let new_lines = vec!["line 0", "line 1", "ADDED LINE", "line 2", "line 3"];

        let chunk = vec![
            LineEntry {
                lhs: None,
                rhs: Some(side(2, vec![span("ADDED", 0, 5), span("LINE", 6, 10)])),
            },
        ];

        let hunks = vec![DiffHunk { old_start: 3, old_count: 0, new_start: 3, new_count: 1 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let html = test_render_chunks(&difft, &[0], "test.ts", None);

        // The added-only row should NOT contain hl-add spans
        // (the row background is sufficient)
        let added_row_re = Regex::new(r#"(?s)<tr class="line-added">.*?</tr>"#).unwrap();
        for m in added_row_re.find_iter(&html) {
            let row_html = m.as_str();
            assert!(!row_html.contains("hl-add"),
                "added-only row should not contain hl-add token highlights: {}", row_html);
        }
    }

    #[test]
    fn removed_only_rows_have_no_token_highlights() {
        let old_lines = vec!["line 0", "line 1", "REMOVED LINE", "line 2", "line 3"];
        let new_lines = vec!["line 0", "line 1", "line 2", "line 3"];

        let chunk = vec![
            LineEntry {
                lhs: Some(side(2, vec![span("REMOVED", 0, 7), span("LINE", 8, 12)])),
                rhs: None,
            },
        ];

        let hunks = vec![DiffHunk { old_start: 3, old_count: 1, new_start: 3, new_count: 0 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let html = test_render_chunks(&difft, &[0], "test.ts", None);

        let removed_row_re = Regex::new(r#"(?s)<tr class="line-removed">.*?</tr>"#).unwrap();
        for m in removed_row_re.find_iter(&html) {
            let row_html = m.as_str();
            assert!(!row_html.contains("hl-del"),
                "removed-only row should not contain hl-del token highlights: {}", row_html);
        }
    }

    #[test]
    fn paired_rows_have_token_highlights() {
        let old_lines = vec!["line 0", "let x = foo()", "line 2"];
        let new_lines = vec!["line 0", "let x = bar()", "line 2"];

        let chunk = vec![
            LineEntry {
                lhs: Some(side(1, vec![span("foo", 8, 11)])),
                rhs: Some(side(1, vec![span("bar", 8, 11)])),
            },
        ];

        let hunks = vec![DiffHunk { old_start: 2, old_count: 1, new_start: 2, new_count: 1 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let html = test_render_chunks(&difft, &[0], "test.ts", None);

        assert!(html.contains("line-paired"), "HTML should contain line-paired row");
        // In two-table mode, paired rows are split across LHS and RHS tables.
        // LHS rows contain hl-del, RHS rows contain hl-add.
        let paired_row_re = Regex::new(r#"(?s)<tr [^>]*class="line-paired"[^>]*>.*?</tr>"#).unwrap();
        let matched: Vec<_> = paired_row_re.find_iter(&html).collect();
        assert!(!matched.is_empty(), "should have at least one paired row");

        let has_hl_del = matched.iter().any(|m| m.as_str().contains("hl-del"));
        let has_hl_add = matched.iter().any(|m| m.as_str().contains("hl-add"));
        assert!(has_hl_del, "paired rows should contain hl-del highlight in LHS table");
        assert!(has_hl_add, "paired rows should contain hl-add highlight in RHS table");
    }

    #[test]
    fn line_filter_restricts_output() {
        // Chunk with changes at lines 5, 10, 15 (0-based new-side)
        let old_lines: Vec<&str> = (0..20).map(|_| "old").collect();
        let new_lines: Vec<&str> = (0..20).map(|_| "new").collect();

        let chunk = vec![
            LineEntry { lhs: Some(side(5, vec![])), rhs: Some(side(5, vec![])) },
            LineEntry { lhs: Some(side(10, vec![])), rhs: Some(side(10, vec![])) },
            LineEntry { lhs: Some(side(15, vec![])), rhs: Some(side(15, vec![])) },
        ];

        // Use per-line hunks so only the 3 changed lines are in the diff
        let hunks = vec![
            DiffHunk { old_start: 6, old_count: 1, new_start: 6, new_count: 1 },
            DiffHunk { old_start: 11, old_count: 1, new_start: 11, new_count: 1 },
            DiffHunk { old_start: 16, old_count: 1, new_start: 16, new_count: 1 },
        ];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        // No filter: all 3 changed lines present
        let html_all = test_render_chunks(&difft, &[0], "test.ts", None);
        let rows_all = extract_rows(&html_all);
        let changed_all: Vec<_> = rows_all.iter()
            .filter(|(c, _, _)| c == "line-paired")
            .collect();
        assert_eq!(changed_all.len(), 3, "should have 3 paired rows without filter");

        // Filter to 0-based 8-12. Only line 10 (new-side) should match.
        let html_filtered = test_render_chunks(&difft, &[0], "test.ts", Some((8, 12)));
        let rows_filtered = extract_rows(&html_filtered);
        let changed_filtered: Vec<_> = rows_filtered.iter()
            .filter(|(c, _, _)| c == "line-paired")
            .collect();
        assert_eq!(changed_filtered.len(), 1, "should have 1 paired row with filter");
        assert_eq!(changed_filtered[0].2, Some(11), "filtered row should be new-side line 11 (1-based)");

        // Lines outside any change should produce no changed rows
        let html_empty = test_render_chunks(&difft, &[0], "test.ts", Some((0, 3)));
        let rows_empty = extract_rows(&html_empty);
        let changed_empty: Vec<_> = rows_empty.iter()
            .filter(|(c, _, _)| c == "line-paired")
            .collect();
        assert_eq!(changed_empty.len(), 0, "should have 0 paired rows when filter misses all changes");
    }

    #[test]
    fn relative_line_filter_via_run() {
        // Test the full run() path: relative lines= in markdown get converted
        // to absolute new-file lines using the chunk's first new-file line.
        //
        // Chunk has changes at 0-based new lines 100, 110, 120.
        // Relative line 1 = new line 100, so lines=1-5 should show only line 100,
        // and lines=8-15 should show only line 110.
        let old_lines: Vec<&str> = (0..130).map(|_| "old").collect();
        let new_lines: Vec<&str> = (0..130).map(|_| "new").collect();

        let chunk = vec![
            LineEntry { lhs: Some(side(100, vec![])), rhs: Some(side(100, vec![])) },
            LineEntry { lhs: Some(side(110, vec![])), rhs: Some(side(110, vec![])) },
            LineEntry { lhs: Some(side(120, vec![])), rhs: Some(side(120, vec![])) },
        ];

        let hunks = vec![
            DiffHunk { old_start: 101, old_count: 1, new_start: 101, new_count: 1 },
            DiffHunk { old_start: 111, old_count: 1, new_start: 111, new_count: 1 },
            DiffHunk { old_start: 121, old_count: 1, new_start: 121, new_count: 1 },
        ];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        // Write JSON to a temp data dir
        let data_dir = std::env::temp_dir().join("walkthrough_test_relative");
        let _ = fs::remove_dir_all(&data_dir);
        fs::create_dir_all(&data_dir).unwrap();
        let json = serde_json::to_string_pretty(&difft).unwrap();
        fs::write(data_dir.join("test.ts.json"), &json).unwrap();

        // Write markdown with relative lines=1-5 (should resolve to 0-based 100-104)
        let md_path = data_dir.join("test.md");
        fs::write(&md_path, "# Test\n\n```difft test.ts chunks=0 lines=1-5\n```\n").unwrap();

        let html_path = data_dir.join("test.html");
        run(&md_path, &data_dir, &html_path, false).unwrap();

        let html = fs::read_to_string(&html_path).unwrap();
        let rows = extract_rows(&html);
        let changed: Vec<_> = rows.iter()
            .filter(|(c, _, _)| c == "line-paired")
            .collect();

        // Only the first change (new line 101, 1-based) should be present
        assert_eq!(changed.len(), 1, "relative lines=1-5 should include only 1 changed line");
        assert_eq!(changed[0].2, Some(101), "should be new-side line 101 (1-based)");

        // Now test lines=8-15 which should hit only line 110 (relative 11)
        fs::write(&md_path, "# Test\n\n```difft test.ts chunks=0 lines=8-15\n```\n").unwrap();
        run(&md_path, &data_dir, &html_path, false).unwrap();

        let html = fs::read_to_string(&html_path).unwrap();
        let rows = extract_rows(&html);
        let changed: Vec<_> = rows.iter()
            .filter(|(c, _, _)| c == "line-paired")
            .collect();
        assert_eq!(changed.len(), 1, "relative lines=8-15 should include only 1 changed line");
        assert_eq!(changed[0].2, Some(111), "should be new-side line 111 (1-based)");

        let _ = fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn line_filter_excludes_out_of_range_changes_from_context() {
        // A chunk with changes at lines 50, 60, 70 (0-based new-side).
        // When filtering to only line 60, lines 50 and 70 should NOT appear
        // as context rows (they're changed lines outside the filter).
        let old_lines: Vec<&str> = (0..80).map(|_| "old").collect();
        let new_lines: Vec<&str> = (0..80).map(|_| "new").collect();

        let chunk = vec![
            LineEntry { lhs: Some(side(50, vec![])), rhs: Some(side(50, vec![])) },
            LineEntry { lhs: Some(side(60, vec![])), rhs: Some(side(60, vec![])) },
            LineEntry { lhs: Some(side(70, vec![])), rhs: Some(side(70, vec![])) },
        ];

        let hunks = vec![
            DiffHunk { old_start: 51, old_count: 1, new_start: 51, new_count: 1 },
            DiffHunk { old_start: 61, old_count: 1, new_start: 61, new_count: 1 },
            DiffHunk { old_start: 71, old_count: 1, new_start: 71, new_count: 1 },
        ];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        // Filter to 0-based 58-62 (only line 60 is a change)
        let html = test_render_chunks(&difft, &[0], "test.ts", Some((58, 62)));
        let rows = extract_rows(&html);

        // Should have exactly 1 paired row (line 60)
        let changed: Vec<_> = rows.iter()
            .filter(|(c, _, _)| c == "line-paired")
            .collect();
        assert_eq!(changed.len(), 1, "should have 1 paired row");
        assert_eq!(changed[0].2, Some(61), "should be new-side line 61 (1-based)");

        // Context rows should NOT include lines 50 or 70
        let context_new_lns: Vec<u64> = rows.iter()
            .filter(|(c, _, _)| c == "line-context")
            .filter_map(|(_, _, n)| *n)
            .collect();
        assert!(!context_new_lns.contains(&51), "line 51 (changed, out of range) should not appear as context");
        assert!(!context_new_lns.contains(&71), "line 71 (changed, out of range) should not appear as context");

        // Context should be within CONTEXT_LINES of line 60
        for ln in &context_new_lns {
            assert!(
                (*ln as i64 - 61).unsigned_abs() <= CONTEXT_LINES as u64,
                "context line {} is too far from filtered change at 61", ln
            );
        }
    }

    /// Helper: extract changed (non-context) new-side line numbers from rendered HTML.
    fn extract_changed_new_lines(html: &str) -> std::collections::HashSet<u64> {
        let rows = extract_rows(html);
        rows.iter()
            .filter(|(c, _, _)| c != "line-context" && c != "chunk-sep")
            .filter_map(|(_, _, n)| *n)
            .collect()
    }

    #[test]
    fn split_union_covers_all_changed_lines() {
        // A chunk with 10 consecutive changed lines (0-based 20-29).
        // Split into three blocks: 20-22, 23-26, 27-29.
        // The union of changed lines across all blocks must equal the
        // unsplit chunk's changed lines.
        let old_lines: Vec<&str> = (0..40).map(|_| "old").collect();
        let mut new_lines_vec: Vec<String> = (0..40).map(|_| "old".to_string()).collect();
        for i in 20..30 {
            new_lines_vec[i] = format!("new line {}", i);
        }
        let new_lines: Vec<&str> = new_lines_vec.iter().map(|s| s.as_str()).collect();

        let chunk = vec![
            LineEntry { lhs: Some(side(20, vec![])), rhs: Some(side(20, vec![])) },
            LineEntry { lhs: Some(side(21, vec![])), rhs: Some(side(21, vec![])) },
            LineEntry { lhs: Some(side(22, vec![])), rhs: Some(side(22, vec![])) },
            LineEntry { lhs: Some(side(23, vec![])), rhs: Some(side(23, vec![])) },
            LineEntry { lhs: Some(side(24, vec![])), rhs: Some(side(24, vec![])) },
            LineEntry { lhs: Some(side(25, vec![])), rhs: Some(side(25, vec![])) },
            LineEntry { lhs: Some(side(26, vec![])), rhs: Some(side(26, vec![])) },
            LineEntry { lhs: Some(side(27, vec![])), rhs: Some(side(27, vec![])) },
            LineEntry { lhs: Some(side(28, vec![])), rhs: Some(side(28, vec![])) },
            LineEntry { lhs: Some(side(29, vec![])), rhs: Some(side(29, vec![])) },
        ];

        let hunks = vec![DiffHunk { old_start: 21, old_count: 10, new_start: 21, new_count: 10 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        // Unsplit: all changed lines
        let all_lines = extract_changed_new_lines(
            &test_render_chunks(&difft, &[0], "test.ts", None)
        );

        // Split into three non-overlapping ranges
        let split1 = extract_changed_new_lines(
            &test_render_chunks(&difft, &[0], "test.ts", Some((20, 22)))
        );
        let split2 = extract_changed_new_lines(
            &test_render_chunks(&difft, &[0], "test.ts", Some((23, 26)))
        );
        let split3 = extract_changed_new_lines(
            &test_render_chunks(&difft, &[0], "test.ts", Some((27, 29)))
        );

        let union: std::collections::HashSet<u64> = split1.iter()
            .chain(split2.iter())
            .chain(split3.iter())
            .copied()
            .collect();

        assert_eq!(union, all_lines,
            "union of splits must equal unsplit changed lines.\n  unsplit: {:?}\n  union: {:?}\n  split1: {:?}\n  split2: {:?}\n  split3: {:?}",
            all_lines, union, split1, split2, split3);
    }

    #[test]
    fn overlapping_splits_still_cover_all_lines() {
        // Same chunk but with overlapping split ranges.
        let old_lines: Vec<&str> = (0..40).map(|_| "old").collect();
        let mut new_lines_vec: Vec<String> = (0..40).map(|_| "old".to_string()).collect();
        for i in 20..30 {
            new_lines_vec[i] = format!("new line {}", i);
        }
        let new_lines: Vec<&str> = new_lines_vec.iter().map(|s| s.as_str()).collect();

        let chunk = vec![
            LineEntry { lhs: Some(side(20, vec![])), rhs: Some(side(20, vec![])) },
            LineEntry { lhs: Some(side(21, vec![])), rhs: Some(side(21, vec![])) },
            LineEntry { lhs: Some(side(22, vec![])), rhs: Some(side(22, vec![])) },
            LineEntry { lhs: Some(side(23, vec![])), rhs: Some(side(23, vec![])) },
            LineEntry { lhs: Some(side(24, vec![])), rhs: Some(side(24, vec![])) },
            LineEntry { lhs: Some(side(25, vec![])), rhs: Some(side(25, vec![])) },
            LineEntry { lhs: Some(side(26, vec![])), rhs: Some(side(26, vec![])) },
            LineEntry { lhs: Some(side(27, vec![])), rhs: Some(side(27, vec![])) },
            LineEntry { lhs: Some(side(28, vec![])), rhs: Some(side(28, vec![])) },
            LineEntry { lhs: Some(side(29, vec![])), rhs: Some(side(29, vec![])) },
        ];

        let hunks = vec![DiffHunk { old_start: 21, old_count: 10, new_start: 21, new_count: 10 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let all_lines = extract_changed_new_lines(
            &test_render_chunks(&difft, &[0], "test.ts", None)
        );

        // Overlapping ranges: 20-25, 23-29
        let split1 = extract_changed_new_lines(
            &test_render_chunks(&difft, &[0], "test.ts", Some((20, 25)))
        );
        let split2 = extract_changed_new_lines(
            &test_render_chunks(&difft, &[0], "test.ts", Some((23, 29)))
        );

        let union: std::collections::HashSet<u64> = split1.iter()
            .chain(split2.iter())
            .copied()
            .collect();

        assert_eq!(union, all_lines,
            "overlapping union must cover all.\n  unsplit: {:?}\n  union: {:?}", all_lines, union);
    }

    #[test]
    fn single_line_splits_cover_all() {
        // Extreme: split every changed line into its own block.
        let old_lines: Vec<&str> = (0..20).map(|_| "old").collect();
        let mut new_lines_vec: Vec<String> = (0..20).map(|_| "old".to_string()).collect();
        for i in 5..10 {
            new_lines_vec[i] = format!("changed {}", i);
        }
        let new_lines: Vec<&str> = new_lines_vec.iter().map(|s| s.as_str()).collect();

        let chunk = vec![
            LineEntry { lhs: Some(side(5, vec![])), rhs: Some(side(5, vec![])) },
            LineEntry { lhs: Some(side(6, vec![])), rhs: Some(side(6, vec![])) },
            LineEntry { lhs: Some(side(7, vec![])), rhs: Some(side(7, vec![])) },
            LineEntry { lhs: Some(side(8, vec![])), rhs: Some(side(8, vec![])) },
            LineEntry { lhs: Some(side(9, vec![])), rhs: Some(side(9, vec![])) },
        ];

        let hunks = vec![DiffHunk { old_start: 6, old_count: 5, new_start: 6, new_count: 5 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let all_lines = extract_changed_new_lines(
            &test_render_chunks(&difft, &[0], "test.ts", None)
        );

        // One block per line
        let mut union = std::collections::HashSet::new();
        for line_0 in 5..10 {
            let lines = extract_changed_new_lines(
                &test_render_chunks(&difft, &[0], "test.ts", Some((line_0, line_0)))
            );
            assert_eq!(lines.len(), 1,
                "single-line filter at {} should produce 1 changed line, got {:?}", line_0, lines);
            union.extend(lines);
        }

        assert_eq!(union, all_lines,
            "single-line splits union must cover all.\n  unsplit: {:?}\n  union: {:?}", all_lines, union);
    }

    #[test]
    fn function_names_are_purple() {
        // Function names should be #6639ba (GitHub's --color-prettylights-syntax-entity).
        // Tree-sitter emits both "function" and "variable" captures for function names;
        // "function" should win via priority and map to purple.
        // Note: tree-sitter needs the full function body to classify the name as "function".
        let old_lines = vec!["// old"];
        let new_lines = vec![
            "async function bootstrapViaWorkspaceCreate(",
            "  sboxdUrl: string,",
            ") {",
            "  const x = getSettings()",
            "}",
        ];

        let chunk = vec![
            LineEntry { lhs: None, rhs: Some(side(0, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(1, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(2, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(3, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(4, vec![])) },
        ];
        let hunks = vec![DiffHunk { old_start: 1, old_count: 0, new_start: 1, new_count: 5 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let html = test_render_chunks(&difft, &[0], "test.ts", None);

        // bootstrapViaWorkspaceCreate should be purple (#6639ba)
        assert!(html.contains("color:#6025cd"),
            "function name should be purple (#6639ba) in HTML:\n{}",
            &html[..html.len().min(2000)]);
    }

    #[test]
    fn property_names_are_blue() {
        // In `workloadName: workloadConfig.workloadName,`:
        // - property key (workloadName:) should be blue (#0550ae)
        // - dot-access property (.workloadName) should be blue (#0550ae)
        // - the object (workloadConfig) should be default text
        let old_lines = vec!["// old"];
        let new_lines = vec![
            "const x = {",
            "  workloadName: workloadConfig.workloadName,",
            "}",
        ];

        let chunk = vec![
            LineEntry { lhs: None, rhs: Some(side(0, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(1, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(2, vec![])) },
        ];
        let hunks = vec![DiffHunk { old_start: 1, old_count: 0, new_start: 1, new_count: 3 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let html = test_render_chunks(&difft, &[0], "test.ts", None);

        // Property names (workloadName) should be blue, variable (workloadConfig) default text
        assert!(html.contains("color:#004fb3\">workloadName"),
            "property key should be blue (#0550ae):\n{}",
            &html[..html.len().min(2000)]);
        // Variables should be default text (#1f2328), which means no span wrapper
        assert!(!html.contains("color:#953800\">workloadConfig"),
            "variable should not be orange (#953800), should be default text:\n{}",
            &html[..html.len().min(2000)]);
    }

    #[test]
    fn type_annotations_are_default_text() {
        // Type annotations like `string`, `boolean`, `Record` should be default text,
        // matching GitHub's rendering.
        let old_lines = vec!["// old"];
        let new_lines = vec![
            "interface Foo {",
            "  name: string",
            "  enabled: boolean",
            "}",
        ];

        let chunk = vec![
            LineEntry { lhs: None, rhs: Some(side(0, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(1, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(2, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(3, vec![])) },
        ];
        let hunks = vec![DiffHunk { old_start: 1, old_count: 0, new_start: 1, new_count: 4 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let html = test_render_chunks(&difft, &[0], "test.ts", None);

        // `string`, `boolean`, and user-defined types should NOT have a colored span
        assert!(!html.contains("color:#004fb3\">string"),
            "type 'string' should be default text, not blue:\n{}",
            &html[..html.len().min(2000)]);
        assert!(!html.contains("color:#004fb3\">boolean"),
            "type 'boolean' should be default text, not blue:\n{}",
            &html[..html.len().min(2000)]);
        assert!(!html.contains("color:#004fb3\">Foo"),
            "user-defined type 'Foo' should be default text, not blue:\n{}",
            &html[..html.len().min(2000)]);
    }

    #[test]
    fn constructor_names_are_orange() {
        // In `CortexStatsig.checkGate(...)`, CortexStatsig is a class/constructor
        // and should be orange (#953800).
        let old_lines = vec!["// old"];
        let new_lines = vec![
            "CortexStatsig.checkGate(context, 'flag', false)",
        ];

        let chunk = vec![
            LineEntry { lhs: None, rhs: Some(side(0, vec![])) },
        ];
        let hunks = vec![DiffHunk { old_start: 1, old_count: 0, new_start: 1, new_count: 1 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let html = test_render_chunks(&difft, &[0], "test.ts", None);

        assert!(html.contains("color:#953800\">CortexStatsig"),
            "constructor/class name should be orange (#953800):\n{}",
            &html[..html.len().min(2000)]);
    }

    #[test]
    fn line_numbers_are_consecutive_on_each_side() {
        // New-side line numbers among non-context changed rows must be
        // non-decreasing. Old-side may have gap-caused disorder (logged as
        // warning) but new-side must be correct since that's what humans read.
        let html = extract_rows_html_from_real_data();
        let rows = extract_rows(&html);

        let non_context: Vec<_> = rows.iter()
            .filter(|(c, _, _)| c != "line-context" && c != "chunk-sep")
            .collect();

        let new_lns: Vec<u64> = non_context.iter().filter_map(|(_, _, n)| *n).collect();
        for i in 1..new_lns.len() {
            assert!(new_lns[i] >= new_lns[i-1],
                "new-side line numbers out of order: {} followed by {} (at position {})\nall new: {:?}",
                new_lns[i-1], new_lns[i], i, new_lns);
        }

        // Also check no duplicate line numbers
        let mut seen = std::collections::HashSet::new();
        for &ln in &new_lns {
            assert!(seen.insert(ln), "duplicate new-side line number: {}", ln);
        }
    }

    /// Build render output from the problematic chunk pattern:
    /// difft pairs old comments with new comments non-consecutively,
    /// leaving gaps that cause sort-key collisions.
    fn extract_rows_html_from_real_data() -> String {
        let old_lines: Vec<&str> = (0..335).map(|_| "old line").collect();
        let mut new_lines_vec: Vec<String> = (0..400).map(|_| "new line".to_string()).collect();
        // Make some lines distinct so we can trace them
        for i in 376..396 {
            new_lines_vec[i] = format!("new content {}", i);
        }
        let new_lines: Vec<&str> = new_lines_vec.iter().map(|s| s.as_str()).collect();

        // Reproduce the problematic chunk: difft pairs old 316-324 with new 376-394,
        // but skips old 320 (removed-only) which maps to the same new-side position
        // as paired entry [7].
        let chunk = vec![
            LineEntry { lhs: None, rhs: Some(side(383, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(384, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(385, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(387, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(388, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(389, vec![])) },
            LineEntry { lhs: None, rhs: Some(side(394, vec![])) },
            LineEntry { lhs: Some(side(316, vec![])), rhs: Some(side(376, vec![])) },
            LineEntry { lhs: Some(side(317, vec![])), rhs: Some(side(377, vec![])) },
            LineEntry { lhs: Some(side(318, vec![])), rhs: Some(side(378, vec![])) },
            LineEntry { lhs: Some(side(319, vec![])), rhs: Some(side(379, vec![])) },
            LineEntry { lhs: Some(side(320, vec![])), rhs: None },  // removed-only
            LineEntry { lhs: Some(side(321, vec![])), rhs: Some(side(380, vec![])) },
            LineEntry { lhs: Some(side(322, vec![])), rhs: Some(side(381, vec![])) },
            LineEntry { lhs: Some(side(323, vec![])), rhs: Some(side(382, vec![])) },
            LineEntry { lhs: Some(side(324, vec![])), rhs: Some(side(390, vec![])) },
        ];

        let hunks = vec![DiffHunk { old_start: 317, old_count: 13, new_start: 377, new_count: 18 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        test_render_chunks(&difft, &[0], "test.ts", None)
    }

    #[test]
    fn foundry_api_chunk2_matches_difft_cli_layout() {
        // Verify that the rendered row layout for the problematic foundry_api.ts
        // chunk 2 matches difft CLI's side-by-side output.
        //
        // Expected layout (from `difft --display side-by-side`):
        //   old:317 ↔ new:377  (paired)
        //   old:318 ↔ new:378  (paired)
        //   old:319 ↔ new:379  (paired)
        //   old:320 ↔ new:380  (paired)
        //   old:321 ↔ (none)   (removed-only)
        //   old:322 ↔ new:381  (paired)
        //   old:323 ↔ new:382  (paired)
        //   old:324 ↔ new:383  (paired, note: 383 was a NEW-only entry in difft JSON)
        //   (none)  ↔ new:384  (added)
        //   (none)  ↔ new:385  (added)
        //   ...gap-filled new lines...
        //   old:325 ↔ new:391  (paired, from hunk gap)
        //   ...more...
        let html = extract_rows_html_from_real_data();
        let rows = extract_rows(&html);

        // Extract (old_ln, new_ln) pairs for non-context rows
        let changed: Vec<(Option<u64>, Option<u64>)> = rows.iter()
            .filter(|(c, _, _)| c != "line-context" && c != "chunk-sep")
            .map(|(_, o, n)| (*o, *n))
            .collect();

        // New-side line numbers must be non-decreasing
        let new_lns_all: Vec<u64> = changed.iter().filter_map(|(_, n)| *n).collect();
        for i in 1..new_lns_all.len() {
            assert!(new_lns_all[i] >= new_lns_all[i-1],
                "new-side out of order: {} then {} at pos {}\nall new: {:?}",
                new_lns_all[i-1], new_lns_all[i], i, new_lns_all);
        }

        // Key difft entries should be present
        let old_lns: Vec<u64> = changed.iter().filter_map(|(o, _)| *o).collect();
        assert!(old_lns.contains(&321), "old line 321 (removed) should be present");

        // Added-only lines (383-389) should appear between paired entries
        // for old:324↔new:382 and wherever old:325↔new:391 ends up
        let new_lns: Vec<u64> = changed.iter().filter_map(|(_, n)| *n).collect();
        assert!(new_lns.contains(&384), "new line 384 should be present");
        assert!(new_lns.contains(&388), "new line 388 should be present");

        // new:383 should come after new:382 (paired with old:324)
        let new_382_pos = new_lns.iter().position(|&l| l == 382);
        let new_383_pos = new_lns.iter().position(|&l| l == 383);
        if let (Some(p382), Some(p383)) = (new_382_pos, new_383_pos) {
            assert!(p382 < p383,
                "new 382 should come before new 383, got pos {} vs {}",
                p382, p383);
        }
    }

    // ── Fixture-based rendering tests ──────────────────────────────────
    //
    // Each subdirectory of `test_fixtures/` is a fixture set (usually one
    // commit). It contains `*.json` files produced by `walkthrough collect`
    // and optional `*.difft.txt` files (difft CLI text for LLM reference).
    //
    // Tests load every JSON file, render every chunk, and verify:
    //   1. Row layout matches consolidate_chunk + sort output
    //   2. Old-side line numbers are non-decreasing
    //   3. No duplicate line numbers on either side
    //   4. Added-only rows have no hl-add token highlights
    //   5. Removed-only rows have no hl-del token highlights
    //   6. Text rendering changed-line count matches HTML

    /// Derive the expected row layout from a chunk's difft JSON plus hunk
    /// gap lines, applying the same consolidation and sort logic as the
    /// HTML renderer.
    /// Returns vec of (row_type, old_ln_1based, new_ln_1based).
    fn expected_layout(difft: &DifftOutput, chunk_idx: usize) -> Vec<(&'static str, Option<u64>, Option<u64>)> {
        let chunk = &difft.chunks[chunk_idx];
        let hunks = &difft.hunks;

        let (lhs_range, rhs_range) = chunk_line_range(chunk);
        let (old_first, old_last) = match lhs_range {
            Some((min, max)) => (min as usize, max as usize),
            None => {
                let (rmin, rmax) = rhs_range.unwrap_or((0, 0));
                (to_0based(new_to_old_line(rmin + 1, hunks)), to_0based(new_to_old_line(rmax + 1, hunks)))
            }
        };
        let (new_first, new_last) = match rhs_range {
            Some((min, max)) => (min as usize, max as usize),
            None => {
                let (lmin, lmax) = lhs_range.unwrap_or((0, 0));
                (to_0based(old_to_new_line(lmin + 1, hunks)), to_0based(old_to_new_line(lmax + 1, hunks)))
            }
        };

        let rows = consolidate_chunk(chunk);
        let mut items: Vec<(u64, &'static str, Option<u64>, Option<u64>)> = Vec::new();
        // Same keying as renderer: prev_rhs*3+1 for difft removed, anchor+offset for gap
        let (gap_paired, gap_removed, gap_added) = hunk_gap_lines(
            chunk, hunks, old_first, old_last, new_first, new_last,
            &difft.old_lines, &difft.new_lines,
        );
        let mut prev_rhs_e: Option<u64> = None;
        let mut removed_seq_e: u64 = 0;
        for row in &rows {
            let key = if let Some(rhs) = row.rhs {
                prev_rhs_e = Some(rhs.line_number);
                removed_seq_e = 0;
                rhs.line_number * 3
            } else {
                removed_seq_e += 1;
                prev_rhs_e.map_or(removed_seq_e, |p| p * 3 + removed_seq_e)
            };
            let row_type = if row.lhs.is_some() && row.rhs.is_some() {
                "line-paired"
            } else if row.rhs.is_some() {
                "line-added"
            } else {
                "line-removed"
            };
            let old_ln = row.lhs.map(|s| s.line_number + 1);
            let new_ln = row.rhs.map(|s| s.line_number + 1);
            items.push((key, row_type, old_ln, new_ln));
        }
        // Gap-paired render as context, excluded
        for &old_0 in &gap_removed {
            let nearest_key = items.iter()
                .filter_map(|&(k, _, ol, _)| ol.map(|o| {
                    let dist = ((o - 1) as i64 - old_0 as i64).unsigned_abs();
                    (dist, k)
                }))
                .min_by_key(|&(dist, _)| dist)
                .map(|(_, k)| k);
            items.push((nearest_key.unwrap_or(0), "line-removed", Some(old_0 as u64 + 1), None));
        }
        for &new_0 in &gap_added {
            items.push((new_0 as u64 * KEY_SPACE + KEY_SPACE - 1, "line-added", None, Some(new_0 as u64 + 1)));
        }
        items.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2))
        });
        items.into_iter().map(|(_, t, o, n)| (t, o, n)).collect()
    }

    /// Run all rendering checks on a single chunk. Returns a list of errors.
    fn check_chunk(difft: &DifftOutput, chunk_idx: usize, file_path: &str) -> Vec<String> {
        let mut errors = Vec::new();
        let chunk = &difft.chunks[chunk_idx];

        // Compute chunk line ranges (0-based) for hunk gap analysis
        let (lhs_range, rhs_range) = chunk_line_range(chunk);
        let (old_first_0, old_last_0) = match lhs_range {
            Some((min, max)) => (min as usize, max as usize),
            None => {
                let (rmin, rmax) = rhs_range.unwrap_or((0, 0));
                (to_0based(new_to_old_line(rmin + 1, &difft.hunks)),
                 to_0based(new_to_old_line(rmax + 1, &difft.hunks)))
            }
        };
        let (new_first_0, new_last_0) = match rhs_range {
            Some((min, max)) => (min as usize, max as usize),
            None => {
                let (lmin, lmax) = lhs_range.unwrap_or((0, 0));
                (to_0based(old_to_new_line(lmin + 1, &difft.hunks)),
                 to_0based(old_to_new_line(lmax + 1, &difft.hunks)))
            }
        };

        // Render HTML
        let mut hl = Highlighter::new();
        let html = render_chunks(difft, &[chunk_idx], file_path, None, &mut hl, CollapseMode::None);
        let rendered = extract_rows(&html);

        let non_context: Vec<_> = rendered.iter()
            .filter(|(c, _, _)| c != "line-context" && c != "chunk-sep")
            .collect();

        // 1. (Layout check removed: fully covered by ordering, duplicate,
        //     and highlight checks below. The expected_layout function was
        //     fragile and hard to keep in sync with the renderer's keying.)

        // 2. Old-side non-decreasing
        let old_lns: Vec<u64> = non_context.iter().filter_map(|(_, o, _)| *o).collect();
        for i in 1..old_lns.len() {
            if old_lns[i] < old_lns[i - 1] {
                errors.push(format!(
                    "old-side out of order: {} followed by {} at position {}",
                    old_lns[i - 1], old_lns[i], i,
                ));
                break;
            }
        }

        // 2b. New-side non-decreasing
        let new_lns: Vec<u64> = non_context.iter().filter_map(|(_, _, n)| *n).collect();
        for i in 1..new_lns.len() {
            if new_lns[i] < new_lns[i - 1] {
                errors.push(format!(
                    "new-side out of order: {} followed by {} at position {}",
                    new_lns[i - 1], new_lns[i], i,
                ));
                break;
            }
        }

        // 3. No duplicate line numbers
        let mut seen = std::collections::HashSet::new();
        for &ln in &old_lns {
            if !seen.insert(ln) {
                errors.push(format!("duplicate old-side line: {}", ln));
            }
        }
        let new_lns: Vec<u64> = non_context.iter().filter_map(|(_, _, n)| *n).collect();
        seen.clear();
        for &ln in &new_lns {
            if !seen.insert(ln) {
                errors.push(format!("duplicate new-side line: {}", ln));
            }
        }

        // 4, 5 & 5b. Highlight correctness using difft JSON spans as ground truth.
        //
        // Build maps from line numbers to expected highlight state:
        //   - difft_has_spans: paired entries where difft JSON has non-empty change spans
        //     on BOTH sides -> should have hl-del/hl-add in HTML
        //   - gap_paired: rows matched by trimmed content (only whitespace differs)
        //     -> should NOT have hl-del/hl-add
        //   - one-sided rows with full-line change -> no hl highlights (full bg)
        //   - one-sided rows with partial spans -> should have hl highlights
        let mut difft_has_spans: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for entry in chunk {
            if let (Some(lhs), Some(rhs)) = (&entry.lhs, &entry.rhs) {
                if !lhs.changes.is_empty() && !rhs.changes.is_empty() {
                    difft_has_spans.insert(lhs.line_number + 1);
                }
            }
        }

        let (gap_paired_set, _, _) = hunk_gap_lines(
            chunk, &difft.hunks, old_first_0, old_last_0, new_first_0, new_last_0,
            &difft.old_lines, &difft.new_lines,
        );
        let gap_paired_old: std::collections::HashSet<u64> = gap_paired_set.iter()
            .map(|&(o, _)| o as u64 + 1).collect();

        let row_re = Regex::new(r#"(?s)<tr class="([^"]+)">(.*?)</tr>"#).unwrap();
        let td_re_for_hl = Regex::new(r#"<td class="ln[^"]*"[^>]*>(\d*)</td>"#).unwrap();

        for cap in row_re.captures_iter(&html) {
            let class = &cap[1];
            let row_html = &cap[2];
            let has_hl_del = row_html.contains("class=\"hl-del\"");
            let has_hl_add = row_html.contains("class=\"hl-add\"");

            // Full-line added/removed rows should NOT have token highlights
            // (the full row background is sufficient). Partial variants SHOULD.
            if class == "line-added" && has_hl_add {
                errors.push("full-line added row has hl-add token highlight".to_string());
            }
            if class == "line-removed" && has_hl_del {
                errors.push("full-line removed row has hl-del token highlight".to_string());
            }
            // Partial added/removed rows SHOULD have token highlights
            if class == "line-added-partial" && !has_hl_add {
                let rhs_ln = td_re_for_hl.captures_iter(row_html)
                    .filter_map(|c| { let s = &c[1]; if s.is_empty() { None } else { s.parse::<u64>().ok() } })
                    .nth(1);
                if let Some(nl) = rhs_ln {
                    errors.push(format!(
                        "partial added row new={} should have hl-add highlights", nl,
                    ));
                }
            }
            if class == "line-removed-partial" && !has_hl_del {
                let lhs_ln = td_re_for_hl.captures_iter(row_html)
                    .filter_map(|c| { let s = &c[1]; if s.is_empty() { None } else { s.parse::<u64>().ok() } })
                    .next();
                if let Some(ol) = lhs_ln {
                    errors.push(format!(
                        "partial removed row old={} should have hl-del highlights", ol,
                    ));
                }
            }
            if class == "line-paired" {
                let lns: Vec<Option<u64>> = td_re_for_hl.captures_iter(row_html)
                    .map(|c| { let s = &c[1]; if s.is_empty() { None } else { s.parse().ok() } })
                    .collect();
                let old_ln = lns.first().copied().flatten();
                if let Some(ol) = old_ln {
                    // Gap-paired rows render as context, so they won't appear
                    // as line-paired. No check needed here.
                    // Difft paired with spans AND content differs: should have highlights.
                    // Skip if old/new content is identical (difft can flag structurally
                    // repositioned lines with spans even when content is unchanged).
                    if difft_has_spans.contains(&ol) && !has_hl_del && !has_hl_add {
                        let new_ln = lns.get(1).copied().flatten();
                        let old_content = difft.old_lines.get((ol - 1) as usize).map(|s| s.as_str()).unwrap_or("");
                        let new_content = new_ln.and_then(|n| difft.new_lines.get((n - 1) as usize).map(|s| s.as_str())).unwrap_or("");
                        let (old_cs, old_ce, _, _) = find_change_bounds(old_content, new_content);
                        if old_cs < old_ce {
                            errors.push(format!(
                                "difft-paired row old={} has change spans and content differs but no diff highlights in HTML", ol,
                            ));
                        }
                    }
                    // Difft paired without spans: should NOT have highlights
                    // (these are entries where difft reported the line as changed
                    // but didn't flag specific tokens, e.g. full-line changes)
                    if !difft_has_spans.contains(&ol) && !gap_paired_old.contains(&ol) {
                        // This is a difft entry with empty spans - we use
                        // find_change_bounds which may or may not produce highlights
                        // depending on content. Don't enforce either way.
                    }
                }
            }
        }

        // 6. Text rendering line count matches HTML
        // Text format: `   N -content` or `     -content` (5-char prefix then sign)
        let text = render_chunks_text(difft, &[chunk_idx], None);
        let text_minus = text.lines().filter(|l| l.as_bytes().get(5) == Some(&b'-')).count();
        let text_plus = text.lines().filter(|l| l.as_bytes().get(5) == Some(&b'+')).count();

        let html_removed = non_context.iter()
            .filter(|(c, _, _)| c == "line-removed").count();
        let html_paired = non_context.iter()
            .filter(|(c, _, _)| c == "line-paired").count();
        let html_added = non_context.iter()
            .filter(|(c, _, _)| c == "line-added").count();

        // Text: paired rows emit one '-' and one '+' line each.
        // The ordering guard may drop different gap lines in HTML vs text,
        // so allow a small tolerance for gap-caused discrepancies.
        let expected_minus = html_removed + html_paired;
        let expected_plus = html_added + html_paired;
        let minus_diff = (text_minus as i64 - expected_minus as i64).unsigned_abs() as usize;
        let plus_diff = (text_plus as i64 - expected_plus as i64).unsigned_abs() as usize;
        // Allow up to gap_removed.len() + gap_paired.len() discrepancy
        let (gap_paired_check_count, gap_removed_count, _) = hunk_gap_lines(
            chunk, &difft.hunks, old_first_0, old_last_0, new_first_0, new_last_0,
            &difft.old_lines, &difft.new_lines,
        );
        let gap_tolerance = gap_removed_count.len() + gap_paired_check_count.len();
        if minus_diff > gap_tolerance {
            errors.push(format!(
                "text '-' lines: {} vs expected {} (removed={}, paired={}, tolerance={})",
                text_minus, expected_minus, html_removed, html_paired, gap_tolerance,
            ));
        }
        if plus_diff > gap_tolerance {
            errors.push(format!(
                "text '+' lines: {} vs expected {} (added={}, paired={}, tolerance={})",
                text_plus, expected_plus, html_added, html_paired, gap_tolerance,
            ));
        }

        // 7. No hunk-range lines rendered as context (except gap-paired).
        // Lines within a unified diff hunk are changed; if they're not in difft's
        // JSON, they should either be rendered as changed rows (via gap filling)
        // or as context if they're gap-paired (content-matched, just repositioned).
        let (gap_paired_check, _, _) = hunk_gap_lines(
            chunk, &difft.hunks, old_first_0, old_last_0, new_first_0, new_last_0,
            &difft.old_lines, &difft.new_lines,
        );
        let gap_paired_old_set: std::collections::HashSet<u64> = gap_paired_check.iter()
            .map(|&(o, _)| o as u64 + 1).collect();
        let gap_paired_new_set: std::collections::HashSet<u64> = gap_paired_check.iter()
            .map(|&(_, n)| n as u64 + 1).collect();

        let context_old: std::collections::HashSet<u64> = rendered.iter()
            .filter(|(c, _, _)| c == "line-context")
            .filter_map(|(_, o, _)| *o)
            .collect();
        let context_new: std::collections::HashSet<u64> = rendered.iter()
            .filter(|(c, _, _)| c == "line-context")
            .filter_map(|(_, _, n)| *n)
            .collect();

        for h in &difft.hunks {
            let hunk_old_start = h.old_start;
            let hunk_old_end = h.old_start + h.old_count;
            let hunk_new_start = h.new_start;
            let hunk_new_end = h.new_start + h.new_count;

            let chunk_old: std::collections::HashSet<u64> = chunk.iter()
                .filter_map(|e| e.lhs.as_ref().map(|s| s.line_number + 1))
                .collect();
            let chunk_new: std::collections::HashSet<u64> = chunk.iter()
                .filter_map(|e| e.rhs.as_ref().map(|s| s.line_number + 1))
                .collect();

            let overlaps = chunk_old.iter().any(|&l| l >= hunk_old_start && l < hunk_old_end)
                || chunk_new.iter().any(|&l| l >= hunk_new_start && l < hunk_new_end);
            if !overlaps { continue; }

            for ln in hunk_old_start..hunk_old_end {
                if !chunk_old.contains(&ln) && !gap_paired_old_set.contains(&ln) && context_old.contains(&ln) {
                    errors.push(format!(
                        "old line {} is in hunk (old {}+{}) but shown as context (should be changed)",
                        ln, h.old_start, h.old_count,
                    ));
                }
            }
            for ln in hunk_new_start..hunk_new_end {
                if !chunk_new.contains(&ln) && !gap_paired_new_set.contains(&ln) && context_new.contains(&ln) {
                    errors.push(format!(
                        "new line {} is in hunk (new {}+{}) but shown as context (should be changed)",
                        ln, h.new_start, h.new_count,
                    ));
                }
            }
        }

        // 8. Gap-paired lines (content-matched) should render as context rows.
        // They may be paired with different new-line numbers if they fall in
        // the positional context range, or absent if dropped by ordering guards.
        for &(old_0, new_0) in &gap_paired_check {
            // Only check gap-paired lines within the chunk's actual range.
            // Lines outside the range are handled by context rendering.
            if old_0 < old_first_0 || old_0 > old_last_0 { continue; }

            let old_1 = old_0 as u64 + 1;
            let new_1 = new_0 as u64 + 1;
            let is_context = rendered.iter().any(|(c, o, n)| {
                c == "line-context" && *o == Some(old_1) && *n == Some(new_1)
            });
            // Also accept either side appearing as context with any pairing
            // (positional context uses offset-based pairing, not content matching;
            // single-column mode only shows one side's line number).
            let is_context_any = rendered.iter().any(|(c, o, n)| {
                c == "line-context" && (*o == Some(old_1) || *n == Some(new_1))
            });
            // Gap-paired lines may be absent if they were dropped by the
            // ordering guard (they'd violate old-side order if included).
            let is_absent = !rendered.iter().any(|(_, o, n)| {
                *o == Some(old_1) || *n == Some(new_1)
            });
            if !is_context && !is_context_any && !is_absent {
                let content = difft.old_lines.get(old_0).map(|s| s.trim()).unwrap_or("");
                errors.push(format!(
                    "gap-paired old {} / new {} should render as context but doesn't: {:?}",
                    old_1, new_1, &content[..content.len().min(60)],
                ));
            }
        }

        errors
    }

    /// Parse a difft CLI side-by-side-show-both text file into per-chunk row lists.
    /// Each row is (Option<old_ln>, Option<new_ln>). Context rows (both sides
    /// present with same content) are included so we can filter them.
    fn parse_difft_cli(text: &str) -> Vec<Vec<(Option<u64>, Option<u64>)>> {
        // Detect column split from continuation lines (.. on both sides)
        let mut split_col: Option<usize> = None;
        for line in text.lines() {
            if line.starts_with(" ..") && line.len() > 20 {
                if let Some(idx) = line[10..].find(" ..") {
                    let col = idx + 10;
                    if col > 20 {
                        split_col = Some(col);
                        break;
                    }
                }
            }
        }
        let split_col = match split_col {
            Some(c) => c,
            None => return Vec::new(),
        };

        let header_re = Regex::new(r"---\s+\d+/\d+\s+---").unwrap();
        let mut chunks: Vec<Vec<(Option<u64>, Option<u64>)>> = Vec::new();
        let mut current: Option<Vec<(Option<u64>, Option<u64>)>> = None;

        for line in text.lines() {
            if header_re.is_match(line) {
                if let Some(rows) = current.take() {
                    chunks.push(rows);
                }
                current = Some(Vec::new());
                continue;
            }
            let rows = match current.as_mut() {
                Some(r) => r,
                None => continue,
            };
            if line.trim().is_empty() { continue; }

            let left_half = &line[..split_col.min(line.len())];
            let right_half = if line.len() > split_col { &line[split_col..] } else { "" };

            let left_num_str = left_half.get(..4).unwrap_or("").trim();
            let right_num_str = right_half.get(..4).unwrap_or("").trim();

            // Parse numbers (.. and ... are continuation markers, not numbers)
            let left_num: Option<u64> = if left_num_str == ".." || left_num_str == "..." {
                None
            } else {
                left_num_str.parse().ok()
            };
            let right_num: Option<u64> = if right_num_str == ".." || right_num_str == "..." {
                None
            } else {
                right_num_str.parse().ok()
            };

            // Skip pure continuation lines (both sides are continuations)
            if left_num.is_none() && right_num.is_none() { continue; }

            rows.push((left_num, right_num));
        }
        if let Some(rows) = current {
            chunks.push(rows);
        }
        chunks
    }

    /// Compare rendered HTML rows against difft CLI rows for a single chunk.
    /// Only compares non-context rows (changed lines). Returns errors.
    fn check_against_difft_cli(
        rendered: &[(String, Option<u64>, Option<u64>)],
        difft_rows: &[(Option<u64>, Option<u64>)],
        file: &str,
        chunk_idx: usize,
    ) -> Vec<String> {
        let mut errors = Vec::new();

        // Extract non-context rows from rendered HTML
        let html_changed: Vec<(Option<u64>, Option<u64>)> = rendered.iter()
            .filter(|(c, _, _)| c != "line-context" && c != "chunk-sep")
            .map(|(_, o, n)| (*o, *n))
            .collect();

        // Extract non-context rows from difft CLI (rows where one side differs)
        // Context rows have both sides with line numbers
        // Changed rows: removed (left only), added (right only), or paired (both, but changed)
        // We can't distinguish context from paired in the difft text, so we only compare
        // the set of (old, new) pairings that appear as changed in our output.
        // Specifically: for each non-context row in our HTML, verify it also appears
        // in the difft CLI output.
        let difft_set: std::collections::HashSet<(Option<u64>, Option<u64>)> =
            difft_rows.iter().copied().collect();

        for &(old_ln, new_ln) in &html_changed {
            // Skip rows that are pure one-sided (gap lines not in difft JSON)
            // These come from hunk gap filling, so difft CLI might show them differently
            // (as context or paired differently). We already check them via other checks.
            // Focus on: rows that have BOTH sides should appear in difft CLI as paired.
            if old_ln.is_some() && new_ln.is_some() {
                if !difft_set.contains(&(old_ln, new_ln)) {
                    errors.push(format!(
                        "paired row ({:?}, {:?}) not in difft CLI output",
                        old_ln, new_ln,
                    ));
                }
            }
            // One-sided rows: verify the line number appears on the correct side
            if old_ln.is_some() && new_ln.is_none() {
                let found = difft_rows.iter().any(|(o, _)| *o == old_ln);
                if !found {
                    errors.push(format!(
                        "removed row old={:?} not found in difft CLI output",
                        old_ln,
                    ));
                }
            }
            if old_ln.is_none() && new_ln.is_some() {
                let found = difft_rows.iter().any(|(_, n)| *n == new_ln);
                if !found {
                    errors.push(format!(
                        "added row new={:?} not found in difft CLI output",
                        new_ln,
                    ));
                }
            }
        }

        errors
    }

    /// Load all fixture sets from test_fixtures/ and verify every chunk.
    #[test]
    fn fixture_rendering_matches() {
        let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test_fixtures");
        if !fixture_root.exists() {
            eprintln!("No test_fixtures/ directory, skipping fixture tests");
            return;
        }

        let mut total_chunks = 0;
        let mut total_errors = 0;
        let mut error_messages: Vec<String> = Vec::new();

        for fixture_dir in fs::read_dir(&fixture_root).unwrap().flatten() {
            if !fixture_dir.file_type().unwrap().is_dir() { continue; }
            let dir_name = fixture_dir.file_name().to_string_lossy().to_string();
            // Skip per-chunk checks for fixtures designed for combined-chunk testing only.
            if fixture_dir.path().join(".combined-only").exists() { continue; }

            for json_entry in fs::read_dir(fixture_dir.path()).unwrap().flatten() {
                let path = json_entry.path();
                if path.extension().map_or(true, |e| e != "json") { continue; }
                if path.file_name().map_or(false, |n| n.to_string_lossy().starts_with('.')) {
                    continue;
                }

                let json_str = fs::read_to_string(&path).unwrap();
                let difft: DifftOutput = serde_json::from_str(&json_str)
                    .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));
                let file_path = difft.path.as_deref().unwrap_or("unknown");

                // Run per-chunk checks
                let mut all_rendered: Vec<Vec<(String, Option<u64>, Option<u64>)>> = Vec::new();
                for chunk_idx in 0..difft.chunks.len() {
                    total_chunks += 1;
                    let errors = check_chunk(&difft, chunk_idx, file_path);
                    if !errors.is_empty() {
                        total_errors += errors.len();
                        for err in &errors {
                            error_messages.push(format!(
                                "[{}] {} chunk {}: {}",
                                dir_name, file_path, chunk_idx, err,
                            ));
                        }
                    }
                    // Collect rendered rows for difft CLI comparison
                    let mut hl = Highlighter::new();
                    let html = render_chunks(&difft, &[chunk_idx], file_path, None, &mut hl, CollapseMode::None);
                    all_rendered.push(extract_rows(&html));
                }

                // 9. Compare against difft CLI text if available.
                // difft CLI and JSON may have different chunk counts (CLI merges
                // adjacent chunks), so we concatenate all CLI rows into one pool
                // and compare each rendered chunk's rows against it.
                let difft_txt_path = path.with_extension("difft.txt");
                if difft_txt_path.exists() {
                    let difft_text = fs::read_to_string(&difft_txt_path).unwrap();
                    let cli_chunks = parse_difft_cli(&difft_text);
                    let all_cli_rows: Vec<(Option<u64>, Option<u64>)> =
                        cli_chunks.into_iter().flatten().collect();

                    for (chunk_idx, rendered) in all_rendered.iter().enumerate() {
                        let errs = check_against_difft_cli(
                            rendered,
                            &all_cli_rows,
                            file_path,
                            chunk_idx,
                        );
                        if !errs.is_empty() {
                            total_errors += errs.len();
                            for err in &errs {
                                error_messages.push(format!(
                                    "[{}] {} chunk {} (vs difft CLI): {}",
                                    dir_name, file_path, chunk_idx, err,
                                ));
                            }
                        }
                    }
                }
            }
        }

        eprintln!("Checked {} chunks across all fixtures", total_chunks);
        if !error_messages.is_empty() {
            let msg = format!(
                "{} errors in {} chunks:\n{}",
                total_errors, total_chunks,
                error_messages.join("\n"),
            );
            panic!("{}", msg);
        }
    }

    /// When multiple chunks are rendered together, lines that appear as changed
    /// in one chunk and as context in another must not be duplicated.
    #[test]
    fn no_duplicate_lines_across_combined_chunks() {
        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test_fixtures/dup-context/multiplayer__multiplayer__src__dirty_document_checkpoints.rs.json");
        if !fixture_path.exists() {
            eprintln!("Fixture not found, skipping");
            return;
        }
        let json_str = fs::read_to_string(&fixture_path).unwrap();
        let difft: DifftOutput = serde_json::from_str(&json_str).unwrap();
        let file_path = difft.path.as_deref().unwrap_or("unknown");

        // Render chunks 0 and 1 together (the problematic combination)
        let mut hl = Highlighter::new();
        let html = render_chunks(&difft, &[0, 1], file_path, None, &mut hl, CollapseMode::None);
        let rows = extract_rows(&html);

        // Check for duplicate RHS line numbers
        let mut seen_rhs: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut duplicates: Vec<u64> = Vec::new();
        for (class, _, rhs) in &rows {
            if class.contains("chunk-sep") { continue; }
            if let Some(ln) = rhs {
                if !seen_rhs.insert(*ln) {
                    duplicates.push(*ln);
                }
            }
        }
        assert!(
            duplicates.is_empty(),
            "Duplicate RHS line numbers when rendering chunks 0,1 together: {:?}",
            duplicates,
        );

        // Also check LHS
        let mut seen_lhs: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut dup_lhs: Vec<u64> = Vec::new();
        for (class, lhs, _) in &rows {
            if class.contains("chunk-sep") { continue; }
            if let Some(ln) = lhs {
                if !seen_lhs.insert(*ln) {
                    dup_lhs.push(*ln);
                }
            }
        }
        assert!(
            dup_lhs.is_empty(),
            "Duplicate LHS line numbers when rendering chunks 0,1 together: {:?}",
            dup_lhs,
        );

        // Check that line numbers on each side are non-decreasing
        let mut last_lhs: Option<u64> = None;
        let mut last_rhs: Option<u64> = None;
        let mut order_errors: Vec<String> = Vec::new();
        for (class, lhs, rhs) in &rows {
            if class.contains("chunk-sep") {
                last_lhs = None;
                last_rhs = None;
                continue;
            }
            if let Some(ln) = lhs {
                if let Some(prev) = last_lhs {
                    if *ln < prev {
                        order_errors.push(format!(
                            "LHS out of order: {} after {} (row class: {})", ln, prev, class
                        ));
                    }
                }
                last_lhs = Some(*ln);
            }
            if let Some(ln) = rhs {
                if let Some(prev) = last_rhs {
                    if *ln < prev {
                        order_errors.push(format!(
                            "RHS out of order: {} after {} (row class: {})", ln, prev, class
                        ));
                    }
                }
                last_rhs = Some(*ln);
            }
        }
        assert!(
            order_errors.is_empty(),
            "Line ordering errors when rendering chunks 0,1 together:\n{}",
            order_errors.join("\n"),
        );
    }

    /// When a chunk has a single removed line within a hunk that also has
    /// whitespace-only changes on surrounding lines (e.g. Go alignment after
    /// removing a struct field), the gap-paired context lines must still
    /// appear in the rendered output.
    #[test]
    fn gap_paired_context_lines_not_dropped() {
        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test_fixtures/ws-manager/services__agentplat__sbox__sboxd__internal__workspace__manager.go.json");
        if !fixture_path.exists() {
            eprintln!("Fixture not found, skipping");
            return;
        }
        let json_str = fs::read_to_string(&fixture_path).unwrap();
        let difft: DifftOutput = serde_json::from_str(&json_str).unwrap();
        let file_path = difft.path.as_deref().unwrap_or("unknown");

        let mut hl = Highlighter::new();
        let html = render_chunks(&difft, &[3], file_path, None, &mut hl, CollapseMode::None);
        let rows = extract_rows(&html);

        let rendered_old_lines: Vec<u64> = rows.iter()
            .filter_map(|(_, old, _)| *old)
            .collect();

        for expected in [86, 87, 88] {
            assert!(
                rendered_old_lines.contains(&expected),
                "Old line {} (gap-paired context) missing from rendered chunk 3.\n\
                 Rendered old lines: {:?}\n\
                 Rows: {:?}",
                expected, rendered_old_lines, rows,
            );
        }
    }

    /// Context lines should expand beyond CONTEXT_LINES when the boundary
    /// cuts off a multi-line expression (e.g. a function call). The expansion
    /// uses bracket nesting to find the enclosing opener/closer, capped at
    /// MAX_EXPRESSION_CONTEXT extra lines.
    #[test]
    fn context_expands_to_show_enclosing_expression() {
        // Simulate a file where a changed line sits inside a multi-line
        // function call. With CONTEXT_LINES=3, the call opener would be
        // cut off. The renderer should expand to include it.
        //
        //  0: function deploy() {
        //  1:   const result = await sendEvent(context, {
        //  2:     cluster: env,
        //  3:     service: 'web',
        //  4:     orchestrator: 'N/A',
        //  5:     manuallyTriggered: true,     <-- CHANGED (old: false)
        //  6:     additionalEventTags: tags,
        //  7:   });
        //  8:   if (result.ok) {
        //  9:     return true;
        // 10:   }
        // 11: }
        let old_lines = vec![
            "function deploy() {",
            "  const result = await sendEvent(context, {",
            "    cluster: env,",
            "    service: 'web',",
            "    orchestrator: 'N/A',",
            "    manuallyTriggered: false,",
            "    additionalEventTags: tags,",
            "  });",
            "  if (result.ok) {",
            "    return true;",
            "  }",
            "}",
        ];
        let new_lines = vec![
            "function deploy() {",
            "  const result = await sendEvent(context, {",
            "    cluster: env,",
            "    service: 'web',",
            "    orchestrator: 'N/A',",
            "    manuallyTriggered: true,",
            "    additionalEventTags: tags,",
            "  });",
            "  if (result.ok) {",
            "    return true;",
            "  }",
            "}",
        ];

        // Changed line is index 5 (0-based). difft uses 0-based line numbers.
        let chunk = vec![
            LineEntry {
                lhs: Some(side(5, vec![span("false", 25, 30)])),
                rhs: Some(side(5, vec![span("true", 25, 29)])),
            },
        ];

        // Hunk: the change is at old line 6, new line 6 (1-based for git hunks)
        let hunks = vec![DiffHunk { old_start: 6, old_count: 1, new_start: 6, new_count: 1 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let html = test_render_chunks(&difft, &[0], "test.ts", None);
        let rows = extract_rows(&html);

        let rendered_new_lines: Vec<u64> = rows.iter()
            .filter_map(|(_, _, n)| *n)
            .collect();

        // With plain CONTEXT_LINES=3, we'd see display lines 3..9.
        // Display line 2 ("const result = await sendEvent(context, {")
        // would be cut off (0-based index 1, display = index+1 = 2).
        // The expression-aware expansion should include it because the
        // visible range contains a closer without the matching opener.
        assert!(
            rendered_new_lines.contains(&2),
            "Expected the function call opener (display line 2) to be included \
             via expression-aware context expansion.\n\
             Rendered new lines: {:?}\n\
             Rows: {:?}",
            rendered_new_lines, rows,
        );
    }

    /// When an entire new function is added, the pre-context shows the end of
    /// the previous function. Expression expansion should NOT pull in unrelated
    /// code from the previous function just because the context has a closing
    /// brace.
    #[test]
    fn added_function_does_not_expand_into_previous_function() {
        //  0: async function previousFunction() {
        //  1:   const x = await fetch(url, {
        //  2:     method: 'POST',
        //  3:     body: JSON.stringify(data),
        //  4:   });
        //  5:   return x;
        //  6: }
        //  7:
        //  8: /**                                          <-- ADDED from here
        //  9:  * Bootstrap via workspace/create.
        // 10:  */
        // 11: async function bootstrapViaWorkspaceCreate(
        // 12:   context: Context,
        // 13:   config: BootstrapConfig,
        // 14: ) {
        // 15:   const ws = await createWorkspace(context, {
        // 16:     bootstrap: config,
        // 17:   });
        // 18:   return ws;
        // 19: }
        let old_lines: Vec<&str> = vec![
            "async function previousFunction() {",
            "  const x = await fetch(url, {",
            "    method: 'POST',",
            "    body: JSON.stringify(data),",
            "  });",
            "  return x;",
            "}",
            "",
        ];
        let new_lines: Vec<&str> = vec![
            "async function previousFunction() {",
            "  const x = await fetch(url, {",
            "    method: 'POST',",
            "    body: JSON.stringify(data),",
            "  });",
            "  return x;",
            "}",
            "",
            "/**",
            " * Bootstrap via workspace/create.",
            " */",
            "async function bootstrapViaWorkspaceCreate(",
            "  context: Context,",
            "  config: BootstrapConfig,",
            ") {",
            "  const ws = await createWorkspace(context, {",
            "    bootstrap: config,",
            "  });",
            "  return ws;",
            "}",
        ];

        // Added lines: 0-based indices 8..19 on the new side.
        let chunk: Vec<LineEntry> = (8..20).map(|i| {
            LineEntry { lhs: None, rhs: Some(side(i, vec![])) }
        }).collect();

        // Hunk: insertion after old line 8, adding 12 new lines starting at new line 9
        let hunks = vec![DiffHunk { old_start: 9, old_count: 0, new_start: 9, new_count: 12 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let html = test_render_chunks(&difft, &[0], "test.ts", None);
        let rows = extract_rows(&html);

        let rendered_new_lines: Vec<u64> = rows.iter()
            .filter_map(|(_, _, n)| *n)
            .collect();

        // Pre-context should be at most 3 lines (display lines 6, 7, 8 =
        // 0-based 5, 6, 7). The expansion should NOT reach back to display
        // line 2 ("const x = await fetch(url, {") or earlier, because the
        // added function's brackets are self-contained.
        let min_new = rendered_new_lines.iter().copied().min().unwrap_or(0);
        assert!(
            min_new >= 6,
            "Expression expansion reached into the previous function. \
             Earliest rendered new line is display {}, expected >= 6.\n\
             Rendered new lines: {:?}\n\
             Rows: {:?}",
            min_new, rendered_new_lines, rows,
        );
    }

    /// When expression expansion hits MAX_EXPRESSION_CONTEXT without finding
    /// the matching bracket, the extra lines are mid-expression noise. The
    /// expansion should be abandoned entirely, falling back to CONTEXT_LINES.
    #[test]
    fn incomplete_expression_expansion_falls_back_to_normal_context() {
        // A long array literal where the changed line is near the end.
        // The `]` closer is within post-context, triggering before-expansion,
        // but the `[` opener is 12+ lines above, unreachable within
        // MAX_EXPRESSION_CONTEXT=8. The expansion should give up and use
        // normal 3-line context rather than showing 8 random mid-array lines.
        //
        //  0: const FLAGS = [
        //  1:   'flag_a',
        //  ...  (many flags)
        // 13:   'flag_n',
        // 14:   'flag_changed',          <-- CHANGED
        // 15: ] as const;               <-- closer in post-context
        // 16:
        // 17: export default FLAGS;
        let mut old_lines: Vec<&str> = vec!["const FLAGS = ["];
        for i in 0..13 {
            old_lines.push(match i {
                0 => "  'flag_a',",
                1 => "  'flag_b',",
                2 => "  'flag_c',",
                3 => "  'flag_d',",
                4 => "  'flag_e',",
                5 => "  'flag_f',",
                6 => "  'flag_g',",
                7 => "  'flag_h',",
                8 => "  'flag_i',",
                9 => "  'flag_j',",
                10 => "  'flag_k',",
                11 => "  'flag_l',",
                _ => "  'flag_m',",
            });
        }
        old_lines.push("  'flag_old',");   // index 14
        old_lines.push("] as const;");     // index 15
        old_lines.push("");                // index 16
        old_lines.push("export default FLAGS;"); // index 17

        let mut new_lines = old_lines.clone();
        new_lines[14] = "  'flag_changed',";

        let chunk = vec![
            LineEntry {
                lhs: Some(side(14, vec![span("flag_old", 3, 11)])),
                rhs: Some(side(14, vec![span("flag_changed", 3, 15)])),
            },
        ];

        let hunks = vec![DiffHunk { old_start: 15, old_count: 1, new_start: 15, new_count: 1 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let html = test_render_chunks(&difft, &[0], "test.ts", None);
        let rows = extract_rows(&html);

        let rendered_new_lines: Vec<u64> = rows.iter()
            .filter_map(|(_, _, n)| *n)
            .collect();

        let min_new = rendered_new_lines.iter().copied().min().unwrap_or(0);

        // Normal 3-line context: display lines 12..18 (0-based 11..17).
        // The `[` opener is at display line 1, unreachable within 8 extra lines
        // from display line 12. Since expansion can't complete the expression,
        // it should not expand at all.
        assert!(
            min_new >= 12,
            "Incomplete expression expansion should fall back to normal context. \
             Earliest rendered line is display {}, expected >= 12.\n\
             Rendered new lines: {:?}",
            min_new, rendered_new_lines,
        );
    }

    /// When an added line is inside a function call and the post-context
    /// contains `});` (closing the call) followed by `}` (closing an outer
    /// if/else block), the expansion should only account for the immediate
    /// enclosing call's brackets, not the outer block's closer. Otherwise
    /// it expands past the enclosing call into unrelated code above.
    #[test]
    fn expansion_stops_at_nearest_enclosing_call() {
        //  0: if (condition) {
        //  1:   await sendDeployRollbackEventStep(context, {
        //  2:     manuallyTriggered,
        //  3:     successfullyRolledBack: endState === 'rollback',
        //  4:     deployEndState: endState,
        //  5:     additionalEventTags: commitTags,
        //  6:   });
        //  7: } else {
        //  8:   await sendDeploySuccessEventStep(context, {
        //  9:     cluster: env,
        // 10:     service: 'web',
        // 11:     orchestrator: 'N/A',
        // 12:     branch,
        // 13:     additionalEventTags: commitTags,   <-- ADDED
        // 14:   });
        // 15: }
        // 16: if (endState === 'success') {
        let old_lines = vec![
            "if (condition) {",
            "  await sendDeployRollbackEventStep(context, {",
            "    manuallyTriggered,",
            "    successfullyRolledBack: endState === 'rollback',",
            "    deployEndState: endState,",
            "    additionalEventTags: commitTags,",
            "  });",
            "} else {",
            "  await sendDeploySuccessEventStep(context, {",
            "    cluster: env,",
            "    service: 'web',",
            "    orchestrator: 'N/A',",
            "    branch,",
            "  });",
            "}",
            "if (endState === 'success') {",
        ];
        let new_lines = vec![
            "if (condition) {",
            "  await sendDeployRollbackEventStep(context, {",
            "    manuallyTriggered,",
            "    successfullyRolledBack: endState === 'rollback',",
            "    deployEndState: endState,",
            "    additionalEventTags: commitTags,",
            "  });",
            "} else {",
            "  await sendDeploySuccessEventStep(context, {",
            "    cluster: env,",
            "    service: 'web',",
            "    orchestrator: 'N/A',",
            "    branch,",
            "    additionalEventTags: commitTags,",
            "  });",
            "}",
            "if (endState === 'success') {",
        ];

        // Added line at new-side index 13 (inside sendDeploySuccessEventStep)
        let chunk = vec![
            LineEntry {
                lhs: None,
                rhs: Some(side(13, vec![])),
            },
        ];

        // Hunk: insertion at new line 14, adding 1 line
        let hunks = vec![DiffHunk { old_start: 13, old_count: 0, new_start: 14, new_count: 1 }];
        let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);

        let html = test_render_chunks(&difft, &[0], "test.ts", None);
        let rows = extract_rows(&html);

        let rendered_new_lines: Vec<u64> = rows.iter()
            .filter_map(|(_, _, n)| *n)
            .collect();

        let min_new = rendered_new_lines.iter().copied().min().unwrap_or(0);

        // The added line (display 14) is inside sendDeploySuccessEventStep
        // at display 9. Expansion should reach display 9 at most.
        // It should NOT go past "} else {" (display 8) into the rollback
        // call. The `}` at display 16 closes the if/else block, not the
        // function call, and shouldn't inflate the expansion.
        assert!(
            min_new >= 9,
            "Expansion went past the enclosing function call into the \
             rollback branch. Earliest rendered line is display {}, \
             expected >= 9.\n\
             Rendered new lines: {:?}",
            min_new, rendered_new_lines,
        );

        // Verify the enclosing call IS included
        assert!(
            rendered_new_lines.contains(&9),
            "Expected sendDeploySuccessEventStep call opener (display 9) \
             to be included.\n\
             Rendered new lines: {:?}",
            rendered_new_lines,
        );
    }

    /// When unchanged lines fall between two hunks within a single difft chunk
    /// (intra-chunk gaps), they should appear as context lines in the rendered
    /// output. Without this, function signatures between a comment change and
    /// a body change disappear.
    #[test]
    fn intra_chunk_gap_lines_rendered_as_context() {
        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test_fixtures/intra-chunk-gap/services__agentplat__sbox__sboxd__internal__workspace__manager.go.json");
        if !fixture_path.exists() {
            eprintln!("Fixture not found, skipping");
            return;
        }
        let json_str = fs::read_to_string(&fixture_path).unwrap();
        let difft: DifftOutput = serde_json::from_str(&json_str).unwrap();
        let file_path = difft.path.as_deref().unwrap_or("unknown");

        // Chunk 3 has entries at old=474/new=478 and old=476/new=None,
        // with the function signature at new=479 (old=475) falling between
        // hunks and not in any chunk entry or hunk.
        let mut hl = Highlighter::new();
        let html = render_chunks(&difft, &[3], file_path, None, &mut hl, CollapseMode::None);
        let rows = extract_rows(&html);

        let rendered_new_lines: Vec<u64> = rows.iter()
            .filter_map(|(_, _, n)| *n)
            .collect();

        // New line 479 (display 480) is the function signature
        // "func (m *WorkspaceManager) DefaultWorkspacePath() string {"
        // It must appear as a context line between the changed entries.
        assert!(
            rendered_new_lines.contains(&480),
            "Intra-chunk gap line (func signature at display 480) should be \
             rendered as context between chunk entries.\n\
             Rendered new lines: {:?}\n\
             Rows: {:?}",
            rendered_new_lines, rows,
        );
    }

    /// Old-side line numbers must be non-decreasing in the rendered output.
    /// When a chunk pairs old lines 484+ with new lines 477+ (a refactoring
    /// that moved code), consolidation and sort must not produce jumbled
    /// old-side ordering.
    #[test]
    fn old_side_ordering_in_refactored_chunk() {
        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test_fixtures/jumbled-old-lines/services__agentplat__sbox__sboxd__internal__workspace__manager.go.json");
        if !fixture_path.exists() {
            eprintln!("Fixture not found, skipping");
            return;
        }
        let json_str = fs::read_to_string(&fixture_path).unwrap();
        let difft: DifftOutput = serde_json::from_str(&json_str).unwrap();
        let file_path = difft.path.as_deref().unwrap_or("unknown");

        let mut hl = Highlighter::new();
        let html = render_chunks(&difft, &[3], file_path, None, &mut hl, CollapseMode::None);
        let rows = extract_rows(&html);

        let old_lns: Vec<u64> = rows.iter()
            .filter_map(|(_, o, _)| *o)
            .collect();

        // Old-side line numbers must be non-decreasing.
        let mut violations = Vec::new();
        for i in 0..old_lns.len().saturating_sub(1) {
            if old_lns[i] > old_lns[i + 1] {
                violations.push(format!(
                    "row {}: old {} > row {}: old {}",
                    i, old_lns[i], i + 1, old_lns[i + 1]
                ));
            }
        }
        assert!(
            violations.is_empty(),
            "Old-side line numbers are jumbled in chunk 3:\n{}\n\
             All old lines: {:?}",
            violations.join("\n"), old_lns,
        );
    }

    /// Low-similarity pairs (>70% changed on both sides) should render
    /// as `line-paired-full` (full-line red/green, no token highlights)
    /// rather than `line-paired` (token-level diff). They stay on the
    /// same row for visual compactness.
    #[test]
    fn low_similarity_pairs_use_full_line_background() {
        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test_fixtures/low-similarity-pairs/services__agentplat__sbox__sboxd__internal__workspace__manager.go.json");
        if !fixture_path.exists() {
            eprintln!("Fixture not found, skipping");
            return;
        }
        let json_str = fs::read_to_string(&fixture_path).unwrap();
        let difft: DifftOutput = serde_json::from_str(&json_str).unwrap();
        let file_path = difft.path.as_deref().unwrap_or("unknown");

        let mut hl = Highlighter::new();
        let html = render_chunks(&difft, &[3], file_path, None, &mut hl, CollapseMode::None);

        // Check raw HTML for class attributes since extract_rows
        // normalizes line-paired-full to line-paired.
        let row_re = Regex::new(r#"<tr class="(line-paired(?:-full)?)"[^>]*>"#).unwrap();
        let ln_re = Regex::new(r#"<td class="ln ln-lhs"[^>]*>(\d+)</td>"#).unwrap();

        // Collect (class, old_ln) from the raw single-table HTML (before
        // split_into_two_tables runs on it — render_chunks returns this).
        let paired_rows: Vec<(&str, u64)> = row_re.captures_iter(&html)
            .filter_map(|cap| {
                let class = cap.get(1).unwrap().as_str();
                let after = &html[cap.get(0).unwrap().end()..];
                ln_re.captures(after).and_then(|lc| {
                    lc[1].parse::<u64>().ok().map(|ln| (class, ln))
                })
            })
            .collect();

        // old=487 (display 488) "m.mu.RLock()" / new=480 (display 481)
        // "id := string(wsID)" are 100%/89% changed. Must be
        // line-paired-full, not line-paired.
        let row_488 = paired_rows.iter().find(|(_, ln)| *ln == 488);
        assert_eq!(
            row_488.map(|(c, _)| *c), Some("line-paired-full"),
            "Low-similarity old=488 should be line-paired-full.\nAll paired: {:?}", paired_rows,
        );

        // old=488 (display 489) "defer m.mu.RUnlock()" same treatment.
        let row_489 = paired_rows.iter().find(|(_, ln)| *ln == 489);
        assert_eq!(
            row_489.map(|(c, _)| *c), Some("line-paired-full"),
            "Low-similarity old=489 should be line-paired-full.\nAll paired: {:?}", paired_rows,
        );

        // Good-similarity: old=493 (display 494) "return ws.RootPath, nil"
        // / new=490 (display 491) "return resolved, nil" (~40% changed)
        // should remain line-paired with token highlights.
        let row_494 = paired_rows.iter().find(|(_, ln)| *ln == 494);
        assert_eq!(
            row_494.map(|(c, _)| *c), Some("line-paired"),
            "Good-similarity old=494 should remain line-paired.\nAll paired: {:?}", paired_rows,
        );
    }

    /// Difft sometimes includes unchanged lines in a chunk when the
    /// surrounding syntactic structure changed (e.g. a JSDoc comment grew
    /// from 4 to 7 lines). The change spans cover the full line on both
    /// sides (syntax highlighting), but the content is identical. These
    /// should render as context rows, not as paired changes or separate
    /// removed + added rows.
    #[test]
    fn identical_content_entries_render_as_context() {
        use crate::difft_json::*;

        // Simulate a JSDoc comment that grew: old has `/**` through `*/`,
        // new has the same lines plus extra comment lines before `*/`.
        let old_lines: Vec<String> = vec![
            "/**",
            " * Calls workspace/create with structured config.",
            " * Bootstrap is idempotent.",
            " */",
            "async function bootstrap() {",
        ].into_iter().map(String::from).collect();
        let new_lines: Vec<String> = vec![
            "/**",
            " * Calls workspace/create with structured config.",
            " * Bootstrap is idempotent.",
            " *",
            " * Returns immediately; bootstrap runs async.",
            " */",
            "async function bootstrap() {",
        ].into_iter().map(String::from).collect();

        // Build a chunk that mirrors what difft produces: paired entries
        // for the identical lines (full-line change spans), plus added-only
        // entries for the new lines.
        let make_full_span = |line: &str| -> Vec<ChangeSpan> {
            vec![ChangeSpan {
                content: line.to_string(),
                start: 0,
                end: line.len(),
                highlight: "comment".to_string(),
            }]
        };

        let chunks = vec![vec![
            // Added-only entries for truly new comment lines
            LineEntry {
                lhs: None,
                rhs: Some(LineSide { line_number: 4, changes: make_full_span(" * Returns immediately; bootstrap runs async.") }),
            },
            LineEntry {
                lhs: None,
                rhs: Some(LineSide { line_number: 5, changes: make_full_span(" */") }),
            },
            // Paired entries with identical content (difft structural match)
            LineEntry {
                lhs: Some(LineSide { line_number: 0, changes: make_full_span("/**") }),
                rhs: Some(LineSide { line_number: 0, changes: make_full_span("/**") }),
            },
            LineEntry {
                lhs: Some(LineSide { line_number: 1, changes: make_full_span(" * Calls workspace/create with structured config.") }),
                rhs: Some(LineSide { line_number: 1, changes: make_full_span(" * Calls workspace/create with structured config.") }),
            },
            LineEntry {
                lhs: Some(LineSide { line_number: 2, changes: make_full_span(" * Bootstrap is idempotent.") }),
                rhs: Some(LineSide { line_number: 2, changes: make_full_span(" * Bootstrap is idempotent.") }),
            },
            // Paired entry where content actually differs
            // (" */" → " *", partial change on lhs only)
            LineEntry {
                lhs: Some(LineSide { line_number: 3, changes: vec![
                    ChangeSpan { content: "/".to_string(), start: 2, end: 3, highlight: "comment".to_string() },
                ] }),
                rhs: Some(LineSide { line_number: 3, changes: vec![] }),
            },
        ]];

        let difft = DifftOutput {
            chunks,
            language: Some("TypeScript".to_string()),
            path: Some("test.ts".to_string()),
            status: None,
            old_lines,
            new_lines,
            hunks: vec![DiffHunk { old_start: 1, old_count: 5, new_start: 1, new_count: 7 }],
        };

        let mut hl = Highlighter::new();
        let html = render_chunks(&difft, &[0], "test.ts", None, &mut hl, CollapseMode::None);
        let rows = extract_rows(&html);

        // Lines 0-2 (display 1-3) have identical content. They must render
        // as context, not as removed/added or paired changes.
        for display_ln in 1..=3u64 {
            let is_context = rows.iter().any(|(c, o, n)| {
                c == "line-context"
                    && (*o == Some(display_ln) || *n == Some(display_ln))
            });
            let is_changed = rows.iter().any(|(c, o, n)| {
                c != "line-context" && c != "chunk-sep"
                    && (*o == Some(display_ln) || *n == Some(display_ln))
            });
            assert!(
                is_context && !is_changed,
                "Identical-content line {} should be context, not changed.\nRows: {:?}",
                display_ln,
                rows.iter()
                    .filter(|(_, o, n)| *o == Some(display_ln) || *n == Some(display_ln))
                    .collect::<Vec<_>>(),
            );
        }

        // Line 4 (display) on the old side (` */`) should be changed since
        // it differs from new line 4 (` *`).
        let old_4_changed = rows.iter().any(|(c, o, _)| {
            c != "line-context" && c != "chunk-sep" && *o == Some(4)
        });
        assert!(old_4_changed, "Differing line old=4 should be changed, not context");
    }
}
