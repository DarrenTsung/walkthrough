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
}
