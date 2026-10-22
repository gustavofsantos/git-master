//! The Stashes page (B2, D94): the stashes of the repository, newest first, one
//! 48 px row each, with its `stash@{n}` chip, message, branch and relative time.
//! A click selects a stash and fills the detail panel with its files; a file
//! opens its diff through the history's commit machinery (H3, H4). The header
//! opens the dialog that stashes the changes. The detail panel's buttons, and a
//! right-click on a row, apply, pop or drop the stash (B2, D94).

use std::ops::Range;

use gm_schema::{Actor, CommandKind, StashEntry};
use gpui::{
    ClickEvent, Context, Div, FontWeight, MouseButton, MouseDownEvent, Stateful, Window, div,
    prelude::*, px, uniform_list,
};

use crate::action_registry::{Action, Args, stash_args};
use crate::model::CommitPage;
use crate::shell_history::{Panel, action_button, commit_panel, now_secs, relative_time};
use crate::stashes::{self, StashList};
use crate::theme;
use crate::view::{Screen, StatusView};
use crate::window_chrome;

const ROW_HEIGHT: f32 = 48.;
const TIME_WIDTH: f32 = 96.;
/// The longest a branch name is shown before it is cut.
const BRANCH_WIDTH: f32 = 160.;
const EMPTY_HINT: &str =
    "Stash changes… sets the working copy's changes aside, so they can come back later.";

impl StatusView {
    /// The page for the main area: a header with the count and the Stash
    /// button, then the list and the detail panel, or the stacked diff of the
    /// selected stash in their place.
    pub(crate) fn stashes_page(&self, cx: &mut Context<Self>) -> Div {
        let count = self
            .model
            .stashes
            .count()
            .map(stashes::count_label)
            .unwrap_or_default();
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
            .child(div().font_weight(FontWeight::MEDIUM).child("Stashes"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .min_w(px(0.))
                    .child(div().text_xs().text_color(theme::muted()).child(count))
                    .children(
                        self.stash_refusal()
                            .map(|reason| div().text_xs().text_color(theme::muted()).child(reason)),
                    )
                    .child(self.stash_button(cx)),
            );
        let body = if self.model.commit_stack(CommitPage::Stashes).is_open() {
            self.commit_file_view(cx)
        } else {
            let selected = self.model.stashes.selected_entry();
            let index = selected.map(|entry| entry.index);
            let actions = selected.map(|entry| self.stash_actions(entry, cx));
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
                        .child(stash_list_column(&self.model.stashes, cx)),
                )
                .child(commit_panel(
                    &self.model.stash_commit,
                    &self.commit_texts,
                    Panel::Stash(index),
                    actions,
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

    /// Why Stash Changes is refused now, from the registry's rule; `None` when it may run.
    fn stash_refusal(&self) -> Option<&'static str> {
        Action::NewStash
            .enabled(&Args::None, &self.action_context(""))
            .err()
    }

    /// The header's Stash button. Refused, it reads muted and its reason is its tooltip.
    fn stash_button(&self, cx: &Context<Self>) -> Stateful<Div> {
        let refusal = self.stash_refusal();
        let button = div()
            .id("stash-new")
            .flex_none()
            .px_2()
            .h(px(24.))
            .flex()
            .items_center()
            .rounded_sm()
            .border_1()
            .border_color(theme::button_border())
            .text_xs()
            .child("Stash changes…")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.run_action(Action::NewStash, &Args::None, window, cx);
                }),
            );
        match refusal {
            None => button.cursor_pointer(),
            Some(reason) => button
                .text_color(theme::muted())
                .tooltip(window_chrome::hint(reason)),
        }
    }

    /// The dialogs of the stash actions: the stash dialog, the drop's, then the
    /// message of a failed apply or pop. No two of them are open at once.
    pub(crate) fn stash_layers(&self, window: &Window, cx: &Context<Self>) -> Vec<Div> {
        self.new_stash_layer(window, cx)
            .into_iter()
            .chain(self.drop_stash_layer(window, cx))
            .chain(self.message_layer(window, cx))
            .collect()
    }

    /// The selected stash's buttons: Apply, Pop and Drop, in that order.
    fn stash_actions(&self, entry: &StashEntry, cx: &Context<Self>) -> Div {
        let target = stash_args(entry);
        let ctx = self.action_context("");
        div().flex().flex_row().gap_2().children(
            [
                (Action::StashApply, "stash-apply", "Apply"),
                (Action::StashPop, "stash-pop", "Pop"),
                (Action::StashDrop, "stash-drop", "Drop"),
            ]
            .map(|(action, id, label)| action_button(action, id, label, &target, &ctx, cx)),
        )
    }

    /// Shows the page: the stash list is read again, and the history's stacked
    /// diff closes, since the diff pane is shared.
    pub(crate) fn show_stashes(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::Stashes;
        self.model.commit_stack.close();
        self.show_commit_texts(cx);
        self.request_stash_list();
        cx.notify();
    }

    /// Asks the engine for the stashes. Nothing is asked before a repository is
    /// open, or while an open is in flight, since the answer would be the old one's.
    pub(crate) fn request_stash_list(&mut self) {
        if self.model.status().is_none() || self.model.is_opening() {
            return;
        }
        let id = self.engine.dispatch(Actor::Human, CommandKind::ListStashes);
        self.model.stashes.request(id);
    }

    /// Selects the stash with commit id `oid` and asks for its detail. Selecting
    /// another stash supersedes the read in flight, as a commit does.
    pub(crate) fn select_stash(&mut self, oid: &str, cx: &mut Context<Self>) {
        if !self.model.stashes.select(oid) {
            return;
        }
        self.model.stash_stack.close();
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::LoadCommitDetail {
                oid: oid.to_owned(),
            },
        );
        self.model.stash_commit.request(id);
        cx.notify();
    }
}

/// The list column: the stashes, or the one line that says why there are none.
fn stash_list_column(list: &StashList, cx: &mut Context<StatusView>) -> Div {
    let failure = list.failure().map(|text| {
        div()
            .px_4()
            .py_2()
            .text_sm()
            .text_color(theme::error())
            .child(text.to_owned())
    });
    let body = if list.is_loaded() && list.entries().is_empty() {
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .child("No stashes yet."),
            )
            .child(div().text_sm().text_color(theme::muted()).child(EMPTY_HINT))
            .into_any_element()
    } else if list.is_loaded() {
        stash_rows(list.count().unwrap_or_default(), cx).into_any_element()
    } else if let Some(text) = list.failure() {
        div()
            .px_4()
            .py_3()
            .text_color(theme::error())
            .child(text.to_owned())
            .into_any_element()
    } else {
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::muted())
            .child("Loading stashes…")
            .into_any_element()
    };
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .children(failure.filter(|_| list.is_loaded()))
        .child(body)
}

/// The rows, read from the list as it is when they are drawn.
fn stash_rows(count: usize, cx: &mut Context<StatusView>) -> Div {
    div().flex_1().min_h(px(0.)).flex().flex_col().child(
        uniform_list(
            "stash-rows",
            count,
            cx.processor(
                move |this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut gpui::Window,
                      cx: &mut Context<StatusView>| {
                    let now = now_secs();
                    let selected = this.model.stashes.selected();
                    this.model
                        .stashes
                        .entries()
                        .get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(entry, index)| {
                            let is_selected = selected == Some(entry.oid.as_str());
                            stash_row(entry, index, now, is_selected, cx)
                        })
                        .collect()
                },
            ),
        )
        .flex_1()
        .min_h(px(0.)),
    )
}

/// One stash: its `stash@{n}` chip, its message, the branch it was made on and
/// its relative time. A click selects it.
fn stash_row(
    entry: &StashEntry,
    index: usize,
    now: i64,
    selected: bool,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let oid = entry.oid.clone();
    let (menu_index, menu_oid) = (entry.index, entry.oid.clone());
    div()
        .id(("stash-row", index))
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
            this.select_stash(&oid, cx);
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.open_stash_menu(menu_index, &menu_oid, event.position, cx);
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
                .child(stashes::label(entry.index)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(entry.message.clone()),
        )
        .children(entry.branch.clone().map(|branch| {
            div()
                .flex_none()
                .max_w(px(BRANCH_WIDTH))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_xs()
                .text_color(theme::muted())
                .child(branch)
        }))
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
