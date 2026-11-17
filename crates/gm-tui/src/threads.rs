//! Review threads in the companion (D236): the box that writes a comment or a reply, and
//! which threads are unfolded. The rules come from `gm_view`: where picked lines can be
//! commented (`compose`), the command a comment sends (`compose::command`), and how a
//! thread folds (`review`). The same box writes a commit message.

use gm_schema::{CommandKind, CommentLabel, OperationId};
use gm_view::compose::{self, ReviewTarget};

/// The types a person picks for a new comment, as chips, with the label each stands for
/// (D236 (7)).
pub const TYPES: [(&str, CommentLabel); 3] = [
    ("change", CommentLabel::Issue),
    ("question", CommentLabel::Question),
    ("nit", CommentLabel::Nitpick),
];

/// What the box writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Review(ReviewTarget),
    /// The message of a commit of what is staged.
    Commit,
}

/// The box that writes a new comment, a reply or a commit message.
#[derive(Debug)]
pub struct Composer {
    pub target: Target,
    pub text: String,
    /// Index into `TYPES`; a reply has no type.
    pub kind: usize,
    /// The write in flight, once sent.
    pub pending: Option<OperationId>,
    /// Why the last write was refused.
    pub error: Option<String>,
    /// `Esc` was pressed once on a box that holds text: a second press discards it.
    pub discarding: bool,
}

impl Composer {
    pub fn new(target: ReviewTarget) -> Self {
        Self::of(Target::Review(target))
    }

    /// The box for a commit message.
    pub fn commit() -> Self {
        Self::of(Target::Commit)
    }

    fn of(target: Target) -> Self {
        Self {
            target,
            text: String::new(),
            kind: 0,
            pending: None,
            error: None,
            discarding: false,
        }
    }

    /// Whether the box offers the type chips: only a new comment has a type.
    pub fn typed(&self) -> bool {
        matches!(
            self.target,
            Target::Review(ReviewTarget::New { .. } | ReviewTarget::NewByText { .. })
        )
    }

    /// The title of the box.
    pub fn title(&self) -> String {
        match &self.target {
            Target::Review(target) => compose::title(target),
            Target::Commit => "Commit the staged changes".to_owned(),
        }
    }

    /// What `Ctrl+S` does, in lower case.
    pub fn save_label(&self) -> String {
        match &self.target {
            Target::Review(target) => compose::save_label(target).to_lowercase(),
            Target::Commit => "commit".to_owned(),
        }
    }

    pub fn label(&self) -> Option<CommentLabel> {
        self.typed().then(|| TYPES[self.kind % TYPES.len()].1)
    }

    pub fn next_type(&mut self) {
        self.kind = (self.kind + 1) % TYPES.len();
    }

    pub fn type_text(&mut self, text: &str) {
        self.text.push_str(text);
        self.discarding = false;
    }

    pub fn backspace(&mut self) {
        self.text.pop();
        self.discarding = false;
    }

    /// Whether the save button works now.
    pub fn may_save(&self) -> bool {
        compose::may_save(self.pending.is_some(), &self.text)
    }

    /// The command to send, once; `None` while one is in flight or with a blank body.
    pub fn save(&self) -> Option<CommandKind> {
        if !self.may_save() {
            return None;
        }
        match &self.target {
            Target::Review(target) => compose::command(target, &self.text, self.label()),
            // Git strips the trailing blank lines; the leading ones would make an empty subject.
            Target::Commit => Some(CommandKind::Commit {
                message: self.text.trim().to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_comment() -> Composer {
        Composer::new(ReviewTarget::New {
            path: "a.rs".to_owned(),
            start_line: 4,
            end_line: 8,
        })
    }

    #[test]
    fn a_new_comment_sends_its_type_as_its_label() {
        let mut composer = new_comment();
        assert_eq!(composer.save(), None);
        composer.type_text("Fail with KeyConflict.");
        composer.next_type();
        assert_eq!(
            composer.save(),
            Some(CommandKind::AddReviewComment {
                path: "a.rs".to_owned(),
                start_line: 4,
                end_line: 8,
                body: "Fail with KeyConflict.".to_owned(),
                label: Some(CommentLabel::Question),
            })
        );
        composer.pending = Some(OperationId(3));
        assert_eq!(composer.save(), None);
    }

    #[test]
    fn a_commit_message_is_sent_trimmed_and_has_no_type() {
        let mut composer = Composer::commit();
        assert!(!composer.typed());
        assert_eq!(composer.save(), None);
        composer.type_text("\nfix: keep the cursor\n\nBody.\n\n");
        assert_eq!(
            composer.save(),
            Some(CommandKind::Commit {
                message: "fix: keep the cursor\n\nBody.".to_owned(),
            })
        );
        assert_eq!(composer.save_label(), "commit");
    }

    #[test]
    fn a_reply_has_no_type() {
        let mut composer = Composer::new(ReviewTarget::Reply {
            id: 7,
            path: "a.rs".to_owned(),
            start_line: 1,
            end_line: 1,
        });
        assert!(!composer.typed());
        composer.type_text("Use ParseError.");
        assert_eq!(
            composer.save(),
            Some(CommandKind::ReplyToReviewComment {
                id: 7,
                body: "Use ParseError.".to_owned(),
            })
        );
    }
}
