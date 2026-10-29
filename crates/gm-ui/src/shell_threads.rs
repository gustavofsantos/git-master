//! The Threads page (T5, D119, D125): the threads of the repository on the left,
//! with a search box and two filter chips above them; the selected thread on the
//! right, grouped by turn. The model is `threads.rs`; this file only draws it and
//! sends the reads it plans. A search runs 250 ms after the last keystroke, Up and
//! Down move the selection, and Esc clears the search.

use std::ops::Range;
use std::time::Duration;

use gm_schema::{
    Actor, CommandKind, Event, LinkKind, LinkOrigin, ThreadDetail, ThreadHit, ThreadLink,
    ThreadSummary,
};
use gpui::{
    AnyElement, ClickEvent, ClipboardItem, Context, Div, Entity, FontWeight, IntoElement,
    KeyDownEvent, ListAlignment, ListOffset, ListState, MouseButton, MouseDownEvent,
    ScrollStrategy, Stateful, Subscription, UniformListScrollHandle, Window, div, list, prelude::*,
    px, uniform_list,
};

use crate::action_registry::{Action, Args, ThreadRef, thread_args};
use crate::commit_threads;
use crate::icon::{self, Icon};
use crate::shell_history::{action_button, now_secs};
use crate::text_area::TextArea;
use crate::text_search::DEBOUNCE_MS;
use crate::theme;
use crate::thread_curate as curate;
use crate::thread_rows::{self, GroupKey, Marker, Row};
use crate::threads::{self, CallResult, CallStatus, Chip, Empty, MessageKind, Mode, Request, Span};
use crate::view::{Screen, StatusView};

const THREAD_ROW_HEIGHT: f32 = 64.;
const HIT_ROW_HEIGHT: f32 = 56.;
const LIST_WIDTH: f32 = 360.;
const BRANCH_WIDTH: f32 = 120.;
/// The next page of threads is asked for when the rows drawn reach this close to the end.
const PREFETCH: usize = 20;
/// Longest text shown in full in a message, before the rest is counted.
const MESSAGE_LINES: usize = 400;
/// Longest result or argument shown when a call is open.
const RESULT_LINES: usize = 200;
const ARG_LINES: usize = 40;
/// Most lines of a patch drawn in an edit card.
const PATCH_LINES: usize = 400;
/// How far, in pixels, a nested row is pushed in from its parent.
const INDENT: f32 = 16.;
/// The rows the detail list draws beyond the visible ones, so scrolling does not show a gap.
const DETAIL_OVERDRAW: f32 = 480.;
const INSTALL_HINT_TEXT: &str = threads::INSTALL_HINT;

/// The window's part of the page: the search box and the lists' scroll. Its state is `model.threads`.
pub(crate) struct ThreadsView {
    pub(crate) query: Entity<TextArea>,
    pub(crate) list: UniformListScrollHandle,
    /// The rows of the thread shown. Its items are the rows the model holds, measured as drawn.
    pub(crate) rows: ListState,
    /// The row hashes the list holds, which the model's rows are compared with.
    pub(crate) shown: Vec<u64>,
    /// The search hit whose row the list was last scrolled to.
    pub(crate) focus_scrolled: Option<i64>,
}

impl ThreadsView {
    pub(crate) fn new(query: Entity<TextArea>) -> Self {
        Self {
            query,
            list: UniformListScrollHandle::new(),
            rows: ListState::new(0, ListAlignment::Top, px(DETAIL_OVERDRAW)),
            shown: Vec::new(),
            focus_scrolled: None,
        }
    }
}

/// The subscription that starts a delayed search when the box changes. Made with the window.
pub(crate) fn subscriptions(
    query: &Entity<TextArea>,
    cx: &mut Context<StatusView>,
) -> Vec<Subscription> {
    vec![cx.observe(query, |this, _, cx| this.on_threads_edited(cx))]
}

impl StatusView {
    /// Shows the page: the box takes the cursor and the list is read again.
    pub(crate) fn show_threads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Threads;
        self.model.commit_stack.close();
        self.model.stash_stack.close();
        self.model.reflog_stack.close();
        self.request_threads_refresh();
        self.threads_view
            .query
            .update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    /// Follows the engine for the Threads page: a changed thread database reads the
    /// list again, a status refresh does too while the page is shown, and a read
    /// thread asks for its edits' states (T5).
    pub(crate) fn follow_threads(
        &mut self,
        (changed, read, listed): (bool, bool, bool),
        reload: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(outcome) = self.model.thread_copy.take() {
            self.finish_thread_copy(outcome, cx);
        }
        if changed
            || (reload && self.screen == Screen::Threads)
            || (reload && !self.model.threads.is_loaded())
        {
            self.request_threads_refresh();
            // A commit's threads are read again too, so a link made by the agent shows.
            if let Some(oid) = self.model.commit_threads.oid().map(str::to_owned) {
                self.request_commit_threads(&oid);
            }
        }
        // A page of the list says whether the selected thread's entries changed.
        if listed && let Some(request) = self.model.threads.plan_tail() {
            self.send_threads(request);
        }
        if read {
            self.after_thread_read();
        }
    }

    /// Brings the list's rows up to date with the rows of the thread shown. Only the
    /// span that changed is replaced, so the rows around it keep their heights and the
    /// list keeps its scroll. A search hit's row is scrolled to once, when it is drawn.
    pub(crate) fn sync_thread_rows(&mut self) {
        let keys = self.model.threads.row_keys();
        let view = &mut self.threads_view;
        if keys != view.shown.as_slice() {
            let (span, count) = thread_rows::splice_span(&view.shown, keys);
            view.rows.splice(span, count);
            view.shown = keys.to_vec();
        }
        match self.model.threads.focus() {
            None => self.threads_view.focus_scrolled = None,
            Some(entry) if self.threads_view.focus_scrolled != Some(entry) => {
                if let Some(ix) = thread_rows::focus_row(self.model.threads.rows(), entry) {
                    self.threads_view.rows.scroll_to(ListOffset {
                        item_ix: ix,
                        offset_in_item: px(0.),
                    });
                    self.threads_view.focus_scrolled = Some(entry);
                }
            }
            Some(_) => {}
        }
    }

    /// Reads the threads linked to commit `oid`, for the commit panel (H3). A repository
    /// known to hold no threads asks for none.
    pub(crate) fn request_commit_threads(&mut self, oid: &str) {
        if self.model.threads.total() == Some(0) {
            self.model.commit_threads.clear();
            return;
        }
        let id = self
            .engine
            .dispatch(Actor::Human, commit_threads::plan(oid));
        self.model.commit_threads.request(oid, id);
    }

    /// Reads thread `uid` as Markdown. Its answer goes to the clipboard when it arrives (T6).
    pub(crate) fn copy_thread_markdown(&mut self, uid: String, cx: &mut Context<Self>) {
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::ReadThreadMarkdown { uid });
        self.model.thread_copy.sent(id);
        cx.notify();
    }

    /// Puts the Markdown of a copy on the clipboard, or says why the copy failed (T6).
    pub(crate) fn finish_thread_copy(
        &mut self,
        outcome: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(markdown) => {
                cx.write_to_clipboard(ClipboardItem::new_string(markdown));
                self.notify_user("Copied thread as Markdown", cx);
            }
            Err(message) => self.notify_user(&message, cx),
        }
    }

    /// Opens the Threads page with thread `uid` selected, from a commit's panel. The
    /// thread shows even when the list's filters leave it out.
    pub(crate) fn open_thread_from_commit(
        &mut self,
        uid: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_threads(window, cx);
        if let Some(request) = self.model.threads.select(uid, None) {
            self.send_threads(request);
        }
        cx.notify();
    }

    /// Reads the threads again. Nothing is asked before a repository is open, or
    /// while an open is in flight. On the page the selected thread is read again too.
    pub(crate) fn request_threads_refresh(&mut self) {
        if self.model.status().is_none() || self.model.is_opening() {
            return;
        }
        let requests = if self.screen == Screen::Threads {
            self.model.threads.plan_refresh()
        } else {
            vec![self.model.threads.plan_list()]
        };
        for request in requests {
            self.send_threads(request);
        }
    }

    /// Asks for the page of threads after the rows shown, when there is one.
    pub(crate) fn request_threads_next(&mut self) {
        if let Some(request) = self.model.threads.plan_next() {
            self.send_threads(request);
        }
    }

    /// Dispatches a planned read and records which read the answer belongs to.
    fn send_threads(&mut self, request: Request) {
        let Request { kind, read } = request;
        let id = self.engine.dispatch(Actor::Human, kind);
        self.model.threads.sent(&read, id);
    }

    /// After a thread's entries are shown: the states of its edits are read, and
    /// the entries of a search hit are read on until the hit's entry is loaded.
    pub(crate) fn after_thread_read(&mut self) {
        for request in self.model.threads.plan_edit_states() {
            self.send_threads(request);
        }
        if self.model.threads.wants_focus_entries()
            && let Some(request) = self.model.threads.plan_more()
        {
            self.send_threads(request);
        }
    }

    /// The box changed: the model takes the new text, and the search runs after a pause.
    fn on_threads_edited(&mut self, cx: &mut Context<Self>) {
        if self.screen != Screen::Threads {
            return;
        }
        let text = self.threads_view.query.read(cx).text().to_owned();
        if text.contains('\n') {
            // The box is one line; Enter is not a line break.
            let first = text.lines().next().unwrap_or("").to_owned();
            self.threads_view
                .query
                .update(cx, |area, cx| area.set_text(&first, cx));
            return;
        }
        let Some(generation) = self.model.threads.set_query(&text) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DEBOUNCE_MS))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.model.threads.is_latest(generation) {
                    view.run_threads_search(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Sends the search for the text now. A blank text shows the threads.
    pub(crate) fn run_threads_search(&mut self, cx: &mut Context<Self>) {
        if let Some(request) = self.model.threads.plan_search() {
            self.send_threads(request);
        }
        self.threads_view
            .list
            .scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Clears the search text (Esc). The list of threads shows again.
    fn clear_threads_search(&mut self, cx: &mut Context<Self>) {
        self.model.threads.set_query("");
        self.threads_view
            .query
            .update(cx, |area, cx| area.set_text("", cx));
        cx.notify();
    }

    /// Selects the row `index` of the list shown.
    fn choose_thread(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(request) = self.model.threads.choose(index) {
            self.send_threads(request);
        }
        cx.notify();
    }

    /// Moves the selection one row (Up and Down) and keeps the row in view.
    fn step_thread(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(request) = self.model.threads.step(delta) {
            self.send_threads(request);
        }
        if let Some(row) = self.model.threads.selected_row() {
            self.threads_view
                .list
                .scroll_to_item(row, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Turns a filter chip on or off. The list, and the search if one is shown, are read again.
    fn toggle_threads_chip(&mut self, chip: Chip, cx: &mut Context<Self>) {
        for request in self.model.threads.toggle(chip) {
            self.send_threads(request);
        }
        self.threads_view
            .list
            .scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Reads the entries before the ones shown (the top row of the detail).
    fn load_earlier_thread_entries(&mut self, cx: &mut Context<Self>) {
        if let Some(request) = self.model.threads.plan_earlier() {
            self.send_threads(request);
        }
        cx.notify();
    }

    /// The page: a header with the count, the search box and chips, then the list and the thread.
    pub(crate) fn threads_page(&self, cx: &mut Context<Self>) -> Div {
        let page = &self.model.threads;
        let count = match page.mode() {
            Mode::Threads if page.is_loaded() => {
                threads::count_label(page.threads().len(), page.has_more())
            }
            Mode::Results if page.hits_shown() => threads::hits_label(page.hits().len()),
            _ => String::new(),
        };
        let ctx = self.action_context("");
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
            .child(div().font_weight(FontWeight::MEDIUM).child("Threads"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(div().text_xs().text_color(theme::muted()).child(count))
                    .child(action_button(
                        Action::ThreadsSettings,
                        "threads-settings",
                        "Settings…",
                        &Args::None,
                        &ctx,
                        cx,
                    )),
            );
        let toolbar = div()
            .h(px(40.))
            .px_4()
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
                    .child(self.threads_view.query.clone()),
            )
            .child(self.threads_chip(Chip::Starred, "Starred", "threads-chip-starred", cx))
            .child(self.threads_chip(Chip::HasEdits, "Has edits", "threads-chip-edits", cx))
            .children(self.threads_path_chip(cx));
        let body = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_row()
            .child(self.threads_list_column(cx))
            .child(self.threads_detail(cx));
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(theme::ground())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "down" => this.step_thread(1, cx),
                    "up" => this.step_thread(-1, cx),
                    "escape" if !this.model.threads.query().is_empty() => {
                        this.clear_threads_search(cx);
                    }
                    _ => {}
                }
            }))
            .child(header)
            .child(toolbar)
            .child(body)
    }

    /// A filter chip: outlined when off, filled in the accent colour when on.
    fn threads_chip(
        &self,
        chip: Chip,
        label: &'static str,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let on = self.model.threads.chip_on(chip);
        let button = div()
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
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.toggle_threads_chip(chip, cx);
                }),
            );
        if on {
            button
                .bg(theme::selection())
                .border_color(theme::accent())
                .text_color(theme::accent())
        } else {
            button
                .border_color(theme::button_border())
                .text_color(theme::muted())
        }
    }

    /// The left column: the failure, then the threads or the hits, or what says there are none.
    fn threads_list_column(&self, cx: &mut Context<Self>) -> Div {
        let page = &self.model.threads;
        let failure = page.failure().map(|text| {
            div()
                .px_4()
                .py_2()
                .text_sm()
                .text_color(theme::error())
                .child(text.to_owned())
        });
        let body: AnyElement = match page.mode() {
            Mode::Results if !page.hits_shown() => note("Searching…".to_owned()).into_any_element(),
            Mode::Results if page.hits().is_empty() => {
                self.empty_view(Empty::Matches, cx).into_any_element()
            }
            Mode::Results => {
                hit_rows(page.hits().len(), self.threads_view.list.clone(), cx).into_any_element()
            }
            Mode::Threads if !page.is_loaded() => {
                note("Loading threads…".to_owned()).into_any_element()
            }
            Mode::Threads => match page.empty() {
                Some(empty) => self.empty_view(empty, cx).into_any_element(),
                None => thread_rows(page.threads().len(), self.threads_view.list.clone(), cx)
                    .into_any_element(),
            },
        };
        div()
            .w(px(LIST_WIDTH))
            .flex_none()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::hairline())
            .children(failure)
            .child(body)
    }

    /// The right column: the thread's header and its turns, or what it is waiting for.
    fn threads_detail(&self, cx: &mut Context<Self>) -> Div {
        let page = &self.model.threads;
        let Some(detail) = page.detail() else {
            let text = match (page.selected(), page.failure()) {
                (Some(_), Some(text)) => text.to_owned(),
                (Some(_), None) if page.is_loading_detail() => "Loading thread…".to_owned(),
                (Some(_), None) => "This thread could not be shown.".to_owned(),
                (None, _) => "Select a thread to read it.".to_owned(),
            };
            return note(text);
        };
        let summary = &detail.summary;
        let branch = summary.branch.as_deref().unwrap_or("detached HEAD");
        let state = if summary.ended_at.is_none() {
            " · open"
        } else {
            ""
        };
        let meta = format!(
            "{} · {branch} · started {}{state}",
            summary.harness,
            threads::relative(now_secs(), summary.started_at)
        );
        let header = div()
            .flex_none()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(theme::hairline())
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(threads::title(summary).to_owned()),
            )
            .child(div().text_xs().text_color(theme::muted()).child(meta))
            .children(note_view(&summary.note))
            .child(self.thread_curation(detail, cx))
            .child(Self::fold_buttons(cx));
        let body = div()
            .flex_1()
            .min_h(px(0.))
            .px_4()
            .py_3()
            .child(self.detail_list(cx));
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(header)
            .child(body)
    }

    /// The header's actions of the selected thread, then its tags and its links (T6).
    fn thread_curation(&self, detail: &ThreadDetail, cx: &mut Context<Self>) -> Div {
        let summary = &detail.summary;
        let target = thread_args(ThreadRef {
            uid: &summary.uid,
            starred: summary.starred,
        });
        let ctx = self.action_context("");
        let star = if summary.starred { "Unstar" } else { "Star" };
        let actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(action_button(
                Action::ThreadRename,
                "thread-rename",
                "Rename",
                &target,
                &ctx,
                cx,
            ))
            .child(action_button(
                Action::ThreadNote,
                "thread-note",
                "Add Note",
                &target,
                &ctx,
                cx,
            ))
            .child(action_button(
                Action::ThreadTags,
                "thread-tags",
                "Tags",
                &target,
                &ctx,
                cx,
            ))
            .child(action_button(
                Action::ThreadStar,
                "thread-star",
                star,
                &target,
                &ctx,
                cx,
            ))
            .child(action_button(
                Action::ThreadLinkCommit,
                "thread-link",
                "Link to Commit",
                &target,
                &ctx,
                cx,
            ))
            .child(action_button(
                Action::ThreadExport,
                "thread-export",
                "Export",
                &target,
                &ctx,
                cx,
            ))
            .child(action_button(
                Action::ThreadCopyMarkdown,
                "thread-copy-markdown",
                "Copy as Markdown",
                &target,
                &ctx,
                cx,
            ))
            .child(action_button(
                Action::ThreadDelete,
                "thread-delete",
                "Delete",
                &target,
                &ctx,
                cx,
            ));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(actions)
            .children(tag_row(&summary.tags))
            .children(link_rows(&detail.links, &summary.uid, cx))
    }

    /// The file filter's chip in the toolbar: `File: <path>`, which a click removes.
    fn threads_path_chip(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let path = self.model.threads.path()?;
        Some(
            div()
                .id("threads-chip-path")
                .cursor_pointer()
                .flex_none()
                .max_w(px(PATH_CHIP_WIDTH))
                .px_2()
                .h(px(24.))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .rounded_full()
                .border_1()
                .border_color(theme::accent())
                .bg(theme::selection())
                .text_xs()
                .text_color(theme::accent())
                .child(
                    div()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(format!("File: {path}")),
                )
                .child(icon::view(Icon::Close, icon::SMALL, theme::accent()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.clear_threads_path(cx);
                    }),
                ),
        )
    }

    /// Shows the Threads page with the threads that touched `path` (T6).
    pub(crate) fn show_threads_for(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.model.threads.set_path(Some(path.to_owned()));
        self.show_threads(window, cx);
    }

    /// Removes the file filter and reads the list again (T6).
    fn clear_threads_path(&mut self, cx: &mut Context<Self>) {
        if self.model.threads.set_path(None) {
            self.request_threads_refresh();
        }
        cx.notify();
    }

    /// Removes a manual link of the thread shown. Nothing to confirm: a link is
    /// only a pointer, and the commit it names is untouched.
    fn unlink_thread(&mut self, uid: &str, link: &ThreadLink, cx: &mut Context<Self>) {
        self.engine
            .dispatch(Actor::Human, curate::unlink_command(uid, link));
        cx.notify();
    }

    /// The buttons over the detail list that open or close every tool-call group.
    fn fold_buttons(cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_row()
            .gap_2()
            .child(small_button(
                "threads-expand-all",
                "Expand all",
                cx,
                |this, cx| {
                    this.model.threads.expand_all_groups();
                    cx.notify();
                },
            ))
            .child(small_button(
                "threads-collapse-all",
                "Collapse all",
                cx,
                |this, cx| {
                    this.model.threads.collapse_all_groups();
                    cx.notify();
                },
            ))
    }

    /// The rows of the thread shown, in the variable-height list. The list asks for
    /// each row it draws, so only the visible ones are built.
    fn detail_list(&self, cx: &mut Context<Self>) -> Div {
        let view = cx.entity();
        div().flex_1().min_h(px(0.)).child(
            list(self.threads_view.rows.clone(), move |ix, _window, cx| {
                view.update(cx, |this, cx| this.detail_row(ix, cx))
            })
            .size_full(),
        )
    }

    /// One row of the detail list. Drawing a row near the end of the rows asks for the
    /// next page of entries; drawing one near the start asks for the entries before.
    pub(crate) fn detail_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let count = self.model.threads.rows().len();
        if thread_rows::near_end(ix, count)
            && let Some(request) = self.model.threads.plan_more()
        {
            self.send_threads(request);
        }
        if thread_rows::near_start(ix)
            && let Some(request) = self.model.threads.plan_earlier()
        {
            self.send_threads(request);
        }
        match self.model.threads.rows().get(ix) {
            Some(row) => self.row_view(ix, row, cx),
            None => div().into_any_element(),
        }
    }

    /// Draws one row of the thread: a turn's header, a prompt, a text, a group's
    /// header, a call and its body, an edit and its patch, or a marker.
    fn row_view(&self, ix: usize, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        match row {
            Row::Turn {
                number,
                collapsed,
                focused,
            } => Self::turn_head(*number, *collapsed, *focused, cx).into_any_element(),
            Row::Prompt { text, focused, .. } => prompt_block(text, *focused).into_any_element(),
            Row::Message {
                entry,
                kind,
                text,
                depth,
                focused,
            } => {
                let body = message_body(*kind, text, *focused);
                indented(*depth, Self::framed(*entry, body, cx)).into_any_element()
            }
            Row::Group {
                key, label, failed, ..
            } => Self::group_head(ix, key, label, failed.as_deref(), cx).into_any_element(),
            Row::Call {
                entry,
                depth,
                tool,
                summary,
                status,
                duration_ms,
                focused,
                ..
            } => {
                let fields = CallFields {
                    entry: *entry,
                    tool,
                    summary,
                    status: *status,
                    duration_ms: *duration_ms,
                    focused: *focused,
                };
                let line = Self::call_line(&fields, cx);
                indented(*depth, Self::framed(*entry, line, cx)).into_any_element()
            }
            Row::CallBody {
                depth,
                args,
                result,
                ..
            } => {
                indented(*depth, call_details(args.as_deref(), result.as_ref())).into_any_element()
            }
            Row::Edit {
                id,
                depth,
                path,
                counts,
                state,
                open,
            } => {
                let card = self.edit_row(*id, path, counts, state, *open, cx);
                indented(*depth, card).into_any_element()
            }
            Row::Patch { depth, patch, .. } => {
                indented(*depth, patch_view(patch)).into_any_element()
            }
            Row::Marker(marker) => Self::marker_view(*marker, cx),
        }
    }

    /// A turn's header. A click folds the turn; the focused turn's label is accent-coloured.
    fn turn_head(
        number: u32,
        collapsed: bool,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label = if collapsed {
            format!("Turn {number} (collapsed)")
        } else {
            format!("Turn {number}")
        };
        div()
            .id(("turn", u64::from(number)))
            .cursor_pointer()
            .text_xs()
            .text_color(if focused {
                theme::accent()
            } else {
                theme::muted()
            })
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.model.threads.toggle_turn(number);
                    cx.notify();
                }),
            )
    }

    /// A tool-call group's header: its label, and its failures in the error colour.
    /// A click opens or closes the group.
    fn group_head(
        ix: usize,
        key: &GroupKey,
        label: &str,
        failed: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let key: GroupKey = key.clone();
        div()
            .id(("thread-group", u64::try_from(ix).unwrap_or(0)))
            .cursor_pointer()
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(theme::hairline())
            .bg(theme::panel())
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .text_xs()
            .text_color(theme::muted())
            .child(
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label.to_owned()),
            )
            .children(failed.map(|text| {
                div()
                    .flex_none()
                    .text_color(theme::error())
                    .child(format!("· {text}"))
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.model.threads.toggle_group(&key);
                cx.notify();
            }))
    }

    /// Wraps a row's body with its entry's delete, which shows while the row is under the pointer (T6).
    fn framed(entry: i64, body: impl IntoElement, cx: &mut Context<Self>) -> Div {
        let group = format!("thread-entry-{entry}");
        let key = usize::try_from(entry).unwrap_or(usize::MAX);
        let delete = div()
            .id(("thread-entry-delete", key))
            .invisible()
            .group_hover(group.clone(), gpui::Styled::visible)
            .cursor_pointer()
            .px_1()
            .text_xs()
            .text_color(theme::removed_fg())
            .child("Delete entry")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.open_delete_entry_dialog(entry, window, cx);
                }),
            );
        div()
            .group(group)
            .flex()
            .flex_col()
            .child(body)
            .child(div().flex().flex_row().justify_end().child(delete))
    }

    /// A tool call's collapsed line, which a click opens into the arguments and the result.
    fn call_line(call: &CallFields<'_>, cx: &mut Context<Self>) -> Stateful<Div> {
        let entry = call.entry;
        let status_colour = match call.status {
            CallStatus::Done => theme::muted(),
            CallStatus::Failed => theme::error(),
            CallStatus::Pending => theme::faint(),
        };
        div()
            .id(("call", u64::try_from(entry).unwrap_or(0)))
            .cursor_pointer()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme::hairline())
            .when(call.focused, |line| line.bg(theme::selected_row()))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.model.threads.toggle_call(entry);
                cx.notify();
            }))
            .child(
                div()
                    .flex_none()
                    .w(px(6.))
                    .h(px(6.))
                    .rounded_full()
                    .bg(status_colour),
            )
            .child(
                div()
                    .flex_none()
                    .px_1p5()
                    .rounded_sm()
                    .font_family(theme::code_font())
                    .text_xs()
                    .bg(theme::selection())
                    .text_color(theme::text())
                    .child(call.tool.to_owned()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(call.summary.to_owned()),
            )
            .children(call.duration_ms.map(|ms| {
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(threads::duration_label(ms))
            }))
    }

    /// The row of a thread's marker: a load control at the top, and a line of text at
    /// the end while entries are read and when the thread is done.
    fn marker_view(marker: Marker, cx: &mut Context<Self>) -> AnyElement {
        match marker {
            Marker::Earlier { loading: true } => {
                small_note("Loading earlier entries…").into_any_element()
            }
            Marker::Earlier { loading: false } => div()
                .id("threads-load-earlier")
                .cursor_pointer()
                .flex_none()
                .px_2()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(theme::button_border())
                .text_xs()
                .child("Load earlier entries")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.load_earlier_thread_entries(cx);
                    }),
                )
                .into_any_element(),
            Marker::Loading => small_note("Loading…").into_any_element(),
            Marker::End => small_note("End of thread").into_any_element(),
        }
    }

    /// View File: opens the edit's file on the Code page, as Quick Open does. Refused, it reads muted.
    fn view_file_button(&self, id: i64, path: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let path = path.to_owned();
        let view_refusal = Action::ViewFile
            .enabled(&Args::File(path.clone()), &self.action_context(""))
            .err();
        let view_button = div()
            .id(("edit-view", u64::try_from(id).unwrap_or(0)))
            .flex_none()
            .px_2()
            .h(px(22.))
            .flex()
            .items_center()
            .rounded_sm()
            .border_1()
            .border_color(theme::button_border())
            .text_xs()
            .child("View File");
        match view_refusal {
            None => view_button.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.run_action(Action::ViewFile, &Args::File(path.clone()), window, cx);
                }),
            ),
            Some(reason) => view_button
                .text_color(theme::muted())
                .tooltip(crate::window_chrome::hint(reason)),
        }
    }

    /// An edit: its path, `+n −m`, the state of its file, and the buttons. Open Diff
    /// shows the patch below the card; View File opens the file on the Code page.
    fn edit_row(
        &self,
        id: i64,
        path: &str,
        counts: &str,
        state: &'static str,
        open: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let view_button = self.view_file_button(id, path, cx);
        let diff_button = div()
            .id(("edit-diff", u64::try_from(id).unwrap_or(0)))
            .cursor_pointer()
            .flex_none()
            .px_2()
            .h(px(22.))
            .flex()
            .items_center()
            .rounded_sm()
            .border_1()
            .border_color(theme::button_border())
            .text_xs()
            .child(if open { "Hide Diff" } else { "Open Diff" })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.model.threads.toggle_diff(id);
                    cx.notify();
                }),
            );
        let head = div()
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
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(theme::code_font())
                    .text_xs()
                    .child(path.to_owned()),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(counts.to_owned()),
            );
        let actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .px_1p5()
                    .rounded_sm()
                    .text_xs()
                    .bg(theme::selection())
                    .text_color(theme::text())
                    .child(state),
            )
            .child(div().flex_1())
            .child(diff_button)
            .child(view_button);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(theme::hairline())
            .bg(theme::panel())
            .child(head)
            .child(actions)
    }
}

/// The fields of a tool call's line, as its row holds them.
struct CallFields<'a> {
    entry: i64,
    tool: &'a str,
    summary: &'a str,
    status: CallStatus,
    duration_ms: Option<i64>,
    focused: bool,
}

/// A row indented by its depth in the thread: a sub-agent's calls sit under their call.
fn indented(depth: usize, body: Div) -> Div {
    let steps = u16::try_from(depth).unwrap_or(u16::MAX);
    div()
        .pl(px(INDENT * f32::from(steps)))
        .min_w(px(0.))
        .child(body)
}

/// The text of a message row, styled by its kind and by its focus.
fn message_body(kind: MessageKind, text: &str, focused: bool) -> Div {
    let block = div()
        .flex()
        .flex_col()
        .gap_1()
        .px_1()
        .rounded_sm()
        .when(focused, |block| block.bg(theme::selected_row()));
    match kind {
        MessageKind::Assistant => block.child(text_block(text, MESSAGE_LINES)),
        MessageKind::Notice => block
            .text_xs()
            .text_color(theme::muted())
            .child(text_block(text, MESSAGE_LINES)),
        MessageKind::Result => block
            .font_family(theme::code_font())
            .text_xs()
            .text_color(theme::muted())
            .child(text_block(text, RESULT_LINES)),
    }
}

/// The human's prompt of a turn, in its panel.
fn prompt_block(text: &str, focused: bool) -> Div {
    div()
        .p_3()
        .rounded_md()
        .bg(theme::panel())
        .border_1()
        .border_color(theme::hairline())
        .when(focused, |block| block.bg(theme::selected_row()))
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(theme::muted()).child("Prompt"))
        .child(text_block(text, MESSAGE_LINES))
}

/// A small outlined text button of the detail's header, which runs `on_click`.
fn small_button(
    id: &'static str,
    label: &'static str,
    cx: &mut Context<StatusView>,
    on_click: fn(&mut StatusView, &mut Context<StatusView>),
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .flex_none()
        .px_2()
        .h(px(22.))
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(theme::button_border())
        .text_xs()
        .text_color(theme::muted())
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| on_click(this, cx)),
        )
}

/// A line of muted small text between the rows of the detail.
fn small_note(text: &str) -> Div {
    div()
        .py_2()
        .text_xs()
        .text_color(theme::muted())
        .child(text.to_owned())
}

/// The arguments and the result of an open tool call.
fn call_details(args: Option<&str>, result: Option<&CallResult>) -> Div {
    let mut details = div().flex().flex_col().gap_1().px_2().pb_1().text_xs();
    if let Some(args) = args {
        details = details
            .child(div().text_color(theme::muted()).child("Arguments"))
            .child(
                div()
                    .font_family(theme::code_font())
                    .child(text_block(args, ARG_LINES)),
            );
    }
    details = match result {
        Some(result) => {
            let mut shown = details
                .child(div().text_color(theme::muted()).child("Result"))
                .child(text_block(&result.text, RESULT_LINES));
            if result.truncated {
                shown = shown.child(
                    div()
                        .text_color(theme::muted())
                        .child("Cut at the capture limit."),
                );
            }
            shown
        }
        None => details.child(
            div()
                .text_color(theme::muted())
                .child("No result was recorded."),
        ),
    };
    details
}

/// A line of muted text in the middle of the list or the detail.
fn note(text: String) -> Div {
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme::muted())
        .child(text)
}

/// What an empty list says, with the install command under the no-integration state.
impl StatusView {
    /// The text of an empty list. The missing integration also gets the button that
    /// installs it, above the command that does the same from a terminal.
    fn empty_view(&self, empty: Empty, cx: &mut Context<Self>) -> Div {
        let ctx = self.action_context("");
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .px_4()
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .child(threads::empty_text(empty)),
            )
            .children((empty == Empty::Integration).then(|| {
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(action_button(
                        Action::ThreadsInstall,
                        "threads-install-empty",
                        "Install Agent Integration…",
                        &Args::None,
                        &ctx,
                        cx,
                    ))
                    .child(
                        div()
                            .font_family(theme::code_font())
                            .text_xs()
                            .text_color(theme::muted())
                            .child(INSTALL_HINT_TEXT),
                    )
            }))
    }
}

/// The threads, read from the list as they are drawn. Reaching near the end asks for the next page.
fn thread_rows(count: usize, list: UniformListScrollHandle, cx: &mut Context<StatusView>) -> Div {
    div().flex_1().min_h(px(0.)).flex().flex_col().child(
        uniform_list(
            "thread-rows",
            count,
            cx.processor(
                move |this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut Window,
                      cx: &mut Context<StatusView>| {
                    if range.end + PREFETCH >= count {
                        this.request_threads_next();
                    }
                    let now = now_secs();
                    let page = &this.model.threads;
                    let selected = page.selected();
                    page.threads()
                        .get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(thread, index)| {
                            let is_selected = selected == Some(thread.uid.as_str());
                            thread_row(thread, index, now, is_selected, cx)
                        })
                        .collect()
                },
            ),
        )
        .track_scroll(list)
        .flex_1()
        .min_h(px(0.)),
    )
}

/// The search's hits, read from the list as they are drawn.
fn hit_rows(count: usize, list: UniformListScrollHandle, cx: &mut Context<StatusView>) -> Div {
    div().flex_1().min_h(px(0.)).flex().flex_col().child(
        uniform_list(
            "thread-hits",
            count,
            cx.processor(
                move |this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut Window,
                      cx: &mut Context<StatusView>| {
                    let selected = this.model.threads.selected();
                    this.model
                        .threads
                        .hits()
                        .get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(hit, index)| {
                            let is_selected = selected == Some(hit.thread_uid.as_str());
                            hit_row(hit, index, is_selected, cx)
                        })
                        .collect()
                },
            ),
        )
        .track_scroll(list)
        .flex_1()
        .min_h(px(0.)),
    )
}

/// The star of a thread row: a click toggles the star, and the row keeps its selection.
fn thread_star(
    thread: &ThreadSummary,
    index: usize,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let uid = thread.uid.clone();
    let starred = thread.starred;
    div()
        .id(("thread-row-star", index))
        .cursor_pointer()
        .flex_none()
        .px_1()
        .text_color(if starred {
            theme::accent()
        } else {
            theme::muted()
        })
        .child(if starred { "★" } else { "☆" })
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            // The star is not a click on the row: the thread stays as it was.
            cx.stop_propagation();
            let target = thread_args(ThreadRef { uid: &uid, starred });
            this.run_action(Action::ThreadStar, &target, window, cx);
        }))
}

/// One thread: its title, a dot while it is open, and its agent, branch, time,
/// edit count and a `redacted` chip. A click selects it.
fn thread_row(
    thread: &ThreadSummary,
    index: usize,
    now: i64,
    selected: bool,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let mut head = div().flex().flex_row().items_center().gap_2().min_w(px(0.));
    if thread.ended_at.is_none() {
        head = head.child(
            div()
                .flex_none()
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(theme::accent()),
        );
    }
    let head = head
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(threads::title(thread).to_owned()),
        )
        .child(thread_star(thread, index, cx));
    let menu_uid = thread.uid.clone();
    let menu_title = threads::title(thread).to_owned();
    let menu_starred = thread.starred;
    let meta = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .text_xs()
        .text_color(theme::muted())
        .child(chip(thread.harness.clone()))
        .children(thread.branch.clone().map(|branch| {
            div()
                .flex_none()
                .max_w(px(BRANCH_WIDTH))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(branch)
        }))
        .child(
            div()
                .flex_none()
                .child(threads::relative(now, thread.last_event_at)),
        )
        .child(
            div()
                .flex_none()
                .child(threads::edits_label(thread.edit_count)),
        )
        .children((thread.redaction_count > 0).then(|| {
            div()
                .flex_none()
                .px_1()
                .rounded_sm()
                .bg(theme::selected_row())
                .child("redacted")
        }));
    div()
        .id(("thread-row", index))
        .cursor_pointer()
        .w_full()
        .h(px(THREAD_ROW_HEIGHT))
        .px_4()
        .flex()
        .flex_col()
        .justify_center()
        .gap_1()
        .border_b_1()
        .border_color(theme::hairline())
        .when(selected, |row| row.bg(theme::selected_row()))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.choose_thread(index, cx);
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.open_thread_menu(&menu_uid, &menu_title, menu_starred, event.position, cx);
            }),
        )
        .child(head)
        .child(meta)
}

/// One hit: the thread's title, its turn, and the snippet around the match.
fn hit_row(
    hit: &ThreadHit,
    index: usize,
    selected: bool,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    div()
        .id(("hit-row", index))
        .cursor_pointer()
        .w_full()
        .h(px(HIT_ROW_HEIGHT))
        .px_4()
        .flex()
        .flex_col()
        .justify_center()
        .gap_1()
        .border_b_1()
        .border_color(theme::hairline())
        .when(selected, |row| row.bg(theme::selected_row()))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.choose_thread(index, cx);
        }))
        .child(
            div()
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
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(hit.title.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme::muted())
                        .child(format!("Turn {}", hit.turn)),
                ),
        )
        .child(
            div()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_xs()
                .text_color(theme::muted())
                .child(hit.snippet.replace('\n', " ")),
        )
}

/// A small chip: a code-font label on the selection colour.
pub(crate) fn chip(text: String) -> Div {
    div()
        .flex_none()
        .px_1p5()
        .rounded_sm()
        .font_family(theme::code_font())
        .bg(theme::selection())
        .text_color(theme::text())
        .child(text)
}

/// A text, line by line, with its redaction markers painted as muted chips. The first
/// `max_lines` are drawn; the rest is counted.
fn text_block(text: &str, max_lines: usize) -> Div {
    let (shown, hidden) = threads::head(text, max_lines);
    div()
        .flex()
        .flex_col()
        .min_w(px(0.))
        .children(shown.lines().map(spans_line))
        .children((hidden > 0).then(|| {
            div()
                .text_xs()
                .text_color(theme::muted())
                .child(format!("… {hidden} more lines"))
        }))
}

/// One line of text. A redaction is a muted `[REDACTED:name]` chip, never a blank.
fn spans_line(line: &str) -> Div {
    if line.is_empty() {
        return div().h(px(8.));
    }
    div().flex().flex_row().flex_wrap().min_w(px(0.)).children(
        threads::spans(line).into_iter().map(|span| match span {
            Span::Text(text) => div().min_w(px(0.)).child(text),
            Span::Redacted(name) => div()
                .flex_none()
                .px_1()
                .rounded_sm()
                .font_family(theme::code_font())
                .text_xs()
                .bg(theme::selected_row())
                .text_color(theme::muted())
                .child(format!("[REDACTED:{name}]")),
        }),
    )
}

/// The patch of an edit, coloured as the diff view colours it.
fn patch_view(patch: &str) -> Div {
    let lines = threads::patch_lines(patch);
    let hidden = lines.len().saturating_sub(PATCH_LINES);
    div()
        .flex()
        .flex_col()
        .font_family(theme::code_font())
        .text_xs()
        .children(lines.into_iter().take(PATCH_LINES).map(|(kind, line)| {
            let (text, background) = match kind {
                threads::PatchKind::Added => (theme::added_fg(), theme::added_bg()),
                threads::PatchKind::Removed => (theme::removed_fg(), theme::removed_bg()),
                threads::PatchKind::Hunk => (theme::muted(), theme::hunk_header_bg()),
                threads::PatchKind::Context => (theme::text(), theme::panel()),
            };
            div()
                .px_1()
                .whitespace_nowrap()
                .text_color(text)
                .bg(background)
                .child(line.to_owned())
        }))
        .children((hidden > 0).then(|| {
            div()
                .text_color(theme::muted())
                .child(format!("… {hidden} more lines"))
        }))
}

/// The width the file filter's chip may take before its text is cut.
const PATH_CHIP_WIDTH: f32 = 260.;

/// The note of a thread, as the header shows it, or nothing when it has none.
fn note_view(note: &str) -> Option<Div> {
    let note = note.trim();
    (!note.is_empty()).then(|| text_block(note, MESSAGE_LINES))
}

/// The tags of a thread as chips, or nothing when it has none.
fn tag_row(tags: &[String]) -> Option<Div> {
    (!tags.is_empty()).then(|| {
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_1()
            .children(tags.iter().map(|tag| chip(tag.clone())))
    })
}

/// The links of a thread: a manual one with its Unlink, an observed one as the
/// commit that was made during the thread, with no button.
fn link_rows(links: &[ThreadLink], uid: &str, cx: &mut Context<StatusView>) -> Option<Div> {
    if links.is_empty() {
        return None;
    }
    let rows = links.iter().enumerate().map(|(index, link)| {
        let kind = match link.kind {
            LinkKind::Commit => "Commit",
            LinkKind::Branch => "Branch",
            LinkKind::File => "File",
        };
        let reference: String = match link.kind {
            LinkKind::Commit => link.reference.chars().take(COMMIT_CHARS).collect(),
            LinkKind::Branch | LinkKind::File => link.reference.clone(),
        };
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .text_xs()
            .child(chip(kind.to_owned()))
            .child(
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(theme::code_font())
                    .child(reference),
            );
        match link.origin {
            LinkOrigin::Observed => row
                .text_color(theme::muted())
                .child("created during this thread"),
            LinkOrigin::Manual => {
                let uid = uid.to_owned();
                let link = link.clone();
                row.child(
                    div()
                        .id(("thread-unlink", index))
                        .cursor_pointer()
                        .px_2()
                        .rounded_sm()
                        .border_1()
                        .border_color(theme::button_border())
                        .text_color(theme::text())
                        .child("Unlink")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.unlink_thread(&uid, &link, cx);
                            }),
                        ),
                )
            }
        }
    });
    Some(
        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(rows.collect::<Vec<_>>()),
    )
}

/// The characters of a commit id a link row shows.
const COMMIT_CHARS: usize = 12;

/// Which of the engine's events the Threads page follows: a changed thread database,
/// an answer with a thread's entries, and an answer with a page of the list.
pub(crate) fn follow_flags(event: &Event) -> (bool, bool, bool) {
    (
        matches!(event, Event::ThreadsChanged),
        matches!(event, Event::ThreadRead { .. }),
        matches!(event, Event::ThreadsListed { .. }),
    )
}
