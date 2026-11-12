//! The Workflows page (design/screens.md, D233): the workflow files on the left; on the
//! right the open script with its Run, Dry Run, Open in Editor, Copy Path and Delete buttons and
//! two tabs. Script is the file as it is on disk, read-only, with its diagnostics: the person
//! edits it in their own editor. Runs lists the runs and shows one run call by call. The
//! state is in `workflows_page`; this file draws it and sends what the user asks
//! for to the engine, through the action registry where one exists.

use std::ops::Range;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use gm_schema::{
    Actor, WorkflowDiagnostic, WorkflowDryCall, WorkflowDryRun, WorkflowInfo, WorkflowRunDetail,
    WorkflowRunStatus, WorkflowRunSummary,
};
use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, IntoElement, MouseButton, MouseDownEvent,
    ScrollStrategy, ScrollWheelEvent, SharedString, Stateful, UniformListScrollHandle, Window, div,
    prelude::*, px, uniform_list,
};

use crate::OpenWorkflowInEditor;
use crate::action_registry::{Action, Args};
use crate::icon::{self, Icon};
use crate::resize::{self};
use crate::resize_handle::{self, Edge};
use crate::restack_dialog;
use crate::script_view::{self, ScriptText};
use crate::shell_code::colored;
use crate::shell_history::{now_secs, relative_time};
use crate::theme;
use crate::view::{Screen, StatusView};
use crate::workflow_keys;
use crate::workflows_page::{
    Preview, ReviewLine, RunControls, StepRecord, SwitchClick, Tab, context_blocks, context_key,
    copy_request, diagnostic_text, duration_label, expand_key, input_text, output_text,
    pretty_json, preview, review_line, review_text, row_title, run_controls, run_duration,
    status_label, step_duration, step_rows, step_summary, switch_click, switch_text,
};

/// The width of the workflow list on the left.
/// The height of the runs list above a run's view.
const RUNS_LIST_HEIGHT: f32 = 220.;
/// The most of the dry run panel shown; it scrolls past this.
const DRY_RUN_HEIGHT: f32 = 260.;
/// The height of a line of the script, and the width of a digit of its line numbers.
const SCRIPT_LINE_HEIGHT: f32 = 20.;
const SCRIPT_DIGIT_WIDTH: f32 = 8.;
/// How many columns a sideways wheel step moves the script.
const SCRIPT_SCROLL_STEP: usize = 4;

/// What the window keeps of the Script tab: the text on screen, where it is scrolled, and the
/// line a problem pointed at.
pub(crate) struct ScriptView {
    pub(crate) text: ScriptText,
    pub(crate) scroll: UniformListScrollHandle,
    /// How many columns the text is scrolled to the right.
    pub(crate) columns: usize,
    /// The line a clicked problem names, marked until another text shows.
    pub(crate) marked: Option<usize>,
}

impl ScriptView {
    pub(crate) fn new() -> Self {
        Self {
            text: ScriptText::default(),
            scroll: UniformListScrollHandle::new(),
            columns: 0,
            marked: None,
        }
    }
}

impl StatusView {
    /// Ctrl or ⌘ E: the registry's Open Workflow in Editor.
    pub(crate) fn open_workflow_in_editor_action(
        &mut self,
        _: &OpenWorkflowInEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_action(Action::OpenWorkflowInEditor, &Args::None, window, cx);
    }

    /// Shows the Workflows page and asks the engine for the workflow files.
    pub(crate) fn show_workflows(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::Workflows;
        self.request_workflow_list();
        cx.notify();
    }

    /// Opens the first listed workflow when none is open, so the page is never empty.
    pub(crate) fn open_first_workflow(&mut self, cx: &mut Context<Self>) {
        if self.model.workflows_page.path().is_some() {
            return;
        }
        let first = self
            .model
            .workflows
            .listed
            .as_deref()
            .and_then(|listed| listed.first())
            .map(|info| info.path.clone());
        if let Some(path) = first {
            self.select_workflow(&path, cx);
        }
    }

    /// Opens the workflow at `path`.
    pub(crate) fn select_workflow(&mut self, path: &str, cx: &mut Context<Self>) {
        let listed = self.model.workflows.listed.as_deref().unwrap_or(&[]);
        if self.model.workflows_page.select(path, listed) {
            let tab = self.model.workflows_page.tab();
            self.show_workflow_tab(tab, cx);
        }
        self.sync_workflow_source(cx);
        cx.notify();
    }

    /// Shows a tab. The Runs tab asks the engine for the open workflow's runs.
    pub(crate) fn show_workflow_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if let Some(kind) = self.model.workflows_page.show_tab(tab) {
            let id = self.engine.dispatch(Actor::Human, kind);
            self.model.workflows_page.track_runs(id);
        }
        cx.notify();
    }

    pub(crate) fn ask_delete_workflow(&mut self, cx: &mut Context<Self>) {
        self.model.workflows_page.ask_delete();
        cx.notify();
    }

    pub(crate) fn cancel_delete_workflow(&mut self, cx: &mut Context<Self>) {
        self.model.workflows_page.cancel_delete();
        cx.notify();
    }

    /// Moves the open file to the trash. The engine's answer closes the page on it.
    pub(crate) fn confirm_delete_workflow(&mut self, cx: &mut Context<Self>) {
        if let Some(kind) = self.model.workflows_page.delete_request() {
            self.engine.dispatch(Actor::Human, kind);
            self.request_workflow_list();
        }
        self.sync_workflow_source(cx);
        cx.notify();
    }

    /// Dry Run: the engine walks the file's text and answers into the dry run panel.
    pub(crate) fn dry_run_workflow(&mut self, cx: &mut Context<Self>) {
        if let Some(kind) = self.model.workflows_page.dry_run_request() {
            let id = self.engine.dispatch(Actor::Human, kind);
            self.model.workflows_page.track_dry_run(id);
        }
        cx.notify();
    }

    fn hide_dry_run(&mut self, cx: &mut Context<Self>) {
        self.model.workflows_page.dismiss_dry_run();
        cx.notify();
    }

    /// Shows a run of the Runs list: asks the engine for its steps.
    pub(crate) fn open_workflow_run(&mut self, run: i64, cx: &mut Context<Self>) {
        if let Some(kind) = self.model.workflows_page.open_run(run) {
            let id = self.engine.dispatch(Actor::Human, kind);
            self.model.workflows_page.track_run(id);
        }
        cx.notify();
    }

    /// A run finished: the Runs list is read again, and the shown run's steps too.
    pub(crate) fn workflow_run_ended(&mut self, run: i64, cx: &mut Context<Self>) {
        let ended_workflow = self
            .model
            .workflows
            .runs
            .get(&run)
            .map(|view| view.workflow.clone());
        let page = &self.model.workflows_page;
        let same_workflow = ended_workflow.is_some() && ended_workflow.as_deref() == page.name();
        if same_workflow && page.tab() == Tab::Runs {
            self.show_workflow_tab(Tab::Runs, cx);
        }
        if let Some(kind) = self.model.workflows_page.refresh_run(run) {
            let id = self.engine.dispatch(Actor::Human, kind);
            self.model.workflows_page.track_run(id);
        }
    }

    /// Asks the engine to check the file's text, unless it has checked this text already.
    pub(crate) fn check_workflow(&mut self, cx: &mut Context<Self>) {
        if let Some(kind) = self.model.workflows_page.check_request() {
            let id = self.engine.dispatch(Actor::Human, kind);
            self.model.workflows_page.track_check(id);
        }
        cx.notify();
    }

    /// Makes the window's shortcuts for the listed manual workflows, and keeps the plan, whose
    /// warnings the Script tab shows. Called when the listing arrives.
    pub(crate) fn bind_workflow_keys(&mut self, cx: &mut Context<Self>) {
        let manual: Vec<(String, String)> = self
            .model
            .workflows
            .listed
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter(|info| info.trigger == "manual" && info.error.is_none())
            .filter_map(|info| info.key.clone().map(|key| (info.name.clone(), key)))
            .collect();
        let built_in = crate::built_in_keys(cx);
        let plan = workflow_keys::plan(&manual, &built_in, cfg!(target_os = "macos"));
        crate::bind_keys_with_workflows(cx, &plan.bound);
        self.model.workflows.keys = plan;
    }

    /// Brings a diagnostic's line into view and marks it (1-based; 0 names no line).
    fn go_to_workflow_line(&mut self, line: u32, cx: &mut Context<Self>) {
        let Some(index) = script_view::line_index(line, self.script_view.text.line_count()) else {
            return;
        };
        self.script_view.marked = Some(index);
        self.script_view
            .scroll
            .scroll_to_item(index.saturating_sub(3), ScrollStrategy::Top);
        cx.notify();
    }

    /// Shows the page's text in the Script tab when it is another text than the one on screen
    /// (a file opened, or edited in the person's editor and listed again), and checks it. The
    /// text shows plain at once; its colors are computed off the UI thread and join it when
    /// they are ready, unless another text has taken its place.
    pub(crate) fn sync_workflow_source(&mut self, cx: &mut Context<Self>) {
        let source = self.model.workflows_page.text();
        if let Some(plain) = self.script_view.text.plain(source) {
            self.script_view.text = plain;
            self.script_view.columns = 0;
            self.script_view.marked = None;
            let source = source.to_owned();
            cx.spawn(async move |this, cx| {
                let text = source.clone();
                let spans = cx
                    .background_spawn(async move {
                        gm_core::syntax::language_for_path("workflow.lua").and_then(|language| {
                            gm_core::syntax::highlight(language, &text, &AtomicBool::new(false))
                        })
                    })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    if let Some(spans) = spans
                        && view.script_view.text.source() == source
                        && !view.script_view.text.is_colored()
                    {
                        view.script_view.text = ScriptText::colored(&source, spans);
                        cx.notify();
                    }
                });
            })
            .detach();
            cx.notify();
        }
        self.check_workflow(cx);
    }

    /// A wheel step with a sideways part moves the script's text.
    fn scroll_script_across(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let dx = event.delta.pixel_delta(px(SCRIPT_LINE_HEIGHT)).x;
        if dx == px(0.) {
            return;
        }
        let view = &mut self.script_view;
        view.columns = if dx < px(0.) {
            (view.columns + SCRIPT_SCROLL_STEP).min(view.text.columns())
        } else {
            view.columns.saturating_sub(SCRIPT_SCROLL_STEP)
        };
        cx.notify();
    }

    /// The whole page: the list on the left, the open file on the right.
    pub(crate) fn workflows_page(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_row()
            .bg(theme::ground())
            // The page holds the focus the palette gives back, so the window's shortcuts
            // (Open Workflow in Editor, a workflow's own key) reach it.
            .track_focus(&self.list_focus)
            .child(self.workflow_list(cx))
            .child(self.workflow_detail(cx))
    }

    /// The strip on the list's right edge that resizes it. It takes the width from the caller:
    /// the view is being rendered, so the view cannot be read from its own entity here.
    fn list_width_strip(width: f32, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        resize_handle::handle(
            "workflow-list-resize",
            Edge::Right,
            width,
            resize::PANE,
            |this: &mut StatusView, width, _| this.workflow_list_width = width,
            |this: &mut StatusView, _| this.workflow_list_width = resize::WORKFLOW_LIST_DEFAULT,
            cx,
        )
    }

    fn workflow_list(&self, cx: &mut Context<Self>) -> Div {
        let width = self.workflow_list_width;
        let width_strip = Self::list_width_strip(width, cx);
        let listed = self.model.workflows.listed.as_deref();
        let selected = self.model.workflows_page.path();
        let all = listed.unwrap_or(&[]);
        // Your workflows first, then the built-in ones under their own heading (`WF4b`).
        let rows = workflow_rows(all, false, selected, cx);
        let built_in = workflow_rows(all, true, selected, cx);
        let empty = match listed {
            None => "Reading workflows…",
            Some([]) => "No workflows yet.",
            Some(_) => "",
        };
        let templates = self
            .model
            .workflows
            .templates
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let template = name.clone();
                div()
                    .id(10_000 + index)
                    .h(px(24.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::selected_row()))
                    .child(name.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.run_action(
                                Action::NewWorkflowFromTemplate,
                                &Args::Template(template.clone()),
                                window,
                                cx,
                            );
                        }),
                    )
            });
        div()
            .relative()
            .flex_none()
            .w(px(width))
            .h_full()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::hairline())
            .bg(theme::panel())
            .child(list_title())
            .child(
                div()
                    .id("workflow-list")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(
                        self.model
                            .workflows
                            .ignored
                            .iter()
                            .map(|line| ignored_line(line)),
                    )
                    .children(
                        (!empty.is_empty())
                            .then(|| div().px_2().py_1().text_color(theme::muted()).child(empty)),
                    )
                    .children(rows)
                    .children((!built_in.is_empty()).then(|| section_label("Built-in")))
                    .children(built_in)
                    .child(
                        div()
                            .mt_3()
                            .px_2()
                            .text_xs()
                            .text_color(theme::muted())
                            .child("New from Template…"),
                    )
                    .children(templates),
            )
            .child(width_strip)
    }

    fn workflow_detail(&self, cx: &Context<Self>) -> Div {
        let page = &self.model.workflows_page;
        let Some(name) = page.name() else {
            return div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::muted())
                .child("Select a workflow to see its script and its runs.");
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(self.workflow_header(name, cx))
            .children(self.review_band(name, cx))
            .children(self.workflow_banners(cx))
            .child(tab_bar(page.tab(), cx))
            .child(match page.tab() {
                Tab::Script => self.script_body(cx),
                Tab::Runs => self.runs_body(cx),
            })
            .children(
                (page.tab() == Tab::Script)
                    .then(|| self.dry_run_panel(cx))
                    .flatten(),
            )
    }

    fn workflow_header(&self, name: &str, cx: &Context<Self>) -> Div {
        let page = &self.model.workflows_page;
        let path = page.path().unwrap_or_default();
        let builtin = page.is_builtin();
        let info = self.selected_workflow_info();
        let shown = info.map_or_else(|| name.to_owned(), |info| row_title(info).to_owned());
        let location = if builtin {
            "Built into GitMaster: read only".to_owned()
        } else {
            path.to_owned()
        };
        let run_reason = Action::RunWorkflow
            .enabled(&Args::Workflow(name.to_owned()), &self.action_context(""))
            .err();
        div()
            .px_4()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .border_b_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(shown))
                            .child(
                                div()
                                    .font_family(theme::code_font())
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(location),
                            ),
                    )
                    .child(self.header_actions(name, builtin, run_reason.is_some(), cx)),
            )
            .children(run_reason.map(|reason| {
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(format!("Run is off: {reason}"))
            }))
    }

    /// The buttons of the header: Run, Dry Run and, for a file the person wrote, Open in Editor,
    /// Copy Path and Delete; for a built-in one, Copy to Edit (`WF4b`). `run_off` says Run is
    /// refused.
    fn header_actions(&self, name: &str, builtin: bool, run_off: bool, cx: &Context<Self>) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(action_button(
                "workflow-run",
                "Run",
                Action::RunWorkflow,
                Args::Workflow(name.to_owned()),
                run_off,
                cx,
            ))
            .child(self.dry_run_button(cx))
            .children((!builtin).then(|| {
                action_button(
                    "workflow-open-in-editor",
                    "Open in Editor",
                    Action::OpenWorkflowInEditor,
                    Args::None,
                    false,
                    cx,
                )
            }))
            .children((!builtin).then(|| {
                action_button(
                    "workflow-copy-path",
                    "Copy Path",
                    Action::CopyWorkflowPath,
                    Args::None,
                    false,
                    cx,
                )
            }))
            .children((!builtin).then(|| {
                action_button(
                    "workflow-delete",
                    "Delete…",
                    Action::DeleteWorkflow,
                    Args::None,
                    false,
                    cx,
                )
            }))
            .children(builtin.then(|| {
                band_button("workflow-copy", "Copy to Edit", cx, |this, _, cx| {
                    let kind = this.selected_workflow_info().and_then(copy_request);
                    if let Some(kind) = kind {
                        this.send_human(kind, cx);
                    }
                })
            }))
    }

    /// The review line of the open workflow and its run controls (`WF3b`): Review, Revoke, Approve,
    /// Reject and Cancel. Nothing when the file does not load and no run is active.
    fn review_band(&self, name: &str, cx: &Context<Self>) -> Option<Div> {
        let info = self.selected_workflow_info();
        let line = review_line(info);
        let controls: RunControls = run_controls(
            self.model.workflows.awaiting_approval_of(name),
            self.model.workflows.active_of(name),
        );
        if info.is_none() && controls == RunControls::default() {
            return None;
        }
        let mut row = div()
            .px_4()
            .py_1()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::hairline())
            .text_sm();
        if let Some(info) = info {
            row = row.child(switch_row(info, cx));
        }
        if let (Some(line), Some(info)) = (line, info) {
            row = row.child(review_row(&line, &info.hash, cx));
        }
        if let Some((run, summary)) = controls.approve {
            // A restack's approval lists the plan under its line; the line names the call only
            // (its summary's first two lines), since the plan says what moves.
            let plan = self.model.workflows.approval_plan(run);
            let line = if plan.is_some() {
                summary.lines().take(2).collect::<Vec<_>>().join(" ")
            } else {
                summary
            };
            row = row.child(approval_row(run, &line, cx));
            if let Some(plan) = plan {
                row = row.child(approval_plan_block(plan));
            }
        }
        if let Some(run) = controls.cancel {
            row = row.child(band_button(
                "workflow-cancel-run",
                "Cancel run",
                cx,
                move |this, _, cx| {
                    this.send_human(gm_schema::CommandKind::CancelWorkflowRun { run }, cx);
                },
            ));
        }
        Some(row)
    }

    /// Sends a human command from the Workflows page, and redraws.
    fn send_human(&mut self, kind: gm_schema::CommandKind, cx: &mut Context<Self>) {
        self.engine.dispatch(Actor::Human, kind);
        cx.notify();
    }

    /// The lines between the header and the tabs: a question the page asks.
    fn workflow_banners(&self, cx: &Context<Self>) -> Vec<Div> {
        let page = &self.model.workflows_page;
        let mut banners = Vec::new();
        if page.asking_delete() {
            let file = page.path().map(file_name).unwrap_or_default();
            banners.push(banner(
                &format!("Move {file} to the trash?"),
                vec![
                    text_button(
                        "workflow-confirm-delete",
                        "Move to trash",
                        cx,
                        |this, cx| {
                            this.confirm_delete_workflow(cx);
                        },
                    ),
                    text_button("workflow-cancel-delete", "Cancel", cx, |this, cx| {
                        this.cancel_delete_workflow(cx);
                    }),
                ],
            ));
        }
        banners
    }

    /// The Script tab: the file's text, read-only, then how to edit it, the shortcut warning and
    /// the problems of the text.
    fn script_body(&self, cx: &Context<Self>) -> AnyElement {
        let page = &self.model.workflows_page;
        let edit_note = if page.is_builtin() {
            "Built into GitMaster. Copy to Edit makes a file of it that you can change."
        } else {
            "Read only here. Edit the file in your editor: the page follows the file, and each change makes this version unreviewed."
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(self.script_text(cx))
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(theme::hairline())
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_xs()
                    .child(div().text_color(theme::muted()).child(edit_note))
                    .children(
                        page.name()
                            .and_then(|name| self.model.workflows.key_warning(name))
                            .map(|warning| {
                                div().text_color(theme::warn_fg()).child(warning.to_owned())
                            }),
                    )
                    .children(
                        page.diagnostics()
                            .iter()
                            .enumerate()
                            .map(|(index, problem)| Self::diagnostic_row(index, problem, cx)),
                    )
                    .children(
                        (page.check_answered() && page.diagnostics().is_empty())
                            .then(|| div().text_color(theme::muted()).child("No problems found.")),
                    )
                    .children(
                        (!page.check_answered())
                            .then(|| div().text_color(theme::muted()).child("Checking…")),
                    ),
            )
            .into_any_element()
    }

    /// The lines of the script: only the ones on screen are built.
    fn script_text(&self, cx: &Context<Self>) -> AnyElement {
        let count = self.script_view.text.line_count();
        if count == 0 {
            return div()
                .id("workflow-script")
                .flex_1()
                .min_h(px(0.))
                .p_4()
                .text_sm()
                .text_color(theme::muted())
                .child("This file is empty.")
                .into_any_element();
        }
        let digits = u16::try_from(count.to_string().len().max(3)).unwrap_or(3);
        let gutter = SCRIPT_DIGIT_WIDTH * f32::from(digits) + 24.;
        // The lines of the problems, so each shows in the text too.
        let problems: Vec<usize> = self
            .model
            .workflows_page
            .diagnostics()
            .iter()
            .filter_map(|problem| script_view::line_index(problem.line, count))
            .collect();
        div()
            .id("workflow-script")
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(
                uniform_list(
                    "workflow-script-lines",
                    count,
                    cx.processor(
                        move |this: &mut StatusView,
                              range: Range<usize>,
                              _window: &mut Window,
                              _cx: &mut Context<StatusView>| {
                            let view = &this.script_view;
                            range
                                .map(|index| {
                                    script_line(
                                        index,
                                        &view.text.line(index, view.columns),
                                        gutter,
                                        view.marked == Some(index),
                                        problems.contains(&index),
                                    )
                                })
                                .collect()
                        },
                    ),
                )
                .track_scroll(self.script_view.scroll.clone())
                .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                    this.scroll_script_across(event, cx);
                }))
                .flex_1()
                .min_h(px(0.))
                .font_family(theme::code_font()),
            )
            .into_any_element()
    }

    /// The Runs tab: the newest runs above, the chosen run's view below.
    fn runs_body(&self, cx: &Context<Self>) -> AnyElement {
        let page = &self.model.workflows_page;
        let list = match (page.runs(), page.runs_failed()) {
            (_, true) => message_line("The runs could not be read. The status bar says why.")
                .into_any_element(),
            (None, false) => message_line("Loading runs…").into_any_element(),
            (Some([]), false) => message_line("No runs yet.").into_any_element(),
            (Some(runs), false) => div()
                .id("workflow-runs")
                .flex_none()
                .h(px(RUNS_LIST_HEIGHT))
                .overflow_y_scroll()
                .border_b_1()
                .border_color(theme::hairline())
                .children(runs.iter().enumerate().map(|(index, run)| {
                    run_row(index, run, page.selected_run() == Some(run.run), cx)
                }))
                .into_any_element(),
        };
        let view = match page.detail() {
            Some(detail) if Some(detail.summary.run) == page.selected_run() => {
                self.run_view(detail, cx)
            }
            _ if page.selected_run().is_some() => message_line("Loading run…"),
            _ => message_line("Select a run to see its steps."),
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(list)
            .child(
                div()
                    .id("workflow-run-view")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p_4()
                    .child(view),
            )
            .into_any_element()
    }

    /// One run: its status and times, its context, then its steps in order.
    fn run_view(&self, detail: &WorkflowRunDetail, cx: &Context<Self>) -> Div {
        let page = &self.model.workflows_page;
        let summary = &detail.summary;
        let now = now_secs();
        let duration = run_duration(summary).map(duration_label);
        let mut facts = vec![
            format!("started by {}", summary.started_by),
            format!("started {}", relative_time(now, summary.started_at / 1000)),
        ];
        if let Some(ended) = summary.ended_at {
            facts.push(format!("ended {}", relative_time(now, ended / 1000)));
        }
        if let Some(duration) = duration {
            facts.push(format!("took {duration}"));
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(status_color(summary.status))
                                    .child(status_label(summary.status)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::muted())
                                    .child(summary.workflow.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(facts.join(" · ")),
                    )
                    .children(summary.end_reason.as_ref().map(|reason| {
                        div()
                            .text_sm()
                            .text_color(theme::error())
                            .child(reason.clone())
                    })),
            )
            .child(section_label("Context"))
            .children(context_blocks(detail).into_iter().map(|(label, text)| {
                let key = context_key(summary.run, &label.to_ascii_lowercase());
                json_block(label, &text, &key, page.is_expanded(&key), cx)
            }))
            .child(section_label("Calls"))
            .children(
                step_rows(detail)
                    .iter()
                    .enumerate()
                    .map(|(index, step)| self.step_card(summary.run, index, step, cx)),
            )
    }

    /// One `ctx` call of a run: its name, what it did, how it ended, and its input and output.
    fn step_card(&self, run: i64, index: usize, step: &StepRecord, cx: &Context<Self>) -> Div {
        let page = &self.model.workflows_page;
        let input_key = expand_key(run, index, "input");
        let output_key = expand_key(run, index, "output");
        let title = format!("{}. {}", step.seq, step.func);
        let took = step_duration(step).map(duration_label);
        let output_label = "Output";
        div()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(theme::hairline())
            .bg(theme::panel())
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .font_family(theme::code_font())
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .children(
                        took.map(|took| div().text_xs().text_color(theme::muted()).child(took)),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(step_summary(step)),
            )
            .child(json_block(
                "Input",
                &input_text(step),
                &input_key,
                page.is_expanded(&input_key),
                cx,
            ))
            .child(json_block(
                output_label,
                &output_text(step),
                &output_key,
                page.is_expanded(&output_key),
                cx,
            ))
    }
}

/// A line of the list's top: a file the listing did not read, and why.
fn ignored_line(line: &str) -> Div {
    div()
        .px_2()
        .text_xs()
        .text_color(theme::warn_fg())
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(format!("Ignored: {line}"))
}

/// One workflow in the list: its name, trigger and key, and a red dot with the
/// first error when it does not load.
/// The rows of the listed workflows that are built-in (`built`) or the user's; each row keeps its
/// index in the listing as its id.
fn workflow_rows(
    all: &[WorkflowInfo],
    built: bool,
    selected: Option<&str>,
    cx: &Context<StatusView>,
) -> Vec<Stateful<Div>> {
    all.iter()
        .enumerate()
        .filter(|(_, info)| info.builtin() == built)
        .map(|(index, info)| {
            let is_selected = selected == Some(info.path.as_str());
            workflow_row(index, info, is_selected, cx)
        })
        .collect()
}

fn workflow_row(
    index: usize,
    info: &WorkflowInfo,
    selected: bool,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let path = info.path.clone();
    let trigger = match &info.key {
        Some(key) => format!("{} · {key}", info.trigger),
        None => info.trigger.clone(),
    };
    div()
        .id(index)
        .px_2()
        .py_1p5()
        .rounded_md()
        .cursor_pointer()
        .flex()
        .flex_col()
        .gap_0p5()
        .when(selected, |row| row.bg(theme::selected_row()))
        .when(!selected, |row| {
            row.hover(|style| style.bg(theme::selected_row()))
        })
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .font_weight(if selected {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .child(info.name.clone()),
                )
                .children(info.error.is_some().then(|| {
                    div()
                        .flex_none()
                        .size(px(8.))
                        .rounded_full()
                        .bg(theme::error())
                })),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::muted())
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(trigger),
        )
        .children(info.error.as_ref().map(|error| {
            div()
                .text_xs()
                .text_color(theme::error())
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(error.clone())
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.select_workflow(&path, cx);
            }),
        )
}

fn tab_bar(current: Tab, cx: &Context<StatusView>) -> Div {
    div()
        .px_4()
        .h(px(36.))
        .flex()
        .flex_row()
        .items_end()
        .gap_1()
        .border_b_1()
        .border_color(theme::hairline())
        .children(Tab::ALL.into_iter().map(|tab| {
            let selected = tab == current;
            div()
                .id(tab.label())
                .px_3()
                .h(px(32.))
                .flex()
                .items_center()
                .cursor_pointer()
                .border_b_2()
                .border_color(if selected {
                    theme::accent()
                } else {
                    theme::hairline()
                })
                .when(selected, |item| item.font_weight(FontWeight::MEDIUM))
                .when(!selected, |item| item.text_color(theme::muted()))
                .child(tab.label())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.show_workflow_tab(tab, cx);
                    }),
                )
        }))
}

impl StatusView {
    /// Dry Run, in the page header: faded while the file does not parse.
    fn dry_run_button(&self, cx: &Context<Self>) -> Stateful<Div> {
        let disabled = Action::DryRunWorkflow
            .enabled(&Args::None, &self.action_context(""))
            .is_err();
        action_button(
            "workflow-dry-run",
            "Dry Run",
            Action::DryRunWorkflow,
            Args::None,
            disabled,
            cx,
        )
    }

    /// The dry run at the foot of the Script tab: the stop line, the inputs the walk
    /// left blank, then one card per step. A stale or running dry run says so.
    fn dry_run_panel(&self, cx: &Context<Self>) -> Option<Stateful<Div>> {
        let page = &self.model.workflows_page;
        let result = page.dry_run_result();
        if result.is_none() && !page.dry_run_pending() && !page.dry_run_stale() {
            return None;
        }
        let body = if page.dry_run_pending() {
            vec![message_line("Walking the file…")]
        } else if let Some(dry) = result {
            self.dry_run_body(dry, cx)
        } else {
            vec![message_line(
                "The file changed since the last dry run. Dry Run again to walk it.",
            )]
        };
        Some(
            div()
                .id("workflow-dry-run")
                .flex_none()
                .max_h(px(DRY_RUN_HEIGHT))
                .overflow_y_scroll()
                .px_4()
                .py_2()
                .flex()
                .flex_col()
                .gap_2()
                .border_t_1()
                .border_color(theme::hairline())
                .bg(theme::panel())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(section_label("Dry run"))
                        .child(text_button("workflow-dry-hide", "Hide", cx, |this, cx| {
                            this.hide_dry_run(cx);
                        })),
                )
                .children(body),
        )
    }

    fn dry_run_body(&self, dry: &WorkflowDryRun, cx: &Context<Self>) -> Vec<Div> {
        let mut lines = Vec::new();
        if let Some(error) = &dry.error {
            lines.push(
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(error.clone()),
            );
        }
        if !dry.stop.is_empty() {
            lines.push(div().text_sm().child(format!("Stop: {}", dry.stop)));
        }
        if !dry.missing_inputs.is_empty() {
            lines.push(message_line(&format!(
                "Missing inputs, empty in this walk: {}",
                dry.missing_inputs.join(", ")
            )));
        }
        lines.extend(
            dry.calls
                .iter()
                .enumerate()
                .map(|(index, step)| self.dry_step_card(index, step, cx)),
        );
        lines
    }

    /// One step of a dry run: its state, action and level, whether the answer
    /// was computed or assumed, and the input it would receive.
    fn dry_step_card(&self, index: usize, step: &WorkflowDryCall, cx: &Context<Self>) -> Div {
        let page = &self.model.workflows_page;
        let key = format!("dry-run-step-{index}-input");
        let answer = if step.executed {
            "computed".to_owned()
        } else if step.assumed {
            format!("not run (assumed {})", step.outcome)
        } else {
            "not run".to_owned()
        };
        div()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(theme::hairline())
            .bg(theme::ground())
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_family(theme::code_font())
                            .font_weight(FontWeight::MEDIUM)
                            .child(step.func.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(step.note.clone()),
                    )
                    .child(
                        div()
                            .px_1p5()
                            .rounded_sm()
                            .text_xs()
                            .bg(theme::selection())
                            .text_color(theme::text())
                            .child(step.level.clone()),
                    ),
            )
            .child(div().text_xs().text_color(theme::muted()).child(answer))
            .child(json_block(
                "Input",
                &pretty_json(&step.args),
                &key,
                page.is_expanded(&key),
                cx,
            ))
    }
}

impl StatusView {
    /// One problem of the text, in the error colour. A click brings its line into view, when it
    /// has one.
    fn diagnostic_row(
        index: usize,
        problem: &WorkflowDiagnostic,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let line = problem.line;
        div()
            .id(30_000 + index)
            .text_xs()
            .text_color(theme::error())
            .when(line > 0, |row| {
                row.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.go_to_workflow_line(line, cx);
                    }),
                )
            })
            .child(diagnostic_text(problem))
    }
}

/// One line of the script: its number, in the error colour when a problem names the line, then
/// its text colored by role.
fn script_line(
    index: usize,
    pieces: &[crate::code_page::Piece],
    gutter: f32,
    marked: bool,
    problem: bool,
) -> AnyElement {
    div()
        .h(px(SCRIPT_LINE_HEIGHT))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .when(marked, |row| row.bg(theme::selected_row()))
        .child(
            div()
                .flex_none()
                .w(px(gutter))
                .pr_3()
                .text_right()
                .text_xs()
                .text_color(if problem {
                    theme::error()
                } else {
                    theme::gutter()
                })
                .child((index + 1).to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(colored(pieces)),
        )
        .into_any_element()
}

fn run_row(
    index: usize,
    run: &WorkflowRunSummary,
    selected: bool,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let number = run.run;
    let now = now_secs();
    let when = relative_time(now, run.started_at / 1000);
    let took = run_duration(run).map_or_else(|| "running".to_owned(), duration_label);
    let mut detail = format!("{when} · {took} · {}", run.started_by);
    if let Some(reason) = &run.end_reason {
        detail.push_str(" · ");
        detail.push_str(reason);
    }
    div()
        .id(index)
        .px_3()
        .py_1p5()
        .flex()
        .flex_col()
        .gap_0p5()
        .cursor_pointer()
        .when(selected, |row| row.bg(theme::selected_row()))
        .when(!selected, |row| {
            row.hover(|style| style.bg(theme::selected_row()))
        })
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(
                    div()
                        .text_color(status_color(run.status))
                        .font_weight(FontWeight::MEDIUM)
                        .child(status_label(run.status)),
                )
                .child(
                    div()
                        .font_family(theme::code_font())
                        .text_xs()
                        .text_color(theme::muted())
                        .child(format!("#{number}")),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::muted())
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(detail),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.open_workflow_run(number, cx);
            }),
        )
}

/// A monospace block with a heading. Folded to a few lines, with "Show all" when
/// it is longer; the toggle is kept on the page by `key`.
fn json_block(label: &str, text: &str, key: &str, expanded: bool, cx: &Context<StatusView>) -> Div {
    let Preview { text: shown, .. } = preview(text, expanded);
    let folded = preview(text, false).hidden;
    let owned_key = key.to_owned();
    let toggle = (folded > 0).then(|| {
        let label = if expanded {
            "Show less".to_owned()
        } else {
            format!("Show all ({folded} more lines)")
        };
        div()
            .id(SharedString::from(key.to_owned()))
            .text_xs()
            .text_color(theme::accent())
            .cursor_pointer()
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.model.workflows_page.toggle_expanded(&owned_key);
                    cx.notify();
                }),
            )
    });
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(theme::muted())
                .child(label.to_owned()),
        )
        .child(
            div()
                .p_2()
                .rounded_md()
                .bg(theme::editor())
                .border_1()
                .border_color(theme::hairline())
                .font_family(theme::code_font())
                .text_xs()
                .overflow_hidden()
                .children(shown.lines().map(|line| {
                    div().whitespace_nowrap().child(if line.is_empty() {
                        " ".to_owned()
                    } else {
                        line.to_owned()
                    })
                })),
        )
        .children(toggle)
}

fn status_color(status: WorkflowRunStatus) -> Hsla {
    match status {
        WorkflowRunStatus::Done => theme::added_fg(),
        WorkflowRunStatus::Failed | WorkflowRunStatus::Blocked => theme::error(),
        WorkflowRunStatus::Running => theme::accent(),
        WorkflowRunStatus::Waiting | WorkflowRunStatus::AwaitingApproval => theme::warn_fg(),
        WorkflowRunStatus::Cancelled | WorkflowRunStatus::Interrupted => theme::muted(),
    }
}

/// A header button that runs an action. It is drawn faded when the action is
/// not enabled; a click then says why in the status bar.
fn action_button(
    id: &'static str,
    label: &'static str,
    action: Action,
    args: Args,
    disabled: bool,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(theme::button_border())
        .cursor_pointer()
        .when(disabled, |button| button.opacity(0.5))
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.run_action(action, &args, window, cx);
            }),
        )
}

/// The review line: what the version is, with Review when it is not reviewed or Revoke when it is.
fn review_row(line: &ReviewLine, hash: &str, cx: &Context<StatusView>) -> Div {
    let colour = match line {
        ReviewLine::Unreviewed => theme::warn_fg(),
        ReviewLine::Reviewed { .. } => theme::muted(),
    };
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(div().text_color(colour).child(review_text(line)));
    match line {
        ReviewLine::Unreviewed => row.child(band_button(
            "workflow-review",
            "Review",
            cx,
            StatusView::open_page_review,
        )),
        ReviewLine::Reviewed { .. } => {
            let hash = hash.to_owned();
            row.child(band_button(
                "workflow-revoke",
                "Revoke",
                cx,
                move |this, _, cx| {
                    this.send_human(
                        gm_schema::CommandKind::RevokeWorkflowReview { hash: hash.clone() },
                        cx,
                    );
                },
            ))
        }
    }
}

/// "Enabled in this repository" (`WF4b`, D164(1)): the switch of the open workflow. A built-in one
/// that is off opens the Switch On dialog; the rest switch at once.
fn switch_row(info: &WorkflowInfo, cx: &Context<StatusView>) -> Div {
    let mark = if info.enabled {
        icon::filled(Icon::CheckSquare, icon::SMALL, theme::accent())
    } else {
        icon::view(Icon::Square, icon::SMALL, theme::muted())
    };
    let click = switch_click(info);
    let text = switch_text(info);
    let info = info.clone();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            div()
                .id("workflow-enabled")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .child(mark)
                .child(
                    div()
                        .text_color(theme::text())
                        .child("Enabled in this repository"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| match &click {
                        SwitchClick::Send(kind) => this.send_human((**kind).clone(), cx),
                        SwitchClick::Ask => this.open_switch_dialog(&info, window, cx),
                    }),
                ),
        )
        .child(div().text_xs().text_color(theme::muted()).child(text))
}

/// What a restack's approval would move and hold back (`WF4c`): the headline, then each branch
/// with what the plan does to it, the held ones in the warning colour, and what stops the plan.
fn approval_plan_block(plan: &gm_schema::RestackPlan) -> Div {
    let lines = restack_dialog::outline(plan).into_iter().map(|line| {
        div()
            .flex()
            .flex_row()
            .items_start()
            .gap_2()
            .child(
                div()
                    .font_family(theme::code_font())
                    .text_color(theme::text())
                    .child(line.name),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_color(if line.held {
                        theme::warn_fg()
                    } else {
                        theme::muted()
                    })
                    .child(line.text),
            )
    });
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .text_sm()
        .child(
            div()
                .text_color(theme::muted())
                .child(restack_dialog::approval_headline(plan)),
        )
        .children(lines)
        .children(
            plan.stopped
                .iter()
                .map(|reason| div().text_color(theme::warn_fg()).child(reason.clone())),
        )
}

/// The run that waits for an approval, with what it asks, and Approve and Reject.
fn approval_row(run: i64, summary: &str, cx: &Context<StatusView>) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            div()
                .text_color(theme::warn_fg())
                .child(format!("Waiting for approval: {summary}")),
        )
        .child(band_button(
            "workflow-approve",
            "Approve",
            cx,
            move |this, _, cx| {
                this.send_human(
                    gm_schema::CommandKind::ApproveWorkflowStep {
                        run,
                        approved: true,
                    },
                    cx,
                );
            },
        ))
        .child(band_button(
            "workflow-reject",
            "Reject",
            cx,
            move |this, _, cx| {
                this.send_human(
                    gm_schema::CommandKind::ApproveWorkflowStep {
                        run,
                        approved: false,
                    },
                    cx,
                );
            },
        ))
}

/// A text button of the review band. It runs `run` with the window, when the person clicks it.
fn band_button(
    id: &'static str,
    label: &'static str,
    cx: &Context<StatusView>,
    run: impl Fn(&mut StatusView, &mut Window, &mut Context<StatusView>) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded_md()
        .cursor_pointer()
        .text_color(theme::accent())
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                run(this, window, cx);
            }),
        )
}

/// A plain text button of a banner.
fn text_button(
    id: &'static str,
    label: &'static str,
    cx: &Context<StatusView>,
    run: fn(&mut StatusView, &mut Context<StatusView>),
) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded_md()
        .cursor_pointer()
        .text_color(theme::accent())
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                run(this, cx);
            }),
        )
}

fn banner(text: &str, buttons: Vec<Stateful<Div>>) -> Div {
    div()
        .px_4()
        .py_1()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .bg(theme::warn_bg())
        .text_sm()
        .child(text.to_owned())
        .child(div().flex().flex_row().gap_1().children(buttons))
}

fn section_label(text: &str) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::muted())
        .child(text.to_owned())
}

fn message_line(text: &str) -> Div {
    div()
        .py_4()
        .text_color(theme::muted())
        .child(text.to_owned())
}

/// The file's name, for a sentence about it.
fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|file| file.to_str())
        .unwrap_or(path)
        .to_owned()
}

/// The heading of the workflows list.
fn list_title() -> Div {
    div()
        .px_3()
        .h(px(44.))
        .flex()
        .items_center()
        .font_weight(FontWeight::MEDIUM)
        .child("Workflows")
}
