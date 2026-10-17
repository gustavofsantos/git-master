//! The commit box at the bottom of the left column: the message, its subject
//! counter and hint, and the Commit button under it.

use gpui::{
    Context, Div, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, div, prelude::*, px,
};

use crate::action_registry::{Action, Args};
use crate::composer;
use crate::hook_run::Phase;
use crate::resize::{self, COMMIT};
use crate::resize_handle::{self, Edge};
use crate::theme;
use crate::view::StatusView;

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
        // Enabled: the dark primary button. Disabled: the panel with muted text
        // and an outline, so the label stays readable.
        let (button_fill, button_text, button_edge) = if enabled {
            (theme::text(), theme::ground(), theme::text())
        } else {
            (theme::panel(), muted, theme::hairline_strong())
        };
        let button = div()
            .id("commit-button")
            .w_full()
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
            .child(format!("Commit on {branch}"));
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
                    .child(self.hook_chip(cx))
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
            .children(self.suggestion_row(cx))
            .child(button)
            .child(height_strip)
    }

    /// The prepare-commit state in the message header: what the run is doing,
    /// with Cancel while it runs, or the Suggest button when a hook applies.
    /// It shares the header row, so nothing below it moves.
    fn hook_chip(&self, cx: &mut Context<Self>) -> Div {
        let hooks = &self.model.hooks;
        let in_flight = hooks.in_flight().is_some();
        let mut row = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .overflow_hidden();
        if let Some(run) = &hooks.run {
            let colour = match run.phase {
                Phase::Failed(_) => theme::error(),
                Phase::Ready { .. } => theme::text(),
                _ => theme::muted(),
            };
            row = row.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(colour)
                    .child(run.phase.label()),
            );
        }
        if in_flight {
            row = row.child(chip_button("hook-cancel", "Cancel", Action::CancelHook, cx));
        } else if hooks.applicable().is_some_and(|count| count > 0) {
            row = row.child(chip_button(
                "hook-suggest",
                "Suggest message",
                Action::SuggestMessage,
                cx,
            ));
        }
        row
    }

    /// The message a hook run offered, with Replace and Discard. Replace fills
    /// the composer with it, which the user asked for; Discard keeps the text.
    fn suggestion_row(&self, cx: &mut Context<Self>) -> Option<Div> {
        let suggestion = self.model.hooks.suggestion.as_deref()?;
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
                    "hook-replace",
                    "Replace",
                    Action::UseSuggestion,
                    cx,
                ))
                .child(chip_button(
                    "hook-discard",
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
