//! The Local review page (B4d, D96, D97): the review comments of the repository,
//! grouped by file under a filter, and the selected comment's detail with its
//! buttons and its Before or After text. The rules are in `local_review`; this file
//! draws them and sends the registry's actions. Its drawing is not tested without
//! a window.

use std::ops::Range;
use std::time::Instant;

use gm_schema::{Actor, CommandKind, ReviewComment, ReviewReply, ReviewStatus};
use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, MouseButton, MouseDownEvent, Stateful, Window, div,
    prelude::*, px, uniform_list,
};

use crate::action_registry::{Action, Args, shortcut_text};
use crate::local_review::{self, Counts, Filter, Row, Version};
use crate::review::{self, Badge, DELETE_CONFIRM, Delete};
use crate::review_composer::{self, KEY_CONTEXT, SaveReviewComment};
use crate::shell_history::{action_button, now_secs};
use crate::shell_memories::tag;
use crate::theme;
use crate::view::{Screen, StatusView};

const ROW_HEIGHT: f32 = 52.;
const DETAIL_WIDTH: f32 = 440.;
const LINE_NUMBER_WIDTH: f32 = 44.;
const LOADING_LIST: &str = "Loading comments…";
const LOADING_TEXT: &str = "Loading…";
const NO_COMMENTS: &str = "No comments yet.";
const NO_MATCH: &str = "No comments match this filter.";
const NOTHING_SELECTED: &str = "Select a comment to read it.";

impl StatusView {
    /// The page for the main area: a header with the filter chips and their counts,
    /// then the list grouped by file and the selected comment's detail.
    pub(crate) fn local_review_page(&self, cx: &mut Context<Self>) -> Div {
        let counts = self.model.review.comments().map(local_review::counts);
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
            .child(div().font_weight(FontWeight::MEDIUM).child("Local review"))
            .child(filter_chips(self.model.local_review.filter(), counts, cx));
        let body = div()
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
                    .child(self.review_list(cx)),
            )
            .child(self.review_detail(cx));
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

    /// The list column: the failure of the last read, then the rows, or the one line
    /// that says why there are none.
    fn review_list(&self, cx: &mut Context<Self>) -> Div {
        let failure = self.model.review.failure().map(|text| {
            div()
                .px_4()
                .py_2()
                .text_sm()
                .text_color(theme::error())
                .child(text.to_owned())
        });
        let body = match self.model.review.comments() {
            None => centred_text(LOADING_LIST, theme::muted()).into_any_element(),
            Some([]) => {
                let shortcut = Action::CommentOnSelection
                    .shortcut_label()
                    .unwrap_or_default();
                empty_state(NO_COMMENTS, Some(local_review::empty_hint(&shortcut)))
                    .into_any_element()
            }
            Some(all) => {
                let rows = local_review::rows(all, self.model.local_review.filter());
                if rows.is_empty() {
                    empty_state(NO_MATCH, None).into_any_element()
                } else {
                    review_rows(rows, cx).into_any_element()
                }
            }
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .children(failure)
            .child(body)
    }

    /// The detail panel: the selected comment, or a line that says none is selected.
    fn review_detail(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let panel = div()
            .id("review-detail")
            .w(px(DETAIL_WIDTH))
            .flex_none()
            .h_full()
            .min_h(px(0.))
            .overflow_y_scroll()
            .border_l_1()
            .border_color(theme::hairline())
            .p_4();
        let selected = self
            .model
            .local_review
            .selected_id()
            .and_then(|id| self.model.review.comment(id));
        match selected {
            None => panel.child(div().text_color(theme::muted()).child(NOTHING_SELECTED)),
            Some(comment) => panel.child(self.comment_detail(comment, cx)),
        }
    }

    /// One comment in full: its badge, range and time, the path, the body, the
    /// buttons, then the Before | After toggle and the text of the version shown.
    fn comment_detail(&self, comment: &ReviewComment, cx: &mut Context<Self>) -> Div {
        let (start, end) = review::shown_range(comment);
        let (label, colour) = badge_text(review::badge(comment));
        let head = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().flex_none().text_color(colour).child(label))
                    .child(chips_row(&review::chips(comment)))
                    .child(div().text_xs().text_color(theme::muted()).child(format!(
                        "{} · {}",
                        review::range_label(start, end),
                        review::made_label(now_secs(), comment.created_at)
                    ))),
            )
            .child(
                div()
                    .font_family(theme::code_font())
                    .text_sm()
                    .child(comment.path.clone()),
            );
        let body = div()
            .flex()
            .flex_col()
            .text_sm()
            .text_color(theme::text())
            .children(comment.body.lines().map(|line| {
                div().min_h(px(16.)).child(if line.is_empty() {
                    " ".to_owned()
                } else {
                    line.to_owned()
                })
            }));
        let version = self.model.local_review.version();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(head)
            .child(body)
            .child(self.thread_block(comment, cx))
            .child(self.reply_area(comment, cx))
            .child(self.comment_actions(comment, cx))
            .child(version_toggle(version, cx))
            .child(self.version_text(comment, version))
    }

    /// The replies under the comment body (D198): the thread as the model shows it, and
    /// the toggle that shows or folds the earlier replies above them.
    fn thread_block(&self, comment: &ReviewComment, cx: &mut Context<Self>) -> Div {
        let id = comment.id;
        let expanded = self.model.local_review.thread_open(id);
        let thread = review::thread(&comment.replies, expanded);
        let now = now_secs();
        let toggle = thread.toggle.map(|toggle| {
            div()
                .id((
                    "review-thread-toggle",
                    u64::try_from(id).unwrap_or_default(),
                ))
                .text_xs()
                .text_color(theme::accent())
                .cursor_pointer()
                .child(toggle.label())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.model.local_review.toggle_thread(id);
                        cx.notify();
                    }),
                )
        });
        div().flex().flex_col().gap_2().children(toggle).children(
            thread
                .shown
                .into_iter()
                .map(|reply| reply_block(reply, now)),
        )
    }

    /// The Reply box under the thread (D198) while it is open for this comment, else the
    /// Reply button. Nothing on a resolved comment: its Reopen comes first.
    fn reply_area(&self, comment: &ReviewComment, cx: &mut Context<Self>) -> Div {
        let id = comment.id;
        if !review::can_reply(comment.status) {
            return div();
        }
        let Some(reply) = self.review_reply.as_ref().filter(|reply| reply.id == id) else {
            let button = div()
                .id(("review-reply-open", u64::try_from(id).unwrap_or_default()))
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
                .cursor_pointer()
                .child("Reply")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.open_inline_reply(id, window, cx);
                    }),
                );
            return div().flex().flex_row().child(button);
        };
        let pending = self.model.review_write.is_pending();
        let text = reply.body.read(cx).text().to_owned();
        let can_send = review_composer::may_save(pending, &text);
        let send = div()
            .id("review-reply-send")
            .px_3()
            .h(px(26.))
            .flex()
            .items_center()
            .rounded_md()
            .bg(theme::accent())
            .text_color(theme::on_accent())
            .font_weight(FontWeight::MEDIUM)
            .when(!can_send, |this| this.opacity(0.5))
            .when(can_send, gpui::Styled::cursor_pointer)
            .child(if pending { "Sending…" } else { "Reply" })
            .when(can_send, |this| {
                this.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.submit_inline_reply(cx);
                    }),
                )
            });
        let cancel = div()
            .id("review-reply-cancel")
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
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.close_inline_reply(cx);
                }),
            );
        div()
            .flex()
            .flex_col()
            .gap_2()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &SaveReviewComment, _, cx| {
                this.submit_inline_reply(cx);
            }))
            .child(reply.body.clone())
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(format!("{} sends · Escape cancels", shortcut_text("⌘↵"))),
            )
            .children(reply.error.as_ref().map(|error| {
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(error.clone())
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .items_center()
                    .gap_2()
                    .child(cancel)
                    .child(send),
            )
    }

    /// The buttons of the selected comment, each run through the registry: the same
    /// rules and reasons as the cards and the palette. Delete asks once, inline.
    fn comment_actions(&self, comment: &ReviewComment, cx: &mut Context<Self>) -> Div {
        let target = Args::ReviewComment(comment.id);
        let ctx = self.action_context("");
        let resolved = comment.status == ReviewStatus::Resolved;
        let resolve = if resolved {
            Action::ReopenReviewComment
        } else {
            Action::ResolveReviewComment
        };
        let resolve_label = review::resolve_label(resolved, review::is_note(comment));
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_2()
            .child(action_button(
                Action::OpenReviewComment,
                "review-open-diff",
                "Open in diff",
                &target,
                &ctx,
                cx,
            ))
            .child(action_button(
                resolve,
                "review-toggle",
                resolve_label,
                &target,
                &ctx,
                cx,
            ))
            .child(action_button(
                Action::EditReviewComment,
                "review-edit-page",
                "Edit",
                &target,
                &ctx,
                cx,
            ))
            .child(self.delete_button(comment.id, cx))
    }

    /// Delete: the first click says "Click again to delete" for a few seconds, the
    /// second deletes. The label goes back when the window of the first click is over.
    fn delete_button(&self, id: i64, cx: &mut Context<Self>) -> Stateful<Div> {
        let armed = self.model.local_review.delete_armed(id, Instant::now());
        let label = if armed {
            "Click again to delete"
        } else {
            "Delete"
        };
        div()
            .id("review-delete-page")
            .flex_none()
            .px_2()
            .h(px(24.))
            .flex()
            .items_center()
            .rounded_sm()
            .border_1()
            .border_color(theme::button_border())
            .text_xs()
            .text_color(theme::removed_fg())
            .cursor_pointer()
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.press_review_delete(id, window, cx);
                }),
            )
    }

    /// The text of the version shown: its lines, numbered, with the lines that differ
    /// from the other version marked, or the line that says why there are none.
    fn version_text(&self, comment: &ReviewComment, version: Version) -> Div {
        let page = &self.model.local_review;
        let caption = div()
            .text_xs()
            .text_color(theme::muted())
            .child(local_review::caption(version, comment));
        let text = match page.lines(version) {
            None => muted_line(LOADING_TEXT),
            Some([]) => muted_line(local_review::empty_text(version, comment)),
            Some(lines) => {
                let highlight = match version {
                    Version::Before => theme::removed_bg(),
                    Version::After => theme::added_bg(),
                };
                region_lines(
                    lines,
                    page.first_line(comment, version),
                    &page.marks(version),
                    highlight,
                )
            }
        };
        div().flex().flex_col().gap_2().child(caption).child(text)
    }

    /// Shows the page: the list is read again, and the selected comment's texts with it.
    pub(crate) fn show_local_review(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::LocalReview;
        self.request_review_list();
        self.request_review_texts();
        cx.notify();
    }

    /// Asks the engine for the Before and the After text of the selected comment: one
    /// read of its two snapshots once an agent has handled it (D140), else its snapshot
    /// and the file now. The answers are kept only for the read in flight, so nothing is
    /// asked with no selection.
    pub(crate) fn request_review_texts(&mut self) {
        let Some(id) = self.model.local_review.selected_id() else {
            return;
        };
        if self
            .model
            .review
            .comment(id)
            .is_some_and(local_review::reads_change)
        {
            let change = self
                .engine
                .dispatch(Actor::Human, CommandKind::LoadReviewChange { id });
            self.model.local_review.expect_change(change);
            return;
        }
        let before = self
            .engine
            .dispatch(Actor::Human, CommandKind::LoadReviewSnapshotText { id });
        let after = self
            .engine
            .dispatch(Actor::Human, CommandKind::LoadReviewCurrentText { id });
        self.model.local_review.expect(before, after);
    }

    /// Selects the comment with `id`, which opens on its default version and asks for its texts.
    pub(crate) fn select_review_comment(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(comment) = self.model.review.comment(id) else {
            return;
        };
        // A reply box typed under another comment does not follow the selection.
        if self
            .review_reply
            .as_ref()
            .is_some_and(|reply| reply.id != id)
        {
            self.review_reply = None;
        }
        if self.model.local_review.select(comment) {
            self.request_review_texts();
        }
        cx.notify();
    }

    pub(crate) fn set_review_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.model.local_review.set_filter(filter);
        cx.notify();
    }

    pub(crate) fn set_review_version(&mut self, version: Version, cx: &mut Context<Self>) {
        self.model.local_review.set_version(version);
        cx.notify();
    }

    /// Delete's click: the first asks, the second runs the registry's delete.
    pub(crate) fn press_review_delete(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.model.local_review.press_delete(id, Instant::now()) {
            Delete::Confirmed => self.run_action(
                Action::DeleteReviewComment,
                &Args::ReviewComment(id),
                window,
                cx,
            ),
            Delete::Ask => cx
                .spawn(async move |this, cx| {
                    cx.background_executor().timer(DELETE_CONFIRM).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach(),
        }
        cx.notify();
    }

    /// Open Comment in Diff: the Working Copy on the comment's file, its unstaged side,
    /// with the cursor on the comment's first line once that file's diff is there.
    pub(crate) fn show_review_in_diff(&mut self, path: &str, line: u32, cx: &mut Context<Self>) {
        self.screen = Screen::WorkingCopy;
        if let Some(row) = self
            .model
            .entries()
            .iter()
            .position(|entry| entry.path == path)
        {
            self.model.select_row(row);
        }
        self.diff_pane
            .update(cx, |pane, cx| pane.reveal_line(path, line, cx));
        self.load_selected(cx);
        cx.notify();
    }
}

/// One reply of a thread (D198): its author and time over its plain-text body. An agent's
/// reply carries the Agent-written flag and an accent bar; the person's a plain one.
fn reply_block(reply: &ReviewReply, now: i64) -> Div {
    let agent = review::is_agent_reply(reply);
    let bar = if agent {
        theme::accent()
    } else {
        theme::hairline()
    };
    let head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .text_xs()
        .text_color(theme::muted())
        .children(agent.then(|| tag(review::AGENT_WRITTEN, theme::selected_row())))
        .child(review::reply_head(&reply.author, reply.created_at, now));
    let body = div()
        .flex()
        .flex_col()
        .text_sm()
        .text_color(theme::text())
        .children(reply.body.lines().map(|line| {
            div().min_h(px(16.)).child(if line.is_empty() {
                " ".to_owned()
            } else {
                line.to_owned()
            })
        }));
    div()
        .flex()
        .flex_col()
        .gap_1()
        .pl_3()
        .border_l_2()
        .border_color(bar)
        .child(head)
        .child(body)
}

/// The filter chips, each with its count once the listing is read.
fn filter_chips(active: Filter, counts: Option<Counts>, cx: &mut Context<StatusView>) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .min_w(px(0.))
        .children(Filter::ALL.map(|filter| {
            let label = match counts {
                Some(counted) => format!("{} {}", filter.label(), counted.of(filter)),
                None => filter.label().to_owned(),
            };
            filter_chip(filter, label, filter == active, cx)
        }))
}

fn filter_chip(
    filter: Filter,
    label: String,
    active: bool,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    div()
        .id(filter.label())
        .flex_none()
        .px_2()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded_full()
        .text_xs()
        .cursor_pointer()
        .when(active, |chip| {
            chip.bg(theme::selection()).text_color(theme::accent())
        })
        .when(!active, |chip| {
            chip.text_color(theme::muted())
                .hover(|style| style.bg(theme::selected_row()))
        })
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.set_review_filter(filter, cx);
            }),
        )
}

/// The rows of the list, read from the rows as they are drawn. A file's heading and a
/// comment both take one row height, which the list needs to be virtual.
fn review_rows(rows: Vec<Row>, cx: &mut Context<StatusView>) -> Div {
    let count = rows.len();
    let now = now_secs();
    div().flex_1().min_h(px(0.)).flex().flex_col().child(
        uniform_list(
            "review-rows",
            count,
            cx.processor(
                move |this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut Window,
                      cx: &mut Context<StatusView>| {
                    let selected = this.model.local_review.selected_id();
                    rows.get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(row, index)| review_row(index, row, selected, now, cx))
                        .collect()
                },
            ),
        )
        .flex_1()
        .min_h(px(0.)),
    )
}

/// One row: a file's heading with its count, or a comment with its range, its badge,
/// its time and the first line of its body. A click on a comment selects it.
fn review_row(
    index: usize,
    row: &Row,
    selected: Option<i64>,
    now: i64,
    cx: &mut Context<StatusView>,
) -> AnyElement {
    match row {
        Row::File { path, count } => div()
            .id(("review-file", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .border_b_1()
            .border_color(theme::hairline())
            .bg(theme::panel())
            .child(
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(theme::code_font())
                    .text_sm()
                    .child(path.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(comments_label(*count)),
            )
            .into_any_element(),
        Row::Comment {
            id,
            lines,
            summary,
            badge,
            chips,
            created_at,
        } => {
            let id = *id;
            let (label, colour) = badge_text(*badge);
            div()
                .id(("review-comment", index))
                .w_full()
                .h(px(ROW_HEIGHT))
                .px_4()
                .pl_6()
                .flex()
                .flex_col()
                .justify_center()
                .gap_0p5()
                .border_b_1()
                .border_color(theme::hairline())
                .cursor_pointer()
                .when(selected == Some(id), |row| row.bg(theme::selected_row()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.select_review_comment(id, cx);
                    }),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .min_w(px(0.))
                        .child(div().flex_none().text_xs().text_color(colour).child(label))
                        .child(chips_row(chips))
                        .child(
                            div()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(format!(
                                    "{lines} · {}",
                                    review::made_label(now, *created_at)
                                )),
                        ),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_sm()
                        .child(summary.clone()),
                )
                .into_any_element()
        }
    }
}

/// The Before | After toggle: the version shown is filled.
fn version_toggle(active: Version, cx: &mut Context<StatusView>) -> Div {
    div()
        .flex()
        .flex_row()
        .gap_1()
        .child(version_segment(Version::Before, active, cx))
        .child(version_segment(Version::After, active, cx))
}

fn version_segment(
    version: Version,
    active: Version,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let selected = version == active;
    div()
        .id(version.label())
        .px_3()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(theme::button_border())
        .text_xs()
        .cursor_pointer()
        .when(selected, |segment| {
            segment
                .bg(theme::selection())
                .text_color(theme::accent())
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!selected, |segment| segment.text_color(theme::muted()))
        .child(version.label())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.set_review_version(version, cx);
            }),
        )
}

/// The lines of a text, numbered from `first`, the marked ones on `highlight`.
fn region_lines(lines: &[String], first: u32, marks: &[bool], highlight: Hsla) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded_sm()
        .border_1()
        .border_color(theme::hairline())
        .overflow_hidden()
        .font_family(theme::code_font())
        .text_xs()
        .children(lines.iter().enumerate().map(|(index, text)| {
            let marked = marks.get(index).copied().unwrap_or(false);
            let number = first.saturating_add(u32::try_from(index).unwrap_or(u32::MAX));
            div()
                .flex()
                .flex_row()
                .min_h(px(18.))
                .when(marked, |line| line.bg(highlight))
                .child(
                    div()
                        .flex_none()
                        .w(px(LINE_NUMBER_WIDTH))
                        .px_2()
                        .text_color(theme::muted())
                        .text_right()
                        .child(number.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(if text.is_empty() {
                            " ".to_owned()
                        } else {
                            text.clone()
                        }),
                )
        }))
}

/// The chips of a root beside its status (D200): the Agent-written flag, the kind and
/// the severity words, and the Needs answer badge in the accent colour.
pub(crate) fn chips_row(chips: &review::Chips) -> Div {
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .text_xs()
        .text_color(theme::muted())
        .children(
            chips
                .agent
                .then(|| tag(review::AGENT_WRITTEN, theme::selected_row())),
        )
        .children(chips.kind.iter().map(|word| {
            let (background, colour) = if chips.tone == review::Tone::Warning {
                (theme::warn_bg(), theme::warn_fg())
            } else {
                (theme::selected_row(), theme::muted())
            };
            tag(word, background).text_color(colour)
        }))
        .children(
            chips
                .needs_answer
                .then(|| tag(review::NEEDS_ANSWER, theme::selection()).text_color(theme::accent())),
        )
}

fn badge_text(badge: Badge) -> (&'static str, Hsla) {
    match badge {
        Badge::Open => ("Open", theme::accent()),
        Badge::Handled => ("Handled", theme::accent()),
        Badge::Resolved => ("Resolved", theme::muted()),
        Badge::Outdated => ("Outdated", theme::warn_fg()),
    }
}

fn comments_label(count: usize) -> String {
    if count == 1 {
        "1 comment".to_owned()
    } else {
        format!("{count} comments")
    }
}

fn centred_text(text: &'static str, colour: Hsla) -> Div {
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(colour)
        .child(text)
}

fn muted_line(text: &'static str) -> Div {
    div().text_sm().text_color(theme::muted()).child(text)
}

fn empty_state(title: &'static str, hint: Option<String>) -> Div {
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
                .child(title),
        )
        .children(hint.map(|hint| div().text_sm().text_color(theme::muted()).child(hint)))
}
