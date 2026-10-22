//! The Workflows page (design/screens.md, D76): the workflow files on the left; on the
//! right the open file with its Run, Save, Revert and Delete buttons and three
//! tabs. Flow shows the states as cards, TOML is the editable text with its
//! diagnostics, and Runs lists the runs and shows one run step by step. The
//! state is in `workflows_page`; this file draws it and sends what the user asks
//! for to the engine, through the action registry where one exists.

use std::path::Path;

use gm_core::workflow::{Diagnostic, Severity};
use gm_schema::{
    Actor, WorkflowDryRun, WorkflowDryStep, WorkflowInfo, WorkflowRunDetail, WorkflowRunStatus,
    WorkflowRunSummary, WorkflowStepRecord,
};
use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, IntoElement, MouseButton, MouseDownEvent, Pixels,
    Point, SharedString, Stateful, Window, div, prelude::*, px,
};

use crate::SaveWorkflow;
use crate::action_registry::{Action, Args};
use crate::context_menu::clamp_origin;
use crate::flow_graph;
use crate::flow_graph_view;
use crate::shell_history::{now_secs, relative_time};
use crate::theme;
use crate::view::{Screen, StatusView};
use crate::workflows_page::{
    Choice, Edge, FlowEdit, FlowRow, Preview, SaveState, Tab, context_blocks, context_key,
    diagnostic_text, duration_label, expand_key, flow_rows, input_text, outcome_line, output_text,
    pretty_json, preview, run_duration, status_label, step_duration, visit_label,
};

/// The width of the workflow list on the left.
const LIST_WIDTH: f32 = 260.;
/// The height of the runs list above a run's view.
const RUNS_LIST_HEIGHT: f32 = 220.;
/// The most of the dry run panel shown; it scrolls past this.
const DRY_RUN_HEIGHT: f32 = 260.;
/// The target picker's width and the height of its rows.
const PICKER_WIDTH: f32 = 240.;
const PICKER_ROW_HEIGHT: f32 = 24.;

impl StatusView {
    /// Ctrl or ⌘ S: the registry's Save Workflow.
    pub(crate) fn save_workflow_action(
        &mut self,
        _: &SaveWorkflow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_action(Action::SaveWorkflow, &Args::None, window, cx);
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

    /// Opens the workflow at `path`. With unsaved changes, the page asks first.
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
        self.commit_pending_flow_fields(cx);
        if let Some(kind) = self.model.workflows_page.show_tab(tab) {
            let id = self.engine.dispatch(Actor::Human, kind);
            self.model.workflows_page.track_runs(id);
        }
        cx.notify();
    }

    /// Writes the buffer. `overwrite` writes over a file that changed on disk.
    pub(crate) fn save_workflow(&mut self, overwrite: bool, cx: &mut Context<Self>) {
        self.commit_pending_flow_fields(cx);
        if let Some(kind) = self.model.workflows_page.save_request(overwrite) {
            let id = self.engine.dispatch(Actor::Human, kind);
            self.model.workflows_page.track_save(id);
        }
        cx.notify();
    }

    pub(crate) fn revert_workflow(&mut self, cx: &mut Context<Self>) {
        self.model.workflows_page.revert();
        self.sync_workflow_source(cx);
        cx.notify();
    }

    /// Takes the file as the listing last read it, after a conflict.
    pub(crate) fn reload_workflow(&mut self, cx: &mut Context<Self>) {
        self.model.workflows_page.reload();
        self.sync_workflow_source(cx);
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

    /// The answer to "Unsaved changes": drops the buffer and opens the file asked for.
    pub(crate) fn discard_workflow_changes(&mut self, cx: &mut Context<Self>) {
        let listed = self.model.workflows.listed.as_deref().unwrap_or(&[]);
        self.model.workflows_page.discard_and_switch(listed);
        let tab = self.model.workflows_page.tab();
        if self.model.workflows_page.path().is_some() {
            self.show_workflow_tab(tab, cx);
        }
        self.sync_workflow_source(cx);
        cx.notify();
    }

    pub(crate) fn keep_workflow_editing(&mut self, cx: &mut Context<Self>) {
        self.model.workflows_page.keep_editing();
        cx.notify();
    }

    /// A Flow control: the page applies the edit and the editor takes the new text.
    pub(crate) fn apply_flow_edit(&mut self, edit: FlowEdit, cx: &mut Context<Self>) {
        self.model.workflows_page.apply_flow_edit(edit);
        self.sync_workflow_edit(cx);
        cx.notify();
    }

    /// Selects a state of the Flow.
    pub(crate) fn select_flow_state(&mut self, id: &str, cx: &mut Context<Self>) {
        self.model.workflows_page.select_state(id);
        cx.notify();
    }

    /// Opens the target picker over one edge of the selected state, under the pointer.
    fn open_flow_picker(
        &mut self,
        state: &str,
        outcome: &str,
        index: usize,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let at = (f32::from(at.x), f32::from(at.y));
        self.model
            .workflows_page
            .open_picker(state, outcome, index, at);
        cx.notify();
    }

    fn close_flow_picker(&mut self, cx: &mut Context<Self>) {
        self.model.workflows_page.close_picker();
        cx.notify();
    }

    /// The picker's choice: the edge is set and the editor takes the new text.
    fn pick_flow_target(&mut self, choice: &Choice, cx: &mut Context<Self>) {
        self.model.workflows_page.pick_target(choice);
        self.sync_workflow_edit(cx);
        cx.notify();
    }

    /// Dry Run: the engine walks the buffer and answers into the Flow's panel.
    pub(crate) fn dry_run_workflow(&mut self, cx: &mut Context<Self>) {
        self.commit_pending_flow_fields(cx);
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

    /// Puts the buffer in the editor as one undoable change, so the editor's undo
    /// takes back a Flow edit. Opening, switching and reloading keep `sync_workflow_source`.
    fn sync_workflow_edit(&mut self, cx: &mut Context<Self>) {
        let text = self.model.workflows_page.text().to_owned();
        self.workflow_source.update(cx, |area, cx| {
            if area.text() != text {
                area.replace_text(&text, cx);
            }
        });
    }

    /// Shows a run of the Runs list: asks the engine for its steps.
    pub(crate) fn open_workflow_run(&mut self, run: i64, cx: &mut Context<Self>) {
        if let Some(kind) = self.model.workflows_page.open_run(run) {
            let id = self.engine.dispatch(Actor::Human, kind);
            self.model.workflows_page.track_run(id);
        }
        cx.notify();
    }

    /// Shows the open run's path in the Flow tab.
    pub(crate) fn show_run_in_flow(&mut self, cx: &mut Context<Self>) {
        self.model.workflows_page.show_run_in_flow();
        cx.notify();
    }

    /// Stops showing a run's path in the Flow tab.
    pub(crate) fn clear_run_path(&mut self, cx: &mut Context<Self>) {
        self.model.workflows_page.clear_run_path();
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

    /// Puts the page's buffer in the editor, when they differ.
    pub(crate) fn sync_workflow_source(&mut self, cx: &mut Context<Self>) {
        let text = self.model.workflows_page.text().to_owned();
        self.workflow_source.update(cx, |area, cx| {
            if area.text() != text {
                area.set_text(&text, cx);
            }
        });
    }

    /// The editor changed: its text becomes the page's buffer.
    pub(crate) fn on_workflow_source_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.workflow_source.read(cx).text().to_owned();
        self.model.workflows_page.edit(&text);
        cx.notify();
    }

    /// The whole page: the list on the left, the open file on the right.
    pub(crate) fn workflows_page(&self, cx: &Context<Self>) -> Div {
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_row()
            .bg(theme::ground())
            .child(self.workflow_list(cx))
            .child(self.workflow_detail(cx))
    }

    fn workflow_list(&self, cx: &Context<Self>) -> Div {
        let listed = self.model.workflows.listed.as_deref();
        let selected = self.model.workflows_page.path();
        let dirty = self.model.workflows_page.dirty();
        let rows: Vec<Stateful<Div>> = listed
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .map(|(index, info)| {
                let is_selected = selected == Some(info.path.as_str());
                workflow_row(index, info, is_selected && dirty, is_selected, cx)
            })
            .collect();
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
            .flex_none()
            .w(px(LIST_WIDTH))
            .h_full()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::hairline())
            .bg(theme::panel())
            .child(
                div()
                    .px_3()
                    .h(px(44.))
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Workflows"),
            )
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
                        (!empty.is_empty())
                            .then(|| div().px_2().py_1().text_color(theme::muted()).child(empty)),
                    )
                    .children(rows)
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
                .child("Select a workflow to see its flow, its file and its runs.");
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(self.workflow_header(name, cx))
            .children(self.workflow_banners(cx))
            .child(tab_bar(page.tab(), cx))
            .children((page.tab() == Tab::Flow).then(|| self.flow_toolbar(cx)))
            .children(
                (page.tab() == Tab::Flow)
                    .then(|| self.path_banner(cx))
                    .flatten(),
            )
            .child(match page.tab() {
                Tab::Flow => self.flow_body(cx),
                Tab::Toml => self.toml_body(),
                Tab::Runs => self.runs_body(cx),
            })
            .children(
                (page.tab() == Tab::Flow)
                    .then(|| self.dry_run_panel(cx))
                    .flatten(),
            )
    }

    fn workflow_header(&self, name: &str, cx: &Context<Self>) -> Div {
        let page = &self.model.workflows_page;
        let path = page.path().unwrap_or_default();
        let run_reason = Action::RunWorkflow
            .enabled(&Args::Workflow(name.to_owned()), &self.action_context(""))
            .err();
        let run_name = name.to_owned();
        let dirty_note = page.dirty().then_some("Unsaved changes");
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
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(name.to_owned()),
                            )
                            .child(
                                div()
                                    .font_family(theme::code_font())
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(path.to_owned()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .children(dirty_note.map(|note| {
                                div().text_xs().text_color(theme::warn_fg()).child(note)
                            }))
                            .child(action_button(
                                "workflow-run",
                                "Run",
                                Action::RunWorkflow,
                                Args::Workflow(run_name),
                                run_reason.is_some(),
                                cx,
                            ))
                            .child(self.dry_run_button(cx))
                            .child(action_button(
                                "workflow-save",
                                "Save",
                                Action::SaveWorkflow,
                                Args::None,
                                false,
                                cx,
                            ))
                            .child(action_button(
                                "workflow-revert",
                                "Revert",
                                Action::RevertWorkflow,
                                Args::None,
                                false,
                                cx,
                            ))
                            .child(action_button(
                                "workflow-delete",
                                "Delete…",
                                Action::DeleteWorkflow,
                                Args::None,
                                false,
                                cx,
                            )),
                    ),
            )
            .children(run_reason.map(|reason| {
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(format!("Run is off: {reason}"))
            }))
    }

    /// The lines between the header and the tabs: a question the page asks, a
    /// conflict, a failed save, or the save in flight.
    fn workflow_banners(&self, cx: &Context<Self>) -> Vec<Div> {
        let page = &self.model.workflows_page;
        let mut banners = Vec::new();
        if let Some(other) = page.asked_switch() {
            let file = file_name(other);
            banners.push(banner(
                &format!("Unsaved changes: {file} is not saved. Discard them, or keep editing."),
                vec![
                    text_button("workflow-discard", "Discard", cx, |this, cx| {
                        this.discard_workflow_changes(cx);
                    }),
                    text_button("workflow-keep", "Keep editing", cx, |this, cx| {
                        this.keep_workflow_editing(cx);
                    }),
                ],
            ));
        }
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
        match page.save_state() {
            SaveState::Conflict => banners.push(banner(
                "The file changed on disk.",
                vec![
                    text_button("workflow-reload", "Reload", cx, |this, cx| {
                        this.reload_workflow(cx);
                    }),
                    text_button("workflow-overwrite", "Overwrite", cx, |this, cx| {
                        this.save_workflow(true, cx);
                    }),
                ],
            )),
            SaveState::Failed(reason) => banners.push(
                div()
                    .px_4()
                    .py_1()
                    .text_sm()
                    .text_color(theme::error())
                    .child(format!("Not saved: {reason}")),
            ),
            SaveState::Saving => banners.push(
                div()
                    .px_4()
                    .py_1()
                    .text_sm()
                    .text_color(theme::muted())
                    .child("Saving…"),
            ),
            SaveState::Idle => {}
        }
        banners
    }

    /// The line above the Flow while a run's path is shown: which run, and
    /// whether the file changed since that run started.
    fn path_banner(&self, cx: &Context<Self>) -> Option<Div> {
        let page = &self.model.workflows_page;
        let (run, _) = page.shown_path()?;
        let mut text = format!("Showing the path of run #{run}");
        if page.shown_path_file_changed() {
            text.push_str(" · the file changed since this run");
        }
        Some(
            div()
                .px_4()
                .py_1()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .text_sm()
                .text_color(theme::muted())
                .child(text)
                .child(text_button(
                    "workflow-clear-run-path",
                    "Clear",
                    cx,
                    |this, cx| {
                        this.clear_run_path(cx);
                    },
                )),
        )
    }

    /// The Flow tab: the state graph, then each state as a card, in file order.
    fn flow_body(&self, cx: &Context<Self>) -> AnyElement {
        let page = &self.model.workflows_page;
        let Some(definition) = page.definition() else {
            return scroll_body()
                .child(
                    div()
                        .text_color(theme::muted())
                        .child("This file does not parse. Fix it in the TOML tab."),
                )
                .children(page.diagnostics().iter().map(diagnostic_line))
                .into_any_element();
        };
        let (rows, unplaced) = flow_rows(definition, page.diagnostics());
        let selected = page.selected();
        let shown = page.shown_path();
        let path = shown.as_ref().map(|(_, path)| path);
        let graph = flow_graph::layout(&rows, &definition.start);
        let geometry = flow_graph::geometry(&graph, &flow_graph_view::METRICS);
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_row()
            .child(flow_graph_view::flow_graph(
                &graph, &geometry, &rows, selected, path, cx,
            ))
            .child(
                scroll_body()
                    .w(px(320.))
                    .min_w(px(320.))
                    .max_w(px(320.))
                    .flex_none()
                    .child(self.inspector_panel(cx))
                    .children(unplaced.iter().map(diagnostic_line))
                    .children(
                        rows.iter()
                            .map(|row| flow_card(row, selected == Some(row.id.as_str()), cx)),
                    ),
            )
            .into_any_element()
    }

    /// The TOML tab: the editor, then the diagnostics, errors first.
    fn toml_body(&self) -> AnyElement {
        let page = &self.model.workflows_page;
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(
                div()
                    .id("workflow-toml")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p_2()
                    .child(self.workflow_source.clone()),
            )
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
                    .child(
                        div()
                            .text_color(theme::muted())
                            .child("Saving revokes this workflow's automatic approvals."),
                    )
                    .children(page.diagnostics().iter().map(diagnostic_line))
                    .children(
                        (page.diagnostics().is_empty())
                            .then(|| div().text_color(theme::muted()).child("No problems found.")),
                    ),
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
                            )
                            .child(text_button(
                                "workflow-show-run-in-flow",
                                "Show in Flow",
                                cx,
                                |this, cx| {
                                    this.show_run_in_flow(cx);
                                },
                            )),
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
            .child(section_label("Steps"))
            .children(
                detail
                    .steps
                    .iter()
                    .enumerate()
                    .map(|(index, step)| self.step_card(summary.run, index, step, cx)),
            )
    }

    /// One step of a run: its state and visit, what it did, how it ended, and
    /// its input and output.
    fn step_card(
        &self,
        run: i64,
        index: usize,
        step: &WorkflowStepRecord,
        cx: &Context<Self>,
    ) -> Div {
        let page = &self.model.workflows_page;
        let input_key = expand_key(run, index, "input");
        let output_key = expand_key(run, index, "output");
        let title = match visit_label(step.visit) {
            Some(visit) => format!("{} · {visit}", step.state),
            None => step.state.clone(),
        };
        let action = step
            .action
            .clone()
            .unwrap_or_else(|| "wait or for_each".to_owned());
        let took = step_duration(step).map(duration_label);
        let output_label = if step.truncated {
            "Output (truncated)"
        } else {
            "Output"
        };
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
                    .child(format!("{action} · {}", outcome_line(step))),
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

/// One workflow in the list: its name, trigger and key, and a red dot with the
/// first error when it does not load. `dirty` marks the open file with unsaved changes.
fn workflow_row(
    index: usize,
    info: &WorkflowInfo,
    dirty: bool,
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
                }))
                .children(dirty.then(|| {
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme::warn_fg())
                        .child("●")
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

    /// The Flow's controls above the cards: add, remove and move the selected
    /// state. A refused edit says why in red beside them.
    fn flow_toolbar(&self, cx: &Context<Self>) -> Div {
        let page = &self.model.workflows_page;
        let ctx = self.action_context("");
        let disabled = |action: Action| action.enabled(&Args::None, &ctx).is_err();
        div()
            .px_4()
            .py_2()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(theme::hairline())
            .child(action_button(
                "workflow-flow-add",
                "Add State",
                Action::AddWorkflowState,
                Args::None,
                disabled(Action::AddWorkflowState),
                cx,
            ))
            .child(action_button(
                "workflow-flow-remove",
                "Remove",
                Action::RemoveWorkflowState,
                Args::None,
                disabled(Action::RemoveWorkflowState),
                cx,
            ))
            .child(action_button(
                "workflow-flow-up",
                "Move Up",
                Action::MoveWorkflowStateUp,
                Args::None,
                disabled(Action::MoveWorkflowStateUp),
                cx,
            ))
            .child(action_button(
                "workflow-flow-down",
                "Move Down",
                Action::MoveWorkflowStateDown,
                Args::None,
                disabled(Action::MoveWorkflowStateDown),
                cx,
            ))
            .children(page.selected().is_none().then(|| {
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child("Select a state to remove or move it.")
            }))
            .children(page.flow_error().map(|error| {
                div()
                    .text_xs()
                    .text_color(theme::error())
                    .child(error.to_owned())
            }))
    }

    /// The target picker over an edge of the selected state: the states it can
    /// go to, then Default. A click outside closes it, as the context menu does.
    pub(crate) fn workflow_picker_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let page = &self.model.workflows_page;
        let picker = page.picker()?;
        let options = page.target_options(&picker.state);
        let viewport = window.viewport_size();
        // The heading, the options, Default and the padding.
        let rows = options.len() + 2;
        let height = f32::from(u16::try_from(rows).unwrap_or(u16::MAX)) * PICKER_ROW_HEIGHT + 8.;
        let (left, top) = clamp_origin(
            picker.at,
            (PICKER_WIDTH, height),
            (f32::from(viewport.width), f32::from(viewport.height)),
        );
        let outside = div()
            .absolute()
            .size_full()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_flow_picker(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_flow_picker(cx)),
            );
        let mut list = div().flex().flex_col().py_1().child(
            div()
                .px_3()
                .h(px(PICKER_ROW_HEIGHT))
                .flex()
                .items_center()
                .text_xs()
                .text_color(theme::muted())
                .child(format!("Target of {}", picker.outcome)),
        );
        for (index, option) in options.iter().enumerate() {
            let choice = Choice::Target(option.clone());
            list = list.child(picker_row(index, option, None, choice, cx));
        }
        list = list
            .child(div().my_1().h(px(1.)).bg(theme::hairline()))
            .child(picker_row(
                options.len(),
                "Default",
                Some("short form"),
                Choice::Default,
                cx,
            ));
        Some(
            div().absolute().inset_0().child(outside).child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(PICKER_WIDTH))
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::hairline_strong())
                    .rounded_md()
                    .shadow_lg()
                    .occlude()
                    .child(list),
            ),
        )
    }

    /// The dry run at the foot of the Flow: the stop line, the inputs the walk
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
            dry.steps
                .iter()
                .enumerate()
                .map(|(index, step)| self.dry_step_card(index, step, cx)),
        );
        lines
    }

    /// One step of a dry run: its state, action and level, whether the answer
    /// was computed or assumed, and the input it would receive.
    fn dry_step_card(&self, index: usize, step: &WorkflowDryStep, cx: &Context<Self>) -> Div {
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
                            .child(step.state.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(step.action.clone()),
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
                &pretty_json(&step.input),
                &key,
                page.is_expanded(&key),
                cx,
            ))
    }
}

/// One choice of the target picker: its label, a note, and what it sets.
fn picker_row(
    index: usize,
    label: &str,
    note: Option<&str>,
    choice: Choice,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    div()
        .id(("workflow-picker", index))
        .h(px(PICKER_ROW_HEIGHT))
        .px_3()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .cursor_pointer()
        .hover(|style| style.bg(theme::selected_row()))
        .child(
            div()
                .font_family(theme::code_font())
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label.to_owned()),
        )
        .children(note.map(|note| {
            div()
                .text_xs()
                .text_color(theme::muted())
                .child(note.to_owned())
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.pick_flow_target(&choice, cx);
            }),
        )
}

/// One card of the Flow: the state, its kind and effect level, its outcomes,
/// and the diagnostics on its lines. A top-level card is clicked to select it;
/// the outcomes of the selected card open the target picker. Nested states of
/// a `for_each` have no controls: they are edited in the TOML tab.
fn flow_card(row: &FlowRow, selected: bool, cx: &Context<StatusView>) -> Div {
    let editable = row.depth == 0;
    let state = row.id.clone();
    let mut card = div()
        .ml(px(
            f32::from(u16::try_from(row.depth).unwrap_or(u16::MAX)) * 24.
        ))
        .mb_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(if selected && editable {
            theme::accent()
        } else {
            theme::hairline()
        })
        .bg(theme::panel())
        .overflow_hidden()
        .flex()
        .flex_col()
        .gap_1p5();
    if editable {
        card = card.cursor_pointer().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.commit_focused_flow_field(window, cx);
                this.select_flow_state(&state, cx);
            }),
        );
    }
    card.child(
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div()
                    .font_family(theme::code_font())
                    .font_weight(FontWeight::MEDIUM)
                    .child(row.id.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(row.kind.clone()),
            )
            .children(row.level.map(|level| {
                div()
                    .px_1p5()
                    .rounded_sm()
                    .text_xs()
                    .bg(theme::selection())
                    .text_color(theme::text())
                    .child(level)
            })),
    )
    .children(
        row.edges
            .iter()
            .enumerate()
            .map(|(position, edge)| flow_edge(row, edge, position, selected && editable, cx)),
    )
    .children(row.diagnostics.iter().map(diagnostic_line))
}

/// One outcome line of a card. On the selected card it opens the target picker
/// for that transition, under the pointer.
fn flow_edge(
    row: &FlowRow,
    edge: &Edge,
    position: usize,
    pickable: bool,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let color = if edge.dangling {
        theme::error()
    } else if edge.defaulted {
        theme::faint()
    } else {
        theme::text()
    };
    let line = div()
        .id(SharedString::from(format!(
            "flow-edge-{}-{position}",
            row.line
        )))
        .flex()
        .flex_row()
        .justify_between()
        .gap_2()
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .font_family(theme::code_font())
                .text_xs()
                .text_color(color)
                .child(edge.label()),
        );
    if !pickable {
        return line;
    }
    let (state, outcome, index) = (row.id.clone(), edge.outcome.clone(), edge.index);
    line.cursor_pointer()
        .hover(|style| style.bg(theme::selected_row()))
        .child(div().text_xs().text_color(theme::muted()).child("▾"))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                this.open_flow_picker(&state, &outcome, index, ev.position, cx);
            }),
        )
}

/// One diagnostic, in its colour: errors red, warnings amber.
pub(crate) fn diagnostic_line(diagnostic: &Diagnostic) -> Div {
    let color = match diagnostic.severity {
        Severity::Error => theme::error(),
        Severity::Warning => theme::warn_fg(),
    };
    div()
        .text_xs()
        .text_color(color)
        .child(diagnostic_text(diagnostic))
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
        WorkflowRunStatus::Cancelled => theme::muted(),
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

fn scroll_body() -> Stateful<Div> {
    div()
        .id("workflow-flow")
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .p_4()
        .flex()
        .flex_col()
}

/// The file's name, for a sentence about it.
fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|file| file.to_str())
        .unwrap_or(path)
        .to_owned()
}
