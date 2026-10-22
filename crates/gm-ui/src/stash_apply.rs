//! Apply and pop of a stash (B2, D94). Both send at once, with no dialog: a
//! success is a notice on the status bar, and a failure opens a message that
//! shows Git's text, since a conflict is several lines and the status bar holds
//! one. The rules that say what is shown are pure and tested here; the window
//! side is the `impl StatusView` at the bottom.

use gm_schema::{CommandKind, OperationId, OperationState};
use gpui::{Context, Window};

use crate::stashes;
use crate::view::StatusView;

/// Which of the two operations ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Apply,
    Pop,
}

impl Kind {
    /// The verb of the failure's title: "Could not apply" or "Could not pop".
    fn verb(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::Pop => "pop",
        }
    }

    /// The past tense of the notice: "Applied" or "Popped".
    fn past(self) -> &'static str {
        match self {
            Self::Apply => "Applied",
            Self::Pop => "Popped",
        }
    }

    /// The participle of the fallback text: "applied" or "popped".
    fn participle(self) -> &'static str {
        match self {
            Self::Apply => "applied",
            Self::Pop => "popped",
        }
    }
}

/// The apply or pop that the engine command is, with the index it names. Other
/// commands are not tracked here.
#[must_use]
pub(crate) fn tracked(kind: &CommandKind) -> Option<(Kind, u32)> {
    match kind {
        CommandKind::ApplyStash { index, .. } => Some((Kind::Apply, *index)),
        CommandKind::PopStash { index, .. } => Some((Kind::Pop, *index)),
        _ => None,
    }
}

/// The notice when the stash was applied or popped: "Applied stash@{0}".
#[must_use]
pub(crate) fn done_notice(kind: Kind, index: u32) -> String {
    format!("{} {}", kind.past(), stashes::label(index))
}

/// The title of the message when the stash was not applied or popped.
#[must_use]
pub(crate) fn failure_title(kind: Kind, index: u32) -> String {
    format!("Could not {} {}", kind.verb(), stashes::label(index))
}

/// The text the message shows when the engine gave no reason.
fn fallback_text(kind: Kind) -> String {
    format!("The stash was not {}.", kind.participle())
}

/// The outcome of an apply or pop, until the window takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Done {
        kind: Kind,
        index: u32,
    },
    /// Git's or the engine's text for the refusal or the conflict.
    Failed {
        kind: Kind,
        index: u32,
        text: String,
    },
}

/// The apply or pop in flight, and its outcome until the window takes it.
/// Engine events for other operations never touch it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Applying {
    /// The operation in flight, what it does and the index it names.
    pending: Option<(OperationId, Kind, u32)>,
    outcome: Option<Outcome>,
}

impl Applying {
    pub(crate) fn start(&mut self, id: OperationId, kind: Kind, index: u32) {
        self.pending = Some((id, kind, index));
        self.outcome = None;
    }

    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending.is_some_and(|(pending, ..)| pending == id)
    }

    /// Ends the operation with its terminal `state`. Anything but Done is a
    /// failure, with the engine's message or a plain one.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, kind, index)) = self.pending.take() else {
            return;
        };
        self.outcome = Some(if state == OperationState::Done {
            Outcome::Done { kind, index }
        } else {
            Outcome::Failed {
                kind,
                index,
                text: detail.unwrap_or_else(|| fallback_text(kind)),
            }
        });
    }

    pub(crate) fn take(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }
}

impl StatusView {
    /// Runs when the engine settles an apply or a pop. A success is a notice;
    /// a failure opens the message that shows Git's text.
    pub(crate) fn on_stash_applied(
        &mut self,
        outcome: Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Outcome::Done { kind, index } => self.notify_user(&done_notice(kind, index), cx),
            Outcome::Failed { kind, index, text } => {
                self.open_message(failure_title(kind, index), text, window, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_of_apply_and_pop_names_the_index_and_nothing_else_is_tracked() {
        let apply = CommandKind::ApplyStash {
            index: 2,
            oid: "a".repeat(40),
        };
        let pop = CommandKind::PopStash {
            index: 0,
            oid: "b".repeat(40),
        };
        let drop = CommandKind::DropStash {
            index: 1,
            oid: "c".repeat(40),
        };
        assert_eq!(tracked(&apply), Some((Kind::Apply, 2)));
        assert_eq!(tracked(&pop), Some((Kind::Pop, 0)));
        assert_eq!(tracked(&drop), None);
        assert_eq!(tracked(&CommandKind::ListStashes), None);
    }

    #[test]
    fn the_notice_and_the_title_name_the_stash_and_the_operation() {
        assert_eq!(done_notice(Kind::Apply, 0), "Applied stash@{0}");
        assert_eq!(done_notice(Kind::Pop, 3), "Popped stash@{3}");
        assert_eq!(failure_title(Kind::Apply, 1), "Could not apply stash@{1}");
        assert_eq!(failure_title(Kind::Pop, 1), "Could not pop stash@{1}");
    }

    #[test]
    fn an_apply_or_pop_is_tracked_only_while_its_operation_runs() {
        let mut applying = Applying::default();
        applying.start(OperationId(7), Kind::Pop, 1);
        assert!(!applying.owns(OperationId(6)));
        assert!(applying.owns(OperationId(7)));
        applying.settle(OperationState::Executing, None);
        assert!(applying.owns(OperationId(7)));
        assert_eq!(applying.take(), None);
        applying.settle(OperationState::Done, None);
        assert!(!applying.owns(OperationId(7)));
        assert_eq!(
            applying.take(),
            Some(Outcome::Done {
                kind: Kind::Pop,
                index: 1
            })
        );
        assert_eq!(applying.take(), None);
    }

    #[test]
    fn a_conflict_keeps_gits_text_for_the_message() {
        let mut applying = Applying::default();
        applying.start(OperationId(8), Kind::Apply, 0);
        let git = "CONFLICT (content): Merge conflict in a.txt".to_owned();
        applying.settle(OperationState::Failed, Some(git.clone()));
        assert_eq!(
            applying.take(),
            Some(Outcome::Failed {
                kind: Kind::Apply,
                index: 0,
                text: git,
            })
        );
    }

    #[test]
    fn a_stale_refusal_is_a_failure_with_the_engines_message() {
        let mut applying = Applying::default();
        applying.start(OperationId(9), Kind::Pop, 2);
        applying.settle(
            OperationState::Failed,
            Some(gm_schema::STASH_LIST_CHANGED.into()),
        );
        assert_eq!(
            applying.take(),
            Some(Outcome::Failed {
                kind: Kind::Pop,
                index: 2,
                text: gm_schema::STASH_LIST_CHANGED.to_owned(),
            })
        );
    }

    #[test]
    fn a_failure_without_a_reason_says_the_stash_was_not_applied_or_popped() {
        let mut applying = Applying::default();
        applying.start(OperationId(10), Kind::Apply, 0);
        applying.settle(OperationState::Cancelled, None);
        assert_eq!(
            applying.take(),
            Some(Outcome::Failed {
                kind: Kind::Apply,
                index: 0,
                text: "The stash was not applied.".to_owned(),
            })
        );
    }
}
