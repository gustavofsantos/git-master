//! The inputs dialog of a manual workflow that declares inputs. It holds one
//! single-line field per input and sends `RunWorkflow` with the typed values.
//! The form is pure and tested here: the fields, when Run may be pressed and
//! the command it builds. The window side is the `impl StatusView` at the
//! bottom, mirroring the new-branch dialog.

use std::collections::BTreeMap;

use gm_schema::{Actor, CommandKind, WorkflowInput};
use gpui::{
    App, Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, Stateful, Subscription,
    Window, div, prelude::*, px,
};

use crate::FocusNext;
use crate::text_area::{Newline, TextArea, TextAreaEvent};
use crate::theme;
use crate::view::StatusView;

const WIDTH: f32 = 520.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;

/// One input of the form: the name the workflow knows it by, the label the
/// dialog shows, and the text typed so far. Untrimmed, as typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InputField {
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) value: String,
}

/// The form of one run: the workflow and its inputs, in the order it declares them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InputsForm {
    pub(crate) workflow: String,
    pub(crate) fields: Vec<InputField>,
}

impl InputsForm {
    /// An empty form for `workflow`. Each field is labelled with its prompt, or
    /// with its name when the prompt is missing or blank.
    #[must_use]
    pub(crate) fn new(workflow: &str, inputs: &[WorkflowInput]) -> Self {
        Self {
            workflow: workflow.to_owned(),
            fields: inputs
                .iter()
                .map(|input| InputField {
                    name: input.name.clone(),
                    label: input
                        .prompt
                        .as_deref()
                        .map(str::trim)
                        .filter(|prompt| !prompt.is_empty())
                        .unwrap_or(&input.name)
                        .to_owned(),
                    value: String::new(),
                })
                .collect(),
        }
    }

    pub(crate) fn set_value(&mut self, index: usize, value: String) {
        if let Some(field) = self.fields.get_mut(index) {
            field.value = value;
        }
    }

    /// Run may be pressed when every field has text that is not only spaces.
    #[must_use]
    pub(crate) fn can_run(&self) -> bool {
        self.fields
            .iter()
            .all(|field| !field.value.trim().is_empty())
    }

    /// The command the form sends: the values trimmed, keyed by input name.
    /// `None` while a field is still empty.
    #[must_use]
    pub(crate) fn command(&self) -> Option<CommandKind> {
        if !self.can_run() {
            return None;
        }
        let inputs: BTreeMap<String, String> = self
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.value.trim().to_owned()))
            .collect();
        Some(CommandKind::RunWorkflow {
            name: self.workflow.clone(),
            inputs,
        })
    }
}

/// The dialog while it is open: the form's shape and one field per input.
pub(crate) struct WorkflowInputsDialog {
    /// The form with no values; each snapshot copies it and sets the texts.
    template: InputsForm,
    areas: Vec<Entity<TextArea>>,
    _subscriptions: Vec<Subscription>,
}

impl WorkflowInputsDialog {
    /// The form as the fields read now.
    pub(crate) fn form(&self, cx: &App) -> InputsForm {
        let mut form = self.template.clone();
        for (index, area) in self.areas.iter().enumerate() {
            form.set_value(index, area.read(cx).text().to_owned());
        }
        form
    }
}

impl StatusView {
    /// Opens the dialog for a workflow that declares `inputs`, with the first field focused.
    pub(crate) fn open_workflow_inputs_dialog(
        &mut self,
        workflow: &str,
        inputs: &[WorkflowInput],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let template = InputsForm::new(workflow, inputs);
        let mut areas = Vec::new();
        let mut subscriptions = Vec::new();
        for field in &template.fields {
            let placeholder = field.label.clone();
            let area = cx.new(|cx| TextArea::new(&placeholder, cx).with_lines(1, 1));
            subscriptions.push(cx.observe(&area, |_, _, cx| cx.notify()));
            subscriptions.push(cx.subscribe_in(
                &area,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    if *event == TextAreaEvent::Cancel {
                        this.close_workflow_inputs_dialog(window, cx);
                    }
                },
            ));
            areas.push(area);
        }
        if let Some(first) = areas.first() {
            first.update(cx, |area, cx| area.focus(window, cx));
        }
        self.workflow_inputs_dialog = Some(WorkflowInputsDialog {
            template,
            areas,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    pub(crate) fn close_workflow_inputs_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workflow_inputs_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// Enter and the Run button: sends the run with the typed inputs and closes
    /// the dialog. Nothing is sent while a field is empty.
    pub(crate) fn submit_workflow_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.workflow_inputs_dialog.as_ref() else {
            return;
        };
        let Some(kind) = dialog.form(cx).command() else {
            return;
        };
        self.engine.dispatch(Actor::Human, kind);
        self.close_workflow_inputs_dialog(window, cx);
    }

    fn next_workflow_input_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.workflow_inputs_dialog.as_ref() else {
            return;
        };
        if dialog.areas.is_empty() {
            return;
        }
        let current = dialog
            .areas
            .iter()
            .position(|area| area.read(cx).is_focused(window))
            .unwrap_or(dialog.areas.len() - 1);
        let next = (current + 1) % dialog.areas.len();
        dialog.areas[next].update(cx, |area, cx| area.focus(window, cx));
    }

    /// The dialog's layer, or nothing while the dialog is closed.
    pub(crate) fn workflow_inputs_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.workflow_inputs_dialog.as_ref()?;
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_workflow_inputs_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        let panel = Self::workflow_inputs_panel(dialog, left, cx);
        let view = cx.entity().downgrade();
        let tab = view.clone();
        let enter = view;
        Some(
            div().absolute().inset_0().child(outside).child(
                panel
                    .capture_action(move |_: &FocusNext, window, cx| {
                        let _ =
                            tab.update(cx, |this, cx| this.next_workflow_input_field(window, cx));
                        cx.stop_propagation();
                    })
                    .capture_action(move |_: &Newline, window, cx| {
                        let _ =
                            enter.update(cx, |this, cx| this.submit_workflow_inputs(window, cx));
                        cx.stop_propagation();
                    }),
            ),
        )
    }

    fn workflow_inputs_panel(dialog: &WorkflowInputsDialog, left: f32, cx: &Context<Self>) -> Div {
        let form = dialog.form(cx);
        let allowed = form.can_run();
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
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Run {}", form.workflow)),
            )
            .children(
                form.fields
                    .iter()
                    .zip(&dialog.areas)
                    .map(|(field, area)| input_row(&field.label, area.clone())),
            )
            .child(buttons(allowed, cx))
    }
}

/// One labelled single-line field of the form.
fn input_row(label: &str, field: Entity<TextArea>) -> Div {
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
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(theme::hairline())
                .child(field),
        )
}

/// Cancel and Run. Run waits until every field has text.
fn buttons(allowed: bool, cx: &Context<StatusView>) -> Div {
    let run: Stateful<Div> = div()
        .id("workflow-inputs-run")
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::accent())
        .text_color(theme::on_accent())
        .font_weight(FontWeight::MEDIUM)
        .when(!allowed, |this| this.opacity(0.5))
        .when(allowed, gpui::Styled::cursor_pointer)
        .child("Run")
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.submit_workflow_inputs(window, cx);
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
                .id("workflow-inputs-cancel")
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
                        this.close_workflow_inputs_dialog(window, cx);
                    }),
                ),
        )
        .child(run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, prompt: Option<&str>) -> WorkflowInput {
        WorkflowInput {
            name: name.into(),
            prompt: prompt.map(str::to_owned),
        }
    }

    fn filled(inputs: &[WorkflowInput], values: &[&str]) -> InputsForm {
        let mut form = InputsForm::new("branch-from-ticket", inputs);
        for (index, value) in values.iter().enumerate() {
            form.set_value(index, (*value).to_owned());
        }
        form
    }

    #[test]
    fn each_field_is_labelled_with_its_prompt_or_its_name() {
        let form = InputsForm::new(
            "w",
            &[
                input("ticket", Some("Ticket id")),
                input("title", None),
                input("scope", Some("   ")),
            ],
        );
        let labels: Vec<&str> = form.fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["Ticket id", "title", "scope"]);
        assert!(form.fields.iter().all(|f| f.value.is_empty()));
        assert_eq!(form.workflow, "w");
    }

    #[test]
    fn run_waits_until_every_field_has_text() {
        let inputs = [input("a", None), input("b", None)];
        assert!(!InputsForm::new("w", &inputs).can_run());
        assert!(!filled(&inputs, &["x", ""]).can_run());
        assert!(!filled(&inputs, &["x", "   "]).can_run());
        assert!(filled(&inputs, &["x", "y"]).can_run());
    }

    #[test]
    fn a_workflow_with_no_inputs_can_run_at_once() {
        assert!(InputsForm::new("w", &[]).can_run());
    }

    #[test]
    fn the_command_carries_the_typed_values_by_input_name() {
        let inputs = [input("ticket", Some("Ticket")), input("title", None)];
        let form = filled(&inputs, &["  ENG-42 ", "Fix login"]);
        let expected = CommandKind::RunWorkflow {
            name: "branch-from-ticket".into(),
            inputs: BTreeMap::from([
                ("ticket".to_owned(), "ENG-42".to_owned()),
                ("title".to_owned(), "Fix login".to_owned()),
            ]),
        };
        assert_eq!(form.command(), Some(expected));
    }

    #[test]
    fn no_command_while_a_field_is_empty() {
        let inputs = [input("a", None), input("b", None)];
        assert_eq!(filled(&inputs, &["x", ""]).command(), None);
    }

    #[test]
    fn setting_a_value_past_the_last_field_changes_nothing() {
        let inputs = [input("a", None)];
        let mut form = InputsForm::new("w", &inputs);
        form.set_value(3, "x".into());
        assert_eq!(form, InputsForm::new("w", &inputs));
    }
}
