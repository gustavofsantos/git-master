//! The dialogs of the Threads page's curation (T6): one field dialog for the
//! rename, note, tags, link and export, and one confirmation for the two deletes
//! and the scrub. The rules are in `thread_curate.rs`; this file draws the dialogs
//! and sends their commands with `Actor::Human` when the confirm button is pressed.
//! A refusal from the engine stays in its dialog. The window side is the
//! `impl StatusView` below.

use gm_schema::{Actor, CommandKind};
use gpui::{
    App, Context, Div, Entity, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    Stateful, Subscription, Window, div, prelude::*, px,
};

use crate::FocusNext;
use crate::FocusPrev;
use crate::text_area::{Newline, TextArea, TextAreaEvent};
use crate::theme;
use crate::thread_curate::{self as curate, Outcome, ThreadField, Work};
use crate::view::StatusView;
use crate::worktree_create::field_row;

const WIDTH: f32 = 520.;
/// The dialogs hang this far below the top of the window.
const TOP: f32 = 96.;

/// The field dialog while it is open: the field, the thread it edits, the text
/// and the error the last try got.
pub(crate) struct FieldDialog {
    pub(crate) field: ThreadField,
    pub(crate) uid: String,
    pub(crate) input: Entity<TextArea>,
    pub(crate) error: Option<String>,
    /// The text the dialog last saw. The field notifies on every caret blink,
    /// so an error is cleared only when the text really changed.
    seen: String,
    _subscriptions: Vec<Subscription>,
}

impl FieldDialog {
    fn text(&self, cx: &App) -> String {
        self.input.read(cx).text().to_owned()
    }
}

/// What a confirmation sends when its button is pressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConfirmTarget {
    /// Delete this thread with its entries, edits and links.
    Thread(String),
    /// Delete this entry and its edits.
    Entry(i64),
    /// Re-apply the redaction patterns to every stored thread.
    Scrub,
}

impl ConfirmTarget {
    fn work(&self) -> Work {
        match self {
            Self::Thread(_) => Work::Delete,
            Self::Entry(_) => Work::DeleteEntry,
            Self::Scrub => Work::Scrub,
        }
    }
}

/// The confirmation while it is open: the question and its lines, computed when
/// it opened, and the error the last try got.
pub(crate) struct ConfirmDialog {
    pub(crate) target: ConfirmTarget,
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
    pub(crate) error: Option<String>,
    pub(crate) focus: FocusHandle,
}

impl StatusView {
    /// Opens the field dialog of thread `uid`, with the field's current value.
    /// A thread the list no longer has gets a notice and no dialog.
    pub(crate) fn open_thread_field(
        &mut self,
        field: ThreadField,
        uid: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(summary) = self.model.threads.summary(uid).cloned() else {
            self.notify_user(
                "This thread is no longer listed. Open the Threads page again.",
                cx,
            );
            return;
        };
        let export_path = curate::default_export_path(&home_dir(), &summary.title);
        let text = curate::prefill(field, &summary, &export_path);
        let lines = if field.multiline() { (4, 12) } else { (1, 1) };
        let input =
            cx.new(|cx| TextArea::new(field.placeholder(), cx).with_lines(lines.0, lines.1));
        input.update(cx, |area, cx| area.set_text(&text, cx));
        let subscriptions = vec![
            cx.observe(&input, |this, _, cx| this.on_thread_field_edited(cx)),
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    if *event == TextAreaEvent::Cancel {
                        this.close_thread_field(window, cx);
                    }
                },
            ),
        ];
        self.thread_field = Some(FieldDialog {
            field,
            uid: uid.to_owned(),
            input: input.clone(),
            error: None,
            seen: text,
            _subscriptions: subscriptions,
        });
        input.update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn close_thread_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.thread_field.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// The text changed: the error of the last try goes, since it may be about the old text.
    fn on_thread_field_edited(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.thread_field.as_mut() else {
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

    /// Enter and the button: sends the field's command with `Actor::Human`. A
    /// text the rules refuse stays in the dialog with the reason.
    pub(crate) fn submit_thread_field(&mut self, cx: &mut Context<Self>) {
        if self.model.thread_work.is_pending() {
            return;
        }
        let Some(dialog) = self.thread_field.as_mut() else {
            return;
        };
        let text = dialog.text(cx);
        let field = dialog.field;
        // The tags the thread has now, so a tag edit sends only what changed (D145).
        let current = self
            .model
            .threads
            .summary(&dialog.uid)
            .map(|summary| summary.tags.clone())
            .unwrap_or_default();
        match curate::field_command(field, &dialog.uid, &text, &current) {
            Err(reason) => {
                dialog.error = Some(reason.to_owned());
                cx.notify();
            }
            Ok(kinds) => {
                dialog.error = None;
                // The engine answers the commands in order, so the last one settles the write.
                let mut last = None;
                for kind in kinds {
                    last = Some(self.engine.dispatch(Actor::Human, kind));
                }
                match last {
                    Some(id) => self.model.thread_work.start(id, Work::Field(field)),
                    // Nothing changed: the dialog stays open and says so, and Cancel leaves it.
                    None => dialog.error = Some("The tags are already these.".to_owned()),
                }
                cx.notify();
            }
        }
    }

    /// Opens the confirmation that deletes thread `uid`. Nothing is sent until its button is pressed.
    pub(crate) fn open_delete_thread_dialog(
        &mut self,
        uid: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(summary) = self.model.threads.summary(uid).cloned() else {
            self.notify_user(
                "This thread is no longer listed. Open the Threads page again.",
                cx,
            );
            return;
        };
        let lines = curate::delete_lines(summary.entry_count, summary.edit_count);
        self.open_confirm(
            ConfirmTarget::Thread(uid.to_owned()),
            curate::delete_title(&summary),
            lines,
            window,
            cx,
        );
    }

    /// Opens the confirmation that deletes entry `id` of the thread shown.
    pub(crate) fn open_delete_entry_dialog(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.model.threads.detail() else {
            return;
        };
        let Some(entry) = detail.entries.iter().find(|entry| entry.id == id) else {
            self.notify_user("This entry is no longer shown. Open the thread again.", cx);
            return;
        };
        let edits = detail
            .edits
            .iter()
            .filter(|edit| edit.entry_id == id)
            .count();
        let lines = curate::delete_entry_lines(entry.kind, &entry.text, edits);
        self.open_confirm(
            ConfirmTarget::Entry(id),
            "Delete this entry?".to_owned(),
            lines,
            window,
            cx,
        );
    }

    /// Opens the confirmation of a scrub of every stored thread.
    pub(crate) fn open_scrub_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let lines = curate::SCRUB_LINES
            .iter()
            .map(|line| (*line).to_owned())
            .collect();
        self.open_confirm(
            ConfirmTarget::Scrub,
            "Scrub threads?".to_owned(),
            lines,
            window,
            cx,
        );
    }

    fn open_confirm(
        &mut self,
        target: ConfirmTarget,
        title: String,
        lines: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        focus.focus(window);
        self.thread_confirm = Some(ConfirmDialog {
            target,
            title,
            lines,
            error: None,
            focus,
        });
        cx.notify();
    }

    pub(crate) fn close_thread_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.thread_confirm.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// The confirm button and Enter: sends the delete or the scrub with `Actor::Human`.
    /// A deleted thread is dropped from the selection first, so no read of it follows.
    pub(crate) fn submit_thread_confirm(&mut self, cx: &mut Context<Self>) {
        if self.model.thread_work.is_pending() {
            return;
        }
        let Some(dialog) = self.thread_confirm.as_mut() else {
            return;
        };
        dialog.error = None;
        let target = dialog.target.clone();
        let kind = match &target {
            ConfirmTarget::Thread(uid) => {
                self.model.threads.forget(uid);
                CommandKind::DeleteThread { uid: uid.clone() }
            }
            ConfirmTarget::Entry(id) => CommandKind::DeleteThreadEntry { id: *id },
            ConfirmTarget::Scrub => CommandKind::ScrubThreads,
        };
        let id = self.engine.dispatch(Actor::Human, kind);
        self.model.thread_work.start(id, target.work());
        cx.notify();
    }

    /// Runs when the engine settles a write. A success closes its dialog and says
    /// so; a refusal stays in the open dialog with the engine's message, or else
    /// goes to the status bar.
    pub(crate) fn on_thread_work(
        &mut self,
        outcome: Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Outcome::Done(work) => {
                match work {
                    Work::Field(_) => self.thread_field = None,
                    Work::Delete | Work::DeleteEntry | Work::Scrub => self.thread_confirm = None,
                }
                self.list_focus.focus(window);
                self.notify_user(curate::done_notice(work), cx);
            }
            Outcome::Exported(path) => {
                self.thread_field = None;
                self.list_focus.focus(window);
                self.notify_user(&curate::exported_notice(&path), cx);
            }
            Outcome::Scrubbed(changed) => {
                self.thread_confirm = None;
                self.list_focus.focus(window);
                self.notify_user(&curate::scrub_notice(changed), cx);
            }
            Outcome::Failed(message) => {
                if let Some(dialog) = self.thread_field.as_mut() {
                    dialog.error = Some(message);
                } else if let Some(dialog) = self.thread_confirm.as_mut() {
                    dialog.error = Some(message);
                } else {
                    self.notify_user(&message, cx);
                }
            }
        }
        cx.notify();
    }

    /// The layers of the two dialogs, or none while both are closed.
    pub(crate) fn thread_layers(&self, window: &Window, cx: &Context<Self>) -> Vec<Div> {
        self.thread_field_layer(window, cx)
            .into_iter()
            .chain(self.thread_confirm_layer(window, cx))
            .collect()
    }

    fn thread_field_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.thread_field.as_ref()?;
        let outside = dialog_backdrop(cx, StatusView::close_thread_field);
        let left = dialog_left(window);
        let panel = self
            .thread_field_panel(dialog, left, cx)
            // The dialog has one field, so Tab has nowhere else to go.
            .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
            .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation());
        // A note takes Enter as a line break; a one-line field sends on Enter.
        let panel = if dialog.field.multiline() {
            panel
        } else {
            let enter = cx.entity().downgrade();
            panel.capture_action(move |_: &Newline, _, cx| {
                let _ = enter.update(cx, StatusView::submit_thread_field);
                cx.stop_propagation();
            })
        };
        Some(div().absolute().inset_0().child(outside).child(panel))
    }

    fn thread_field_panel(&self, dialog: &FieldDialog, left: f32, cx: &Context<Self>) -> Div {
        let field = dialog.field;
        let pending = self.model.thread_work.is_working(Work::Field(field));
        panel_base(left)
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(field.title()),
            )
            .child(field_row(field.label(), dialog.input.clone()))
            .children(error_lines(dialog.error.as_deref()))
            .child(buttons(
                ("thread-field-cancel", "thread-field-confirm"),
                if pending {
                    "Saving…"
                } else {
                    field.confirm()
                },
                false,
                pending,
                StatusView::close_thread_field,
                StatusView::submit_thread_field,
                cx,
            ))
    }

    fn thread_confirm_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.thread_confirm.as_ref()?;
        let outside = dialog_backdrop(cx, StatusView::close_thread_confirm);
        let left = dialog_left(window);
        let panel = self.thread_confirm_panel(dialog, left, cx);
        Some(div().absolute().inset_0().child(outside).child(panel))
    }

    fn thread_confirm_panel(&self, dialog: &ConfirmDialog, left: f32, cx: &Context<Self>) -> Div {
        let pending = self.model.thread_work.is_pending();
        // A delete is red: the text it removes cannot come back. A scrub is accent.
        let danger = dialog.target != ConfirmTarget::Scrub;
        let label = match (pending, &dialog.target) {
            (true, _) => "Working…",
            (false, ConfirmTarget::Thread(_)) => "Delete thread",
            (false, ConfirmTarget::Entry(_)) => "Delete entry",
            (false, ConfirmTarget::Scrub) => "Scrub",
        };
        panel_base(left)
            .track_focus(&dialog.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.close_thread_confirm(window, cx),
                    "enter" => this.submit_thread_confirm(cx),
                    _ => {}
                }
            }))
            .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
            .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(dialog.title.clone()),
            )
            .children(dialog.lines.iter().map(|line| {
                div()
                    .text_sm()
                    .text_color(theme::text())
                    .child(line.clone())
            }))
            .children(error_lines(dialog.error.as_deref()))
            .child(buttons(
                ("thread-confirm-cancel", "thread-confirm-go"),
                label,
                danger,
                pending,
                StatusView::close_thread_confirm,
                StatusView::submit_thread_confirm,
                cx,
            ))
    }
}

/// The dialog's panel at `left`, before its content.
fn panel_base(left: f32) -> Div {
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
}

/// The area outside a dialog: a click there closes it.
fn dialog_backdrop(
    cx: &Context<StatusView>,
    close: fn(&mut StatusView, &mut Window, &mut Context<StatusView>),
) -> Div {
    div().absolute().size_full().occlude().on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, _: &MouseDownEvent, window, cx| close(this, window, cx)),
    )
}

/// The left edge that centres a dialog of `WIDTH` in the window.
fn dialog_left(window: &Window) -> f32 {
    let viewport = window.viewport_size();
    ((f32::from(viewport.width) - WIDTH) / 2.).max(0.)
}

/// The refusal or failure a dialog shows, one line each in error colour.
fn error_lines(error: Option<&str>) -> Option<Div> {
    error.map(|error| {
        div()
            .flex()
            .flex_col()
            .text_sm()
            .text_color(theme::error())
            .children(error.lines().map(|line| div().child(line.to_owned())))
    })
}

/// Cancel and the confirm button. The confirm button is red for a delete and
/// waits while a write runs.
fn buttons(
    ids: (&'static str, &'static str),
    label: &'static str,
    danger: bool,
    pending: bool,
    cancel: fn(&mut StatusView, &mut Window, &mut Context<StatusView>),
    confirm: fn(&mut StatusView, &mut Context<StatusView>),
    cx: &Context<StatusView>,
) -> Div {
    let colour = if danger {
        theme::error()
    } else {
        theme::accent()
    };
    let confirm_button: Stateful<Div> = div()
        .id(ids.1)
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(colour)
        .text_color(theme::on_accent())
        .font_weight(FontWeight::MEDIUM)
        .when(pending, |this| this.opacity(0.5))
        .when(!pending, gpui::Styled::cursor_pointer)
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                confirm(this, cx);
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
                .id(ids.0)
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
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cancel(this, window, cx);
                    }),
                ),
        )
        .child(confirm_button)
}

/// The home folder the export proposes its path in, from `$HOME`; empty when it is not set.
fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_confirmation_writes_what_its_target_names() {
        assert_eq!(ConfirmTarget::Thread("u".into()).work(), Work::Delete);
        assert_eq!(ConfirmTarget::Entry(3).work(), Work::DeleteEntry);
        assert_eq!(ConfirmTarget::Scrub.work(), Work::Scrub);
    }
}
