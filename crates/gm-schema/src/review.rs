use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where a review comment stands (B4, D96, D97, D135). `Handled` is set by an agent
/// through MCP, with a summary; only a human makes a comment `Resolved`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Open,
    Handled,
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
    /// What the agent says it did, set with `Handled` and kept when the comment is
    /// resolved; cleared when it is reopened. Narrative only (D96): untrusted.
    #[serde(default)]
    pub summary: Option<String>,
    /// Object id of the snapshot taken when the comment was made; `None` when
    /// the snapshot failed.
    pub snapshot: Option<String>,
    /// Object id of the snapshot taken when an agent marked the comment handled
    /// (D140); `None` before that, when the snapshot failed, and once the
    /// comment is reopened.
    #[serde(default)]
    pub after_snapshot: Option<String>,
    /// Where the anchor is now; `None` when it is outdated.
    pub location: Option<ReviewLocation>,
    /// True when the anchor is not found exactly once in the file now, or the
    /// file is gone.
    pub outdated: bool,
}

/// The anchored lines of a comment as the two snapshots hold them (D140): the
/// region in the "before" snapshot, and the same region in the "after" one,
/// widened to cover the edits that overlap or touch it. Observed by the app,
/// not reported by the agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewChange {
    /// The anchored lines in the "before" snapshot.
    pub before: Vec<String>,
    /// The same region in the "after" snapshot; empty when the file is gone.
    pub after: Vec<String>,
    /// True when an edit between the snapshots overlaps or touches the region.
    pub changed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comment_written_before_the_after_snapshot_reads_without_one() {
        let json = serde_json::json!({
            "id": 1,
            "path": "a.rs",
            "body": "rename",
            "status": "handled",
            "anchor": { "text": "x", "before": [], "after": [], "start_line": 1 },
            "created_at": 5,
            "resolved_at": null,
            "snapshot": "abc",
            "location": null,
            "outdated": true
        });
        let comment: ReviewComment = serde_json::from_value(json).unwrap();
        assert_eq!(comment.after_snapshot, None);
        assert_eq!(comment.summary, None);
    }
}
