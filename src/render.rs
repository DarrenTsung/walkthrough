use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use pulldown_cmark::{Options, Parser};
use regex::Regex;

use crate::difft_json::{DifftOutput, LineEntry, LineSide};

const CSS: &str = r#"
:root {
    --bg: #ffffff;
    --fg: #24292f;
    --bg-code: #f6f8fa;
    --border: #d0d7de;
    --diff-header-bg: #f1f8ff;
    --diff-header-fg: #0969da;
    --ln-fg: #8b949e;
    --added-bg: #dafbe1;
    --removed-bg: #ffebe9;
    --sep-bg: #f6f8fa;
    --empty-bg: #f6f8fa;
    --hl-keyword: #cf222e;
    --hl-string: #0a3069;
    --hl-comment: #6e7781;
    --hl-type: #8250df;
    --hl-delimiter: #24292f;
    --hl-normal: #24292f;
}

@media (prefers-color-scheme: dark) {
    :root {
        --bg: #0d1117;
        --fg: #e6edf3;
        --bg-code: #161b22;
        --border: #30363d;
        --diff-header-bg: #161b22;
        --diff-header-fg: #58a6ff;
        --ln-fg: #8b949e;
        --added-bg: #12261e;
        --removed-bg: #2d1215;
        --sep-bg: #161b22;
        --empty-bg: #161b22;
        --hl-keyword: #ff7b72;
        --hl-string: #a5d6ff;
        --hl-comment: #8b949e;
        --hl-type: #d2a8ff;
        --hl-delimiter: #e6edf3;
        --hl-normal: #e6edf3;
    }
}

* { box-sizing: border-box; margin: 0; padding: 0; }

body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
    color: var(--fg);
    background: var(--bg);
    line-height: 1.6;
    max-width: 1200px;
    margin: 2rem auto;
    padding: 0 1rem;
}

article > * + * { margin-top: 1rem; }
h1 { font-size: 1.8rem; border-bottom: 1px solid var(--border); padding-bottom: 0.4rem; }
h2 { font-size: 1.4rem; margin-top: 2rem; }
h3 { font-size: 1.15rem; }
p { margin-top: 0.5rem; }
ul, ol { margin-top: 0.5rem; padding-left: 1.5rem; }
code {
    background: var(--bg-code);
    padding: 0.15em 0.4em;
    border-radius: 3px;
    font-size: 0.9em;
    font-family: "SF Mono", "Fira Code", Menlo, Consolas, monospace;
}
pre {
    background: var(--bg-code);
    padding: 1rem;
    border-radius: 6px;
    overflow-x: auto;
}
pre code { background: none; padding: 0; }

.diff-block {
    margin: 1rem 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
}

.diff-header {
    position: sticky;
    top: 0;
    background: var(--diff-header-bg);
    color: var(--diff-header-fg);
    padding: 0.4rem 0.8rem;
    font-family: "SF Mono", "Fira Code", Menlo, Consolas, monospace;
    font-size: 0.82rem;
    font-weight: 600;
    border-bottom: 1px solid var(--border);
    z-index: 1;
}

.diff-table {
    width: 100%;
    border-collapse: collapse;
    font-family: "SF Mono", "Fira Code", Menlo, Consolas, monospace;
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

/* Removed lines: red background on lhs, empty rhs gets subtle bg */
tr.line-removed td.code-lhs { background: var(--removed-bg); }
tr.line-removed td.ln:first-child { background: var(--removed-bg); }
tr.line-removed td.code-rhs { background: var(--empty-bg); }
tr.line-removed td.ln:nth-child(3) { background: var(--empty-bg); }

/* Added lines: green background on rhs, empty lhs gets subtle bg */
tr.line-added td.code-rhs { background: var(--added-bg); }
tr.line-added td.ln:nth-child(3) { background: var(--added-bg); }
tr.line-added td.code-lhs { background: var(--empty-bg); }
tr.line-added td.ln:first-child { background: var(--empty-bg); }

/* Changed lines: both sides highlighted */
tr.line-changed td.code-lhs { background: var(--removed-bg); }
tr.line-changed td.ln:first-child { background: var(--removed-bg); }
tr.line-changed td.code-rhs { background: var(--added-bg); }
tr.line-changed td.ln:nth-child(3) { background: var(--added-bg); }

/* Paired rows: one removed on left, one added on right */
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

.hl-keyword { color: var(--hl-keyword); }
.hl-string { color: var(--hl-string); }
.hl-comment { color: var(--hl-comment); font-style: italic; }
.hl-type { color: var(--hl-type); }
.hl-delimiter { color: var(--hl-delimiter); }
.hl-normal { color: var(--hl-normal); }
.hl-change { background: rgba(255,200,50,0.25); border-radius: 2px; }
"#;

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render a line using full source content, highlighting changed spans.
/// `full_line` is the complete line from old/new file.
/// `spans` are the difft change spans (with byte offsets into the line).
/// For changed lines, only the changed tokens are in spans; the rest is context.
fn render_full_line(full_line: &str, spans: &[crate::difft_json::ChangeSpan]) -> String {
    let mut html = String::new();
    let mut pos: usize = 0;
    let line_len = full_line.len();

    for span in spans {
        let start = span.start.min(line_len);
        let end = span.end.min(line_len);

        // Unchanged portion before this span
        if start > pos {
            if let Some(text) = full_line.get(pos..start) {
                html.push_str(&html_escape(text));
            }
        }

        // Changed portion with highlight
        html.push_str(&format!(
            "<span class=\"hl-change\">{}</span>",
            html_escape(&span.content)
        ));
        pos = end;
    }

    // Unchanged remainder after last span
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

/// Render one side of a line entry.
/// Uses full line content with changed spans highlighted when available.
/// difft uses 0-based line numbers, so `line_number` is used directly as the index.
fn render_side(side: &LineSide, full_lines: &[String]) -> String {
    let line_idx = side.line_number as usize; // 0-based

    if let Some(full_line) = full_lines.get(line_idx) {
        // Render full line with changed spans highlighted
        render_full_line(full_line, &side.changes)
    } else {
        // Fallback: render from spans only
        render_spans_only(side)
    }
}

/// A row to render in the diff table.
struct DiffRow<'a> {
    lhs: Option<&'a LineSide>,
    rhs: Option<&'a LineSide>,
}

/// Consolidate entries within a chunk for better side-by-side layout.
/// Consecutive lhs-only entries followed by rhs-only entries (or vice versa)
/// are paired together on the same rows.
fn consolidate_chunk<'a>(entries: &'a [LineEntry]) -> Vec<DiffRow<'a>> {
    let mut rows = Vec::new();
    let mut i = 0;

    while i < entries.len() {
        let entry = &entries[i];
        let has_lhs = entry.lhs.is_some();
        let has_rhs = entry.rhs.is_some();

        if has_lhs && has_rhs {
            // Both sides present: render as-is
            rows.push(DiffRow {
                lhs: entry.lhs.as_ref(),
                rhs: entry.rhs.as_ref(),
            });
            i += 1;
        } else if has_lhs {
            // Collect consecutive lhs-only entries
            let lhs_start = i;
            while i < entries.len() && entries[i].lhs.is_some() && entries[i].rhs.is_none() {
                i += 1;
            }
            let lhs_run = &entries[lhs_start..i];

            // Check if immediately followed by rhs-only entries
            let rhs_start = i;
            while i < entries.len() && entries[i].rhs.is_some() && entries[i].lhs.is_none() {
                i += 1;
            }
            let rhs_run = &entries[rhs_start..i];

            // Pair them side-by-side
            let max_len = lhs_run.len().max(rhs_run.len());
            for j in 0..max_len {
                rows.push(DiffRow {
                    lhs: lhs_run.get(j).and_then(|e| e.lhs.as_ref()),
                    rhs: rhs_run.get(j).and_then(|e| e.rhs.as_ref()),
                });
            }
        } else if has_rhs {
            // Collect consecutive rhs-only entries
            let rhs_start = i;
            while i < entries.len() && entries[i].rhs.is_some() && entries[i].lhs.is_none() {
                i += 1;
            }
            let rhs_run = &entries[rhs_start..i];

            // Check if immediately followed by lhs-only entries
            let lhs_start = i;
            while i < entries.len() && entries[i].lhs.is_some() && entries[i].rhs.is_none() {
                i += 1;
            }
            let lhs_run = &entries[lhs_start..i];

            // Pair them side-by-side
            let max_len = lhs_run.len().max(rhs_run.len());
            for j in 0..max_len {
                rows.push(DiffRow {
                    lhs: lhs_run.get(j).and_then(|e| e.lhs.as_ref()),
                    rhs: rhs_run.get(j).and_then(|e| e.rhs.as_ref()),
                });
            }
        } else {
            // Neither side - skip
            i += 1;
        }
    }

    rows
}

fn render_row(row: &DiffRow, old_lines: &[String], new_lines: &[String]) -> String {
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

    // LHS cells (display 1-based line number, difft stores 0-based)
    if let Some(lhs) = row.lhs {
        html.push_str(&format!(
            "<td class=\"ln\">{}</td><td class=\"code-lhs\">{}</td>",
            lhs.line_number + 1,
            render_side(lhs, old_lines)
        ));
    } else {
        html.push_str("<td class=\"ln\"></td><td class=\"code-lhs\"></td>");
    }

    // RHS cells
    if let Some(rhs) = row.rhs {
        html.push_str(&format!(
            "<td class=\"ln\">{}</td><td class=\"code-rhs\">{}</td>",
            rhs.line_number + 1,
            render_side(rhs, new_lines)
        ));
    } else {
        html.push_str("<td class=\"ln\"></td><td class=\"code-rhs\"></td>");
    }

    html.push_str("</tr>");
    html
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

    let mut first_chunk = true;
    for &idx in chunk_indices {
        if let Some(chunk) = difft.chunks.get(idx) {
            if !first_chunk {
                html.push_str("<tr class=\"chunk-sep\"><td colspan=\"4\"></td></tr>");
            }
            first_chunk = false;

            let rows = consolidate_chunk(chunk);
            for row in &rows {
                html.push_str(&render_row(row, &difft.old_lines, &difft.new_lines));
            }
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

    // First pass: replace difft code blocks with HTML placeholders.
    let mut processed_md = String::new();
    let mut diff_blocks: Vec<String> = Vec::new();
    let mut in_difft_block = false;

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

                        let rendered_html = render_chunks(difft, &indices, &file);
                        let placeholder_id = diff_blocks.len();
                        diff_blocks.push(rendered_html);
                        processed_md
                            .push_str(&format!("<!-- DIFF_PLACEHOLDER_{} -->\n", placeholder_id));
                    } else {
                        eprintln!("Warning: no data for file '{}', passing through", file);
                        processed_md.push_str(line);
                        processed_md.push('\n');
                    }
                    in_difft_block = true;
                    continue;
                }
            }
            processed_md.push_str(line);
            processed_md.push('\n');
        } else {
            if line.trim_start().starts_with("```") {
                in_difft_block = false;
            }
        }
    }

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

    // Wrap in HTML template
    let full_html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Walkthrough</title>
<style>
{css}
</style>
</head>
<body>
<article>
{body}
</article>
</body>
</html>"#,
        css = CSS,
        body = html_body
    );

    fs::write(output_path, full_html)
        .with_context(|| format!("Failed to write {}", output_path.display()))?;

    eprintln!("Rendered walkthrough to {}", output_path.display());
    Ok(())
}
