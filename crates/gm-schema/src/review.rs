use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where a review comment stands (B4, D96, D97, D135). `Handled` is set by an agent
/// through MCP, with a reply; only a human makes a comment `Resolved`.
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

/// A reply on a review comment: the agent's account of what it did, or the person's
/// answer. Replies are one level deep: a reply to a reply belongs to the root comment.
/// The text is untrusted (D96).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewReply {
    pub id: i64,
    /// `human`, or `agent:<client>` for an agent's reply.
    pub author: String,
    pub body: String,
    /// Milliseconds since the Unix epoch, as the engine's clock gave them.
    pub created_at: i64,
}

/// One review comment on a file of the working tree (B4, D96). `location` and
/// `outdated` are computed on read against the file as it is now, not stored.
/// Lists hold the root comments only; their replies are in `replies`, oldest first.
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
    /// The replies to this comment, oldest first: the agent's accounts and the
    /// person's answers. Narrative only (D96): untrusted.
    #[serde(default)]
    pub replies: Vec<ReviewReply>,
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
    /// The 1-based number of the first line of `before` in the "before" snapshot.
    #[serde(default)]
    pub before_start_line: u32,
    /// The 1-based number of the first line of `after` in the "after" snapshot; where
    /// the region would start when `after` is empty.
    #[serde(default)]
    pub after_start_line: u32,
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
        assert!(comment.replies.is_empty());
    }

    #[test]
    fn a_comment_lists_its_replies_and_no_summary() {
        let comment = ReviewComment {
            id: 1,
            path: "a.rs".into(),
            body: "rename".into(),
            status: ReviewStatus::Handled,
            anchor: ReviewAnchor {
                text: "x".into(),
                before: Vec::new(),
                after: Vec::new(),
                start_line: 1,
            },
            created_at: 5,
            resolved_at: None,
            snapshot: None,
            after_snapshot: None,
            location: None,
            outdated: true,
            replies: vec![ReviewReply {
                id: 2,
                author: "agent:claude-code".into(),
                body: "renamed".into(),
                created_at: 6,
            }],
        };
        let json = serde_json::to_value(&comment).unwrap();
        assert!(json.get("summary").is_none());
        assert_eq!(json["replies"][0]["author"], "agent:claude-code");
        let back: ReviewComment = serde_json::from_value(json).unwrap();
        assert_eq!(back, comment);
    }
}
