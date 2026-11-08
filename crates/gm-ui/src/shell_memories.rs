//! The Memories tab of the Threads page (D190): the memories the repository sees on the left,
//! with a search box, filter chips and the bulk accepts above them; the selected memory on the
//! right, its text as plain text, its sources, its relations and its actions. The model is
//! `memories.rs`, and its labels and choices are in `memory_rows.rs`; this file draws them and
//! sends the reads and writes the model plans. The dialogs are in `memory_dialogs.rs`.

use std::ops::Range;
use std::time::Duration;

use gm_schema::threads::{Memory, MemoryBasis, MemoryStatus};
use gm_schema::{Actor, Event, OperationId, OperationState};
use gpui::{
    AnyElement, ClickEvent, Context, Div, Entity, Hsla, IntoElement, MouseButton, MouseDownEvent,
    ScrollStrategy, Stateful, Subscription, UniformListScrollHandle, Window, div, prelude::*, px,
    uniform_list,
};

use crate::action_registry::{Action, Args, MemoryEffect};
use crate::icon::{self, Icon};
use crate::memories::{self, MemoriesPage, Request, Scope};
use crate::memory_dialogs::PlaceAction;
use crate::memory_rows::{self, DetailAction, SourceTarget};
use crate::resize::{self};
use crate::resize_handle::{self, Edge};
use crate::shell_history::action_button;
use crate::text_area::{Newline, TextArea};
use crate::text_search::DEBOUNCE_MS;
use crate::theme;
use crate::view::{Screen, StatusView};
use crate::worktree_create::field_row;

/// The height of a memory's row in the list.
const MEMORY_ROW_HEIGHT: f32 = 64.;
/// The next page of memories is asked for when the rows drawn reach this close to the end.
const PREFETCH: usize = 20;
/// The message of a failed write when the engine gave none.
pub(crate) const NO_WRITE: &str = "The memory was not saved.";

/// The tab of the Threads page that shows (D190). It is kept while the page is open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ThreadsTab {
    #[default]
    Threads,
    Memories,
}

/// The Memories tab's own parts: its search box, its file box, the memory being edited and the
/// line that says why the last action was not done. Its state is `model.memories`.
pub(crate) struct MemoriesView {
    pub(crate) query: Entity<TextArea>,
    pub(crate) path: Entity<TextArea>,
    /// Whether the file box is shown under the chips.
    pub(crate) path_open: bool,
    pub(crate) list: UniformListScrollHandle,
    /// The proposal whose text is edited in the detail pane, while it is.
    pub(crate) edit: Option<MemoryEdit>,
    /// Why the last action was not done, as the model planned it.
    pub(crate) notice: Option<String>,
}

/// A proposal's text being edited in the detail pane, and the write of its accept once sent.
pub(crate) struct MemoryEdit {
    pub(crate) uid: String,
    pub(crate) input: Entity<TextArea>,
    pub(crate) error: Option<String>,
    pub(crate) pending: Option<OperationId>,
    _subscriptions: Vec<Subscription>,
}

impl MemoriesView {
    pub(crate) fn new(query: Entity<TextArea>, path: Entity<TextArea>) -> Self {
        Self {
            query,
            path,
            path_open: false,
            list: UniformListScrollHandle::new(),
            edit: None,
            notice: None,
        }
    }
}

/// The subscription that starts a delayed search when the box changes. Made with the window.
pub(crate) fn subscriptions(
    query: &Entity<TextArea>,
    cx: &mut Context<StatusView>,
) -> Vec<Subscription> {
    vec![cx.observe(query, |this, _, cx| this.on_memories_edited(cx))]
}

impl StatusView {
    /// Runs a memory action the registry planned (D190).
    pub(crate) fn run_memory_effect(
        &mut self,
        effect: MemoryEffect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match effect {
            MemoryEffect::Show => self.show_memories(window, cx),
            MemoryEffect::New => self.open_new_memory_dialog(window, cx),
            MemoryEffect::Accept(uid) => self.accept_memory(&uid, cx),
            MemoryEffect::Reject(uid) => self.reject_memory(&uid, cx),
            MemoryEffect::ManageContexts => self.open_contexts_dialog(window, cx),
            MemoryEffect::MoveToContext(uid) => {
                self.open_place_dialog(&uid, PlaceAction::Move, window, cx);
            }
            MemoryEffect::AttachToContext(uid) => {
                self.open_place_dialog(&uid, PlaceAction::Attach, window, cx);
            }
            MemoryEffect::DetachFromContext(uid) => self.open_detach_dialog(&uid, cx),
        }
    }

    /// Shows the Memories tab of the Threads page: the list and the contexts are read again, and
    /// the search box takes the cursor.
    pub(crate) fn show_memories(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Threads;
        self.threads_tab = ThreadsTab::Memories;
        self.model.commit_stack.close();
        self.model.stash_stack.close();
        self.model.reflog_stack.close();
        self.request_memories_refresh();
        self.memories_view
            .query
            .update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    /// Reads the memories again when the thread database changed and the tab has been read
    /// before. A tab not yet read is read when it is shown.
    pub(crate) fn follow_memories(&mut self, event: &Event, cx: &mut Context<Self>) {
        if matches!(event, Event::ThreadsChanged) && self.model.memories.is_loaded() {
            self.request_memories_refresh();
        }
        self.follow_memory_write(event, cx);
    }

    /// Follows the answers to the writes of the Memories tab: a saved edit leaves the detail pane,
    /// a refused one stays with its reason; the dialogs follow theirs the same way.
    pub(crate) fn follow_memory_write(&mut self, event: &Event, cx: &mut Context<Self>) {
        let pending = self
            .memories_view
            .edit
            .as_ref()
            .and_then(|edit| edit.pending);
        match (event, pending) {
            (Event::MemoryChanged { id, .. }, Some(pending)) if *id == pending => {
                self.memories_view.edit = None;
                self.memories_view.notice = None;
                cx.notify();
            }
            (
                Event::OperationChanged {
                    id, state, detail, ..
                },
                Some(pending),
            ) if *id == pending && state.is_terminal() => {
                if let Some(edit) = self.memories_view.edit.as_mut() {
                    edit.pending = None;
                    if *state == OperationState::Failed {
                        edit.error = Some(detail.clone().unwrap_or_else(|| NO_WRITE.to_owned()));
                    }
                }
                cx.notify();
            }
            _ => {}
        }
        self.follow_memory_dialog_write(event, cx);
    }

    /// Sends a planned read or write and records which one the answer belongs to. Returns the
    /// operation, so a caller can wait for its answer.
    pub(crate) fn send_memories(&mut self, request: Request) -> OperationId {
        let Request { kind, read } = request;
        let id = self.engine.dispatch(Actor::Human, kind);
        self.model.memories.sent(&read, id);
        id
    }

    /// Reads the first page of the list under the filters as they are, the contexts, and the
    /// selected memory again. Nothing is asked before a repository is open, or while one opens.
    pub(crate) fn request_memories_refresh(&mut self) {
        if self.model.status().is_none() || self.model.is_opening() {
            return;
        }
        self.request_memories_list();
        self.send_memories(MemoriesPage::plan_contexts());
        if let Some(uid) = self.model.memories.selected().map(str::to_owned) {
            self.send_memories(MemoriesPage::plan_read(&uid));
        }
    }

    /// Reads the first page of the list under the filters and the search as they are now.
    fn request_memories_list(&mut self) {
        let request = self.model.memories.plan_list();
        self.send_memories(request);
    }

    /// Asks for the page of memories after the rows shown, when one follows.
    fn request_memories_next(&mut self) {
        if let Some(request) = self.model.memories.plan_more() {
            self.send_memories(request);
        }
    }

    /// The count beside the tab's title: the memories shown, with a plus when more follow.
    pub(crate) fn memories_count_label(&self) -> String {
        let page = &self.model.memories;
        if !page.is_loaded() {
            return String::new();
        }
        memory_rows::count_label(page.memories().len(), page.has_more())
    }

    /// The box changed: the model takes the new text, and the search runs after a pause.
    fn on_memories_edited(&mut self, cx: &mut Context<Self>) {
        if self.screen != Screen::Threads || self.threads_tab != ThreadsTab::Memories {
            return;
        }
        let text = self.memories_view.query.read(cx).text().to_owned();
        if text.contains('\n') {
            // The box is one line; Enter is not a line break.
            let first = text.lines().next().unwrap_or("").to_owned();
            self.memories_view
                .query
                .update(cx, |area, cx| area.set_text(&first, cx));
            return;
        }
        let Some(generation) = self.model.memories.set_query(&text) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DEBOUNCE_MS))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.model.memories.is_latest(generation) {
                    view.run_memory_search(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Sends the list under the new search, and shows its first rows.
    fn run_memory_search(&mut self, cx: &mut Context<Self>) {
        self.request_memories_list();
        self.memories_view
            .list
            .scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Clears the search text (Esc). The list under the filters shows again.
    pub(crate) fn clear_memories_search(&mut self, cx: &mut Context<Self>) {
        let changed = self.model.memories.set_query("");
        self.memories_view
            .query
            .update(cx, |area, cx| area.set_text("", cx));
        self.apply_memory_filter(changed, cx);
    }

    /// A filter changed: the list is read again from its first page, and the rows shown stay until
    /// the answer comes. A filter that changed nothing asks for nothing.
    fn apply_memory_filter(&mut self, changed: Option<u64>, cx: &mut Context<Self>) {
        if changed.is_some() {
            self.request_memories_list();
            self.memories_view
                .list
                .scroll_to_item(0, ScrollStrategy::Top);
        }
        cx.notify();
    }

    /// The kind chip: the next kind, then any kind again.
    fn cycle_memory_kind(&mut self, cx: &mut Context<Self>) {
        let current = self.model.memories.kind();
        let next = memory_rows::next_choice(&memory_rows::KINDS, &current).unwrap_or(None);
        let changed = self.model.memories.set_kind(next);
        self.apply_memory_filter(changed, cx);
    }

    /// The status chip: the next status, then any status again.
    fn cycle_memory_status(&mut self, cx: &mut Context<Self>) {
        let current = self.model.memories.status();
        let next = memory_rows::next_choice(&memory_rows::STATUSES, &current).unwrap_or(None);
        let changed = self.model.memories.set_status(next);
        self.apply_memory_filter(changed, cx);
    }

    /// The author chip: the next origin the loaded memories hold, then any author again.
    fn cycle_memory_author(&mut self, cx: &mut Context<Self>) {
        let options = memory_rows::author_options(
            self.model.memories.memories(),
            self.model.memories.author(),
        );
        let current = self.model.memories.author().map(str::to_owned);
        let next = memory_rows::next_choice(&options, &current).unwrap_or(None);
        let changed = self.model.memories.set_author(next);
        self.apply_memory_filter(changed, cx);
    }

    /// The scope chip: everything, this repository, then each context the repository belongs to.
    fn cycle_memory_scope(&mut self, cx: &mut Context<Self>) {
        let options = memory_rows::scope_options(self.model.memories.contexts());
        let current = self.model.memories.scope().clone();
        let next = memory_rows::next_choice(&options, &current).unwrap_or(Scope::Everything);
        let changed = self.model.memories.set_scope(next);
        self.apply_memory_filter(changed, cx);
    }

    /// The file chip: clears the file filter when one is set, else shows the file box.
    fn toggle_memory_path(&mut self, cx: &mut Context<Self>) {
        if self.model.memories.path().is_some() {
            let changed = self.model.memories.set_path(None);
            self.apply_memory_filter(changed, cx);
        } else {
            self.memories_view.path_open = !self.memories_view.path_open;
            cx.notify();
        }
    }

    /// Enter in the file box: the memories naming that file, or a path under it, are listed.
    fn apply_memory_path(&mut self, cx: &mut Context<Self>) {
        let text = self.memories_view.path.read(cx).text().to_owned();
        let changed = self
            .model
            .memories
            .set_path(memory_rows::path_filter(&text));
        self.apply_memory_filter(changed, cx);
    }

    /// Selects memory `uid` and reads its detail. An edit of another memory is dropped.
    pub(crate) fn choose_memory(&mut self, uid: &str, cx: &mut Context<Self>) {
        if self
            .memories_view
            .edit
            .as_ref()
            .is_some_and(|edit| edit.uid != uid)
        {
            self.memories_view.edit = None;
        }
        let request = self.model.memories.select(uid);
        self.send_memories(request);
        cx.notify();
    }

    /// Moves the selection one row (Up and Down) and keeps the row in view.
    pub(crate) fn step_memory(&mut self, delta: isize, cx: &mut Context<Self>) {
        let memories = self.model.memories.memories();
        let count = memories.len();
        if count == 0 {
            return;
        }
        let current = self
            .model
            .memories
            .selected()
            .and_then(|uid| memories.iter().position(|memory| memory.uid == uid));
        let next = match current {
            Some(index) => index.saturating_add_signed(delta).min(count - 1),
            None => 0,
        };
        let uid = memories[next].uid.clone();
        self.choose_memory(&uid, cx);
        self.memories_view
            .list
            .scroll_to_item(next, ScrollStrategy::Center);
    }

    /// Chooses or unchooses a proposal for a bulk accept.
    fn toggle_memory_chosen(&mut self, uid: &str, cx: &mut Context<Self>) {
        self.model.memories.toggle_chosen(uid);
        cx.notify();
    }

    /// Sends the accept or the reject the model planned, or keeps its reason on the tab.
    fn send_planned_memory(
        &mut self,
        planned: Result<Request, &'static str>,
        cx: &mut Context<Self>,
    ) {
        match planned {
            Ok(request) => {
                self.memories_view.notice = None;
                self.send_memories(request);
            }
            Err(reason) => self.memories_view.notice = Some(reason.to_owned()),
        }
        cx.notify();
    }

    /// Accepts proposal `uid` as it is (the registry's Accept Memory and the button).
    pub(crate) fn accept_memory(&mut self, uid: &str, cx: &mut Context<Self>) {
        let planned = self.model.memories.plan_accept(uid);
        self.send_planned_memory(planned, cx);
    }

    /// Rejects proposal `uid` (the registry's Reject Memory and the button).
    pub(crate) fn reject_memory(&mut self, uid: &str, cx: &mut Context<Self>) {
        let planned = self.model.memories.plan_reject(uid);
        self.send_planned_memory(planned, cx);
    }

    /// Accepts every chosen proposal, one write each.
    fn accept_selected(&mut self, cx: &mut Context<Self>) {
        match self.model.memories.plan_bulk_accept() {
            Ok(requests) => {
                self.memories_view.notice = None;
                for request in requests {
                    self.send_memories(request);
                }
            }
            Err(reason) => self.memories_view.notice = Some(reason.to_owned()),
        }
        cx.notify();
    }

    /// Accepts every proposal from the author the filter names, when the list shows them all.
    fn accept_author(&mut self, cx: &mut Context<Self>) {
        let author = self.model.memories.author().unwrap_or_default().to_owned();
        match self.model.memories.plan_accept_author(&author) {
            Ok(requests) => {
                self.memories_view.notice = None;
                for request in requests {
                    self.send_memories(request);
                }
            }
            Err(reason) => self.memories_view.notice = Some(reason.to_owned()),
        }
        cx.notify();
    }

    /// Opens the detail pane's editor on the selected proposal, with its text.
    pub(crate) fn start_memory_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(memory) = self.model.memories.selected_memory() else {
            return;
        };
        if memory.status != MemoryStatus::Proposed {
            self.memories_view.notice = Some("Only a proposed memory can be edited.".to_owned());
            cx.notify();
            return;
        }
        let uid = memory.uid.clone();
        let text = memory.text.clone();
        let input = cx.new(|cx| TextArea::new("The memory text", cx).with_lines(4, 12));
        input.update(cx, |area, cx| area.set_text(&text, cx));
        input.update(cx, |area, cx| area.focus(window, cx));
        let subscriptions = vec![cx.observe(&input, |_, _, cx| cx.notify())];
        self.memories_view.edit = Some(MemoryEdit {
            uid,
            input,
            error: None,
            pending: None,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    /// Accepts the edited text: the model checks it, and the edit leaves the pane once the engine
    /// answers. A refusal stays in the editor with its reason.
    fn save_memory_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.memories_view.edit.as_ref() else {
            return;
        };
        if edit.pending.is_some() {
            return;
        }
        let uid = edit.uid.clone();
        let text = edit.input.read(cx).text().to_owned();
        match self.model.memories.plan_accept_edited(&uid, &text) {
            Ok(request) => {
                let id = self.send_memories(request);
                if let Some(edit) = self.memories_view.edit.as_mut() {
                    edit.pending = Some(id);
                    edit.error = None;
                }
            }
            Err(reason) => {
                if let Some(edit) = self.memories_view.edit.as_mut() {
                    edit.error = Some(reason.to_owned());
                }
            }
        }
        cx.notify();
    }

    /// Leaves the editor without a change.
    fn cancel_memory_edit(&mut self, cx: &mut Context<Self>) {
        self.memories_view.edit = None;
        cx.notify();
    }

    /// Opens source `index` of the selected memory: a thread at its entries, a commit in History,
    /// a file in the Code page. A source of another repository is not opened.
    pub(crate) fn open_memory_source(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self
            .model
            .memories
            .selected_memory()
            .and_then(|memory| memory.sources.get(index))
            .map(|source| memory_rows::source_target(source, self.model.memories.repo_id()));
        match target {
            Some(SourceTarget::Thread { uid, from_seq }) => {
                self.open_thread_from_memory(&uid, from_seq, window, cx);
            }
            Some(SourceTarget::Commit(oid)) => {
                self.show_history(cx);
                self.select_commit(&oid, cx);
            }
            Some(SourceTarget::File(path)) => self.view_file(&path, None, cx),
            Some(SourceTarget::OtherRepository) => {
                self.notify_user(
                    "This source is in another repository, which is not open.",
                    cx,
                );
            }
            None => {}
        }
    }

    /// The toolbar under the tab strip: the search box, the filter chips, and the file box when it
    /// is open.
    pub(crate) fn memories_toolbar(&self, cx: &mut Context<Self>) -> Div {
        let file_box = (self.memories_view.path_open || self.model.memories.path().is_some())
            .then(|| self.memories_path_box(cx));
        div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .h(px(40.))
                    .px_4()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(icon::view(
                        Icon::MagnifyingGlass,
                        icon::SMALL,
                        theme::muted(),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(self.memories_view.query.clone()),
                    ),
            )
            .child(self.memory_filter_chips(cx))
            .children(file_box)
    }

    /// The filter chips: kind, status, author, file and scope. Each one cycles its choices, and
    /// names its filter, or Any.
    fn memory_filter_chips(&self, cx: &Context<Self>) -> Div {
        let page = &self.model.memories;
        let scope_on = *page.scope() != Scope::Everything;
        let path = page.path().map(str::to_owned);
        let kind = page
            .kind()
            .map(|kind| memories::kind_label(kind).to_owned());
        let status = page
            .status()
            .map(|status| memories::status_label(status).to_owned());
        let file_text = match &path {
            Some(path) => memory_rows::chip_text("File", Some(path.clone())),
            None => "File…".to_owned(),
        };
        div()
            .px_4()
            .pb_2()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(filter_chip(
                "memories-chip-kind",
                memory_rows::chip_text("Kind", kind.clone()),
                kind.is_some(),
                cx,
                StatusView::cycle_memory_kind,
            ))
            .child(filter_chip(
                "memories-chip-status",
                memory_rows::chip_text("Status", status.clone()),
                status.is_some(),
                cx,
                StatusView::cycle_memory_status,
            ))
            .child(filter_chip(
                "memories-chip-author",
                memory_rows::chip_text("Author", page.author().map(memories::origin_label)),
                page.author().is_some(),
                cx,
                StatusView::cycle_memory_author,
            ))
            .child(filter_chip(
                "memories-chip-file",
                file_text,
                path.is_some(),
                cx,
                StatusView::toggle_memory_path,
            ))
            .child(filter_chip(
                "memories-chip-scope",
                memory_rows::chip_text(
                    "Scope",
                    scope_on.then(|| memory_rows::scope_label(page.scope())),
                ),
                scope_on,
                cx,
                StatusView::cycle_memory_scope,
            ))
    }

    /// The file box under the chips. Enter applies the file; the box is one line, so the key never
    /// reaches the text.
    fn memories_path_box(&self, cx: &mut Context<Self>) -> Div {
        let enter = cx.entity().downgrade();
        div()
            .px_4()
            .pb_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(self.memories_view.path.clone()),
            )
            .capture_action(move |_: &Newline, _, cx| {
                let _ = enter.update(cx, StatusView::apply_memory_path);
                cx.stop_propagation();
            })
    }

    /// The tab's body: the list on the left, the selected memory on the right.
    pub(crate) fn memories_body(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_row()
            .child(self.memories_list_column(cx))
            .child(self.memory_detail_column(cx))
    }

    /// The left column: the notes on the last read and action, the bulk accepts, then the rows, or
    /// what says there are none.
    fn memories_list_column(&self, cx: &mut Context<Self>) -> Div {
        let page = &self.model.memories;
        let mut notes = Vec::new();
        if let Some(text) = page.failure() {
            notes.push(status_line(text, theme::error()));
        }
        if let Some(text) = self.memories_view.notice.as_deref() {
            notes.push(status_line(text, theme::muted()));
        }
        if page.is_loading_list() && page.is_loaded() {
            notes.push(status_line("Updating…", theme::muted()));
        }
        if let Some(text) = page.note() {
            notes.push(status_line(text, theme::warn_fg()));
        }
        if page.is_loaded() && !page.contexts_read() && page.note().is_none() {
            notes.push(status_line(
                "The contexts could not be read; only this repository is shown.",
                theme::warn_fg(),
            ));
        }
        if !page.is_loaded() {
            let loading = if page.failure().is_some() {
                "The memories could not be read."
            } else {
                "Loading memories…"
            };
            let bulk = self.memories_bulk_row(cx);
            return self.memories_column(notes, bulk, note_line(loading), cx);
        }
        if page.memories().is_empty() {
            let bulk = self.memories_bulk_row(cx);
            return self.memories_column(notes, bulk, note_line(&empty_text(page)), cx);
        }
        self.memories_list_with_rows(notes, cx)
    }

    /// The column with the rows of the list, the bulk accepts above them and the resize strip.
    fn memories_list_with_rows(&self, notes: Vec<Div>, cx: &mut Context<Self>) -> Div {
        let count = self.model.memories.memories().len();
        let rows = div().flex_1().min_h(px(0.)).flex().flex_col().child(
            uniform_list(
                "memory-rows",
                count,
                cx.processor(
                    move |this: &mut StatusView,
                          range: Range<usize>,
                          _window: &mut Window,
                          cx: &mut Context<StatusView>| {
                        if range.end + PREFETCH >= count {
                            this.request_memories_next();
                        }
                        let page = &this.model.memories;
                        let selected = page.selected();
                        page.memories()
                            .get(range.clone())
                            .unwrap_or_default()
                            .iter()
                            .zip(range)
                            .map(|(memory, index)| {
                                let is_selected = selected == Some(memory.uid.as_str());
                                let chosen = page.is_chosen(&memory.uid);
                                memory_row(memory, index, is_selected, chosen, cx)
                            })
                            .collect()
                    },
                ),
            )
            .track_scroll(self.memories_view.list.clone())
            .flex_1()
            .min_h(px(0.)),
        );
        let bulk = self.memories_bulk_row(cx);
        self.memories_column(notes, bulk, rows, cx)
    }

    /// The frame of the left column: the notes, the bulk row, the body and the resize strip. The
    /// strip resizes the same column as the threads' list.
    fn memories_column(
        &self,
        notes: Vec<Div>,
        bulk: Option<Div>,
        body: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> Div {
        let width = self.thread_list_width;
        let width_strip = resize_handle::handle(
            "memory-list-resize",
            Edge::Right,
            width,
            resize::PANE,
            |this: &mut StatusView, width, _| this.thread_list_width = width,
            |this: &mut StatusView, _| this.thread_list_width = resize::THREAD_LIST_DEFAULT,
            cx,
        );
        div()
            .relative()
            .w(px(width))
            .flex_none()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::hairline())
            .children(notes)
            .children(bulk)
            .child(body)
            .child(width_strip)
    }

    /// The bulk row above the rows: Accept selected when proposals are chosen, Accept all from the
    /// author the filter names, and a line when no proposal is waiting.
    fn memories_bulk_row(&self, cx: &mut Context<Self>) -> Option<Div> {
        let page = &self.model.memories;
        let selected = memory_rows::accept_selected_label(page.chosen_count());
        let author = page.author().map(str::to_owned);
        let waiting = page.is_loaded() && page.proposed_count() == 0 && !page.memories().is_empty();
        if selected.is_none() && author.is_none() && !waiting {
            return None;
        }
        let row = div()
            .px_4()
            .py_1()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .text_xs()
            .children(selected.map(|label| {
                button("memories-accept-selected", label, cx, |this, _, cx| {
                    this.accept_selected(cx);
                })
            }))
            .children(author.map(|author| {
                button(
                    "memories-accept-author",
                    memory_rows::accept_author_label(&author),
                    cx,
                    |this, _, cx| this.accept_author(cx),
                )
            }))
            .children(waiting.then(|| {
                div()
                    .text_color(theme::muted())
                    .child("No proposal is waiting for review.")
            }));
        Some(row)
    }

    /// The right column: the selected memory, or what the tab is waiting for.
    fn memory_detail_column(&self, cx: &mut Context<Self>) -> Div {
        let body: AnyElement = match self.model.memories.selected_memory() {
            None => note_line("Select a memory to read it.").into_any_element(),
            Some(memory) => self.memory_detail(memory, cx).into_any_element(),
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(body)
    }

    /// The detail of a memory: its kind, status, origin and scope; its actions; its text as plain
    /// text; the files that changed since it was accepted; its sources and its relations.
    fn memory_detail(&self, memory: &Memory, cx: &mut Context<Self>) -> Stateful<Div> {
        let page = &self.model.memories;
        let text = match self
            .memories_view
            .edit
            .as_ref()
            .filter(|edit| edit.uid == memory.uid)
        {
            Some(edit) => Self::memory_edit_panel(edit, cx),
            None => plain_text(&memory.text),
        };
        let loading = (page.is_loading_detail() && page.detail().is_none())
            .then(|| note_line("Loading the sources…"));
        div()
            .id("memory-detail")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(memory_header(memory))
            .child(self.detail_buttons(memory, cx))
            .child(text)
            .children(stale_lines(memory))
            .children(loading)
            .child(self.memory_sources(memory, cx))
            .children(self.relation_section(
                "Supersedes",
                &memory.supersedes,
                "memory-supersedes-row",
                cx,
            ))
            .children(self.relation_section(
                "Superseded by",
                &memory.superseded_by,
                "memory-superseded-by-row",
                cx,
            ))
    }

    /// The actions the memory shown offers, as its state gives them (D190).
    fn detail_buttons(&self, memory: &Memory, cx: &mut Context<Self>) -> Div {
        let ctx = self.action_context("");
        let target = Args::Memory {
            uid: memory.uid.clone(),
            proposed: memory.status == MemoryStatus::Proposed,
        };
        let mut row = div().flex().flex_row().flex_wrap().items_center().gap_2();
        for action in memory_rows::detail_actions(Some(memory)) {
            row = row.child(match action {
                DetailAction::Accept => action_button(
                    Action::AcceptMemory,
                    "memory-accept",
                    "Accept",
                    &target,
                    &ctx,
                    cx,
                ),
                DetailAction::Reject => action_button(
                    Action::RejectMemory,
                    "memory-reject",
                    "Reject",
                    &target,
                    &ctx,
                    cx,
                ),
                DetailAction::Edit => button("memory-edit", "Edit", cx, |this, window, cx| {
                    this.start_memory_edit(window, cx);
                }),
                DetailAction::Supersede => {
                    button("memory-supersede", "Supersedes…", cx, |this, _, cx| {
                        this.open_supersede_dialog(cx);
                    })
                }
                DetailAction::MoveToContext => action_button(
                    Action::MoveMemoryToContext,
                    "memory-move-context",
                    "Move to Context…",
                    &target,
                    &ctx,
                    cx,
                ),
                DetailAction::AttachToContext => action_button(
                    Action::AttachMemoryToContext,
                    "memory-attach-context",
                    "Attach to Context…",
                    &target,
                    &ctx,
                    cx,
                ),
                DetailAction::DetachFromContext => action_button(
                    Action::DetachMemoryFromContext,
                    "memory-detach-context",
                    "Detach from Context…",
                    &target,
                    &ctx,
                    cx,
                ),
                DetailAction::Delete => button("memory-delete", "Delete…", cx, |this, _, cx| {
                    this.open_delete_memory_dialog(cx);
                }),
            });
        }
        row
    }

    /// The text of the memory being edited: an editor, the characters left, the error, and the
    /// buttons that accept the edit or leave it.
    fn memory_edit_panel(edit: &MemoryEdit, cx: &mut Context<Self>) -> Div {
        let chars = edit.input.read(cx).text().chars().count();
        let over = memory_rows::over_limit(chars);
        let pending = edit.pending.is_some();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(field_row("Memory text", edit.input.clone()))
            .child(
                div()
                    .text_xs()
                    .text_color(if over { theme::error() } else { theme::muted() })
                    .child(memory_rows::remaining_text(chars)),
            )
            .children(
                edit.error
                    .as_deref()
                    .map(|text| status_line(text, theme::error())),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(button(
                        "memory-edit-save",
                        if pending {
                            "Saving…"
                        } else {
                            "Accept edited"
                        },
                        cx,
                        |this, _, cx| {
                            this.save_memory_edit(cx);
                        },
                    ))
                    .child(button("memory-edit-cancel", "Cancel", cx, |this, _, cx| {
                        this.cancel_memory_edit(cx);
                    })),
            )
    }

    /// The sources of a memory, each with its label, the note when it is in another repository,
    /// and an Open link when it opens here.
    fn memory_sources(&self, memory: &Memory, cx: &mut Context<Self>) -> Div {
        let repo = self.model.memories.repo_id();
        let rows: Vec<Div> = memory
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let target = memory_rows::source_target(source, repo);
                let openable = !matches!(target, SourceTarget::OtherRepository);
                let mut row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_w(px(0.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(memory_rows::source_label(source)),
                    );
                if let Some(text) = memory_rows::source_note(&target) {
                    row = row.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(text),
                    );
                }
                if openable {
                    row = row.child(
                        div()
                            .id(("memory-source-open", index))
                            .cursor_pointer()
                            .flex_none()
                            .text_xs()
                            .text_color(theme::accent())
                            .child("Open")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    this.open_memory_source(index, window, cx);
                                }),
                            ),
                    );
                }
                row
            })
            .collect();
        section(
            "Sources",
            rows,
            memory.sources.is_empty().then_some("No sources."),
        )
    }

    /// A relation of the memory: the memories it supersedes, or that supersede it. Each names
    /// its memory, and a click shows that memory.
    fn relation_section(
        &self,
        title: &'static str,
        uids: &[String],
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if uids.is_empty() {
            return None;
        }
        let rows: Vec<Stateful<Div>> = uids
            .iter()
            .enumerate()
            .map(|(index, uid)| {
                let label = self.model.memories.find(uid).map_or_else(
                    || format!("Memory {}", uid.chars().take(8).collect::<String>()),
                    |memory| memory_rows::snippet(&memory.text),
                );
                let target = uid.clone();
                div()
                    .id((id, index))
                    .cursor_pointer()
                    .text_sm()
                    .text_color(theme::accent())
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.choose_memory(&target, cx);
                        }),
                    )
            })
            .collect();
        Some(section(title, rows, None))
    }
}

/// A memory's row: its first line, a box to choose a proposal, and its meta line with the origin,
/// the Agent-written flag, its scope and the flag of a changed file. A click selects it.
fn memory_row(
    memory: &Memory,
    index: usize,
    selected: bool,
    chosen: bool,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let uid = memory.uid.clone();
    let mut head = div().flex().flex_row().items_center().gap_2().min_w(px(0.));
    if memory.status == MemoryStatus::Proposed {
        let toggle = uid.clone();
        head = head.child(
            div()
                .id(("memory-check", index))
                .cursor_pointer()
                .flex_none()
                .w(px(14.))
                .h(px(14.))
                .rounded_sm()
                .border_1()
                .border_color(theme::button_border())
                .when(chosen, |check| check.bg(theme::accent()))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    // The box is not a click on the row: the selection stays as it was.
                    cx.stop_propagation();
                    this.toggle_memory_chosen(&toggle, cx);
                })),
        );
    }
    let head = head.child(
        div()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(memory_rows::snippet(&memory.text)),
    );
    let meta = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .text_xs()
        .text_color(theme::muted())
        .child(
            div()
                .flex_none()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(memories::row_label(memory)),
        )
        .children(
            memories::agent_written(&memory.origin)
                .then(|| tag("Agent-written", theme::selected_row())),
        )
        .children(basis_tag(memory.basis))
        .child(tag(
            &memories::scope_chip(&memory.scope),
            theme::selected_row(),
        ))
        .children(memory_rows::is_stale(memory).then(|| tag("Files changed", theme::warn_bg())));
    div()
        .id(("memory-row", index))
        .cursor_pointer()
        .w_full()
        .h(px(MEMORY_ROW_HEIGHT))
        .px_4()
        .flex()
        .flex_col()
        .justify_center()
        .gap_1()
        .border_b_1()
        .border_color(theme::hairline())
        .when(selected, |row| row.bg(theme::selected_row()))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.choose_memory(&uid, cx);
        }))
        .child(head)
        .child(meta)
}

/// The header of the memory shown: its kind, status and origin, its origin flag and its scope.
fn memory_header(memory: &Memory) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_color(theme::muted())
                .text_xs()
                .child(memories::row_label(memory)),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_1()
                .children(
                    memories::agent_written(&memory.origin)
                        .then(|| tag("Agent-written", theme::selected_row())),
                )
                .children(basis_tag(memory.basis))
                .child(tag(
                    &memories::scope_chip(&memory.scope),
                    theme::selected_row(),
                )),
        )
}

/// The chip of a memory's basis, when it shows one (D201 (1)): a tested memory on the selection
/// colour, an unconfirmed one flagged in the warning colour, as a changed file is.
fn basis_tag(basis: MemoryBasis) -> Option<Div> {
    let text = memory_rows::basis_chip(basis)?;
    let background = if basis == MemoryBasis::Unconfirmed {
        theme::warn_bg()
    } else {
        theme::selected_row()
    };
    Some(tag(text, background))
}

/// The text the empty list says, by why it is empty.
fn empty_text(page: &MemoriesPage) -> String {
    if page.failure().is_some() {
        "The memories could not be read.".to_owned()
    } else if page.filter_active() {
        "No memory matches these filters.".to_owned()
    } else {
        "No memories yet. Propose one from a thread with /gitmaster:propose-memories, or choose New Memory."
            .to_owned()
    }
}

/// A plain line of the page, in the muted colour.
fn note_line(text: &str) -> Div {
    div()
        .px_4()
        .py_3()
        .text_sm()
        .text_color(theme::muted())
        .child(text.to_owned())
}

/// A line that says why something is not done, in a colour.
fn status_line(text: &str, colour: Hsla) -> Div {
    div()
        .px_4()
        .py_1()
        .text_sm()
        .text_color(colour)
        .child(text.to_owned())
}

/// A small chip: a label on the selection colour, or a flag on another.
pub(crate) fn tag(text: &str, background: Hsla) -> Div {
    div()
        .flex_none()
        .px_1()
        .rounded_sm()
        .bg(background)
        .child(text.to_owned())
}

/// A filter chip: outlined when it names no filter, filled in the accent colour when it does.
pub(crate) fn filter_chip(
    id: &'static str,
    text: String,
    on: bool,
    cx: &Context<StatusView>,
    click: fn(&mut StatusView, &mut Context<StatusView>),
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .flex_none()
        .px_2()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded_full()
        .border_1()
        .text_xs()
        .when(on, |chip| {
            chip.bg(theme::selection())
                .border_color(theme::accent())
                .text_color(theme::accent())
        })
        .when(!on, |chip| {
            chip.border_color(theme::button_border())
                .text_color(theme::muted())
        })
        .child(text)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| click(this, cx)),
        )
}

/// A text button of the page. Its click gets the window, for the dialogs and the editor to focus.
pub(crate) fn button(
    id: &'static str,
    label: impl IntoElement,
    cx: &Context<StatusView>,
    click: impl Fn(&mut StatusView, &mut Window, &mut Context<StatusView>) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .flex_none()
        .px_2()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(theme::button_border())
        .text_xs()
        .text_color(theme::text())
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                click(this, window, cx);
            }),
        )
}

/// A titled part of the detail: its title, its rows, or the note when it has none.
fn section<E: IntoElement>(title: &'static str, rows: Vec<E>, empty: Option<&'static str>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(theme::muted()).child(title))
        .children(rows)
        .children(empty.map(|text| div().text_sm().text_color(theme::muted()).child(text)))
}

/// The memory's text as plain text: one line of the page per line of the text. It is never read as
/// Markdown or HTML, since it is untrusted.
fn plain_text(text: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .min_w(px(0.))
        .text_color(theme::text())
        .children(
            text.lines()
                .map(|line| div().min_h(px(22.)).child(line.to_owned())),
        )
}

/// The lines saying which files a memory names changed since it was accepted. They are coloured as
/// a warning when the memory may be stale.
fn stale_lines(memory: &Memory) -> Vec<Div> {
    let changes = memories::files_changed(&memory.files_changed_since);
    let colour = if changes.is_stale() {
        theme::warn_fg()
    } else {
        theme::muted()
    };
    changes
        .lines()
        .into_iter()
        .map(|line| div().text_sm().text_color(colour).child(line))
        .collect()
}

/// The dialogs take a form's choice as a chip: the chosen one is filled.
pub(crate) fn pick_chip(
    id: (&'static str, usize),
    label: String,
    on: bool,
    cx: &Context<StatusView>,
    pick: impl Fn(&mut StatusView, &mut Context<StatusView>) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .flex_1()
        .py_1()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .text_sm()
        .when(on, |chip| {
            chip.bg(theme::selection())
                .border_color(theme::accent())
                .text_color(theme::accent())
        })
        .when(!on, |chip| {
            chip.border_color(theme::hairline())
                .text_color(theme::muted())
        })
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| pick(this, cx)),
        )
}
