use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;

pub fn run(walkthrough_path: &Path, data_dir: &Path) -> Result<bool> {
    // 1. Load all JSON files and count chunks per file
    let mut file_chunks: HashMap<String, usize> = HashMap::new();

    for entry in fs::read_dir(data_dir).context("Failed to read data directory")? {
        let entry = entry?;
        if entry.path().extension().map_or(false, |e| e == "json") {
            let json_str = fs::read_to_string(entry.path())?;
            let json: serde_json::Value = serde_json::from_str(&json_str)
                .with_context(|| format!("Failed to parse {}", entry.path().display()))?;

            let path = json
                .get("path")
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string();

            let chunk_count = json
                .get("chunks")
                .and_then(|c| c.as_array())
                .map(|a| a.len())
                .unwrap_or(0);

            if chunk_count > 0 && !path.is_empty() {
                file_chunks.insert(path, chunk_count);
            }
        }
    }

    if file_chunks.is_empty() {
        eprintln!("No chunk data found in {}", data_dir.display());
        let result = serde_json::json!({
            "complete": true,
            "uncovered": [],
            "total_chunks": 0,
            "referenced_chunks": 0,
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(true);
    }

    // 2. Parse markdown for difft annotations
    let md_content = fs::read_to_string(walkthrough_path)
        .with_context(|| format!("Failed to read {}", walkthrough_path.display()))?;

    let re = Regex::new(r"```difft\s+(\S+)\s+chunks=(\S+)")?;

    let mut referenced: HashSet<(String, usize)> = HashSet::new();

    for cap in re.captures_iter(&md_content) {
        let file = cap[1].to_string();
        let chunks_spec = &cap[2];

        if chunks_spec == "all" {
            if let Some(&count) = file_chunks.get(&file) {
                for i in 0..count {
                    referenced.insert((file.clone(), i));
                }
            }
        } else {
            for part in chunks_spec.split(',') {
                if let Ok(idx) = part.trim().parse::<usize>() {
                    referenced.insert((file.clone(), idx));
                }
            }
        }
    }

    // 3. Check completeness
    let mut uncovered: Vec<(String, usize)> = Vec::new();
    let total_chunks: usize = file_chunks.values().sum();

    for (file, chunk_count) in &file_chunks {
        for i in 0..*chunk_count {
            if !referenced.contains(&(file.clone(), i)) {
                uncovered.push((file.clone(), i));
            }
        }
    }

    let all_covered = uncovered.is_empty();

    if all_covered {
        eprintln!("All {} chunks are referenced.", total_chunks);
    } else {
        eprintln!(
            "{} uncovered chunks (out of {}):",
            uncovered.len(),
            total_chunks
        );
        for (file, idx) in &uncovered {
            eprintln!("  {} chunk {}", file, idx);
        }
    }

    // Structured output for the skill to parse
    let result = serde_json::json!({
        "complete": all_covered,
        "uncovered": uncovered.iter().map(|(f, i)| {
            serde_json::json!({"file": f, "chunk": *i})
        }).collect::<Vec<_>>(),
        "total_chunks": total_chunks,
        "referenced_chunks": referenced.len(),
    });

    println!("{}", serde_json::to_string_pretty(&result)?);

    Ok(all_covered)
}
