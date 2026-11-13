//! Writing a review comment (B4c, D96, D141): where the picked lines are, what the composer
//! writes, and the command it sends. Shared by the app's composer and the terminal
//! companion's (D235 (3)); moved here from `gm-ui` without a change of rule.

use gm_schema::{CommandKind, CommentLabel};

use crate::diff_rows::Side;
use crate::review::{Covered, Picked, range_label};

/// Where the lines picked to comment on are, for a review comment (B4c, D141).
/// Built by a front end from its diff or its file viewer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommentSpot<'a> {
    /// Nothing is picked: no file is shown, or the cursor is on a hunk header.
    #[default]
    Nothing,
    /// Lines of `path` that the working tree has at these numbers: the unstaged
    /// diff's new side and the Code viewer. `working` is the lowest and highest
    /// line, `None` when all are removed lines.
    Lines {
        path: &'a str,
        working: Option<(u32, u32)>,
    },
    /// Lines of `path` on a surface that is not the working tree (the staged side,
    /// a commit, a stash): the engine finds the text in the file now.
    Text { path: &'a str },
    /// The pick runs over more than one hunk, so its text is not one run of lines.
    Split,
}

/// The spot of rows picked in the Working Copy diff of `path` on `side`: the unstaged new
/// side is the working tree, so its numbers are the file's; the staged side is found by its
/// text, which must be one run of lines.
#[must_use]
pub fn diff_spot<'a>(
    side: Side,
    covered: Covered,
    picked: &Picked,
    path: &'a str,
) -> CommentSpot<'a> {
    if !covered.has_line {
        return CommentSpot::Nothing;
    }
    match (side, covered.working) {
        (Side::Unstaged, working) => CommentSpot::Lines { path, working },
        (Side::Staged, None) => CommentSpot::Lines {
            path,
            working: None,
        },
        (Side::Staged, Some(_)) if picked.contiguous => CommentSpot::Text { path },
        (Side::Staged, Some(_)) => CommentSpot::Split,
    }
}

/// Why a spot cannot be commented on, if it cannot: the person reads it as it is.
pub fn spot_refusal(spot: CommentSpot<'_>) -> Result<(), &'static str> {
    match spot {
        CommentSpot::Nothing => Err("Select lines to comment on first."),
        CommentSpot::Lines { working: None, .. } => Err("Those lines are not in the working tree."),
        CommentSpot::Split => Err("Select lines of one hunk to comment on."),
        CommentSpot::Lines { .. } | CommentSpot::Text { .. } => Ok(()),
    }
}

/// What the composer writes: a new comment on the lines of `path`, or the body of
/// comment `id`, whose lines are shown with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewTarget {
    New {
        path: String,
        start_line: u32,
        end_line: u32,
    },
    /// A new comment on text picked in a surface that is not the working tree
    /// (D141): the engine looks for the text in the file now.
    NewByText { path: String, text: String },
    Edit {
        id: i64,
        path: String,
        start_line: u32,
        end_line: u32,
    },
    /// A reply on the thread of comment `id`, whose lines are shown with it (D198).
    Reply {
        id: i64,
        path: String,
        start_line: u32,
        end_line: u32,
    },
}

impl ReviewTarget {
    /// The new comment a commentable spot opens, with `text` the picked text for a spot
    /// found by its text. `None` for a spot `spot_refusal` refuses.
    #[must_use]
    pub fn for_spot(spot: CommentSpot<'_>, text: impl FnOnce() -> Option<String>) -> Option<Self> {
        match spot {
            CommentSpot::Lines {
                path,
                working: Some((start_line, end_line)),
            } => Some(ReviewTarget::New {
                path: path.to_owned(),
                start_line,
                end_line,
            }),
            CommentSpot::Text { path } => Some(ReviewTarget::NewByText {
                path: path.to_owned(),
                text: text()?,
            }),
            _ => None,
        }
    }

    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            ReviewTarget::New { path, .. }
            | ReviewTarget::NewByText { path, .. }
            | ReviewTarget::Edit { path, .. }
            | ReviewTarget::Reply { path, .. } => path,
        }
    }

    /// The lines the composer names: the target's own, or for picked text one to its length.
    #[must_use]
    pub fn lines(&self) -> (u32, u32) {
        match self {
            ReviewTarget::NewByText { text, .. } => (
                1,
                u32::try_from(text.lines().count().max(1)).unwrap_or(u32::MAX),
            ),
            ReviewTarget::New {
                start_line,
                end_line,
                ..
            }
            | ReviewTarget::Edit {
                start_line,
                end_line,
                ..
            }
            | ReviewTarget::Reply {
                start_line,
                end_line,
                ..
            } => (*start_line, *end_line),
        }
    }
}

/// The body a comment carries: trimmed, and none when it is blank.
#[must_use]
pub fn body_of(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Whether Comment or Save may be pressed: no write is in flight and the body is not blank.
#[must_use]
pub fn may_save(pending: bool, text: &str) -> bool {
    !pending && body_of(text).is_some()
}

/// The command the composer sends for its target, with `Actor::Human`. None when the
/// body is blank. `label` is the type the person chose for a new comment (D236 (7)); an
/// edit or a reply has none.
#[must_use]
pub fn command(
    target: &ReviewTarget,
    text: &str,
    label: Option<CommentLabel>,
) -> Option<CommandKind> {
    let body = body_of(text)?;
    Some(match target {
        ReviewTarget::New {
            path,
            start_line,
            end_line,
        } => CommandKind::AddReviewComment {
            path: path.clone(),
            start_line: *start_line,
            end_line: *end_line,
            body,
            label,
        },
        ReviewTarget::NewByText { path, text } => CommandKind::AddReviewCommentByText {
            path: path.clone(),
            text: text.clone(),
            body,
            label,
        },
        ReviewTarget::Edit { id, .. } => CommandKind::EditReviewComment { id: *id, body },
        ReviewTarget::Reply { id, .. } => CommandKind::ReplyToReviewComment { id: *id, body },
    })
}

/// The composer's title: what the comment is about.
#[must_use]
pub fn title(target: &ReviewTarget) -> String {
    let (start, end) = target.lines();
    let range = match target {
        ReviewTarget::NewByText { .. } if end > 1 => format!("{end} selected lines"),
        ReviewTarget::NewByText { .. } => "the selected line".to_owned(),
        _ => range_label(start, end),
    };
    match target {
        ReviewTarget::New { .. } | ReviewTarget::NewByText { .. } => format!("Comment on {range}"),
        ReviewTarget::Edit { .. } => format!("Edit comment on {range}"),
        ReviewTarget::Reply { .. } => format!("Reply on {range}"),
    }
}

/// The label of the button that writes the comment.
#[must_use]
pub fn save_label(target: &ReviewTarget) -> &'static str {
    match target {
        ReviewTarget::New { .. } | ReviewTarget::NewByText { .. } => "Comment",
        ReviewTarget::Edit { .. } => "Save",
        ReviewTarget::Reply { .. } => "Reply",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered(working: Option<(u32, u32)>) -> Covered {
        Covered {
            has_line: true,
            working,
        }
    }

    fn picked(contiguous: bool) -> Picked {
        Picked {
            lines: vec!["a".to_owned(), "b".to_owned()],
            contiguous,
        }
    }

    #[test]
    fn the_unstaged_side_is_the_working_tree_and_the_staged_side_is_found_by_text() {
        let unstaged = diff_spot(
            Side::Unstaged,
            covered(Some((4, 9))),
            &picked(false),
            "a.rs",
        );
        assert_eq!(
            unstaged,
            CommentSpot::Lines {
                path: "a.rs",
                working: Some((4, 9))
            }
        );
        let staged = diff_spot(Side::Staged, covered(Some((4, 5))), &picked(true), "a.rs");
        assert_eq!(staged, CommentSpot::Text { path: "a.rs" });
        let split = diff_spot(Side::Staged, covered(Some((4, 9))), &picked(false), "a.rs");
        assert_eq!(split, CommentSpot::Split);
        assert_eq!(
            spot_refusal(split),
            Err("Select lines of one hunk to comment on.")
        );
        let removed = diff_spot(Side::Unstaged, covered(None), &picked(true), "a.rs");
        assert_eq!(
            spot_refusal(removed),
            Err("Those lines are not in the working tree.")
        );
        let header = Covered {
            has_line: false,
            working: None,
        };
        assert_eq!(
            diff_spot(Side::Unstaged, header, &picked(true), "a.rs"),
            CommentSpot::Nothing
        );
    }

    #[test]
    fn a_new_comment_carries_the_label_chosen() {
        let target = ReviewTarget::for_spot(
            CommentSpot::Lines {
                path: "a.rs",
                working: Some((4, 9)),
            },
            || None,
        )
        .unwrap();
        assert_eq!(title(&target), "Comment on lines 4–9");
        assert_eq!(
            command(&target, "  Fail here.  ", Some(CommentLabel::Issue)),
            Some(CommandKind::AddReviewComment {
                path: "a.rs".to_owned(),
                start_line: 4,
                end_line: 9,
                body: "Fail here.".to_owned(),
                label: Some(CommentLabel::Issue),
            })
        );
        assert_eq!(command(&target, "   ", None), None);
        let by_text = ReviewTarget::for_spot(CommentSpot::Text { path: "a.rs" }, || {
            Some("x\ny".to_owned())
        })
        .unwrap();
        assert_eq!(title(&by_text), "Comment on 2 selected lines");
        assert_eq!(
            ReviewTarget::for_spot(CommentSpot::Text { path: "a.rs" }, || None),
            None
        );
    }
}
