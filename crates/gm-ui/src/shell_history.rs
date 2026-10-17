//! The history page (docs/07, H1 and H2): the commits of HEAD, newest first, one
//! 48 px row each, with its ref chips, subject, short id and relative time. The
//! next page loads as the list nears its end; a click selects a row by its id.
//! Scope, graph and search come in later sub-slices.

use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use gm_schema::{CommitSummary, RefKind, RefLabel};
use gpui::{
    Context, Div, FontWeight, MouseButton, MouseDownEvent, Stateful, Window, div, prelude::*, px,
    uniform_list,
};

use crate::model::HistoryList;
use crate::theme;
use crate::view::StatusView;

const ROW_HEIGHT: f32 = 48.;
const TIME_WIDTH: f32 = 96.;
/// The next page is asked for when the rows drawn reach this close to the end.
const PAGE_PREFETCH: usize = 20;

impl StatusView {
    /// The page for the main area: a header, then the list, or the one line
    /// that says why there is no list.
    pub(crate) fn history_page(&self, cx: &mut Context<Self>) -> Div {
        let body = match &self.model.history {
            HistoryList::Idle => message("Loading history…"),
            HistoryList::Failed => {
                message("The history could not be read. The status bar says why.")
            }
            HistoryList::Shown(listing) if !listing.loaded => message("Loading history…"),
            HistoryList::Shown(listing) if listing.history.unborn => {
                message("HEAD points at a branch with no commits yet.")
            }
            HistoryList::Shown(listing) if listing.history.commits.is_empty() => {
                message("No commits to show.")
            }
            HistoryList::Shown(listing) => history_list(listing.history.commits.len(), cx),
        };
        let count = match &self.model.history {
            HistoryList::Shown(listing) if listing.loaded => {
                count_label(listing.history.commits.len(), listing.history.more)
            }
            _ => String::new(),
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(theme::ground())
            .child(
                div()
                    .h(px(44.))
                    .px_4()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(div().font_weight(FontWeight::MEDIUM).child("History"))
                    .child(div().text_xs().text_color(theme::muted()).child(count)),
            )
            .child(body)
    }
}

fn message(text: &'static str) -> Div {
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme::muted())
        .child(text)
}

/// The rows, read from the listing as it is when they are drawn.
fn history_list(count: usize, cx: &mut Context<StatusView>) -> Div {
    div().flex_1().min_h(px(0.)).flex().flex_col().child(
        uniform_list(
            "history-commits",
            count,
            cx.processor(
                move |this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut Window,
                      cx: &mut Context<StatusView>| {
                    if range.end + PAGE_PREFETCH >= count {
                        this.request_history_page();
                    }
                    let HistoryList::Shown(listing) = &this.model.history else {
                        return Vec::new();
                    };
                    let now = now_secs();
                    let selected = listing.selected.as_deref();
                    listing
                        .history
                        .commits
                        .get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(commit, index)| {
                            let is_selected = selected == Some(commit.oid.as_str());
                            commit_row(commit, index, now, is_selected, cx)
                        })
                        .collect()
                },
            ),
        )
        .flex_1()
        .min_h(px(0.)),
    )
}

fn commit_row(
    commit: &CommitSummary,
    index: usize,
    now: i64,
    selected: bool,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let oid = commit.oid.clone();
    div()
        .id(index)
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
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                this.select_commit(&oid, cx);
            }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .overflow_hidden()
                .children(commit.refs.iter().map(ref_chip))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(commit.subject.clone()),
                ),
        )
        .child(
            div()
                .flex_none()
                .font_family(theme::code_font())
                .text_xs()
                .text_color(theme::muted())
                .child(commit.short.clone()),
        )
        .child(
            div()
                .flex_none()
                .w(px(TIME_WIDTH))
                .text_xs()
                .text_color(theme::muted())
                .text_right()
                .child(relative_time(now, commit.author_time)),
        )
}

/// A ref as a small chip: HEAD in the accent colour, branches in the selection
/// colour, tags muted.
fn ref_chip(label: &RefLabel) -> Div {
    let (background, text) = match label.kind {
        RefKind::Head => (theme::accent(), theme::on_accent()),
        RefKind::Branch => (theme::selection(), theme::text()),
        RefKind::Tag => (theme::selected_row(), theme::muted()),
    };
    div()
        .flex_none()
        .px_1p5()
        .rounded_sm()
        .text_xs()
        .bg(background)
        .text_color(text)
        .child(label.name.clone())
}

impl StatusView {
    /// Selects the commit `oid`; the selection survives a refresh of the list.
    pub(crate) fn select_commit(&mut self, oid: &str, cx: &mut Context<Self>) {
        self.model.history.select(oid);
        cx.notify();
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
}

/// The count in the header: "1 commit", "12 commits", or "First N commits"
/// while more pages follow.
#[must_use]
pub fn count_label(count: usize, more: bool) -> String {
    if more {
        format!("First {count} commits")
    } else if count == 1 {
        "1 commit".to_owned()
    } else {
        format!("{count} commits")
    }
}

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const WEEK: i64 = 7 * DAY;
const MONTH: i64 = 30 * DAY;
const YEAR: i64 = 365 * DAY;

/// How long ago `then` was, as the list shows it in English. A time in the
/// future (clock skew) reads as "just now".
#[must_use]
pub fn relative_time(now: i64, then: i64) -> String {
    let age = (now - then).max(0);
    if age < MINUTE {
        "just now".to_owned()
    } else if age < HOUR {
        format!("{} min ago", age / MINUTE)
    } else if age < DAY {
        format!("{} h ago", age / HOUR)
    } else if age < 2 * DAY {
        "yesterday".to_owned()
    } else if age < WEEK {
        ago(age / DAY, "day")
    } else if age < MONTH {
        ago(age / WEEK, "week")
    } else if age < YEAR {
        ago(age / MONTH, "month")
    } else {
        ago(age / YEAR, "year")
    }
}

fn ago(count: i64, unit: &str) -> String {
    if count == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{count} {unit}s ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_times_are_in_minutes_and_hours() {
        let now = 1_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now, now - 59), "just now");
        assert_eq!(relative_time(now, now - 5 * MINUTE), "5 min ago");
        assert_eq!(relative_time(now, now - 3 * HOUR), "3 h ago");
    }

    #[test]
    fn a_day_is_yesterday_and_then_counted() {
        let now = 1_000_000;
        assert_eq!(relative_time(now, now - DAY - HOUR), "yesterday");
        assert_eq!(relative_time(now, now - 2 * DAY), "2 days ago");
        assert_eq!(relative_time(now, now - 6 * DAY), "6 days ago");
    }

    #[test]
    fn weeks_months_and_years_are_singular_at_one() {
        let now = 1_000_000_000;
        assert_eq!(relative_time(now, now - WEEK), "1 week ago");
        assert_eq!(relative_time(now, now - 3 * WEEK), "3 weeks ago");
        assert_eq!(relative_time(now, now - MONTH), "1 month ago");
        assert_eq!(relative_time(now, now - 2 * YEAR), "2 years ago");
    }

    #[test]
    fn a_time_in_the_future_reads_as_just_now() {
        assert_eq!(relative_time(100, 500), "just now");
    }

    #[test]
    fn the_count_says_when_the_list_is_cut() {
        assert_eq!(count_label(0, false), "0 commits");
        assert_eq!(count_label(1, false), "1 commit");
        assert_eq!(count_label(42, false), "42 commits");
        assert_eq!(count_label(200, true), "First 200 commits");
        assert_eq!(count_label(200, false), "200 commits");
    }
}
