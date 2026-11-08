//! The dialogs of the Memories tab (D190): New Memory, Accept and supersede, and the confirmation
//! of a delete. The rules are in `memory_form.rs` and `memories.rs`; this file draws the dialogs,
//! sends the writes they plan, and closes each one when the engine answers its write. A refusal
//! stays in the open dialog with the reason.

use std::collections::{BTreeSet, HashSet};

use gm_schema::threads::{
    Memory, MemoryContext, MemoryKind, MemorySourceInput, MemorySourceKind, MemoryStatus,
};
use gm_schema::{Event, OperationId, OperationState};
use gpui::{
    Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, Stateful, Subscription, Window,
    div, prelude::*, px,
};

use crate::contexts_model;
use crate::memories::{self, DetachStart, MemoriesPage, Request, Scope};
use crate::memory_form::{self, SourceDraft};
use crate::memory_rows;
use crate::shell_memories::{NO_WRITE, button, pick_chip};
use crate::text_area::{TextArea, TextAreaEvent};
use crate::theme;
use crate::thread_dialogs::{buttons, dialog_backdrop, dialog_left, error_lines, panel_base};
use crate::view::StatusView;
use crate::worktree_create::field_row;

/// The most of the list of memories a Supersedes… picker shows before it scrolls.
const PICK_HEIGHT: f32 = 240.;

/// A dialog of the Memories tab while it is open: what it asks, the error the last try got, and
/// the write it sent, which its answer settles.
pub(crate) struct MemoryDialog {
    pub(crate) form: DialogForm,
    pub(crate) error: Option<String>,
    pub(crate) pending: Option<OperationId>,
    _subscriptions: Vec<Subscription>,
}

/// What a dialog asks.
pub(crate) enum DialogForm {
    /// Writes a memory of the human's, accepted at once.
    New(NewForm),
    /// Accepts the proposal `uid` and marks the accepted memories chosen as superseded.
    Supersede {
        uid: String,
        chosen: BTreeSet<String>,
    },
    /// Deletes memory `uid`, after the question `title` and the lines under it.
    Delete {
        uid: String,
        title: String,
        lines: Vec<String>,
    },
    /// Moves or attaches memory `uid` into a context the picker lists, filtered by `filter`.
    Place {
        uid: String,
        action: PlaceAction,
        filter: Entity<TextArea>,
    },
    /// Detaches context memory `uid` from `context`, its only context.
    Detach { uid: String, context: MemoryContext },
    /// Detaches context memory `uid` from the context the picker chooses among `contexts`.
    DetachPick {
        uid: String,
        contexts: Vec<MemoryContext>,
    },
}

/// The two ways a memory joins a context from the picker (D193 (8)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlaceAction {
    /// The repository memory moves: it leaves the repository.
    Move,
    /// The accepted context memory is also attached to the chosen context.
    Attach,
}

/// The New Memory form as typed: its kind, its scope, the sources added so far, and the boxes
/// of the text and of the source being typed.
pub(crate) struct NewForm {
    pub(crate) kind: MemoryKind,
    pub(crate) scope: Scope,
    pub(crate) source_kind: MemorySourceKind,
    pub(crate) sources: Vec<MemorySourceInput>,
    pub(crate) text: Entity<TextArea>,
    pub(crate) reference: Entity<TextArea>,
    pub(crate) entry_from: Entity<TextArea>,
    pub(crate) entry_to: Entity<TextArea>,
}

impl StatusView {
    /// Opens New Memory. The contexts are read for the scope chips; the text box takes the cursor.
    pub(crate) fn open_new_memory_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.send_memories(MemoriesPage::plan_contexts());
        let text =
            cx.new(|cx| TextArea::new("Write the memory in plain words", cx).with_lines(4, 10));
        let reference = cx.new(|cx| {
            TextArea::new("A thread uid, a commit id or a file path", cx).with_lines(1, 1)
        });
        let entry_from = cx.new(|cx| TextArea::new("First entry", cx).with_lines(1, 1));
        let entry_to = cx.new(|cx| TextArea::new("Last entry", cx).with_lines(1, 1));
        let subscriptions = vec![
            cx.observe(&text, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &text,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    if *event == TextAreaEvent::Cancel {
                        this.close_memory_dialog(window, cx);
                    }
                },
            ),
        ];
        let form = NewForm {
            kind: MemoryKind::Decision,
            scope: Scope::Repository,
            source_kind: MemorySourceKind::Thread,
            sources: Vec::new(),
            text: text.clone(),
            reference,
            entry_from,
            entry_to,
        };
        self.memory_dialog = Some(MemoryDialog {
            form: DialogForm::New(form),
            error: None,
            pending: None,
            _subscriptions: subscriptions,
        });
        text.update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    /// Closes the open dialog without a write, and gives the list the keyboard back.
    pub(crate) fn close_memory_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.memory_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// The New Memory form, when that dialog is open.
    fn new_form_mut(&mut self) -> Option<&mut NewForm> {
        match self.memory_dialog.as_mut().map(|dialog| &mut dialog.form) {
            Some(DialogForm::New(form)) => Some(form),
            _ => None,
        }
    }

    /// Shows the reason a try was refused, in the open dialog.
    fn set_dialog_error(&mut self, error: Option<String>) {
        if let Some(dialog) = self.memory_dialog.as_mut() {
            dialog.error = error;
        }
    }

    /// Records the write the open dialog sent: it waits for the answer, with no error.
    fn set_dialog_pending(&mut self, id: OperationId) {
        if let Some(dialog) = self.memory_dialog.as_mut() {
            dialog.pending = Some(id);
            dialog.error = None;
        }
    }

    pub(crate) fn set_new_kind(&mut self, kind: MemoryKind, cx: &mut Context<Self>) {
        if let Some(form) = self.new_form_mut() {
            form.kind = kind;
        }
        cx.notify();
    }

    fn set_new_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if let Some(form) = self.new_form_mut() {
            form.scope = scope;
        }
        cx.notify();
    }

    fn set_new_source_kind(&mut self, kind: MemorySourceKind, cx: &mut Context<Self>) {
        if let Some(form) = self.new_form_mut() {
            form.source_kind = kind;
        }
        cx.notify();
    }

    /// Adds the source typed: it is checked first, and the boxes clear when it is added.
    fn add_new_source(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.new_form_mut() else {
            return;
        };
        let draft = SourceDraft {
            kind: form.source_kind,
            reference: form.reference.read(cx).text().to_owned(),
            entry_from: form.entry_from.read(cx).text().to_owned(),
            entry_to: form.entry_to.read(cx).text().to_owned(),
        };
        let planned = memory_form::source_input(&draft);
        if let Ok(source) = &planned {
            form.sources.push(source.clone());
            form.reference.update(cx, |area, cx| area.set_text("", cx));
            form.entry_from.update(cx, |area, cx| area.set_text("", cx));
            form.entry_to.update(cx, |area, cx| area.set_text("", cx));
        }
        match planned {
            Ok(_) => self.set_dialog_error(None),
            Err(reason) => self.set_dialog_error(Some(reason.to_owned())),
        }
        cx.notify();
    }

    /// Takes a source off the form.
    fn remove_new_source(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(form) = self.new_form_mut()
            && index < form.sources.len()
        {
            form.sources.remove(index);
        }
        cx.notify();
    }

    /// Save: the memory is written with the form's kind, scope, text and sources. The model
    /// checks them; a refusal stays in the dialog.
    fn submit_new_memory(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.memory_dialog.as_ref() else {
            return;
        };
        if dialog.pending.is_some() {
            return;
        }
        let DialogForm::New(form) = &dialog.form else {
            return;
        };
        let text = form.text.read(cx).text().to_owned();
        let planned =
            memory_form::save_request(form.kind, form.scope.clone(), &text, &form.sources);
        self.send_planned_dialog(planned, cx);
    }

    /// Opens Accept and supersede on the selected proposal: the picker lists the accepted
    /// memories the Memories tab shows.
    pub(crate) fn open_supersede_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(memory) = self.model.memories.selected_memory() else {
            return;
        };
        if memory.status != MemoryStatus::Proposed {
            self.memories_view.notice =
                Some("Only a proposed memory can supersede another.".to_owned());
            cx.notify();
            return;
        }
        let uid = memory.uid.clone();
        self.memory_dialog = Some(MemoryDialog {
            form: DialogForm::Supersede {
                uid,
                chosen: BTreeSet::new(),
            },
            error: None,
            pending: None,
            _subscriptions: Vec::new(),
        });
        cx.notify();
    }

    /// Chooses or unchooses an accepted memory the proposal supersedes.
    fn toggle_supersede(&mut self, memory: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = self.memory_dialog.as_mut()
            && let DialogForm::Supersede { chosen, .. } = &mut dialog.form
            && !chosen.remove(memory)
        {
            chosen.insert(memory.to_owned());
        }
        cx.notify();
    }

    /// Accept and supersede: the proposal is accepted and the chosen memories become superseded in
    /// the same write.
    fn submit_supersede(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.memory_dialog.as_ref() else {
            return;
        };
        if dialog.pending.is_some() {
            return;
        }
        let DialogForm::Supersede { uid, chosen } = &dialog.form else {
            return;
        };
        let supersedes: Vec<String> = chosen.iter().cloned().collect();
        let planned = self.model.memories.plan_supersede(uid, &supersedes);
        self.send_planned_dialog(planned, cx);
    }

    /// Opens the confirmation that deletes the selected memory. Nothing is sent before its button.
    pub(crate) fn open_delete_memory_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(memory) = self.model.memories.selected_memory() else {
            return;
        };
        let uid = memory.uid.clone();
        self.memory_dialog = Some(MemoryDialog {
            form: DialogForm::Delete {
                uid,
                title: "Delete this memory?".to_owned(),
                lines: vec![
                    "The memory, its sources and its links are deleted.".to_owned(),
                    "This cannot be undone.".to_owned(),
                ],
            },
            error: None,
            pending: None,
            _subscriptions: Vec::new(),
        });
        cx.notify();
    }

    /// The delete the confirmation asks for.
    fn submit_memory_delete(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.memory_dialog.as_ref() else {
            return;
        };
        if dialog.pending.is_some() {
            return;
        }
        let DialogForm::Delete { uid, .. } = &dialog.form else {
            return;
        };
        let planned = self.model.memories.plan_delete(uid);
        self.send_planned_dialog(planned, cx);
    }

    /// Opens the picker that moves or attaches memory `uid` into a context. The filter box takes
    /// the cursor; a choice in the list is sent at once, and a refusal stays in the dialog.
    pub(crate) fn open_place_dialog(
        &mut self,
        uid: &str,
        action: PlaceAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let filter = cx.new(|cx| TextArea::new("Filter the contexts", cx).with_lines(1, 1));
        let subscriptions = vec![
            cx.observe(&filter, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &filter,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    if *event == TextAreaEvent::Cancel {
                        this.close_memory_dialog(window, cx);
                    }
                },
            ),
        ];
        self.memory_dialog = Some(MemoryDialog {
            form: DialogForm::Place {
                uid: uid.to_owned(),
                action,
                filter: filter.clone(),
            },
            error: None,
            pending: None,
            _subscriptions: subscriptions,
        });
        filter.update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    /// Opens the detach of memory `uid` from its contexts: the confirmation when it is in one
    /// visible context, the picker of them when it is in several (D193 (1)).
    pub(crate) fn open_detach_dialog(&mut self, uid: &str, cx: &mut Context<Self>) {
        let start = match self.model.memories.find(uid) {
            Some(memory) => memories::detach_start(memory),
            None => return,
        };
        let form = match start {
            DetachStart::Nothing => return,
            DetachStart::Confirm(context) => DialogForm::Detach {
                uid: uid.to_owned(),
                context,
            },
            DetachStart::Pick(contexts) => DialogForm::DetachPick {
                uid: uid.to_owned(),
                contexts,
            },
        };
        self.memory_dialog = Some(MemoryDialog {
            form,
            error: None,
            pending: None,
            _subscriptions: Vec::new(),
        });
        cx.notify();
    }

    /// A context chosen in the detach picker: the detach it plans is sent.
    fn choose_detach_context(&mut self, context: i64, cx: &mut Context<Self>) {
        let Some(dialog) = self.memory_dialog.as_ref() else {
            return;
        };
        if dialog.pending.is_some() {
            return;
        }
        let DialogForm::DetachPick { uid, .. } = &dialog.form else {
            return;
        };
        let planned = self.model.memories.plan_detach(uid, context);
        self.send_planned_dialog(planned, cx);
    }

    /// A context chosen in the picker: the move or attach it plans is sent.
    fn choose_place_context(&mut self, context: i64, cx: &mut Context<Self>) {
        let Some(dialog) = self.memory_dialog.as_ref() else {
            return;
        };
        if dialog.pending.is_some() {
            return;
        }
        let DialogForm::Place { uid, action, .. } = &dialog.form else {
            return;
        };
        let planned = match action {
            PlaceAction::Move => self.model.memories.plan_move(uid, context),
            PlaceAction::Attach => self.model.memories.plan_attach(uid, context),
        };
        self.send_planned_dialog(planned, cx);
    }

    /// The detach the confirmation asks for.
    fn submit_memory_detach(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.memory_dialog.as_ref() else {
            return;
        };
        if dialog.pending.is_some() {
            return;
        }
        let DialogForm::Detach { uid, context } = &dialog.form else {
            return;
        };
        let planned = self.model.memories.plan_detach(uid, context.id);
        self.send_planned_dialog(planned, cx);
    }

    /// Sends the write a dialog planned, and waits for its answer; a refusal stays in the dialog.
    fn send_planned_dialog(
        &mut self,
        planned: Result<Request, &'static str>,
        cx: &mut Context<Self>,
    ) {
        match planned {
            Ok(request) => {
                let id = self.send_memories(request);
                self.set_dialog_pending(id);
            }
            Err(reason) => self.set_dialog_error(Some(reason.to_owned())),
        }
        cx.notify();
    }

    /// Follows the answer to the dialog's write: a saved memory closes the dialog and says so; a
    /// refused one stays with the engine's reason.
    pub(crate) fn follow_memory_dialog_write(&mut self, event: &Event, cx: &mut Context<Self>) {
        let Some(pending) = self
            .memory_dialog
            .as_ref()
            .and_then(|dialog| dialog.pending)
        else {
            return;
        };
        match event {
            Event::MemoryChanged { id, created, .. } if *id == pending => {
                let message = match self.memory_dialog.as_ref().map(|dialog| &dialog.form) {
                    Some(DialogForm::New(_)) if *created => "Memory saved.",
                    Some(DialogForm::New(_)) => "That memory is already here.",
                    Some(DialogForm::Supersede { .. }) => "Memory accepted.",
                    Some(DialogForm::Delete { .. }) => "Memory deleted.",
                    Some(DialogForm::Place {
                        action: PlaceAction::Move,
                        ..
                    }) => "Memory moved to the context.",
                    Some(DialogForm::Place {
                        action: PlaceAction::Attach,
                        ..
                    }) => "Memory attached to the context.",
                    Some(DialogForm::Detach { .. } | DialogForm::DetachPick { .. }) => {
                        "Memory detached from the context."
                    }
                    None => "",
                };
                self.memory_dialog = None;
                self.notify_user(message, cx);
            }
            Event::OperationChanged {
                id, state, detail, ..
            } if *id == pending && state.is_terminal() => {
                let failed = *state == OperationState::Failed;
                if let Some(dialog) = self.memory_dialog.as_mut() {
                    dialog.pending = None;
                    if failed {
                        dialog.error = Some(detail.clone().unwrap_or_else(|| NO_WRITE.to_owned()));
                    }
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// The layer of the open dialog, or none while it is closed.
    pub(crate) fn memory_layers(&self, window: &Window, cx: &Context<Self>) -> Vec<Div> {
        let Some(dialog) = self.memory_dialog.as_ref() else {
            return Vec::new();
        };
        let outside = dialog_backdrop(cx, StatusView::close_memory_dialog);
        let left = dialog_left(window);
        let panel = match &dialog.form {
            DialogForm::New(form) => self.new_memory_panel(dialog, form, left, cx),
            DialogForm::Supersede { uid, chosen } => {
                self.supersede_panel(dialog, uid, chosen, left, cx)
            }
            DialogForm::Delete { title, lines, .. } => {
                Self::memory_delete_panel(dialog, title, lines, left, cx)
            }
            DialogForm::Place { action, filter, .. } => {
                self.place_panel(dialog, *action, filter, left, cx)
            }
            DialogForm::Detach { context, .. } => {
                Self::memory_detach_panel(dialog, &context.path, left, cx)
            }
            DialogForm::DetachPick { contexts, .. } => {
                Self::memory_detach_pick_panel(dialog, contexts, left, cx)
            }
        };
        vec![div().absolute().inset_0().child(outside).child(panel)]
    }

    /// The New Memory panel: the kind and the scope chips, the text, the sources added and the box
    /// of the next one, then the error and Save.
    fn new_memory_panel(
        &self,
        dialog: &MemoryDialog,
        form: &NewForm,
        left: f32,
        cx: &Context<Self>,
    ) -> Div {
        let pending = dialog.pending.is_some();
        let chars = form.text.read(cx).text().chars().count();
        panel_base(left)
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("New Memory"),
            )
            .child(section_label("Kind"))
            .child(kind_chips(form, cx))
            .child(section_label("Scope"))
            .child(self.scope_chips(form, cx))
            .child(field_row("Text", form.text.clone()))
            .child(
                div()
                    .text_xs()
                    .text_color(if memory_rows::over_limit(chars) {
                        theme::error()
                    } else {
                        theme::muted()
                    })
                    .child(memory_rows::remaining_text(chars)),
            )
            .child(section_label("Sources (optional)"))
            .children(added_sources(form, cx))
            .child(source_kind_chips(form, cx))
            .child(field_row("Reference", form.reference.clone()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(field_row("First entry", form.entry_from.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(field_row("Last entry", form.entry_to.clone())),
                    ),
            )
            .child(button(
                "memory-new-add-source",
                "Add source",
                cx,
                |this, _, cx| {
                    this.add_new_source(cx);
                },
            ))
            .children(error_lines(dialog.error.as_deref()))
            .child(buttons(
                ("memory-new-cancel", "memory-new-save"),
                if pending { "Saving…" } else { "Save" },
                false,
                pending,
                StatusView::close_memory_dialog,
                StatusView::submit_new_memory,
                cx,
            ))
    }

    /// The scope chips of the form: this repository and each context it belongs to. Everything is
    /// not a place to write to, so the first option is left out.
    fn scope_chips(&self, form: &NewForm, cx: &Context<Self>) -> Div {
        let chips: Vec<Stateful<Div>> = memory_rows::scope_options(self.model.memories.contexts())
            .into_iter()
            .skip(1)
            .enumerate()
            .map(|(index, scope)| {
                let on = form.scope == scope;
                let label = memory_rows::scope_label(&scope);
                pick_chip(
                    ("memory-new-scope", index),
                    label,
                    on,
                    cx,
                    move |this, cx| this.set_new_scope(scope.clone(), cx),
                )
            })
            .collect();
        div().flex().flex_row().flex_wrap().gap_1().children(chips)
    }

    /// The Accept and supersede panel: the accepted memories the list shows, each chosen or not.
    fn supersede_panel(
        &self,
        dialog: &MemoryDialog,
        uid: &str,
        chosen: &BTreeSet<String>,
        left: f32,
        cx: &Context<Self>,
    ) -> Div {
        let pending = dialog.pending.is_some();
        let candidates: Vec<&Memory> = self
            .model
            .memories
            .memories()
            .iter()
            .filter(|memory| memory.status == MemoryStatus::Accepted && memory.uid != uid)
            .collect();
        let rows: Vec<Stateful<Div>> = candidates
            .iter()
            .enumerate()
            .map(|(index, memory)| {
                let on = chosen.contains(&memory.uid);
                let target = memory.uid.clone();
                div()
                    .id(("memory-supersede-row", index))
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .child(
                        div()
                            .flex_none()
                            .w(px(14.))
                            .h(px(14.))
                            .rounded_sm()
                            .border_1()
                            .border_color(theme::button_border())
                            .when(on, |check| check.bg(theme::accent())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(memory_rows::snippet(&memory.text)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.toggle_supersede(&target, cx);
                        }),
                    )
            })
            .collect();
        let empty = rows.is_empty().then(|| {
            div().text_sm().text_color(theme::muted()).child(
                "No accepted memory is shown. Show accepted memories in the list, then try again.",
            )
        });
        panel_base(left)
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Accept and supersede"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text())
                    .child("The memories you choose become Superseded when this one is accepted."),
            )
            .child(
                div()
                    .id("memory-supersede-list")
                    .max_h(px(PICK_HEIGHT))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows)
                    .children(empty),
            )
            .children(error_lines(dialog.error.as_deref()))
            .child(buttons(
                ("memory-supersede-cancel", "memory-supersede-go"),
                if pending {
                    "Saving…"
                } else {
                    "Accept and supersede"
                },
                false,
                pending,
                StatusView::close_memory_dialog,
                StatusView::submit_supersede,
                cx,
            ))
    }

    /// The delete confirmation: its question and lines, the error, and the red button.
    fn memory_delete_panel(
        dialog: &MemoryDialog,
        title: &str,
        lines: &[String],
        left: f32,
        cx: &Context<Self>,
    ) -> Div {
        let pending = dialog.pending.is_some();
        panel_base(left)
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_owned()),
            )
            .children(lines.iter().map(|line| {
                div()
                    .text_sm()
                    .text_color(theme::text())
                    .child(line.clone())
            }))
            .children(error_lines(dialog.error.as_deref()))
            .child(buttons(
                ("memory-delete-cancel", "memory-delete-go"),
                if pending {
                    "Deleting…"
                } else {
                    "Delete memory"
                },
                true,
                pending,
                StatusView::close_memory_dialog,
                StatusView::submit_memory_delete,
                cx,
            ))
    }

    /// The picker of Move to Context and Attach to Context: the filter box, the contexts it lets
    /// through (a choice sends the write), the error and Cancel.
    fn place_panel(
        &self,
        dialog: &MemoryDialog,
        action: PlaceAction,
        filter: &Entity<TextArea>,
        left: f32,
        cx: &Context<Self>,
    ) -> Div {
        let typed = filter.read(cx).text().to_owned();
        let choices = contexts_model::pick(
            self.model.memories.contexts(),
            &typed,
            &HashSet::new(),
            false,
        );
        let rows: Vec<Stateful<Div>> = choices
            .into_iter()
            .filter_map(|choice| choice.id.map(|id| (id, choice.label)))
            .enumerate()
            .map(|(index, (id, label))| {
                div()
                    .id(("memory-place-row", index))
                    .cursor_pointer()
                    .py_1()
                    .text_sm()
                    .text_color(theme::text())
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.choose_place_context(id, cx);
                        }),
                    )
            })
            .collect();
        let empty = rows.is_empty().then(|| {
            div().text_sm().text_color(theme::muted()).child(
                "No context matches. Choose a context this repository belongs to, or clear the filter.",
            )
        });
        let title = match action {
            PlaceAction::Move => "Move to Context",
            PlaceAction::Attach => "Attach to Context",
        };
        let intro = match action {
            PlaceAction::Move => {
                "The memory moves into the context you choose and leaves this repository."
            }
            PlaceAction::Attach => {
                "The memory stays where it is and is also found from the context you choose."
            }
        };
        panel_base(left)
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(div().text_sm().text_color(theme::text()).child(intro))
            .child(field_row("Filter", filter.clone()))
            .child(
                div()
                    .id("memory-place-list")
                    .max_h(px(PICK_HEIGHT))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows)
                    .children(empty),
            )
            .children(error_lines(dialog.error.as_deref()))
            .child(button(
                "memory-place-cancel",
                "Cancel",
                cx,
                StatusView::close_memory_dialog,
            ))
    }

    /// The detach picker: the contexts the memory is attached to, one row each by path. A row is
    /// sent at once, and a refusal stays in the dialog.
    fn memory_detach_pick_panel(
        dialog: &MemoryDialog,
        contexts: &[MemoryContext],
        left: f32,
        cx: &Context<Self>,
    ) -> Div {
        let rows: Vec<Stateful<Div>> = contexts
            .iter()
            .enumerate()
            .map(|(index, context)| {
                let id = context.id;
                div()
                    .id(("memory-detach-row", index))
                    .cursor_pointer()
                    .py_1()
                    .text_sm()
                    .text_color(theme::text())
                    .child(context.path.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.choose_detach_context(id, cx);
                        }),
                    )
            })
            .collect();
        panel_base(left)
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Detach from Context"),
            )
            .child(div().text_sm().text_color(theme::text()).child(
                "Choose the context the memory leaves. It stays in the others it belongs to; \
                     the last context of a memory cannot be detached.",
            ))
            .child(
                div()
                    .id("memory-detach-list")
                    .max_h(px(PICK_HEIGHT))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .children(error_lines(dialog.error.as_deref()))
            .child(button(
                "memory-detach-cancel",
                "Cancel",
                cx,
                StatusView::close_memory_dialog,
            ))
    }

    /// The detach confirmation: the context it leaves, the error, and the Detach button.
    fn memory_detach_panel(
        dialog: &MemoryDialog,
        context: &str,
        left: f32,
        cx: &Context<Self>,
    ) -> Div {
        let pending = dialog.pending.is_some();
        panel_base(left)
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Detach this memory?"),
            )
            .child(div().text_sm().text_color(theme::text()).child(format!(
                "The memory leaves {context}. It stays in the other contexts it belongs to; \
                     the last context of a memory cannot be detached."
            )))
            .children(error_lines(dialog.error.as_deref()))
            .child(buttons(
                ("memory-detach-cancel", "memory-detach-go"),
                if pending { "Detaching…" } else { "Detach" },
                false,
                pending,
                StatusView::close_memory_dialog,
                StatusView::submit_memory_detach,
                cx,
            ))
    }
}

/// The kinds of the form, one chip each: the chosen one filled.
fn kind_chips(form: &NewForm, cx: &Context<StatusView>) -> Div {
    let chips: Vec<Stateful<Div>> = memory_form::KIND_CHOICES
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            let kind = *kind;
            pick_chip(
                ("memory-new-kind", index),
                memories::kind_label(kind).to_owned(),
                form.kind == kind,
                cx,
                move |this, cx| this.set_new_kind(kind, cx),
            )
        })
        .collect();
    div().flex().flex_row().gap_1().children(chips)
}

/// The kinds of source a form names, one chip each.
fn source_kind_chips(form: &NewForm, cx: &Context<StatusView>) -> Div {
    let chips: Vec<Stateful<Div>> = memory_form::SOURCE_CHOICES
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            let kind = *kind;
            pick_chip(
                ("memory-new-source-kind", index),
                source_kind_label(kind).to_owned(),
                form.source_kind == kind,
                cx,
                move |this, cx| this.set_new_source_kind(kind, cx),
            )
        })
        .collect();
    div().flex().flex_row().gap_1().children(chips)
}

/// The sources added to the form, each with its line and a Remove link.
fn added_sources(form: &NewForm, cx: &Context<StatusView>) -> Vec<Div> {
    form.sources
        .iter()
        .enumerate()
        .map(|(index, source)| {
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
                        .text_ellipsis()
                        .text_sm()
                        .child(draft_label(source)),
                )
                .child(
                    div()
                        .id(("memory-new-source-remove", index))
                        .cursor_pointer()
                        .flex_none()
                        .text_xs()
                        .text_color(theme::muted())
                        .child("Remove")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.remove_new_source(index, cx);
                            }),
                        ),
                )
        })
        .collect()
}

/// A small heading of a dialog's part.
fn section_label(text: &'static str) -> Div {
    div().text_xs().text_color(theme::muted()).child(text)
}

/// The name of a kind of source on its chip and in the list of the form.
fn source_kind_label(kind: MemorySourceKind) -> &'static str {
    match kind {
        MemorySourceKind::Thread => "Thread",
        MemorySourceKind::Commit => "Commit",
        MemorySourceKind::File => "File",
    }
}

/// A source added to the form, as its list line: its kind, its reference and its entries.
fn draft_label(source: &MemorySourceInput) -> String {
    let entries = match (source.entry_from, source.entry_to) {
        (Some(from), Some(to)) if from == to => format!(" (entry {from})"),
        (Some(from), Some(to)) => format!(" (entries {from} to {to})"),
        _ => String::new(),
    };
    format!(
        "{}: {}{entries}",
        source_kind_label(source.kind),
        source.reference
    )
}
