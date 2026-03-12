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
    --fg: #24292f;
    --bg-code: #f6f8fa;
    --border: #d0d7de;
    --diff-header-bg: #f1f8ff;
    --diff-header-fg: #0969da;
    --ln-fg: #8b949e;
    --added-bg: #e6ffec;
    --added-hl: #aceebb;
    --removed-bg: #ffebe9;
    --removed-hl: #ffcecb;
    --sep-bg: #f6f8fa;
    --empty-bg: #f6f8fa;
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
        --added-hl: #1a4721;
        --removed-bg: #2d1215;
        --removed-hl: #5d1214;
        --sep-bg: #161b22;
        --empty-bg: #161b22;
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

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render a full source line, highlighting changed spans with the given CSS class.
fn render_full_line(full_line: &str, spans: &[ChangeSpan], hl_class: &str) -> String {
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
            "<span class=\"{}\">{}</span>",
            hl_class,
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
    let mut html = String::new();
    let cs = change_start.min(full_line.len());
    let ce = change_end.min(full_line.len());

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

    // Track the last rendered line indices to avoid duplicating context between chunks.
    let mut last_old_rendered: Option<usize> = None;
    let mut last_new_rendered: Option<usize> = None;

    let mut first_chunk = true;
    for &idx in chunk_indices {
        let Some(chunk) = difft.chunks.get(idx) else { continue };

        let (lhs_range, rhs_range) = chunk_line_range(chunk);

        // Determine context start lines (0-based indices into old/new files).
        let old_start = lhs_range.map(|(min, _)| (min as usize).saturating_sub(CONTEXT_LINES));
        let new_start = rhs_range.map(|(min, _)| (min as usize).saturating_sub(CONTEXT_LINES));
        let old_end = lhs_range.map(|(_, max)| {
            ((max as usize) + 1 + CONTEXT_LINES).min(difft.old_lines.len())
        });
        let new_end = rhs_range.map(|(_, max)| {
            ((max as usize) + 1 + CONTEXT_LINES).min(difft.new_lines.len())
        });

        if !first_chunk {
            html.push_str("<tr class=\"chunk-sep\"><td colspan=\"4\"></td></tr>");
        }
        first_chunk = false;

        // Render context lines BEFORE the chunk.
        if let (Some(os), Some(ns)) = (old_start, new_start) {
            let old_ctx_start = match last_old_rendered {
                Some(last) if last + 1 > os => last + 1,
                _ => os,
            };
            let new_ctx_start = match last_new_rendered {
                Some(last) if last + 1 > ns => last + 1,
                _ => ns,
            };
            let lhs_first = lhs_range.map(|(min, _)| min as usize).unwrap_or(old_ctx_start);
            let rhs_first = rhs_range.map(|(min, _)| min as usize).unwrap_or(new_ctx_start);

            let old_ctx_count = lhs_first.saturating_sub(old_ctx_start);
            let new_ctx_count = rhs_first.saturating_sub(new_ctx_start);
            let ctx_count = old_ctx_count.min(new_ctx_count);

            for i in 0..ctx_count {
                html.push_str(&render_context_row(
                    lhs_first - ctx_count + i,
                    rhs_first - ctx_count + i,
                    &difft.old_lines,
                    &difft.new_lines,
                ));
            }
        }

        // Render the diff rows, sorted by line number to avoid out-of-order display.
        // difft's structural matching can produce entries where e.g. rhs L879 appears
        // before rhs L878 because L878 is paired with an lhs entry.
        let mut rows = consolidate_chunk(chunk);
        rows.sort_by_key(|row| {
            row.rhs
                .map(|s| s.line_number)
                .or(row.lhs.map(|s| s.line_number))
                .unwrap_or(u64::MAX)
        });
        for row in &rows {
            html.push_str(&render_diff_row(row, &difft.old_lines, &difft.new_lines));
        }

        // Render context lines AFTER the chunk.
        if let (Some(oe), Some(ne)) = (old_end, new_end) {
            let lhs_last = lhs_range.map(|(_, max)| max as usize + 1).unwrap_or(0);
            let rhs_last = rhs_range.map(|(_, max)| max as usize + 1).unwrap_or(0);

            let old_ctx_count = oe.saturating_sub(lhs_last);
            let new_ctx_count = ne.saturating_sub(rhs_last);
            let ctx_count = old_ctx_count.min(new_ctx_count);

            for i in 0..ctx_count {
                html.push_str(&render_context_row(
                    lhs_last + i,
                    rhs_last + i,
                    &difft.old_lines,
                    &difft.new_lines,
                ));
            }

            last_old_rendered = Some(lhs_last + ctx_count - 1);
            last_new_rendered = Some(rhs_last + ctx_count - 1);
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
