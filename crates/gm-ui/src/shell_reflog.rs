//! The Reflog page (B3, D95): the entries of HEAD's reflog, newest first, one
//! 48 px row each, with its `HEAD@{n}` chip, its action, the commit's subject or
//! the detail, the short id and the relative time. The next page loads as the
//! list nears its end, as the history's does. A click selects an entry and fills
//! the detail panel with its files; a file opens its diff through the commit
//! machinery. The panel's buttons, and a right-click on a row, check the commit
//! out (detached), start a branch there or copy its id.

use std::ops::Range;

use gm_schema::{Actor, CommandKind, ReflogEntry};
use gpui::{
    ClickEvent, Context, Div, FontWeight, MouseButton, MouseDownEvent, Stateful, Window, div,
    prelude::*, px, uniform_list,
};

use crate::action_registry::{Action, Args};
use crate::model::CommitPage;
use crate::reflog::{self, CheckoutOutcome, EntryKey, ReflogList};
use crate::shell_history::{Panel, action_button, commit_panel, now_secs, relative_time};
use crate::theme;
use crate::view::{Screen, StatusView};

const ROW_HEIGHT: f32 = 48.;
const TIME_WIDTH: f32 = 96.;
/// The next page is asked for when the rows drawn reach this close to the end.
const PAGE_PREFETCH: usize = 20;
const EMPTY_TEXT: &str = "The reflog of HEAD is empty.";

impl StatusView {
    /// The page for the main area: a header with the count, then the list and the
    /// detail panel, or the stacked diff of the selected entry in their place.
    pub(crate) fn reflog_page(&self, cx: &mut Context<Self>) -> Div {
        let list = &self.model.reflog;
        let count = if list.is_loaded() {
            reflog::count_label(list.entries().len(), list.has_more())
        } else {
            String::new()
        };
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
            .child(div().font_weight(FontWeight::MEDIUM).child("Reflog"))
            .child(div().text_xs().text_color(theme::muted()).child(count));
        let body = if self.model.commit_stack(CommitPage::Reflog).is_open() {
            self.commit_file_view(cx)
        } else {
            let selected = list.selected_entry();
            let actions = selected.map(|entry| self.reflog_actions(entry, cx));
            let panel = Panel::Reflog(selected.map(reflog::header));
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_row()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .flex()
                        .flex_col()
                        .child(reflog_list_column(list, cx)),
                )
                .child(commit_panel(
                    self.model.commit_pane(CommitPage::Reflog),
                    &self.commit_texts,
                    &panel,
                    actions,
                    None,
                    cx,
                ))
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(theme::ground())
            .child(header)
            .child(body)
    }

    /// The selected entry's buttons: check out (detached), a new branch there and
    /// copy the hash. Each is refused by the registry's rule, with the reason as its tooltip.
    fn reflog_actions(&self, entry: &ReflogEntry, cx: &Context<Self>) -> Div {
        let target = Args::Commit(entry.oid.clone());
        let ctx = self.action_context("");
        div().flex().flex_row().flex_wrap().gap_2().children(
            [
                (
                    Action::CheckoutCommit,
                    "reflog-checkout",
                    "Check out (detached)",
                ),
                (
                    Action::NewBranchFromCommit,
                    "reflog-new-branch",
                    "New branch here…",
                ),
                (Action::CopyCommitHash, "reflog-copy-hash", "Copy hash"),
            ]
            .map(|(action, id, label)| action_button(action, id, label, &target, &ctx, cx)),
        )
    }

    /// Shows the page: its first page is read again, and the stacked diffs of the
    /// history and the Stashes page close, since the diff pane is shared.
    pub(crate) fn show_reflog(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::Reflog;
        self.model.commit_stack.close();
        self.model.stash_stack.close();
        self.show_commit_texts(cx);
        self.request_reflog_top();
        cx.notify();
    }

    /// Asks the engine for the first page. Nothing is asked before a repository is
    /// open, or while an open is in flight, since the answer would be the old one's.
    /// A status refresh asks again while the page is open: a reload keeps only the
    /// first page, and the pages after it are read again as the list is scrolled.
    pub(crate) fn request_reflog_top(&mut self) {
        if self.model.status().is_none() || self.model.is_opening() {
            return;
        }
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::LoadReflog { skip: 0 });
        self.model.reflog.request(id);
    }

    /// Asks for the page after the entries shown, when there is one to ask for.
    pub(crate) fn request_reflog_page(&mut self) {
        if !self.model.reflog.can_page() {
            return;
        }
        let skip = self.model.reflog.next_skip();
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::LoadReflog { skip });
        self.model.reflog.request(id);
    }

    /// Selects the entry `key` and asks for its commit's detail. Selecting another
    /// entry supersedes the read in flight, as a commit does.
    pub(crate) fn select_reflog_entry(&mut self, key: &EntryKey, cx: &mut Context<Self>) {
        if !self.model.reflog.select(key) {
            return;
        }
        self.model.reflog_stack.close();
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::LoadCommitDetail {
                oid: key.oid().to_owned(),
            },
        );
        self.model.reflog_commit.request(id);
        cx.notify();
    }

    /// Says how a detached checkout ended: a notice, or the message with Git's text.
    pub(crate) fn on_reflog_checkout_settled(
        &mut self,
        outcome: CheckoutOutcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            CheckoutOutcome::Done { short } => self.notify_user(&reflog::done_notice(&short), cx),
            CheckoutOutcome::Failed { short, text } => {
                self.open_message(reflog::failure_title(&short), text, window, cx);
            }
        }
    }
}

/// The list column: the entries, or the one line that says why there are none.
fn reflog_list_column(list: &ReflogList, cx: &mut Context<StatusView>) -> Div {
    let failure = list.failure().filter(|_| list.is_loaded()).map(|text| {
        div()
            .px_4()
            .py_2()
            .text_sm()
            .text_color(theme::error())
            .child(text.to_owned())
    });
    let body = if let Some(text) = list.failure().filter(|_| !list.is_loaded()) {
        div()
            .px_4()
            .py_3()
            .text_color(theme::error())
            .child(text.to_owned())
    } else if !list.is_loaded() {
        placeholder("Loading reflog…")
    } else if list.entries().is_empty() {
        placeholder(EMPTY_TEXT)
    } else {
        reflog_rows(list.entries().len(), cx)
    };
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .children(failure)
        .child(body)
}

/// A line of muted text in the middle of the list column.
fn placeholder(text: &'static str) -> Div {
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme::muted())
        .child(text)
}

/// The rows, read from the list as it is when they are drawn.
fn reflog_rows(count: usize, cx: &mut Context<StatusView>) -> Div {
    div().flex_1().min_h(px(0.)).flex().flex_col().child(
        uniform_list(
            "reflog-rows",
            count,
            cx.processor(
                move |this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut Window,
                      cx: &mut Context<StatusView>| {
                    if range.end + PAGE_PREFETCH >= count {
                        this.request_reflog_page();
                    }
                    let now = now_secs();
                    let list = &this.model.reflog;
                    list.entries()
                        .get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(entry, index)| {
                            let selected = list.is_selected(entry);
                            reflog_row(entry, index, now, selected, cx)
                        })
                        .collect()
                },
            ),
        )
        .flex_1()
        .min_h(px(0.)),
    )
}

/// One entry: its `HEAD@{n}` chip, its action, the subject or the detail, the
/// short id and its relative time. A click selects it; a right-click opens its menu.
fn reflog_row(
    entry: &ReflogEntry,
    index: usize,
    now: i64,
    selected: bool,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let key = EntryKey::of(entry);
    let (menu_index, menu_oid) = (entry.index, entry.oid.clone());
    div()
        .id(("reflog-row", index))
        .cursor_pointer()
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_4()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .border_b_1()
        .border_color(theme::hairline())
        .when(selected, |row| row.bg(theme::selected_row()))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.select_reflog_entry(&key, cx);
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.open_reflog_menu(menu_index, &menu_oid, event.position, cx);
            }),
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
                .child(reflog::label(entry.index)),
        )
        .child(
            div()
                .flex_none()
                .px_1p5()
                .rounded_sm()
                .text_xs()
                .bg(theme::selected_row())
                .text_color(theme::muted())
                .child(entry.action.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(reflog::row_text(entry).to_owned()),
        )
        .child(
            div()
                .flex_none()
                .font_family(theme::code_font())
                .text_xs()
                .text_color(theme::muted())
                .child(reflog::short(&entry.oid).to_owned()),
        )
        .child(
            div()
                .flex_none()
                .w(px(TIME_WIDTH))
                .text_xs()
                .text_color(theme::muted())
                .text_right()
                .child(relative_time(now, entry.time)),
        )
}
