//! The Labels page (P4, D145): the labels of the repository on the left, with a filter
//! box above them; the selected label on the right, with what is attached to it,
//! grouped by kind. The model is `labels_page.rs`; this file only draws it and sends
//! the reads it plans. A filter re-reads the list 250 ms after the last keystroke,
//! and Esc clears the filter.

use std::ops::Range;
use std::time::Duration;

use gm_schema::threads::{AttachKind, Label, LabelAttachment};
use gm_schema::{Actor, CommandKind, Event};
use gpui::{
    AnyElement, App, Context, Div, Entity, FocusHandle, FontWeight, KeyDownEvent, MouseButton,
    MouseDownEvent, Stateful, Subscription, UniformListScrollHandle, Window, div, prelude::*, px,
    uniform_list,
};

use crate::action_registry::{Action, Args, LabelDialog, LabelRef, label_args};
use crate::icon::{self, Icon};
use crate::label_curate::{self, Outcome, Write};
use crate::labels_page::{self, DetailRow, Empty};
use crate::shell_history::now_secs;
use crate::shell_threads::chip;
use crate::text_area::{Newline, TextArea, TextAreaEvent};
use crate::text_search::DEBOUNCE_MS;
use crate::theme;
use crate::thread_dialogs::{buttons, dialog_backdrop, dialog_left, error_lines, panel_base};
use crate::threads;
use crate::view::{Screen, StatusView};
use crate::window_chrome;
use crate::worktree_create::field_row;
use crate::{FocusNext, FocusPrev};

/// The left column's width: the filter box and the labels. Fixed, so the detail keeps the rest.
const LIST_WIDTH: f32 = 280.;
/// A label row's height in the list.
const LABEL_ROW_HEIGHT: f32 = 32.;
/// An attachment row's height: a title line and a line of its details. A group's heading
/// has the same height, since a uniform list draws rows of one height.
const DETAIL_ROW_HEIGHT: f32 = 44.;

/// The window's part of the page: the filter box and the lists' scroll. Its state is `model.labels`.
pub(crate) struct LabelsView {
    pub(crate) query: Entity<TextArea>,
    pub(crate) list: UniformListScrollHandle,
    pub(crate) detail: UniformListScrollHandle,
    /// Counts the filter's edits, so a delayed re-read knows whether it is still the latest.
    edits: u64,
}

impl LabelsView {
    pub(crate) fn new(query: Entity<TextArea>) -> Self {
        Self {
            query,
            list: UniformListScrollHandle::new(),
            detail: UniformListScrollHandle::new(),
            edits: 0,
        }
    }
}

/// The subscription that starts a delayed re-read when the filter changes. Made with the window.
pub(crate) fn subscriptions(
    query: &Entity<TextArea>,
    cx: &mut Context<StatusView>,
) -> Vec<Subscription> {
    vec![cx.observe(query, |this, _, cx| this.on_labels_edited(cx))]
}

/// Whether an engine event changes what the Labels page shows: a label write, or a
/// thread write, since a thread row shows its labels. Read before the model takes the event.
pub(crate) fn follows_change(event: &Event) -> bool {
    matches!(event, Event::LabelsChanged { .. } | Event::ThreadsChanged)
}

impl StatusView {
    /// Shows the page: the filter takes the cursor and the list is read again.
    pub(crate) fn show_labels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Labels;
        self.model.commit_stack.close();
        self.model.stash_stack.close();
        self.model.reflog_stack.close();
        self.request_labels_list();
        self.labels_view
            .query
            .update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    /// Reads the labels the filter asks for. Nothing is asked before a repository is
    /// open, or while an open is in flight.
    pub(crate) fn request_labels_list(&mut self) {
        if self.model.status().is_none() || self.model.is_opening() {
            return;
        }
        let kind = self.model.labels.plan_list();
        self.send_label_read(kind);
    }

    /// After a change the list and the selected label are read again, while the page is shown.
    pub(crate) fn follow_labels(&mut self, changed: bool) {
        if !changed || self.screen != Screen::Labels {
            return;
        }
        for kind in self.model.labels.changed() {
            self.send_label_read(kind);
        }
    }

    /// Dispatches a planned read and records which read the answer belongs to.
    fn send_label_read(&mut self, kind: CommandKind) {
        let is_list = matches!(kind, CommandKind::ListLabels { .. });
        let id = self.engine.dispatch(Actor::Human, kind);
        if is_list {
            self.model.labels.request_list(id);
        } else {
            self.model.labels.request_detail(id);
        }
    }

    /// The filter changed: the model takes the new text, and the list is read again
    /// after a pause, unless another edit came first.
    fn on_labels_edited(&mut self, cx: &mut Context<Self>) {
        if self.screen != Screen::Labels {
            return;
        }
        let text = self.labels_view.query.read(cx).text().to_owned();
        if text.contains('\n') {
            // The box is one line; Enter is not a line break.
            let first = text.lines().next().unwrap_or("").to_owned();
            self.labels_view
                .query
                .update(cx, |area, cx| area.set_text(&first, cx));
            return;
        }
        self.model.labels.set_query(&text);
        self.labels_view.edits += 1;
        let edit = self.labels_view.edits;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DEBOUNCE_MS))
                .await;
            let _ = this.update(cx, |view, _cx| {
                if view.labels_view.edits == edit {
                    view.request_labels_list();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Esc: the filter is cleared and the list shows every label again.
    fn clear_labels_filter(&mut self, cx: &mut Context<Self>) {
        self.labels_view
            .query
            .update(cx, |area, cx| area.set_text("", cx));
        cx.notify();
    }

    /// Selects label `id` (a click on its row) and reads what is attached to it.
    fn choose_label(&mut self, id: i64, cx: &mut Context<Self>) {
        let kind = self.model.labels.select(id);
        self.send_label_read(kind);
        cx.notify();
    }

    /// The page: a header with the count, then the list and the selected label.
    pub(crate) fn labels_page(&self, cx: &mut Context<Self>) -> Div {
        let page = &self.model.labels;
        let header = div()
            .h(px(44.))
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .border_b_1()
            .border_color(theme::hairline())
            .child(div().font_weight(FontWeight::MEDIUM).child("Labels"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(labels_page::count_label(page.labels().len())),
                    )
                    .child(self.label_action_button("labels-new", Action::LabelNew, Args::None, cx))
                    .child(self.label_action_button(
                        "labels-rename",
                        Action::LabelRename,
                        self.selected_label_args(),
                        cx,
                    ))
                    .child(self.label_action_button(
                        "labels-delete",
                        Action::LabelDelete,
                        self.selected_label_args(),
                        cx,
                    )),
            );
        let body = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_row()
            .child(self.labels_list_column(cx))
            .child(self.labels_detail(cx));
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(theme::ground())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key.as_str() == "escape" && !this.model.labels.query().is_empty()
                {
                    this.clear_labels_filter(cx);
                }
            }))
            .child(header)
            .child(body)
    }

    /// The left column: the filter box, the failure of the last read, then the labels or
    /// what says there are none.
    fn labels_list_column(&self, cx: &mut Context<Self>) -> Div {
        let page = &self.model.labels;
        let filter = div()
            .h(px(40.))
            .px_3()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(theme::hairline())
            .child(icon::view(
                Icon::MagnifyingGlass,
                icon::SMALL,
                theme::muted(),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(self.labels_view.query.clone()),
            );
        let failure = page.failure().map(|text| {
            div()
                .px_3()
                .py_2()
                .text_sm()
                .text_color(theme::error())
                .child(text.to_owned())
        });
        let body: AnyElement = match page.empty() {
            _ if page.is_loading() && page.labels().is_empty() => {
                note("Loading labels…").into_any_element()
            }
            Some(Empty::NoLabels) => note("No labels yet").into_any_element(),
            Some(Empty::NoMatches) => note("No label matches").into_any_element(),
            None => label_rows(page.visible().len(), self.labels_view.list.clone(), cx)
                .into_any_element(),
        };
        div()
            .w(px(LIST_WIDTH))
            .min_w(px(LIST_WIDTH))
            .max_w(px(LIST_WIDTH))
            .flex_none()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::hairline())
            .child(filter)
            .children(failure)
            .child(body)
    }

    /// The right column: the selected label's name, description and origin, then the
    /// attachments under their group headings, or what the page is waiting for.
    fn labels_detail(&self, cx: &mut Context<Self>) -> Div {
        let page = &self.model.labels;
        let Some(detail) = page.detail() else {
            let text = if page.selected().is_some() {
                "Loading label…"
            } else {
                "Select a label to see what is attached to it."
            };
            return div()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .flex()
                .flex_col()
                .child(note(text));
        };
        let label = &detail.label;
        let (origin, agent) = labels_page::origin_text(&label.origin);
        let title = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .child(color_dot(label))
            .child(
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::MEDIUM)
                    .child(label.name.clone()),
            );
        let origin_line = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(theme::muted())
            .child("Created by")
            .children(agent.then(|| chip(origin.clone())))
            .children((!agent).then(|| div().child(origin.clone())));
        let header = div()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_1()
            .border_b_1()
            .border_color(theme::hairline())
            .child(title)
            .children((!label.description.is_empty()).then(|| {
                div()
                    .text_sm()
                    .text_color(theme::muted())
                    .child(label.description.clone())
            }))
            .child(origin_line);
        let count = page.detail_rows().len();
        let attachments: AnyElement = if count == 0 {
            note("Nothing is attached to this label yet.").into_any_element()
        } else {
            detail_rows(count, self.labels_view.detail.clone(), cx).into_any_element()
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(header)
            .child(attachments)
    }
}

/// The New Label or Rename Label dialog while it is open: its fields and the error the
/// last try got. A rename shows only the name; a new label also takes a description and a colour.
pub(crate) struct LabelForm {
    /// The label being renamed; `None` for a new label.
    pub(crate) editing: Option<i64>,
    pub(crate) name: Entity<TextArea>,
    pub(crate) description: Entity<TextArea>,
    pub(crate) colour: Entity<TextArea>,
    pub(crate) error: Option<String>,
    /// The text the dialog last saw. The fields notify on every caret blink, so an error is
    /// cleared only when a field really changed.
    seen: String,
    _subscriptions: Vec<Subscription>,
}

impl LabelForm {
    /// The fields Tab moves between, in order: the name alone for a rename.
    fn visible_fields(&self) -> Vec<Entity<TextArea>> {
        if self.editing.is_some() {
            vec![self.name.clone()]
        } else {
            vec![
                self.name.clone(),
                self.description.clone(),
                self.colour.clone(),
            ]
        }
    }

    fn text(&self, cx: &App) -> String {
        [&self.name, &self.description, &self.colour]
            .map(|field| field.read(cx).text().to_owned())
            .join("\n")
    }
}

/// The confirmation of a label delete while it is open: its question and lines, and the
/// error the last try got.
pub(crate) struct LabelConfirm {
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
    pub(crate) error: Option<String>,
    pub(crate) focus: FocusHandle,
}

impl StatusView {
    /// The page's header buttons and the label rows' menus: each runs its action on the
    /// label it names. A button that cannot run now stays dimmed, and its hint gives the reason.
    fn label_action_button(
        &self,
        id: &'static str,
        action: Action,
        args: Args,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let state = action.enabled(&args, &self.action_context(""));
        let button = div()
            .id(id)
            .h(px(26.))
            .px_3()
            .flex()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme::button_border())
            .text_xs()
            .child(action.spec().title);
        match state {
            Ok(()) => button.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.run_action(action, &args, window, cx);
                }),
            ),
            Err(reason) => button
                .text_color(theme::muted())
                .tooltip(window_chrome::hint(reason)),
        }
    }

    /// The label the page has selected, as the actions take it; `Args::None` when none is selected.
    fn selected_label_args(&self) -> Args {
        self.model
            .labels
            .selected_label()
            .map_or(Args::None, |label| {
                label_args(LabelRef {
                    id: label.id,
                    name: &label.name,
                    attachments: label.attachments,
                })
            })
    }

    /// Opens the dialog a label action names. Nothing is sent here.
    pub(crate) fn open_label_dialog(
        &mut self,
        dialog: LabelDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match dialog {
            LabelDialog::New => self.open_label_form(None, "", window, cx),
            LabelDialog::Rename { id, name } => {
                self.open_label_form(Some(id), &name, window, cx);
            }
            LabelDialog::Delete {
                id,
                name,
                attachments,
            } => self.open_label_confirm(id, &name, attachments, window, cx),
        }
    }

    /// Opens the New Label dialog (`editing` is `None`) or the Rename Label dialog of
    /// label `editing`, with its name in the field.
    fn open_label_form(
        &mut self,
        editing: Option<i64>,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name_field = cx.new(|cx| TextArea::new("Label name", cx).with_lines(1, 1));
        let description =
            cx.new(|cx| TextArea::new("What the label means (optional)", cx).with_lines(1, 1));
        let colour = cx.new(|cx| TextArea::new("#rrggbb (optional)", cx).with_lines(1, 1));
        name_field.update(cx, |area, cx| area.set_text(name, cx));
        let mut subscriptions = Vec::new();
        for field in [&name_field, &description, &colour] {
            subscriptions.push(cx.observe(field, |this, _, cx| this.on_label_form_edited(cx)));
            subscriptions.push(cx.subscribe_in(
                field,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    if *event == TextAreaEvent::Cancel {
                        this.close_label_form(window, cx);
                    }
                },
            ));
        }
        let mut form = LabelForm {
            editing,
            name: name_field.clone(),
            description,
            colour,
            error: None,
            seen: String::new(),
            _subscriptions: subscriptions,
        };
        form.seen = form.text(cx);
        self.label_form = Some(form);
        name_field.update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    /// The text changed: the error of the last try goes, since it may be about the old text.
    fn on_label_form_edited(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.label_form.as_ref() else {
            return;
        };
        let text = form.text(cx);
        if form.seen == text {
            return;
        }
        if let Some(form) = self.label_form.as_mut() {
            form.seen = text;
            form.error = None;
        }
        cx.notify();
    }

    pub(crate) fn close_label_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.label_form.take().is_some() {
            self.return_label_focus(window, cx);
        }
        cx.notify();
    }

    /// Puts the cursor back where a dialog was opened from: the filter on the Labels page.
    fn return_label_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Labels {
            self.labels_view
                .query
                .update(cx, |area, cx| area.focus(window, cx));
        } else {
            self.list_focus.focus(window);
        }
    }

    /// Tab and Shift+Tab: the next or previous field of the dialog, wrapping around.
    fn move_label_field(&self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.label_form.as_ref() else {
            return;
        };
        let fields = form.visible_fields();
        let current = fields
            .iter()
            .position(|area| area.read(cx).is_focused(window))
            .unwrap_or(fields.len() - 1);
        let next = if forward {
            (current + 1) % fields.len()
        } else {
            (current + fields.len() - 1) % fields.len()
        };
        let target = fields[next].clone();
        target.update(cx, |area, cx| area.focus(window, cx));
    }

    /// The button of the form: checks the fields with the rules of `label_curate`, then sends
    /// the create or the rename with `Actor::Human`. A refusal stays in the dialog.
    pub(crate) fn submit_label_form(&mut self, cx: &mut Context<Self>) {
        if self.model.label_writes.is_pending() {
            return;
        }
        let Some(form) = self.label_form.as_ref() else {
            return;
        };
        let editing = form.editing;
        let name = form.name.read(cx).text().to_owned();
        let description = form.description.read(cx).text().to_owned();
        let colour = form.colour.read(cx).text().to_owned();
        let labels = self.model.labels.labels();
        let planned = match editing {
            Some(id) => label_curate::rename_label_command(id, &name, labels)
                .map(|kind| (kind, Write::Rename)),
            None => label_curate::new_label_command(&name, &description, &colour, labels)
                .map(|kind| (kind, Write::Create)),
        };
        match planned {
            Ok((kind, write)) => {
                if let Some(form) = self.label_form.as_mut() {
                    form.error = None;
                }
                self.send_label_write(kind, write, cx);
            }
            Err(reason) => {
                if let Some(form) = self.label_form.as_mut() {
                    form.error = Some(reason.to_owned());
                }
                cx.notify();
            }
        }
    }

    /// Opens the confirmation that deletes label `id`. Nothing is sent until its button is pressed.
    fn open_label_confirm(
        &mut self,
        id: i64,
        name: &str,
        attachments: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        focus.focus(window);
        self.label_confirm = Some(LabelConfirm {
            id,
            title: label_curate::delete_title(name),
            lines: label_curate::delete_lines(attachments),
            error: None,
            focus,
        });
        cx.notify();
    }

    pub(crate) fn close_label_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.label_confirm.take().is_some() {
            self.return_label_focus(window, cx);
        }
        cx.notify();
    }

    /// The delete button and Enter. The deleted label leaves the selection first, so no
    /// read of it follows the change.
    pub(crate) fn submit_label_confirm(&mut self, cx: &mut Context<Self>) {
        if self.model.label_writes.is_pending() {
            return;
        }
        let Some(confirm) = self.label_confirm.as_mut() else {
            return;
        };
        confirm.error = None;
        let id = confirm.id;
        self.model.labels.forget(id);
        self.send_label_write(label_curate::delete_command(id), Write::Delete, cx);
    }

    /// The Remove button of an attachment row: detaches it from `label`. A detach asks
    /// nothing, since attaching it again restores it.
    pub(crate) fn detach_label(
        &mut self,
        attachment: &LabelAttachment,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        if self.model.label_writes.is_pending() {
            self.notify_user("Wait for the last label change to finish.", cx);
            return;
        }
        let kind = label_curate::detach_command(label, attachment);
        self.send_label_write(kind, Write::Detach, cx);
    }

    /// Sends a label write with `Actor::Human` and tracks it until the engine settles it.
    fn send_label_write(&mut self, kind: CommandKind, write: Write, cx: &mut Context<Self>) {
        let id = self.engine.dispatch(Actor::Human, kind);
        self.model.label_writes.start(id, write);
        cx.notify();
    }

    /// Runs when the engine settles a label write. A success closes its dialog and says so;
    /// a refusal stays in the open dialog, or else goes to the status bar.
    pub(crate) fn on_label_write(
        &mut self,
        outcome: Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Outcome::Done(write) => {
                let closed = match write {
                    Write::Create | Write::Rename => self.label_form.take().is_some(),
                    Write::Delete => self.label_confirm.take().is_some(),
                    Write::Detach => false,
                };
                if closed {
                    self.return_label_focus(window, cx);
                }
                self.notify_user(label_curate::done_notice(write), cx);
            }
            Outcome::Failed(message) => {
                if let Some(form) = self.label_form.as_mut() {
                    form.error = Some(message);
                } else if let Some(confirm) = self.label_confirm.as_mut() {
                    confirm.error = Some(message);
                } else {
                    self.notify_user(&message, cx);
                }
            }
        }
        cx.notify();
    }

    /// The layers of the two label dialogs, or none while both are closed.
    pub(crate) fn label_layers(&self, window: &Window, cx: &Context<Self>) -> Vec<Div> {
        self.label_form_layer(window, cx)
            .into_iter()
            .chain(self.label_confirm_layer(window, cx))
            .collect()
    }

    fn label_form_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let form = self.label_form.as_ref()?;
        let outside = dialog_backdrop(cx, StatusView::close_label_form);
        let left = dialog_left(window);
        let editing = form.editing.is_some();
        let pending = self.model.label_writes.is_pending();
        let saving = self.model.label_writes.is_working(if editing {
            Write::Rename
        } else {
            Write::Create
        });
        let title = if editing { "Rename Label" } else { "New Label" };
        let confirm = match (saving, editing) {
            (true, _) => "Saving…",
            (false, true) => "Rename",
            (false, false) => "Create",
        };
        let enter = cx.entity().downgrade();
        let forward = enter.clone();
        let backward = enter.clone();
        let panel = panel_base(left)
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(field_row("NAME", form.name.clone()))
            .children((!editing).then(|| field_row("DESCRIPTION", form.description.clone())))
            .children((!editing).then(|| field_row("COLOUR", form.colour.clone())))
            .children(error_lines(form.error.as_deref()))
            .child(buttons(
                ("label-form-cancel", "label-form-confirm"),
                confirm,
                false,
                pending,
                StatusView::close_label_form,
                StatusView::submit_label_form,
                cx,
            ))
            .capture_action(move |_: &FocusNext, window, cx| {
                let _ = forward.update(cx, |this, cx| this.move_label_field(true, window, cx));
                cx.stop_propagation();
            })
            .capture_action(move |_: &FocusPrev, window, cx| {
                let _ = backward.update(cx, |this, cx| this.move_label_field(false, window, cx));
                cx.stop_propagation();
            })
            // Enter sends: each field is one line.
            .capture_action(move |_: &Newline, _, cx| {
                let _ = enter.update(cx, StatusView::submit_label_form);
                cx.stop_propagation();
            });
        Some(div().absolute().inset_0().child(outside).child(panel))
    }

    fn label_confirm_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let confirm = self.label_confirm.as_ref()?;
        let outside = dialog_backdrop(cx, StatusView::close_label_confirm);
        let left = dialog_left(window);
        let pending = self.model.label_writes.is_pending();
        let saving = self.model.label_writes.is_working(Write::Delete);
        let label = if saving { "Working…" } else { "Delete label" };
        let panel = panel_base(left)
            .track_focus(&confirm.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.close_label_confirm(window, cx),
                    "enter" => this.submit_label_confirm(cx),
                    _ => {}
                }
            }))
            .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
            .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(confirm.title.clone()),
            )
            .children(confirm.lines.iter().map(|line| {
                div()
                    .text_sm()
                    .text_color(theme::text())
                    .child(line.clone())
            }))
            .children(error_lines(confirm.error.as_deref()))
            .child(buttons(
                ("label-confirm-cancel", "label-confirm-go"),
                label,
                true,
                pending,
                StatusView::close_label_confirm,
                StatusView::submit_label_confirm,
                cx,
            ));
        Some(div().absolute().inset_0().child(outside).child(panel))
    }
}

/// The labels, read from the list as they are drawn. Each row selects its label.
fn label_rows(count: usize, list: UniformListScrollHandle, cx: &mut Context<StatusView>) -> Div {
    div().flex_1().min_h(px(0.)).flex().flex_col().child(
        uniform_list(
            "label-rows",
            count,
            cx.processor(
                move |this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut Window,
                      cx: &mut Context<StatusView>| {
                    let selected = this.model.labels.selected();
                    let labels = this.model.labels.visible();
                    labels
                        .get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(label, index)| {
                            let is_selected = selected == Some(label.id);
                            label_row(label, index, is_selected, cx)
                        })
                        .collect::<Vec<_>>()
                },
            ),
        )
        .track_scroll(list)
        .flex_1()
        .min_h(px(0.)),
    )
}

/// One label in the list: its colour, its name, an agent's name as a tag, and how many rows it holds.
fn label_row(
    label: &Label,
    index: usize,
    selected: bool,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let id = label.id;
    let menu_name = label.name.clone();
    let menu_attachments = label.attachments;
    let (origin, agent) = labels_page::origin_text(&label.origin);
    let mut row = div()
        .id(("label-row", index))
        .cursor_pointer()
        .h(px(LABEL_ROW_HEIGHT))
        .px_3()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| this.choose_label(id, cx)),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.open_label_menu(id, &menu_name, menu_attachments, event.position, cx);
            }),
        );
    if selected {
        row = row.bg(theme::selected_row());
    }
    row.child(color_dot(label))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label.name.clone()),
        )
        .children(agent.then(|| chip(origin)))
        .child(
            div()
                .flex_none()
                .px_2()
                .rounded_full()
                .bg(theme::selection())
                .text_color(theme::accent())
                .text_xs()
                .child(label.attachments.to_string()),
        )
}

/// The attachments of the selected label, read from the list as they are drawn.
fn detail_rows(count: usize, list: UniformListScrollHandle, cx: &mut Context<StatusView>) -> Div {
    div().flex_1().min_h(px(0.)).flex().flex_col().child(
        uniform_list(
            "label-detail-rows",
            count,
            cx.processor(
                move |this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut Window,
                      cx: &mut Context<StatusView>| {
                    let now = now_secs();
                    let label = this
                        .model
                        .labels
                        .detail()
                        .map(|detail| detail.label.name.clone())
                        .unwrap_or_default();
                    let rows = this.model.labels.detail_rows();
                    rows.get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(row, index)| match row {
                            DetailRow::Heading(kind) => heading_row(*kind).into_any_element(),
                            DetailRow::Attachment(attachment) => {
                                attachment_row(attachment, index, now, &label, cx)
                                    .into_any_element()
                            }
                        })
                        .collect::<Vec<_>>()
                },
            ),
        )
        .track_scroll(list)
        .flex_1()
        .min_h(px(0.)),
    )
}

/// The heading of one group of attachments.
fn heading_row(kind: AttachKind) -> Div {
    let title = match kind {
        AttachKind::Thread => "Threads",
        AttachKind::File => "Files",
        AttachKind::Commit => "Commits",
        AttachKind::Label => "Labels",
    };
    div()
        .h(px(DETAIL_ROW_HEIGHT))
        .px_4()
        .flex()
        .items_end()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::muted())
        .child(title)
}

/// One attachment: its title, then who made it, when, and what is worth knowing about
/// it (a commit's state, a file's old path). A thread row opens that thread. Its Remove
/// button detaches it from `label`; an agent's attachment keeps the button in view, the
/// others show it on hover.
fn attachment_row(
    attachment: &LabelAttachment,
    index: usize,
    now: i64,
    label: &str,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let group = format!("label-attachment-{index}");
    let mut row = div()
        .id(("label-attachment", index))
        .group(group.clone())
        .h(px(DETAIL_ROW_HEIGHT))
        .px_4()
        .flex()
        .flex_col()
        .justify_center()
        .min_w(px(0.))
        .overflow_hidden();
    if attachment.kind == AttachKind::Thread {
        let uid = attachment.reference.clone();
        row = row.cursor_pointer().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.open_thread_from_commit(&uid, window, cx);
            }),
        );
    }
    let (origin, agent) = labels_page::origin_text(&attachment.origin);
    let extra = match attachment.kind {
        AttachKind::Commit => labels_page::commit_state_text(attachment.state)
            .map(|text| div().text_color(theme::warn_fg()).child(text)),
        AttachKind::File => attachment
            .current_path
            .as_deref()
            .filter(|path| *path != attachment.reference)
            .map(|_| {
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(format!("attached as {}", attachment.reference))
            }),
        AttachKind::Thread | AttachKind::Label => None,
    };
    let meta = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .text_xs()
        .text_color(theme::muted())
        .children(agent.then(|| chip(origin.clone())))
        .children((!agent).then(|| div().child(origin.clone())))
        .child(threads::relative(now, attachment.at))
        .children(extra);
    let detached = attachment.clone();
    let label = label.to_owned();
    let remove = div()
        .id(("label-attachment-remove", index))
        .flex_none()
        .w(px(20.))
        .h(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor_pointer()
        .tooltip(window_chrome::hint("Remove from label"))
        .child(icon::view(Icon::Close, icon::SMALL, theme::muted()));
    let remove = if agent {
        remove
    } else {
        remove.invisible().group_hover(group, gpui::Styled::visible)
    };
    let remove = remove.on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
            // The row under the button opens its thread; a Remove must not.
            cx.stop_propagation();
            this.detach_label(&detached, &label, cx);
        }),
    );
    let title = div()
        .flex_1()
        .min_w(px(0.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_sm()
        .child(labels_page::attachment_title(attachment));
    row.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .child(title)
            .child(remove),
    )
    .child(meta)
}

/// The label's colour as a dot; a label without one shows a faint dot.
fn color_dot(label: &Label) -> Div {
    let colour = label
        .color
        .as_deref()
        .and_then(labels_page::hex_colour)
        .map_or_else(theme::faint, theme::color);
    div()
        .flex_none()
        .w(px(8.))
        .h(px(8.))
        .rounded_full()
        .bg(colour)
}

/// A line of text in the middle of a column, for the empty and loading states.
fn note(text: &str) -> Div {
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .items_center()
        .justify_center()
        .px_4()
        .text_color(theme::muted())
        .child(text.to_owned())
}

#[cfg(test)]
mod tests {
    use gm_schema::OperationId;

    use super::*;

    #[test]
    fn a_label_or_thread_change_is_followed_and_nothing_else() {
        assert!(follows_change(&Event::LabelsChanged {
            id: OperationId(1),
            label: None,
        }));
        assert!(follows_change(&Event::ThreadsChanged));
        assert!(!follows_change(&Event::LabelsListed {
            id: OperationId(1),
            labels: Vec::new(),
        }));
    }
}
