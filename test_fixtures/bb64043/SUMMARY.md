# Remove hunk gap filling, trust difft's output

This change simplifies the rendering pipeline by removing the hunk-gap-filling system
that tried to supplement difft's structural matching with lines from `git diff` unified
hunks. The gap filler was the root cause of line ordering bugs, duplicate lines, and
incorrect old/new pairings. Difft's output is now rendered directly.

## 1. Drop unused import

The `Span` import from arborium's advanced module is no longer needed since the
highlight span merging was removed in a prior commit.

```difft src/render.rs chunks=0
   5  use anyhow::{Context, Result};
   6  use pulldown_cmark::{Options, Parser};
   7  use regex::Regex;
   8 -use arborium::advanced::Span;
   9  use arborium::Highlighter;
  10  
  11  use crate::difft_json::{DifftOutput, LineEntry, LineSide};
```

## 2. Make boundary variables mutable for line filtering

The `old_first`/`old_last` and `new_first`/`new_last` variables need to be mutable
because the line filter can narrow the range after initial computation.

```difft src/render.rs chunks=1,2
      
              let (lhs_range, rhs_range) = chunk_line_range(chunk);
      
   1 -        let (old_first, old_last) = match lhs_range {
   1 +        let (mut old_first, mut old_last) = match lhs_range {
   2              Some((min, max)) => (min as usize, max as usize),
   3              None => {
   4                  let (rmin, rmax) = rhs_range.unwrap_or((0, 0));
  ...
   7                  (o_min, o_max)
   8              }
   9          };
  10 -        let (new_first, new_last) = match rhs_range {
  10 +        let (mut new_first, mut new_last) = match rhs_range {
  11              Some((min, max)) => (min as usize, max as usize),
  12              None => {
  13                  let (lmin, lmax) = lhs_range.unwrap_or((0, 0));
```

## 3. Remove hunk gap filling from the HTML renderer

This is the core change. The previous code cross-referenced unified diff hunks with
difft's output to find "gap" lines that difft's structural matching missed, then
inserted them as extra removed/added/paired rows. This caused line ordering bugs
because positional pairing of gap lines produced wrong old/new correspondences.

The replacement trusts difft's chunk entries directly. Each entry becomes a `DifftRow`
via `consolidate_chunk`, sorted by new-side line number (with removed-only entries
slotted after their preceding paired entry via `prev_rhs * 2 + 1`).

```difft src/render.rs chunks=3,4,5,6,7,8
                      (n_min, n_max)
                  }
              };
     -        let mut difft_old_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
     -        let mut difft_new_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
     -        for entry in chunk {
     -            if let Some(lhs) = &entry.lhs { difft_old_lines.insert(lhs.line_number as usize); }
     -            if let Some(rhs) = &entry.rhs { difft_new_lines.insert(rhs.line_number as usize); }
     -        }
     -
     -        let mut extra_removed: Vec<usize> = Vec::new();
     -        let mut extra_added: Vec<usize> = Vec::new();
     -        let mut extra_paired: Vec<(usize, usize)> = Vec::new();
     -
     -        for h in hunks {
     -            let h_old_start_0 = (h.old_start as usize).saturating_sub(1);
     -            let h_old_end_0 = h_old_start_0 + h.old_count as usize;
     -            let h_new_start_0 = (h.new_start as usize).saturating_sub(1);
     -            let h_new_end_0 = h_new_start_0 + h.new_count as usize;
     -
     -            let overlaps_old = h_old_end_0 > old_first.saturating_sub(1) && h_old_start_0 <= old_last + 1;
     -            let overlaps_new = h_new_end_0 > new_first.saturating_sub(1) && h_new_start_0 <= new_last + 1;
     -
     -            if overlaps_old || overlaps_new {
     -                let mut hunk_removed: Vec<usize> = Vec::new();
     -                let mut hunk_added: Vec<usize> = Vec::new();
     -                for line_0 in h_old_start_0..h_old_end_0 {
     -                    if !difft_old_lines.contains(&line_0) { hunk_removed.push(line_0); }
     -                }
     -                for line_0 in h_new_start_0..h_new_end_0 {
     -                    if !difft_new_lines.contains(&line_0) { hunk_added.push(line_0); }
     -                }
     -                let pair_count = hunk_removed.len().min(hunk_added.len());
     -                for i in 0..pair_count {
     -                    extra_paired.push((hunk_removed[i], hunk_added[i]));
     -                }
     -                for &r in &hunk_removed[pair_count..] { extra_removed.push(r); }
     -                for &a in &hunk_added[pair_count..] { extra_added.push(a); }
     -            }
     -        }
     -
     -        let mut old_first = old_first;
     -        let mut old_last = old_last;
     -        let mut new_first = new_first;
     -        let mut new_last = new_last;
     -        for &old_0 in &extra_removed { old_first = old_first.min(old_0); old_last = old_last.max(old_0); }
     -        for &new_0 in &extra_added { new_first = new_first.min(new_0); new_last = new_last.max(new_0); }
     -        for &(old_0, new_0) in &extra_paired {
     -            old_first = old_first.min(old_0); old_last = old_last.max(old_0);
     -            new_first = new_first.min(new_0); new_last = new_last.max(new_0);
     -        }
     -
              let old_ctx_before = old_first.saturating_sub(CONTEXT_LINES);
              let new_ctx_before = new_first.saturating_sub(CONTEXT_LINES);
  ...
              // Build unified item list (same logic as HTML renderer)
              #[derive(Clone, Copy)]
              enum TextItem<'a> {
     -            RemovedLine(usize),
     -            AddedLine(usize),
     -            PairedLine(usize, usize),
              }
      
              let rows = consolidate_chunk(chunk);
              let mut items: Vec<(u64, TextItem)> = Vec::new();
   1 -        for (i, row) in rows.iter().enumerate() {
   1 +        let mut prev_rhs: Option<u64> = None;
   2 +        for row in &rows {
   3 +            let key = if let Some(rhs) = row.rhs {
   4 +                prev_rhs = Some(rhs.line_number);
   5 +                rhs.line_number * 2
   6 +            } else {
   7 -            items.push((i as u64, TextItem::DifftRow(row)));
   7 +                prev_rhs.map_or(0, |p| p * 2 + 1)
   1 -        }
   8 +            };
   9 +            items.push((key, TextItem::DifftRow(row)));
  11 -        let base = rows.len() as u64;
  11 +        items.sort_by_key(|&(k, _)| k);
   1 -        let mut extras: Vec<(u64, TextItem)> = Vec::new();
   1 -        for &(old_0, new_0) in &extra_paired {
   1 -            extras.push((new_0 as u64, TextItem::PairedLine(old_0, new_0)));
   1 -        }
   1 -        for &old_0 in &extra_removed {
   1 -            let mapped = old_to_new_line(old_0 as u64 + 1, hunks);
   1 -            extras.push((mapped - 1, TextItem::RemovedLine(old_0)));
   1 -        }
   1 -        for &new_0 in &extra_added {
   1 -            extras.push((new_0 as u64, TextItem::AddedLine(new_0)));
   1 -        }
   1 -        extras.sort_by_key(|&(k, _)| k);
   1 -        for (i, (_, item)) in extras.into_iter().enumerate() {
   1 -            items.push((base + i as u64, item));
  11 -        }
  12  
  13          // Collect ALL item line numbers before filtering so filtered-out
  14          // changed lines don't become context rows.
  ...
  19                  TextItem::DifftRow(row) => {
  20                      if let Some(lhs) = row.lhs { item_old_lines_t.insert(lhs.line_number as usize); }
  21                      if let Some(rhs) = row.rhs { item_new_lines_t.insert(rhs.line_number as usize); }
  22 -                TextItem::RemovedLine(o) => { item_old_lines_t.insert(*o); }
  22 -                TextItem::AddedLine(n) => { item_new_lines_t.insert(*n); }
  22 -                TextItem::PairedLine(o, n) => { item_old_lines_t.insert(*o); item_new_lines_t.insert(*n); }
  23              }
  24          }
  25  
  ...
  28              items.retain(|&(_, ref item)| {
  29                  let n = match item {
  30                      TextItem::DifftRow(row) => row.rhs.map(|s| s.line_number as usize)
  31 -                    TextItem::RemovedLine(o) => Some(*o),
  31 -                    TextItem::AddedLine(n) => Some(*n),
  31 -                    TextItem::PairedLine(_, n) => Some(*n),
  32                  };
  33                  n.map_or(false, |n| n >= filter_start && n <= filter_end)
  34              });
  ...
  41                      TextItem::DifftRow(row) => {
  42                          if let Some(lhs) = row.lhs { old_first = old_first.min(lhs.line_number as usize); old_last = old_last.max(lhs.line_number as usize); }
  43                          if let Some(rhs) = row.rhs { new_first = new_first.min(rhs.line_number as usize); new_last = new_last.max(rhs.line_number as usize); }
  44 -                    TextItem::RemovedLine(o) => { old_first = old_first.min(*o); old_last = old_last.max(*o); }
  44 -                    TextItem::AddedLine(n) => { new_first = new_first.min(*n); new_last = new_last.max(*n); }
  44 -                    TextItem::PairedLine(o, n) => {
  44 -                        old_first = old_first.min(*o); old_last = old_last.max(*o);
  44 -                        new_first = new_first.min(*n); new_last = new_last.max(*n);
  44 -                    }
  45                  }
  46              }
  47              if items.is_empty() { continue; }
```

## 4. Remove hunk gap filling from the text renderer

The same simplification applied to `render_chunks_text`, which mirrors the HTML
renderer's logic for the plain-text diff written back into the markdown file.

```difft src/render.rs chunks=9,10,11,12,13,14,15,16
                      TextItem::DifftRow(row) => (
                          row.lhs.map(|s| s.line_number as usize),
                          row.rhs.map(|s| s.line_number as usize),
     -                TextItem::RemovedLine(o) => (Some(*o), None),
     -                TextItem::AddedLine(n) => (None, Some(*n)),
     -                TextItem::PairedLine(o, n) => (Some(*o), Some(*n)),
     -            // Fill gaps with context, skipping lines that items already cover
     -            let expected_old = prev_old.map(|p| p + 1);
     -            let expected_new = prev_new.map(|p| p + 1);
     -            if let (Some(exp_o), Some(cur_o)) = (expected_old, cur_old) {
     -                if cur_o > exp_o {
     -                    let exp_n = expected_new.unwrap_or_else(|| to_0based(old_to_new_line(exp_o as u64 + 1, hunks)));
     -                    for i in 0..(cur_o - exp_o) {
     -                        let n = exp_n + i;
     -                        let o = exp_o + i;
     -                        if !item_old_lines_t.contains(&o) && !item_new_lines_t.contains(&n) {
     -                            let line = difft.new_lines.get(n).map(|s| s.as_str()).unwrap_or("");
     -                            out.push_str(&fmt_line(" ", n, line));
     -                        }
     -                    }
     -                }
     -            } else if let (Some(exp_n), Some(cur_n)) = (expected_new, cur_new) {
     -                if cur_n > exp_n {
     -                    let exp_o = expected_old.unwrap_or_else(|| to_0based(new_to_old_line(exp_n as u64 + 1, hunks)));
     -                    for i in 0..(cur_n - exp_n) {
     -                        let n = exp_n + i;
     -                        let o = exp_o + i;
     -                        if !item_old_lines_t.contains(&o) && !item_new_lines_t.contains(&n) {
     -                            let line = difft.new_lines.get(n).map(|s| s.as_str()).unwrap_or("");
     -                            out.push_str(&fmt_line(" ", n, line));
     -                        }
     -                    }
     -                }
     -            }
      
                  // Render the item with relative line numbers
                  match item {
  ...
                              }
                              (None, None) => {}
                          }
     -                TextItem::RemovedLine(old_0) => {
     -                    let n = to_0based(old_to_new_line(*old_0 as u64 + 1, hunks));
     -                    let line = difft.old_lines.get(*old_0).map(|s| s.as_str()).unwrap_or("");
     -                    out.push_str(&fmt_line("-", n, line));
     -                }
     -                TextItem::AddedLine(new_0) => {
     -                    let line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
     -                    out.push_str(&fmt_line("+", *new_0, line));
     -                }
     -                TextItem::PairedLine(old_0, new_0) => {
     -                    let old_line = difft.old_lines.get(*old_0).map(|s| s.as_str()).unwrap_or("");
     -                    let new_line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
     -                    out.push_str(&fmt_line("-", *new_0, old_line));
     -                    out.push_str(&fmt_line("+", *new_0, new_line));
     -                }
                  }
      
                  if let Some(o) = cur_old { prev_old = Some(o); }
  ...
      
              // Resolve the first/last 0-based line on each side, using unified diff hunks
              // to map between old/new when one side has no entries in this chunk.
   1 -        let (old_first, old_last) = match lhs_range {
   1 +        let (mut old_first, mut old_last) = match lhs_range {
   2              Some((min, max)) => (min as usize, max as usize),
   3              None => {
   4                  let (rmin, rmax) = rhs_range.unwrap_or((0, 0));
  ...
   8                  (o_min, o_max)
   9              }
  10          };
  11 -        let (new_first, new_last) = match rhs_range {
  11 +        let (mut new_first, mut new_last) = match rhs_range {
  12              Some((min, max)) => (min as usize, max as usize),
  13              None => {
  14                  let (lmin, lmax) = lhs_range.unwrap_or((0, 0));
  ...
  18              }
  19          };
  20  
  21 -        // Collect 0-based line numbers that difft already covers in this chunk.
  21 +        // Build a list of render items directly from difft entries.
  22 -        let mut difft_old_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
  22 +        // No hunk-gap filling: trust difft's structural matching.
  21 -        let mut difft_new_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
  21 -        for entry in chunk {
  21 -            if let Some(lhs) = &entry.lhs {
  21 -                difft_old_lines.insert(lhs.line_number as usize);
  21 -            }
  21 -            if let Some(rhs) = &entry.rhs {
  21 -                difft_new_lines.insert(rhs.line_number as usize);
  21 -            }
  21 -        }
  21 -
  21 -        // Find unified diff hunk lines that overlap this chunk's range but difft missed.
  21 -        // These are removed/added lines that difft's structural matching didn't flag.
  21 -        // Must be computed BEFORE context so we can extend boundaries to include hunk lines.
  21 -        let mut extra_removed: Vec<usize> = Vec::new(); // 0-based old line indices
  21 -        let mut extra_added: Vec<usize> = Vec::new(); // 0-based new line indices
  21 -        let mut extra_paired: Vec<(usize, usize)> = Vec::new(); // (old_0, new_0)
  21 -
  21 -        for h in hunks {
  21 -            let h_old_start_0 = (h.old_start as usize).saturating_sub(1); // 0-based
  21 -            let h_old_end_0 = h_old_start_0 + h.old_count as usize;
  28 -        // Render difft entries in chunk order (preserving difft's structural
  28 +        // Sort by new-side line number (matching difft CLI's render order).
  21 -            let h_new_start_0 = (h.new_start as usize).saturating_sub(1);
  21 -            let h_new_end_0 = h_new_start_0 + h.new_count as usize;
  21 -            let overlaps_old = h_old_end_0 > old_first.saturating_sub(1) && h_old_start_0 <= old_last + 1;
  29 -        // matching), then append hunk-gap extras sorted by line number.
  29 +        // For removed-only entries, use the previous entry's rhs to keep them
  21 -            let overlaps_new = h_new_end_0 > new_first.saturating_sub(1) && h_new_start_0 <= new_last + 1;
  21 -            if overlaps_old || overlaps_new {
  30 +        // positioned between their neighboring paired entries.
  21 -                let mut hunk_removed: Vec<usize> = Vec::new();
  21 -                let mut hunk_added: Vec<usize> = Vec::new();
  21 -                for line_0 in h_old_start_0..h_old_end_0 {
  21 -                    if !difft_old_lines.contains(&line_0) {
  21 -                        hunk_removed.push(line_0);
  21 -                    }
  21 -                }
  21 -                for line_0 in h_new_start_0..h_new_end_0 {
  21 -                    if !difft_new_lines.contains(&line_0) {
  21 -                        hunk_added.push(line_0);
  21 -                    }
  21 -                }
  21 -                // Pair removed/added lines from the same hunk together
  34 -        // Difft rows get sequential keys to preserve chunk order
  34 +        let mut prev_rhs: Option<u64> = None;
  21 -                let pair_count = hunk_removed.len().min(hunk_added.len());
  21 -                for i in 0..pair_count {
  35 +        for row in &rows {
  21 -                    extra_paired.push((hunk_removed[i], hunk_added[i]));
  21 -                }
  21 -                for &r in &hunk_removed[pair_count..] {
  21 -                    extra_removed.push(r);
  36 -        for (i, row) in rows.iter().enumerate() {
  36 +            let key = if let Some(rhs) = row.rhs {
  21 -                }
  21 -                for &a in &hunk_added[pair_count..] {
  37 +                prev_rhs = Some(rhs.line_number);
  21 -                    extra_added.push(a);
  21 -                }
  21 -            }
  38 +                rhs.line_number * 2
  21 -        }
  21 -        // Extend chunk boundaries to include hunk-only lines so context doesn't
  21 -        // overlap with changed lines.
  39 +            } else {
  21 -        let mut old_first = old_first;
  21 -        let mut old_last = old_last;
  21 -        let mut new_first = new_first;
  40 +                // Removed-only: place just after the previous entry's rhs
  21 -        let mut new_last = new_last;
  21 -        for &old_0 in &extra_removed {
  21 -            old_first = old_first.min(old_0);
  41 +                prev_rhs.map_or(0, |p| p * 2 + 1)
  21 -            old_last = old_last.max(old_0);
  21 -        }
  21 -        for &new_0 in &extra_added {
  42 +            };
  21 -            new_first = new_first.min(new_0);
  21 -            new_last = new_last.max(new_0);
  21 -        }
  21 -        for &(old_0, new_0) in &extra_paired {
  43 -            items.push((i as u64, RenderItem::DifftRow(row)));
  43 +            items.push((key, RenderItem::DifftRow(row)));
  34 -        }
  21 -            old_first = old_first.min(old_0);
  21 -            old_last = old_last.max(old_0);
  21 -            new_first = new_first.min(new_0);
  21 -            new_last = new_last.max(new_0);
  21 -        }
  21 -        // Build a unified list of render items: difft rows + hunk-only lines.
  45 -        let base = rows.len() as u64;
  45 +        items.sort_by_key(|&(k, _)| k);
  34 -
  25 -            RemovedLine(usize),        // 0-based old line
  34 -        // Hunk-gap extras are sorted by new-side line and placed after difft rows
  25 -            AddedLine(usize),          // 0-based new line
  34 -        let mut extras: Vec<(u64, RenderItem)> = Vec::new();
  25 -            PairedLine(usize, usize),  // (old_0, new_0) from same hunk
  34 -        for &(old_0, new_0) in &extra_paired {
  34 -            extras.push((new_0 as u64, RenderItem::PairedLine(old_0, new_0)));
  34 -        }
  34 -        for &old_0 in &extra_removed {
  34 -            let mapped = old_to_new_line(old_0 as u64 + 1, hunks);
  34 -            extras.push((mapped - 1, RenderItem::RemovedLine(old_0)));
  34 -        }
  34 -        for &new_0 in &extra_added {
  34 -            extras.push((new_0 as u64, RenderItem::AddedLine(new_0)));
  34 -        }
  34 -        extras.sort_by_key(|&(k, _)| k);
  34 -        for (i, (_, item)) in extras.into_iter().enumerate() {
  34 -            items.push((base + i as u64, item));
  45 -        }
  46 -
  46  
  47          // Collect ALL item line numbers before filtering, so the gap filler
  48          // never renders filtered-out changed lines as context rows.
  ...
  53                  RenderItem::DifftRow(row) => {
  54                      if let Some(lhs) = row.lhs { item_old_lines.insert(lhs.line_number as usize); }
  55                      if let Some(rhs) = row.rhs { item_new_lines.insert(rhs.line_number as usize); }
  56 -                RenderItem::RemovedLine(o) => { item_old_lines.insert(*o); }
  56 -                RenderItem::AddedLine(n) => { item_new_lines.insert(*n); }
  56 -                RenderItem::PairedLine(o, n) => { item_old_lines.insert(*o); item_new_lines.insert(*n); }
  57              }
  58          }
  59  
  ...
  63              items.retain(|&(_, ref item)| {
  64                  let n = match item {
  65                      RenderItem::DifftRow(row) => row.rhs.map(|s| s.line_number as usize)
  66 -                    RenderItem::RemovedLine(o) => Some(*o),
  66 -                    RenderItem::AddedLine(n) => Some(*n),
  66 -                    RenderItem::PairedLine(_, n) => Some(*n),
  67                  };
  68                  n.map_or(false, |n| n >= filter_start && n <= filter_end)
  69              });
  ...
  76                      RenderItem::DifftRow(row) => {
  77                          if let Some(lhs) = row.lhs { old_first = old_first.min(lhs.line_number as usize); old_last = old_last.max(lhs.line_number as usize); }
  78                          if let Some(rhs) = row.rhs { new_first = new_first.min(rhs.line_number as usize); new_last = new_last.max(rhs.line_number as usize); }
  79 -                    RenderItem::RemovedLine(o) => { old_first = old_first.min(*o); old_last = old_last.max(*o); }
  79 -                    RenderItem::AddedLine(n) => { new_first = new_first.min(*n); new_last = new_last.max(*n); }
  79 -                    RenderItem::PairedLine(o, n) => {
  79 -                        old_first = old_first.min(*o); old_last = old_last.max(*o);
  79 -                        new_first = new_first.min(*n); new_last = new_last.max(*n);
  79 -                    }
  80                  }
  81              }
  82              if items.is_empty() { continue; }
```

## 5. Remove gap filler from context line rendering

The post-chunk context line loop previously had to skip lines that were already
rendered by the gap filler. With the gap filler gone, the skip set now only contains
lines from difft's own entries.

```difft src/render.rs chunks=17,18
1330                  RenderItem::DifftRow(row) => (
1331                      row.lhs.map(|s| s.line_number as usize),
1332                      row.rhs.map(|s| s.line_number as usize),
1333 -                RenderItem::RemovedLine(o) => (Some(*o), None),
1333 -                RenderItem::AddedLine(n) => (None, Some(*n)),
1333 -                RenderItem::PairedLine(o, n) => (Some(*o), Some(*n)),
1335 -            // Fill gap with context lines, skipping any that overlap with items
1335 -            let expected_old = prev_old.map(|p| p + 1);
1335 -            let expected_new = prev_new.map(|p| p + 1);
1335 -
1335 -            if let (Some(exp_o), Some(cur_o)) = (expected_old, cur_old) {
1335 -                if cur_o > exp_o {
1335 -                    let exp_n = expected_new.unwrap_or_else(|| {
1335 -                        to_0based(old_to_new_line(exp_o as u64 + 1, hunks))
1335 -                    });
1335 -                    for i in 0..(cur_o - exp_o) {
1335 -                        let o = exp_o + i;
1335 -                        let n = exp_n + i;
1335 -                        if !item_old_lines.contains(&o) && !item_new_lines.contains(&n) {
1335 -                            html.push_str(&render_context_row(o, n, &old_hl, &new_hl));
1335 -                        }
1335 -                    }
1335 -                }
1335 -            } else if let (Some(exp_n), Some(cur_n)) = (expected_new, cur_new) {
1335 -                if cur_n > exp_n {
1335 -                    let exp_o = expected_old.unwrap_or_else(|| {
1335 -                        to_0based(new_to_old_line(exp_n as u64 + 1, hunks))
1335 -                    });
1335 -                    for i in 0..(cur_n - exp_n) {
1335 -                        let o = exp_o + i;
1335 -                        let n = exp_n + i;
1335 -                        if !item_old_lines.contains(&o) && !item_new_lines.contains(&n) {
1335 -                            html.push_str(&render_context_row(o, n, &old_hl, &new_hl));
1335 -                        }
1335 -                    }
1335 -                }
1335 -            }
1335 -
1336              // Render the item
1337              match item {
1338                  RenderItem::DifftRow(row) => {
1339                      html.push_str(&render_diff_row(row, &difft.old_lines, &difft.new_lines, &old_hl, &new_hl));
1340 -                RenderItem::RemovedLine(old_0) => {
1340 -                    let content = old_hl.get(*old_0).map(|s| s.as_str()).unwrap_or("");
1340 -                    html.push_str(&format!(
1340 -                        "<tr class=\"line-removed\"><td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td>\
1340 -                         <td class=\"ln ln-rhs\"></td><td class=\"sign sign-rhs\"></td><td class=\"code-rhs\"></td></tr>",
1340 -                        old_0 + 1, content
1340 -                    ));
1340 -                }
1340 -                RenderItem::AddedLine(new_0) => {
1340 -                    let content = new_hl.get(*new_0).map(|s| s.as_str()).unwrap_or("");
1340 -                    html.push_str(&format!(
1340 -                        "<tr class=\"line-added\"><td class=\"ln ln-lhs\"></td><td class=\"sign sign-lhs\"></td><td class=\"code-lhs\"></td>\
1340 -                         <td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td></tr>",
1340 -                        new_0 + 1, content
1340 -                    ));
1340 -                }
1340 -                RenderItem::PairedLine(old_0, new_0) => {
1340 -                    let old_line = difft.old_lines.get(*old_0).map(|s| s.as_str()).unwrap_or("");
1340 -                    let new_line = difft.new_lines.get(*new_0).map(|s| s.as_str()).unwrap_or("");
1340 -                    let old_highlighted = old_hl.get(*old_0).map(|s| s.as_str()).unwrap_or("");
1340 -                    let new_highlighted = new_hl.get(*new_0).map(|s| s.as_str()).unwrap_or("");
1340 -                    let (old_cs, old_ce, new_cs, new_ce) = find_change_bounds(old_line, new_line);
1340 -                    html.push_str(&format!(
1340 -                        "<tr class=\"line-paired\"><td class=\"ln ln-lhs\">{}</td><td class=\"sign sign-lhs\">\u{2212}</td><td class=\"code-lhs\">{}</td>\
1340 -                         <td class=\"ln ln-rhs\">{}</td><td class=\"sign sign-rhs\">+</td><td class=\"code-rhs\">{}</td></tr>",
1340 -                        old_0 + 1, insert_diff_highlight(old_highlighted, old_cs, old_ce, "hl-del"),
1340 -                        new_0 + 1, insert_diff_highlight(new_highlighted, new_cs, new_ce, "hl-add"),
1340 -                    ));
1340 -                }
1341              }
1342  
1343              if let Some(o) = cur_old { prev_old = Some(o); }
```

## 6. Add foundry_api.ts layout test

A new test case (`foundry_api_chunk2_matches_difft_cli_layout`) captures the exact
chunk structure from a real-world diff that triggered the line ordering bugs. It
verifies that old-side line numbers are non-decreasing and that the removed-only
line (old:321) appears in the correct position between its neighbors.

```difft src/render.rs chunks=19
              let difft = make_difft(old_lines, new_lines, vec![chunk], hunks);
      
              test_render_chunks(&difft, &[0], "test.ts", None)
   1 +    }
   3 +    #[test]
   4 +    fn foundry_api_chunk2_matches_difft_cli_layout() {
   5 +        // Verify that the rendered row layout for the problematic foundry_api.ts
   6 +        // chunk 2 matches difft CLI's side-by-side output.
   7 +        //
   8 +        // Expected layout (from `difft --display side-by-side`):
   9 +        //   old:317 ↔ new:377  (paired)
  10 +        //   old:318 ↔ new:378  (paired)
  11 +        //   old:319 ↔ new:379  (paired)
  12 +        //   old:320 ↔ new:380  (paired)
  13 +        //   old:321 ↔ (none)   (removed-only)
  14 +        //   old:322 ↔ new:381  (paired)
  15 +        //   old:323 ↔ new:382  (paired)
  16 +        //   old:324 ↔ new:383  (paired, note: 383 was a NEW-only entry in difft JSON)
  17 +        //   (none)  ↔ new:384  (added)
  18 +        //   (none)  ↔ new:385  (added)
  19 +        //   ...gap-filled new lines...
  20 +        //   old:325 ↔ new:391  (paired, from hunk gap)
  21 +        //   ...more...
  22 +        let html = extract_rows_html_from_real_data();
  23 +        let rows = extract_rows(&html);
  25 +        // Extract (old_ln, new_ln) pairs for non-context rows
  26 +        let changed: Vec<(Option<u64>, Option<u64>)> = rows.iter()
  27 +            .filter(|(c, _, _)| c != "line-context" && c != "chunk-sep")
  28 +            .map(|(_, o, n)| (*o, *n))
  29 +            .collect();
  31 +        // Old-side line numbers must be consecutive (non-decreasing)
  32 +        let old_lns: Vec<u64> = changed.iter().filter_map(|(o, _)| *o).collect();
  33 +        for i in 1..old_lns.len() {
  34 +            assert!(old_lns[i] >= old_lns[i-1],
  35 +                "old-side out of order: {} then {} at pos {}\nall old: {:?}",
  36 +                old_lns[i-1], old_lns[i], i, old_lns);
  37 +        }
  39 +        // The removed-only line (old:321) must appear between old:320 and old:322
  40 +        let old_321_pos = old_lns.iter().position(|&l| l == 321);
  41 +        let old_320_pos = old_lns.iter().position(|&l| l == 320);
  42 +        let old_322_pos = old_lns.iter().position(|&l| l == 322);
  43 +        assert!(old_321_pos.is_some(), "old line 321 should be present");
  44 +        assert!(old_320_pos.unwrap() < old_321_pos.unwrap(),
  45 +            "old 320 should come before old 321");
  46 +        assert!(old_321_pos.unwrap() < old_322_pos.unwrap(),
  47 +            "old 321 should come before old 322");
  49 +        // Added-only lines (383-389) should appear between paired entries
  50 +        // for old:324↔new:382 and wherever old:325↔new:391 ends up
  51 +        let new_lns: Vec<u64> = changed.iter().filter_map(|(_, n)| *n).collect();
  52 +        assert!(new_lns.contains(&384), "new line 384 should be present");
  53 +        assert!(new_lns.contains(&388), "new line 388 should be present");
  55 +        // new:383 should come after new:382 (paired with old:324)
  56 +        let new_382_pos = new_lns.iter().position(|&l| l == 382);
  57 +        let new_383_pos = new_lns.iter().position(|&l| l == 383);
  58 +        if let (Some(p382), Some(p383)) = (new_382_pos, new_383_pos) {
  59 +            assert!(p382 < p383,
  60 +                "new 382 should come before new 383, got pos {} vs {}",
  61 +                p382, p383);
  62 +        }
   2 +
  24 +
  30 +
  38 +
  48 +
  54 +
  63 +    }
  64  }
```
