use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use pulldown_cmark::{Options, Parser};
use regex::Regex;

use crate::difft_json::{ChangeSpan, DifftOutput, LineEntry, LineSide};

/// Number of unchanged context lines to show before/after each chunk.
const CONTEXT_LINES: usize = 3;

const CSS: &str = r#"
:root {
    --bg: #ffffff;
    --bg-secondary: #f6f7f8;
    --border: #d0d4d9;
    --text: #1a1e24;
    --text-secondary: #474d57;
    --text-muted: #7a8290;
    --primary: #0550ae;
    --diff-header-bg: #f1f8ff;
    --diff-header-fg: #0969da;
    --ln-fg: #8b949e;
    --added-bg: #e6ffec;
    --added-hl: #aceebb;
    --removed-bg: #ffebe9;
    --removed-hl: #ffcecb;
    --sep-bg: #f6f7f8;
    --empty-bg: #f6f7f8;
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
    padding: 24px 28px 60px;
    line-height: 1.6;
}

.diff-block {
    width: 85vw;
    max-width: 1200px;
    margin-left: 50%;
    transform: translateX(-50%);
    margin-top: 1rem;
    margin-bottom: 1rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    max-height: 80vh;
    overflow: hidden;
}

h1 {
    font-size: 1.75em;
    font-weight: 700;
    margin: 0.5em 0 0.4em;
    padding-bottom: 0.2em;
    border-bottom: 1px solid var(--border);
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
    margin: 0 0 0.75em;
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
    top: 24px;
    left: 16px;
    width: 200px;
    max-height: calc(100vh - 48px);
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
}

.diff-table {
    width: 100%;
    border-collapse: collapse;
    font-family: "SFMono-Regular", Consolas, "Liberation Mono", Menlo, monospace;
    font-size: 0.8rem;
    line-height: 1.5;
    table-layout: fixed;
}

col.ln-col { width: 3.5em; }
col.code-col { width: calc(50% - 3.5em); }

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
    border-right: 1px solid var(--border);
    padding-right: 0.4rem;
    min-width: 3em;
}

.diff-table td.code-lhs {
    border-right: 1px solid var(--border);
}

/* Context lines (unchanged) */
tr.line-context td { background: var(--bg); }

/* Removed lines */
tr.line-removed td.code-lhs { background: var(--removed-bg); }
tr.line-removed td.ln:first-child { background: var(--removed-bg); }
tr.line-removed td.code-rhs { background: var(--empty-bg); }
tr.line-removed td.ln:nth-child(3) { background: var(--empty-bg); }

/* Added lines */
tr.line-added td.code-rhs { background: var(--added-bg); }
tr.line-added td.ln:nth-child(3) { background: var(--added-bg); }
tr.line-added td.code-lhs { background: var(--empty-bg); }
tr.line-added td.ln:first-child { background: var(--empty-bg); }

/* Paired rows: removed on left, added on right */
tr.line-paired td.code-lhs { background: var(--removed-bg); }
tr.line-paired td.ln:first-child { background: var(--removed-bg); }
tr.line-paired td.code-rhs { background: var(--added-bg); }
tr.line-paired td.ln:nth-child(3) { background: var(--added-bg); }

.chunk-sep td {
    height: 0.5rem;
    background: var(--sep-bg);
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
}

/* Token-level highlights within changed lines */
.hl-del { background: var(--removed-hl); border-radius: 2px; }
.hl-add { background: var(--added-hl); border-radius: 2px; }
"#;

const JS: &str = r#"
(function() {
    var blocks = document.querySelectorAll('.diff-block');
    var pinnedY = null;
    var activeBlock = null;

    window.addEventListener('wheel', function(e) {
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
            var rect = block.getBoundingClientRect();

            var maxScroll = block.scrollHeight - block.clientHeight;
            if (maxScroll <= 0) continue;
            if (rect.bottom < 0 || rect.top > window.innerHeight) continue;

            var atEnd = block.scrollTop >= maxScroll - 1;
            var atStart = block.scrollTop <= 0;

            if (e.deltaY > 0 && rect.top <= 80 && !atEnd) {
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

    // Also pin on scroll events caused by momentum
    window.addEventListener('scroll', function() {
        if (pinnedY !== null) {
            window.scrollTo(0, pinnedY);
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
        a.href = '#' + h.id;
        a.textContent = h.textContent;
        a.className = 'toc-' + h.tagName.toLowerCase();
        nav.appendChild(a);
    }
    document.body.appendChild(nav);

    var links = nav.querySelectorAll('a');
    var ticking = false;

    function updateActive() {
        var current = null;
        for (var i = 0; i < headings.length; i++) {
            if (headings[i].getBoundingClientRect().top <= 60) {
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
"#;

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Merge adjacent change spans that are only separated by whitespace.
/// Returns a list of (start, end) byte ranges.
fn merge_whitespace_spans(spans: &[ChangeSpan], full_line: &str) -> Vec<(usize, usize)> {
    if spans.is_empty() {
        return Vec::new();
    }
    let line_len = full_line.len();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    let mut cur_start = spans[0].start.min(line_len);
    let mut cur_end = spans[0].end.min(line_len);

    for span in &spans[1..] {
        let next_start = span.start.min(line_len);
        let next_end = span.end.min(line_len);
        let gap = full_line.get(cur_end..next_start).unwrap_or("");
        if gap.is_empty() || gap.bytes().all(|b| b == b' ' || b == b'\t') {
            cur_end = next_end;
        } else {
            merged.push((cur_start, cur_end));
            cur_start = next_start;
            cur_end = next_end;
        }
    }
    merged.push((cur_start, cur_end));
    merged
}

/// Render a full source line, highlighting changed spans with the given CSS class.
/// Merges adjacent spans separated only by whitespace into single highlight regions.
fn render_full_line(full_line: &str, spans: &[ChangeSpan], hl_class: &str) -> String {
    if spans.is_empty() {
        return html_escape(full_line);
    }

    let line_len = full_line.len();
    let merged = merge_whitespace_spans(spans, full_line);

    let mut html = String::new();
    let mut pos: usize = 0;

    for &(start, end) in &merged {
        if start > pos {
            if let Some(text) = full_line.get(pos..start) {
                html.push_str(&html_escape(text));
            }
        }
        if let Some(text) = full_line.get(start..end) {
            html.push_str(&format!(
                "<span class=\"{}\">{}</span>",
                hl_class,
                html_escape(text)
            ));
        }
        pos = end;
    }

    if pos < line_len {
        if let Some(text) = full_line.get(pos..) {
            html.push_str(&html_escape(text));
        }
    }

    html
}

/// Render a side using only the span data (fallback when full lines unavailable).
fn render_spans_only(side: &LineSide) -> String {
    let mut html = String::new();
    let mut pos = 0;
    let mut is_leading = true;

    for span in &side.changes {
        if span.start > pos {
            let gap = span.start - pos;
            if is_leading {
                html.push_str(&" ".repeat(gap));
            } else {
                html.push_str(&" ".repeat(gap.min(4)));
            }
        }
        is_leading = false;
        html.push_str(&html_escape(&span.content));
        pos = span.end;
    }
    html
}

/// Render one side of a diff line with the given highlight class for changed spans.
fn render_side(side: &LineSide, full_lines: &[String], hl_class: &str) -> String {
    let line_idx = side.line_number as usize; // 0-based
    if let Some(full_line) = full_lines.get(line_idx) {
        render_full_line(full_line, &side.changes, hl_class)
    } else {
        render_spans_only(side)
    }
}

/// Render a full line with only the region [change_start..change_end] highlighted.
fn render_refined_line(full_line: &str, change_start: usize, change_end: usize, hl_class: &str) -> String {
    let cs = change_start.min(full_line.len());
    let ce = change_end.min(full_line.len());

    let mut html = String::new();
    if cs > 0 {
        html.push_str(&html_escape(&full_line[..cs]));
    }
    if ce > cs {
        html.push_str(&format!(
            "<span class=\"{}\">{}</span>",
            hl_class,
            html_escape(&full_line[cs..ce])
        ));
    }
    if ce < full_line.len() {
        html.push_str(&html_escape(&full_line[ce..]));
    }
    html
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
            let rhs_start = i;
            while i < entries.len() && entries[i].rhs.is_some() && entries[i].lhs.is_none() {
                i += 1;
            }
            let rhs_run = &entries[rhs_start..i];

            let lhs_start = i;
            while i < entries.len() && entries[i].lhs.is_some() && entries[i].rhs.is_none() {
                i += 1;
            }
            let lhs_run = &entries[lhs_start..i];

            let max_len = lhs_run.len().max(rhs_run.len());
            for j in 0..max_len {
                rows.push(DiffRow {
                    lhs: lhs_run.get(j).and_then(|e| e.lhs.as_ref()),
                    rhs: rhs_run.get(j).and_then(|e| e.rhs.as_ref()),
                });
            }
        } else {
            i += 1;
        }
    }

    rows
}

fn render_diff_row(row: &DiffRow, old_lines: &[String], new_lines: &[String]) -> String {
    let mut html = String::new();
    let has_lhs = row.lhs.is_some();
    let has_rhs = row.rhs.is_some();

    let row_class = match (has_lhs, has_rhs) {
        (true, true) => "line-paired",
        (true, false) => "line-removed",
        (false, true) => "line-added",
        (false, false) => return String::new(),
    };

    html.push_str(&format!("<tr class=\"{}\">", row_class));

    // For paired rows, use refined character-level diff instead of difft's structural spans.
    // This gives GitHub-style highlights where only the actual differing characters are marked.
    if let (Some(lhs), Some(rhs)) = (row.lhs, row.rhs) {
        let old_line = old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let new_line = new_lines.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let (old_cs, old_ce, new_cs, new_ce) = find_change_bounds(old_line, new_line);

        html.push_str(&format!(
            "<td class=\"ln\">{}</td><td class=\"code-lhs\">{}</td>",
            lhs.line_number + 1,
            render_refined_line(old_line, old_cs, old_ce, "hl-del")
        ));
        html.push_str(&format!(
            "<td class=\"ln\">{}</td><td class=\"code-rhs\">{}</td>",
            rhs.line_number + 1,
            render_refined_line(new_line, new_cs, new_ce, "hl-add")
        ));
    } else if let Some(lhs) = row.lhs {
        html.push_str(&format!(
            "<td class=\"ln\">{}</td><td class=\"code-lhs\">{}</td>",
            lhs.line_number + 1,
            render_side(lhs, old_lines, "hl-del")
        ));
        html.push_str("<td class=\"ln\"></td><td class=\"code-rhs\"></td>");
    } else if let Some(rhs) = row.rhs {
        html.push_str("<td class=\"ln\"></td><td class=\"code-lhs\"></td>");
        html.push_str(&format!(
            "<td class=\"ln\">{}</td><td class=\"code-rhs\">{}</td>",
            rhs.line_number + 1,
            render_side(rhs, new_lines, "hl-add")
        ));
    }

    html.push_str("</tr>");
    html
}

/// Render a context (unchanged) line showing both old and new sides.
fn render_context_row(old_idx: usize, new_idx: usize, old_lines: &[String], new_lines: &[String]) -> String {
    let old_content = old_lines.get(old_idx).map(|s| html_escape(s)).unwrap_or_default();
    let new_content = new_lines.get(new_idx).map(|s| html_escape(s)).unwrap_or_default();
    format!(
        "<tr class=\"line-context\"><td class=\"ln\">{}</td><td class=\"code-lhs\">{}</td>\
         <td class=\"ln\">{}</td><td class=\"code-rhs\">{}</td></tr>",
        old_idx + 1, old_content, new_idx + 1, new_content
    )
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

/// Produce a unified-diff-style text representation of selected chunks.
/// Uses the same chunk processing logic as HTML rendering (context lines, hunk gap
/// filling, consolidation) but outputs plain text with ` `/`-`/`+` prefixes.
fn render_chunks_text(difft: &DifftOutput, chunk_indices: &[usize]) -> String {
    let mut out = String::new();
    let hunks = &difft.hunks;

    let mut last_old_rendered: Option<usize> = None;
    let mut last_new_rendered: Option<usize> = None;

    let mut first_chunk = true;
    for &idx in chunk_indices {
        let Some(chunk) = difft.chunks.get(idx) else { continue };

        let (lhs_range, rhs_range) = chunk_line_range(chunk);

        let (old_first, old_last) = match lhs_range {
            Some((min, max)) => (min as usize, max as usize),
            None => {
                let (rmin, rmax) = rhs_range.unwrap_or((0, 0));
                let o_min = to_0based(new_to_old_line(rmin + 1, hunks));
                let o_max = to_0based(new_to_old_line(rmax + 1, hunks));
                (o_min, o_max)
            }
        };
        let (new_first, new_last) = match rhs_range {
            Some((min, max)) => (min as usize, max as usize),
            None => {
                let (lmin, lmax) = lhs_range.unwrap_or((0, 0));
                let n_min = to_0based(old_to_new_line(lmin + 1, hunks));
                let n_max = to_0based(old_to_new_line(lmax + 1, hunks));
                (n_min, n_max)
            }
        };

        let mut difft_old_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut difft_new_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for entry in chunk {
            if let Some(lhs) = &entry.lhs { difft_old_lines.insert(lhs.line_number as usize); }
            if let Some(rhs) = &entry.rhs { difft_new_lines.insert(rhs.line_number as usize); }
        }

        let mut extra_removed: Vec<usize> = Vec::new();
        let mut extra_added: Vec<usize> = Vec::new();
        let mut extra_paired: Vec<(usize, usize)> = Vec::new();

        for h in hunks {
            let h_old_start_0 = (h.old_start as usize).saturating_sub(1);
            let h_old_end_0 = h_old_start_0 + h.old_count as usize;
            let h_new_start_0 = (h.new_start as usize).saturating_sub(1);
            let h_new_end_0 = h_new_start_0 + h.new_count as usize;

            let overlaps_old = h_old_end_0 > old_first.saturating_sub(1) && h_old_start_0 <= old_last + 1;
            let overlaps_new = h_new_end_0 > new_first.saturating_sub(1) && h_new_start_0 <= new_last + 1;

            if overlaps_old || overlaps_new {
                let mut hunk_removed: Vec<usize> = Vec::new();
                let mut hunk_added: Vec<usize> = Vec::new();
                for line_0 in h_old_start_0..h_old_end_0 {
                    if !difft_old_lines.contains(&line_0) { hunk_removed.push(line_0); }
                }
                for line_0 in h_new_start_0..h_new_end_0 {
                    if !difft_new_lines.contains(&line_0) { hunk_added.push(line_0); }
                }
                let pair_count = hunk_removed.len().min(hunk_added.len());
                for i in 0..pair_count {
                    extra_paired.push((hunk_removed[i], hunk_added[i]));
                }
                for &r in &hunk_removed[pair_count..] { extra_removed.push(r); }
                for &a in &hunk_added[pair_count..] { extra_added.push(a); }
            }
        }

        let mut old_first = old_first;
        let mut old_last = old_last;
        let mut new_first = new_first;
        let mut new_last = new_last;
        for &old_0 in &extra_removed { old_first = old_first.min(old_0); old_last = old_last.max(old_0); }
        for &new_0 in &extra_added { new_first = new_first.min(new_0); new_last = new_last.max(new_0); }
        for &(old_0, new_0) in &extra_paired {
            old_first = old_first.min(old_0); old_last = old_last.max(old_0);
            new_first = new_first.min(new_0); new_last = new_last.max(new_0);
        }

        let old_ctx_before = old_first.saturating_sub(CONTEXT_LINES);
        let new_ctx_before = new_first.saturating_sub(CONTEXT_LINES);
        let old_ctx_after = (old_last + 1 + CONTEXT_LINES).min(difft.old_lines.len());
        let new_ctx_after = (new_last + 1 + CONTEXT_LINES).min(difft.new_lines.len());

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
            out.push_str(&format!(" {}\n", line));
        }

        // Build unified item list (same logic as HTML renderer)
        #[derive(Clone, Copy)]
        enum TextItem<'a> {
            DifftRow(&'a DiffRow<'a>),
            RemovedLine(usize),
            AddedLine(usize),
            PairedLine(usize, usize),
        }

        let rows = consolidate_chunk(chunk);
        let mut items: Vec<(u64, TextItem)> = Vec::new();
        for row in &rows {
            let sort_key = row.rhs.map(|s| s.line_number)
                .or(row.lhs.map(|s| s.line_number))
                .unwrap_or(u64::MAX);
            items.push((sort_key, TextItem::DifftRow(row)));
        }
        for &(old_0, new_0) in &extra_paired {
            items.push((new_0 as u64, TextItem::PairedLine(old_0, new_0)));
        }
        for &old_0 in &extra_removed {
            let new_1 = old_to_new_line(old_0 as u64 + 1, hunks);
            items.push((new_1 - 1, TextItem::RemovedLine(old_0)));
        }
        for &new_0 in &extra_added {
            items.push((new_0 as u64, TextItem::AddedLine(new_0)));
        }
        items.sort_by_key(|&(k, _)| k);

        let mut prev_old: Option<usize> = None;
        let mut prev_new: Option<usize> = None;

        for &(_, ref item) in &items {
            let (cur_old, cur_new) = match item {
                TextItem::DifftRow(row) => (
                    row.lhs.map(|s| s.line_number as usize),
                    row.rhs.map(|s| s.line_number as usize),
                ),
                TextItem::RemovedLine(o) => (Some(*o), None),
                TextItem::AddedLine(n) => (None, Some(*n)),
                TextItem::PairedLine(o, n) => (Some(*o), Some(*n)),
            };

            // Fill gaps with context
            let expected_old = prev_old.map(|p| p + 1);
            let expected_new = prev_new.map(|p| p + 1);
            if let (Some(exp_o), Some(cur_o)) = (expected_old, cur_old) {
                if cur_o > exp_o {
                    let exp_n = expected_new.unwrap_or_else(|| to_0based(old_to_new_line(exp_o as u64 + 1, hunks)));
                    for i in 0..(cur_o - exp_o) {
                        let line = difft.new_lines.get(exp_n + i).map(|s| s.as_str()).unwrap_or("");
                        out.push_str(&format!(" {}\n", line));
                    }
                }
            } else if let (Some(exp_n), Some(cur_n)) = (expected_new, cur_new) {
                if cur_n > exp_n {
                    let exp_o = expected_old.unwrap_or_else(|| to_0based(new_to_old_line(exp_n as u64 + 1, hunks)));
                    for i in 0..(cur_n - exp_n) {
                        let line = difft.new_lines.get(exp_n + i).map(|s| s.as_str()).unwrap_or("");
                        let _ = exp_o; // context from new side is sufficient
                        out.push_str(&format!(" {}\n", line));
                    }
                }
            }

            // Render the item
            match item {
                TextItem::DifftRow(row) => {
                    match (row.lhs, row.rhs) {
                        (Some(lhs), Some(rhs)) => {
                            let old_line = difft.old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
                            let new_line = difft.new_lines.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
                            out.push_str(&format!("-{}\n", old_line));
                            out.push_str(&format!("+{}\n", new_line));
                        }
                        (Some(lhs), None) => {
                            let old_line = difft.old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
                            out.push_str(&format!("-{}\n", old_line));
                        }
                        (None, Some(rhs)) => {
                            let new_line = difft.new_lines.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
                            out.push_str(&format!("+{}\n", new_line));
                        }
                        (None, None) => {}
                    }
                }
                TextItem::RemovedLine(old_0) => {
                    let line = difft.old_lines.get(*old_0).map(|s| s.as_str()).unwrap_or("");
                    out.push_str(&format!("-{}\n", line));
                }
                TextItem::AddedLine(new_0) => {
                    let line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
                    out.push_str(&format!("+{}\n", line));
                }
                TextItem::PairedLine(old_0, new_0) => {
                    let old_line = difft.old_lines.get(*old_0).map(|s| s.as_str()).unwrap_or("");
                    let new_line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
                    out.push_str(&format!("-{}\n", old_line));
                    out.push_str(&format!("+{}\n", new_line));
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
            let line = difft.new_lines.get(new_post_start + i).map(|s| s.as_str()).unwrap_or("");
            out.push_str(&format!(" {}\n", line));
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

fn render_chunks(difft: &DifftOutput, chunk_indices: &[usize], file_path: &str) -> String {
    let mut html = String::new();
    html.push_str(&format!(
        "<div class=\"diff-block\"><div class=\"diff-header\">{}</div>",
        html_escape(file_path)
    ));
    html.push_str(
        "<table class=\"diff-table\"><colgroup>\
         <col class=\"ln-col\"><col class=\"code-col\">\
         <col class=\"ln-col\"><col class=\"code-col\">\
         </colgroup><tbody>",
    );

    let hunks = &difft.hunks;

    // Track the last rendered line indices to avoid duplicating context between chunks.
    let mut last_old_rendered: Option<usize> = None;
    let mut last_new_rendered: Option<usize> = None;

    let mut first_chunk = true;
    for &idx in chunk_indices {
        let Some(chunk) = difft.chunks.get(idx) else { continue };

        let (lhs_range, rhs_range) = chunk_line_range(chunk);

        // Resolve the first/last 0-based line on each side, using unified diff hunks
        // to map between old/new when one side has no entries in this chunk.
        let (old_first, old_last) = match lhs_range {
            Some((min, max)) => (min as usize, max as usize),
            None => {
                let (rmin, rmax) = rhs_range.unwrap_or((0, 0));
                // rmin/rmax are 0-based difft lines; hunks use 1-based git lines
                let o_min = to_0based(new_to_old_line(rmin + 1, hunks));
                let o_max = to_0based(new_to_old_line(rmax + 1, hunks));
                (o_min, o_max)
            }
        };
        let (new_first, new_last) = match rhs_range {
            Some((min, max)) => (min as usize, max as usize),
            None => {
                let (lmin, lmax) = lhs_range.unwrap_or((0, 0));
                let n_min = to_0based(old_to_new_line(lmin + 1, hunks));
                let n_max = to_0based(old_to_new_line(lmax + 1, hunks));
                (n_min, n_max)
            }
        };

        // Collect 0-based line numbers that difft already covers in this chunk.
        let mut difft_old_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut difft_new_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for entry in chunk {
            if let Some(lhs) = &entry.lhs {
                difft_old_lines.insert(lhs.line_number as usize);
            }
            if let Some(rhs) = &entry.rhs {
                difft_new_lines.insert(rhs.line_number as usize);
            }
        }

        // Find unified diff hunk lines that overlap this chunk's range but difft missed.
        // These are removed/added lines that difft's structural matching didn't flag.
        // Must be computed BEFORE context so we can extend boundaries to include hunk lines.
        let mut extra_removed: Vec<usize> = Vec::new(); // 0-based old line indices
        let mut extra_added: Vec<usize> = Vec::new(); // 0-based new line indices
        let mut extra_paired: Vec<(usize, usize)> = Vec::new(); // (old_0, new_0)

        for h in hunks {
            let h_old_start_0 = (h.old_start as usize).saturating_sub(1); // 0-based
            let h_old_end_0 = h_old_start_0 + h.old_count as usize;
            let h_new_start_0 = (h.new_start as usize).saturating_sub(1);
            let h_new_end_0 = h_new_start_0 + h.new_count as usize;

            let overlaps_old = h_old_end_0 > old_first.saturating_sub(1) && h_old_start_0 <= old_last + 1;
            let overlaps_new = h_new_end_0 > new_first.saturating_sub(1) && h_new_start_0 <= new_last + 1;

            if overlaps_old || overlaps_new {
                let mut hunk_removed: Vec<usize> = Vec::new();
                let mut hunk_added: Vec<usize> = Vec::new();
                for line_0 in h_old_start_0..h_old_end_0 {
                    if !difft_old_lines.contains(&line_0) {
                        hunk_removed.push(line_0);
                    }
                }
                for line_0 in h_new_start_0..h_new_end_0 {
                    if !difft_new_lines.contains(&line_0) {
                        hunk_added.push(line_0);
                    }
                }
                // Pair removed/added lines from the same hunk together
                let pair_count = hunk_removed.len().min(hunk_added.len());
                for i in 0..pair_count {
                    extra_paired.push((hunk_removed[i], hunk_added[i]));
                }
                for &r in &hunk_removed[pair_count..] {
                    extra_removed.push(r);
                }
                for &a in &hunk_added[pair_count..] {
                    extra_added.push(a);
                }
            }
        }

        // Extend chunk boundaries to include hunk-only lines so context doesn't
        // overlap with changed lines.
        let mut old_first = old_first;
        let mut old_last = old_last;
        let mut new_first = new_first;
        let mut new_last = new_last;
        for &old_0 in &extra_removed {
            old_first = old_first.min(old_0);
            old_last = old_last.max(old_0);
        }
        for &new_0 in &extra_added {
            new_first = new_first.min(new_0);
            new_last = new_last.max(new_0);
        }
        for &(old_0, new_0) in &extra_paired {
            old_first = old_first.min(old_0);
            old_last = old_last.max(old_0);
            new_first = new_first.min(new_0);
            new_last = new_last.max(new_0);
        }

        let old_ctx_before = old_first.saturating_sub(CONTEXT_LINES);
        let new_ctx_before = new_first.saturating_sub(CONTEXT_LINES);
        let old_ctx_after = (old_last + 1 + CONTEXT_LINES).min(difft.old_lines.len());
        let new_ctx_after = (new_last + 1 + CONTEXT_LINES).min(difft.new_lines.len());

        // Clamp to avoid re-rendering lines already shown by the previous chunk.
        let old_ctx_start = match last_old_rendered {
            Some(last) if last + 1 > old_ctx_before => last + 1,
            _ => old_ctx_before,
        };
        let new_ctx_start = match last_new_rendered {
            Some(last) if last + 1 > new_ctx_before => last + 1,
            _ => new_ctx_before,
        };

        // Show separator only if there's a gap between the last rendered line
        // and this chunk's pre-context.
        if !first_chunk {
            let has_gap = match last_old_rendered {
                Some(last) => last + 1 < old_ctx_before,
                None => match last_new_rendered {
                    Some(last) => last + 1 < new_ctx_before,
                    None => true,
                },
            };
            if has_gap {
                html.push_str("<tr class=\"chunk-sep\"><td colspan=\"4\"></td></tr>");
            }
        }
        first_chunk = false;

        // Render context lines BEFORE the chunk.
        let old_pre_count = old_first.saturating_sub(old_ctx_start);
        let new_pre_count = new_first.saturating_sub(new_ctx_start);
        let pre_count = old_pre_count.min(new_pre_count);

        for i in 0..pre_count {
            html.push_str(&render_context_row(
                old_first - pre_count + i,
                new_first - pre_count + i,
                &difft.old_lines,
                &difft.new_lines,
            ));
        }

        // Build a unified list of render items: difft rows + hunk-only lines.
        #[derive(Clone, Copy)]
        enum RenderItem<'a> {
            DifftRow(&'a DiffRow<'a>),
            RemovedLine(usize),        // 0-based old line
            AddedLine(usize),          // 0-based new line
            PairedLine(usize, usize),  // (old_0, new_0) from same hunk
        }

        let rows = consolidate_chunk(chunk);
        let mut items: Vec<(u64, RenderItem)> = Vec::new();

        for row in &rows {
            let sort_key = row.rhs.map(|s| s.line_number)
                .or(row.lhs.map(|s| s.line_number))
                .unwrap_or(u64::MAX);
            items.push((sort_key, RenderItem::DifftRow(row)));
        }
        for &(old_0, new_0) in &extra_paired {
            items.push((new_0 as u64, RenderItem::PairedLine(old_0, new_0)));
        }
        for &old_0 in &extra_removed {
            let new_1 = old_to_new_line(old_0 as u64 + 1, hunks);
            items.push((new_1 - 1, RenderItem::RemovedLine(old_0)));
        }
        for &new_0 in &extra_added {
            items.push((new_0 as u64, RenderItem::AddedLine(new_0)));
        }
        items.sort_by_key(|&(k, _)| k);

        // Render items, filling gaps with context lines.
        let mut prev_old: Option<usize> = None;
        let mut prev_new: Option<usize> = None;

        for &(_, ref item) in &items {
            let (cur_old, cur_new) = match item {
                RenderItem::DifftRow(row) => (
                    row.lhs.map(|s| s.line_number as usize),
                    row.rhs.map(|s| s.line_number as usize),
                ),
                RenderItem::RemovedLine(o) => (Some(*o), None),
                RenderItem::AddedLine(n) => (None, Some(*n)),
                RenderItem::PairedLine(o, n) => (Some(*o), Some(*n)),
            };

            // Fill gap with context lines
            let expected_old = prev_old.map(|p| p + 1);
            let expected_new = prev_new.map(|p| p + 1);

            if let (Some(exp_o), Some(cur_o)) = (expected_old, cur_old) {
                if cur_o > exp_o {
                    let exp_n = expected_new.unwrap_or_else(|| {
                        to_0based(old_to_new_line(exp_o as u64 + 1, hunks))
                    });
                    let gap = cur_o - exp_o;
                    for i in 0..gap {
                        html.push_str(&render_context_row(
                            exp_o + i, exp_n + i,
                            &difft.old_lines, &difft.new_lines,
                        ));
                    }
                }
            } else if let (Some(exp_n), Some(cur_n)) = (expected_new, cur_new) {
                if cur_n > exp_n {
                    let exp_o = expected_old.unwrap_or_else(|| {
                        to_0based(new_to_old_line(exp_n as u64 + 1, hunks))
                    });
                    let gap = cur_n - exp_n;
                    for i in 0..gap {
                        html.push_str(&render_context_row(
                            exp_o + i, exp_n + i,
                            &difft.old_lines, &difft.new_lines,
                        ));
                    }
                }
            }

            // Render the item
            match item {
                RenderItem::DifftRow(row) => {
                    html.push_str(&render_diff_row(row, &difft.old_lines, &difft.new_lines));
                }
                RenderItem::RemovedLine(old_0) => {
                    let content = difft.old_lines.get(*old_0).map(|s| html_escape(s)).unwrap_or_default();
                    html.push_str(&format!(
                        "<tr class=\"line-removed\"><td class=\"ln\">{}</td><td class=\"code-lhs\">{}</td>\
                         <td class=\"ln\"></td><td class=\"code-rhs\"></td></tr>",
                        old_0 + 1, content
                    ));
                }
                RenderItem::AddedLine(new_0) => {
                    let content = difft.new_lines.get(*new_0).map(|s| html_escape(s)).unwrap_or_default();
                    html.push_str(&format!(
                        "<tr class=\"line-added\"><td class=\"ln\"></td><td class=\"code-lhs\"></td>\
                         <td class=\"ln\">{}</td><td class=\"code-rhs\">{}</td></tr>",
                        new_0 + 1, content
                    ));
                }
                RenderItem::PairedLine(old_0, new_0) => {
                    let old_line = difft.old_lines.get(*old_0).map(|s| s.as_str()).unwrap_or("");
                    let new_line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
                    let (old_cs, old_ce, new_cs, new_ce) = find_change_bounds(old_line, new_line);
                    html.push_str(&format!(
                        "<tr class=\"line-paired\"><td class=\"ln\">{}</td><td class=\"code-lhs\">{}</td>\
                         <td class=\"ln\">{}</td><td class=\"code-rhs\">{}</td></tr>",
                        old_0 + 1, render_refined_line(old_line, old_cs, old_ce, "hl-del"),
                        new_0 + 1, render_refined_line(new_line, new_cs, new_ce, "hl-add"),
                    ));
                }
            }

            if let Some(o) = cur_old { prev_old = Some(o); }
            if let Some(n) = cur_new { prev_new = Some(n); }
        }

        // Render context lines AFTER the chunk.
        let old_post_start = old_last + 1;
        let new_post_start = new_last + 1;
        let old_post_count = old_ctx_after.saturating_sub(old_post_start);
        let new_post_count = new_ctx_after.saturating_sub(new_post_start);
        let post_count = old_post_count.min(new_post_count);

        for i in 0..post_count {
            html.push_str(&render_context_row(
                old_post_start + i,
                new_post_start + i,
                &difft.old_lines,
                &difft.new_lines,
            ));
        }

        if post_count > 0 {
            last_old_rendered = Some(old_post_start + post_count - 1);
            last_new_rendered = Some(new_post_start + post_count - 1);
        } else {
            last_old_rendered = Some(old_last);
            last_new_rendered = Some(new_last);
        }
    }

    html.push_str("</tbody></table></div>");
    html
}

pub fn run(walkthrough_path: &Path, data_dir: &Path, output_path: &Path) -> Result<()> {
    let md_content = fs::read_to_string(walkthrough_path)
        .with_context(|| format!("Failed to read {}", walkthrough_path.display()))?;

    let difft_re = Regex::new(r"^difft\s+(\S+)\s+chunks=(\S+)")?;

    // Load all difft JSON data
    let mut data: HashMap<String, DifftOutput> = HashMap::new();
    for entry in fs::read_dir(data_dir).context("Failed to read data directory")? {
        let entry = entry?;
        if entry.path().extension().map_or(false, |e| e == "json") {
            let json_str = fs::read_to_string(entry.path())?;
            let difft: DifftOutput = serde_json::from_str(&json_str)
                .with_context(|| format!("Failed to parse {}", entry.path().display()))?;
            if let Some(ref path) = difft.path {
                data.insert(path.clone(), difft);
            }
        }
    }

    // First pass: replace difft code blocks with HTML placeholders,
    // and build the enriched markdown with text diffs in code block bodies.
    // Also track which (file, chunk) pairs are referenced for verification.
    let mut processed_md = String::new();
    let mut enriched_md = String::new();
    let mut diff_blocks: Vec<String> = Vec::new();
    let mut in_difft_block = false;
    let mut referenced: std::collections::HashSet<(String, usize)> = std::collections::HashSet::new();

    for line in md_content.lines() {
        if !in_difft_block {
            if line.starts_with("```") && line.chars().filter(|&c| c == '`').count() >= 3 {
                let backtick_count = line.chars().take_while(|&c| c == '`').count();
                let info = &line[backtick_count..];

                if let Some(caps) = difft_re.captures(info.trim()) {
                    let file = caps[1].to_string();
                    let chunks_spec = caps[2].to_string();

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
                        let rendered_html = render_chunks(difft, &indices, &file);
                        let placeholder_id = diff_blocks.len();
                        diff_blocks.push(rendered_html);
                        processed_md
                            .push_str(&format!("<!-- DIFF_PLACEHOLDER_{} -->\n", placeholder_id));

                        // Write enriched markdown: opening fence + text diff + closing fence
                        let text_diff = render_chunks_text(difft, &indices);
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
    let options = Options::empty();
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

    if !all_covered {
        eprintln!("{} uncovered chunks:", uncovered.len());
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

    let full_html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
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
