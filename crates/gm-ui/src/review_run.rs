//! The **Run review…** picker of the Review page (S5, D219 (1)): the workflows marked
//! `review = true`, with a checkbox each, and a Run button that starts each picked one through the
//! registry's ordinary Run Workflow, so the "Run this version?" review, the inputs dialog and the
//! concurrency rules apply as for any workflow. The rules of the list and of the selection are
//! pure and tested here; the window side is the `impl StatusView` at the bottom.

use std::collections::BTreeSet;

use gm_schema::{WorkflowInfo, WorkflowLocation, review_workflows};
use gpui::{
    AnyElement, ClickEvent, Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton,
    MouseDownEvent, Stateful, Window, div, prelude::*, px,
};

use crate::action_registry::{Action, Args};
use crate::icon::{self, Icon};
use crate::theme;
use crate::view::StatusView;

const WIDTH: f32 = 520.;
const TOP: f32 = 96.;
/// The rows of the list stop growing here and scroll.
const LIST_MAX_HEIGHT: f32 = 360.;

pub(crate) const TITLE: &str = "Run review";
pub(crate) const INTRO: &str = "Each picked review starts as an ordinary manual run in this repository. A review that is new or changed asks you to review it first.";
pub(crate) const EMPTY_TITLE: &str = "No review workflow yet.";
pub(crate) const EMPTY_HINT: &str = "Copy review-invariants or review-for-task from New workflow, or ask an agent to write a .lua file in .git/gitmaster/workflows/. Nothing runs until you approve it.";
pub(crate) const OFF_HERE: &str = "Off in this repository";

/// One review the picker lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickRow {
    pub name: String,
    pub description: Option<String>,
    /// Where the file is: `user` or `repository`, or `built-in`.
    pub place: &'static str,
    /// Why the row cannot be picked: the file does not load, or it is off here.
    pub problem: Option<String>,
}

impl PickRow {
    /// Whether the person can pick the row.
    #[must_use]
    pub fn pickable(&self) -> bool {
        self.problem.is_none()
    }
}

/// The place word of a location.
#[must_use]
pub fn place(location: WorkflowLocation) -> &'static str {
    match location {
        WorkflowLocation::User => "user",
        WorkflowLocation::Repository => "repository",
        WorkflowLocation::BuiltIn => "built-in",
    }
}

/// The rows of the picker: the reviews of `workflows`, in the order given. A review that does not
/// load, or is switched off in this repository, is listed with the reason and cannot be picked.
#[must_use]
pub fn pick_rows(workflows: &[WorkflowInfo]) -> Vec<PickRow> {
    review_workflows(workflows)
        .into_iter()
        .map(|workflow| PickRow {
            name: workflow.name.clone(),
            description: workflow.description.clone(),
            place: place(workflow.location),
            problem: match (&workflow.error, workflow.enabled) {
                (Some(error), _) => Some(error.clone()),
                (None, false) => Some(OFF_HERE.to_owned()),
                (None, true) => None,
            },
        })
        .collect()
}

/// The picker's state: the rows and the names picked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunPicker {
    rows: Vec<PickRow>,
    selected: BTreeSet<String>,
}

impl RunPicker {
    #[must_use]
    pub fn new(rows: Vec<PickRow>) -> Self {
        Self {
            rows,
            selected: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn rows(&self) -> &[PickRow] {
        &self.rows
    }

    /// Picks the row `name` if it can be picked, or unpicks it. Returns whether the picks changed.
    pub fn toggle(&mut self, name: &str) -> bool {
        let pickable = self
            .rows
            .iter()
            .any(|row| row.name == name && row.pickable());
        if !pickable {
            return false;
        }
        if !self.selected.remove(name) {
            self.selected.insert(name.to_owned());
        }
        true
    }

    #[must_use]
    pub fn is_selected(&self, name: &str) -> bool {
        self.selected.contains(name)
    }

    /// The names picked, in the order of the list.
    #[must_use]
    pub fn selected_names(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.pickable() && self.selected.contains(&row.name))
            .map(|row| row.name.clone())
            .collect()
    }

    /// Run is offered when at least one review is picked.
    #[must_use]
    pub fn can_run(&self) -> bool {
        !self.selected_names().is_empty()
    }
}

/// The Run button's words for `count` picked reviews.
#[must_use]
pub fn run_label(count: usize) -> String {
    match count {
        0 | 1 => "Run".to_owned(),
        n => format!("Run {n} reviews"),
    }
}

/// The dialog's state in the window: the pure picker and the focus that takes the keys.
pub(crate) struct RunDialog {
    pub(crate) focus: FocusHandle,
    pub(crate) picker: RunPicker,
}

impl StatusView {
    /// Opens the picker over the reviews the workflow listing has, and reads the listing again.
    pub(crate) fn open_review_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = cx.focus_handle();
        focus.focus(window);
        let rows = pick_rows(self.model.workflows.listed.as_deref().unwrap_or_default());
        self.review_run = Some(RunDialog {
            focus,
            picker: RunPicker::new(rows),
        });
        self.engine.dispatch(
            gm_schema::Actor::Human,
            gm_schema::CommandKind::ListWorkflows,
        );
        cx.notify();
    }

    pub(crate) fn close_review_run(&mut self, cx: &mut Context<Self>) {
        self.review_run = None;
        cx.notify();
    }

    pub(crate) fn toggle_review_run(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = self.review_run.as_mut() {
            dialog.picker.toggle(name);
        }
        cx.notify();
    }

    /// Run: each picked review goes through the registry's Run Workflow, one after another.
    pub(crate) fn submit_review_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names = match self.review_run.as_ref() {
            Some(dialog) if dialog.picker.can_run() => dialog.picker.selected_names(),
            _ => return,
        };
        self.review_run = None;
        for name in names {
            self.run_action(Action::RunWorkflow, &Args::Workflow(name), window, cx);
        }
        cx.notify();
    }

    pub(crate) fn review_run_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let Some(dialog) = self.review_run.as_ref() else {
            return div();
        };
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_review_run(cx)),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        div()
            .absolute()
            .inset_0()
            .child(outside)
            .child(review_run_panel(dialog, left, cx))
    }
}

fn review_run_panel(dialog: &RunDialog, left: f32, cx: &Context<StatusView>) -> Div {
    let picker = &dialog.picker;
    let body: AnyElement =
        if picker.rows().is_empty() {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_color(theme::text()).child(EMPTY_TITLE))
                .child(div().text_sm().text_color(theme::muted()).child(EMPTY_HINT))
                .into_any_element()
        } else {
            div()
                .id("review-run-rows")
                .flex()
                .flex_col()
                .max_h(px(LIST_MAX_HEIGHT))
                .overflow_y_scroll()
                .children(picker.rows().iter().enumerate().map(|(index, row)| {
                    review_run_row(index, row, picker.is_selected(&row.name), cx)
                }))
                .into_any_element()
        };
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
                "escape" => this.close_review_run(cx),
                "enter" => this.submit_review_run(window, cx),
                _ => {}
            }
        }))
        .child(
            div()
                .text_color(theme::text())
                .font_weight(FontWeight::SEMIBOLD)
                .child(TITLE),
        )
        .child(div().text_sm().text_color(theme::muted()).child(INTRO))
        .child(body)
        .child(review_run_buttons(picker, cx))
}

/// One review: its checkbox, its name, where it lives, its description, and why it cannot be
/// picked when it cannot.
fn review_run_row(
    index: usize,
    row: &PickRow,
    selected: bool,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let pickable = row.pickable();
    let name = row.name.clone();
    let box_icon = if selected {
        icon::filled(Icon::CheckSquare, icon::SMALL, theme::accent())
    } else {
        icon::view(Icon::Square, icon::SMALL, theme::muted())
    };
    let mut text = div().flex().flex_col().min_w(px(0.)).gap_0p5().child(
        div()
            .flex()
            .flex_row()
            .gap_2()
            .child(
                div()
                    .font_family(theme::code_font())
                    .font_weight(FontWeight::MEDIUM)
                    .child(row.name.clone()),
            )
            .child(div().text_xs().text_color(theme::muted()).child(row.place)),
    );
    if let Some(description) = &row.description {
        text = text.child(
            div()
                .text_sm()
                .text_color(theme::muted())
                .child(description.clone()),
        );
    }
    if let Some(problem) = &row.problem {
        text = text.child(
            div()
                .text_xs()
                .text_color(theme::warn_fg())
                .child(problem.clone()),
        );
    }
    div()
        .id(("review-run-row", index))
        .flex()
        .flex_row()
        .items_start()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_sm()
        .when(pickable, gpui::Styled::cursor_pointer)
        .when(!pickable, |row| row.opacity(0.6))
        .child(div().flex_none().pt_0p5().child(box_icon))
        .child(text)
        .when(pickable, |this| {
            this.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.toggle_review_run(&name, cx);
                }),
            )
        })
}

/// Cancel, and Run, which waits for at least one picked review.
fn review_run_buttons(picker: &RunPicker, cx: &Context<StatusView>) -> Div {
    let enabled = picker.can_run();
    let run = div()
        .id("review-run-confirm")
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::accent())
        .text_color(theme::on_accent())
        .font_weight(FontWeight::MEDIUM)
        .when(!enabled, |this| this.opacity(0.5))
        .when(enabled, gpui::Styled::cursor_pointer)
        .child(run_label(picker.selected_names().len()))
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.submit_review_run(window, cx);
        }));
    div()
        .flex()
        .flex_row()
        .justify_end()
        .items_center()
        .gap_2()
        .child(
            div()
                .id("review-run-cancel")
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
                    cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_review_run(cx)),
                ),
        )
        .child(run)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{WorkflowInfo, WorkflowRole};

    fn workflow(name: &str, role: WorkflowRole, enabled: bool) -> WorkflowInfo {
        WorkflowInfo {
            name: name.into(),
            path: format!("/w/{name}.lua"),
            trigger: "manual".into(),
            key: None,
            inputs: Vec::new(),
            hash: "h".into(),
            error: None,
            warnings: Vec::new(),
            source: String::new(),
            reviewed: true,
            auto_approve: false,
            enabled,
            title: None,
            location: WorkflowLocation::Repository,
            role,
            description: Some(format!("about {name}")),
        }
    }

    fn picker(workflows: &[WorkflowInfo]) -> RunPicker {
        RunPicker::new(pick_rows(workflows))
    }

    #[test]
    fn only_the_reviews_are_listed_in_the_order_given_with_their_place_and_description() {
        let rows = pick_rows(&[
            workflow("commit-msg", WorkflowRole::Workflow, true),
            workflow("security", WorkflowRole::Review, true),
            workflow("invariants", WorkflowRole::Review, true),
        ]);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["security", "invariants"]);
        assert_eq!(rows[0].place, "repository");
        assert_eq!(rows[0].description.as_deref(), Some("about security"));
        assert!(rows.iter().all(PickRow::pickable));
    }

    #[test]
    fn a_review_that_does_not_load_or_is_off_here_is_listed_with_its_reason_and_not_pickable() {
        let mut broken = workflow("broken", WorkflowRole::Review, true);
        broken.error = Some("line 3: unknown field".into());
        let off = workflow("off", WorkflowRole::Review, false);
        let rows = pick_rows(&[broken, off]);
        assert_eq!(rows[0].problem.as_deref(), Some("line 3: unknown field"));
        assert_eq!(rows[1].problem.as_deref(), Some(OFF_HERE));
        let mut picked = RunPicker::new(rows);
        assert!(!picked.toggle("broken"));
        assert!(!picked.toggle("off"));
        assert!(!picked.can_run());
    }

    #[test]
    fn picking_toggles_a_review_and_the_selection_comes_in_list_order() {
        let mut picked = picker(&[
            workflow("first", WorkflowRole::Review, true),
            workflow("second", WorkflowRole::Review, true),
        ]);
        assert!(!picked.can_run(), "nothing is picked at first");
        assert!(picked.toggle("second"));
        assert!(picked.toggle("first"));
        assert!(picked.is_selected("first"));
        assert_eq!(picked.selected_names(), ["first", "second"]);
        assert!(picked.toggle("first"), "a second pick unpicks it");
        assert_eq!(picked.selected_names(), ["second"]);
        assert!(!picked.toggle("missing"), "an unknown name changes nothing");
    }

    #[test]
    fn the_run_button_names_the_count_and_an_empty_list_says_how_to_add_one() {
        assert_eq!(run_label(0), "Run");
        assert_eq!(run_label(1), "Run");
        assert_eq!(run_label(3), "Run 3 reviews");
        assert!(EMPTY_HINT.contains("review-invariants"));
        assert!(EMPTY_HINT.contains(".git/gitmaster/workflows/"));
        assert!(pick_rows(&[workflow("commit", WorkflowRole::Workflow, true)]).is_empty());
    }

    #[test]
    fn the_places_are_named_in_words() {
        assert_eq!(place(WorkflowLocation::User), "user");
        assert_eq!(place(WorkflowLocation::Repository), "repository");
        assert_eq!(place(WorkflowLocation::BuiltIn), "built-in");
    }
}
