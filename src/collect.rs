use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

pub fn run(diff_args: &[String], output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;

    // Clean any previous JSON files in the output dir
    if let Ok(entries) = fs::read_dir(output_dir) {
        for entry in entries.flatten() {
            if entry.path().extension().map_or(false, |e| e == "json") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

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
                // Normal: "M\tpath"
                2 => Some((parts[0].to_string(), parts[1].to_string())),
                // Rename: "R100\told\tnew"
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

        // Parse JSON and inject/override path and status
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
        }

        // Count chunks
        let chunk_count = json
            .get("chunks")
            .and_then(|c| c.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        total_chunks += chunk_count;

        // Save to output dir with safe filename
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
    Ok(())
}
