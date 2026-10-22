//! The "Run this version?" dialog. A script whose exact text the person has not
//! reviewed does not start; the engine asks first and the dialog shows the
//! source (D101). Confirming sends `ReviewWorkflow` for that hash, then the
//! run again with the same inputs. The text rules are pure and tested here;
//! the window side is the `impl StatusView` at the bottom.

use gm_schema::{Actor, CommandKind};
use gpui::{
    Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, Window, div,
    prelude::*, px,
};

use crate::FocusNext;
use crate::FocusPrev;
use crate::theme;
use crate::view::StatusView;
use crate::workflow_run::ReviewRequest;

const WIDTH: f32 = 680.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 72.;
/// The source box stops growing here and scrolls.
const SOURCE_MAX_HEIGHT: f32 = 360.;
/// The width of the line-number gutter.
const GUTTER: f32 = 44.;
/// The longest source the dialog shows; a longer script says how many lines are hidden.
pub(crate) const SOURCE_LINES: usize = 400;

pub(crate) const TITLE: &str = "Run this version?";

/// The line under the title: what the dialog asks and why.
#[must_use]
pub(crate) fn intro(name: &str) -> String {
    format!(
        "{name} has not been run here in this form. Read it before you run it: it can start the programs it names."
    )
}

/// The first `max` lines of `source`, numbered from 1, with tabs as four spaces
/// and a trailing `\r` removed, and the count of lines left out. An empty source
/// has no lines.
#[must_use]
pub(crate) fn source_lines(source: &str, max: usize) -> (Vec<(usize, String)>, usize) {
    if source.is_empty() {
        return (Vec::new(), 0);
    }
    let body = source.strip_suffix('\n').unwrap_or(source);
    let all: Vec<&str> = body.split('\n').collect();
    let shown = all
        .iter()
        .take(max)
        .enumerate()
        .map(|(index, line)| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            (index + 1, line.replace('\t', "    "))
        })
        .collect();
    (shown, all.len().saturating_sub(max))
}

/// The commands that confirm the review: the reviewed hash is recorded as
/// reviewed (not auto-approved), then the run goes ahead with the same inputs.
/// The engine handles them in this order.
#[must_use]
pub(crate) fn commands(request: &ReviewRequest) -> Vec<CommandKind> {
    vec![
        CommandKind::ReviewWorkflow {
            name: request.name.clone(),
            hash: request.hash.clone(),
            auto_approve: false,
        },
        CommandKind::RunWorkflow {
            name: request.name.clone(),
            inputs: request.inputs.clone(),
        },
    ]
}

/// The dialog while it is open: the request it asks about and its focus.
pub(crate) struct WorkflowReviewDialog {
    pub(crate) request: ReviewRequest,
    pub(crate) focus: FocusHandle,
}

impl StatusView {
    /// Opens the dialog for `request`, replacing a dialog about another script.
    pub(crate) fn open_workflow_review_dialog(
        &mut self,
        request: ReviewRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        focus.focus(window);
        self.workflow_review_dialog = Some(WorkflowReviewDialog { request, focus });
        cx.notify();
    }

    /// Opens the dialog for the review the model holds, unless it already shows
    /// it. The model holds a request only until the person answers, so this runs
    /// after every applied event and opens the dialog once per request.
    pub(crate) fn open_pending_workflow_review(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.model.workflows.review.as_ref() else {
            return;
        };
        if self
            .workflow_review_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.request == *request)
        {
            return;
        }
        let request = request.clone();
        self.open_workflow_review_dialog(request, window, cx);
    }

    /// Closes the dialog and tells the model the review is answered. Escape,
    /// a click outside and Cancel all come here; nothing is sent.
    pub(crate) fn close_workflow_review_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_open = self.workflow_review_dialog.take().is_some();
        self.model.workflows.review_answered();
        if was_open {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// Enter and the confirm button: sends the review and the run, in order, then closes.
    pub(crate) fn submit_workflow_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.workflow_review_dialog.as_ref() else {
            return;
        };
        let kinds = commands(&dialog.request);
        for kind in kinds {
            self.engine.dispatch(Actor::Human, kind);
        }
        self.close_workflow_review_dialog(window, cx);
    }

    /// The dialog's layer, or nothing while the dialog is closed.
    pub(crate) fn workflow_review_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.workflow_review_dialog.as_ref()?;
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_workflow_review_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        Some(
            div().absolute().inset_0().child(outside).child(
                Self::workflow_review_panel(dialog, left, cx)
                    // Tab has nothing else to reach while the dialog is open.
                    .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
                    .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation()),
            ),
        )
    }

    fn workflow_review_panel(dialog: &WorkflowReviewDialog, left: f32, cx: &Context<Self>) -> Div {
        let request = &dialog.request;
        let (lines, hidden) = source_lines(&request.source, SOURCE_LINES);
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
                    "escape" => this.close_workflow_review_dialog(window, cx),
                    "enter" => this.submit_workflow_review(window, cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(TITLE),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::muted())
                    .child(intro(&request.name)),
            )
            .child(
                div()
                    .id("workflow-review-source")
                    .flex()
                    .flex_col()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::hairline())
                    .max_h(px(SOURCE_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .font_family(theme::code_font())
                    .text_xs()
                    .text_color(theme::text())
                    .children(lines.into_iter().map(|(number, text)| {
                        div()
                            .flex()
                            .flex_row()
                            .px_2()
                            .child(
                                div()
                                    .w(px(GUTTER))
                                    .flex_none()
                                    .pr_2()
                                    .text_right()
                                    .text_color(theme::muted())
                                    .child(number.to_string()),
                            )
                            .child(div().flex_1().child(text))
                    })),
            )
            .children((hidden > 0).then(|| {
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(format!("… {hidden} more lines not shown"))
            }))
            .child(buttons(cx))
    }
}

/// Cancel, and the confirm button that reviews and runs this version.
fn buttons(cx: &Context<StatusView>) -> Div {
    div()
        .flex()
        .flex_row()
        .justify_end()
        .items_center()
        .gap_2()
        .child(
            div()
                .id("workflow-review-cancel")
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
                        this.close_workflow_review_dialog(window, cx);
                    }),
                ),
        )
        .child(
            div()
                .id("workflow-review-confirm")
                .px_3()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded_md()
                .bg(theme::accent())
                .text_color(theme::on_accent())
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .child("Run this version")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.submit_workflow_review(window, cx);
                    }),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(lines: &[&str]) -> Vec<(usize, String)> {
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| (index + 1, (*line).to_owned()))
            .collect()
    }

    #[test]
    fn the_title_is_the_question() {
        assert_eq!(TITLE, "Run this version?");
    }

    #[test]
    fn the_intro_names_the_script_and_the_risk() {
        assert_eq!(
            intro("branch-from-ticket"),
            "branch-from-ticket has not been run here in this form. Read it before you run it: it can start the programs it names."
        );
    }

    #[test]
    fn an_empty_source_has_no_lines_and_nothing_hidden() {
        assert_eq!(source_lines("", 400), (Vec::new(), 0));
    }

    #[test]
    fn a_source_without_a_trailing_newline_keeps_its_last_line() {
        assert_eq!(source_lines("a\nb", 400), (numbered(&["a", "b"]), 0));
    }

    #[test]
    fn a_trailing_newline_does_not_add_a_line() {
        assert_eq!(source_lines("a\nb\n", 400), (numbered(&["a", "b"]), 0));
    }

    #[test]
    fn crlf_line_endings_lose_their_carriage_return() {
        assert_eq!(source_lines("a\r\nb\r\n", 400), (numbered(&["a", "b"]), 0));
        assert_eq!(source_lines("a\r\nb", 400), (numbered(&["a", "b"]), 0));
    }

    #[test]
    fn a_lone_newline_is_one_empty_line() {
        assert_eq!(source_lines("\n", 400), (numbered(&[""]), 0));
    }

    #[test]
    fn tabs_become_four_spaces() {
        assert_eq!(
            source_lines("\tx\n\ta\tb", 400),
            (numbered(&["    x", "    a    b"]), 0)
        );
    }

    #[test]
    fn exactly_max_lines_are_all_shown() {
        let source = "1\n2\n3\n";
        assert_eq!(source_lines(source, 3), (numbered(&["1", "2", "3"]), 0));
    }

    #[test]
    fn more_than_max_lines_show_the_first_and_count_the_rest() {
        let source = "1\n2\n3\n4\n5\n";
        assert_eq!(source_lines(source, 2), (numbered(&["1", "2"]), 3));
    }

    #[test]
    fn a_source_longer_than_max_without_a_trailing_newline_counts_correctly() {
        assert_eq!(source_lines("1\n2\n3", 2), (numbered(&["1", "2"]), 1));
    }

    #[test]
    fn max_zero_hides_every_line() {
        assert_eq!(source_lines("a\nb\n", 0), (Vec::new(), 2));
    }

    #[test]
    fn the_commands_review_the_hash_first_and_then_run_with_the_inputs() {
        use std::collections::BTreeMap;

        let inputs = BTreeMap::from([("ticket".to_owned(), "ENG-42".to_owned())]);
        let req = ReviewRequest {
            name: "w".into(),
            hash: "abc123".into(),
            source: "return 1".into(),
            inputs: inputs.clone(),
        };
        assert_eq!(
            commands(&req),
            vec![
                CommandKind::ReviewWorkflow {
                    name: "w".into(),
                    hash: "abc123".into(),
                    auto_approve: false,
                },
                CommandKind::RunWorkflow {
                    name: "w".into(),
                    inputs,
                },
            ]
        );
    }
}
