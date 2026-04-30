use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Detect the default branch remote tracking ref (origin/master or
/// origin/main). Falls back to local master/main if no remote exists.
fn detect_base_ref() -> Result<String> {
    // Prefer remote tracking refs to avoid stale local branches
    for candidate in ["origin/master", "origin/main"] {
        let out = Command::new("git")
            .args(["rev-parse", "--verify", candidate])
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                return Ok(candidate.to_string());
            }
        }
    }
    for candidate in ["master", "main"] {
        let out = Command::new("git")
            .args(["rev-parse", "--verify", candidate])
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                return Ok(candidate.to_string());
            }
        }
    }
    bail!("Could not detect default branch (tried origin/master, origin/main, master, main)")
}

/// Get old and new file content by reading blob SHAs from `git diff --raw`.
fn get_file_contents(
    diff_args: &[String],
    file_path: &str,
) -> Result<(Vec<String>, Vec<String>)> {
    let raw_output = Command::new("git")
        .arg("diff")
        .args(diff_args)
        .arg("--raw")
        .arg("--")
        .arg(file_path)
        .output()
        .context("Failed to run git diff --raw")?;

    let raw_str = String::from_utf8_lossy(&raw_output.stdout);
    let Some(line) = raw_str.lines().next() else {
        return Ok((Vec::new(), Vec::new()));
    };

    // Format: ":old_mode new_mode old_sha new_sha status\tpath"
    // The status+path part is tab-separated from the mode/sha part
    let meta = line.split('\t').next().unwrap_or("");
    let parts: Vec<&str> = meta.split_whitespace().collect();
    if parts.len() < 5 {
        return Ok((Vec::new(), Vec::new()));
    }

    let old_sha = parts[2];
    let new_sha = parts[3];

    let old_content = if old_sha.chars().all(|c| c == '0') {
        String::new()
    } else {
        let out = Command::new("git")
            .arg("cat-file")
            .arg("blob")
            .arg(old_sha)
            .output()
            .context("Failed to read old blob")?;
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    let new_content = if new_sha.chars().all(|c| c == '0') {
        // Working tree: read from disk
        fs::read_to_string(file_path).unwrap_or_default()
    } else {
        let out = Command::new("git")
            .arg("cat-file")
            .arg("blob")
            .arg(new_sha)
            .output()
            .context("Failed to read new blob")?;
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    let old_lines: Vec<String> = old_content.lines().map(|s| s.to_string()).collect();
    let new_lines: Vec<String> = new_content.lines().map(|s| s.to_string()).collect();

    Ok((old_lines, new_lines))
}

/// A parsed unified-diff hunk: header positions plus the body's +/-/' ' lines.
struct ParsedHunk {
    old_start: u64,
    old_count: u64,
    new_start: u64,
    new_count: u64,
    /// Body lines, each tagged with its prefix: '-', '+', or ' '.
    body: Vec<(char, String)>,
}

/// Generate chunks by walking each hunk body. Within a hunk, runs of `-` lines
/// are positionally paired with the immediately following run of `+` lines;
/// any unpaired remainder is recorded as removed-only or added-only. Context
/// lines (' ') just advance the line counters and split paired runs.
fn generate_chunks_from_hunks(hunks: &[ParsedHunk]) -> Vec<serde_json::Value> {
    let mut chunks = Vec::new();

    for hunk in hunks {
        let mut old_line = if hunk.old_start > 0 { hunk.old_start - 1 } else { 0 };
        let mut new_line = if hunk.new_start > 0 { hunk.new_start - 1 } else { 0 };

        let mut entries = Vec::new();

        // Walk the body, accumulating runs of removals followed by additions
        // and flushing them as paired entries when the run ends.
        let mut removed: Vec<u64> = Vec::new();
        let mut added: Vec<u64> = Vec::new();

        let flush = |removed: &mut Vec<u64>, added: &mut Vec<u64>, entries: &mut Vec<serde_json::Value>| {
            let paired = removed.len().min(added.len());
            for i in 0..paired {
                entries.push(serde_json::json!({
                    "lhs": { "line_number": removed[i], "changes": [] },
                    "rhs": { "line_number": added[i], "changes": [] },
                }));
            }
            for &idx in &removed[paired..] {
                entries.push(serde_json::json!({
                    "lhs": { "line_number": idx, "changes": [] },
                }));
            }
            for &idx in &added[paired..] {
                entries.push(serde_json::json!({
                    "rhs": { "line_number": idx, "changes": [] },
                }));
            }
            removed.clear();
            added.clear();
        };

        for (prefix, _text) in &hunk.body {
            match prefix {
                '-' => {
                    removed.push(old_line);
                    old_line += 1;
                }
                '+' => {
                    added.push(new_line);
                    new_line += 1;
                }
                ' ' => {
                    // Context line splits the run; flush before advancing.
                    flush(&mut removed, &mut added, &mut entries);
                    old_line += 1;
                    new_line += 1;
                }
                _ => {}
            }
        }
        flush(&mut removed, &mut added, &mut entries);

        // Sanity: counters should match the hunk header.
        let _ = hunk.old_count;
        let _ = hunk.new_count;

        if !entries.is_empty() {
            chunks.push(serde_json::Value::Array(entries));
        }
    }

    chunks
}

/// Parse a unified-diff hunk header line into the four numbers.
fn parse_hunk_header(line: &str) -> Option<(u64, u64, u64, u64)> {
    let hunk_re = regex::Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").ok()?;
    let cap = hunk_re.captures(line)?;
    let old_start: u64 = cap[1].parse().ok()?;
    let old_count: u64 = cap.get(2).map_or(1, |m| m.as_str().parse().unwrap_or(1));
    let new_start: u64 = cap[3].parse().ok()?;
    let new_count: u64 = cap.get(4).map_or(1, |m| m.as_str().parse().unwrap_or(1));
    Some((old_start, old_count, new_start, new_count))
}

/// Get unified diff hunks (header + body) for a file.
fn get_diff_hunks(
    diff_args: &[String],
    file_path: &str,
    ignore_whitespace: bool,
) -> Result<Vec<ParsedHunk>> {
    let mut cmd = Command::new("git");
    cmd.arg("diff")
        .args(diff_args)
        .arg("-U0")
        .arg("--inter-hunk-context=3")
        .arg("--no-ext-diff");
    if ignore_whitespace {
        cmd.arg("-w");
    }
    let output = cmd
        .arg("--")
        .arg(file_path)
        .output()
        .context("Failed to run git diff -U0")?;

    let diff_str = String::from_utf8_lossy(&output.stdout);
    let mut hunks: Vec<ParsedHunk> = Vec::new();
    let mut in_body = false;

    for line in diff_str.lines() {
        if line.starts_with("@@") {
            if let Some((os, oc, ns, nc)) = parse_hunk_header(line) {
                hunks.push(ParsedHunk {
                    old_start: os,
                    old_count: oc,
                    new_start: ns,
                    new_count: nc,
                    body: Vec::new(),
                });
                in_body = true;
            } else {
                in_body = false;
            }
            continue;
        }
        if !in_body {
            continue;
        }
        // Body lines start with '-', '+', or ' '. Skip everything else
        // (including "\ No newline at end of file").
        let prefix = line.chars().next().unwrap_or('?');
        if prefix == '-' || prefix == '+' || prefix == ' ' {
            // Skip the file headers `--- a/...` / `+++ b/...` that may slip
            // in if a new file appears mid-stream — they only show up before
            // any `@@`, so `in_body` already excludes them.
            if let Some(h) = hunks.last_mut() {
                h.body.push((prefix, line[1..].to_string()));
            }
        } else {
            in_body = false;
        }
    }

    Ok(hunks)
}

pub fn run(diff_args: &[String], output_dir: &Path, ignore_whitespace: bool) -> Result<()> {
    // When no diff args provided, use three-dot syntax against the remote
    // default branch. Three-dot (`A...B`) tells git diff to compute the
    // merge-base itself, giving only the changes on our branch.
    let auto_args;
    let diff_args = if diff_args.is_empty() {
        let base_ref = detect_base_ref()?;
        auto_args = vec![format!("{}...HEAD", base_ref)];
        eprintln!("Auto-detected diff range: {}...HEAD", base_ref);
        &auto_args
    } else {
        diff_args
    };

    fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;

    // Ensure the data dir is gitignored
    let gitignore_path = output_dir.join(".gitignore");
    if !gitignore_path.exists() {
        let _ = fs::write(&gitignore_path, "*\n");
    }

    // Clean any previous JSON files in the output dir
    if let Ok(entries) = fs::read_dir(output_dir) {
        for entry in entries.flatten() {
            if entry.path().extension().map_or(false, |e| e == "json") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    // Resolve commit SHAs for staleness detection. Parse the diff args to
    // find a range like "A..B" or "HEAD~N..HEAD" and resolve to full SHAs.
    let resolve_rev = |rev: &str| -> Option<String> {
        Command::new("git")
            .args(["rev-parse", rev])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let head_sha = resolve_rev("HEAD");
    let mut diff_shas: Option<(String, String)> = None;
    for arg in diff_args {
        if let Some((left, right)) = arg.split_once("..") {
            if let (Some(l), Some(r)) = (resolve_rev(left), resolve_rev(right)) {
                diff_shas = Some((l, r));
            }
            break;
        }
    }
    let mut meta = serde_json::json!({ "diff_args": diff_args });
    if let Some(sha) = &head_sha {
        meta["head_sha"] = serde_json::json!(sha);
    }
    if let Some((left, right)) = &diff_shas {
        meta["diff_left_sha"] = serde_json::json!(left);
        meta["diff_right_sha"] = serde_json::json!(right);
    }
    let _ = fs::write(output_dir.join(".meta.json"), serde_json::to_string_pretty(&meta)?);

    // Get list of changed files with status
    let mut cmd = Command::new("git");
    cmd.arg("diff").args(diff_args).arg("--name-status");
    let output = cmd
        .output()
        .context("Failed to run git diff --name-status")?;

    if !output.status.success() {
        bail!(
            "git diff --name-status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let name_status = String::from_utf8_lossy(&output.stdout);
    let files: Vec<(String, String)> = name_status
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            match parts.len() {
                2 => Some((parts[0].to_string(), parts[1].to_string())),
                3 if parts[0].starts_with('R') => {
                    Some((parts[0].to_string(), parts[2].to_string()))
                }
                _ => {
                    eprintln!("Warning: could not parse name-status line: {}", line);
                    None
                }
            }
        })
        .collect();

    if files.is_empty() {
        eprintln!("No changed files found.");
        return Ok(());
    }

    eprintln!("Found {} changed files:", files.len());

    let mut total_chunks = 0;

    for (status, file_path) in &files {
        eprint!("  {} {} ... ", status, file_path);

        // Get file contents and unified diff hunks
        let (old_lines, new_lines) = get_file_contents(diff_args, file_path)
            .unwrap_or_else(|e| {
                eprintln!("(warning: could not get file contents: {})", e);
                (Vec::new(), Vec::new())
            });

        let hunks = get_diff_hunks(diff_args, file_path, ignore_whitespace)
            .unwrap_or_else(|e| {
                eprintln!("(warning: could not get diff hunks: {})", e);
                Vec::new()
            });

        if hunks.is_empty() && old_lines.is_empty() && new_lines.is_empty() {
            eprintln!("(no changes, skipping)");
            continue;
        }

        // With -w, a modified file may have no non-whitespace hunks at all.
        if ignore_whitespace && hunks.is_empty() && status.starts_with('M') {
            eprintln!("(only whitespace changes, skipping)");
            continue;
        }

        // Generate chunks by parsing each hunk's body.
        let chunks = generate_chunks_from_hunks(&hunks);

        // Convert ParsedHunk -> JSON shape expected by the JSON output / render.
        let hunks_json: Vec<serde_json::Value> = hunks
            .iter()
            .map(|h| {
                serde_json::json!({
                    "old_start": h.old_start,
                    "old_count": h.old_count,
                    "new_start": h.new_start,
                    "new_count": h.new_count,
                })
            })
            .collect();

        let diff_status = match status.chars().next() {
            Some('A') => "added",
            Some('D') => "deleted",
            Some('M') => "changed",
            Some('R') => "renamed",
            _ => "changed",
        };

        let json = serde_json::json!({
            "path": file_path,
            "status": diff_status,
            "language": null,
            "chunks": chunks,
            "old_lines": old_lines,
            "new_lines": new_lines,
            "hunks": hunks_json,
        });

        let chunk_count = json
            .get("chunks")
            .and_then(|c| c.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        total_chunks += chunk_count;

        let safe_name = file_path.replace('/', "__");
        let output_path = output_dir.join(format!("{}.json", safe_name));
        fs::write(&output_path, serde_json::to_string_pretty(&json)?)
            .with_context(|| format!("Failed to write {}", output_path.display()))?;

        eprintln!("{} chunks", chunk_count);
    }

    eprintln!(
        "\nCollected {} total chunks across {} files into {}",
        total_chunks,
        files.len(),
        output_dir.display()
    );

    // Write human-readable summary for LLM consumption
    let summary_path = output_dir.join("SUMMARY.md");
    crate::render::write_summary(output_dir, &summary_path)?;
    eprintln!("Wrote summary to {}", summary_path.display());

    Ok(())
}
