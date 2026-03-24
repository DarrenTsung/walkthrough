use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

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

/// Get unified diff hunk headers for a file. These give exact old/new line mappings.
fn get_diff_hunks(
    diff_args: &[String],
    file_path: &str,
) -> Result<Vec<serde_json::Value>> {
    let output = Command::new("git")
        .arg("diff")
        .args(diff_args)
        .arg("-U0")
        .arg("--no-ext-diff")
        .arg("--")
        .arg(file_path)
        .output()
        .context("Failed to run git diff -U0")?;

    let diff_str = String::from_utf8_lossy(&output.stdout);
    let hunk_re = regex::Regex::new(r"@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")?;

    let mut hunks = Vec::new();
    for cap in hunk_re.captures_iter(&diff_str) {
        let old_start: u64 = cap[1].parse()?;
        let old_count: u64 = cap.get(2).map_or(1, |m| m.as_str().parse().unwrap_or(1));
        let new_start: u64 = cap[3].parse()?;
        let new_count: u64 = cap.get(4).map_or(1, |m| m.as_str().parse().unwrap_or(1));
        hunks.push(serde_json::json!({
            "old_start": old_start,
            "old_count": old_count,
            "new_start": new_start,
            "new_count": new_count,
        }));
    }

    Ok(hunks)
}

pub fn run(diff_args: &[String], output_dir: &Path) -> Result<()> {
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

        // Run difft for this file via GIT_EXTERNAL_DIFF
        let output = Command::new("git")
            .arg("diff")
            .args(diff_args)
            .arg("--")
            .arg(file_path)
            .env("GIT_EXTERNAL_DIFF", "difft --display json --color never")
            .env("DFT_UNSTABLE", "yes")
            .output()
            .with_context(|| format!("Failed to run difft for {}", file_path))?;

        let json_str = String::from_utf8_lossy(&output.stdout);

        if json_str.trim().is_empty() {
            eprintln!("(no output, skipping)");
            continue;
        }

        // Parse JSON and inject path, status, and file contents
        let mut json: serde_json::Value = serde_json::from_str(&json_str).with_context(|| {
            format!(
                "Failed to parse difft JSON for {}: {}",
                file_path,
                &json_str[..json_str.len().min(200)]
            )
        })?;

        if let Some(obj) = json.as_object_mut() {
            obj.insert(
                "path".to_string(),
                serde_json::Value::String(file_path.clone()),
            );
            let difft_status = match status.chars().next() {
                Some('A') => "added",
                Some('D') => "deleted",
                Some('M') => "changed",
                Some('R') => "renamed",
                _ => "changed",
            };
            obj.entry("status".to_string())
                .or_insert_with(|| serde_json::Value::String(difft_status.to_string()));

            // Embed old/new file contents for full-line rendering
            match get_file_contents(diff_args, file_path) {
                Ok((old_lines, new_lines)) => {
                    obj.insert(
                        "old_lines".to_string(),
                        serde_json::Value::Array(
                            old_lines
                                .into_iter()
                                .map(serde_json::Value::String)
                                .collect(),
                        ),
                    );
                    obj.insert(
                        "new_lines".to_string(),
                        serde_json::Value::Array(
                            new_lines
                                .into_iter()
                                .map(serde_json::Value::String)
                                .collect(),
                        ),
                    );
                }
                Err(e) => {
                    eprintln!("(warning: could not get file contents: {})", e);
                }
            }

            // Embed unified diff hunks for accurate line mapping
            match get_diff_hunks(diff_args, file_path) {
                Ok(hunks) => {
                    obj.insert(
                        "hunks".to_string(),
                        serde_json::Value::Array(hunks),
                    );
                }
                Err(e) => {
                    eprintln!("(warning: could not get diff hunks: {})", e);
                }
            }
        }

        // Ensure chunks field exists (difft omits it for binary/large files).
        // For deleted/added files with 0 chunks, synthesize a chunk from file
        // contents so there's something to render.
        if let Some(obj) = json.as_object_mut() {
            obj.entry("chunks".to_string())
                .or_insert_with(|| serde_json::Value::Array(Vec::new()));

            let needs_synthetic = obj.get("chunks")
                .and_then(|c| c.as_array())
                .map_or(false, |a| a.is_empty());

            if needs_synthetic {
                let is_deleted = obj.get("status")
                    .and_then(|s| s.as_str()) == Some("deleted");
                let is_added = obj.get("status")
                    .and_then(|s| s.as_str()) == Some("added");
                let line_count = if is_deleted {
                    obj.get("old_lines").and_then(|v| v.as_array()).map(|a| a.len())
                } else if is_added {
                    obj.get("new_lines").and_then(|v| v.as_array()).map(|a| a.len())
                } else {
                    None
                };
                if let Some(count) = line_count {
                    if count > 0 {
                        let entries: Vec<serde_json::Value> = (0..count)
                            .map(|i| {
                                let side = serde_json::json!({
                                    "line_number": i,
                                    "changes": []
                                });
                                if is_deleted {
                                    serde_json::json!({"lhs": side, "rhs": null})
                                } else {
                                    serde_json::json!({"lhs": null, "rhs": side})
                                }
                            })
                            .collect();
                        obj.insert("chunks".to_string(), serde_json::json!([entries]));
                    }
                }
            }
        }

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
