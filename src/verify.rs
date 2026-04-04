use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;

use crate::difft_json::DifftOutput;

/// Per-chunk metadata for line-level coverage checking.
struct ChunkMeta {
    /// Min rhs line number (0-based) across all entries, or None for removed-only chunks.
    base_rhs: Option<usize>,
    /// Sorted unique absolute 0-based positions of entries (rhs preferred, lhs fallback).
    /// Matches the render.rs line_filter logic: `rhs.line_number.or(lhs.line_number)`.
    positions: Vec<usize>,
}

/// Coverage state for a (file, chunk_index) pair.
enum Coverage {
    /// Referenced without `lines=` — fully covered.
    Full,
    /// Referenced only with `lines=` — list of absolute 0-based inclusive ranges.
    Ranges(Vec<(usize, usize)>),
}

pub fn run(walkthrough_path: &Path, data_dir: &Path) -> Result<bool> {
    // 1. Load all JSON files and compute chunk metadata per file.
    let mut file_chunks: HashMap<String, Vec<ChunkMeta>> = HashMap::new();

    for entry in fs::read_dir(data_dir).context("Failed to read data directory")? {
        let entry = entry?;
        if entry.path().extension().map_or(false, |e| e == "json") {
            let json_str = fs::read_to_string(entry.path())?;
            let difft: DifftOutput = match serde_json::from_str(&json_str) {
                Ok(d) => d,
                Err(_) => continue, // skip non-difft JSON (e.g. .meta.json)
            };

            let path = difft.path.unwrap_or_default();
            if path.is_empty() || difft.chunks.is_empty() {
                continue;
            }

            let metas: Vec<ChunkMeta> = difft
                .chunks
                .iter()
                .map(|chunk| {
                    let mut base_rhs: Option<usize> = None;
                    let mut positions = Vec::new();

                    for entry in chunk {
                        if let Some(rhs) = &entry.rhs {
                            let ln = rhs.line_number as usize;
                            base_rhs = Some(base_rhs.map_or(ln, |b: usize| b.min(ln)));
                        }

                        let pos = entry
                            .rhs
                            .as_ref()
                            .map(|s| s.line_number as usize)
                            .or(entry.lhs.as_ref().map(|s| s.line_number as usize));
                        if let Some(p) = pos {
                            positions.push(p);
                        }
                    }

                    positions.sort();
                    positions.dedup();
                    ChunkMeta {
                        base_rhs,
                        positions,
                    }
                })
                .collect();

            file_chunks.insert(path, metas);
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

    // 2. Parse markdown for difft code blocks with optional lines= parameter.
    let md_content = fs::read_to_string(walkthrough_path)
        .with_context(|| format!("Failed to read {}", walkthrough_path.display()))?;

    let re = Regex::new(r"```difft?\s+(\S+)\s+chunks=(\S+)(?:\s+lines=(\S+))?")?;

    let mut coverage: HashMap<(String, usize), Coverage> = HashMap::new();

    for cap in re.captures_iter(&md_content) {
        let file = cap[1].to_string();
        let chunks_spec = &cap[2];
        let lines_spec = cap.get(3).map(|m| m.as_str());

        let metas = match file_chunks.get(&file) {
            Some(m) => m,
            None => continue,
        };

        let indices: Vec<usize> = if chunks_spec == "all" {
            (0..metas.len()).collect()
        } else {
            chunks_spec
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect()
        };

        // Convert lines= to absolute 0-based range (matching render.rs logic).
        let abs_range: Option<(usize, usize)> = lines_spec.and_then(|spec| {
            let parts: Vec<&str> = spec.split('-').collect();
            if parts.len() != 2 {
                return None;
            }
            let rel_start: usize = parts[0].parse().ok()?;
            let rel_end: usize = parts[1].parse().ok()?;

            // Combined base across selected chunks (same as render.rs).
            let mut base = usize::MAX;
            for &ci in &indices {
                if let Some(meta) = metas.get(ci) {
                    if let Some(b) = meta.base_rhs {
                        base = base.min(b);
                    }
                }
            }
            if base == usize::MAX {
                return None;
            }

            Some((base + rel_start - 1, base + rel_end - 1))
        });

        for &idx in &indices {
            if idx >= metas.len() {
                continue;
            }
            let key = (file.clone(), idx);

            if matches!(coverage.get(&key), Some(Coverage::Full)) {
                continue;
            }

            match abs_range {
                None => {
                    // No lines= or unparseable — full coverage.
                    coverage.insert(key, Coverage::Full);
                }
                Some(range) => match coverage.entry(key) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(Coverage::Ranges(vec![range]));
                    }
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        if let Coverage::Ranges(ref mut ranges) = e.get_mut() {
                            ranges.push(range);
                        }
                    }
                },
            }
        }
    }

    // 3. Check completeness.
    let total_chunks: usize = file_chunks.values().map(|v| v.len()).sum();
    let referenced_chunks = coverage.len();
    let mut uncovered: Vec<serde_json::Value> = Vec::new();

    let mut sorted_files: Vec<_> = file_chunks.iter().collect();
    sorted_files.sort_by_key(|(f, _)| (*f).clone());

    for (file, metas) in sorted_files {
        for (i, meta) in metas.iter().enumerate() {
            let key = (file.clone(), i);
            match coverage.get(&key) {
                None => {
                    uncovered.push(serde_json::json!({"file": file, "chunk": i}));
                }
                Some(Coverage::Full) => {}
                Some(Coverage::Ranges(ranges)) => {
                    // Check every entry position is within some lines= range.
                    let uncovered_abs: Vec<usize> = meta
                        .positions
                        .iter()
                        .filter(|&&pos| !ranges.iter().any(|&(s, e)| pos >= s && pos <= e))
                        .copied()
                        .collect();

                    if !uncovered_abs.is_empty() {
                        let base = meta.base_rhs.unwrap_or(0);
                        let rel: Vec<usize> =
                            uncovered_abs.iter().map(|&p| p - base + 1).collect();
                        let lines_str = format_ranges(&rel);
                        uncovered.push(serde_json::json!({
                            "file": file,
                            "chunk": i,
                            "uncovered_lines": lines_str,
                        }));
                    }
                }
            }
        }
    }

    let all_covered = uncovered.is_empty();

    if all_covered {
        eprintln!("All {} chunks are referenced.", total_chunks);
    } else {
        let full_uncovered = uncovered
            .iter()
            .filter(|u| u.get("uncovered_lines").is_none())
            .count();
        let partial = uncovered.len() - full_uncovered;

        if full_uncovered > 0 {
            eprintln!(
                "{} uncovered chunks (out of {}):",
                full_uncovered, total_chunks
            );
        }
        if partial > 0 {
            eprintln!("{} partially covered chunks:", partial);
        }

        for item in &uncovered {
            let file = item["file"].as_str().unwrap_or("");
            let chunk = item["chunk"].as_u64().unwrap_or(0);
            if let Some(lines) = item["uncovered_lines"].as_str() {
                eprintln!("  {} chunk {} lines={}", file, chunk, lines);
            } else {
                eprintln!("  {} chunk {}", file, chunk);
            }
        }
    }

    let result = serde_json::json!({
        "complete": all_covered,
        "uncovered": uncovered,
        "total_chunks": total_chunks,
        "referenced_chunks": referenced_chunks,
    });

    println!("{}", serde_json::to_string_pretty(&result)?);

    Ok(all_covered)
}

/// Format sorted positions into compact range notation: "5-10,15,20-25".
fn format_ranges(positions: &[usize]) -> String {
    if positions.is_empty() {
        return String::new();
    }
    let mut ranges = Vec::new();
    let mut start = positions[0];
    let mut end = positions[0];
    for &p in &positions[1..] {
        if p <= end + 1 {
            end = p;
        } else {
            ranges.push(if start == end {
                format!("{start}")
            } else {
                format!("{start}-{end}")
            });
            start = p;
            end = p;
        }
    }
    ranges.push(if start == end {
        format!("{start}")
    } else {
        format!("{start}-{end}")
    });
    ranges.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn test_dir(name: &str) -> std::path::PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("walkthrough_verify_{}_{}", name, id));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_chunk_json(
        dir: &Path,
        filename: &str,
        path: &str,
        entries: Vec<Vec<(Option<u64>, Option<u64>)>>,
    ) {
        let chunks: Vec<serde_json::Value> = entries
            .iter()
            .map(|chunk| {
                let entries: Vec<serde_json::Value> = chunk
                    .iter()
                    .map(|(lhs, rhs)| {
                        let mut obj = serde_json::Map::new();
                        if let Some(l) = lhs {
                            obj.insert(
                                "lhs".into(),
                                serde_json::json!({"line_number": l, "changes": []}),
                            );
                        }
                        if let Some(r) = rhs {
                            obj.insert(
                                "rhs".into(),
                                serde_json::json!({"line_number": r, "changes": []}),
                            );
                        }
                        serde_json::Value::Object(obj)
                    })
                    .collect();
                serde_json::Value::Array(entries)
            })
            .collect();

        let json = serde_json::json!({
            "path": path,
            "chunks": chunks,
            "old_lines": [],
            "new_lines": [],
            "hunks": [],
        });
        fs::write(dir.join(filename), serde_json::to_string(&json).unwrap()).unwrap();
    }

    #[test]
    fn full_coverage_without_lines() {
        let dir = test_dir("full");
        let data = dir.join("data");
        fs::create_dir(&data).unwrap();

        write_chunk_json(
            &data,
            "foo.json",
            "src/foo.rs",
            vec![vec![
                (None, Some(10)),
                (None, Some(15)),
                (None, Some(20)),
            ]],
        );

        let md = dir.join("walkthrough.md");
        fs::write(&md, "# Test\n\n```difft src/foo.rs chunks=0\n```\n").unwrap();

        assert!(run(&md, &data).unwrap());
    }

    #[test]
    fn lines_covers_all_entries() {
        let dir = test_dir("lines_all");
        let data = dir.join("data");
        fs::create_dir(&data).unwrap();

        // Chunk 0: entries at rhs lines 100, 105, 110 (base=100, relative 1,6,11)
        write_chunk_json(
            &data,
            "foo.json",
            "src/foo.rs",
            vec![vec![
                (None, Some(100)),
                (None, Some(105)),
                (None, Some(110)),
            ]],
        );

        let md = dir.join("walkthrough.md");
        fs::write(
            &md,
            "# Test\n\n```difft src/foo.rs chunks=0 lines=1-11\n```\n",
        )
        .unwrap();

        assert!(run(&md, &data).unwrap());
    }

    #[test]
    fn lines_misses_tail_entries() {
        let dir = test_dir("lines_tail");
        let data = dir.join("data");
        fs::create_dir(&data).unwrap();

        // Chunk 0: entries at rhs lines 100, 105, 110, 120, 130
        // base=100, relative positions: 1, 6, 11, 21, 31
        write_chunk_json(
            &data,
            "foo.json",
            "src/foo.rs",
            vec![vec![
                (None, Some(100)),
                (None, Some(105)),
                (None, Some(110)),
                (None, Some(120)),
                (None, Some(130)),
            ]],
        );

        let md = dir.join("walkthrough.md");
        // lines=1-11 covers only entries at 100, 105, 110. Misses 120, 130.
        fs::write(
            &md,
            "# Test\n\n```difft src/foo.rs chunks=0 lines=1-11\n```\n",
        )
        .unwrap();

        assert!(!run(&md, &data).unwrap());
    }

    #[test]
    fn split_lines_covers_all() {
        let dir = test_dir("split");
        let data = dir.join("data");
        fs::create_dir(&data).unwrap();

        write_chunk_json(
            &data,
            "foo.json",
            "src/foo.rs",
            vec![vec![
                (None, Some(100)),
                (None, Some(105)),
                (None, Some(110)),
                (None, Some(120)),
                (None, Some(130)),
            ]],
        );

        let md = dir.join("walkthrough.md");
        fs::write(
            &md,
            "# Test\n\n\
             ```difft src/foo.rs chunks=0 lines=1-11\n```\n\n\
             ```difft src/foo.rs chunks=0 lines=21-31\n```\n",
        )
        .unwrap();

        assert!(run(&md, &data).unwrap());
    }

    #[test]
    fn unreferenced_chunk_still_detected() {
        let dir = test_dir("unref");
        let data = dir.join("data");
        fs::create_dir(&data).unwrap();

        write_chunk_json(
            &data,
            "foo.json",
            "src/foo.rs",
            vec![vec![(None, Some(10))], vec![(None, Some(50))]],
        );

        let md = dir.join("walkthrough.md");
        fs::write(&md, "# Test\n\n```difft src/foo.rs chunks=0\n```\n").unwrap();

        assert!(!run(&md, &data).unwrap());
    }

    #[test]
    fn format_ranges_groups_consecutive() {
        assert_eq!(format_ranges(&[1, 2, 3, 5, 7, 8, 9, 15]), "1-3,5,7-9,15");
        assert_eq!(format_ranges(&[42]), "42");
        assert_eq!(format_ranges(&[1, 3, 5]), "1,3,5");
    }
}
