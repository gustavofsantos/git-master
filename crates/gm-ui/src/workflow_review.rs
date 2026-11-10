//! The review of a workflow version (D101, `WF3b`): the "Run this version?" dialog, the notice of
//! an event run that waits for its review, and the Review link beside a gate's veto. The dialog
//! sends `ReviewWorkflow` for the reviewed hash, with the grant the person chose for a trigger that
//! is not `manual`; a manual run then runs again with the same inputs. The text rules are pure and
//! tested here; the window side is the `impl StatusView` at the bottom.

use gm_schema::{Actor, CommandKind, WorkflowInfo};
use gpui::{
    ClickEvent, Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    Stateful, Window, div, prelude::*, px,
};

use crate::FocusNext;
use crate::FocusPrev;
use crate::icon::{self, Icon};
use crate::theme;
use crate::view::StatusView;
use crate::workflow_run::{ReviewRequest, restack_conflict_text};

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
/// The title of a review that does not run the script.
pub(crate) const REVIEW_TITLE: &str = "Review this version?";
pub(crate) const MARK: &str = "Mark Reviewed";
pub(crate) const RUN: &str = "Run this version";
/// The title of the dialog that switches a built-in workflow on in this repository (`WF4b`).
pub(crate) const SWITCH_TITLE: &str = "Switch this workflow on?";
/// The confirm button of that dialog.
pub(crate) const SWITCH_ON: &str = "Switch On";
/// The checkbox that grants automatic approval for the event runs of the version (D101).
pub(crate) const GRANT: &str =
    "Approve applying actions automatically for event runs of this version";

/// The line under the title: what the dialog asks and why.
#[must_use]
pub(crate) fn intro(name: &str) -> String {
    format!(
        "{name} has not been run here in this form. Read it before you run it: it can start the programs it names."
    )
}

/// The line under the title of a review that does not run the script: its trigger, and the risk.
#[must_use]
pub(crate) fn review_intro(name: &str, on: &str) -> String {
    format!(
        "{name} runs on {on}. Read it before you mark it reviewed: it can start the programs it names."
    )
}

/// The line under the title of the switch dialog: what the workflow does, and that it changes the
/// repository when it runs.
#[must_use]
pub(crate) fn switch_intro(name: &str, on: &str) -> String {
    format!(
        "{name} runs on {on}. It is off in every repository until you switch it on here, and then it changes this repository when it runs."
    )
}

/// The title of the dialog for `request`: the question when it runs the script.
#[must_use]
pub(crate) fn title(request: &ReviewRequest) -> &'static str {
    if request.switch {
        SWITCH_TITLE
    } else if request.run {
        TITLE
    } else {
        REVIEW_TITLE
    }
}

/// The line under the title for `request`.
#[must_use]
pub(crate) fn intro_for(request: &ReviewRequest) -> String {
    if request.switch {
        switch_intro(&request.name, &request.on)
    } else if request.run {
        intro(&request.name)
    } else {
        review_intro(&request.name, &request.on)
    }
}

/// The confirm button's words: a run confirms and runs, a review only marks the version.
#[must_use]
pub(crate) fn confirm_label(request: &ReviewRequest) -> &'static str {
    if request.switch {
        SWITCH_ON
    } else if request.run {
        RUN
    } else {
        MARK
    }
}

/// Whether the dialog offers the grant. A manual workflow has no event runs, so it never does.
#[must_use]
pub(crate) fn grant_offered(request: &ReviewRequest) -> bool {
    request.on != "manual"
}

/// The line of a notice for an event run that waits for its review. A transform is not run
/// retroactively, so its notice asks the person to ask again; a conflict hunk's runs start when
/// the file is read, so its notice asks the person to reopen the file (D168(4)).
#[must_use]
pub(crate) fn wanted_text(name: &str, trigger: &str) -> String {
    match trigger {
        "transform.conflict_hunk" => {
            format!("{name} wants to run on {trigger}: review it, then reopen the file")
        }
        _ if trigger.starts_with("transform.") => {
            format!("{name} wants to run on {trigger}: review it, then ask again")
        }
        _ => format!("{name} wants to run on {trigger}"),
    }
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

/// The commands that confirm the review: the reviewed hash is recorded, with the grant when the
/// dialog offers it and the person ticked it. A run confirms with `RunWorkflow` after it, with the
/// same inputs. The engine handles them in this order; an event run that waited for this version
/// starts inside the review.
#[must_use]
pub(crate) fn commands(request: &ReviewRequest, auto_approve: bool) -> Vec<CommandKind> {
    if request.switch {
        let ticked = grant_offered(request) && auto_approve;
        return switch_commands(&request.name, &request.hash, ticked, request.granted);
    }
    let mut kinds = vec![CommandKind::ReviewWorkflow {
        name: request.name.clone(),
        hash: request.hash.clone(),
        auto_approve: grant_offered(request) && auto_approve,
    }];
    if request.run {
        kinds.push(CommandKind::RunWorkflow {
            name: request.name.clone(),
            inputs: request.inputs.clone(),
        });
    }
    kinds
}

/// The commands that switch a workflow on in this repository (`WF4b`): `SetWorkflowEnabled`, and
/// `ReviewWorkflow` with the grant when the ticked grant differs from the one the version has. A
/// version that is already granted and not ticked loses its grant; the review stays.
#[must_use]
pub(crate) fn switch_commands(
    name: &str,
    hash: &str,
    ticked: bool,
    granted: bool,
) -> Vec<CommandKind> {
    let mut kinds = vec![CommandKind::SetWorkflowEnabled {
        name: name.to_owned(),
        enabled: true,
    }];
    if ticked != granted {
        kinds.push(CommandKind::ReviewWorkflow {
            name: name.to_owned(),
            hash: hash.to_owned(),
            auto_approve: ticked,
        });
    }
    kinds
}

/// The request of the Switch On dialog for a built-in workflow the page shows (`WF4b`).
#[must_use]
pub(crate) fn switch_request_for_info(info: &WorkflowInfo) -> ReviewRequest {
    ReviewRequest {
        name: info.name.clone(),
        hash: info.hash.clone(),
        source: info.source.clone(),
        inputs: std::collections::BTreeMap::new(),
        on: info.trigger.clone(),
        run: false,
        switch: true,
        granted: info.auto_approve,
    }
}

/// The review request for a loaded workflow the page shows (the Review button of the page).
/// A manual workflow with declared inputs is only marked reviewed: its run asks for the inputs.
#[must_use]
pub(crate) fn request_for_info(info: &WorkflowInfo) -> ReviewRequest {
    ReviewRequest {
        name: info.name.clone(),
        hash: info.hash.clone(),
        source: info.source.clone(),
        inputs: std::collections::BTreeMap::new(),
        on: info.trigger.clone(),
        run: info.trigger == "manual" && info.inputs.is_empty(),
        switch: false,
        granted: info.auto_approve,
    }
}

impl StatusView {
    /// Opens the Switch On dialog for the built-in `info` (`WF4b`): its script, the grant, and the
    /// switch on confirm.
    pub(crate) fn open_switch_dialog(
        &mut self,
        info: &WorkflowInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_workflow_review_dialog(switch_request_for_info(info), window, cx);
    }
}

/// The dialog while it is open: the request it asks about, the grant ticked, and its focus.
pub(crate) struct WorkflowReviewDialog {
    pub(crate) request: ReviewRequest,
    pub(crate) auto_approve: bool,
    pub(crate) focus: FocusHandle,
}

impl StatusView {
    /// Opens the dialog for `request`, replacing a dialog about another script. The grant starts
    /// unticked.
    pub(crate) fn open_workflow_review_dialog(
        &mut self,
        request: ReviewRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        focus.focus(window);
        // The grant starts as the version has it: unticked for a review, the current one for a switch.
        let auto_approve = request.granted;
        self.workflow_review_dialog = Some(WorkflowReviewDialog {
            request,
            auto_approve,
            focus,
        });
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

    /// Opens the review of the version `hash` of the workflow `name`, when the listing has that
    /// version. Otherwise the listing is read again, and the person asks once more.
    pub(crate) fn open_review_of(
        &mut self,
        name: &str,
        hash: &str,
        run: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let info = self
            .model
            .workflows
            .listed
            .as_deref()
            .and_then(|listed| {
                listed
                    .iter()
                    .find(|info| info.name == name && info.hash == hash)
            })
            .cloned();
        let Some(info) = info else {
            self.request_workflow_list();
            cx.notify();
            return;
        };
        let mut request = request_for_info(&info);
        request.run = run && request.run;
        self.open_workflow_review_dialog(request, window, cx);
    }

    /// The Review button of the Workflows page: the dialog for the open workflow's version.
    pub(crate) fn open_page_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let info = self.selected_workflow_info().cloned();
        let Some(info) = info else {
            return;
        };
        self.open_workflow_review_dialog(request_for_info(&info), window, cx);
    }

    /// The listed workflow the Workflows page has open, when the listing still has it.
    pub(crate) fn selected_workflow_info(&self) -> Option<&WorkflowInfo> {
        let path = self.model.workflows_page.path()?;
        self.model
            .workflows
            .listed
            .as_deref()?
            .iter()
            .find(|info| info.path == path)
    }

    /// Ticks or unticks the grant of the open dialog.
    pub(crate) fn toggle_review_grant(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.workflow_review_dialog.as_mut()
            && grant_offered(&dialog.request)
        {
            dialog.auto_approve = !dialog.auto_approve;
        }
        cx.notify();
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

    /// Enter and the confirm button: sends the review (and the run), in order, then closes.
    pub(crate) fn submit_workflow_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.workflow_review_dialog.as_ref() else {
            return;
        };
        let kinds = commands(&dialog.request, dialog.auto_approve);
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
                    "space" => this.toggle_review_grant(cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title(request)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::muted())
                    .child(intro_for(request)),
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
            .children(grant_offered(request).then(|| grant_row(dialog.auto_approve, cx)))
            .child(buttons(request, cx))
    }
}

/// The checkbox that grants automatic approval for the event runs of this version. Space and a
/// click tick it.
fn grant_row(checked: bool, cx: &Context<StatusView>) -> Stateful<Div> {
    let mark = if checked {
        icon::filled(Icon::CheckSquare, icon::SMALL, theme::accent())
    } else {
        icon::view(Icon::Square, icon::SMALL, theme::muted())
    };
    div()
        .id("workflow-review-grant")
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(mark)
        .child(div().text_sm().text_color(theme::text()).child(GRANT))
        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.toggle_review_grant(cx);
        }))
}

/// Cancel, and the confirm button that reviews (and runs) this version.
fn buttons(request: &ReviewRequest, cx: &Context<StatusView>) -> Div {
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
                .child(confirm_label(request))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.submit_workflow_review(window, cx);
                    }),
                ),
        )
}

/// The non-modal notices of event runs that wait for their review, and the Review link beside a
/// gate's veto (`WF3b`). Both are drawn by the window; nothing here blocks the work.
impl StatusView {
    /// The non-modal notices above the status bar: the event runs that wait for their review, and
    /// the runs that stopped on a restack conflict (`WF4c`). `None` when there is none.
    pub(crate) fn notices_strip(&self, cx: &Context<Self>) -> Option<Div> {
        let wanted = self.wanted_strip(cx);
        let conflict = self.restack_conflict_strip(cx);
        if wanted.is_none() && conflict.is_none() {
            return None;
        }
        Some(div().flex().flex_col().children(wanted).children(conflict))
    }

    /// The notice of a run that stopped on a restack conflict: its line, Resolve conflict, which
    /// opens the restack dialog on the plan's root (D154), and Dismiss. `None` when no run stopped
    /// (`WF4c`).
    pub(crate) fn restack_conflict_strip(&self, cx: &Context<Self>) -> Option<Div> {
        let notice = self.model.workflows.restack_conflict.as_ref()?;
        let branch = notice.branch()?.to_owned();
        let root = notice.plan.from.clone();
        let text = restack_conflict_text(
            &self.model.workflows.display_name(&notice.workflow),
            &branch,
        );
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_4()
                .py_1()
                .text_sm()
                .bg(theme::warn_bg())
                .border_t_1()
                .border_color(theme::hairline())
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(theme::warn_fg())
                        .child(text),
                )
                .child(
                    div()
                        .id("restack-conflict-resolve")
                        .flex_none()
                        .text_color(theme::accent())
                        .cursor_pointer()
                        .child("Resolve conflict")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                this.open_restack_dialog_on(&root, window, cx);
                            }),
                        ),
                )
                .child(
                    div()
                        .id("restack-conflict-dismiss")
                        .flex_none()
                        .text_color(theme::muted())
                        .cursor_pointer()
                        .child("Dismiss")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                this.model.workflows.dismiss_restack_conflict();
                                cx.notify();
                            }),
                        ),
                ),
        )
    }

    /// One line per run that waits: its text, Review and Dismiss. `None` when none waits.
    pub(crate) fn wanted_strip(&self, cx: &Context<Self>) -> Option<Div> {
        let rows: Vec<Div> = self
            .model
            .workflows
            .wanted
            .iter()
            .enumerate()
            .map(|(index, (name, wanted))| {
                let review_name = name.clone();
                let dismiss_name = name.clone();
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_1()
                    .text_sm()
                    .text_color(theme::warn_fg())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(wanted_text(name, &wanted.trigger)),
                    )
                    .child(
                        div()
                            .id(("wanted-review", index))
                            .flex_none()
                            .text_color(theme::accent())
                            .cursor_pointer()
                            .child("Review")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    this.open_review_of(
                                        &review_name,
                                        &wanted_hash(this, &review_name),
                                        false,
                                        window,
                                        cx,
                                    );
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id(("wanted-dismiss", index))
                            .flex_none()
                            .text_color(theme::muted())
                            .cursor_pointer()
                            .child("Dismiss")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    this.engine.dispatch(
                                        Actor::Human,
                                        CommandKind::DismissWorkflowRequest {
                                            workflow: dismiss_name.clone(),
                                        },
                                    );
                                    cx.notify();
                                }),
                            ),
                    )
            })
            .collect();
        if rows.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .bg(theme::warn_bg())
                .border_t_1()
                .border_color(theme::hairline())
                .children(rows),
        )
    }

    /// The Review link beside a gate's veto, when the operation that failed was vetoed for want of
    /// a review (`WF3b`).
    pub(crate) fn gate_review_link(&self, cx: &Context<Self>) -> Option<Stateful<Div>> {
        self.model.error.as_ref()?;
        let gate = self.model.workflows.gate_review.clone()?;
        Some(
            div()
                .id("gate-review")
                .flex_none()
                .text_color(theme::accent())
                .cursor_pointer()
                .child("Review")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.open_review_of(&gate.workflow, &gate.hash, false, window, cx);
                    }),
                ),
        )
    }
}

/// The hash the notice of `name` asked about.
fn wanted_hash(this: &StatusView, name: &str) -> String {
    this.model
        .workflows
        .wanted
        .get(name)
        .map(|wanted| wanted.hash.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn numbered(lines: &[&str]) -> Vec<(usize, String)> {
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| (index + 1, (*line).to_owned()))
            .collect()
    }

    fn request(on: &str, run: bool) -> ReviewRequest {
        ReviewRequest {
            name: "w".into(),
            hash: "abc123".into(),
            source: "return 1".into(),
            inputs: BTreeMap::new(),
            on: on.into(),
            run,
            switch: false,
            granted: false,
        }
    }

    #[test]
    fn the_title_is_the_question() {
        assert_eq!(TITLE, "Run this version?");
        assert_eq!(title(&request("manual", true)), "Run this version?");
    }

    #[test]
    fn a_review_that_does_not_run_asks_for_the_review_not_the_run() {
        assert_eq!(
            title(&request("commit.created", false)),
            "Review this version?"
        );
        assert_eq!(
            review_intro("on-commit", "commit.created"),
            "on-commit runs on commit.created. Read it before you mark it reviewed: it can start the programs it names."
        );
    }

    #[test]
    fn a_manual_workflow_confirms_with_a_run_and_an_event_one_only_marks_it() {
        assert_eq!(confirm_label(&request("manual", true)), "Run this version");
        assert_eq!(
            confirm_label(&request("commit.created", false)),
            "Mark Reviewed"
        );
        assert_eq!(
            confirm_label(&request("before.commit", false)),
            "Mark Reviewed"
        );
    }

    #[test]
    fn the_grant_is_offered_for_a_trigger_that_is_not_manual_only() {
        assert!(!grant_offered(&request("manual", true)));
        assert!(grant_offered(&request("commit.created", false)));
        assert!(grant_offered(&request("after.commit", false)));
        assert!(grant_offered(&request("transform.commit_message", false)));
    }

    #[test]
    fn the_intro_of_a_run_is_the_old_text() {
        assert_eq!(
            intro_for(&request("manual", true)),
            "w has not been run here in this form. Read it before you run it: it can start the programs it names."
        );
    }

    #[test]
    fn a_waiting_event_run_says_what_it_wants() {
        assert_eq!(
            wanted_text("on-commit", "commit.created"),
            "on-commit wants to run on commit.created"
        );
        assert_eq!(
            wanted_text("llm", "transform.commit_message"),
            "llm wants to run on transform.commit_message: review it, then ask again"
        );
        assert_eq!(
            wanted_text("llm-resolve", "transform.conflict_hunk"),
            "llm-resolve wants to run on transform.conflict_hunk: review it, then reopen the file"
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
    fn a_manual_run_reviews_the_hash_first_and_then_runs_with_the_inputs() {
        let inputs = BTreeMap::from([("ticket".to_owned(), "ENG-42".to_owned())]);
        let req = ReviewRequest {
            name: "w".into(),
            hash: "abc123".into(),
            source: "return 1".into(),
            inputs: inputs.clone(),
            on: "manual".into(),
            run: true,
            switch: false,
            granted: false,
        };
        assert_eq!(
            commands(&req, true),
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

    #[test]
    fn an_event_review_sends_the_grant_chosen_and_never_runs() {
        let req = request("commit.created", false);
        assert_eq!(
            commands(&req, true),
            vec![CommandKind::ReviewWorkflow {
                name: "w".into(),
                hash: "abc123".into(),
                auto_approve: true,
            }]
        );
        assert_eq!(
            commands(&req, false),
            vec![CommandKind::ReviewWorkflow {
                name: "w".into(),
                hash: "abc123".into(),
                auto_approve: false,
            }]
        );
    }

    #[test]
    fn a_manual_review_never_sends_the_grant() {
        let req = request("manual", true);
        assert_eq!(
            commands(&req, true),
            vec![
                CommandKind::ReviewWorkflow {
                    name: "w".into(),
                    hash: "abc123".into(),
                    auto_approve: false,
                },
                CommandKind::RunWorkflow {
                    name: "w".into(),
                    inputs: BTreeMap::new(),
                },
            ]
        );
    }

    fn info(trigger: &str, inputs: usize) -> WorkflowInfo {
        WorkflowInfo {
            name: "w".into(),
            path: "/w/w.lua".into(),
            trigger: trigger.into(),
            key: None,
            inputs: (0..inputs)
                .map(|i| gm_schema::WorkflowInput {
                    name: format!("i{i}"),
                    prompt: None,
                })
                .collect(),
            hash: "h".into(),
            error: None,
            warnings: Vec::new(),
            source: "src".into(),
            reviewed: false,
            auto_approve: false,
            enabled: false,
            title: None,
            location: gm_schema::WorkflowLocation::User,
            role: gm_schema::WorkflowRole::Workflow,
            description: None,
        }
    }

    #[test]
    fn the_page_review_of_a_manual_workflow_without_inputs_runs_it() {
        let req = request_for_info(&info("manual", 0));
        assert!(req.run);
        assert_eq!(req.on, "manual");
        assert_eq!(req.source, "src");
    }

    #[test]
    fn the_page_review_of_a_manual_workflow_with_inputs_only_marks_it() {
        let req = request_for_info(&info("manual", 1));
        assert!(!req.run);
    }

    #[test]
    fn the_page_review_of_an_event_workflow_only_marks_it() {
        let req = request_for_info(&info("commit.created", 0));
        assert!(!req.run);
        assert_eq!(req.on, "commit.created");
    }

    fn built_in(granted: bool) -> WorkflowInfo {
        WorkflowInfo {
            name: "restack-stale-after-fetch".into(),
            path: "builtin:restack-stale-after-fetch".into(),
            trigger: "fetch.done".into(),
            key: None,
            inputs: Vec::new(),
            hash: "shipped-hash".into(),
            error: None,
            warnings: Vec::new(),
            source: "return 1".into(),
            reviewed: true,
            auto_approve: granted,
            enabled: false,
            title: Some("Restack stale stacks after fetch".into()),
            location: gm_schema::WorkflowLocation::User,
            role: gm_schema::WorkflowRole::Workflow,
            description: None,
        }
    }

    #[test]
    fn switching_on_sends_the_switch_and_the_grant_only_when_it_changes() {
        // Not ticked and not granted: the switch alone.
        assert_eq!(
            switch_commands("b", "h", false, false),
            vec![CommandKind::SetWorkflowEnabled {
                name: "b".into(),
                enabled: true,
            }]
        );
        // Ticked: the switch, then the review with the grant.
        assert_eq!(
            switch_commands("b", "h", true, false),
            vec![
                CommandKind::SetWorkflowEnabled {
                    name: "b".into(),
                    enabled: true,
                },
                CommandKind::ReviewWorkflow {
                    name: "b".into(),
                    hash: "h".into(),
                    auto_approve: true,
                },
            ]
        );
        // Granted before and unticked now: the grant goes, the review stays.
        assert_eq!(
            switch_commands("b", "h", false, true),
            vec![
                CommandKind::SetWorkflowEnabled {
                    name: "b".into(),
                    enabled: true,
                },
                CommandKind::ReviewWorkflow {
                    name: "b".into(),
                    hash: "h".into(),
                    auto_approve: false,
                },
            ]
        );
        // Granted and still ticked: nothing about the grant changes.
        assert_eq!(switch_commands("b", "h", true, true).len(), 1);
    }

    #[test]
    fn the_switch_dialog_says_switch_on_and_offers_the_grant() {
        let info = built_in(false);
        let request = switch_request_for_info(&info);
        assert_eq!(title(&request), SWITCH_TITLE);
        assert_eq!(confirm_label(&request), SWITCH_ON);
        assert!(grant_offered(&request));
        assert!(intro_for(&request).contains("fetch.done"));
    }

    #[test]
    fn the_switch_dialog_confirms_with_the_commands_of_its_grant() {
        let request = switch_request_for_info(&built_in(false));
        assert_eq!(commands(&request, false).len(), 1);
        assert_eq!(commands(&request, true).len(), 2);
        // A switch never runs the script: it is not a run.
        assert!(
            !commands(&request, true)
                .iter()
                .any(|kind| matches!(kind, CommandKind::RunWorkflow { .. }))
        );
    }

    #[test]
    fn the_dialog_for_a_granted_built_in_starts_from_its_grant() {
        assert!(switch_request_for_info(&built_in(true)).granted);
        assert!(!switch_request_for_info(&built_in(false)).granted);
    }
}
