//! The Manage Contexts dialog (D191, design/threads.md "Contexts"): the contexts as an indented
//! tree on the left, the selected context's repositories on the right, and the writes that change
//! them. The rules are in `contexts_model.rs`; this file draws the dialog, sends the commands the
//! model plans and follows their answers. A refusal stays in the open dialog with the reason; an
//! engine failure is shown as the model keeps it.
//!
//! Context names and paths are text the person typed or the engine stored: they are shown, never
//! read as instructions.

use std::ops::Range;

use gm_schema::{Actor, CommandKind, Event, OperationId};
use gpui::{
    Context, Div, Entity, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    PathPromptOptions, Stateful, Subscription, UniformListScrollHandle, Window, div, prelude::*,
    px, uniform_list,
};

use crate::contexts_model::{
    ContextRow, ContextsModel, DeleteConfirm, DeletePlan, DeletePreview, counts_text,
};
use crate::icon::{self, Icon};
use crate::memory_rows;
use crate::shell_memories::button;
use crate::text_area::{Newline, TextArea};
use crate::theme;
use crate::thread_dialogs::{buttons, dialog_backdrop, error_lines};
use crate::view::StatusView;
use crate::workspace_create::repo_choices;
use crate::worktree_create::field_row;

/// The width of the dialog.
const WIDTH: f32 = 720.;
/// The top edge of the dialog, under the title bar.
const TOP: f32 = 72.;
/// The width of the tree, the context names beside it and the right pane.
const TREE_WIDTH: f32 = 320.;
/// The height of the tree's list. Its rows are virtualized.
const TREE_HEIGHT: f32 = 320.;
/// The height of one row of the tree; `uniform_list` needs every row the same.
const ROW_HEIGHT: f32 = 28.;
/// The indent of one level of the tree.
const INDENT: f32 = 16.;
/// The most titles the delete confirmation names before it says how many more follow.
const TITLES_SHOWN: usize = 50;
/// The most of the parents list the Move… picker shows before it scrolls.
const PICK_HEIGHT: f32 = 240.;

/// What the dialog shows and waits for: its mode, the write or preview in flight, its inputs,
/// and the focus that carries the keys.
pub(crate) struct ContextsDialog {
    pub(crate) mode: ContextsMode,
    /// The refusal of the last try, as the model planned it.
    pub(crate) error: Option<String>,
    /// The write or the delete preview in flight. Its answer settles it.
    pub(crate) pending: Option<OperationId>,
    /// The context whose ⋯ menu is open.
    pub(crate) menu: Option<i64>,
    /// Whether the repository chooser of Bind Repository is open.
    pub(crate) adding: bool,
    pub(crate) name: Entity<TextArea>,
    pub(crate) filter: Entity<TextArea>,
    pub(crate) focus: FocusHandle,
    pub(crate) tree: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

/// What the dialog's right pane is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ContextsMode {
    /// The tree and the selected context's repositories.
    Browse,
    /// A name for a new context, or for the rename of one.
    Name(NameFor),
    /// The parents the Move… picker offers for context `id`.
    Move(i64),
    /// The confirmation of a delete that removes memories held by no other context.
    Delete(DeleteConfirm),
}

/// Which name the name box is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameFor {
    /// A new context, under the selected one when `under_selected` is set.
    New { under_selected: bool },
    /// The rename of context `id`.
    Rename(i64),
}

impl StatusView {
    /// Opens Manage Contexts. The contexts are read at once; the dialog takes the keys.
    pub(crate) fn open_contexts_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = cx.new(|cx| TextArea::new("A name for the context", cx).with_lines(1, 1));
        let filter = cx.new(|cx| TextArea::new("Filter the parents", cx).with_lines(1, 1));
        let subscriptions = vec![
            cx.observe(&name, |_, _, cx| cx.notify()),
            cx.observe(&filter, |_, _, cx| cx.notify()),
        ];
        let focus = cx.focus_handle();
        focus.focus(window);
        self.contexts_dialog = Some(ContextsDialog {
            mode: ContextsMode::Browse,
            error: None,
            pending: None,
            menu: None,
            adding: false,
            name,
            filter,
            focus,
            tree: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        });
        self.send_contexts(&ContextsModel::plan_list());
        cx.notify();
    }

    /// Closes the dialog, and gives the list the keyboard back. A write in flight goes on; its
    /// answer is applied to the model when it comes.
    pub(crate) fn close_contexts_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.contexts_dialog.take().is_some() {
            self.model.contexts.deselect();
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// Sends a command of the dialog and records it with the model, so its answer is applied.
    fn send_contexts(&mut self, kind: &CommandKind) -> OperationId {
        let id = self.engine.dispatch(Actor::Human, kind.clone());
        self.model.contexts.sent(kind, id);
        id
    }

    /// Sends the command a planner gave, and waits for its answer; a refusal stays in the dialog.
    fn send_planned_contexts(
        &mut self,
        planned: Result<CommandKind, &'static str>,
        cx: &mut Context<Self>,
    ) {
        match planned {
            Ok(kind) => {
                let id = self.send_contexts(&kind);
                self.set_contexts_pending(Some(id));
            }
            Err(reason) => self.set_contexts_error(Some(reason.to_owned())),
        }
        cx.notify();
    }

    /// Records the write or preview the dialog waits for, with no error.
    fn set_contexts_pending(&mut self, pending: Option<OperationId>) {
        if let Some(dialog) = self.contexts_dialog.as_mut() {
            dialog.pending = pending;
            dialog.error = None;
        }
    }

    /// Shows the reason a try was refused, in the open dialog.
    fn set_contexts_error(&mut self, error: Option<String>) {
        if let Some(dialog) = self.contexts_dialog.as_mut() {
            dialog.error = error;
        }
    }

    /// Puts the dialog into `mode`, with no error and no ⋯ menu open.
    fn set_contexts_mode(&mut self, mode: ContextsMode) {
        if let Some(dialog) = self.contexts_dialog.as_mut() {
            dialog.mode = mode;
            dialog.error = None;
            dialog.menu = None;
        }
    }

    /// Selects a context of the tree and closes any ⋯ menu.
    fn select_context(&mut self, id: i64, cx: &mut Context<Self>) {
        self.model.contexts.select(id);
        if let Some(dialog) = self.contexts_dialog.as_mut() {
            dialog.menu = None;
            dialog.adding = false;
        }
        cx.notify();
    }

    /// Opens or closes the ⋯ menu of row `id`.
    fn toggle_contexts_menu(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(dialog) = self.contexts_dialog.as_mut() {
            dialog.menu = if dialog.menu == Some(id) {
                None
            } else {
                Some(id)
            };
        }
        cx.notify();
    }

    /// Opens the name box: for a new context, under the selected one when there is one; for the
    /// rename of `id`, with its name in the box.
    fn start_contexts_name(
        &mut self,
        target: NameFor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = match target {
            NameFor::New { .. } => String::new(),
            NameFor::Rename(id) => self
                .model
                .contexts
                .find(id)
                .map(|context| context.name.clone())
                .unwrap_or_default(),
        };
        self.set_contexts_mode(ContextsMode::Name(target));
        if let Some(dialog) = self.contexts_dialog.as_ref() {
            let name = dialog.name.clone();
            name.update(cx, |area, cx| {
                area.set_text(&text, cx);
                area.focus(window, cx);
            });
        }
        cx.notify();
    }

    /// Flips whether a new context goes under the selected one.
    fn toggle_under_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.contexts_dialog.as_mut()
            && let ContextsMode::Name(NameFor::New { under_selected }) = &mut dialog.mode
        {
            *under_selected = !*under_selected;
        }
        cx.notify();
    }

    /// Save of the name box: the context is created or renamed as the model plans it.
    fn confirm_contexts_name(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.contexts_dialog.as_ref() else {
            return;
        };
        if dialog.pending.is_some() {
            return;
        }
        let ContextsMode::Name(target) = dialog.mode.clone() else {
            return;
        };
        let text = dialog.name.read(cx).text().to_owned();
        let planned = match target {
            NameFor::New { under_selected } => {
                self.model.contexts.new_context(&text, under_selected)
            }
            NameFor::Rename(id) => self.model.contexts.rename(id, &text),
        };
        self.send_planned_contexts(planned, cx);
    }

    /// Enter in the name box is Save.
    fn contexts_enter(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.contexts_dialog.as_ref().map(|dialog| &dialog.mode),
            Some(ContextsMode::Name(_))
        ) {
            self.confirm_contexts_name(cx);
        }
    }

    /// Opens the Move… picker of context `id`, with an empty filter.
    fn open_contexts_move(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.set_contexts_mode(ContextsMode::Move(id));
        if let Some(dialog) = self.contexts_dialog.as_ref() {
            let filter = dialog.filter.clone();
            filter.update(cx, |area, cx| {
                area.set_text("", cx);
                area.focus(window, cx);
            });
        }
        cx.notify();
    }

    /// A parent chosen in the Move… picker (`None` for the top level): the move is sent.
    fn move_context_to(&mut self, parent: Option<i64>, cx: &mut Context<Self>) {
        let Some(ContextsMode::Move(id)) = self
            .contexts_dialog
            .as_ref()
            .map(|dialog| dialog.mode.clone())
        else {
            return;
        };
        let planned = self.model.contexts.move_to(id, parent);
        self.send_planned_contexts(planned, cx);
    }

    /// Delete from the ⋯ menu: the memories the delete would remove are previewed first, and the
    /// model says whether the person must confirm.
    fn start_contexts_delete(&mut self, id: i64, cx: &mut Context<Self>) {
        let preview = self.send_contexts(&CommandKind::PreviewDeleteContext { id });
        self.set_contexts_pending(Some(preview));
        if let Some(dialog) = self.contexts_dialog.as_mut() {
            dialog.menu = None;
        }
        cx.notify();
    }

    /// The delete the confirmation asks for.
    fn confirm_contexts_delete(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.contexts_dialog.as_ref() else {
            return;
        };
        if dialog.pending.is_some() {
            return;
        }
        let ContextsMode::Delete(confirm) = dialog.mode.clone() else {
            return;
        };
        let kind = confirm.confirmed();
        self.send_planned_contexts(Ok(kind), cx);
    }

    /// Back from a name, a picker or a confirmation to the tree.
    fn cancel_contexts_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_contexts_mode(ContextsMode::Browse);
        if let Some(dialog) = self.contexts_dialog.as_ref() {
            dialog.focus.focus(window);
        }
        cx.notify();
    }

    /// Shows or hides the repository chooser of Bind Repository.
    fn toggle_adding_repository(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.contexts_dialog.as_mut() {
            dialog.adding = !dialog.adding;
        }
        cx.notify();
    }

    /// Browse… of Bind Repository: shows the native folder prompt, then adds the folder it returns
    /// to the selected context. The window does not wait for the prompt; the answer comes back
    /// later. A cancelled prompt does nothing; a folder that is not a repository is refused by the
    /// engine, and the refusal shows in the dialog.
    fn pick_context_repository(cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| match picked.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let text = path.display().to_string();
                    let _ = this.update(cx, |view, cx| view.add_repository_to_context(&text, cx));
                }
            }
            // Cancelled: nothing to do.
            Ok(Ok(None)) | Err(_) => {}
            Ok(Err(error)) => {
                let text = format!("Could not open the folder prompt: {error}");
                let _ = this.update(cx, |view, cx| view.notify_user(&text, cx));
            }
        })
        .detach();
    }

    /// Adds the repository in folder `path` to the selected context.
    fn add_repository_to_context(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(context) = self.model.contexts.selected() else {
            return;
        };
        let planned = self.model.contexts.add_repository(context, path);
        if let Some(dialog) = self.contexts_dialog.as_mut() {
            dialog.adding = false;
        }
        self.send_planned_contexts(planned, cx);
    }

    /// Removes repository `repo_id` from context `context`; its memories stay.
    fn remove_repository_from_context(
        &mut self,
        context: i64,
        repo_id: &str,
        cx: &mut Context<Self>,
    ) {
        let planned = self.model.contexts.remove_repository(context, repo_id);
        self.send_planned_contexts(planned, cx);
    }

    /// Give this clone a new id, for a clash with another clone.
    fn renew_contexts_repo_id(&mut self, cx: &mut Context<Self>) {
        let planned = self.model.contexts.renew_repo_id();
        self.send_planned_contexts(planned, cx);
    }

    /// Follows the answers the dialog waits for: a delete preview plans the delete, a write that
    /// changed the contexts closes its mode and reads the list again, and a failure is left to
    /// the model, which keeps its message for the dialog to show.
    pub(crate) fn follow_contexts(&mut self, event: &Event, cx: &mut Context<Self>) {
        let Some(pending) = self
            .contexts_dialog
            .as_ref()
            .and_then(|dialog| dialog.pending)
        else {
            return;
        };
        match event {
            Event::ContextDeletePreview {
                id,
                context,
                sub_contexts,
                memories,
                total,
            } if *id == pending => {
                let preview = DeletePreview {
                    sub_contexts: *sub_contexts,
                    total: *total,
                    titles: memories
                        .iter()
                        .map(|memory| memory_rows::snippet(&memory.text))
                        .collect(),
                };
                let planned = self.model.contexts.delete(*context, &preview);
                self.set_contexts_pending(None);
                match planned {
                    Ok(DeletePlan::Now(kind)) => {
                        let write = self.send_contexts(&kind);
                        self.set_contexts_pending(Some(write));
                    }
                    Ok(DeletePlan::Confirm(confirm)) => {
                        self.set_contexts_mode(ContextsMode::Delete(confirm));
                    }
                    Err(reason) => self.set_contexts_error(Some(reason.to_owned())),
                }
                cx.notify();
            }
            Event::ContextsChanged { id, .. } if *id == pending => {
                self.set_contexts_mode(ContextsMode::Browse);
                self.set_contexts_pending(None);
                if let Some(dialog) = self.contexts_dialog.as_mut() {
                    dialog.adding = false;
                }
                self.send_contexts(&ContextsModel::plan_list());
                cx.notify();
            }
            // A refused or failed write is shown by the model; the dialog only stops waiting.
            Event::OperationChanged { id, state, .. } if *id == pending && state.is_terminal() => {
                self.set_contexts_pending(None);
                cx.notify();
            }
            _ => {}
        }
    }

    /// The layer of the open dialog, or none while it is closed.
    pub(crate) fn contexts_layers(&self, window: &Window, cx: &Context<Self>) -> Vec<Div> {
        let Some(dialog) = self.contexts_dialog.as_ref() else {
            return Vec::new();
        };
        let outside = dialog_backdrop(cx, StatusView::close_contexts_dialog);
        let viewport = f32::from(window.viewport_size().width);
        let left = ((viewport - WIDTH) / 2.).max(0.);
        let panel = self.contexts_panel(dialog, left, cx);
        vec![div().absolute().inset_0().child(outside).child(panel)]
    }

    /// The dialog's panel: the title, the notes, the tree and the right pane, and the footer.
    /// Escape closes it; Enter saves a name.
    fn contexts_panel(&self, dialog: &ContextsDialog, left: f32, cx: &Context<Self>) -> Div {
        let view = cx.entity().downgrade();
        let problem = dialog
            .error
            .clone()
            .or_else(|| self.model.contexts.failure().map(str::to_owned));
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
                if event.keystroke.key == "escape" {
                    this.close_contexts_dialog(window, cx);
                }
            }))
            .capture_action(move |_: &Newline, _, cx| {
                let _ = view.update(cx, StatusView::contexts_enter);
                cx.stop_propagation();
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_color(theme::text())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Manage Contexts"),
                    )
                    .child(button(
                        "contexts-close",
                        "Close",
                        cx,
                        StatusView::close_contexts_dialog,
                    )),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::muted())
                    .child("A context is an area of knowledge. A sub-context inherits the memories of the contexts above it."),
            )
            .children(self.clash_notice(cx))
            .children(error_lines(problem.as_deref()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_4()
                    .child(self.contexts_tree(dialog, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(self.contexts_right(dialog, cx)),
                    ),
            )
            .child(self.contexts_footer(dialog, cx))
    }

    /// The notice of a clash: this clone shares its id with another, and the button gives it a
    /// new one. None when there is no clash.
    fn clash_notice(&self, cx: &Context<Self>) -> Option<Div> {
        let folder = self.model.contexts.clash()?;
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_sm().text_color(theme::error()).child(format!(
                    "This clone shares its repository id with {folder}, so its contexts are \
                         not used. Give this clone a new id to use them; it then belongs to no \
                         context."
                )))
                .child(div().child(button(
                    "contexts-renew-id",
                    "Give this clone a new id",
                    cx,
                    |this, _, cx| this.renew_contexts_repo_id(cx),
                ))),
        )
    }

    /// The tree: the contexts in the model's order, virtualized, each with its counts and its ⋯
    /// menu.
    fn contexts_tree(&self, dialog: &ContextsDialog, cx: &Context<Self>) -> Div {
        let count = self.model.contexts.rows().len();
        let selected = self.model.contexts.selected();
        let menu = dialog.menu;
        div()
            .w(px(TREE_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_xs().text_color(theme::muted()).child("CONTEXTS"))
            .children((!self.model.contexts.is_loaded()).then(|| {
                div()
                    .text_sm()
                    .text_color(theme::muted())
                    .child("Loading the contexts…")
            }))
            .child(
                // A flex column gives the list its height: `flex_1` in a plain block lays it out at
                // zero height, and no row paints (the Memories list uses the same shape).
                div()
                    .h(px(TREE_HEIGHT))
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .child(
                    uniform_list(
                        "contexts-tree",
                        count,
                        cx.processor(
                            move |this: &mut StatusView,
                                  range: Range<usize>,
                                  _window: &mut Window,
                                  cx: &mut Context<StatusView>| {
                                let rows = this.model.contexts.rows();
                                rows.get(range.clone())
                                    .unwrap_or_default()
                                    .iter()
                                    .zip(range)
                                    .map(|(row, index)| {
                                        StatusView::context_row(row, index, selected, menu, cx)
                                    })
                                    .collect()
                            },
                        ),
                    )
                    .track_scroll(dialog.tree.clone())
                    .flex_1()
                    .min_h(px(0.)),
                ),
            )
    }

    /// One row of the tree. Its ⋯ menu, when open, is the three actions on the row.
    fn context_row(
        row: &ContextRow,
        index: usize,
        selected: Option<i64>,
        menu: Option<i64>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let id = row.id;
        let on = selected == Some(id);
        let open = menu == Some(id);
        let indent = f32::from(u16::try_from(row.depth).unwrap_or(u16::MAX)) * INDENT;
        let mut line = div()
            .id(("context-row", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .pl(px(8. + indent))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(on, |line| line.bg(theme::selection()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_sm()
                    .child(row.name.clone()),
            )
            .children(row.bound.then(|| {
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme::accent())
                    .child("bound")
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.select_context(id, cx);
                }),
            );
        if open {
            line = line
                .child(context_menu_item(
                    ("context-rename", index),
                    "Rename",
                    cx,
                    move |this, window, cx| {
                        this.start_contexts_name(NameFor::Rename(id), window, cx);
                    },
                ))
                .child(context_menu_item(
                    ("context-move", index),
                    "Move…",
                    cx,
                    move |this, window, cx| this.open_contexts_move(id, window, cx),
                ))
                .child(context_menu_item(
                    ("context-delete", index),
                    "Delete",
                    cx,
                    move |this, _, cx| this.start_contexts_delete(id, cx),
                ));
        } else {
            line = line.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(counts_text(row.repo_count, row.accepted_memories)),
            );
        }
        line.child(
            div()
                .id(("context-more", index))
                .flex_none()
                .px_1()
                .text_color(theme::muted())
                .cursor_pointer()
                .child("⋯")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.toggle_contexts_menu(id, cx);
                    }),
                ),
        )
    }

    /// The right pane: the selected context's repositories, or the name, the picker or the
    /// confirmation the mode asks for.
    fn contexts_right(&self, dialog: &ContextsDialog, cx: &Context<Self>) -> Div {
        match &dialog.mode {
            ContextsMode::Browse => self.contexts_repositories(dialog, cx),
            ContextsMode::Name(target) => self.contexts_name(dialog, *target, cx),
            ContextsMode::Move(id) => self.contexts_move(dialog, *id, cx),
            ContextsMode::Delete(confirm) => self.contexts_delete(dialog, confirm, cx),
        }
    }

    /// The selected context and its repositories, each with Unbind, and Bind Repository.
    fn contexts_repositories(&self, dialog: &ContextsDialog, cx: &Context<Self>) -> Div {
        let Some(context) = self.model.contexts.selected_context() else {
            return div()
                .text_sm()
                .text_color(theme::muted())
                .child("Choose a context to see its repositories.");
        };
        let context_id = context.id;
        let rows: Vec<Stateful<Div>> = self
            .model
            .contexts
            .selected_repos()
            .iter()
            .enumerate()
            .map(|(index, repo)| {
                let repo_id = repo.repo_id.clone();
                div()
                    .id(("context-repo", index))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::text())
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(repo.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(repo.path.clone()),
                            ),
                    )
                    .child(context_menu_item(
                        ("context-repo-remove", index),
                        "Unbind",
                        cx,
                        move |this, _, cx| {
                            this.remove_repository_from_context(context_id, &repo_id, cx);
                        },
                    ))
            })
            .collect();
        let empty = rows.is_empty().then(|| {
            div()
                .text_sm()
                .text_color(theme::muted())
                .child("No repository is bound to this context.")
        });
        let chooser = dialog.adding.then(|| self.repository_chooser(cx));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(context.name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(context.path.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child("REPOSITORIES"),
            )
            .children(rows)
            .children(empty)
            .child(div().child(button(
                "contexts-add-repo",
                if dialog.adding {
                    "Hide the choices"
                } else {
                    "Bind Repository"
                },
                cx,
                |this, _, cx| this.toggle_adding_repository(cx),
            )))
            .children(chooser)
    }

    /// The choices of Bind Repository: Browse… first, then the window's own repository and the
    /// recent ones. Browse… is there even when the list is empty.
    fn repository_chooser(&self, cx: &Context<Self>) -> Stateful<Div> {
        let root = self.model.status().map(|status| status.root.clone());
        let choices = repo_choices(root.as_deref(), self.recents.paths());
        let browse = div()
            .id("context-repo-browse")
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .py_1()
            .text_sm()
            .text_color(theme::text())
            .child(icon::view(Icon::Folder, icon::SMALL, theme::muted()))
            .child("Browse…")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| {
                    Self::pick_context_repository(cx);
                }),
            );
        let rows: Vec<Stateful<Div>> = choices
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                let target = path.clone();
                div()
                    .id(("context-repo-choice", index))
                    .cursor_pointer()
                    .py_1()
                    .text_sm()
                    .text_color(theme::text())
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(path)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.add_repository_to_context(&target, cx);
                        }),
                    )
            })
            .collect();
        let empty = rows.is_empty().then(|| {
            div()
                .text_sm()
                .text_color(theme::muted())
                .child("No recent repository is known. Use Browse… to choose a folder.")
        });
        div()
            .id("context-repo-choices")
            .max_h(px(PICK_HEIGHT))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(theme::hairline())
            .pt_2()
            .child(browse)
            .children(rows)
            .children(empty)
    }

    /// The name box of New Context or Rename: the name, the sub-context choice, the error, Cancel
    /// and Save.
    fn contexts_name(&self, dialog: &ContextsDialog, target: NameFor, cx: &Context<Self>) -> Div {
        let pending = dialog.pending.is_some();
        let (title, save) = match target {
            NameFor::New { .. } => ("New Context", "Create"),
            NameFor::Rename(_) => ("Rename Context", "Rename"),
        };
        let sub_choice = match target {
            NameFor::New { under_selected } => {
                self.model.contexts.selected_context().map(|context| {
                    let label = if under_selected {
                        format!("As a sub-context of {}: yes", context.name)
                    } else {
                        format!("As a sub-context of {}: no", context.name)
                    };
                    button("contexts-under", label, cx, |this, _, cx| {
                        this.toggle_under_selected(cx);
                    })
                })
            }
            NameFor::Rename(_) => None,
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .children(sub_choice)
            .child(field_row("Name", dialog.name.clone()))
            .children(error_lines(dialog.error.as_deref()))
            .child(buttons(
                ("contexts-name-cancel", "contexts-name-save"),
                if pending { "Saving…" } else { save },
                false,
                pending,
                StatusView::cancel_contexts_mode,
                StatusView::confirm_contexts_name,
                cx,
            ))
    }

    /// The Move… picker: the filter, then the top level and each parent the model lets through,
    /// with Cancel.
    fn contexts_move(&self, dialog: &ContextsDialog, id: i64, cx: &Context<Self>) -> Div {
        let name = self
            .model
            .contexts
            .find(id)
            .map_or_else(String::new, |context| context.name.clone());
        let typed = dialog.filter.read(cx).text().to_owned();
        let rows: Vec<Stateful<Div>> = self
            .model
            .contexts
            .move_choices(id, &typed)
            .into_iter()
            .enumerate()
            .map(|(index, choice)| {
                let parent = choice.id;
                div()
                    .id(("context-move-row", index))
                    .cursor_pointer()
                    .py_1()
                    .text_sm()
                    .text_color(theme::text())
                    .child(choice.label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.move_context_to(parent, cx);
                        }),
                    )
            })
            .collect();
        let empty = rows.is_empty().then(|| {
            div()
                .text_sm()
                .text_color(theme::muted())
                .child("No parent matches the filter.")
        });
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Move {name} under")),
            )
            .child(field_row("Filter", dialog.filter.clone()))
            .child(
                div()
                    .id("context-move-list")
                    .max_h(px(PICK_HEIGHT))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows)
                    .children(empty),
            )
            .children(error_lines(dialog.error.as_deref()))
            .child(button(
                "contexts-move-cancel",
                "Cancel",
                cx,
                StatusView::cancel_contexts_mode,
            ))
    }

    /// The delete confirmation: the sub-contexts it refuses, the memories it removes (up to 50
    /// titles, then how many more), and the red button.
    fn contexts_delete(
        &self,
        dialog: &ContextsDialog,
        confirm: &DeleteConfirm,
        cx: &Context<Self>,
    ) -> Div {
        let pending = dialog.pending.is_some();
        let name = self
            .model
            .contexts
            .find(confirm.id)
            .map_or_else(String::new, |context| context.name.clone());
        let preview = &confirm.preview;
        let shown = preview.titles.len().min(TITLES_SHOWN);
        let more = usize::try_from(preview.total)
            .unwrap_or(usize::MAX)
            .saturating_sub(shown);
        let titles = preview.titles.iter().take(shown).map(|title| {
            div()
                .text_sm()
                .text_color(theme::text())
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(title.clone())
        });
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Delete {name}?")),
            )
            .child(div().text_sm().text_color(theme::text()).child(format!(
                "{} {} to no other context and {} deleted with it. This cannot be undone.",
                preview.total,
                if preview.total == 1 {
                    "memory belongs"
                } else {
                    "memories belong"
                },
                if preview.total == 1 { "is" } else { "are" },
            )))
            .children(titles)
            .children((more > 0).then(|| {
                div()
                    .text_sm()
                    .text_color(theme::muted())
                    .child(format!("… and {more} more"))
            }))
            .children(error_lines(dialog.error.as_deref()))
            .child(buttons(
                ("contexts-delete-cancel", "contexts-delete-go"),
                if pending {
                    "Deleting…"
                } else {
                    "Delete context"
                },
                true,
                pending,
                StatusView::cancel_contexts_mode,
                StatusView::confirm_contexts_delete,
                cx,
            ))
    }

    /// What the footer's left side says: a write or a read in flight, or the memories the last
    /// delete removed with its context.
    fn contexts_status(&self, dialog: &ContextsDialog) -> String {
        let deleted = self.model.contexts.deleted_memories();
        if dialog.pending.is_some() {
            "Working…".to_owned()
        } else if self.model.contexts.is_loading() {
            "Loading…".to_owned()
        } else if deleted > 0 {
            format!("The last delete removed {deleted} memories.")
        } else {
            String::new()
        }
    }

    /// The footer of the browsing dialog: New Context. The dialog's Close is in its title.
    fn contexts_footer(&self, dialog: &ContextsDialog, cx: &Context<Self>) -> Div {
        let browsing = dialog.mode == ContextsMode::Browse;
        div()
            .flex()
            .flex_row()
            .justify_between()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(self.contexts_status(dialog)),
            )
            .children(browsing.then(|| {
                button("contexts-new", "New Context", cx, |this, window, cx| {
                    let under = this.model.contexts.selected().is_some();
                    this.start_contexts_name(
                        NameFor::New {
                            under_selected: under,
                        },
                        window,
                        cx,
                    );
                })
            }))
    }
}

/// A small text action of a row: Rename, Move…, Remove, Delete. It stops the click from selecting
/// the row it sits on.
fn context_menu_item(
    id: (&'static str, usize),
    label: &'static str,
    cx: &Context<StatusView>,
    run: impl Fn(&mut StatusView, &mut Window, &mut Context<StatusView>) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px_1()
        .text_xs()
        .text_color(theme::text())
        .cursor_pointer()
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                run(this, window, cx);
            }),
        )
}
