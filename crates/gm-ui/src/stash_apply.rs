//! Apply and pop of a stash (B2, D94). Both send at once, with no dialog: a
//! success is a notice on the status bar, and a failure opens a message that
//! shows Git's text, since a conflict is several lines and the status bar holds
//! one. The rules that say what is shown are pure and tested here; the window
//! side is the `impl StatusView` at the bottom.

use gm_schema::{CommandKind, OperationId, OperationState};
use gpui::{
    Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, Window, div,
    prelude::*, px,
};

use crate::FocusNext;
use crate::FocusPrev;
use crate::stashes;
use crate::theme;
use crate::view::StatusView;

const WIDTH: f32 = 520.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;
/// How many lines of Git's text the message shows before it says how many are left.
pub(crate) const SHOWN_LINES: usize = 15;

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

/// The first `SHOWN_LINES` lines of Git's text, and how many lines follow them.
#[must_use]
pub(crate) fn excerpt(text: &str) -> (Vec<&str>, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let left = lines.len().saturating_sub(SHOWN_LINES);
    (lines.into_iter().take(SHOWN_LINES).collect(), left)
}

/// The line under the text that counts the lines left out, when there are any.
#[must_use]
pub(crate) fn more_label(left: usize) -> Option<String> {
    match left {
        0 => None,
        1 => Some("and 1 more line".to_owned()),
        n => Some(format!("and {n} more lines")),
    }
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

/// The message while it is open: its title, Git's text and the focus that
/// carries Enter and Escape.
pub(crate) struct FailureDialog {
    pub(crate) title: String,
    pub(crate) text: String,
    pub(crate) focus: FocusHandle,
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
                let focus = cx.focus_handle();
                focus.focus(window);
                self.stash_failure = Some(FailureDialog {
                    title: failure_title(kind, index),
                    text,
                    focus,
                });
                cx.notify();
            }
        }
    }

    pub(crate) fn close_stash_failure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stash_failure.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// The message's layer, or nothing while it is closed.
    pub(crate) fn stash_failure_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.stash_failure.as_ref()?;
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_stash_failure(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        Some(
            div().absolute().inset_0().child(outside).child(
                failure_panel(dialog, left, cx)
                    // The message has one button, so Tab has nowhere to go.
                    .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
                    .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation()),
            ),
        )
    }
}

fn failure_panel(dialog: &FailureDialog, left: f32, cx: &Context<StatusView>) -> Div {
    let (shown, left_out) = excerpt(&dialog.text);
    div()
        .absolute()
        .top(px(TOP))
        .left(px(left))
        .w(px(WIDTH))
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .bg(theme::panel())
        .border_1()
        .border_color(theme::hairline_strong())
        .rounded_md()
        .shadow_lg()
        .occlude()
        .track_focus(&dialog.focus)
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            match event.keystroke.key.as_str() {
                "escape" | "enter" => this.close_stash_failure(window, cx),
                _ => {}
            }
        }))
        .child(
            div()
                .text_color(theme::text())
                .font_weight(FontWeight::SEMIBOLD)
                .child(dialog.title.clone()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .p_2()
                .rounded_sm()
                .bg(theme::ground())
                .font_family(theme::code_font())
                .text_xs()
                .text_color(theme::text())
                .children(shown.into_iter().map(|line| div().child(line.to_owned())))
                .children(
                    more_label(left_out)
                        .map(|label| div().pt_1().text_color(theme::muted()).child(label)),
                ),
        )
        .child(
            div().flex().flex_row().justify_end().child(
                div()
                    .id("stash-failure-ok")
                    .px_3()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .bg(theme::accent())
                    .text_color(theme::on_accent())
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .child("OK")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.close_stash_failure(window, cx);
                        }),
                    ),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(count: usize) -> String {
        (1..=count)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

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
    fn a_text_of_fifteen_lines_or_fewer_is_shown_whole() {
        let text = lines_of(SHOWN_LINES);
        let (shown, left) = excerpt(&text);
        assert_eq!(shown.len(), SHOWN_LINES);
        assert_eq!(left, 0);
        assert_eq!(more_label(left), None);
    }

    #[test]
    fn a_longer_text_is_cut_at_fifteen_lines_and_counts_the_rest() {
        let text = lines_of(18);
        let (shown, left) = excerpt(&text);
        assert_eq!(shown.len(), SHOWN_LINES);
        assert_eq!(shown.last(), Some(&"line 15"));
        assert_eq!(left, 3);
        assert_eq!(more_label(left).as_deref(), Some("and 3 more lines"));
        assert_eq!(more_label(1).as_deref(), Some("and 1 more line"));
    }

    #[test]
    fn git_conflict_lines_are_shown_as_git_wrote_them() {
        let text = "CONFLICT (content): Merge conflict in a.txt\nThe stash entry is kept in case you need it again.";
        let (shown, left) = excerpt(text);
        assert_eq!(
            shown,
            [
                "CONFLICT (content): Merge conflict in a.txt",
                "The stash entry is kept in case you need it again."
            ]
        );
        assert_eq!(left, 0);
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
