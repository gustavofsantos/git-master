//! The stash dialog (B2, D94). It stashes the working copy's changes under an
//! optional message, and optionally the untracked files too. The rules are pure
//! and tested here: the message the command carries, the command line it shows,
//! whether Stash may be pressed, and what an outcome means. The window side is
//! the `impl StatusView` at the bottom. Git's refusal (nothing to stash) shows
//! in the dialog.

use std::fmt::Write as _;

use gm_schema::{Actor, CommandKind, OperationId, OperationState};
use gpui::{
    Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, Stateful, Subscription, Window,
    div, prelude::*, px,
};

use crate::FocusNext;
use crate::new_branch::checkbox;
use crate::text_area::{Newline, TextArea, TextAreaEvent};
use crate::theme;
use crate::view::StatusView;
use crate::worktree_create::{command_block, field_row};

const WIDTH: f32 = 520.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;
/// The notice when a stash is made without a message.
const DONE: &str = "Stashed changes";

/// The message the stash carries: trimmed, and none when it is blank.
#[must_use]
pub(crate) fn message_of(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The command the dialog sends.
#[must_use]
pub(crate) fn command(text: &str, include_untracked: bool) -> CommandKind {
    CommandKind::CreateStash {
        message: message_of(text),
        include_untracked,
    }
}

/// The Git command the dialog will run, shown under the fields.
#[must_use]
pub(crate) fn command_line(text: &str, include_untracked: bool) -> String {
    let mut line = "git stash push".to_owned();
    if include_untracked {
        line.push_str(" --include-untracked");
    }
    if let Some(message) = message_of(text) {
        let _ = write!(line, " -m {message:?}");
    }
    line
}

/// Whether Stash may be pressed: no create is in flight.
#[must_use]
pub(crate) fn may_stash(pending: bool) -> bool {
    !pending
}

/// The create in flight, and its outcome until the window takes it. Engine
/// events for other operations never touch it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StashCreate {
    pending: Option<OperationId>,
    outcome: Option<Result<(), String>>,
}

impl StashCreate {
    pub(crate) fn start(&mut self, id: OperationId) {
        self.pending = Some(id);
        self.outcome = None;
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending == Some(id)
    }

    /// Ends the create with the operation's terminal `state`. A stash is
    /// Ok; anything else gives Git's message, or a plain one.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() || self.pending.take().is_none() {
            return;
        }
        self.outcome = Some(if state == OperationState::Done {
            Ok(())
        } else {
            Err(detail.unwrap_or_else(|| "The changes were not stashed.".to_owned()))
        });
    }

    pub(crate) fn take(&mut self) -> Option<Result<(), String>> {
        self.outcome.take()
    }
}

/// The dialog while it is open: the message field, whether untracked files go
/// too, and the error it shows.
pub(crate) struct NewStashDialog {
    pub(crate) message: Entity<TextArea>,
    pub(crate) include_untracked: bool,
    /// Git's refusal, or the engine's.
    pub(crate) error: Option<String>,
    /// The text the dialog last saw. The field notifies on every caret blink,
    /// so an error is cleared only when the text really changed.
    seen: String,
    _subscriptions: Vec<Subscription>,
}

impl NewStashDialog {
    fn text(&self, cx: &gpui::App) -> String {
        self.message.read(cx).text().to_owned()
    }
}

impl StatusView {
    /// Opens the dialog with an empty message and untracked files left out.
    pub(crate) fn open_new_stash_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = cx.new(|cx| TextArea::new("Message (optional)", cx).with_lines(1, 1));
        let subscriptions = vec![
            cx.observe(&message, |this, _, cx| this.on_new_stash_text(cx)),
            cx.subscribe_in(
                &message,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    if *event == TextAreaEvent::Cancel {
                        this.close_new_stash_dialog(window, cx);
                    }
                },
            ),
        ];
        let seen = message.read(cx).text().to_owned();
        self.new_stash_dialog = Some(NewStashDialog {
            message: message.clone(),
            include_untracked: false,
            error: None,
            seen,
            _subscriptions: subscriptions,
        });
        message.update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn close_new_stash_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.new_stash_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// The text changed: the error of the last try goes, since it may be about the old text.
    fn on_new_stash_text(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.new_stash_dialog.as_mut() else {
            return;
        };
        let text = dialog.text(cx);
        if dialog.seen == text {
            return;
        }
        dialog.seen = text;
        dialog.error = None;
        cx.notify();
    }

    /// The checkbox: ticks or unticks "Include untracked files".
    pub(crate) fn toggle_new_stash_untracked(&mut self, cx: &mut Context<Self>) {
        if self.model.stash_create.is_pending() {
            return;
        }
        if let Some(dialog) = self.new_stash_dialog.as_mut() {
            dialog.include_untracked = !dialog.include_untracked;
            cx.notify();
        }
    }

    /// Enter and the Stash button: sends the create with `Actor::Human`.
    pub(crate) fn submit_new_stash(&mut self, cx: &mut Context<Self>) {
        if !may_stash(self.model.stash_create.is_pending()) {
            return;
        }
        let Some(dialog) = self.new_stash_dialog.as_mut() else {
            return;
        };
        dialog.error = None;
        let kind = command(&dialog.text(cx), dialog.include_untracked);
        let id = self.engine.dispatch(Actor::Human, kind);
        self.model.stash_create.start(id);
        cx.notify();
    }

    /// Runs when the engine settles a create: success closes the dialog and
    /// says so; a failure stays in the dialog with Git's message.
    pub(crate) fn on_stash_created(
        &mut self,
        outcome: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(()) => {
                self.new_stash_dialog = None;
                self.list_focus.focus(window);
                self.notify_user(DONE, cx);
            }
            Err(message) => match self.new_stash_dialog.as_mut() {
                Some(dialog) => dialog.error = Some(message),
                None => self.notify_user(&message, cx),
            },
        }
        cx.notify();
    }

    /// The dialog's layer, or nothing while it is closed.
    pub(crate) fn new_stash_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.new_stash_dialog.as_ref()?;
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_new_stash_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        let panel = self.new_stash_panel(dialog, left, cx);
        let enter = cx.entity().downgrade();
        Some(
            div().absolute().inset_0().child(outside).child(
                panel
                    // The dialog has one field, so Tab has nowhere else to go.
                    .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
                    .capture_action(move |_: &Newline, _, cx| {
                        let _ = enter.update(cx, StatusView::submit_new_stash);
                        cx.stop_propagation();
                    }),
            ),
        )
    }

    fn new_stash_panel(&self, dialog: &NewStashDialog, left: f32, cx: &Context<Self>) -> Div {
        let pending = self.model.stash_create.is_pending();
        let text = dialog.text(cx);
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
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Stash changes"),
            )
            .child(field_row("MESSAGE", dialog.message.clone()))
            .child(untracked_row(dialog.include_untracked, cx))
            .child(command_block(&command_line(
                &text,
                dialog.include_untracked,
            )))
            .children(dialog.error.as_ref().map(|error| {
                div()
                    .flex()
                    .flex_col()
                    .text_sm()
                    .text_color(theme::error())
                    .children(error.lines().map(|line| div().child(line.to_owned())))
            }))
            .child(buttons(pending, cx))
    }
}

/// The checkbox that adds the untracked files to the stash.
fn untracked_row(checked: bool, cx: &Context<StatusView>) -> Stateful<Div> {
    checkbox(
        "new-stash-untracked",
        "Include untracked files",
        checked,
        StatusView::toggle_new_stash_untracked,
        cx,
    )
}

/// Cancel and the Stash button; the button waits while a create runs.
fn buttons(pending: bool, cx: &Context<StatusView>) -> Div {
    let stash: Stateful<Div> = div()
        .id("new-stash-create")
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::accent())
        .text_color(theme::on_accent())
        .font_weight(FontWeight::MEDIUM)
        .when(pending, |this| this.opacity(0.5))
        .when(may_stash(pending), gpui::Styled::cursor_pointer)
        .child(if pending { "Stashing…" } else { "Stash" })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.submit_new_stash(cx);
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
                .id("new-stash-cancel")
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
                        this.close_new_stash_dialog(window, cx);
                    }),
                ),
        )
        .child(stash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_message_is_no_message_and_the_rest_is_trimmed() {
        assert_eq!(message_of(""), None);
        assert_eq!(message_of("   \t"), None);
        assert_eq!(message_of("  wip: login  "), Some("wip: login".to_owned()));
    }

    #[test]
    fn the_command_carries_the_message_and_the_untracked_choice() {
        assert_eq!(
            command("  wip ", true),
            CommandKind::CreateStash {
                message: Some("wip".into()),
                include_untracked: true,
            }
        );
        assert_eq!(
            command("", false),
            CommandKind::CreateStash {
                message: None,
                include_untracked: false,
            }
        );
    }

    #[test]
    fn the_command_line_shows_what_git_will_run() {
        assert_eq!(command_line("", false), "git stash push");
        assert_eq!(command_line("", true), "git stash push --include-untracked");
        assert_eq!(command_line(" wip ", false), "git stash push -m \"wip\"");
        assert_eq!(
            command_line("say \"hi\"", true),
            "git stash push --include-untracked -m \"say \\\"hi\\\"\""
        );
    }

    #[test]
    fn stash_waits_only_while_no_create_is_in_flight() {
        assert!(may_stash(false));
        assert!(!may_stash(true));
    }

    #[test]
    fn stashing_keeps_only_its_own_operation_and_says_when_it_is_done() {
        let mut create = StashCreate::default();
        assert!(!create.is_pending());
        create.start(OperationId(4));
        assert!(create.is_pending());
        assert!(!create.owns(OperationId(3)));
        assert!(create.owns(OperationId(4)));
        create.settle(OperationState::Done, None);
        assert!(!create.is_pending());
        assert_eq!(create.take(), Some(Ok(())));
        assert_eq!(create.take(), None);
    }

    #[test]
    fn a_refused_stash_keeps_gits_message_or_says_it_was_not_stashed() {
        let mut create = StashCreate::default();
        create.start(OperationId(4));
        create.settle(
            OperationState::Failed,
            Some("No local changes to save".into()),
        );
        assert_eq!(
            create.take(),
            Some(Err("No local changes to save".to_owned()))
        );
        create.start(OperationId(5));
        create.settle(OperationState::Vetoed, None);
        assert_eq!(
            create.take(),
            Some(Err("The changes were not stashed.".to_owned()))
        );
    }

    #[test]
    fn an_operation_that_has_not_ended_settles_nothing() {
        let mut create = StashCreate::default();
        create.start(OperationId(4));
        create.settle(OperationState::Executing, None);
        assert!(create.is_pending());
        assert_eq!(create.take(), None);
    }
}
