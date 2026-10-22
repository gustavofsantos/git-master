use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where a review comment stands (B4, D96, D97). Stored as `open` or
/// `resolved`; the agent's `addressed` state comes with Slice 7.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Open,
    Resolved,
}

/// What a comment is attached to, as it was when the comment was made. The
/// text is matched exactly, as whole lines; the context is kept for display
/// only and never breaks a tie (D97).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewAnchor {
    /// The commented lines, joined with `\n`, without their line endings.
    pub text: String,
    /// Up to three lines above the selection, nearest last.
    pub before: Vec<String>,
    /// Up to three lines below the selection, nearest first.
    pub after: Vec<String>,
    /// The first commented line, 1-based, when the comment was made.
    pub start_line: u32,
}

/// Where an anchor's text is in the file now, as 1-based lines (inclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewLocation {
    pub start_line: u32,
    pub end_line: u32,
}

/// One review comment on a file of the working tree (B4, D96). `location` and
/// `outdated` are computed on read against the file as it is now, not stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewComment {
    pub id: i64,
    /// Path of the file, relative to the worktree root, with `/` separators.
    pub path: String,
    pub body: String,
    pub status: ReviewStatus,
    pub anchor: ReviewAnchor,
    /// Milliseconds since the Unix epoch, as the engine's clock gave them.
    pub created_at: i64,
    /// Set while the comment is resolved; cleared when it is reopened.
    pub resolved_at: Option<i64>,
    /// Object id of the snapshot taken when the comment was made; `None` when
    /// the snapshot failed.
    pub snapshot: Option<String>,
    /// Where the anchor is now; `None` when it is outdated.
    pub location: Option<ReviewLocation>,
    /// True when the anchor is not found exactly once in the file now, or the
    /// file is gone.
    pub outdated: bool,
}
