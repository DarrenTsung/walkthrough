use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use pulldown_cmark::{Options, Parser};
use regex::Regex;

use arborium::advanced::Span;
use arborium::Highlighter;

use crate::difft_json::{DifftOutput, LineEntry, LineSide};

/// Number of unchanged context lines to show before/after each chunk.
const CONTEXT_LINES: usize = 3;

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
    --added-hl: #aceebb;
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
    padding: 24px 28px 60px;
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
col.sign-col { width: 1.5em; }
col.code-col { width: calc(50% - 5em); }

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

.diff-table td.code-lhs {
    border-right: 1px solid var(--border);
}

/* Context lines (unchanged) */
tr.line-context td { background: var(--bg); }

/* Removed lines (lhs side highlighted, rhs side empty) */
tr.line-removed td.code-lhs,
tr.line-removed .sign-lhs { background: var(--removed-bg); }
tr.line-removed .ln-lhs { background: var(--removed-num-bg); }
tr.line-removed td.code-rhs,
tr.line-removed .sign-rhs,
tr.line-removed .ln-rhs { background: var(--empty-bg); }

/* Added lines (rhs side highlighted, lhs side empty) */
tr.line-added td.code-rhs,
tr.line-added .sign-rhs { background: var(--added-bg); }
tr.line-added .ln-rhs { background: var(--added-num-bg); }
tr.line-added td.code-lhs,
tr.line-added .sign-lhs,
tr.line-added .ln-lhs { background: var(--empty-bg); }

/* Paired rows: removed on left, added on right */
tr.line-paired td.code-lhs,
tr.line-paired .sign-lhs { background: var(--removed-bg); }
tr.line-paired .ln-lhs { background: var(--removed-num-bg); }
tr.line-paired td.code-rhs,
tr.line-paired .sign-rhs { background: var(--added-bg); }
tr.line-paired .ln-rhs { background: var(--added-num-bg); }

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

/// Map a tree-sitter capture name to a GitHub prettylights color.
/// Returns (color, italic). No more fragile RGB matching.
fn github_color_for_capture(capture: &str) -> (&'static str, bool) {
    // Tree-sitter captures are hierarchical: "keyword.function", "string.special", etc.
    // Match the most specific prefix first.
    if capture.starts_with("comment") {
        return ("#59636e", true);
    }
    if capture.starts_with("string") {
        return ("#0a3069", false);
    }
    if capture.starts_with("constant") || capture.starts_with("number") || capture.starts_with("boolean") || capture.starts_with("float") {
        return ("#0550ae", false);
    }
    if capture.starts_with("keyword") || capture.starts_with("repeat") || capture.starts_with("conditional")
        || capture.starts_with("exception") || capture.starts_with("include") || capture.starts_with("storageclass")
    {
        return ("#cf222e", false);
    }
    if capture.starts_with("type") || capture.starts_with("constructor") {
        return ("#0550ae", false);
    }
    if capture.starts_with("function") || capture.starts_with("method") {
        return ("#6639ba", false);
    }
    if capture.starts_with("variable") || capture.starts_with("parameter") || capture.starts_with("field")
        || capture.starts_with("property")
    {
        return ("#953800", false);
    }
    if capture.starts_with("operator") || capture.starts_with("punctuation") {
        return ("#1f2328", false);
    }
    if capture.starts_with("tag") {
        return ("#0550ae", false);
    }
    if capture.starts_with("attribute") {
        return ("#6639ba", false);
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
    if capture.starts_with("type") || capture.starts_with("constructor") { return 8; }
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

fn render_diff_row(
    row: &DiffRow,
    old_lines: &[String], new_lines: &[String],
    old_hl: &[String], new_hl: &[String],
) -> String {
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

    // For paired rows, syntax-highlight the line then overlay the diff highlight.
    if let (Some(lhs), Some(rhs)) = (row.lhs, row.rhs) {
        let old_line = old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let new_line = new_lines.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let old_highlighted = old_hl.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let new_highlighted = new_hl.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        let (old_cs, old_ce, new_cs, new_ce) = find_change_bounds(old_line, new_line);

        html.push_str(&format!(
            "<td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td>",
            lhs.line_number + 1,
            insert_diff_highlight(old_highlighted, old_cs, old_ce, "hl-del")
        ));
        html.push_str(&format!(
            "<td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td>",
            rhs.line_number + 1,
            insert_diff_highlight(new_highlighted, new_cs, new_ce, "hl-add")
        ));
    } else if let Some(lhs) = row.lhs {
        let content = old_hl.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        html.push_str(&format!(
            "<td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td>",
            lhs.line_number + 1, content
        ));
        html.push_str("<td class=\"ln ln-rhs\"></td><td class=\"sign sign-rhs\"></td><td class=\"code-rhs\"></td>");
    } else if let Some(rhs) = row.rhs {
        let content = new_hl.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
        html.push_str("<td class=\"ln ln-lhs\"></td><td class=\"sign sign-lhs\"></td><td class=\"code-lhs\"></td>");
        html.push_str(&format!(
            "<td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td>",
            rhs.line_number + 1, content
        ));
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
/// Optional 0-based line range filter (inclusive on both ends, new-file lines).
type LineFilter = Option<(usize, usize)>;

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
            out.push_str(&fmt_line(" ", new_idx, line));
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
                .or(row.lhs.map(|s| {
                    old_to_new_line(s.line_number + 1, hunks) - 1
                }))
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
                TextItem::RemovedLine(o) => { item_old_lines_t.insert(*o); }
                TextItem::AddedLine(n) => { item_new_lines_t.insert(*n); }
                TextItem::PairedLine(o, n) => { item_old_lines_t.insert(*o); item_new_lines_t.insert(*n); }
            }
        }

        // Apply line filter after collecting skip sets.
        if let Some((filter_start, filter_end)) = line_filter {
            items.retain(|&(k, _)| {
                let n = k as usize;
                n >= filter_start && n <= filter_end
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
                    TextItem::RemovedLine(o) => { old_first = old_first.min(*o); old_last = old_last.max(*o); }
                    TextItem::AddedLine(n) => { new_first = new_first.min(*n); new_last = new_last.max(*n); }
                    TextItem::PairedLine(o, n) => {
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
                TextItem::RemovedLine(o) => (Some(*o), None),
                TextItem::AddedLine(n) => (None, Some(*n)),
                TextItem::PairedLine(o, n) => (Some(*o), Some(*n)),
            };

            // Fill gaps with context, skipping lines that items already cover
            let expected_old = prev_old.map(|p| p + 1);
            let expected_new = prev_new.map(|p| p + 1);
            if let (Some(exp_o), Some(cur_o)) = (expected_old, cur_old) {
                if cur_o > exp_o {
                    let exp_n = expected_new.unwrap_or_else(|| to_0based(old_to_new_line(exp_o as u64 + 1, hunks)));
                    for i in 0..(cur_o - exp_o) {
                        let n = exp_n + i;
                        let o = exp_o + i;
                        if !item_old_lines_t.contains(&o) && !item_new_lines_t.contains(&n) {
                            let line = difft.new_lines.get(n).map(|s| s.as_str()).unwrap_or("");
                            out.push_str(&fmt_line(" ", n, line));
                        }
                    }
                }
            } else if let (Some(exp_n), Some(cur_n)) = (expected_new, cur_new) {
                if cur_n > exp_n {
                    let exp_o = expected_old.unwrap_or_else(|| to_0based(new_to_old_line(exp_n as u64 + 1, hunks)));
                    for i in 0..(cur_n - exp_n) {
                        let n = exp_n + i;
                        let o = exp_o + i;
                        if !item_old_lines_t.contains(&o) && !item_new_lines_t.contains(&n) {
                            let line = difft.new_lines.get(n).map(|s| s.as_str()).unwrap_or("");
                            out.push_str(&fmt_line(" ", n, line));
                        }
                    }
                }
            }

            // Render the item with relative line numbers
            match item {
                TextItem::DifftRow(row) => {
                    match (row.lhs, row.rhs) {
                        (Some(lhs), Some(rhs)) => {
                            let n = rhs.line_number as usize;
                            let old_line = difft.old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
                            let new_line = difft.new_lines.get(n).map(|s| s.as_str()).unwrap_or("");
                            out.push_str(&fmt_line("-", n, old_line));
                            out.push_str(&fmt_line("+", n, new_line));
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
                TextItem::RemovedLine(old_0) => {
                    let n = to_0based(old_to_new_line(*old_0 as u64 + 1, hunks));
                    let line = difft.old_lines.get(*old_0).map(|s| s.as_str()).unwrap_or("");
                    out.push_str(&fmt_line("-", n, line));
                }
                TextItem::AddedLine(new_0) => {
                    let line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
                    out.push_str(&fmt_line("+", *new_0, line));
                }
                TextItem::PairedLine(old_0, new_0) => {
                    let old_line = difft.old_lines.get(*old_0).map(|s| s.as_str()).unwrap_or("");
                    let new_line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
                    out.push_str(&fmt_line("-", *new_0, old_line));
                    out.push_str(&fmt_line("+", *new_0, new_line));
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

fn render_chunks(difft: &DifftOutput, chunk_indices: &[usize], file_path: &str, line_filter: LineFilter, hl: &mut Highlighter) -> String {
    let lang = arborium::detect_language(file_path);
    let old_hl = syntax_highlight_lines(&difft.old_lines, hl, lang);
    let new_hl = syntax_highlight_lines(&difft.new_lines, hl, lang);

    let mut html = String::new();
    html.push_str(&format!(
        "<div class=\"diff-block\"><div class=\"diff-header\">{}</div>",
        html_escape(file_path)
    ));
    html.push_str(
        "<table class=\"diff-table\"><colgroup>\
         <col class=\"ln-col\"><col class=\"sign-col\"><col class=\"code-col\">\
         <col class=\"ln-col\"><col class=\"sign-col\"><col class=\"code-col\">\
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
                .or(row.lhs.map(|s| {
                    // Map old-file line to new-file position for consistent sorting
                    old_to_new_line(s.line_number + 1, hunks) - 1
                }))
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

        // Collect ALL item line numbers before filtering, so the gap filler
        // never renders filtered-out changed lines as context rows.
        let mut item_old_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut item_new_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for &(_, ref item) in &items {
            match item {
                RenderItem::DifftRow(row) => {
                    if let Some(lhs) = row.lhs { item_old_lines.insert(lhs.line_number as usize); }
                    if let Some(rhs) = row.rhs { item_new_lines.insert(rhs.line_number as usize); }
                }
                RenderItem::RemovedLine(o) => { item_old_lines.insert(*o); }
                RenderItem::AddedLine(n) => { item_new_lines.insert(*n); }
                RenderItem::PairedLine(o, n) => { item_old_lines.insert(*o); item_new_lines.insert(*n); }
            }
        }

        // Apply line filter: remove items outside the range but keep their
        // line numbers in the skip sets so they don't become context rows.
        if let Some((filter_start, filter_end)) = line_filter {
            items.retain(|&(k, _)| {
                let n = k as usize;
                n >= filter_start && n <= filter_end
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
                    RenderItem::RemovedLine(o) => { old_first = old_first.min(*o); old_last = old_last.max(*o); }
                    RenderItem::AddedLine(n) => { new_first = new_first.min(*n); new_last = new_last.max(*n); }
                    RenderItem::PairedLine(o, n) => {
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

        // Compute context boundaries (after filter so they reflect the filtered range).
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
                html.push_str("<tr class=\"chunk-sep\"><td colspan=\"6\"></td></tr>");
            }
        }
        first_chunk = false;

        // Render context lines BEFORE the chunk.
        let old_pre_count = old_first.saturating_sub(old_ctx_start);
        let new_pre_count = new_first.saturating_sub(new_ctx_start);
        let pre_count = old_pre_count.min(new_pre_count);

        for i in 0..pre_count {
            let o = old_first - pre_count + i;
            let n = new_first - pre_count + i;
            if !item_old_lines.contains(&o) && !item_new_lines.contains(&n) {
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
                RenderItem::RemovedLine(o) => (Some(*o), None),
                RenderItem::AddedLine(n) => (None, Some(*n)),
                RenderItem::PairedLine(o, n) => (Some(*o), Some(*n)),
            };

            // Fill gap with context lines, skipping any that overlap with items
            let expected_old = prev_old.map(|p| p + 1);
            let expected_new = prev_new.map(|p| p + 1);

            if let (Some(exp_o), Some(cur_o)) = (expected_old, cur_old) {
                if cur_o > exp_o {
                    let exp_n = expected_new.unwrap_or_else(|| {
                        to_0based(old_to_new_line(exp_o as u64 + 1, hunks))
                    });
                    for i in 0..(cur_o - exp_o) {
                        let o = exp_o + i;
                        let n = exp_n + i;
                        if !item_old_lines.contains(&o) && !item_new_lines.contains(&n) {
                            html.push_str(&render_context_row(o, n, &old_hl, &new_hl));
                        }
                    }
                }
            } else if let (Some(exp_n), Some(cur_n)) = (expected_new, cur_new) {
                if cur_n > exp_n {
                    let exp_o = expected_old.unwrap_or_else(|| {
                        to_0based(new_to_old_line(exp_n as u64 + 1, hunks))
                    });
                    for i in 0..(cur_n - exp_n) {
                        let o = exp_o + i;
                        let n = exp_n + i;
                        if !item_old_lines.contains(&o) && !item_new_lines.contains(&n) {
                            html.push_str(&render_context_row(o, n, &old_hl, &new_hl));
                        }
                    }
                }
            }

            // Render the item
            match item {
                RenderItem::DifftRow(row) => {
                    html.push_str(&render_diff_row(row, &difft.old_lines, &difft.new_lines, &old_hl, &new_hl));
                }
                RenderItem::RemovedLine(old_0) => {
                    let content = old_hl.get(*old_0).map(|s| s.as_str()).unwrap_or("");
                    html.push_str(&format!(
                        "<tr class=\"line-removed\"><td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td>\
                         <td class=\"ln ln-rhs\"></td><td class=\"sign sign-rhs\"></td><td class=\"code-rhs\"></td></tr>",
                        old_0 + 1, content
                    ));
                }
                RenderItem::AddedLine(new_0) => {
                    let content = new_hl.get(*new_0).map(|s| s.as_str()).unwrap_or("");
                    html.push_str(&format!(
                        "<tr class=\"line-added\"><td class=\"ln ln-lhs\"></td><td class=\"sign sign-lhs\"></td><td class=\"code-lhs\"></td>\
                         <td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td></tr>",
                        new_0 + 1, content
                    ));
                }
                RenderItem::PairedLine(old_0, new_0) => {
                    let old_line = difft.old_lines.get(*old_0).map(|s| s.as_str()).unwrap_or("");
                    let new_line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
                    let old_highlighted = old_hl.get(*old_0).map(|s| s.as_str()).unwrap_or("");
                    let new_highlighted = new_hl.get(*new_0).map(|s| s.as_str()).unwrap_or("");
                    let (old_cs, old_ce, new_cs, new_ce) = find_change_bounds(old_line, new_line);
                    html.push_str(&format!(
                        "<tr class=\"line-paired\"><td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td>\
                         <td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td></tr>",
                        old_0 + 1, insert_diff_highlight(old_highlighted, old_cs, old_ce, "hl-del"),
                        new_0 + 1, insert_diff_highlight(new_highlighted, new_cs, new_ce, "hl-add"),
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
            let o = old_post_start + i;
            let n = new_post_start + i;
            if !item_old_lines.contains(&o) && !item_new_lines.contains(&n) {
                html.push_str(&render_context_row(o, n, &old_hl, &new_hl));
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

    html.push_str("</tbody></table></div>");
    html
}

/// Generate a walkthrough-ready markdown with all diffs as difft code blocks.
/// The LLM can use this directly as a starting point for the narrative.
pub fn write_summary(data_dir: &Path, output: &Path) -> Result<()> {
    let mut data: Vec<(String, DifftOutput)> = Vec::new();
    for entry in fs::read_dir(data_dir).context("Failed to read data directory")? {
        let entry = entry?;
        if entry.path().extension().map_or(false, |e| e == "json") {
            let json_str = fs::read_to_string(entry.path())?;
            let difft: DifftOutput = serde_json::from_str(&json_str)
                .with_context(|| format!("Failed to parse {}", entry.path().display()))?;
            if let Some(ref path) = difft.path {
                data.push((path.clone(), difft));
            }
        }
    }
    data.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = String::new();
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

pub fn run(walkthrough_path: &Path, data_dir: &Path, output_path: &Path) -> Result<()> {
    let md_content = fs::read_to_string(walkthrough_path)
        .with_context(|| format!("Failed to read {}", walkthrough_path.display()))?;

    let difft_re = Regex::new(r"^difft\s+(\S+)\s+chunks=(\S+)(?:\s+lines=(\S+))?")?;

    let mut hl = Highlighter::new();

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
                        let rendered_html = render_chunks(difft, &indices, &file, line_filter, &mut hl);
                        let placeholder_id = diff_blocks.len();
                        diff_blocks.push(rendered_html);
                        processed_md
                            .push_str(&format!("<!-- DIFF_PLACEHOLDER_{} -->\n", placeholder_id));

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
        render_chunks(difft, chunk_indices, file_path, line_filter, &mut hl)
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
    fn extract_rows(html: &str) -> Vec<(String, Option<u64>, Option<u64>)> {
        let row_re = Regex::new(r#"<tr class="([^"]+)">"#).unwrap();
        let td_re = Regex::new(r#"<td class="ln[^"]*">(\d*)</td>"#).unwrap();

        let mut rows = Vec::new();
        for row_cap in row_re.captures_iter(html) {
            let class = row_cap[1].to_string();
            let start = row_cap.get(0).unwrap().end();
            // Find the closing </tr> after this point
            let rest = &html[start..];
            let end = rest.find("</tr>").unwrap_or(rest.len());
            let row_html = &rest[..end];

            let lns: Vec<Option<u64>> = td_re.captures_iter(row_html)
                .map(|c| {
                    let s = &c[1];
                    if s.is_empty() { None } else { s.parse().ok() }
                })
                .collect();

            let old_ln = lns.first().copied().flatten();
            let new_ln = lns.get(1).copied().flatten();
            rows.push((class, old_ln, new_ln));
        }
        rows
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
        let paired_row_re = Regex::new(r#"(?s)<tr class="line-paired">.*?</tr>"#).unwrap();
        let matched: Vec<_> = paired_row_re.find_iter(&html).collect();
        assert!(!matched.is_empty(), "should have at least one paired row");

        for m in &matched {
            let row_html = m.as_str();
            assert!(row_html.contains("hl-del"),
                "paired row should contain hl-del highlight: {}", row_html);
            assert!(row_html.contains("hl-add"),
                "paired row should contain hl-add highlight: {}", row_html);
        }
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
        run(&md_path, &data_dir, &html_path).unwrap();

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
        run(&md_path, &data_dir, &html_path).unwrap();

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
}
