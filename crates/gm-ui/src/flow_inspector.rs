//! The Flow's inspector (design/workflow-scripts.md, the Flow bullet): the workflow's name,
//! description, trigger, match and inputs, and the selected state's action and
//! `with` fields, edited as typed values. The rows, the lists the pickers show
//! and the edit a commit sends are pure and tested here. The window side is the
//! `impl StatusView` at the bottom: one single-line or multi-line text field per
//! typed value (commit on Enter or blur, Escape reverts), and the popovers for
//! the trigger, the action and the choice fields. Every change goes through the
//! page's `FlowEdit`, so the TOML text stays the one source.

use std::collections::HashMap;

use gm_core::workflow::catalog::{self, ActionSpec, FieldKind, FieldSpec, Level};
use gm_core::workflow::dry_run;
use gm_core::workflow::edit::FieldValue;
use gm_core::workflow::model::{Definition, StateKind};
use gm_core::workflow::validate::field_problem;
use gpui::{
    AnyElement, Context, Div, Entity, Focusable, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, Stateful, Subscription, Window, div, prelude::*, px,
};
use serde_json::Value;

use crate::context_menu::clamp_origin;
use crate::shell_workflows::diagnostic_line;
use crate::text_area::{Newline, TextArea, TextAreaEvent};
use crate::theme;
use crate::view::{Screen, StatusView};
use crate::workflows_page::{FlowEdit, Tab, flow_rows};

/// The width of a picker popover, the height of its rows and the most it shows
/// before it scrolls.
const PICKER_WIDTH: f32 = 260.;
const ROW_HEIGHT: f32 = 24.;
const PICKER_MAX_HEIGHT: f32 = 480.;
/// Where a picker opened from the keyboard hangs: this far from the right edge, and this far down.
const KEYBOARD_PICKER_INSET: f32 = 300.;
const KEYBOARD_PICKER_TOP: f32 = 140.;

/// The text field of one value of the inspector, keyed by what it edits.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Slot {
    Name,
    Description,
    Match,
    /// The prompt of the input with this name.
    Prompt(String),
    /// The name typed to add an input.
    NewInput,
    /// The `with` field of the selected state. Only the text kinds have one.
    Field(String),
}

/// What a popover of the inspector chooses between.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickerKind {
    /// The actions of the catalog, grouped by domain.
    Action,
    /// The triggers the catalog knows.
    Trigger,
    /// The values of a choice field of the selected state.
    Choice(String),
}

/// One `with` field of an action state, with its value as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FieldRow {
    pub(crate) spec: FieldSpec,
    /// A list is one item per line; a bool is `true` or `false`; absent is empty.
    pub(crate) value: String,
}

/// The rows of an action state: its action, the level and domain of the
/// action, its catalog fields, and the `with` keys the action does not know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StateRows {
    pub(crate) action: String,
    /// `None` when the catalog does not know the action.
    pub(crate) level: Option<&'static str>,
    pub(crate) domain: String,
    pub(crate) fields: Vec<FieldRow>,
    /// Keys of `with` that are not fields of the action, in key order. `set`
    /// takes any key, so it has none.
    pub(crate) unknown: Vec<String>,
}

/// One action in a picker. `reason` says why it cannot be picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActionEntry {
    pub(crate) name: &'static str,
    pub(crate) level: &'static str,
    pub(crate) reason: Option<&'static str>,
}

/// The actions of one domain, in catalog order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActionGroup {
    pub(crate) domain: &'static str,
    pub(crate) entries: Vec<ActionEntry>,
}

/// The rows of the action state `state`, or `None` when the state is not an
/// action state (a wait or a `for_each`).
#[must_use]
pub(crate) fn state_rows(definition: &Definition, state: &str) -> Option<StateRows> {
    let found = definition
        .states
        .iter()
        .find(|candidate| candidate.id == state)?;
    let StateKind::Action { action, with, .. } = &found.kind else {
        return None;
    };
    let spec = catalog::action(action);
    let known: &[FieldSpec] = spec.map_or(&[][..], |spec| spec.fields);
    let fields = known
        .iter()
        .map(|field| FieldRow {
            spec: *field,
            value: value_text(field.kind, with.get(field.name)),
        })
        .collect();
    let unknown = with.as_object().map_or_else(Vec::new, |map| {
        map.keys()
            .filter(|key| {
                action.as_str() != "set" && !known.iter().any(|field| field.name == key.as_str())
            })
            .cloned()
            .collect()
    });
    Some(StateRows {
        action: action.clone(),
        level: spec.map(|spec| level_label(spec.level)),
        domain: catalog::domain(action).to_owned(),
        fields,
        unknown,
    })
}

/// The value of a `with` key as the inspector shows it.
fn value_text(kind: FieldKind, value: Option<&Value>) -> String {
    match (kind, value) {
        (_, None) => String::new(),
        (FieldKind::Bool, Some(flag)) => flag.as_bool().unwrap_or(false).to_string(),
        (FieldKind::TextList, Some(Value::Array(items))) => {
            items.iter().map(scalar_text).collect::<Vec<_>>().join("\n")
        }
        (_, Some(value)) => scalar_text(value),
    }
}

fn scalar_text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

/// The level badge text of an action (design/workflow-scripts.md): `approve` is the pause.
#[must_use]
pub(crate) fn level_label(level: Level) -> &'static str {
    match level {
        Level::Read => "read",
        Level::Propose => "propose",
        Level::Apply => "apply",
        Level::Pause => "approve",
    }
}

/// The actions the catalog knows, grouped by domain in first-seen order. An
/// action the driver cannot run yet is disabled with `not available yet`; a
/// phase-limited action (`set`) is disabled outside its trigger phase.
#[must_use]
pub(crate) fn action_groups(trigger_on: &str) -> Vec<ActionGroup> {
    let mut groups: Vec<ActionGroup> = Vec::new();
    for spec in catalog::actions() {
        let domain = catalog::domain(spec.name);
        let entry = ActionEntry {
            name: spec.name,
            level: level_label(spec.level),
            reason: unavailable_reason(spec, trigger_on),
        };
        match groups.iter_mut().find(|group| group.domain == domain) {
            Some(group) => group.entries.push(entry),
            None => groups.push(ActionGroup {
                domain,
                entries: vec![entry],
            }),
        }
    }
    groups
}

fn unavailable_reason(spec: &ActionSpec, trigger_on: &str) -> Option<&'static str> {
    if !dry_run::is_implemented(spec.name) {
        return Some("not available yet");
    }
    match spec.phase {
        Some(phase) if trigger_on.split('.').next() != Some(phase) => {
            Some("only in a transform trigger")
        }
        _ => None,
    }
}

/// Whether the field takes typed text (a single or a multi-line field).
fn is_typed(kind: FieldKind) -> bool {
    matches!(
        kind,
        FieldKind::Text | FieldKind::Duration | FieldKind::TextList
    )
}

/// The items of a list field: one per non-empty line.
#[must_use]
pub(crate) fn list_items(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Why the text typed into `field` does not fit it, or `None`. It is the check
/// the edit runs (`validate::field_problem`), so the field shows what the
/// commit would refuse. An empty required field says so; an empty optional one fits.
#[must_use]
pub(crate) fn typed_problem(field: &FieldSpec, text: &str) -> Option<String> {
    let missing = || field.required.then(|| "Required.".to_owned());
    match field.kind {
        FieldKind::Any | FieldKind::Bool | FieldKind::Choice(_) => None,
        FieldKind::TextList => {
            let items = list_items(text);
            if items.is_empty() {
                return missing();
            }
            field_problem(field, &Value::from(items))
        }
        _ if text.is_empty() => missing(),
        _ => field_problem(field, &Value::from(text)),
    }
}

/// The value of `slot` in the workflow as the file has it now.
#[must_use]
pub(crate) fn slot_value(definition: &Definition, selected: Option<&str>, slot: &Slot) -> String {
    match slot {
        Slot::Name => definition.name.clone(),
        Slot::Description => definition.description.clone().unwrap_or_default(),
        Slot::Match => definition.trigger.matcher.clone().unwrap_or_default(),
        Slot::Prompt(name) => definition
            .inputs
            .iter()
            .find(|input| input.name == *name)
            .and_then(|input| input.prompt.clone())
            .unwrap_or_default(),
        Slot::NewInput => String::new(),
        Slot::Field(field) => selected
            .and_then(|id| state_rows(definition, id))
            .and_then(|rows| rows.fields.into_iter().find(|row| row.spec.name == field))
            .map(|row| row.value)
            .unwrap_or_default(),
    }
}

/// The slots the inspector shows for this workflow: the workflow's own, the
/// inputs' prompts, and the typed fields of the selected action state.
#[must_use]
pub(crate) fn wanted_slots(definition: &Definition, selected: Option<&str>) -> Vec<Slot> {
    let mut slots = vec![Slot::Name, Slot::Description, Slot::Match, Slot::NewInput];
    slots.extend(
        definition
            .inputs
            .iter()
            .map(|input| Slot::Prompt(input.name.clone())),
    );
    if let Some(rows) = selected.and_then(|id| state_rows(definition, id)) {
        slots.extend(
            rows.fields
                .iter()
                .filter(|row| is_typed(row.spec.kind))
                .map(|row| Slot::Field(row.spec.name.to_owned())),
        );
    }
    slots
}

/// The edit a commit of `text` in `slot` sends. `None` when the text is not
/// sent: a new input name that is blank, or a field whose text does not fit
/// (the field already shows why).
#[must_use]
pub(crate) fn slot_edit(
    definition: &Definition,
    selected: Option<&str>,
    slot: &Slot,
    text: &str,
) -> Option<FlowEdit> {
    Some(match slot {
        // `set_meta` removes a description it is not given, so the name's edit carries it.
        Slot::Name => FlowEdit::SetMeta {
            name: Some(text.to_owned()),
            description: definition.description.clone(),
        },
        Slot::Description => FlowEdit::SetMeta {
            name: Some(definition.name.clone()),
            description: Some(text.to_owned()),
        },
        Slot::Match => FlowEdit::SetTrigger {
            on: definition.trigger.on.clone(),
            matcher: Some(text.to_owned()),
        },
        Slot::Prompt(name) => FlowEdit::SetInput {
            name: name.clone(),
            prompt: Some(text.to_owned()),
        },
        Slot::NewInput => {
            let name = text.trim();
            if name.is_empty() {
                return None;
            }
            FlowEdit::SetInput {
                name: name.to_owned(),
                prompt: None,
            }
        }
        Slot::Field(field) => {
            let rows = state_rows(definition, selected?)?;
            let row = rows
                .fields
                .iter()
                .find(|row| row.spec.name == field.as_str())?;
            if !is_typed(row.spec.kind) || typed_problem(&row.spec, text).is_some() {
                return None;
            }
            let value = match row.spec.kind {
                FieldKind::TextList => {
                    let items = list_items(text);
                    if items.is_empty() && !row.spec.required {
                        return Some(FlowEdit::RemoveField(field.clone()));
                    }
                    FieldValue::TextList(items)
                }
                _ => FieldValue::Text(text.to_owned()),
            };
            FlowEdit::SetField {
                field: field.clone(),
                value,
            }
        }
    })
}

/// The live parts of the inspector, kept by the window: one text field per
/// slot shown, and the popover open. The file is the source of every value.
#[derive(Default)]
pub(crate) struct FlowInspector {
    fields: HashMap<Slot, FlowField>,
    picker: Option<OpenPicker>,
}

/// A text field and the value it last showed. A field the user is typing in
/// keeps what was typed until it is committed or reverted.
struct FlowField {
    area: Entity<TextArea>,
    shown: String,
    _subscriptions: Vec<Subscription>,
}

struct OpenPicker {
    kind: PickerKind,
    /// Where the popover opens, in window pixels.
    at: (f32, f32),
}

fn level_badge(label: &str) -> Div {
    div()
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(theme::hairline())
        .text_xs()
        .text_color(theme::muted())
        .child(label.to_owned())
}

fn label(text: &str) -> Div {
    div()
        .text_xs()
        .text_color(theme::muted())
        .child(text.to_owned())
}

fn title(text: &str) -> Div {
    div()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::text())
        .child(text.to_owned())
}

fn note(text: &str) -> Div {
    div()
        .text_sm()
        .text_color(theme::muted())
        .child(text.to_owned())
}

fn point_of(event: &MouseDownEvent) -> (f32, f32) {
    (f32::from(event.position.x), f32::from(event.position.y))
}

fn placeholder_of(slot: &Slot) -> &'static str {
    match slot {
        Slot::Name => "Workflow name",
        Slot::Description => "What the workflow does",
        Slot::Match => "Optional, such as branch:feat/*",
        Slot::Prompt(_) => "Prompt shown when the run asks",
        Slot::NewInput => "New input name, then Enter",
        Slot::Field(_) => "Value",
    }
}

/// Creates the text field of `slot` showing `value`, and the subscriptions that
/// keep it in step: a change redraws, Escape reverts and leaving commits.
fn new_field(
    slot: &Slot,
    value: &str,
    multi: bool,
    window: &mut Window,
    cx: &mut Context<StatusView>,
) -> FlowField {
    let placeholder = placeholder_of(slot);
    let (min, max) = if multi { (2, 8) } else { (1, 1) };
    let area = cx.new(|cx| TextArea::new(placeholder, cx).with_lines(min, max));
    area.update(cx, |area, cx| area.set_text(value, cx));
    let observe = cx.observe(&area, |_, _, cx| cx.notify());
    let on_cancel = slot.clone();
    let cancel = cx.subscribe(&area, move |this, _, event: &TextAreaEvent, cx| {
        if *event == TextAreaEvent::Cancel {
            this.revert_flow_field(&on_cancel, cx);
        }
    });
    let handle = area.read(cx).focus_handle(cx);
    let on_blur = slot.clone();
    let blur = cx.on_blur(&handle, window, move |this, _, cx| {
        // A new input is added on Enter only; a stray blur must not add one.
        if on_blur != Slot::NewInput {
            this.commit_flow_field(&on_blur, cx);
        }
    });
    FlowField {
        area,
        shown: value.to_owned(),
        _subscriptions: vec![observe, cancel, blur],
    }
}

/// The row of one action of a picker. A row that cannot be picked is dimmed
/// and shows its reason.
fn option_row(
    index: usize,
    text: &str,
    detail: Option<&str>,
    edit: Option<FlowEdit>,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let row = div()
        .id(("inspector-option", index))
        .h(px(ROW_HEIGHT))
        .px_3()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .font_family(theme::code_font())
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(text.to_owned()),
        )
        .children(detail.map(|detail| {
            div()
                .text_xs()
                .text_color(theme::muted())
                .child(detail.to_owned())
        }));
    match edit {
        Some(edit) => row
            .cursor_pointer()
            .hover(|style| style.bg(theme::selected_row()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.pick_inspector_edit(edit.clone(), window, cx);
                }),
            ),
        None => row.opacity(0.5),
    }
}

fn heading(domain: &str) -> Div {
    div()
        .px_3()
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .text_xs()
        .text_color(theme::muted())
        .child(domain.to_owned())
}

/// The rows of the action picker: each domain, then its actions.
fn action_rows(trigger_on: &str, cx: &Context<StatusView>) -> Vec<AnyElement> {
    let mut rows = Vec::new();
    for group in action_groups(trigger_on) {
        rows.push(heading(group.domain).into_any_element());
        for entry in group.entries {
            let index = rows.len();
            let edit = entry
                .reason
                .is_none()
                .then(|| FlowEdit::SetAction(entry.name.to_owned()));
            let detail = entry.reason.unwrap_or(entry.level);
            rows.push(option_row(index, entry.name, Some(detail), edit, cx).into_any_element());
        }
    }
    rows
}

/// The rows of a choice field's picker: the values the catalog allows.
fn choice_rows(
    definition: &Definition,
    selected: Option<&str>,
    field: &str,
    cx: &Context<StatusView>,
) -> Vec<AnyElement> {
    let found = selected
        .and_then(|id| state_rows(definition, id))
        .and_then(|rows| rows.fields.into_iter().find(|row| row.spec.name == field));
    let values: &[&str] = match found {
        Some(FieldRow {
            spec:
                FieldSpec {
                    kind: FieldKind::Choice(values),
                    ..
                },
            ..
        }) => values,
        _ => &[],
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let edit = FlowEdit::SetField {
                field: field.to_owned(),
                value: FieldValue::Text((*value).to_owned()),
            };
            option_row(index, value, None, Some(edit), cx).into_any_element()
        })
        .collect()
}

impl StatusView {
    /// Keeps one text field per slot the inspector shows. It creates the missing
    /// ones, drops the ones no longer shown, and takes the file's value into a
    /// field whose file value changed and which the user is not typing in.
    pub(crate) fn sync_flow_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wanted = self.inspector_values();
        let inspector = &mut self.flow_inspector;
        inspector
            .fields
            .retain(|slot, _| wanted.iter().any(|(known, _, _)| known == slot));
        for (slot, value, multi) in wanted {
            if let Some(field) = inspector.fields.get_mut(&slot) {
                if field.shown != value && !field.area.read(cx).is_focused(window) {
                    field.shown.clone_from(&value);
                    field.area.update(cx, |area, cx| area.set_text(&value, cx));
                }
            } else {
                let field = new_field(&slot, &value, multi, window, cx);
                inspector.fields.insert(slot, field);
            }
        }
    }

    /// The slots shown with their file values, and whether each is a list
    /// (multi-line). Empty outside the Flow tab.
    fn inspector_values(&self) -> Vec<(Slot, String, bool)> {
        let page = &self.model.workflows_page;
        let shown = matches!(self.screen, Screen::Workflows) && page.tab() == Tab::Flow;
        let Some(definition) = page.definition().filter(|_| shown) else {
            return Vec::new();
        };
        let selected = page.selected();
        let rows = selected.and_then(|id| state_rows(definition, id));
        wanted_slots(definition, selected)
            .into_iter()
            .map(|slot| {
                let value = slot_value(definition, selected, &slot);
                let multi = match &slot {
                    Slot::Field(name) => rows.as_ref().is_some_and(|rows| {
                        rows.fields.iter().any(|row| {
                            row.spec.name == name && matches!(row.spec.kind, FieldKind::TextList)
                        })
                    }),
                    _ => false,
                };
                (slot, value, multi)
            })
            .collect()
    }

    fn typed_text(&self, slot: &Slot, cx: &Context<Self>) -> String {
        self.flow_inspector
            .fields
            .get(slot)
            .map_or_else(String::new, |field| field.area.read(cx).text().to_owned())
    }

    /// Commits the text of `slot`: its edit goes through the page, and the field
    /// takes the normalised value when the edit is accepted. A refused edit keeps
    /// the text and the page's message shows the reason.
    fn commit_flow_field(&mut self, slot: &Slot, cx: &mut Context<Self>) {
        let text = self.typed_text(slot, cx);
        let page = &self.model.workflows_page;
        let Some(definition) = page.definition() else {
            return;
        };
        let selected = page.selected();
        if text == slot_value(definition, selected, slot) {
            return;
        }
        let Some(edit) = slot_edit(definition, selected, slot, &text) else {
            return;
        };
        self.apply_flow_edit(edit, cx);
        if self.model.workflows_page.flow_error().is_none() {
            self.accept_flow_field(slot, cx);
        }
    }

    /// Puts the file's value of `slot` in its field, and remembers it as shown.
    fn accept_flow_field(&mut self, slot: &Slot, cx: &mut Context<Self>) {
        let page = &self.model.workflows_page;
        let value = page.definition().map_or_else(String::new, |definition| {
            slot_value(definition, page.selected(), slot)
        });
        if let Some(field) = self.flow_inspector.fields.get_mut(slot) {
            field.shown.clone_from(&value);
            field.area.update(cx, |area, cx| {
                if area.text() != value {
                    area.set_text(&value, cx);
                }
            });
        }
    }

    /// Escape: the field shows the value the file has again.
    fn revert_flow_field(&mut self, slot: &Slot, cx: &mut Context<Self>) {
        if let Some(field) = self.flow_inspector.fields.get(slot) {
            let shown = field.shown.clone();
            field.area.update(cx, |area, cx| area.set_text(&shown, cx));
        }
    }

    /// Commits the text field the keyboard is in, so typed text is not lost when
    /// the selection or the picker moves on from it. The new-input field is left
    /// alone: it is added on Enter only.
    pub(crate) fn commit_focused_flow_field(&mut self, window: &Window, cx: &mut Context<Self>) {
        let focused = self
            .flow_inspector
            .fields
            .iter()
            .find(|(slot, field)| {
                *slot != &Slot::NewInput && field.area.read(cx).is_focused(window)
            })
            .map(|(slot, _)| slot.clone());
        if let Some(slot) = focused {
            self.commit_flow_field(&slot, cx);
        }
    }

    /// Commits every text field whose text differs from the file. Run before
    /// Save, Dry Run and a tab change: a click on a label or a button does not
    /// move the keyboard focus, so no blur would commit the typed text.
    pub(crate) fn commit_pending_flow_fields(&mut self, cx: &mut Context<Self>) {
        let mut slots: Vec<Slot> = self
            .flow_inspector
            .fields
            .keys()
            .filter(|slot| **slot != Slot::NewInput)
            .cloned()
            .collect();
        // Sorted, so two new `with` keys land in the file in a fixed order.
        slots.sort();
        for slot in slots {
            self.commit_flow_field(&slot, cx);
        }
    }

    /// A picker's choice: the typed text in focus is committed first, then the
    /// picker closes and the edit is applied.
    fn pick_inspector_edit(&mut self, edit: FlowEdit, window: &Window, cx: &mut Context<Self>) {
        self.commit_focused_flow_field(window, cx);
        self.flow_inspector.picker = None;
        self.apply_flow_edit(edit, cx);
    }

    /// Opens a picker under the pointer.
    fn open_inspector_picker(&mut self, kind: PickerKind, at: (f32, f32), cx: &mut Context<Self>) {
        self.flow_inspector.picker = Some(OpenPicker { kind, at });
        cx.notify();
    }

    /// Opens a picker from the keyboard, near the inspector's column.
    pub(crate) fn open_inspector_picker_from_keyboard(
        &mut self,
        kind: PickerKind,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let width = f32::from(window.viewport_size().width);
        let at = (width - KEYBOARD_PICKER_INSET, KEYBOARD_PICKER_TOP);
        self.open_inspector_picker(kind, at, cx);
    }

    /// Moves the keyboard to the field that adds an input.
    pub(crate) fn focus_new_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.flow_inspector.fields.get(&Slot::NewInput) {
            field.area.update(cx, |area, cx| area.focus(window, cx));
        }
    }

    fn close_inspector_picker(&mut self, cx: &mut Context<Self>) {
        self.flow_inspector.picker = None;
        cx.notify();
    }

    /// The inspector's text field of `slot`: a box, then the problem under it.
    /// A single-line field commits on Enter; Enter in a list adds a line.
    fn text_box(
        &self,
        slot: &Slot,
        multi: bool,
        problem: Option<String>,
        cx: &Context<Self>,
    ) -> Div {
        let Some(field) = self.flow_inspector.fields.get(slot) else {
            return div();
        };
        let commit = slot.clone();
        let boxed = div()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme::hairline())
            .child(field.area.clone());
        let boxed = if multi {
            boxed
        } else {
            boxed.capture_action(cx.listener(move |this, _: &Newline, _, cx| {
                this.commit_flow_field(&commit, cx);
                cx.stop_propagation();
            }))
        };
        div().flex().flex_col().gap_1().child(boxed).children(
            problem.map(|problem| div().text_xs().text_color(theme::error()).child(problem)),
        )
    }

    fn text_row(
        &self,
        text: &str,
        slot: &Slot,
        multi: bool,
        problem: Option<String>,
        cx: &Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(label(text))
            .child(self.text_box(slot, multi, problem, cx))
    }

    /// The problem of a typed field, from the text it holds now.
    fn field_problem_now(
        &self,
        slot: &Slot,
        spec: &FieldSpec,
        cx: &Context<Self>,
    ) -> Option<String> {
        typed_problem(spec, &self.typed_text(slot, cx))
    }

    /// The inspector, placed above the card list in the Flow's right column.
    pub(crate) fn inspector_panel(&self, cx: &Context<Self>) -> Div {
        let page = &self.model.workflows_page;
        let Some(definition) = page.definition() else {
            return div();
        };
        let selected = page.selected();
        let diagnostics: Vec<_> = flow_rows(definition, page.diagnostics())
            .0
            .into_iter()
            .find(|row| Some(row.id.as_str()) == selected)
            .map(|row| row.diagnostics)
            .unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .pb_4()
            .border_b_1()
            .border_color(theme::hairline())
            .child(self.workflow_section(definition, cx))
            .child(self.state_section(definition, selected, cx))
            .children(diagnostics.iter().map(diagnostic_line))
    }

    fn workflow_section(&self, definition: &Definition, cx: &Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(title("Workflow"))
            .child(self.text_row("Name", &Slot::Name, false, None, cx))
            .child(self.text_row("Description", &Slot::Description, false, None, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(label("Trigger"))
                    .child(
                        div()
                            .id("inspector-trigger")
                            .px_2()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .rounded_md()
                            .border_1()
                            .border_color(theme::button_border())
                            .cursor_pointer()
                            .font_family(theme::code_font())
                            .child(definition.trigger.on.clone())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.open_inspector_picker(
                                        PickerKind::Trigger,
                                        point_of(event),
                                        cx,
                                    );
                                }),
                            ),
                    ),
            )
            .child(self.text_row("Match (optional)", &Slot::Match, false, None, cx))
            .child(self.inputs_block(definition, cx))
    }

    fn inputs_block(&self, definition: &Definition, cx: &Context<Self>) -> Div {
        let mut block = div().flex().flex_col().gap_2().child(label("Inputs"));
        for (index, input) in definition.inputs.iter().enumerate() {
            let name = input.name.clone();
            let remove = name.clone();
            block = block.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(div().font_family(theme::code_font()).child(name.clone()))
                            .child(
                                div()
                                    .id(("inspector-remove-input", index))
                                    .px_2()
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .child("Remove")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                            this.pick_inspector_edit(
                                                FlowEdit::RemoveInput(remove.clone()),
                                                window,
                                                cx,
                                            );
                                        }),
                                    ),
                            ),
                    )
                    .child(self.text_box(&Slot::Prompt(name), false, None, cx)),
            );
        }
        block.child(self.text_row("Add input", &Slot::NewInput, false, None, cx))
    }

    fn state_section(
        &self,
        definition: &Definition,
        selected: Option<&str>,
        cx: &Context<Self>,
    ) -> Div {
        let section = div().flex().flex_col().gap_3().child(title("State"));
        let Some(id) = selected else {
            return section.child(note(
                "Select a state in the Flow to edit its action and fields.",
            ));
        };
        let Some(rows) = state_rows(definition, id) else {
            return section.child(note("This state waits or loops. Edit it in the TOML tab."));
        };
        let mut section = section.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(label("Action"))
                .child(
                    div()
                        .id("inspector-action")
                        .px_2()
                        .h(px(26.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme::button_border())
                        .cursor_pointer()
                        .child(
                            div()
                                .font_family(theme::code_font())
                                .child(rows.action.clone()),
                        )
                        .children(rows.level.map(level_badge))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                this.open_inspector_picker(PickerKind::Action, point_of(event), cx);
                            }),
                        ),
                ),
        );
        if rows.level.is_none() {
            section = section.child(note("The catalog does not know this action."));
        }
        if !rows.unknown.is_empty() {
            section = section.child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme::warn_bg())
                    .text_sm()
                    .child(format!(
                        "Not fields of {}: {}. Edit them in the TOML tab.",
                        rows.action,
                        rows.unknown.join(", ")
                    )),
            );
        }
        for (index, row) in rows.fields.iter().enumerate() {
            section = section.child(self.field_block(index, row, cx));
        }
        section
    }

    fn field_block(&self, index: usize, row: &FieldRow, cx: &Context<Self>) -> Div {
        let spec = row.spec;
        let mut head = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(div().font_family(theme::code_font()).child(spec.name));
        if spec.required {
            head = head.child(label("required"));
        }
        let control = match spec.kind {
            FieldKind::Text | FieldKind::Duration | FieldKind::TextList => {
                let slot = Slot::Field(spec.name.to_owned());
                let multi = matches!(spec.kind, FieldKind::TextList);
                let problem = self.field_problem_now(&slot, &spec, cx);
                self.text_box(&slot, multi, problem, cx)
            }
            FieldKind::Choice(_) => {
                div().child(Self::choice_button(index, spec.name, &row.value, cx))
            }
            FieldKind::Bool => div().child(check_row(index, spec.name, row.value == "true", cx)),
            FieldKind::Any => note("Edit `set` values in the TOML tab."),
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(head)
            .child(label(spec.help))
            .child(control)
    }

    fn choice_button(index: usize, field: &str, value: &str, cx: &Context<Self>) -> Stateful<Div> {
        let kind = PickerKind::Choice(field.to_owned());
        let shown = if value.is_empty() { "choose" } else { value };
        div()
            .id(("inspector-choice", index))
            .px_2()
            .h(px(26.))
            .flex()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme::button_border())
            .cursor_pointer()
            .font_family(theme::code_font())
            .child(shown.to_owned())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_inspector_picker(kind.clone(), point_of(event), cx);
                }),
            )
    }

    /// The popover of the open picker, or nothing while none is open.
    pub(crate) fn inspector_picker_layer(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let open = self.flow_inspector.picker.as_ref()?;
        let page = &self.model.workflows_page;
        let definition = page.definition()?;
        let selected = page.selected();
        let rows: Vec<AnyElement> = match &open.kind {
            PickerKind::Trigger => catalog::triggers()
                .iter()
                .enumerate()
                .map(|(index, on)| {
                    let edit = FlowEdit::SetTrigger {
                        on: on.clone(),
                        matcher: definition.trigger.matcher.clone(),
                    };
                    option_row(index, on, None, Some(edit), cx).into_any_element()
                })
                .collect(),
            PickerKind::Action => action_rows(&definition.trigger.on, cx),
            PickerKind::Choice(field) => choice_rows(definition, selected, field, cx),
        };
        let viewport = window.viewport_size();
        let height = (f32::from(u16::try_from(rows.len()).unwrap_or(u16::MAX)) * ROW_HEIGHT + 8.)
            .min(PICKER_MAX_HEIGHT);
        let (left, top) = clamp_origin(
            open.at,
            (PICKER_WIDTH, height),
            (f32::from(viewport.width), f32::from(viewport.height)),
        );
        let outside = div()
            .absolute()
            .size_full()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_inspector_picker(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_inspector_picker(cx)),
            );
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
                    .child(
                        div()
                            .id("inspector-picker")
                            .max_h(px(PICKER_MAX_HEIGHT))
                            .overflow_y_scroll()
                            .py_1()
                            .flex()
                            .flex_col()
                            .children(rows),
                    ),
            ),
        )
    }
}

/// A checkbox row for a bool field. Clicking it writes the other value.
fn check_row(index: usize, field: &str, checked: bool, cx: &Context<StatusView>) -> Stateful<Div> {
    let field = field.to_owned();
    div()
        .id(("inspector-check", index))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(
            div()
                .size(px(12.))
                .rounded_md()
                .border_1()
                .border_color(theme::hairline_strong())
                .when(checked, |box_| box_.bg(theme::accent())),
        )
        .child(
            div()
                .font_family(theme::code_font())
                .child(if checked { "true" } else { "false" }),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                let edit = FlowEdit::SetField {
                    field: field.clone(),
                    value: FieldValue::Bool(!checked),
                };
                this.pick_inspector_edit(edit, window, cx);
            }),
        )
}

#[cfg(test)]
mod tests {
    use gm_core::workflow::parse;

    use super::*;

    const SOURCE: &str = "\
schema = 1
name = \"flow\"
description = \"Demo\"

[inputs]
ticket = { type = \"string\", prompt = \"Ticket key\" }

[trigger]
on = \"manual\"

[[state]]
id = \"run\"
action = \"exec\"
with = { argv = [\"git\", \"status\"], timeout = \"30s\", shell = \"sh\" }
on.done = \"end.done\"

[[state]]
id = \"wait\"
wait = \"restack.resumed\"
";

    fn definition(source: &str) -> Definition {
        parse::parse(source).expect("the test workflow parses")
    }

    fn field<'a>(rows: &'a StateRows, name: &str) -> &'a FieldRow {
        rows.fields
            .iter()
            .find(|row| row.spec.name == name)
            .expect("field exists")
    }

    fn text_field(name: &'static str, kind: FieldKind, required: bool) -> FieldSpec {
        FieldSpec {
            name,
            kind,
            required,
            help: "help",
        }
    }

    #[test]
    fn state_rows_show_the_action_its_fields_and_the_unknown_keys() {
        let definition = definition(SOURCE);
        let rows = state_rows(&definition, "run").expect("an action state");
        assert_eq!(rows.action, "exec");
        assert_eq!(rows.level, Some("read"));
        assert_eq!(rows.domain, "general");
        let argv = field(&rows, "argv");
        assert!(argv.spec.required);
        assert_eq!(argv.value, "git\nstatus", "a list is one item per line");
        assert_eq!(field(&rows, "timeout").value, "30s");
        assert_eq!(field(&rows, "stdin").value, "");
        assert_eq!(rows.unknown, ["shell"]);
    }

    #[test]
    fn a_wait_state_has_no_action_rows() {
        assert!(state_rows(&definition(SOURCE), "wait").is_none());
    }

    #[test]
    fn set_takes_any_key_and_its_value_is_not_typed() {
        let source = "schema = 1\nname = \"t\"\n\n[trigger]\non = \"transform.commit_message\"\n\n[[state]]\nid = \"s\"\naction = \"set\"\nwith = { value = \"x\", extra = 1 }\n";
        let rows = state_rows(&definition(source), "s").expect("an action state");
        assert!(rows.unknown.is_empty());
        assert_eq!(field(&rows, "value").spec.kind, FieldKind::Any);
    }

    #[test]
    fn action_groups_mark_what_the_driver_cannot_run_and_the_phase_limited_set() {
        let groups = action_groups("manual");
        let entry = |name: &str| {
            groups
                .iter()
                .flat_map(|group| group.entries.iter())
                .find(|entry| entry.name == name)
                .expect("action listed")
                .clone()
        };
        assert_eq!(entry("exec").reason, None);
        assert_eq!(entry("exec").level, "read");
        assert_eq!(entry("restack.plan").reason, Some("not available yet"));
        assert_eq!(entry("set").reason, Some("only in a transform trigger"));
        assert_eq!(entry("commit.create").level, "apply");
        assert_eq!(entry("approve").level, "approve");
        assert_eq!(groups[0].domain, "general");
        let commit = groups.iter().find(|group| group.domain == "commit");
        assert!(commit.is_some_and(|group| group.entries.len() == 2));
    }

    #[test]
    fn set_is_enabled_in_a_transform_trigger() {
        let groups = action_groups("transform.commit_message");
        let set = groups
            .iter()
            .flat_map(|group| group.entries.iter())
            .find(|entry| entry.name == "set")
            .expect("set listed");
        assert_eq!(set.reason, None);
    }

    #[test]
    fn typed_problems_are_the_validators_word_for_word() {
        let timeout = text_field("timeout", FieldKind::Duration, false);
        assert!(typed_problem(&timeout, "10x").is_some_and(|text| text.contains("30s")));
        assert_eq!(typed_problem(&timeout, "2m"), None);
        assert_eq!(
            typed_problem(&timeout, ""),
            None,
            "an empty optional field fits"
        );
        let argv = text_field("argv", FieldKind::TextList, true);
        assert_eq!(typed_problem(&argv, "\n").as_deref(), Some("Required."));
        assert_eq!(typed_problem(&argv, "git"), None);
        let format = text_field("format", FieldKind::Choice(&["json"]), false);
        assert_eq!(
            typed_problem(&format, "yaml"),
            None,
            "choices are picked, not typed"
        );
    }

    #[test]
    fn list_items_are_the_non_empty_lines() {
        assert_eq!(list_items("git\n\nstatus\n"), ["git", "status"]);
        assert!(list_items("").is_empty());
    }

    #[test]
    fn slot_values_read_the_file() {
        let definition = definition(SOURCE);
        assert_eq!(slot_value(&definition, None, &Slot::Name), "flow");
        assert_eq!(slot_value(&definition, None, &Slot::Description), "Demo");
        assert_eq!(slot_value(&definition, None, &Slot::Match), "");
        assert_eq!(
            slot_value(&definition, None, &Slot::Prompt("ticket".to_owned())),
            "Ticket key"
        );
        assert_eq!(
            slot_value(&definition, Some("run"), &Slot::Field("argv".to_owned())),
            "git\nstatus"
        );
        assert_eq!(
            slot_value(&definition, None, &Slot::Field("argv".to_owned())),
            ""
        );
    }

    #[test]
    fn wanted_slots_follow_the_selected_action() {
        let definition = definition(SOURCE);
        let with_state = wanted_slots(&definition, Some("run"));
        assert!(with_state.contains(&Slot::Field("argv".to_owned())));
        assert!(with_state.contains(&Slot::Field("timeout".to_owned())));
        assert!(
            !with_state.contains(&Slot::Field("format".to_owned())),
            "a choice is not typed"
        );
        assert!(with_state.contains(&Slot::NewInput));
        assert!(with_state.contains(&Slot::Prompt("ticket".to_owned())));
        let without = wanted_slots(&definition, None);
        assert!(!without.iter().any(|slot| matches!(slot, Slot::Field(_))));
    }

    #[test]
    fn name_and_description_each_keep_the_other() {
        let definition = definition(SOURCE);
        let name = slot_edit(&definition, None, &Slot::Name, "renamed");
        assert_eq!(
            name,
            Some(FlowEdit::SetMeta {
                name: Some("renamed".to_owned()),
                description: Some("Demo".to_owned()),
            })
        );
        let description = slot_edit(&definition, None, &Slot::Description, "");
        assert_eq!(
            description,
            Some(FlowEdit::SetMeta {
                name: Some("flow".to_owned()),
                description: Some(String::new()),
            })
        );
    }

    #[test]
    fn match_keeps_the_trigger_event() {
        let definition = definition(SOURCE);
        assert_eq!(
            slot_edit(&definition, None, &Slot::Match, "branch:feat/*"),
            Some(FlowEdit::SetTrigger {
                on: "manual".to_owned(),
                matcher: Some("branch:feat/*".to_owned()),
            })
        );
    }

    #[test]
    fn a_new_input_is_trimmed_and_a_blank_one_is_not_sent() {
        let definition = definition(SOURCE);
        assert_eq!(
            slot_edit(&definition, None, &Slot::NewInput, "  ticket "),
            Some(FlowEdit::SetInput {
                name: "ticket".to_owned(),
                prompt: None,
            })
        );
        assert_eq!(slot_edit(&definition, None, &Slot::NewInput, "  "), None);
    }

    #[test]
    fn a_field_edit_sends_its_typed_value_or_nothing_when_it_does_not_fit() {
        let definition = definition(SOURCE);
        let argv = Slot::Field("argv".to_owned());
        assert_eq!(
            slot_edit(&definition, Some("run"), &argv, "git\n\nstatus"),
            Some(FlowEdit::SetField {
                field: "argv".to_owned(),
                value: FieldValue::TextList(vec!["git".to_owned(), "status".to_owned()]),
            })
        );
        assert_eq!(
            slot_edit(&definition, Some("run"), &argv, ""),
            None,
            "a required list cannot be empty"
        );
        let timeout = Slot::Field("timeout".to_owned());
        assert_eq!(
            slot_edit(&definition, Some("run"), &timeout, ""),
            Some(FlowEdit::SetField {
                field: "timeout".to_owned(),
                value: FieldValue::Text(String::new()),
            }),
            "an empty optional text removes the key in the edit"
        );
        assert_eq!(slot_edit(&definition, Some("run"), &timeout, "10x"), None);
        let shell = Slot::Field("shell".to_owned());
        assert_eq!(slot_edit(&definition, Some("wait"), &shell, "sh"), None);
    }

    #[test]
    fn the_level_labels_are_read_propose_apply_and_approve() {
        assert_eq!(level_label(Level::Read), "read");
        assert_eq!(level_label(Level::Propose), "propose");
        assert_eq!(level_label(Level::Apply), "apply");
        assert_eq!(level_label(Level::Pause), "approve");
    }
}
