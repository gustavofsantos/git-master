//! The commit box above the change list: the message, its subject counter and
//! hint, and the Commit button.

use gpui::{Context, Div, KeyDownEvent, MouseButton, MouseDownEvent, div, prelude::*, px};

use crate::composer;
use crate::theme;
use crate::view::StatusView;

/// The subject length a commit should not pass.
const SUBJECT_LIMIT: usize = 72;

impl StatusView {
    pub(crate) fn commit_block(&self, cx: &Context<Self>) -> Div {
        let muted = theme::muted();
        let text = self.composer.read(cx).text(cx);
        let subject = composer::subject_len(&text);
        let hint = composer::subject_hint(&text);
        let staged = self.model.summary().staged;
        let counter_colour = if subject > SUBJECT_LIMIT {
            theme::error()
        } else {
            muted
        };
        let enabled = staged > 0 && !text.trim().is_empty();
        // Enabled: solid accent with its own text colour. Disabled: the panel
        // with muted text and an outline, so the label stays readable.
        let (button_fill, button_text, button_edge) = if enabled {
            (theme::accent(), theme::on_accent(), theme::accent())
        } else {
            (theme::panel(), muted, theme::hairline_strong())
        };
        let button = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .h(px(26.))
            .rounded_md()
            .border_1()
            .border_color(button_edge)
            .bg(button_fill)
            .text_color(button_text)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.commit(cx)),
            )
            .child(format!("Commit ({staged})"))
            .child(div().child("⌘↵"));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .text_color(muted)
                    .child("Message")
                    .child(
                        div()
                            .font_family(theme::code_font())
                            .text_color(counter_colour)
                            .child(format!("{subject}/{SUBJECT_LIMIT}")),
                    ),
            )
            // Typing does not notify this view, so every key asks for a redraw
            // in which the counter and hint are read again.
            .child(
                div()
                    .capture_key_down(cx.listener(|_, _: &KeyDownEvent, _, cx| cx.notify()))
                    .child(self.composer.clone()),
            )
            .child(
                div()
                    .h(px(18.))
                    .text_color(theme::warn_fg())
                    .child(hint.unwrap_or("")),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .px_2()
                            .rounded_sm()
                            .bg(theme::added_bg())
                            .text_color(theme::added_fg())
                            .child(format!("{staged} staged")),
                    )
                    .child(button),
            )
    }
}
