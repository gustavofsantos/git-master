//! The commit box at the bottom of the left column: the message, its subject
//! counter and hint, and the Commit button under it.

use gpui::{
    Context, Div, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, Stateful, div,
    prelude::*, px,
};

use crate::action_registry::{Action, Args};
use crate::composer;
use crate::gate_wait;
use crate::name_fit::{self, name_label};
use crate::resize::{self, COMMIT};
use crate::resize_handle::{self, Edge};
use crate::theme;
use crate::transform_run::{self, Field};
use crate::view::StatusView;
use crate::window_chrome;

/// The link that stops the gates a waiting operation runs (`WF3c`). Shared by every surface that
/// shows the wait.
pub(crate) fn gate_cancel_link(cx: &Context<StatusView>) -> Stateful<Div> {
    div()
        .id("gate-cancel")
        .flex_none()
        .text_sm()
        .text_color(theme::accent())
        .cursor_pointer()
        .child(gate_wait::CANCEL)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| this.cancel_gate(cx)),
        )
}

/// The link that cancels a waiting commit message transform (`WF3d`), beside its waiting line.
fn suggest_cancel_link(cx: &Context<StatusView>) -> Stateful<Div> {
    div()
        .id("suggest-cancel")
        .flex_none()
        .text_sm()
        .text_color(theme::accent())
        .cursor_pointer()
        .child(transform_run::CANCEL)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| this.cancel_suggestion(cx)),
        )
}

/// The subject length a commit should not pass.
const SUBJECT_LIMIT: usize = 72;

/// A small outlined button in the commit box that runs `action` with no target.
fn chip_button(
    id: &'static str,
    label: &'static str,
    action: Action,
    cx: &mut Context<StatusView>,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .px_2()
        .h(px(18.))
        .flex()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(theme::hairline_strong())
        .text_color(theme::text())
        .cursor_pointer()
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.run_action(action, &Args::None, window, cx);
            }),
        )
}

/// The commit button under the message box: the dark primary button when enabled, the panel with
/// muted text and an outline when not, so the label stays readable. A click commits.
fn commit_button(branch: &str, enabled: bool, cx: &mut Context<StatusView>) -> Stateful<Div> {
    let muted = theme::muted();
    // Enabled: the dark primary button. Disabled: the panel with muted text
    // and an outline, so the label stays readable.
    let (button_fill, button_text, button_edge) = if enabled {
        (theme::text(), theme::ground(), theme::text())
    } else {
        (theme::panel(), muted, theme::hairline_strong())
    };
    let full = branch.to_owned();
    div()
        .id("commit-button")
        .w_full()
        .when(name_fit::needs_hover(&full), |this| {
            this.tooltip(window_chrome::hint(full))
        })
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(button_edge)
        .bg(button_fill)
        .text_color(button_text)
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.run_action(Action::Commit, &Args::None, window, cx);
            }),
        )
        .child(commit_label(branch))
}

impl StatusView {
    /// The commit box, with its message box `height` tall. Its top edge is the
    /// strip that resizes the message box.
    pub(crate) fn commit_block(&self, height: f32, cx: &mut Context<Self>) -> Div {
        let muted = theme::muted();
        let text = self.composer.read(cx).text(cx);
        let subject = composer::subject_len(&text);
        let hint = composer::subject_hint(&text);
        let counter_colour = if subject > SUBJECT_LIMIT {
            theme::error()
        } else {
            muted
        };
        let branch = self
            .model
            .status()
            .and_then(|status| status.branch.head.as_deref())
            .unwrap_or("detached HEAD")
            .to_owned();
        let enabled = self.commit_enabled(&text);
        let button = commit_button(&branch, enabled, cx);
        let height_strip = resize_handle::handle(
            "commit-height-resize",
            Edge::Top,
            height,
            COMMIT,
            |this: &mut StatusView, height, _| this.commit_height = height,
            |this: &mut StatusView, _| this.commit_height = resize::COMMIT_DEFAULT,
            cx,
        );
        div()
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(theme::panel())
            .border_t_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .text_color(muted)
                    .child("Message")
                    .child(self.suggest_chip(cx))
                    .child(
                        div()
                            .flex_none()
                            .font_family(theme::code_font())
                            .text_color(counter_colour)
                            .child(format!("{subject}/{SUBJECT_LIMIT}")),
                    ),
            )
            // Typing does not notify this view, so every key asks for a redraw
            // in which the counter and hint are read again.
            .child(
                div()
                    .h(px(height))
                    .capture_key_down(cx.listener(|_, _: &KeyDownEvent, _, cx| cx.notify()))
                    .child(self.composer.clone()),
            )
            .child(
                div()
                    .h(px(18.))
                    .text_color(theme::warn_fg())
                    .child(hint.unwrap_or("")),
            )
            .children(self.workflow_line())
            .children(self.suggestion_row(cx))
            .children(self.gate_line(cx))
            .children(self.suggest_line(cx))
            .child(button)
            .child(height_strip)
    }

    /// The Suggest button in the message header, when a commit message transform is loaded
    /// (`WF3d`). It shares the header row, so nothing below it moves.
    fn suggest_chip(&self, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .overflow_hidden();
        if transform_run::loaded(self.model.workflows.listed.as_deref(), Field::CommitMessage) {
            row = row.child(chip_button(
                "suggest-message",
                "Suggest message",
                Action::SuggestMessage,
                cx,
            ));
        }
        row
    }

    /// The commit waits for its workflow gates (`WF3c`): a line above the button, with the link
    /// that cancels the wait.
    fn gate_line(&self, cx: &Context<Self>) -> Option<Div> {
        self.model.commit_waits_for_gates().then(|| {
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(div().text_color(theme::muted()).child(gate_wait::WAITING))
                .child(gate_cancel_link(cx))
        })
    }

    /// A commit message transform waits for its workflow (`WF3d`): a line above the button, with the
    /// link that cancels it.
    fn suggest_line(&self, cx: &Context<Self>) -> Option<Div> {
        self.model.transforms.cancel_target(Field::CommitMessage)?;
        Some(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_color(theme::muted())
                        .child(transform_run::WAITING),
                )
                .child(suggest_cancel_link(cx)),
        )
    }

    /// Stops the transform that the message box waits for (`WF3d`).
    pub(crate) fn cancel_suggestion(&mut self, cx: &mut Context<Self>) {
        self.cancel_transform(Field::CommitMessage, cx);
    }

    /// Stops the operation that waits for its gates (`WF3c`): sends `CancelOperation` for it.
    pub(crate) fn cancel_gate(&mut self, cx: &mut Context<Self>) {
        if let Some(op) = self.model.gate_wait.cancel_target() {
            self.engine.dispatch(
                gm_schema::Actor::Human,
                gm_schema::CommandKind::CancelOperation { op },
            );
        }
        cx.notify();
    }

    /// The newest manual workflow run: its status, and its notice or the reason
    /// it failed. Wraps to at most three lines.
    fn workflow_line(&self) -> Option<Div> {
        let line = self.model.workflows.status_line()?;
        Some(
            // Up to three lines: a failure names the command and the reason,
            // which one line of this narrow column cuts off.
            div()
                .w_full()
                .min_w_0()
                .line_clamp(3)
                .text_color(theme::muted())
                .child(one_line(&line)),
        )
    }

    /// The message a workflow offered, with Replace and Discard. Replace fills
    /// the composer with it, which the user asked for; Discard keeps the text.
    fn suggestion_row(&self, cx: &mut Context<Self>) -> Option<Div> {
        let suggestion = self.model.suggestion.current()?;
        let first_line = suggestion.lines().next().unwrap_or("");
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(theme::muted())
                        .child(format!("Suggested: {first_line}")),
                )
                .child(chip_button(
                    "suggest-replace",
                    "Replace",
                    Action::UseSuggestion,
                    cx,
                ))
                .child(chip_button(
                    "suggest-discard",
                    "Discard",
                    Action::DiscardSuggestion,
                    cx,
                )),
        )
    }

    /// Whether the Commit button is drawn enabled: something staged and a message.
    fn commit_enabled(&self, text: &str) -> bool {
        self.model.summary().staged > 0 && !text.trim().is_empty()
    }
}

/// "Commit on" and the branch: the name is the part that gives way when the
/// panel is narrow, so the label never wraps inside the fixed-height button.
fn commit_label(branch: &str) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .min_w(px(0.))
        .overflow_hidden()
        .child(div().flex_none().whitespace_nowrap().child("Commit on "))
        .child(name_label(branch, &[]))
}

/// The status text on one line: an error that carries stderr can span many.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
