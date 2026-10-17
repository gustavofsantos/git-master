//! The history page (docs/07, H1 to H3): the commits of HEAD, newest first, one
//! 48 px row each, with its ref chips, subject, short id and relative time. The
//! next page loads as the list nears its end; a click selects a row by its id
//! and fills the detail panel on the right. Scope, graph and search come in
//! later sub-slices.

use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use gm_schema::{
    Actor, Change, CommandKind, CommitDetail, CommitFile, CommitSummary, RefKind, RefLabel,
};
use gpui::{
    ClickEvent, Context, Div, FontWeight, Hsla, MouseButton, MouseDownEvent, Stateful, Window, div,
    prelude::*, px, uniform_list,
};

use crate::model::{CommitPane, HistoryList};
use crate::shell_list::change_letter;
use crate::theme;
use crate::view::StatusView;

const ROW_HEIGHT: f32 = 48.;
const TIME_WIDTH: f32 = 96.;
const DETAIL_WIDTH: f32 = 360.;
/// The next page is asked for when the rows drawn reach this close to the end.
const PAGE_PREFETCH: usize = 20;

impl StatusView {
    /// The page for the main area: a header, then the list, or the one line
    /// that says why there is no list.
    pub(crate) fn history_page(&self, cx: &mut Context<Self>) -> Div {
        let list = match &self.model.history {
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
            .child(if self.model.commit_stack.is_open() {
                self.commit_file_view(cx)
            } else {
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
                            .child(list),
                    )
                    .child(commit_panel(&self.model.commit, cx))
            })
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

/// The detail panel (H3): the selected commit's subject, body, hash, author,
/// date, parents and files. A file row opens its diff (H4).
fn commit_panel(pane: &CommitPane, cx: &mut Context<StatusView>) -> Stateful<Div> {
    let content = match pane {
        CommitPane::Empty => message_text("Select a commit to see its details.").into_any_element(),
        CommitPane::Loading(_) => message_text("Loading commit…").into_any_element(),
        CommitPane::Failed(text) => div()
            .text_color(theme::error())
            .child(text.clone())
            .into_any_element(),
        CommitPane::Shown(detail) => commit_detail(detail, cx).into_any_element(),
    };
    div()
        .id("commit-detail")
        .flex_none()
        .w(px(DETAIL_WIDTH))
        .h_full()
        .min_h(px(0.))
        .overflow_y_scroll()
        .border_l_1()
        .border_color(theme::hairline())
        .bg(theme::panel())
        .p_4()
        .flex()
        .flex_col()
        .gap_3p5()
        .text_sm()
        .child(content)
}

fn message_text(text: &'static str) -> Div {
    div().text_color(theme::muted()).child(text)
}

fn commit_detail(detail: &CommitDetail, cx: &mut Context<StatusView>) -> Div {
    let mut column = div()
        .flex()
        .flex_col()
        .gap_3p5()
        .child(section_label("SELECTED COMMIT"))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_base()
                .child(detail.subject.clone()),
        );
    if !detail.body.is_empty() {
        column = column.child(body_block(&detail.body));
    }
    column
        .child(
            div()
                .grid()
                .grid_cols(2)
                .gap_y_2()
                .text_xs()
                .child(field_label("Hash"))
                .child(
                    div()
                        .font_family(theme::code_font())
                        .child(detail.oid.clone()),
                )
                .child(field_label("Author"))
                .child(author_label(&detail.author_name, &detail.author_email))
                .child(field_label("Date"))
                .child(detail.author_date.clone())
                .child(field_label("Parents"))
                .child(
                    div()
                        .font_family(theme::code_font())
                        .child(parents_label(&detail.parents)),
                ),
        )
        .child(section_label(&format!(
            "FILES ({})",
            detail.files.len() + detail.more_files
        )))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .text_xs()
                .children(
                    detail
                        .files
                        .iter()
                        .enumerate()
                        .map(|(ix, file)| file_row(file, ix, cx)),
                )
                .children((detail.more_files > 0).then(|| {
                    div()
                        .text_color(theme::muted())
                        .child(more_files_label(detail.more_files))
                })),
        )
}

fn section_label(text: &str) -> Div {
    div()
        .text_xs()
        .text_color(theme::muted())
        .child(text.to_owned())
}

fn field_label(text: &'static str) -> Div {
    div().text_color(theme::muted()).child(text)
}

/// The body, one line per row; a blank line is a paragraph break.
fn body_block(body: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .text_xs()
        .line_height(px(17.))
        .children(body.split('\n').map(|line| {
            if line.is_empty() {
                div().h(px(8.))
            } else {
                div().child(line.to_owned())
            }
        }))
}

/// One changed file: its change letter, its path (old to new for a rename) and
/// its line counts. A click opens the file's diff (H4).
fn file_row(file: &CommitFile, ix: usize, cx: &mut Context<StatusView>) -> Stateful<Div> {
    let (letter, color) = (change_letter(file.change), change_color(file.change));
    div()
        .id(("commit-file", ix))
        .cursor_pointer()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(16.))
                .flex_none()
                .text_center()
                .font_family(theme::code_font())
                .text_color(color)
                .child(letter.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(path_label(file)),
        )
        .child(
            div()
                .flex_none()
                .font_family(theme::code_font())
                .text_color(theme::muted())
                .child(file_counts(file)),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_commit_file(ix, window, cx);
        }))
}

pub(crate) fn change_color(change: Change) -> Hsla {
    match change {
        Change::Added => theme::staged(),
        Change::Modified | Change::TypeChanged => theme::unstaged(),
        Change::Deleted => theme::error(),
        Change::Renamed | Change::Copied => theme::untracked(),
    }
}

/// The line counts of a file: `+20 −2`, `+44`, `−3`, `±0`, or `bin` for a binary file.
#[must_use]
pub fn file_counts(file: &CommitFile) -> String {
    if file.binary {
        return "bin".to_owned();
    }
    let mut parts = Vec::new();
    if file.added > 0 {
        parts.push(format!("+{}", file.added));
    }
    if file.deleted > 0 {
        parts.push(format!("\u{2212}{}", file.deleted));
    }
    if parts.is_empty() {
        "\u{b1}0".to_owned()
    } else {
        parts.join(" ")
    }
}

/// The path a row shows: the new path, or `old → new` for a rename.
#[must_use]
pub fn path_label(file: &CommitFile) -> String {
    match &file.old_path {
        Some(old) => format!("{old} \u{2192} {}", file.path),
        None => file.path.clone(),
    }
}

#[must_use]
pub fn more_files_label(count: usize) -> String {
    format!("and {count} more")
}

/// The parents, abbreviated and comma-separated; `none` for a root commit.
#[must_use]
pub fn parents_label(parents: &[String]) -> String {
    if parents.is_empty() {
        "none".to_owned()
    } else {
        parents.join(", ")
    }
}

/// The author as Git writes it: the name, then the email in angle brackets.
#[must_use]
pub fn author_label(name: &str, email: &str) -> String {
    format!("{name} <{email}>")
}

impl StatusView {
    /// Selects the commit `oid` and asks for its detail. The selection survives a
    /// refresh of the list. Selecting another commit supersedes the read in flight.
    pub(crate) fn select_commit(&mut self, oid: &str, cx: &mut Context<Self>) {
        self.model.history.select(oid);
        self.model.commit_stack.close();
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::LoadCommitDetail {
                oid: oid.to_owned(),
            },
        );
        self.model.commit.request(id);
        cx.notify();
    }

    /// Opens the stacked diff of the selected commit, with file `focus` at the top
    /// (H4c). The read is the whole commit's patch, against its first parent.
    pub(crate) fn select_commit_file(&mut self, focus: usize, cx: &mut Context<Self>) {
        let (oid, files, more) = match &self.model.commit {
            CommitPane::Shown(detail) if focus < detail.files.len() => {
                (detail.oid.clone(), detail.files.clone(), detail.more_files)
            }
            _ => return,
        };
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::LoadCommitStack {
                oid,
                files: files.clone(),
            },
        );
        self.model.commit_stack.request(id, focus, files, more);
        cx.notify();
    }

    /// A click on a file row: opens the stack at that file and gives the diff pane the keys.
    pub(crate) fn open_commit_file(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_commit_file(index, cx);
        let handle = self.commit_diff_pane.read(cx).focus_handle();
        window.focus(&handle);
    }

    /// Back to the commit view, from its button or Escape.
    pub(crate) fn close_commit_file(&mut self, cx: &mut Context<Self>) {
        self.model.commit_stack.close();
        cx.notify();
    }

    /// The stacked diff of the commit in place of its list and panel (H4c): a
    /// header with the way back, the commit, then the diff of every file, a
    /// loading line or git's message.
    pub(crate) fn commit_file_view(&self, cx: &mut Context<Self>) -> Div {
        let (short, subject) = match &self.model.commit {
            CommitPane::Shown(detail) => (detail.short.clone(), detail.subject.clone()),
            _ => (String::new(), String::new()),
        };
        let back = div()
            .id("commit-file-back")
            .px_2()
            .h(px(24.))
            .flex()
            .items_center()
            .rounded_sm()
            .border_1()
            .border_color(theme::button_border())
            .cursor_pointer()
            .child("Back to commit")
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_commit_file(cx)));
        let header = div()
            .h(px(36.))
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .min_w(px(0.))
            .border_b_1()
            .border_color(theme::hairline())
            .child(back)
            .child(
                div()
                    .flex_none()
                    .font_family(theme::code_font())
                    .text_color(theme::muted())
                    .child(short),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(subject),
            );
        let body = if let Some(message) = self.model.commit_stack.failure() {
            div()
                .px_4()
                .py_3()
                .text_color(theme::error())
                .child(message.to_owned())
        } else {
            // The pane stays in the tree while the diffs load, so it keeps the
            // keys. Its box is empty until the stack is shown.
            let loading = !self.model.commit_stack.is_shown();
            let pane = div()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .overflow_hidden()
                .when(loading, |this| this.h(px(0.)))
                .when(!loading, gpui::Styled::flex_1)
                .child(self.commit_diff_pane.clone());
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .children(loading.then(|| {
                    div()
                        .px_4()
                        .py_3()
                        .text_color(theme::muted())
                        .child("Loading the commit's files…")
                }))
                .child(pane)
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(header)
            .child(body)
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
    use gm_schema::{Change, CommitFile};

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

    fn file(path: &str, added: u32, deleted: u32, binary: bool) -> CommitFile {
        CommitFile {
            path: path.into(),
            old_path: None,
            change: Change::Modified,
            added,
            deleted,
            binary,
        }
    }

    #[test]
    fn counts_show_the_added_and_deleted_lines() {
        assert_eq!(file_counts(&file("a", 20, 2, false)), "+20 \u{2212}2");
        assert_eq!(file_counts(&file("a", 44, 0, false)), "+44");
        assert_eq!(file_counts(&file("a", 0, 3, false)), "\u{2212}3");
        assert_eq!(file_counts(&file("a", 0, 0, false)), "\u{b1}0");
    }

    #[test]
    fn a_binary_file_says_bin_instead_of_counts() {
        assert_eq!(file_counts(&file("logo.png", 0, 0, true)), "bin");
    }

    #[test]
    fn a_rename_shows_the_old_path_before_the_new_one() {
        let renamed = CommitFile {
            path: "src/new.rs".into(),
            old_path: Some("src/old.rs".into()),
            change: Change::Renamed,
            added: 0,
            deleted: 0,
            binary: false,
        };
        assert_eq!(path_label(&renamed), "src/old.rs \u{2192} src/new.rs");
        assert_eq!(
            path_label(&file("api/routes.rs", 1, 0, false)),
            "api/routes.rs"
        );
    }

    #[test]
    fn the_list_says_how_many_files_it_leaves_out() {
        assert_eq!(more_files_label(50), "and 50 more");
        assert_eq!(more_files_label(1), "and 1 more");
    }

    #[test]
    fn parents_are_listed_short_and_a_root_has_none() {
        assert_eq!(parents_label(&[]), "none");
        assert_eq!(
            parents_label(&["6b0b1eb".into(), "41a9853".into()]),
            "6b0b1eb, 41a9853"
        );
    }

    #[test]
    fn the_author_shows_the_name_then_the_email() {
        assert_eq!(
            author_label("Zo\u{eb} \u{dc}nal", "zoe@example.test"),
            "Zo\u{eb} \u{dc}nal <zoe@example.test>"
        );
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
