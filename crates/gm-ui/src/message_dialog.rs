//! The message dialog: a title, Git's text and one OK button, for an operation
//! that did not do what it was asked (B2 stash apply and pop, B3 detached
//! checkout). A failure is several lines long, which the status bar cannot hold.
//! The text is cut at `SHOWN_LINES`, and the dialog closes on OK, Escape, Enter
//! or a click outside it.

use gpui::{
    Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, Window, div,
    prelude::*, px,
};

use crate::FocusNext;
use crate::FocusPrev;
use crate::theme;
use crate::view::StatusView;

const WIDTH: f32 = 520.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;
/// How many lines of Git's text the message shows before it says how many are left.
pub(crate) const SHOWN_LINES: usize = 15;

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

/// The message while it is open: its title, Git's text and the focus that
/// carries Enter and Escape.
pub(crate) struct MessageDialog {
    pub(crate) title: String,
    pub(crate) text: String,
    focus: FocusHandle,
}

impl StatusView {
    /// Opens the message `title` with `text`, and takes the keyboard for it.
    pub(crate) fn open_message(
        &mut self,
        title: String,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        focus.focus(window);
        self.message_dialog = Some(MessageDialog { title, text, focus });
        cx.notify();
    }

    pub(crate) fn close_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.message_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// The message's layer, or nothing while it is closed.
    pub(crate) fn message_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.message_dialog.as_ref()?;
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_message(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        Some(
            div().absolute().inset_0().child(outside).child(
                message_panel(dialog, left, cx)
                    // The message has one button, so Tab has nowhere to go.
                    .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
                    .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation()),
            ),
        )
    }
}

fn message_panel(dialog: &MessageDialog, left: f32, cx: &Context<StatusView>) -> Div {
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
                "escape" | "enter" => this.close_message(window, cx),
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
                    .id("message-ok")
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
                            this.close_message(window, cx);
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
}
