---
author: Darren Tsung
pr: 
---

# Replace difftastic with git-diff-based chunk generation

Difftastic's structural matching silently excluded lines it considered "moved" rather than "changed." In a workspace_handler.go walkthrough, old lines 30-32 were missing from the rendered diff because difft matched them to identical new-side content and omitted them from its chunks. The `hunk_gap_lines()` gap-filler caught some such cases but not this one.

Rather than patching another edge case in the gap-filling logic, this change replaces difft entirely. The JSON already contained unified diff hunks (from `git diff -U0`) and full file contents. We now generate chunks directly from hunks, guaranteeing every changed line is present. This also removes difftastic as an external dependency.

## 1. New chunk generation from hunks

The core of the change: a new function that converts unified diff hunks into the chunk format the renderer expects. Each hunk becomes one chunk with positionally paired old/new lines. ChangeSpans are left empty since the renderer already has a `find_change_bounds()` fallback for prefix/suffix-based token highlighting.

```difft src/collect.rs chunks=0
          Ok((old_lines, new_lines))
      }
      
   1 +/// Generate chunks from unified diff hunks. Each hunk becomes one chunk with
   2 +/// positionally paired old/new lines. This replaces difftastic's structural
   3 +/// matching, which could silently exclude lines it considers "moved."
   4 +fn generate_chunks_from_hunks(
   5 +    hunks: &[serde_json::Value],
   6 +    old_lines: &[String],
   7 +    new_lines: &[String],
   8 +) -> Vec<serde_json::Value> {
   9 +    let mut chunks = Vec::new();
  10 +
  11 +    for hunk in hunks {
  12 +        let old_start = hunk["old_start"].as_u64().unwrap_or(0);
  13 +        let old_count = hunk["old_count"].as_u64().unwrap_or(0);
  14 +        let new_start = hunk["new_start"].as_u64().unwrap_or(0);
  15 +        let new_count = hunk["new_count"].as_u64().unwrap_or(0);
  16 +
  17 +        // Convert 1-based hunk positions to 0-based indices
  18 +        let old_0 = if old_start > 0 { old_start - 1 } else { 0 };
  19 +        let new_0 = if new_start > 0 { new_start - 1 } else { 0 };
  20 +
  21 +        let mut entries = Vec::new();
  22 +        let paired = old_count.min(new_count);
  23 +
  24 +        // Paired lines (both old and new)
  25 +        for i in 0..paired {
  26 +            let old_idx = old_0 + i;
  27 +            let new_idx = new_0 + i;
  28 +            // Skip pairs where both lines are identical (context within hunk).
  29 +            // The renderer would show these as context anyway, but including
  30 +            // them inflates the chunk with non-changes.
  31 +            let old_text = old_lines.get(old_idx as usize).map(|s| s.as_str()).unwrap_or("");
  32 +            let new_text = new_lines.get(new_idx as usize).map(|s| s.as_str()).unwrap_or("");
  33 +            if old_text == new_text {
  34 +                continue;
  35 +            }
  36 +            entries.push(serde_json::json!({
  37 +                "lhs": { "line_number": old_idx, "changes": [] },
  38 +                "rhs": { "line_number": new_idx, "changes": [] },
  39 +            }));
  40 +        }
  41 +
  42 +        // Extra old lines (removed-only)
  43 +        for i in paired..old_count {
  44 +            entries.push(serde_json::json!({
  45 +                "lhs": { "line_number": old_0 + i, "changes": [] },
  46 +            }));
  47 +        }
  48 +
  49 +        // Extra new lines (added-only)
  50 +        for i in paired..new_count {
  51 +            entries.push(serde_json::json!({
  52 +                "rhs": { "line_number": new_0 + i, "changes": [] },
  53 +            }));
  54 +        }
  55 +
  56 +        if !entries.is_empty() {
  57 +            chunks.push(serde_json::Value::Array(entries));
  58 +        }
  59 +    }
  60 +
  61 +    chunks
  62 +}
  63 +
  64  /// Get unified diff hunk headers for a file. These give exact old/new line mappings.
  65  fn get_diff_hunks(
  66      diff_args: &[String],
```

Identical-content pairs within a hunk are skipped because the renderer detects `old_line == new_line` and renders them as context anyway. Keeping them would inflate chunks with non-changes.

## 2. Simplified collection pipeline

The per-file loop in `run()` previously invoked difft as a subprocess via `GIT_EXTERNAL_DIFF`, parsed its JSON, injected file contents and hunks, and synthesized chunks for added/deleted files. Now it just calls three existing functions and builds the JSON directly.

```difft src/collect.rs chunks=1
          for (status, file_path) in &files {
              eprint!("  {} {} ... ", status, file_path);
      
   1 -        // Run difft for this file via GIT_EXTERNAL_DIFF
   1 +        // Get file contents and unified diff hunks
   2 -        let output = Command::new("git")
   2 +        let (old_lines, new_lines) = get_file_contents(diff_args, file_path)
   3 -            .arg("diff")
   3 +            .unwrap_or_else(|e| {
   4 -            .args(diff_args)
   4 +                eprintln!("(warning: could not get file contents: {})", e);
   5 -            .arg("--")
   5 +                (Vec::new(), Vec::new())
   6 -            .arg(file_path)
   6 +            });
   7 -            .env("GIT_EXTERNAL_DIFF", "difft --display json --color never")
   7 +
   8 -            .env("DFT_UNSTABLE", "yes")
   8 +        let hunks = get_diff_hunks(diff_args, file_path)
   9 -            .output()
   9 +            .unwrap_or_else(|e| {
  10 -            .with_context(|| format!("Failed to run difft for {}", file_path))?;
  10 +                eprintln!("(warning: could not get diff hunks: {})", e);
  11 -
  11 +                Vec::new()
  12 -        let json_str = String::from_utf8_lossy(&output.stdout);
  12 +            });
  14 -        if json_str.trim().is_empty() {
  14 +        if hunks.is_empty() && old_lines.is_empty() && new_lines.is_empty() {
  15 -            eprintln!("(no output, skipping)");
  15 +            eprintln!("(no changes, skipping)");
  16              continue;
  17          }
  18  
```

```difft src/collect.rs chunks=2
                  continue;
              }
      
   1 -        // Parse JSON and inject path, status, and file contents
   1 +        // Generate chunks from hunks (each hunk = one chunk, positional pairing)
   2 -        let mut json: serde_json::Value = serde_json::from_str(&json_str).with_context(|| {
   2 +        let chunks = generate_chunks_from_hunks(&hunks, &old_lines, &new_lines);
   3 -            format!(
   3 +
   4 -                "Failed to parse difft JSON for {}: {}",
   4 +        let diff_status = match status.chars().next() {
   5 -                file_path,
   5 +            Some('A') => "added",
   6 -                &json_str[..json_str.len().min(200)]
   6 +            Some('D') => "deleted",
   7 -            )
   7 +            Some('M') => "changed",
   8 -        })?;
   8 +            Some('R') => "renamed",
   9 -
   9 +            _ => "changed",
  10 -        if let Some(obj) = json.as_object_mut() {
  10 +        };
  11 -            obj.insert(
  11 +
  12 -                "path".to_string(),
  12 +        let json = serde_json::json!({
  13 -                serde_json::Value::String(file_path.clone()),
  13 +            "path": file_path,
  14 -            );
  14 +            "status": diff_status,
  15 -            let difft_status = match status.chars().next() {
  15 +            "language": null,
  16 -                Some('A') => "added",
  16 +            "chunks": chunks,
  17 -                Some('D') => "deleted",
  17 +            "old_lines": old_lines,
  18 -                Some('M') => "changed",
  18 +            "new_lines": new_lines,
  19 -                Some('R') => "renamed",
  19 +            "hunks": hunks,
  20 -                _ => "changed",
  20 +        });
   1 -            };
   1 -            obj.entry("status".to_string())
   1 -                .or_insert_with(|| serde_json::Value::String(difft_status.to_string()));
   1 -
   1 -            // Embed old/new file contents for full-line rendering
   1 -            match get_file_contents(diff_args, file_path) {
   1 -                Ok((old_lines, new_lines)) => {
   1 -                    obj.insert(
   1 -                        "old_lines".to_string(),
   1 -                        serde_json::Value::Array(
   1 -                            old_lines
   1 -                                .into_iter()
   1 -                                .map(serde_json::Value::String)
   1 -                                .collect(),
   1 -                        ),
   1 -                    );
   1 -                    obj.insert(
   1 -                        "new_lines".to_string(),
   1 -                        serde_json::Value::Array(
   1 -                            new_lines
   1 -                                .into_iter()
   1 -                                .map(serde_json::Value::String)
   1 -                                .collect(),
   1 -                        ),
   1 -                    );
   1 -                }
   1 -                Err(e) => {
   1 -                    eprintln!("(warning: could not get file contents: {})", e);
   1 -                }
   1 -            }
   1 -
   1 -            // Embed unified diff hunks for accurate line mapping
   1 -            match get_diff_hunks(diff_args, file_path) {
   1 -                Ok(hunks) => {
   1 -                    obj.insert(
   1 -                        "hunks".to_string(),
   1 -                        serde_json::Value::Array(hunks),
   1 -                    );
   1 -                }
   1 -                Err(e) => {
   1 -                    eprintln!("(warning: could not get diff hunks: {})", e);
   1 -                }
   1 -            }
   1 -        }
   1 -
   1 -        // Ensure chunks field exists (difft omits it for binary/large files).
   1 -        // For deleted/added files with 0 chunks, synthesize a chunk from file
   1 -        // contents so there's something to render.
   1 -        if let Some(obj) = json.as_object_mut() {
   1 -            obj.entry("chunks".to_string())
   1 -                .or_insert_with(|| serde_json::Value::Array(Vec::new()));
   1 -
   1 -            let needs_synthetic = obj.get("chunks")
   1 -                .and_then(|c| c.as_array())
   1 -                .map_or(false, |a| a.is_empty());
   1 -
   1 -            if needs_synthetic {
   1 -                let is_deleted = obj.get("status")
   1 -                    .and_then(|s| s.as_str()) == Some("deleted");
   1 -                let is_added = obj.get("status")
   1 -                    .and_then(|s| s.as_str()) == Some("added");
   1 -                let line_count = if is_deleted {
   1 -                    obj.get("old_lines").and_then(|v| v.as_array()).map(|a| a.len())
   1 -                } else if is_added {
   1 -                    obj.get("new_lines").and_then(|v| v.as_array()).map(|a| a.len())
   1 -                } else {
   1 -                    None
   1 -                };
   1 -                if let Some(count) = line_count {
   1 -                    if count > 0 {
   1 -                        let entries: Vec<serde_json::Value> = (0..count)
   1 -                            .map(|i| {
   1 -                                let side = serde_json::json!({
   1 -                                    "line_number": i,
   1 -                                    "changes": []
   1 -                                });
   1 -                                if is_deleted {
   1 -                                    serde_json::json!({"lhs": side, "rhs": null})
   1 -                                } else {
   1 -                                    serde_json::json!({"lhs": null, "rhs": side})
   1 -                                }
   1 -                            })
   1 -                            .collect();
   1 -                        obj.insert("chunks".to_string(), serde_json::json!([entries]));
   1 -                    }
   1 -                }
   1 -            }
   1 -        }
  21  
  22          let chunk_count = json
  23              .get("chunks")
```

## 3. Render.rs: dead code removal

With hunk-based chunks, every line that git considers changed is already in a chunk entry. This makes several rendering subsystems unnecessary.

### Removed: reorder detection and gap-filling

`fix_reorders()` used LCS to detect structurally-moved lines and re-pair them as context. `hunk_gap_lines()` found lines in unified diff hunks that difft missed and filled them in as gap items. Neither is needed when chunks come from hunks directly.

```difft src/render.rs chunks=0
1770  
1771      rows
1772  }
1773 -/// Compute the Longest Common Subsequence of two string slices.
1773 -/// Returns pairs of (old_idx, new_idx) that form the LCS.
1773 -fn longest_common_subsequence(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
1773 -    let m = old.len();
1773 -    let n = new.len();
1773 -    // dp[i][j] = LCS length for old[..i] vs new[..j]
1773 -    let mut dp = vec![vec![0u32; n + 1]; m + 1];
1773 -
1773 -    for i in 1..=m {
1773 -        for j in 1..=n {
1773 -            if old[i - 1] == new[j - 1] {
1773 -                dp[i][j] = dp[i - 1][j - 1] + 1;
1773 -            } else {
1773 -                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
1773 -            }
1773 -        }
1773 -    }
1773 -
1773 -    // Backtrack to find matched pairs
1773 -    let mut matches = Vec::new();
1773 -    let mut i = m;
1773 -    let mut j = n;
1773 -    while i > 0 && j > 0 {
1773 -        if old[i - 1] == new[j - 1] {
1773 -            matches.push((i - 1, j - 1));
1773 -            i -= 1;
1773 -            j -= 1;
1773 -        } else if dp[i - 1][j] >= dp[i][j - 1] {
1773 -            i -= 1;
1773 -        } else {
1773 -            j -= 1;
1773 -        }
1773 -    }
1773 -    matches.reverse();
1773 -    matches
1773 -}
1773 -
1773 -/// Post-process consolidated rows to detect reordered lines. When a block of
1773 -/// "changed" rows contains lines that appear in both old and new (just at
1773 -/// different positions), re-pair them as context so only the truly moved lines
1773 -/// show as removed/added.
1773 -///
1773 -/// Uses LCS to determine which lines are in the same relative order (context)
1773 -/// vs which were actually moved (shown as remove + add).
1773 -fn fix_reorders<'a>(
1773 -    rows: Vec<DiffRow<'a>>,
1773 -    old_lines: &[String],
1773 -    new_lines: &[String],
1773 -) -> Vec<DiffRow<'a>> {
1773 -    let is_context = |row: &DiffRow| -> bool {
1773 -        if let (Some(lhs), Some(rhs)) = (row.lhs, row.rhs) {
1773 -            let old = old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
1773 -            let new = new_lines.get(rhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
1773 -            old == new
1773 -        } else {
1773 -            false
1773 -        }
1773 -    };
1773 -
1773 -    let mut result = Vec::with_capacity(rows.len());
1773 -    let mut i = 0;
1773 -
1773 -    while i < rows.len() {
1773 -        if is_context(&rows[i]) {
1773 -            result.push(rows[i]);
1773 -            i += 1;
1773 -            continue;
1773 -        }
1773 -
1773 -        // Collect a maximal run of non-context (changed) rows
1773 -        let run_start = i;
1773 -        while i < rows.len() && !is_context(&rows[i]) {
1773 -            i += 1;
1773 -        }
1773 -        let run = &rows[run_start..i];
1773 -
1773 -        // Collect old-side and new-side entries from the run
1773 -        let mut old_entries: Vec<&'a LineSide> = Vec::new();
1773 -        let mut new_entries: Vec<&'a LineSide> = Vec::new();
1773 -
1773 -        for row in run {
1773 -            if let Some(lhs) = row.lhs {
1773 -                old_entries.push(lhs);
1773 -            }
1773 -            if let Some(rhs) = row.rhs {
1773 -                new_entries.push(rhs);
1773 -            }
1773 -        }
1773 -
1773 -        // Need at least 2 entries on each side for reorder detection
1773 -        // (and cap at 200 to avoid expensive LCS on huge runs)
1773 -        if old_entries.len() < 2 || new_entries.len() < 2
1773 -            || old_entries.len() > 200 || new_entries.len() > 200
1773 -        {
1773 -            result.extend_from_slice(run);
1773 -            continue;
1773 -        }
1773 -
1773 -        // Build content vectors for LCS
1773 -        let old_contents: Vec<&str> = old_entries.iter()
1773 -            .map(|s| old_lines.get(s.line_number as usize).map(|l| l.as_str()).unwrap_or(""))
1773 -            .collect();
1773 -        let new_contents: Vec<&str> = new_entries.iter()
1773 -            .map(|s| new_lines.get(s.line_number as usize).map(|l| l.as_str()).unwrap_or(""))
1773 -            .collect();
1773 -
1773 -        let lcs = longest_common_subsequence(&old_contents, &new_contents);
1773 -
1773 -        // Only re-pair when a large portion of lines have exact content
1773 -        // matches (evidence of reordering, not a refactor with coincidental
1773 -        // matches). Require >= 50% of both sides to be matched.
1773 -        let min_side = old_entries.len().min(new_entries.len());
1773 -        if lcs.len() * 2 < min_side {
1773 -            result.extend_from_slice(run);
1773 -            continue;
1773 -        }
1773 -
1773 -        // Build sets of matched indices
1773 -        let mut matched_old = vec![false; old_entries.len()];
1773 -        let mut matched_new = vec![false; new_entries.len()];
1773 -        // Map from new_idx → old_idx for LCS matches
1773 -        let mut new_to_old: HashMap<usize, usize> = HashMap::new();
1773 -        for &(oi, ni) in &lcs {
1773 -            matched_old[oi] = true;
1773 -            matched_new[ni] = true;
1773 -            new_to_old.insert(ni, oi);
1773 -        }
1773 -
1773 -        // Emit unmatched old entries as removals (in old-file order)
1773 -        for (oi, &old_side) in old_entries.iter().enumerate() {
1773 -            if !matched_old[oi] {
1773 -                result.push(DiffRow { lhs: Some(old_side), rhs: None });
1773 -            }
1773 -        }
1773 -
1773 -        // Emit new entries in order: matched as paired context, unmatched as additions
1773 -        for (ni, &new_side) in new_entries.iter().enumerate() {
1773 -            if let Some(&oi) = new_to_old.get(&ni) {
1773 -                result.push(DiffRow { lhs: Some(old_entries[oi]), rhs: Some(new_side) });
1773 -            } else {
1773 -                result.push(DiffRow { lhs: None, rhs: Some(new_side) });
1773 -            }
1773 -        }
1773 -    }
1773 -
1773 -    result
1773 -}
1774  
1775  /// Merge adjacent difft change spans separated by whitespace or punctuation.
1776  /// Returns sorted, non-overlapping (start, end) byte ranges.
```

### Removed: Gap item variants and low-similarity split

Both the text renderer and HTML renderer had `GapPaired`, `GapRemoved`, and `GapAdded` variants for lines found by gap-filling. These enum variants and all their handling code are removed, along with the low-similarity split logic (which only triggered with non-empty ChangeSpans from difft):

```difft src/render.rs chunks=3
2114  fn to_0based(line_1: u64) -> usize {
2115      (line_1 as usize).saturating_sub(1)
2116  }
2117 -/// Find lines within unified diff hunks that overlap a chunk's range but are
2117 -/// not present in difft's structural matching output. Returns paired (old, new),
2117 -/// removed-only, and added-only lists (all 0-based indices).
2117 -///
2117 -/// Pairing uses content similarity (trimmed equality) rather than positional
2117 -/// matching, which avoids the line-ordering bugs from the old approach.
2117 -fn hunk_gap_lines(
2117 -    chunk: &[LineEntry],
2117 -    hunks: &[DiffHunk],
2117 -    old_first: usize,
2117 -    old_last: usize,
2117 -    new_first: usize,
2117 -    new_last: usize,
2117 -    old_lines: &[String],
2117 -    new_lines: &[String],
2117 -) -> (Vec<(usize, usize)>, Vec<usize>, Vec<usize>) {
2117 -    let difft_old: std::collections::HashSet<usize> = chunk.iter()
2117 -        .filter_map(|e| e.lhs.as_ref().map(|s| s.line_number as usize))
2117 -        .collect();
2117 -    let difft_new: std::collections::HashSet<usize> = chunk.iter()
2117 -        .filter_map(|e| e.rhs.as_ref().map(|s| s.line_number as usize))
2117 -        .collect();
2117 -
2117 -    let mut raw_removed = Vec::new();
2117 -    let mut raw_added = Vec::new();
2117 -
2117 -    for h in hunks {
2117 -        let h_old_start = (h.old_start as usize).saturating_sub(1);
2117 -        let h_old_end = h_old_start + h.old_count as usize;
2117 -        let h_new_start = (h.new_start as usize).saturating_sub(1);
2117 -        let h_new_end = h_new_start + h.new_count as usize;
2117 -
2117 -        // Only process hunks that overlap this chunk's line range
2117 -        let overlaps_old = h_old_end > old_first.saturating_sub(1) && h_old_start <= old_last + 1;
2117 -        let overlaps_new = h_new_end > new_first.saturating_sub(1) && h_new_start <= new_last + 1;
2117 -        if !overlaps_old && !overlaps_new { continue; }
2117 -
2117 -        for line_0 in h_old_start..h_old_end {
2117 -            if !difft_old.contains(&line_0) {
2117 -                raw_removed.push(line_0);
2117 -            }
2117 -        }
2117 -        for line_0 in h_new_start..h_new_end {
2117 -            if !difft_new.contains(&line_0) {
2117 -                raw_added.push(line_0);
2117 -            }
2117 -        }
2117 -    }
2117 -
2117 -    // Capture inter-hunk lines: when a chunk spans multiple hunks, unchanged
2117 -    // lines between the hunks are not inside any hunk range. Add them so they
2117 -    // appear as context in the rendered output. Only fill gaps on sides that
2117 -    // have actual difft entries (synthetic ranges from new_to_old_line can
2117 -    // produce spurious gap lines).
2117 -    let hunk_covered_new: std::collections::HashSet<usize> = hunks.iter()
2117 -        .flat_map(|h| {
2117 -            let s = (h.new_start as usize).saturating_sub(1);
2117 -            s..s + h.new_count as usize
2117 -        })
2117 -        .collect();
2117 -    let hunk_covered_old: std::collections::HashSet<usize> = hunks.iter()
2117 -        .flat_map(|h| {
2117 -            let s = (h.old_start as usize).saturating_sub(1);
2117 -            s..s + h.old_count as usize
2117 -        })
2117 -        .collect();
2117 -    if !difft_new.is_empty() {
2117 -        let raw_added_set: std::collections::HashSet<usize> = raw_added.iter().copied().collect();
2117 -        for line_0 in new_first..=new_last {
2117 -            if !difft_new.contains(&line_0) && !hunk_covered_new.contains(&line_0) && !raw_added_set.contains(&line_0) {
2117 -                raw_added.push(line_0);
2117 -            }
2117 -        }
2117 -    }
2117 -    if !difft_old.is_empty() {
2117 -        let raw_removed_set: std::collections::HashSet<usize> = raw_removed.iter().copied().collect();
2117 -        for line_0 in old_first..=old_last {
2117 -            if !difft_old.contains(&line_0) && !hunk_covered_old.contains(&line_0) && !raw_removed_set.contains(&line_0) {
2117 -                raw_removed.push(line_0);
2117 -            }
2117 -        }
2117 -    }
2117 -
2117 -    // Pair gap lines by content similarity. Match criteria (in priority order):
2117 -    // 1. Exact trimmed equality (content is identical, just leading/trailing ws)
2117 -    // 2. Whitespace-normalized equality (only internal whitespace differs, e.g.
2117 -    //    Go alignment: `mu                   sync.RWMutex` == `mu         sync.RWMutex`)
2117 -    // 3. One line's trimmed content starts with the other's (structural match,
2117 -    //    e.g. `cbOrOpts?:` matches `cbOrOpts?: Callback<...> | ...`)
2117 -    let normalize_ws = |s: &str| -> String {
2117 -        s.split_whitespace().collect::<Vec<_>>().join(" ")
2117 -    };
2117 -    let mut paired = Vec::new();
2117 -    let mut used_added: std::collections::HashSet<usize> = std::collections::HashSet::new();
2117 -    let mut unmatched_removed = Vec::new();
2117 -
2117 -    for &old_0 in &raw_removed {
2117 -        let old_content = old_lines.get(old_0).map(|s| s.trim()).unwrap_or("");
2117 -        if old_content.is_empty() {
2117 -            unmatched_removed.push(old_0);
2117 -            continue;
2117 -        }
2117 -        let old_normalized = normalize_ws(old_content);
2117 -        // First try exact trimmed match, then whitespace-normalized, then prefix
2117 -        let matched = raw_added.iter()
2117 -            .find(|&&new_0| {
2117 -                !used_added.contains(&new_0) && {
2117 -                    let new_content = new_lines.get(new_0).map(|s| s.trim()).unwrap_or("");
2117 -                    old_content == new_content
2117 -                }
2117 -            })
2117 -            .or_else(|| raw_added.iter()
2117 -                .find(|&&new_0| {
2117 -                    !used_added.contains(&new_0) && {
2117 -                        let new_content = new_lines.get(new_0).map(|s| s.trim()).unwrap_or("");
2117 -                        normalize_ws(new_content) == old_normalized
2117 -                    }
2117 -                }))
2117 -            .or_else(|| raw_added.iter()
2117 -                .find(|&&new_0| {
2117 -                    !used_added.contains(&new_0) && {
2117 -                        let new_content = new_lines.get(new_0).map(|s| s.trim()).unwrap_or("");
2117 -                        !new_content.is_empty()
2117 -                            && (old_content.starts_with(new_content) || new_content.starts_with(old_content))
2117 -                    }
2117 -                }))
2117 -            .copied();
2117 -        if let Some(new_0) = matched {
2117 -            paired.push((old_0, new_0));
2117 -            used_added.insert(new_0);
2117 -        } else {
2117 -            unmatched_removed.push(old_0);
2117 -        }
2117 -    }
2117 -
2117 -    let unmatched_added: Vec<usize> = raw_added.into_iter()
2117 -        .filter(|n| !used_added.contains(n))
2117 -        .collect();
2117 -
2117 -    (paired, unmatched_removed, unmatched_added)
2117 -}
2117 -
2118  /// Produce a unified-diff-style text representation of selected chunks.
2119  /// Uses the same chunk processing logic as HTML rendering (context lines, hunk gap
2120  /// filling, consolidation) but outputs plain text with ` `/`-`/`+` prefixes.
```

### GitHub-style token highlighting

Paired lines now use full-line background (light red/green wash) with optional darker token-level highlights. The highlights only appear for pure insertions or deletions within a line, where the shared prefix+suffix contains alphabetic text:

```difft src/render.rs chunks=1,2
                      return html;
                  }
              } else {
   1 -            // No difft spans: fall back to prefix/suffix comparison
   1 +            // Full-line background, with darker token highlights only when:
   2 +            // 1. One side's changed region is empty (pure insertion/deletion)
   3 +            // 2. The shared prefix+suffix has actual alphabetic content (not
   4 +            //    just matching whitespace/punctuation/brackets)
   5 +            is_full_line = true;
   6              let (old_cs, old_ce, new_cs, new_ce) = find_change_bounds(old_line, new_line);
   7              let old_changed = old_ce - old_cs;
   8              let new_changed = new_ce - new_cs;
   7 -            is_full_line = false;
   7 +            let old_changed = old_ce - old_cs;
   8 -            old_code = insert_diff_highlight(old_highlighted, old_cs, old_ce, "hl-del");
   8 +            let new_changed = new_ce - new_cs;
   9 -            new_code = insert_diff_highlight(new_highlighted, new_cs, new_ce, "hl-add");
   9 +            let shared_has_alpha = old_line[..old_cs].bytes()
  10 +                .chain(old_line[old_ce..].bytes())
  11 +                .any(|b| b.is_ascii_alphabetic());
  12 +            if (old_changed == 0 || new_changed == 0) && shared_has_alpha {
  13 +                old_code = insert_diff_highlight(old_highlighted, old_cs, old_ce, "hl-del");
  14 +                new_code = insert_diff_highlight(new_highlighted, new_cs, new_ce, "hl-add");
  15 +            } else {
  16 +                old_code = old_highlighted.to_string();
  17 +                new_code = new_highlighted.to_string();
  18 +            }
  19          }
  20  
  21          let row_class = if is_full_line { "line-paired-full" } else { "line-paired" };
```

### Simplified test helper

The fixture test helper removed its `hunk_gap_lines` calls and gap-context checks:

```difft src/render.rs chunks=4,5,6
                  out.push_str(&fmt_line(" ", new_idx, line));
              }
      
   1 -        // Build unified item list (same logic as HTML renderer)
   1 +        // Build unified item list from consolidated difft rows.
   1 -        #[derive(Clone, Copy)]
   1 -        enum TextItem<'a> {
   1 -            DifftRow(&'a DiffRow<'a>),
   1 -            GapRemoved(usize),
   1 -            GapAdded(usize),
   1 -            GapPaired(usize, usize),
   1 -        }
   1 -
   2          let rows = consolidate_chunk(chunk);
   3          let mut items: Vec<(u64, &DiffRow)> = Vec::new();
   4          let mut prev_rhs_t: Option<u64> = None;
   3 -        let rows = fix_reorders(rows, &difft.old_lines, &difft.new_lines);
   3 +        let mut items: Vec<(u64, &DiffRow)> = Vec::new();
   3 -        let mut items: Vec<(u64, TextItem)> = Vec::new();
   4          let mut prev_rhs_t: Option<u64> = None;
   5          let mut removed_seq_t: u64 = 0;
   6          for row in &rows {
   7              let key = if let Some(rhs) = row.rhs {
   8                  prev_rhs_t = Some(rhs.line_number);
   9                  removed_seq_t = 0;
  10                  rhs.line_number * 3
  11              } else {
  12                  removed_seq_t += 1;
  13                  prev_rhs_t.map_or(removed_seq_t, |p| p * 3 + removed_seq_t)
  14              };
  15 -            items.push((key, TextItem::DifftRow(row)));
  15 +            items.push((key, row));
  15 -        }
  15 -
  15 -        let (gap_paired, gap_removed, gap_added) = hunk_gap_lines(
  15 -            chunk, hunks, old_first, old_last, new_first, new_last,
  15 -            &difft.old_lines, &difft.new_lines,
  15 -        );
  15 -        for &(old_0, new_0) in &gap_paired {
  15 -            items.push((new_0 as u64 * KEY_SPACE, TextItem::GapPaired(old_0, new_0)));
  15 -            old_first = old_first.min(old_0); old_last = old_last.max(old_0);
  15 -            new_first = new_first.min(new_0); new_last = new_last.max(new_0);
  15 -        }
  15 -        for &old_0 in &gap_removed {
  15 -            let nearest_key = items.iter()
  15 -                .filter_map(|&(k, ref item)| match item {
  15 -                    TextItem::DifftRow(row) => row.lhs.map(|s| {
  15 -                        let dist = (s.line_number as i64 - old_0 as i64).unsigned_abs();
  15 -                        (dist, k)
  15 -                    }),
  15 -                    _ => None,
  15 -                })
  15 -                .min_by_key(|&(dist, _)| dist)
  15 -                .map(|(_, k)| k);
  15 -            items.push((nearest_key.unwrap_or(0), TextItem::GapRemoved(old_0)));
  15 -            old_first = old_first.min(old_0); old_last = old_last.max(old_0);
  15 -        }
  15 -        for &new_0 in &gap_added {
  15 -            items.push((new_0 as u64 * KEY_SPACE + KEY_SPACE - 1, TextItem::GapAdded(new_0)));
  15 -            new_first = new_first.min(new_0); new_last = new_last.max(new_0);
  16          }
  17          items.sort_by(|a, b| {
  18              a.0.cmp(&b.0).then_with(|| {
```

### Remaining render.rs simplifications

The rest of the render.rs changes are mechanical: removing Gap* variant handling from sorting, filtering, item collection, range computation, and rendering throughout both the text and HTML pipelines:

```difft src/render.rs chunks=7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53
              }
              items.sort_by(|a, b| {
                  a.0.cmp(&b.0).then_with(|| {
   1 -                let old_a = match &a.1 {
   1 +                let old_a = a.1.lhs.map(|s| s.line_number);
   2 -                    TextItem::DifftRow(r) => r.lhs.map(|s| s.line_number),
   2 +                let old_b = b.1.lhs.map(|s| s.line_number);
   1 -                    TextItem::GapRemoved(o) | TextItem::GapPaired(o, _) => Some(*o as u64),
   1 -                    TextItem::GapAdded(_) => None,
   1 -                };
   1 -                let old_b = match &b.1 {
   1 -                    TextItem::DifftRow(r) => r.lhs.map(|s| s.line_number),
   1 -                    TextItem::GapRemoved(o) | TextItem::GapPaired(o, _) => Some(*o as u64),
   1 -                    TextItem::GapAdded(_) => None,
   1 -                };
   3                  old_a.cmp(&old_b)
   4              })
   5          });
   6 -        // Remove gap lines that violate old-side ordering
   6 -        let get_old_t = |item: &TextItem| -> Option<u64> {
   6 -            match item {
   6 -                TextItem::DifftRow(r) => r.lhs.map(|s| s.line_number),
   6 -                TextItem::GapRemoved(o) | TextItem::GapPaired(o, _) => Some(*o as u64),
   6 -                TextItem::GapAdded(_) => None,
   6 -            }
   6 -        };
   6 -        let mut max_old_t: Option<u64> = None;
   6 -        items.retain(|&(_, ref item)| {
   6 -            if let Some(o) = get_old_t(item) {
   6 -                if let Some(m) = max_old_t {
   6 -                    if o < m {
   6 -                        return !matches!(item,
   6 -                            TextItem::GapRemoved(_) | TextItem::GapAdded(_) | TextItem::GapPaired(_, _));
   6 -                    }
   6 -                }
   6 -                max_old_t = Some(o);
   6 -            }
   6 -            true
   6 -        });
   6 -
   7          // Collect ALL item line numbers before filtering so filtered-out
   8          // changed lines don't become context rows.
   9          let mut item_old_lines_t: std::collections::HashSet<usize> = std::collections::HashSet::new();
  10          let mut item_new_lines_t: std::collections::HashSet<usize> = std::collections::HashSet::new();
  11 -        for &(_, ref item) in &items {
  11 +        for &(_, row) in &items {
  12 -            match item {
  12 +            if let Some(lhs) = row.lhs { item_old_lines_t.insert(lhs.line_number as usize); }
  13 -                TextItem::DifftRow(row) => {
  13 +            if let Some(rhs) = row.rhs { item_new_lines_t.insert(rhs.line_number as usize); }
  11 -                    if let Some(lhs) = row.lhs { item_old_lines_t.insert(lhs.line_number as usize); }
  11 -                    if let Some(rhs) = row.rhs { item_new_lines_t.insert(rhs.line_number as usize); }
  11 -                }
  11 -                TextItem::GapRemoved(o) => { item_old_lines_t.insert(*o); }
  11 -                TextItem::GapAdded(n) => { item_new_lines_t.insert(*n); }
  11 -                TextItem::GapPaired(o, n) => { item_old_lines_t.insert(*o); item_new_lines_t.insert(*n); }
  11 -            }
  14          }
  15  
  16          // Apply line filter after collecting skip sets.
  17          if let Some((filter_start, filter_end)) = line_filter {
  18 -            items.retain(|&(_, ref item)| {
  18 +            items.retain(|&(_, row)| {
  19 -                let n = match item {
  19 +                let n = row.rhs.map(|s| s.line_number as usize)
  20 -                    TextItem::DifftRow(row) => row.rhs.map(|s| s.line_number as usize)
  20 +                    .or(row.lhs.map(|s| s.line_number as usize));
  18 -                        .or(row.lhs.map(|s| s.line_number as usize)),
  18 -                    TextItem::GapRemoved(o) => Some(*o),
  18 -                    TextItem::GapAdded(n) => Some(*n),
  18 -                    TextItem::GapPaired(_, n) => Some(*n),
  18 -                };
  21                  n.map_or(false, |n| n >= filter_start && n <= filter_end)
  22              });
  23              new_first = usize::MAX;
  24              new_last = 0;
  25              old_first = usize::MAX;
  26              old_last = 0;
  27 -            for &(_, ref item) in &items {
  27 +            for &(_, row) in &items {
  28 -                match item {
  28 +                if let Some(lhs) = row.lhs { old_first = old_first.min(lhs.line_number as usize); old_last = old_last.max(lhs.line_number as usize); }
  29 -                    TextItem::DifftRow(row) => {
  29 +                if let Some(rhs) = row.rhs { new_first = new_first.min(rhs.line_number as usize); new_last = new_last.max(rhs.line_number as usize); }
  27 -                        if let Some(lhs) = row.lhs { old_first = old_first.min(lhs.line_number as usize); old_last = old_last.max(lhs.line_number as usize); }
  27 -                        if let Some(rhs) = row.rhs { new_first = new_first.min(rhs.line_number as usize); new_last = new_last.max(rhs.line_number as usize); }
  27 -                    }
  27 -                    TextItem::GapRemoved(o) => { old_first = old_first.min(*o); old_last = old_last.max(*o); }
  27 -                    TextItem::GapAdded(n) => { new_first = new_first.min(*n); new_last = new_last.max(*n); }
  27 -                    TextItem::GapPaired(o, n) => {
  27 -                        old_first = old_first.min(*o); old_last = old_last.max(*o);
  27 -                        new_first = new_first.min(*n); new_last = new_last.max(*n);
  27 -                    }
  27 -                }
  30              }
  31              if items.is_empty() { continue; }
  32  
  33              if old_first == usize::MAX && new_first != usize::MAX {
  34                  old_last = to_0based(new_to_old_line(new_last as u64 + 1, hunks));
  35              } else if new_first == usize::MAX && old_first != usize::MAX {
  36                  new_last = to_0based(old_to_new_line(old_last as u64 + 1, hunks));
  37              }
  38          }
  33 -                old_first = to_0based(new_to_old_line(new_first as u64 + 1, hunks));
  34                  old_last = to_0based(new_to_old_line(new_last as u64 + 1, hunks));
  35              } else if new_first == usize::MAX && old_first != usize::MAX {
  36                  new_last = to_0based(old_to_new_line(old_last as u64 + 1, hunks));
  35 -                new_first = to_0based(old_to_new_line(old_first as u64 + 1, hunks));
  36                  new_last = to_0based(old_to_new_line(old_last as u64 + 1, hunks));
  37              }
  38          }
  39  
  40 -        let mut prev_old: Option<usize> = None;
  40 +        for &(_, row) in &items {
  40 -        let mut prev_new: Option<usize> = None;
  40 -
  40 -        for &(_, ref item) in &items {
  40 -            let (cur_old, cur_new) = match item {
  40 -                TextItem::DifftRow(row) => (
  40 -                    row.lhs.map(|s| s.line_number as usize),
  40 -                    row.rhs.map(|s| s.line_number as usize),
  40 -                ),
  40 -                TextItem::GapRemoved(o) => (Some(*o), None),
  40 -                TextItem::GapAdded(n) => (None, Some(*n)),
  40 -                TextItem::GapPaired(o, n) => (Some(*o), Some(*n)),
  40 -            };
  40 -
  41              // Render the item with relative line numbers
  42              match (row.lhs, row.rhs) {
  43                  (Some(lhs), Some(rhs)) => {
  42 -            match item {
  42 +            match (row.lhs, row.rhs) {
  43 -                TextItem::DifftRow(row) => {
  43 +                (Some(lhs), Some(rhs)) => {
  44 -                    match (row.lhs, row.rhs) {
  44 +                    let n = rhs.line_number as usize;
  45 -                        (Some(lhs), Some(rhs)) => {
  45 +                    let old_line = difft.old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
  46 -                            let n = rhs.line_number as usize;
  46 +                    let new_line = difft.new_lines.get(n).map(|s| s.as_str()).unwrap_or("");
  47 -                            let old_line = difft.old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
  47 +                    if old_line == new_line {
  48 -                            let new_line = difft.new_lines.get(n).map(|s| s.as_str()).unwrap_or("");
  48 +                        out.push_str(&fmt_line(" ", n, new_line));
  49 -                            if old_line == new_line {
  49 +                    } else {
  50 -                                out.push_str(&fmt_line(" ", n, new_line));
  50 +                        out.push_str(&fmt_line("-", n, old_line));
  51 -                            } else {
  51 +                        out.push_str(&fmt_line("+", n, new_line));
  42 -                                out.push_str(&fmt_line("-", n, old_line));
  42 -                                out.push_str(&fmt_line("+", n, new_line));
  42 -                            }
  42 -                        }
  42 -                        (Some(lhs), None) => {
  42 -                            let n = to_0based(old_to_new_line(lhs.line_number + 1, hunks));
  42 -                            let old_line = difft.old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
  42 -                            out.push_str(&fmt_line("-", n, old_line));
  42 -                        }
  42 -                        (None, Some(rhs)) => {
  42 -                            let n = rhs.line_number as usize;
  42 -                            let new_line = difft.new_lines.get(n).map(|s| s.as_str()).unwrap_or("");
  42 -                            out.push_str(&fmt_line("+", n, new_line));
  42 -                        }
  42 -                        (None, None) => {}
  52                      }
  53                  }
  54                  (Some(lhs), None) => {
  54 -                TextItem::GapRemoved(old_0) => {
  54 +                (Some(lhs), None) => {
  55 -                    let n = to_0based(old_to_new_line(*old_0 as u64 + 1, hunks));
  55 +                    let n = to_0based(old_to_new_line(lhs.line_number + 1, hunks));
  56 -                    let line = difft.old_lines.get(*old_0).map(|s| s.as_str()).unwrap_or("");
  56 +                    let old_line = difft.old_lines.get(lhs.line_number as usize).map(|s| s.as_str()).unwrap_or("");
  57 -                    out.push_str(&fmt_line("-", n, line));
  57 +                    out.push_str(&fmt_line("-", n, old_line));
  54 -                }
  54 -                TextItem::GapAdded(new_0) => {
  54 -                    let line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
  54 -                    out.push_str(&fmt_line("+", *new_0, line));
  58                  }
  59                  (None, Some(rhs)) => {
  60                      let n = rhs.line_number as usize;
  59 -                TextItem::GapPaired(_old_0, new_0) => {
  59 +                (None, Some(rhs)) => {
  60 -                    // Content-matched, just repositioned: render as context
  60 +                    let n = rhs.line_number as usize;
  61 -                    let new_line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
  61 +                    let new_line = difft.new_lines.get(n).map(|s| s.as_str()).unwrap_or("");
  62 -                    out.push_str(&fmt_line(" ", *new_0, new_line));
  62 +                    out.push_str(&fmt_line("+", n, new_line));
  63                  }
  64                  (None, None) => {}
  65              }
  64 +                (None, None) => {}
  65              }
  66          }
  67  
  65 -
  65 -            if let Some(o) = cur_old { prev_old = Some(o); }
  65 -            if let Some(n) = cur_new { prev_new = Some(n); }
  66          }
  67  
  68          // Context lines after
  ...
 403              }
 404          };
 405  
 406 -        // Build a list of render items from difft entries, supplemented with
 406 +        // Build a list of render items from consolidated difft rows.
 406 -        // hunk gap lines (lines git considers changed but difft didn't flag).
 406 -        // Gap lines are added as individual removed/added rows (no pairing).
 406 -        #[derive(Clone, Copy)]
 406 -        enum RenderItem<'a> {
 406 -            DifftRow(&'a DiffRow<'a>),
 406 -            GapRemoved(usize),  // 0-based old line
 406 -            GapAdded(usize),    // 0-based new line
 406 -            GapPaired(usize, usize), // (old_0, new_0)
 406 -        }
 406 -
 407          let rows = consolidate_chunk(chunk);
 408  
 409          let mut items: Vec<(u64, &DiffRow)> = Vec::new();
 407 -        let rows = fix_reorders(rows, &difft.old_lines, &difft.new_lines);
 407 -
 407 -        // Compute gap lines first so we can use gap-paired positions to
 407 -        // assign correct sort keys for removed-only difft entries that
 407 -        // have no preceding rhs entry.
 407 -        let (gap_paired, gap_removed, gap_added) = hunk_gap_lines(
 407 -            chunk, hunks, old_first, old_last, new_first, new_last,
 407 -            &difft.old_lines, &difft.new_lines,
 407 -        );
 408  
 409          let mut items: Vec<(u64, &DiffRow)> = Vec::new();
 410          let mut prev_rhs: Option<u64> = None;
 409 -        // Sort entries by new-file position with KEY_SPACE spacing.
 409 +        let mut items: Vec<(u64, &DiffRow)> = Vec::new();
 409 -        // For removed-only entries with no preceding rhs, use gap-paired
 409 -        // items to find the correct new-file position so the entry sorts
 409 -        // correctly relative to gap-paired context lines.
 409 -        let mut items: Vec<(u64, RenderItem)> = Vec::new();
 410          let mut prev_rhs: Option<u64> = None;
 411          let mut removed_seq: u64 = 0;
 412          for row in &rows {
 413 -                // Only anchor prev_rhs from truly paired entries (both sides).
 413 -                // Added-only entries shouldn't position subsequent removed entries.
 414                  if row.lhs.is_some() {
 415                      prev_rhs = Some(rhs.line_number);
 416                  }
  ...
 418                  rhs.line_number * KEY_SPACE
 419              } else if let Some(p) = prev_rhs {
 420                  removed_seq += 1;
 421 -            } else if let Some(lhs) = row.lhs {
 421 -                // No prev_rhs: find the gap-paired item just before this
 421 -                // entry in old-line order, but only from the same hunk.
 421 -                removed_seq += 1;
 421 -                let old_1 = lhs.line_number + 1; // 1-based
 421 -                // Find the hunk containing this old line
 421 -                let containing_hunk = hunks.iter().find(|h|
 421 -                    h.old_count > 0
 421 -                        && old_1 >= h.old_start
 421 -                        && old_1 < h.old_start + h.old_count
 421 -                );
 421 -                let old_0 = lhs.line_number as usize;
 421 -                if let Some(h) = containing_hunk {
 421 -                    let h_old_start = (h.old_start as usize).saturating_sub(1);
 421 -                    let h_old_end = h_old_start + h.old_count as usize;
 421 -                    if let Some(&(_, gn)) = gap_paired.iter()
 421 -                        .filter(|&&(go, _)| go < old_0 && go >= h_old_start && go < h_old_end)
 421 -                        .max_by_key(|&&(go, _)| go)
 421 -                    {
 421 -                        gn as u64 * KEY_SPACE + KEY_SPACE / 2 + removed_seq
 421 -                    } else {
 421 -                        removed_seq
 421 -                    }
 421 -                } else {
 421 -                    removed_seq
 421 -                }
 422              } else {
 423                  removed_seq += 1;
 424                  removed_seq
 425              };
 426 -            items.push((key, RenderItem::DifftRow(row)));
 426 +            items.push((key, row));
 426 -        }
 426 -
 426 -        for &(old_0, new_0) in &gap_paired {
 426 -            items.push((new_0 as u64 * KEY_SPACE, RenderItem::GapPaired(old_0, new_0)));
 426 -            old_first = old_first.min(old_0);
 426 -            old_last = old_last.max(old_0);
 426 -            new_first = new_first.min(new_0);
 426 -            new_last = new_last.max(new_0);
 426 -        }
 426 -        for &old_0 in &gap_removed {
 426 -            // Use same key as nearby difft removed entries so they interleave
 426 -            // by old-line via the tiebreaker. Find the nearest difft entry.
 426 -            let nearest_key = items.iter()
 426 -                .filter_map(|&(k, ref item)| match item {
 426 -                    RenderItem::DifftRow(row) => row.lhs.map(|s| {
 426 -                        let dist = (s.line_number as i64 - old_0 as i64).unsigned_abs();
 426 -                        (dist, k)
 426 -                    }),
 426 -                    _ => None,
 426 -                })
 426 -                .min_by_key(|&(dist, _)| dist)
 426 -                .map(|(_, k)| k);
 426 -            let key = nearest_key.unwrap_or(0);
 426 -            items.push((key, RenderItem::GapRemoved(old_0)));
 426 -            old_first = old_first.min(old_0);
 426 -            old_last = old_last.max(old_0);
 426 -        }
 426 -        for &new_0 in &gap_added {
 426 -            items.push((new_0 as u64 * KEY_SPACE + KEY_SPACE - 1, RenderItem::GapAdded(new_0)));
 426 -            new_first = new_first.min(new_0);
 426 -            new_last = new_last.max(new_0);
 427          }
 428  
 429          // Sort by key, tiebreak by old-line number
 430          items.sort_by(|a, b| {
 431              a.0.cmp(&b.0).then_with(|| {
 432 -                let old_a = match &a.1 {
 432 +                let old_a = a.1.lhs.map(|s| s.line_number);
 433 -                    RenderItem::DifftRow(r) => r.lhs.map(|s| s.line_number),
 433 +                let old_b = b.1.lhs.map(|s| s.line_number);
 432 -                    RenderItem::GapRemoved(o) | RenderItem::GapPaired(o, _) => Some(*o as u64),
 432 -                    RenderItem::GapAdded(_) => None,
 432 -                };
 432 -                let old_b = match &b.1 {
 432 -                    RenderItem::DifftRow(r) => r.lhs.map(|s| s.line_number),
 432 -                    RenderItem::GapRemoved(o) | RenderItem::GapPaired(o, _) => Some(*o as u64),
 432 -                    RenderItem::GapAdded(_) => None,
 432 -                };
 434                  old_a.cmp(&old_b)
 435              })
 436          });
 437  
 438 -        // Un-pair low-similarity entries. Difft sometimes pairs lines by
 438 +        // Collect ALL item line numbers before filtering, so
 438 -        // position even when content is almost entirely different. Split
 438 -        // these into adjacent removed + added items at the same sort key.
 438 -        let mut split_rows: Vec<DiffRow> = Vec::new();
 438 -        // Owned LineSides with cleared changes for split rows, so they
 438 -        // render as full-line removals/additions instead of partial highlights.
 438 -        let mut split_sides: Vec<LineSide> = Vec::new();
 438 -        let is_low_similarity = |row: &DiffRow| -> bool {
 438 -            if let (Some(lhs), Some(rhs)) = (row.lhs, row.rhs) {
 438 -                // Skip entries where content is identical (difft includes
 438 -                // unchanged lines in chunks when the surrounding structure
 438 -                // changed, e.g. a JSDoc comment grew longer).
 438 -                let old_line = difft.old_lines.get(lhs.line_number as usize);
 438 -                let new_line = difft.new_lines.get(rhs.line_number as usize);
 438 -                if old_line == new_line { return false; }
 438 -
 438 -                let old_text = old_line.map(|s| s.trim().len()).unwrap_or(0);
 438 -                let new_text = new_line.map(|s| s.trim().len()).unwrap_or(0);
 438 -                let old_changed: usize = lhs.changes.iter()
 438 -                    .map(|c| c.end.saturating_sub(c.start))
 438 -                    .sum();
 438 -                let new_changed: usize = rhs.changes.iter()
 438 -                    .map(|c| c.end.saturating_sub(c.start))
 438 -                    .sum();
 438 -                let old_pct = if old_text > 0 { old_changed * 100 / old_text } else { 0 };
 438 -                let new_pct = if new_text > 0 { new_changed * 100 / new_text } else { 0 };
 438 -                old_pct > 70 && new_pct > 70
 438 -            } else {
 438 -                false
 438 -            }
 438 -        };
 438 -        // First pass: identify which items need splitting and create the
 438 -        // split DiffRows (need stable references before building new items).
 438 -        let split_indices: Vec<usize> = items.iter().enumerate()
 438 -            .filter_map(|(i, (_, item))| {
 438 -                if let RenderItem::DifftRow(row) = item {
 438 -                    if is_low_similarity(row) { return Some(i); }
 438 -                }
 438 -                None
 438 -            })
 438 -            .collect();
 438 -        for &idx in &split_indices {
 438 -            if let RenderItem::DifftRow(row) = &items[idx].1 {
 438 -                // Create owned LineSides with full-line change spans so
 438 -                // the rows render with full-line background (no token
 438 -                // highlights implying a semantic match).
 438 -                if let Some(lhs) = row.lhs {
 438 -                    let len = difft.old_lines.get(lhs.line_number as usize)
 438 -                        .map(|s| s.len()).unwrap_or(0);
 438 -                    split_sides.push(LineSide {
 438 -                        line_number: lhs.line_number,
 438 -                        changes: vec![crate::difft_json::ChangeSpan { content: String::new(), start: 0, end: len, highlight: String::new() }],
 438 -                    });
 438 -                }
 438 -                if let Some(rhs) = row.rhs {
 438 -                    let len = difft.new_lines.get(rhs.line_number as usize)
 438 -                        .map(|s| s.len()).unwrap_or(0);
 438 -                    split_sides.push(LineSide {
 438 -                        line_number: rhs.line_number,
 438 -                        changes: vec![crate::difft_json::ChangeSpan { content: String::new(), start: 0, end: len, highlight: String::new() }],
 438 -                    });
 438 -                }
 438 -            }
 438 -        }
 438 -        // Build split DiffRows referencing the owned sides.
 438 -        {
 438 -            let mut side_idx = 0;
 438 -            for _ in &split_indices {
 438 -                split_rows.push(DiffRow { lhs: Some(&split_sides[side_idx]), rhs: None });
 438 -                split_rows.push(DiffRow { lhs: None, rhs: Some(&split_sides[side_idx + 1]) });
 438 -                side_idx += 2;
 438 -            }
 438 -        }
 438 -        // Second pass: rebuild items. Re-pair the split entries
 438 -        // positionally (old line N beside new line N) so they share a
 438 -        // row in side-by-side view. Changes are cleared so they render
 438 -        // with full-row background, not token highlights.
 438 -        if !split_indices.is_empty() {
 438 -            // Rebuild split_rows as re-paired: combine each removed/added
 438 -            // pair into a single row with both sides but no change spans.
 438 -            let mut repaired: Vec<DiffRow> = Vec::new();
 438 -            for si in (0..split_rows.len()).step_by(2) {
 438 -                repaired.push(DiffRow {
 438 -                    lhs: split_sides.get(si).map(|s| s as &LineSide),
 438 -                    rhs: split_sides.get(si + 1).map(|s| s as &LineSide),
 438 -                });
 438 -            }
 438 -            split_rows = repaired;
 438 -
 438 -            let split_set: std::collections::HashSet<usize> = split_indices.into_iter().collect();
 438 -            let mut new_items: Vec<(u64, RenderItem)> = Vec::new();
 438 -            let mut repaired_idx = 0;
 438 -            for (i, (key, item)) in items.into_iter().enumerate() {
 438 -                if split_set.contains(&i) {
 438 -                    new_items.push((key, RenderItem::DifftRow(&split_rows[repaired_idx])));
 438 -                    repaired_idx += 1;
 438 -                } else {
 438 -                    new_items.push((key, item));
 438 -                }
 438 -            }
 438 -            items = new_items;
 438 -        }
 438 -
 438 -        // Enforce old-side monotonicity. When old and new sides have
 438 -        // different ordering (refactored code), the new-side-based sort can
 438 -        // scatter old-side line numbers. Stable-sort items so old-side line
 438 -        // numbers are non-decreasing while preserving new-side order for
 438 -        // items without old-side lines (added-only).
 438 -        let get_old = |item: &RenderItem| -> Option<u64> {
 438 -            match item {
 438 -                RenderItem::DifftRow(r) => r.lhs.map(|s| s.line_number),
 438 -                RenderItem::GapRemoved(o) | RenderItem::GapPaired(o, _) => Some(*o as u64),
 438 -                RenderItem::GapAdded(_) => None,
 438 -            }
 438 -        };
 438 -
 438 -        // First pass: remove gap lines that violate old-side ordering.
 438 -        let mut max_old: Option<u64> = None;
 438 -        items.retain(|&(_, ref item)| {
 438 -            if let Some(o) = get_old(item) {
 438 -                if let Some(m) = max_old {
 438 -                    if o < m {
 438 -                        return !matches!(item,
 438 -                            RenderItem::GapRemoved(_) | RenderItem::GapAdded(_) | RenderItem::GapPaired(_, _));
 438 -                    }
 438 -                }
 438 -                max_old = Some(o);
 438 -            }
 438 -            true
 438 -        });
 438 -
 438 -
 438 -        // If the chunk has gap-paired lines AND the difft entries have both
 438 -        // sides, use side-by-side rendering. Don't override for purely
 438 -        // one-sided chunks (all removed or all added) where gap-paired lines
 438 -        // are just whitespace realignment from the removal/addition.
 438 -        let has_difft_lhs = rows.iter().any(|r| r.lhs.is_some());
 438 -        let has_difft_rhs = rows.iter().any(|r| r.rhs.is_some());
 438 -        let (single, layout) = if single && !gap_paired.is_empty() && has_difft_lhs && has_difft_rhs {
 438 -            (false, DiffLayout::SideBySide)
 438 -        } else {
 438 -            (single, layout)
 438 -        };
 438 -
 438 -        // Collect ALL item line numbers before filtering, so gap lines and
 439          // filtered-out changed lines don't become context rows.
 440          let mut item_old_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
 441          let mut item_new_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
 442 -        for &(_, ref item) in &items {
 442 +        for &(_, row) in &items {
 443 -            match item {
 443 +            if let Some(lhs) = row.lhs { item_old_lines.insert(lhs.line_number as usize); }
 444 -                RenderItem::DifftRow(row) => {
 444 +            if let Some(rhs) = row.rhs { item_new_lines.insert(rhs.line_number as usize); }
 442 -                    if let Some(lhs) = row.lhs { item_old_lines.insert(lhs.line_number as usize); }
 442 -                    if let Some(rhs) = row.rhs { item_new_lines.insert(rhs.line_number as usize); }
 442 -                }
 442 -                RenderItem::GapRemoved(o) => { item_old_lines.insert(*o); }
 442 -                RenderItem::GapAdded(n) => { item_new_lines.insert(*n); }
 442 -                RenderItem::GapPaired(o, n) => { item_old_lines.insert(*o); item_new_lines.insert(*n); }
 442 -            }
 445          }
 446  
 447          // Apply line filter: remove items outside the range but keep their
 448          // line numbers in the skip sets so they don't become context rows.
 449          if let Some((filter_start, filter_end)) = line_filter {
 450 -            items.retain(|&(_, ref item)| {
 450 +            items.retain(|&(_, row)| {
 451 -                let n = match item {
 451 +                let n = row.rhs.map(|s| s.line_number as usize)
 452 -                    RenderItem::DifftRow(row) => row.rhs.map(|s| s.line_number as usize)
 452 +                    .or(row.lhs.map(|s| s.line_number as usize));
 450 -                        .or(row.lhs.map(|s| s.line_number as usize)),
 450 -                    RenderItem::GapRemoved(o) => Some(*o),
 450 -                    RenderItem::GapAdded(n) => Some(*n),
 450 -                    RenderItem::GapPaired(_, n) => Some(*n),
 450 -                };
 453                  n.map_or(false, |n| n >= filter_start && n <= filter_end)
 454              });
 455              new_first = usize::MAX;
 456              new_last = 0;
 457              old_first = usize::MAX;
 458              old_last = 0;
 459 -            for &(_, ref item) in &items {
 459 +            for &(_, row) in &items {
 460 -                match item {
 460 +                if let Some(lhs) = row.lhs { old_first = old_first.min(lhs.line_number as usize); old_last = old_last.max(lhs.line_number as usize); }
 461 -                    RenderItem::DifftRow(row) => {
 461 +                if let Some(rhs) = row.rhs { new_first = new_first.min(rhs.line_number as usize); new_last = new_last.max(rhs.line_number as usize); }
 459 -                        if let Some(lhs) = row.lhs { old_first = old_first.min(lhs.line_number as usize); old_last = old_last.max(lhs.line_number as usize); }
 459 -                        if let Some(rhs) = row.rhs { new_first = new_first.min(rhs.line_number as usize); new_last = new_last.max(rhs.line_number as usize); }
 459 -                    }
 459 -                    RenderItem::GapRemoved(o) => { old_first = old_first.min(*o); old_last = old_last.max(*o); }
 459 -                    RenderItem::GapAdded(n) => { new_first = new_first.min(*n); new_last = new_last.max(*n); }
 459 -                    RenderItem::GapPaired(o, n) => {
 459 -                        old_first = old_first.min(*o); old_last = old_last.max(*o);
 459 -                        new_first = new_first.min(*n); new_last = new_last.max(*n);
 459 -                    }
 459 -                }
 462              }
 463              if items.is_empty() { continue; }
 464  
  ...
 489          old_vis_after = (old_vis_after + new_after_delta).min(difft.old_lines.len());
 490  
 491          // Wider expanded context boundaries for hidden expandable rows.
 492 -        let mut old_ctx_before = old_first.saturating_sub(EXPANDED_CONTEXT_LINES);
 492 +        let old_ctx_before = old_first.saturating_sub(EXPANDED_CONTEXT_LINES);
 493 -        let mut new_ctx_before = new_first.saturating_sub(EXPANDED_CONTEXT_LINES);
 493 +        let new_ctx_before = new_first.saturating_sub(EXPANDED_CONTEXT_LINES);
 494          let mut old_ctx_after = (old_last + 1 + EXPANDED_CONTEXT_LINES).min(difft.old_lines.len());
 495          let mut new_ctx_after = (new_last + 1 + EXPANDED_CONTEXT_LINES).min(difft.new_lines.len());
 496  
  ...
 619          let mut prev_old: Option<usize> = None;
 620          let mut prev_new: Option<usize> = None;
 621  
 622 -        for &(_, ref item) in &items {
 622 +        for &(_, row) in &items {
 623 -            let (cur_old, cur_new) = match item {
 623 +            let cur_old = row.lhs.map(|s| s.line_number as usize);
 624 -                RenderItem::DifftRow(row) => (
 624 +            let cur_new = row.rhs.map(|s| s.line_number as usize);
 622 -                    row.lhs.map(|s| s.line_number as usize),
 622 -                    row.rhs.map(|s| s.line_number as usize),
 622 -                ),
 622 -                RenderItem::GapRemoved(o) => (Some(*o), None),
 622 -                RenderItem::GapAdded(n) => (None, Some(*n)),
 622 -                RenderItem::GapPaired(o, n) => (Some(*o), Some(*n)),
 622 -            };
 625  
 626              // Skip items whose lines were already rendered by a previous chunk.
 627              let dominated_old = cur_old.map_or(false, |o| rendered_old.contains(&o));
  ...
 685  
 686              // Render the item
 687              if single {
 688 -                match item {
 688 +                html.push_str(&render_single_diff_row(row, layout, &old_hl, &new_hl));
 688 -                    RenderItem::DifftRow(row) => {
 688 -                        html.push_str(&render_single_diff_row(row, layout, &old_hl, &new_hl));
 688 -                    }
 688 -                    RenderItem::GapRemoved(old_0) => {
 688 -                        let content = old_hl.get(*old_0).map(|s| s.as_str()).unwrap_or("");
 688 -                        html.push_str(&format!(
 688 -                            "<tr class=\"line-removed\"><td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td></tr>",
 688 -                            old_0 + 1, content
 688 -                        ));
 688 -                    }
 688 -                    RenderItem::GapAdded(new_0) => {
 688 -                        let content = new_hl.get(*new_0).map(|s| s.as_str()).unwrap_or("");
 688 -                        html.push_str(&format!(
 688 -                            "<tr class=\"line-added\"><td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td></tr>",
 688 -                            new_0 + 1, content
 688 -                        ));
 688 -                    }
 688 -                    RenderItem::GapPaired(old_0, new_0) => {
 688 -                        let (idx, hl_lines) = if layout == DiffLayout::AddOnly {
 688 -                            (*new_0, &new_hl)
 688 -                        } else {
 688 -                            (*old_0, &old_hl)
 688 -                        };
 688 -                        html.push_str(&render_single_context_row(idx, hl_lines));
 688 -                    }
 688 -                }
 689              } else {
 690                  html.push_str(&render_diff_row(row, &difft.old_lines, &difft.new_lines, &old_hl, &new_hl));
 691              }
 692  
 693              if let Some(o) = cur_old { prev_old = Some(o); }
 694              if let Some(n) = cur_new { prev_new = Some(n); }
 695          }
 690 -                match item {
 690 +                html.push_str(&render_diff_row(row, &difft.old_lines, &difft.new_lines, &old_hl, &new_hl));
 690 -                    RenderItem::DifftRow(row) => {
 690 -                        html.push_str(&render_diff_row(row, &difft.old_lines, &difft.new_lines, &old_hl, &new_hl));
 690 -                    }
 690 -                    RenderItem::GapRemoved(old_0) => {
 690 -                        let content = old_hl.get(*old_0).map(|s| s.as_str()).unwrap_or("");
 690 -                        html.push_str(&format!(
 690 -                            "<tr class=\"line-removed\"><td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td>\
 690 -                             <td class=\"ln ln-rhs\"></td><td class=\"sign sign-rhs\"></td><td class=\"code-rhs\"></td></tr>",
 690 -                            old_0 + 1, content
 690 -                        ));
 690 -                    }
 690 -                    RenderItem::GapAdded(new_0) => {
 690 -                        let content = new_hl.get(*new_0).map(|s| s.as_str()).unwrap_or("");
 690 -                        html.push_str(&format!(
 690 -                            "<tr class=\"line-added\"><td class=\"ln ln-lhs\"></td><td class=\"sign sign-lhs\"></td><td class=\"code-lhs\"></td>\
 690 -                             <td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td></tr>",
 690 -                            new_0 + 1, content
 690 -                        ));
 690 -                    }
 690 -                    RenderItem::GapPaired(old_0, new_0) => {
 690 -                        // Gap-paired lines matched by trimmed content: only whitespace
 690 -                        // differs. Render as context (no color, no signs) since the
 690 -                        // content is effectively unchanged, just repositioned.
 690 -                        html.push_str(&render_context_row(*old_0, *new_0, &old_hl, &new_hl));
 690 -                    }
 690 -                }
 691              }
 692  
 693              if let Some(o) = cur_old { prev_old = Some(o); }
 694              if let Some(n) = cur_new { prev_new = Some(n); }
 695          }
  ...
2869      //   4. Added-only rows have no hl-add token highlights
2870      //   5. Removed-only rows have no hl-del token highlights
2871      //   6. Text rendering changed-line count matches HTML
2872 -    /// Derive the expected row layout from a chunk's difft JSON plus hunk
2872 -    /// gap lines, applying the same consolidation and sort logic as the
2872 -    /// HTML renderer.
2872 -    /// Returns vec of (row_type, old_ln_1based, new_ln_1based).
2872 -    fn expected_layout(difft: &DifftOutput, chunk_idx: usize) -> Vec<(&'static str, Option<u64>, Option<u64>)> {
2872 -        let chunk = &difft.chunks[chunk_idx];
2872 -        let hunks = &difft.hunks;
2872 -
2872 -        let (lhs_range, rhs_range) = chunk_line_range(chunk);
2872 -        let (old_first, old_last) = match lhs_range {
2872 -            Some((min, max)) => (min as usize, max as usize),
2872 -            None => {
2872 -                let (rmin, rmax) = rhs_range.unwrap_or((0, 0));
2872 -                (to_0based(new_to_old_line(rmin + 1, hunks)), to_0based(new_to_old_line(rmax + 1, hunks)))
2872 -            }
2872 -        };
2872 -        let (new_first, new_last) = match rhs_range {
2872 -            Some((min, max)) => (min as usize, max as usize),
2872 -            None => {
2872 -                let (lmin, lmax) = lhs_range.unwrap_or((0, 0));
2872 -                (to_0based(old_to_new_line(lmin + 1, hunks)), to_0based(old_to_new_line(lmax + 1, hunks)))
2872 -            }
2872 -        };
2872 -
2872 -        let rows = consolidate_chunk(chunk);
2872 -        let mut items: Vec<(u64, &'static str, Option<u64>, Option<u64>)> = Vec::new();
2872 -        // Same keying as renderer: prev_rhs*3+1 for difft removed, anchor+offset for gap
2872 -        let (gap_paired, gap_removed, gap_added) = hunk_gap_lines(
2872 -            chunk, hunks, old_first, old_last, new_first, new_last,
2872 -            &difft.old_lines, &difft.new_lines,
2872 -        );
2872 -        let mut prev_rhs_e: Option<u64> = None;
2872 -        let mut removed_seq_e: u64 = 0;
2872 -        for row in &rows {
2872 -            let key = if let Some(rhs) = row.rhs {
2872 -                prev_rhs_e = Some(rhs.line_number);
2872 -                removed_seq_e = 0;
2872 -                rhs.line_number * 3
2872 -            } else {
2872 -                removed_seq_e += 1;
2872 -                prev_rhs_e.map_or(removed_seq_e, |p| p * 3 + removed_seq_e)
2872 -            };
2872 -            let row_type = if row.lhs.is_some() && row.rhs.is_some() {
2872 -                "line-paired"
2872 -            } else if row.rhs.is_some() {
2872 -                "line-added"
2872 -            } else {
2872 -                "line-removed"
2872 -            };
2872 -            let old_ln = row.lhs.map(|s| s.line_number + 1);
2872 -            let new_ln = row.rhs.map(|s| s.line_number + 1);
2872 -            items.push((key, row_type, old_ln, new_ln));
2872 -        }
2872 -        // Gap-paired render as context, excluded
2872 -        for &old_0 in &gap_removed {
2872 -            let nearest_key = items.iter()
2872 -                .filter_map(|&(k, _, ol, _)| ol.map(|o| {
2872 -                    let dist = ((o - 1) as i64 - old_0 as i64).unsigned_abs();
2872 -                    (dist, k)
2872 -                }))
2872 -                .min_by_key(|&(dist, _)| dist)
2872 -                .map(|(_, k)| k);
2872 -            items.push((nearest_key.unwrap_or(0), "line-removed", Some(old_0 as u64 + 1), None));
2872 -        }
2872 -        for &new_0 in &gap_added {
2872 -            items.push((new_0 as u64 * KEY_SPACE + KEY_SPACE - 1, "line-added", None, Some(new_0 as u64 + 1)));
2872 -        }
2872 -        items.sort_by(|a, b| {
2872 -            a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2))
2872 -        });
2872 -        items.into_iter().map(|(_, t, o, n)| (t, o, n)).collect()
2872 -    }
2872 -
2873      /// Run all rendering checks on a single chunk. Returns a list of errors.
2874      fn check_chunk(difft: &DifftOutput, chunk_idx: usize, file_path: &str) -> Vec<String> {
2875          let mut errors = Vec::new();
2876          let chunk = &difft.chunks[chunk_idx];
2877 -        // Compute chunk line ranges (0-based) for hunk gap analysis
2877 -        let (lhs_range, rhs_range) = chunk_line_range(chunk);
2877 -        let (old_first_0, old_last_0) = match lhs_range {
2877 -            Some((min, max)) => (min as usize, max as usize),
2877 -            None => {
2877 -                let (rmin, rmax) = rhs_range.unwrap_or((0, 0));
2877 -                (to_0based(new_to_old_line(rmin + 1, &difft.hunks)),
2877 -                 to_0based(new_to_old_line(rmax + 1, &difft.hunks)))
2877 -            }
2877 -        };
2877 -        let (new_first_0, new_last_0) = match rhs_range {
2877 -            Some((min, max)) => (min as usize, max as usize),
2877 -            None => {
2877 -                let (lmin, lmax) = lhs_range.unwrap_or((0, 0));
2877 -                (to_0based(old_to_new_line(lmin + 1, &difft.hunks)),
2877 -                 to_0based(old_to_new_line(lmax + 1, &difft.hunks)))
2877 -            }
2877 -        };
2877 -
2878          // Render HTML
2879          let mut hl = Highlighter::new();
2880          let html = render_chunks(difft, &[chunk_idx], file_path, None, &mut hl, CollapseMode::None);
  ...
2931          //
2932          // Build maps from line numbers to expected highlight state:
2933          //   - difft_has_spans: paired entries where difft JSON has non-empty change spans
2934 -        //   - gap_paired: rows matched by trimmed content (only whitespace differs)
2934 -        //     -> should NOT have hl-del/hl-add
2935          //   - one-sided rows with full-line change -> no hl highlights (full bg)
2936          //   - one-sided rows with partial spans -> should have hl highlights
2937          let mut difft_has_spans: std::collections::HashSet<u64> = std::collections::HashSet::new();
  ...
2942                  }
2943              }
2944          }
2945 -        let (gap_paired_set, _, _) = hunk_gap_lines(
2945 -            chunk, &difft.hunks, old_first_0, old_last_0, new_first_0, new_last_0,
2945 -            &difft.old_lines, &difft.new_lines,
2945 -        );
2945 -        let gap_paired_old: std::collections::HashSet<u64> = gap_paired_set.iter()
2945 -            .map(|&(o, _)| o as u64 + 1).collect();
2945 -
2946          let row_re = Regex::new(r#"(?s)<tr class="([^"]+)">(.*?)</tr>"#).unwrap();
2947          let td_re_for_hl = Regex::new(r#"<td class="ln[^"]*"[^>]*>(\d*)</td>"#).unwrap();
2948  
  ...
2986                      .map(|c| { let s = &c[1]; if s.is_empty() { None } else { s.parse().ok() } })
2987                      .collect();
2988                  let old_ln = lns.first().copied().flatten();
2989 -                    // Gap-paired rows render as context, so they won't appear
2989 -                    // as line-paired. No check needed here.
2990                      // Difft paired with spans AND content differs: should have highlights.
2991                      // Skip if old/new content is identical (difft can flag structurally
2992                      // repositioned lines with spans even when content is unchanged).
  ...
2999                              errors.push(format!(
3000                                  "difft-paired row old={} has change spans and content differs but no diff highlights in HTML", ol,
3001                              ));
3002                          }
3003 -                    // Difft paired without spans: should NOT have highlights
3003 -                    // (these are entries where difft reported the line as changed
3003 -                    // but didn't flag specific tokens, e.g. full-line changes)
3003 -                    if !difft_has_spans.contains(&ol) && !gap_paired_old.contains(&ol) {
3003 -                        // This is a difft entry with empty spans - we use
3003 -                        // find_change_bounds which may or may not produce highlights
3003 -                        // depending on content. Don't enforce either way.
3003 -                    }
3004                  }
3005              }
3006          }
  ...
3017              .filter(|(c, _, _)| c == "line-paired").count();
3018          let html_added = non_context.iter()
3019              .filter(|(c, _, _)| c == "line-added").count();
3020 -        // Text: paired rows emit one '-' and one '+' line each.
3020 -        // The ordering guard may drop different gap lines in HTML vs text,
3020 -        // so allow a small tolerance for gap-caused discrepancies.
3021          let expected_minus = html_removed + html_paired;
3022          let expected_plus = html_added + html_paired;
3023          if text_minus != expected_minus {
3023 -        let minus_diff = (text_minus as i64 - expected_minus as i64).unsigned_abs() as usize;
3023 +        if text_minus != expected_minus {
3023 -        let plus_diff = (text_plus as i64 - expected_plus as i64).unsigned_abs() as usize;
3023 -        // Allow up to gap_removed.len() + gap_paired.len() discrepancy
3023 -        let (gap_paired_check_count, gap_removed_count, _) = hunk_gap_lines(
3023 -            chunk, &difft.hunks, old_first_0, old_last_0, new_first_0, new_last_0,
3023 -            &difft.old_lines, &difft.new_lines,
3023 -        );
3023 -        let gap_tolerance = gap_removed_count.len() + gap_paired_check_count.len();
3023 -        if minus_diff > gap_tolerance {
3024              errors.push(format!(
3025                  "text '-' lines: {} vs expected {} (removed={}, paired={})",
3026                  text_minus, expected_minus, html_removed, html_paired,
3027              ));
3025 -                "text '-' lines: {} vs expected {} (removed={}, paired={}, tolerance={})",
3025 +                "text '-' lines: {} vs expected {} (removed={}, paired={})",
3026 -                text_minus, expected_minus, html_removed, html_paired, gap_tolerance,
3026 +                text_minus, expected_minus, html_removed, html_paired,
3027              ));
3028          }
3029          if text_plus != expected_plus {
3029 -        if plus_diff > gap_tolerance {
3029 +        if text_plus != expected_plus {
3030              errors.push(format!(
3031                  "text '+' lines: {} vs expected {} (added={}, paired={})",
3032                  text_plus, expected_plus, html_added, html_paired,
3031 -                "text '+' lines: {} vs expected {} (added={}, paired={}, tolerance={})",
3031 +                "text '+' lines: {} vs expected {} (added={}, paired={})",
3032 -                text_plus, expected_plus, html_added, html_paired, gap_tolerance,
3032 +                text_plus, expected_plus, html_added, html_paired,
3033              ));
3034          }
3035  
3035 -        // 7. No hunk-range lines rendered as context (except gap-paired).
3035 -        // Lines within a unified diff hunk are changed; if they're not in difft's
3035 -        // JSON, they should either be rendered as changed rows (via gap filling)
3035 -        // or as context if they're gap-paired (content-matched, just repositioned).
3035 -        let (gap_paired_check, _, _) = hunk_gap_lines(
3035 -            chunk, &difft.hunks, old_first_0, old_last_0, new_first_0, new_last_0,
3035 -            &difft.old_lines, &difft.new_lines,
3035 -        );
3035 -        let gap_paired_old_set: std::collections::HashSet<u64> = gap_paired_check.iter()
3035 -            .map(|&(o, _)| o as u64 + 1).collect();
3035 -        let gap_paired_new_set: std::collections::HashSet<u64> = gap_paired_check.iter()
3035 -            .map(|&(_, n)| n as u64 + 1).collect();
3035 -
3035 -        let context_old: std::collections::HashSet<u64> = rendered.iter()
3035 -            .filter(|(c, _, _)| c == "line-context")
3035 -            .filter_map(|(_, o, _)| *o)
3035 -            .collect();
3035 -        let context_new: std::collections::HashSet<u64> = rendered.iter()
3035 -            .filter(|(c, _, _)| c == "line-context")
3035 -            .filter_map(|(_, _, n)| *n)
3035 -            .collect();
3035 -
3035 -        for h in &difft.hunks {
3035 -            let hunk_old_start = h.old_start;
3035 -            let hunk_old_end = h.old_start + h.old_count;
3035 -            let hunk_new_start = h.new_start;
3035 -            let hunk_new_end = h.new_start + h.new_count;
3035 -
3035 -            let chunk_old: std::collections::HashSet<u64> = chunk.iter()
3035 -                .filter_map(|e| e.lhs.as_ref().map(|s| s.line_number + 1))
3035 -                .collect();
3035 -            let chunk_new: std::collections::HashSet<u64> = chunk.iter()
3035 -                .filter_map(|e| e.rhs.as_ref().map(|s| s.line_number + 1))
3035 -                .collect();
3035 -
3035 -            let overlaps = chunk_old.iter().any(|&l| l >= hunk_old_start && l < hunk_old_end)
3035 -                || chunk_new.iter().any(|&l| l >= hunk_new_start && l < hunk_new_end);
3035 -            if !overlaps { continue; }
3035 -
3035 -            for ln in hunk_old_start..hunk_old_end {
3035 -                if !chunk_old.contains(&ln) && !gap_paired_old_set.contains(&ln) && context_old.contains(&ln) {
3035 -                    errors.push(format!(
3035 -                        "old line {} is in hunk (old {}+{}) but shown as context (should be changed)",
3035 -                        ln, h.old_start, h.old_count,
3035 -                    ));
3035 -                }
3035 -            }
3035 -            for ln in hunk_new_start..hunk_new_end {
3035 -                if !chunk_new.contains(&ln) && !gap_paired_new_set.contains(&ln) && context_new.contains(&ln) {
3035 -                    errors.push(format!(
3035 -                        "new line {} is in hunk (new {}+{}) but shown as context (should be changed)",
3035 -                        ln, h.new_start, h.new_count,
3035 -                    ));
3035 -                }
3035 -            }
3035 -        }
3035 -
3035 -        // 8. Gap-paired lines (content-matched) should render as context rows.
3035 -        // They may be paired with different new-line numbers if they fall in
3035 -        // the positional context range, or absent if dropped by ordering guards.
3035 -        for &(old_0, new_0) in &gap_paired_check {
3035 -            // Only check gap-paired lines within the chunk's actual range.
3035 -            // Lines outside the range are handled by context rendering.
3035 -            if old_0 < old_first_0 || old_0 > old_last_0 { continue; }
3035 -
3035 -            let old_1 = old_0 as u64 + 1;
3035 -            let new_1 = new_0 as u64 + 1;
3035 -            let is_context = rendered.iter().any(|(c, o, n)| {
3035 -                c == "line-context" && *o == Some(old_1) && *n == Some(new_1)
3035 -            });
3035 -            // Also accept either side appearing as context with any pairing
3035 -            // (positional context uses offset-based pairing, not content matching;
3035 -            // single-column mode only shows one side's line number).
3035 -            let is_context_any = rendered.iter().any(|(c, o, n)| {
3035 -                c == "line-context" && (*o == Some(old_1) || *n == Some(new_1))
3035 -            });
3035 -            // Gap-paired lines may be absent if they were dropped by the
3035 -            // ordering guard (they'd violate old-side order if included).
3035 -            let is_absent = !rendered.iter().any(|(_, o, n)| {
3035 -                *o == Some(old_1) || *n == Some(new_1)
3035 -            });
3035 -            if !is_context && !is_context_any && !is_absent {
3035 -                let content = difft.old_lines.get(old_0).map(|s| s.trim()).unwrap_or("");
3035 -                errors.push(format!(
3035 -                    "gap-paired old {} / new {} should render as context but doesn't: {:?}",
3035 -                    old_1, new_1, &content[..content.len().min(60)],
3035 -                ));
3035 -            }
3035 -        }
3035 -
3036          errors
3037      }
3038  
  ...
3746              rendered_new_lines,
3747          );
3748      }
3749 -    /// When unchanged lines fall between two hunks within a single difft chunk
3749 -    /// (intra-chunk gaps), they should appear as context lines in the rendered
3749 -    /// output. Without this, function signatures between a comment change and
3749 -    /// a body change disappear.
3749 -    #[test]
3749 -    fn intra_chunk_gap_lines_rendered_as_context() {
3749 -        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
3749 -            .join("test_fixtures/intra-chunk-gap/services__agentplat__sbox__sboxd__internal__workspace__manager.go.json");
3749 -        if !fixture_path.exists() {
3749 -            eprintln!("Fixture not found, skipping");
3749 -            return;
3749 -        }
3749 -        let json_str = fs::read_to_string(&fixture_path).unwrap();
3749 -        let difft: DifftOutput = serde_json::from_str(&json_str).unwrap();
3749 -        let file_path = difft.path.as_deref().unwrap_or("unknown");
3749 -
3749 -        // Chunk 3 has entries at old=474/new=478 and old=476/new=None,
3749 -        // with the function signature at new=479 (old=475) falling between
3749 -        // hunks and not in any chunk entry or hunk.
3749 -        let mut hl = Highlighter::new();
3749 -        let html = render_chunks(&difft, &[3], file_path, None, &mut hl, CollapseMode::None);
3749 -        let rows = extract_rows(&html);
3749 -
3749 -        let rendered_new_lines: Vec<u64> = rows.iter()
3749 -            .filter_map(|(_, _, n)| *n)
3749 -            .collect();
3749 -
3749 -        // New line 479 (display 480) is the function signature
3749 -        // "func (m *WorkspaceManager) DefaultWorkspacePath() string {"
3749 -        // It must appear as a context line between the changed entries.
3749 -        assert!(
3749 -            rendered_new_lines.contains(&480),
3749 -            "Intra-chunk gap line (func signature at display 480) should be \
3749 -             rendered as context between chunk entries.\n\
3749 -             Rendered new lines: {:?}\n\
3749 -             Rows: {:?}",
3749 -            rendered_new_lines, rows,
3749 -        );
3749 -    }
3749 -
3750      /// Old-side line numbers must be non-decreasing in the rendered output.
3751      /// When a chunk pairs old lines 484+ with new lines 477+ (a refactoring
3752      /// that moved code), consolidation and sort must not produce jumbled
  ...
3788              violations.join("\n"), old_lns,
3789          );
3790      }
3791 -    /// Low-similarity pairs (>70% changed on both sides) should render
3791 -    /// as `line-paired-full` (full-line red/green, no token highlights)
3791 -    /// rather than `line-paired` (token-level diff). They stay on the
3791 -    /// same row for visual compactness.
3791 -    #[test]
3791 -    fn low_similarity_pairs_use_full_line_background() {
3791 -        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
3791 -            .join("test_fixtures/low-similarity-pairs/services__agentplat__sbox__sboxd__internal__workspace__manager.go.json");
3791 -        if !fixture_path.exists() {
3791 -            eprintln!("Fixture not found, skipping");
3791 -            return;
3791 -        }
3791 -        let json_str = fs::read_to_string(&fixture_path).unwrap();
3791 -        let difft: DifftOutput = serde_json::from_str(&json_str).unwrap();
3791 -        let file_path = difft.path.as_deref().unwrap_or("unknown");
3791 -
3791 -        let mut hl = Highlighter::new();
3791 -        let html = render_chunks(&difft, &[3], file_path, None, &mut hl, CollapseMode::None);
3791 -
3791 -        // Check raw HTML for class attributes since extract_rows
3791 -        // normalizes line-paired-full to line-paired.
3791 -        let row_re = Regex::new(r#"<tr class="(line-paired(?:-full)?)"[^>]*>"#).unwrap();
3791 -        let ln_re = Regex::new(r#"<td class="ln ln-lhs"[^>]*>(\d+)</td>"#).unwrap();
3791 -
3791 -        // Collect (class, old_ln) from the raw single-table HTML (before
3791 -        // split_into_two_tables runs on it — render_chunks returns this).
3791 -        let paired_rows: Vec<(&str, u64)> = row_re.captures_iter(&html)
3791 -            .filter_map(|cap| {
3791 -                let class = cap.get(1).unwrap().as_str();
3791 -                let after = &html[cap.get(0).unwrap().end()..];
3791 -                ln_re.captures(after).and_then(|lc| {
3791 -                    lc[1].parse::<u64>().ok().map(|ln| (class, ln))
3791 -                })
3791 -            })
3791 -            .collect();
3791 -
3791 -        // old=487 (display 488) "m.mu.RLock()" / new=480 (display 481)
3791 -        // "id := string(wsID)" are 100%/89% changed. Must be
3791 -        // line-paired-full, not line-paired.
3791 -        let row_488 = paired_rows.iter().find(|(_, ln)| *ln == 488);
3791 -        assert_eq!(
3791 -            row_488.map(|(c, _)| *c), Some("line-paired-full"),
3791 -            "Low-similarity old=488 should be line-paired-full.\nAll paired: {:?}", paired_rows,
3791 -        );
3791 -
3791 -        // old=488 (display 489) "defer m.mu.RUnlock()" same treatment.
3791 -        let row_489 = paired_rows.iter().find(|(_, ln)| *ln == 489);
3791 -        assert_eq!(
3791 -            row_489.map(|(c, _)| *c), Some("line-paired-full"),
3791 -            "Low-similarity old=489 should be line-paired-full.\nAll paired: {:?}", paired_rows,
3791 -        );
3791 -
3791 -        // Good-similarity: old=493 (display 494) "return ws.RootPath, nil"
3791 -        // / new=490 (display 491) "return resolved, nil" (~40% changed)
3791 -        // should remain line-paired with token highlights.
3791 -        let row_494 = paired_rows.iter().find(|(_, ln)| *ln == 494);
3791 -        assert_eq!(
3791 -            row_494.map(|(c, _)| *c), Some("line-paired"),
3791 -            "Good-similarity old=494 should remain line-paired.\nAll paired: {:?}", paired_rows,
3791 -        );
3791 -    }
3791 -
3792      /// Difft sometimes includes unchanged lines in a chunk when the
3793      /// surrounding syntactic structure changed (e.g. a JSDoc comment grew
3794      /// from 4 to 7 lines). The change spans cover the full line on both
  ...
3970          assert_eq!(btns3, 0, "Last split of a new file should have no expand buttons");
3971      }
3972  
3973 -    /// When a difft chunk spans two separate unified diff hunks, unchanged lines
3973 +    /// Old lines 30-32 (1-based) must appear in the rendered output. With
3974 -    /// between the hunks must appear as context in the rendered output.
3974 +    /// difft's structural matching these were silently excluded because difft
3975 +    /// matched them to identical new-side content. With hunk-based chunks,
3976 +    /// every line in a hunk range is present.
3977      #[test]
3978      fn missing_old_lines_in_difft_chunks() {
3979          let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
3978 -    fn inter_hunk_gap_lines_rendered() {
3978 +    fn missing_old_lines_in_difft_chunks() {
3979          let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
3980              .join("test_fixtures/missing-old-lines/services__agentplat__sbox__sboxd__handler__workspace_handler.go.json");
3981          if !fixture_path.exists() {
3982              eprintln!("Fixture not found, skipping");
3983              return;
3984          }
3980 -            .join("test_fixtures/inter-hunk-gap/services__agentplat__sbox__sboxd__internal__workspace__manager.go.json");
3980 +            .join("test_fixtures/missing-old-lines/services__agentplat__sbox__sboxd__handler__workspace_handler.go.json");
3981          if !fixture_path.exists() {
3982              eprintln!("Fixture not found, skipping");
3983              return;
3984          }
  ...
3986          let difft: DifftOutput = serde_json::from_str(&json_str).unwrap();
3987          let file_path = difft.path.as_deref().unwrap_or("unknown");
3988  
3989 -        // Chunk 13 has rhs entries at 0-based lines 799-804 and 807-809.
3989 +        // Render all chunks together to check the full file diff
3990 -        // Lines 805-806 (0-based) fall between hunk 13 (new 799-805) and
3990 +        let all_indices: Vec<usize> = (0..difft.chunks.len()).collect();
3989 -        // hunk 14 (new 807-809). Line 806 (= file line 807, "case StateReady:")
3989 -        // is not inside either hunk and must still appear as a gap/context line.
3991          let mut hl = Highlighter::new();
3992          let html = render_chunks(&difft, &all_indices, file_path, None, &mut hl, CollapseMode::None);
3993          let rows = extract_rows(&html);
3992 -        let html = render_chunks(&difft, &[13], file_path, None, &mut hl, CollapseMode::None);
3992 +        let html = render_chunks(&difft, &all_indices, file_path, None, &mut hl, CollapseMode::None);
3993          let rows = extract_rows(&html);
3994  
3995          let old_lines_in_html: Vec<u64> = rows.iter()
```

### New tests

A regression test verifies old lines 30-32 appear in the rendered output (the original bug). Six new tests cover the token highlighting heuristic:

```difft src/render.rs chunks=54,55
              let html = render_chunks(&difft, &all_indices, file_path, None, &mut hl, CollapseMode::None);
              let rows = extract_rows(&html);
      
   1 -        // 1-based line 807 = 0-based 806. Rendered line numbers are 1-based.
   1 +        let old_lines_in_html: Vec<u64> = rows.iter()
   2 -        let has_807 = rows.iter().any(|(_, _, rhs)| *rhs == Some(807));
   2 +            .filter_map(|(_, lhs, _)| *lhs)
   3 -        assert!(
   3 +            .collect();
   4 -            has_807,
   4 +
   5 -            "New-side line 807 (case StateReady:) missing from chunk 13 render. \
   5 +        // Old lines 30, 31, 32 (1-based) are within hunk old_start=29, old_count=4
   6 -             Rendered RHS lines: {:?}",
   6 +        // (covers 1-based lines 29-32).
   7 -            rows.iter().filter_map(|(_, _, rhs)| *rhs).collect::<Vec<_>>(),
   7 +        for expected_line in [30, 31, 32] {
   8 +            assert!(
   9 +                old_lines_in_html.contains(&expected_line),
  10 +                "Old line {} (1-based) missing from rendered output. \
  11 +                 Rendered LHS lines: {:?}",
  12 +                expected_line, old_lines_in_html,
  13 +            );
  14 +        }
  15 +    }
  16 +
  17 +    /// Helper: render a single paired row from two lines and return the HTML.
  18 +    fn render_paired_row(old_line: &str, new_line: &str) -> String {
  19 +        let old_lines = vec![old_line.to_string()];
  20 +        let new_lines = vec![new_line.to_string()];
  21 +        let old_hl = vec![html_escape(old_line)];
  22 +        let new_hl = vec![html_escape(new_line)];
  23 +        let lhs = LineSide { line_number: 0, changes: vec![] };
  24 +        let rhs = LineSide { line_number: 0, changes: vec![] };
  25 +        let row = DiffRow { lhs: Some(&lhs), rhs: Some(&rhs) };
  26 +        render_diff_row(&row, &old_lines, &new_lines, &old_hl, &new_hl)
  27 +    }
  28 +
  29 +    /// Pure insertion within a line (shared alphabetic content, one side's
  30 +    /// change is empty) should get token-level highlights.
  31 +    #[test]
  32 +    fn token_highlight_on_pure_insertion() {
  33 +        let html = render_paired_row(
  34 +            "    const sandbox = await createSandbox('client-itest')",
  35 +            "    const sandbox = await createSandbox('client-itest', WORKLOAD_ENV)",
  36 +        );
  37 +        assert!(html.contains("line-paired-full"), "should have full-line background");
  38 +        assert!(html.contains("hl-add"), "pure insertion should have token highlight on new side");
  39 +    }
  40 +
  41 +    /// Pure deletion within a line should get token-level highlights.
  42 +    #[test]
  43 +    fn token_highlight_on_pure_deletion() {
  44 +        let html = render_paired_row(
  45 +            "    await waitForBootstrap(sboxUrl, timeout)",
  46 +            "    await waitForBootstrap(sboxUrl)",
  47 +        );
  48 +        assert!(html.contains("line-paired-full"), "should have full-line background");
  49 +        assert!(html.contains("hl-del"), "pure deletion should have token highlight on old side");
  50 +    }
  51 +
  52 +    /// Lines that are completely rewritten (both sides have changed content)
  53 +    /// should NOT get token-level highlights.
  54 +    #[test]
  55 +    fn no_token_highlight_on_rewrite() {
  56 +        let html = render_paired_row(
  57 +            "        let output = Command::new(\"git\")",
  58 +            "        let (old_lines, new_lines) = get_file_contents(diff_args, file_path)",
  59 +        );
  60 +        assert!(html.contains("line-paired-full"), "should have full-line background");
  61 +        assert!(!html.contains("hl-del"), "rewrite should not have token highlights");
  62 +        assert!(!html.contains("hl-add"), "rewrite should not have token highlights");
  63 +    }
  64 +
  65 +    /// A line paired against an empty line should NOT get token highlights
  66 +    /// even though one side's change region is technically empty. The shared
  67 +    /// content has no alphabetic characters.
  68 +    #[test]
  69 +    fn no_token_highlight_on_empty_pair() {
  70 +        let html = render_paired_row(
  71 +            "            Vec::new()",
  72 +            "",
  73 +        );
  74 +        assert!(html.contains("line-paired-full"), "should have full-line background");
  75 +        assert!(!html.contains("hl-del"), "empty pair should not have token highlights");
  76 +        assert!(!html.contains("hl-add"), "empty pair should not have token highlights");
  77 +    }
  78 +
  79 +    /// Lines sharing only punctuation/whitespace (no alphabetic chars in
  80 +    /// common) should NOT get token highlights.
  81 +    #[test]
  82 +    fn no_token_highlight_on_punctuation_only_match() {
  83 +        // Both lines start with whitespace+punctuation but no shared alpha
  84 +        let html = render_paired_row(
  85 +            "            .env(\"GIT_EXTERNAL_DIFF\", \"difft --display json --color never\")",
  86 +            "            .unwrap_or_else(|e| {",
  87 +        );
  88 +        assert!(html.contains("line-paired-full"), "should have full-line background");
  89 +        assert!(!html.contains("hl-del"), "punctuation-only match should not have token highlights");
  90 +        assert!(!html.contains("hl-add"), "punctuation-only match should not have token highlights");
  91 +    }
  92 +
  93 +    /// A small edit to a function name should get token highlights since
  94 +    /// the shared prefix has alphabetic content.
  95 +    #[test]
  96 +    fn token_highlight_on_small_edit() {
  97 +        let html = render_paired_row(
  98 +            "    let output = Command::new(\"git\")",
  99 +            "    let output = Command::new(\"git-foo\")",
 100          );
 101          assert!(html.contains("line-paired-full"), "should have full-line background");
 102          assert!(html.contains("hl-add"), "small edit should have token highlight");
 101 +        assert!(html.contains("line-paired-full"), "should have full-line background");
 102 +        assert!(html.contains("hl-add"), "small edit should have token highlight");
 103      }
 104  }
```

## 4. Documentation updates

CLAUDE.md is updated to reflect the new architecture. Difftastic references are removed from the description, architecture section, external dependencies, and fixture documentation:

```difft CLAUDE.md chunks=all
      # Walkthrough
      
   1 -Rust CLI that generates narrative walkthroughs of code changes with difftastic diffs, rendered as GitHub-style side-by-side HTML.
   1 +Rust CLI that generates narrative walkthroughs of code changes with side-by-side diffs, rendered as GitHub-style HTML.
   2  
   3  ## Architecture
   4  
   5  - `src/main.rs` - CLI entry point with three subcommands via clap
   6 -- `src/collect.rs` - Runs `difft --display json` via `GIT_EXTERNAL_DIFF` on each changed file, enriches the JSON with file contents and unified diff hunks, writes per-file JSON to an output directory
   6 +- `src/collect.rs` - Runs `git diff -U0` to get unified diff hunks, reads file contents via `git cat-file`, generates chunks by positionally pairing old/new lines within each hunk, writes per-file JSON to an output directory
   7 -- `src/difft_json.rs` - Serde types for difftastic's JSON output (`DifftOutput`, `LineEntry`, `LineSide`, `ChangeSpan`, `DiffHunk`)
   7 +- `src/difft_json.rs` - Serde types for the collected JSON output (`DifftOutput`, `LineEntry`, `LineSide`, `ChangeSpan`, `DiffHunk`)
   8  - `src/render.rs` - Parses walkthrough markdown, replaces `` ```difft `` code blocks with rendered HTML diff tables, converts the rest via pulldown-cmark. Also writes enriched markdown back to the input file with text diffs in the code block bodies.
   9  - `src/verify.rs` - Checks that every chunk in the collected data is referenced by at least one difft code block in the walkthrough
  10  
  11  ## Commands
  12  
  13  ```
  14 -# Collect difft data for the current branch (auto-detects merge-base with origin/master or origin/main)
  14 +# Collect diff data for the current branch (auto-detects merge-base with origin/master or origin/main)
  15  cargo run -- collect
  16  
  17  # Collect with explicit diff range (only needed for non-branch cases)
  ...
  72  Collapsed folds show italic pseudocode text with a yellow background and left yellow border. Clicking expands to reveal the original code, with the yellow left border continuing along the expanded lines.
  73  
  74  ## External dependencies
  75 -- **difftastic** (`difft`) must be installed and on PATH. Used with `--display json --color never` and `DFT_UNSTABLE=yes`.
  76  - **mermaid-cli** (`mmdc`) for pre-rendering mermaid diagrams to inline SVG. Install with `npm install -g @mermaid-js/mermaid-cli`. Uses system Chrome via a puppeteer config. If not installed, mermaid blocks fall back to showing source code.
  77  
  78  ## Build and check
  ...
  84  ```
  85  
  86  ## Rendering details
  87 -### Highlight span merging
  87 -
  87 -Adjacent difft highlight spans that are directly adjacent or separated only by whitespace are merged into single `<span>` regions (`merge_whitespace_spans` in render.rs). Without this, difft's structural matching produces separate spans for each token (e.g. `meta:`, `{`, `level`, `},`), leaving unhighlighted gaps between them.
  87 -
  88  ### Scroll-pinned diff blocks
  89  
  90  Diff blocks have `max-height: 80vh` with `overflow: hidden` (no scrollbar). A JS script intercepts `wheel` events at the window level: when a diff block's top reaches near the viewport top, page scroll is captured and redirected to scroll the block internally. The page's `scrollY` is pinned via a `scroll` event listener to prevent trackpad momentum from pushing surrounding text off-screen. The sticky `.diff-header` stays pinned at the top of the block. When the diff content reaches its end, the pin releases and normal page scrolling resumes.
  91  
  92  ### Markdown enrichment
  93  
  94 -The render command writes back to the input markdown file, replacing each difft code block body with a unified-diff-style text representation (` ` context, `-` removed, `+` added). This uses the same chunk processing logic as the HTML renderer (context lines, consolidation). The enriched markdown is idempotent: re-running render produces identical HTML and re-populates the same text diffs. This enables an LLM workflow where the narrative is written first, then refined after seeing the actual diffs inline.
  94 +The render command writes back to the input markdown file, replacing each difft code block body with a unified-diff-style text representation (` ` context, `-` removed, `+` added). This uses the same chunk processing logic as the HTML renderer (context lines, consolidation). The enriched markdown is idempotent: re-running render produces identical HTML and re-populates the same text diffs.
  95  
  96  ### Expression-aware context expansion
  97  
  ...
 109  
 110  ## Fixture-based rendering tests
 111  
 112 -The `test_fixtures/` directory holds per-commit fixture data (JSON from `walkthrough collect`) that `cargo test` uses to verify the rendering pipeline. Each subdirectory is named by commit hash and contains `*.json` files.
 112 +The `test_fixtures/` directory holds fixture data (JSON from `walkthrough collect`) that `cargo test` uses to verify the rendering pipeline. Each subdirectory contains `*.json` files.
 113  
 114  ### What the tests check
 115  
  ...
 132  # 2. Remove internal files (keep SUMMARY.md for rendering)
 133  rm -f test_fixtures/<commit>/.gitignore test_fixtures/<commit>/.meta.json
 134  
 135 -# 3. Optionally capture difft CLI text for visual reference
 135 +# 3. Run tests
 135 -GIT_EXTERNAL_DIFF="difft --display side-by-side-show-both --color never" \
 135 -  git diff <commit>~1..<commit> -- <file> > test_fixtures/<commit>/<file>.difft.txt
 135 -
 135 -# 4. Run tests
 136  cargo test fixture_rendering_matches
 137  
 138  # 4. Render the fixture walkthrough (uses SUMMARY.md from collect)
 138 -# 5. Render the fixture walkthrough (uses SUMMARY.md from collect)
 138 +# 4. Render the fixture walkthrough (uses SUMMARY.md from collect)
 139  walkthrough render test_fixtures/<commit>/SUMMARY.md \
 140    --data-dir test_fixtures/<commit>/ -o walkthrough-<commit>.html
 141  ```
 142  
 143  Each fixture directory contains:
 144 -- `*.json` - enriched difft JSON (from `walkthrough collect`)
 144 +- `*.json` - collected diff JSON (from `walkthrough collect`)
 145  - `SUMMARY.md` - walkthrough markdown referencing all chunks (renderable)
 146  
 147  ### When to add fixtures
 145 -- `*.difft.txt` - optional difft CLI text output for LLM visual reference
 146  
 147  ### When to add fixtures
 148  
 149 -Add a fixture when you find rendering that differs from difft's CLI output. The fixture captures the exact difft JSON that triggered the issue, ensuring the fix is regression-tested.
 149 +Add a fixture when you find a rendering bug. The fixture captures the exact JSON that triggered the issue, ensuring the fix is regression-tested.
 150  
 151  ## Testing rendered HTML with playwright-cli
 152  
```
