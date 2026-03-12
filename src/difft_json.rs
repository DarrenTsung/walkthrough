use serde::{Deserialize, Serialize};

/// A single syntax-highlighted span within a line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSpan {
    pub content: String,
    pub highlight: String,
    #[serde(default)]
    pub start: usize,
    #[serde(default)]
    pub end: usize,
}

/// One side (lhs or rhs) of a line entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineSide {
    pub line_number: u64,
    pub changes: Vec<ChangeSpan>,
}

/// A single line entry in a chunk. Both sides are optional:
/// - Only lhs: line was removed
/// - Only rhs: line was added
/// - Both: line was modified (spans show only the changed tokens)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineEntry {
    pub lhs: Option<LineSide>,
    pub rhs: Option<LineSide>,
}

/// A unified diff hunk header, giving exact old/new line ranges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u64,
    pub old_count: u64,
    pub new_start: u64,
    pub new_count: u64,
}

/// The full difft JSON output for a single file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DifftOutput {
    pub chunks: Vec<Vec<LineEntry>>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// Full lines of the old file version (for rendering full-line context).
    #[serde(default)]
    pub old_lines: Vec<String>,
    /// Full lines of the new file version (for rendering full-line context).
    #[serde(default)]
    pub new_lines: Vec<String>,
    /// Unified diff hunks for accurate old<->new line mapping.
    #[serde(default)]
    pub hunks: Vec<DiffHunk>,
}
