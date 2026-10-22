//! The drop dialog of a stash (B2, D94). Dropping a stash removes its changes
//! from the stash list for good, so the dialog asks first and says what is lost.
//! The rules that say what the dialog shows are pure and tested here; the window
//! side is the `impl StatusView` at the bottom. A refusal (the list moved on) is
//! shown in the dialog, with Git's or the engine's message.

use gm_schema::{Actor, CommandKind, OperationId, OperationState, StashEntry};
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
/// The line that says what dropping costs.
const LOST: &str =
    "Its changes are removed from the stash list and cannot be restored from GitMaster.";
/// The label of the destructive button.
pub(crate) const CONFIRM: &str = "Drop stash";

/// The title: which stash the drop is about, by its name.
#[must_use]
pub(crate) fn title(label: &str) -> String {
    format!("Drop {label}?")
}

/// The body: the stash's message, then what dropping it costs.
#[must_use]
pub(crate) fn lines(entry: &StashEntry) -> Vec<String> {
    vec![entry.message.clone(), LOST.to_owned()]
}

/// The notice when the drop succeeded.
#[must_use]
pub(crate) fn dropped_notice(index: u32) -> String {
    format!("Dropped {}", stashes::label(index))
}

/// The drop in flight, and its outcome until the window takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The stash at this index is gone.
    Dropped(u32),
    /// The engine refused, with the message to show.
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Dropping {
    /// The operation in flight and the index of the stash it drops.
    pending: Option<(OperationId, u32)>,
    outcome: Option<Outcome>,
}

impl Dropping {
    pub(crate) fn start(&mut self, id: OperationId, index: u32) {
        self.pending = Some((id, index));
        self.outcome = None;
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending.is_some_and(|(pending, _)| pending == id)
    }

    /// Ends the drop with the operation's terminal `state`.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, index)) = self.pending.take() else {
            return;
        };
        self.outcome = Some(if state == OperationState::Done {
            Outcome::Dropped(index)
        } else {
            let message = detail.unwrap_or_else(|| "The stash was not dropped.".to_owned());
            Outcome::Failed(message)
        });
    }

    pub(crate) fn take(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }
}

/// The dialog while it is open: the stash as the list had it when the dialog
/// opened, the focus that carries the keys and the error it shows.
pub(crate) struct DropStashDialog {
    pub(crate) entry: StashEntry,
    pub(crate) focus: FocusHandle,
    pub(crate) error: Option<String>,
}

impl StatusView {
    /// Opens the drop dialog for the listed stash `oid` at `index`. A stash the
    /// list no longer has gets a notice and no dialog.
    pub(crate) fn open_drop_stash_dialog(
        &mut self,
        index: u32,
        oid: &str,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self
            .model
            .stashes
            .entries()
            .iter()
            .find(|entry| entry.index == index && entry.oid == oid)
            .cloned()
        else {
            self.notify_user(
                &format!("{label} is no longer listed. Open the Stashes page again."),
                cx,
            );
            return;
        };
        let focus = cx.focus_handle();
        focus.focus(window);
        self.drop_stash_dialog = Some(DropStashDialog {
            entry,
            focus,
            error: None,
        });
        cx.notify();
    }

    pub(crate) fn close_drop_stash_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drop_stash_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// Enter and the drop button: sends the drop with `Actor::Human`.
    pub(crate) fn submit_drop_stash(&mut self, cx: &mut Context<Self>) {
        if self.model.stash_drop.is_pending() {
            return;
        }
        let Some(dialog) = self.drop_stash_dialog.as_mut() else {
            return;
        };
        dialog.error = None;
        let (index, oid) = (dialog.entry.index, dialog.entry.oid.clone());
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::DropStash { index, oid });
        self.model.stash_drop.start(id, index);
        cx.notify();
    }

    /// Runs when the engine settles a drop. A dropped stash closes the dialog and
    /// says so; a refusal stays in the dialog with its message.
    pub(crate) fn on_stash_dropped(
        &mut self,
        outcome: Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Outcome::Dropped(index) => {
                self.drop_stash_dialog = None;
                self.list_focus.focus(window);
                self.notify_user(&dropped_notice(index), cx);
            }
            Outcome::Failed(message) => match self.drop_stash_dialog.as_mut() {
                Some(dialog) => dialog.error = Some(message),
                None => self.notify_user(&message, cx),
            },
        }
        cx.notify();
    }

    pub(crate) fn drop_stash_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.drop_stash_dialog.as_ref()?;
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_drop_stash_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        Some(
            div().absolute().inset_0().child(outside).child(
                self.drop_stash_panel(dialog, left, cx)
                    // Tab has nothing else to reach while the dialog is open.
                    .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
                    .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation()),
            ),
        )
    }

    fn drop_stash_panel(&self, dialog: &DropStashDialog, left: f32, cx: &Context<Self>) -> Div {
        let pending = self.model.stash_drop.is_pending();
        let label = stashes::label(dialog.entry.index);
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
                    "escape" => this.close_drop_stash_dialog(window, cx),
                    "enter" => this.submit_drop_stash(cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title(&label)),
            )
            .children(
                lines(&dialog.entry)
                    .into_iter()
                    .map(|line| div().text_sm().text_color(theme::text()).child(line)),
            )
            .children(dialog.error.as_ref().map(|error| {
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(error.clone())
            }))
            .child(buttons(pending, cx))
    }
}

/// Cancel, and the destructive drop button, which waits while a drop runs.
fn buttons(pending: bool, cx: &Context<StatusView>) -> Div {
    let label = if pending { "Dropping…" } else { CONFIRM };
    let confirm = div()
        .id("drop-stash-confirm")
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::error())
        .text_color(theme::on_accent())
        .font_weight(FontWeight::MEDIUM)
        .when(pending, |this| this.opacity(0.5))
        .when(!pending, gpui::Styled::cursor_pointer)
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.submit_drop_stash(cx);
            }),
        );
    div()
        .flex()
        .flex_row()
        .justify_end()
        .items_center()
        .gap_2()
        .child(
            div()
                .id("drop-stash-cancel")
                .px_3()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .text_color(theme::text())
                .child("Cancel")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_drop_stash_dialog(window, cx);
                    }),
                ),
        )
        .child(confirm)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(index: u32, message: &str) -> StashEntry {
        StashEntry {
            index,
            oid: "a".repeat(40),
            message: message.into(),
            branch: Some("main".into()),
            time: 1_000,
        }
    }

    #[test]
    fn the_title_names_the_stash_and_the_body_gives_its_message_and_the_cost() {
        assert_eq!(title("stash@{2}"), "Drop stash@{2}?");
        assert_eq!(lines(&entry(2, "On main: wip")), ["On main: wip", LOST]);
        assert!(LOST.contains("cannot be restored"));
    }

    #[test]
    fn the_notice_names_the_stash_that_was_dropped() {
        assert_eq!(dropped_notice(0), "Dropped stash@{0}");
        assert_eq!(dropped_notice(3), "Dropped stash@{3}");
    }

    #[test]
    fn a_drop_is_tracked_only_while_its_operation_runs() {
        let mut dropping = Dropping::default();
        dropping.start(OperationId(4), 1);
        assert!(dropping.is_pending());
        assert!(!dropping.owns(OperationId(3)));
        assert!(dropping.owns(OperationId(4)));
        dropping.settle(OperationState::Executing, None);
        assert!(dropping.is_pending());
        assert_eq!(dropping.take(), None);
        dropping.settle(OperationState::Done, None);
        assert_eq!(dropping.take(), Some(Outcome::Dropped(1)));
        assert!(!dropping.is_pending());
        assert_eq!(dropping.take(), None);
    }

    #[test]
    fn a_refused_drop_keeps_the_engines_message() {
        let mut dropping = Dropping::default();
        dropping.start(OperationId(5), 0);
        dropping.settle(
            OperationState::Failed,
            Some(gm_schema::STASH_LIST_CHANGED.into()),
        );
        assert_eq!(
            dropping.take(),
            Some(Outcome::Failed(gm_schema::STASH_LIST_CHANGED.to_owned()))
        );
    }

    #[test]
    fn a_failure_without_a_reason_says_the_stash_was_not_dropped() {
        let mut dropping = Dropping::default();
        dropping.start(OperationId(6), 0);
        dropping.settle(OperationState::Vetoed, None);
        assert_eq!(
            dropping.take(),
            Some(Outcome::Failed("The stash was not dropped.".to_owned()))
        );
    }
}
