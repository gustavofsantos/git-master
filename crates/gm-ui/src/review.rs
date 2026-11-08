//! Review comments in the Working Copy diff (B4c, D96, D97), with no GPUI in it:
//! the comments the engine listed and the read in flight, the write of the
//! composer, where each comment sits among the rows of the diff, and the
//! two-click delete of a card. The diff pane draws the `Entry` list built here.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use gm_schema::{OperationId, OperationState, ReviewComment, ReviewLocation, ReviewStatus};

use crate::diff_rows::{Diffs, Row, SplitRows};
use crate::shell_history::relative_time;

/// The columns a card's body is wrapped to.
pub const WRAP_COLUMNS: usize = 72;
/// How long a second click of delete may come after the first.
pub const DELETE_CONFIRM: Duration = Duration::from_secs(3);
/// How much of the first line a folded card shows before it is cut.
const SUMMARY_COLUMNS: usize = 60;
/// Git's message when a failed read gave none.
const NO_MESSAGE: &str = "The comments could not be read.";
/// Git's message when a failed write gave none.
const NOT_SAVED: &str = "The comment was not saved.";

/// The comments of the open repository and the read that will replace them.
/// Ids follow dispatch order, so an answer older than the one shown, or than the
/// read in flight, is superseded and dropped. A write answers with a listing
/// too, and that listing is at least as fresh, so it is shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewList {
    pending: Option<OperationId>,
    applied: Option<OperationId>,
    comments: Vec<ReviewComment>,
    failure: Option<String>,
}

impl ReviewList {
    /// Starts the read `id`. The list on screen stays until its answer comes.
    pub fn request(&mut self, id: OperationId) {
        self.pending = Some(id);
        self.failure = None;
    }

    /// Whether `id` is the read in flight.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.pending == Some(id)
    }

    /// Applies a listing answered by `id`. False when it is superseded.
    pub fn listed(&mut self, id: OperationId, comments: Vec<ReviewComment>) -> bool {
        if self.applied.is_some_and(|shown| id < shown)
            || self.pending.is_some_and(|wanted| id < wanted)
        {
            return false;
        }
        if self.pending.is_some_and(|wanted| wanted <= id) {
            self.pending = None;
        }
        self.applied = Some(id);
        self.failure = None;
        self.comments = comments;
        true
    }

    /// The read `id` ended. A failure keeps Git's message, or a plain one; a read
    /// that stopped leaves the list as it was.
    pub fn ended(&mut self, id: OperationId, state: OperationState, detail: Option<String>) {
        if !self.owns(id) {
            return;
        }
        if state == OperationState::Failed {
            self.pending = None;
            self.failure = Some(detail.unwrap_or_else(|| NO_MESSAGE.to_owned()));
        } else if state.is_terminal() {
            self.pending = None;
        }
    }

    /// Every comment of the listing on screen; `None` until a listing has been shown.
    #[must_use]
    pub fn comments(&self) -> Option<&[ReviewComment]> {
        self.applied.map(|_| self.comments.as_slice())
    }

    /// The comment with this id, as the listing on screen has it.
    #[must_use]
    pub fn comment(&self, id: i64) -> Option<&ReviewComment> {
        self.comments()
            .and_then(|all| all.iter().find(|c| c.id == id))
    }

    /// Git's message when the newest read failed.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
}

/// The comment write of the composer in flight, and its outcome until the window
/// takes it. Engine events for other operations never touch it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewWrite {
    pending: Option<OperationId>,
    outcome: Option<Result<(), String>>,
}

impl ReviewWrite {
    pub fn start(&mut self, id: OperationId) {
        self.pending = Some(id);
        self.outcome = None;
    }

    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.pending == Some(id)
    }

    /// Ends the write with the operation's terminal `state`.
    pub fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() || self.pending.take().is_none() {
            return;
        }
        self.outcome = Some(if state == OperationState::Done {
            Ok(())
        } else {
            Err(detail.unwrap_or_else(|| NOT_SAVED.to_owned()))
        });
    }

    pub fn take(&mut self) -> Option<Result<(), String>> {
        self.outcome.take()
    }
}

/// The comments of each path, in the listing's order.
#[must_use]
pub fn group_by_path(comments: &[ReviewComment]) -> BTreeMap<&str, Vec<&ReviewComment>> {
    let mut groups: BTreeMap<&str, Vec<&ReviewComment>> = BTreeMap::new();
    for comment in comments {
        groups
            .entry(comment.path.as_str())
            .or_default()
            .push(comment);
    }
    groups
}

/// The working-tree lines a comment is about: where its anchor is now, or where
/// it was when it was made, when it is outdated.
#[must_use]
pub fn shown_range(comment: &ReviewComment) -> (u32, u32) {
    if let Some(location) = comment.location {
        return (location.start_line, location.end_line);
    }
    let start = comment.anchor.start_line;
    let count = comment.anchor.text.split('\n').count();
    let lines = u32::try_from(count).unwrap_or(u32::MAX);
    (start, start.saturating_add(lines.saturating_sub(1)))
}

/// "line 12", or "lines 12–15" for a range of more than one line.
#[must_use]
pub fn range_label(start: u32, end: u32) -> String {
    if start == end {
        format!("line {start}")
    } else {
        format!("lines {start}–{end}")
    }
}

/// The badge of a card: resolved wins, then outdated, then handled, else open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Badge {
    Open,
    Handled,
    Resolved,
    Outdated,
}

#[must_use]
pub fn badge(comment: &ReviewComment) -> Badge {
    if comment.status == ReviewStatus::Resolved {
        Badge::Resolved
    } else if comment.location.is_none() {
        Badge::Outdated
    } else if comment.status == ReviewStatus::Handled {
        Badge::Handled
    } else {
        Badge::Open
    }
}

/// Splits a body into lines of at most `columns` characters, at spaces where it
/// can. A word longer than a line is cut. Empty lines of the body are kept.
#[must_use]
pub fn wrap(body: &str, columns: usize) -> Vec<String> {
    let columns = columns.max(1);
    let mut out = Vec::new();
    for line in body.lines() {
        let mut current = String::new();
        let mut width = 0;
        for word in line.split(' ') {
            let mut word = word.to_owned();
            // A word wider than a line: fill the current line, then cut the word.
            while word.chars().count() > columns {
                if width > 0 {
                    out.push(std::mem::take(&mut current));
                    width = 0;
                }
                let head: String = word.chars().take(columns).collect();
                word = word.chars().skip(columns).collect();
                out.push(head);
            }
            let len = word.chars().count();
            if width == 0 {
                current = word;
                width = len;
            } else if width + 1 + len <= columns {
                current.push(' ');
                current.push_str(&word);
                width += 1 + len;
            } else {
                out.push(std::mem::take(&mut current));
                current = word;
                width = len;
            }
        }
        out.push(current);
    }
    out
}

/// The one line a folded resolved card shows: its range, then the first line of
/// the body, cut to `SUMMARY_COLUMNS` with an ellipsis.
#[must_use]
pub fn folded_summary(comment: &ReviewComment) -> String {
    let (start, end) = shown_range(comment);
    let first = comment.body.lines().next().unwrap_or_default().trim();
    let cut = if first.chars().count() > SUMMARY_COLUMNS {
        let head: String = first.chars().take(SUMMARY_COLUMNS).collect();
        format!("{head}…")
    } else {
        first.to_owned()
    };
    format!("Resolved · {} · {cut}", range_label(start, end))
}

/// One row of the diff list. A diff row keeps its index in the layout's rows; a
/// heading and the lines of a card are drawn by the pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Diff(usize),
    Heading(Heading),
    Card { id: i64, part: Part },
}

/// The strip above a file's rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heading {
    /// The outdated comments of the file; `open` shows their cards.
    Outdated { count: usize, open: bool },
    /// The comments whose lines are not in any hunk of the diff.
    Outside,
}

/// A row of a comment's card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// The top row: the badge, the range, when it was made, and the buttons.
    Head {
        badge: Badge,
        lines: String,
        created_at: i64,
    },
    /// One wrapped line of the body.
    Text(String),
    /// A resolved comment, shown as one line until it is opened.
    Folded(String),
}

/// The card rows of one comment: a folded line when it is resolved and not
/// opened, else its head and its wrapped body.
#[must_use]
pub fn card_entries(comment: &ReviewComment, opened: bool) -> Vec<Entry> {
    let id = comment.id;
    if comment.status == ReviewStatus::Resolved && !opened {
        return vec![Entry::Card {
            id,
            part: Part::Folded(folded_summary(comment)),
        }];
    }
    let (start, end) = shown_range(comment);
    let head = Entry::Card {
        id,
        part: Part::Head {
            badge: badge(comment),
            lines: range_label(start, end),
            created_at: comment.created_at,
        },
    };
    std::iter::once(head)
        .chain(
            wrap(&comment.body, WRAP_COLUMNS)
                .into_iter()
                .map(|line| Entry::Card {
                    id,
                    part: Part::Text(line),
                }),
        )
        .collect()
}

/// The rows of one list: the entries in drawing order, and the entry of each
/// diff row, so the cursor and the scroll can find a row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// The index in `entries` of each diff row.
    pub at: Vec<usize>,
}

/// Lays out `count` diff rows, with `after[row]` drawn under each row and `top`
/// above the first.
fn listing(count: usize, after: &[Vec<Entry>], top: &[Entry]) -> Listing {
    let mut entries = top.to_vec();
    let mut at = Vec::with_capacity(count);
    for (row, blocks) in after.iter().enumerate().take(count) {
        at.push(entries.len());
        entries.push(Entry::Diff(row));
        entries.extend(blocks.iter().cloned());
    }
    Listing { entries, at }
}

/// The drawn list of a file in both layouts, and the working-tree lines an open
/// comment covers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notes {
    pub unified: Listing,
    pub split: Listing,
    /// Lines covered by an open comment; their gutter gets a marker.
    pub marked: BTreeSet<u32>,
}

/// What the pane has opened: whether the outdated strip is open, and which
/// resolved comments are.
#[derive(Clone, Copy, Debug)]
pub struct Opened<'a> {
    pub outdated: bool,
    pub comments: &'a BTreeSet<i64>,
}

/// The number of a diff row's line on the new side: a working-tree line on the
/// unstaged side. Removed lines and the rows of a hunk header have none.
fn new_number(row: Row, diffs: Diffs<'_>) -> Option<u32> {
    let Row::Line {
        side,
        hunk_index,
        line_index,
    } = row
    else {
        return None;
    };
    diffs.hunk(side, hunk_index)?.lines.get(line_index)?.new_no
}

/// The unified row a comment is drawn after: the row of the last line of its
/// range that the diff shows. `None` when no line of the range is in a hunk.
#[must_use]
pub fn anchor_row(rows: &[Row], diffs: Diffs<'_>, location: ReviewLocation) -> Option<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(index, row)| {
            let number = new_number(*row, diffs)?;
            (location.start_line..=location.end_line)
                .contains(&number)
                .then_some((number, index))
        })
        .max()
        .map(|(_, index)| index)
}

/// The first unified row that shows working-tree line `line` on the new side, if
/// any. Used to put the cursor on a comment's first line (B4d).
#[must_use]
pub fn row_of_line(rows: &[Row], diffs: Diffs<'_>, line: u32) -> Option<usize> {
    rows.iter()
        .position(|row| new_number(*row, diffs) == Some(line))
}

/// Builds the drawn list of `path` in both layouts. Each comment's card goes under
/// its anchor row (in the split layout, under the split row that holds it); an
/// outdated comment, or one outside the hunks, goes to the strip above the file.
/// Comments of other paths are left out.
#[must_use]
pub fn notes(
    rows: &[Row],
    diffs: Diffs<'_>,
    split: &SplitRows,
    comments: &[ReviewComment],
    path: &str,
    opened: Opened<'_>,
) -> Notes {
    let mut mine: Vec<&ReviewComment> = group_by_path(comments).remove(path).unwrap_or_default();
    mine.sort_by_key(|comment| (shown_range(comment).0, comment.id));
    let mut unified_after: Vec<Vec<Entry>> = vec![Vec::new(); rows.len()];
    let mut split_after: Vec<Vec<Entry>> = vec![Vec::new(); split.rows.len()];
    let mut outdated = Vec::new();
    let mut outside = Vec::new();
    let mut marked = BTreeSet::new();
    for comment in mine {
        let opened_card = opened.comments.contains(&comment.id);
        let Some(location) = comment.location else {
            outdated.push(comment);
            continue;
        };
        if comment.status == ReviewStatus::Open {
            marked.extend(location.start_line..=location.end_line);
        }
        let Some(row) = anchor_row(rows, diffs, location) else {
            outside.push(comment);
            continue;
        };
        let card = card_entries(comment, opened_card);
        if let Some(slot) = unified_after.get_mut(row) {
            slot.extend(card.iter().cloned());
        }
        if let Some(&split_row) = split.of_unified.get(row)
            && let Some(slot) = split_after.get_mut(split_row)
        {
            slot.extend(card);
        }
    }
    let mut top = Vec::new();
    if !outdated.is_empty() {
        top.push(Entry::Heading(Heading::Outdated {
            count: outdated.len(),
            open: opened.outdated,
        }));
        if opened.outdated {
            top.extend(
                outdated.iter().flat_map(|comment| {
                    card_entries(comment, opened.comments.contains(&comment.id))
                }),
            );
        }
    }
    if !outside.is_empty() {
        top.push(Entry::Heading(Heading::Outside));
        top.extend(
            outside
                .iter()
                .flat_map(|comment| card_entries(comment, opened.comments.contains(&comment.id))),
        );
    }
    Notes {
        unified: listing(rows.len(), &unified_after, &top),
        split: listing(split.rows.len(), &split_after, &top),
        marked,
    }
}

/// What a cursor or a visual selection covers in the rows of the diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Covered {
    /// Whether a changed or context line is covered, not only hunk headers.
    pub has_line: bool,
    /// The working-tree lines covered, lowest to highest; `None` when all are removed.
    pub working: Option<(u32, u32)>,
}

/// The lines of the file that the rows `from` to `to` show: context and added
/// lines in order, without the removed ones, which the file no longer has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picked {
    pub lines: Vec<String>,
    /// Whether those lines follow each other in the file, with no line between
    /// them that the picked rows skipped (a second hunk, say).
    pub contiguous: bool,
}

impl Picked {
    /// The lines joined with `\n`, as the engine looks for them in the file.
    #[must_use]
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// What the rows `from` to `to` (both included, in either order) show of the file.
#[must_use]
pub fn picked(rows: &[Row], diffs: Diffs<'_>, from: usize, to: usize) -> Picked {
    let (lo, hi) = (from.min(to), from.max(to));
    let mut lines = Vec::new();
    let mut contiguous = true;
    let mut last: Option<u32> = None;
    for row in rows.iter().take(hi + 1).skip(lo) {
        let Row::Line {
            side,
            hunk_index,
            line_index,
        } = *row
        else {
            continue;
        };
        let Some(line) = diffs
            .hunk(side, hunk_index)
            .and_then(|hunk| hunk.lines.get(line_index))
        else {
            continue;
        };
        let Some(number) = line.new_no else { continue };
        if last.is_some_and(|before| before.checked_add(1) != Some(number)) {
            contiguous = false;
        }
        last = Some(number);
        lines.push(line.text.clone());
    }
    Picked { lines, contiguous }
}

/// What the rows `from` to `to` (both included, in either order) cover.
#[must_use]
pub fn covered(rows: &[Row], diffs: Diffs<'_>, from: usize, to: usize) -> Covered {
    let (lo, hi) = (from.min(to), from.max(to));
    let mut has_line = false;
    let mut lowest: Option<u32> = None;
    let mut highest: Option<u32> = None;
    for (index, row) in rows.iter().enumerate() {
        if index < lo || index > hi {
            continue;
        }
        has_line |= matches!(row, Row::Line { .. });
        if let Some(number) = new_number(*row, diffs) {
            lowest = Some(lowest.map_or(number, |low| low.min(number)));
            highest = Some(highest.map_or(number, |high| high.max(number)));
        }
    }
    Covered {
        has_line,
        working: lowest.zip(highest),
    }
}

/// What a press of delete does: the first press asks, a second within
/// `DELETE_CONFIRM` of the first, on the same comment, deletes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delete {
    Ask,
    Confirmed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeleteArm {
    armed: Option<(i64, Instant)>,
}

impl DeleteArm {
    pub fn press(&mut self, id: i64, now: Instant) -> Delete {
        if self.is_armed(id, now) {
            self.armed = None;
            Delete::Confirmed
        } else {
            self.armed = Some((id, now));
            Delete::Ask
        }
    }

    /// Whether the card of `id` shows "Click again to delete" at `now`.
    #[must_use]
    pub fn is_armed(&self, id: i64, now: Instant) -> bool {
        self.armed
            .is_some_and(|(armed, at)| armed == id && now.duration_since(at) < DELETE_CONFIRM)
    }

    pub fn clear(&mut self) {
        self.armed = None;
    }
}

/// How long ago a comment was made, as the list shows times.
#[must_use]
pub fn made_label(now_secs: i64, created_at_ms: i64) -> String {
    relative_time(now_secs, created_at_ms / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_rows::{Side, build_rows, split_rows};
    use gm_schema::{DiffLine, FileDiff, Hunk, HunkId, LineKind, ReviewAnchor, ReviewStatus};
    use proptest::prelude::*;

    fn comment(id: i64, path: &str, location: Option<(u32, u32)>, text: &str) -> ReviewComment {
        ReviewComment {
            id,
            path: path.into(),
            body: format!("note {id}"),
            status: ReviewStatus::Open,
            anchor: ReviewAnchor {
                text: text.into(),
                before: Vec::new(),
                after: Vec::new(),
                start_line: location.map_or(1, |(start, _)| start),
            },
            created_at: 1_000_000,
            resolved_at: None,
            replies: Vec::new(),
            snapshot: None,
            after_snapshot: None,
            location: location.map(|(start_line, end_line)| ReviewLocation {
                start_line,
                end_line,
            }),
            outdated: location.is_none(),
        }
    }

    fn resolved(mut value: ReviewComment) -> ReviewComment {
        value.status = ReviewStatus::Resolved;
        value.resolved_at = Some(2);
        value
    }

    fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_no: old,
            new_no: new,
            text: text.into(),
            no_newline: false,
        }
    }

    /// One hunk: context 1, removed 2 (old), added 2 and 3 (new), context 4.
    fn file() -> FileDiff {
        FileDiff {
            path: "a.txt".into(),
            old_path: None,
            binary: false,
            hunks: vec![Hunk {
                id: HunkId("h".into()),
                old_start: 1,
                old_lines: 3,
                new_start: 1,
                new_lines: 4,
                header: String::new(),
                lines: vec![
                    line(LineKind::Context, Some(1), Some(1), "one"),
                    line(LineKind::Removed, Some(2), None, "two"),
                    line(LineKind::Added, None, Some(2), "TWO"),
                    line(LineKind::Added, None, Some(3), "three"),
                    line(LineKind::Context, Some(3), Some(4), "four"),
                ],
            }],
        }
    }

    fn fixture() -> (Vec<Row>, FileDiff, SplitRows) {
        let file = file();
        let diffs = Diffs {
            unstaged: &file.hunks,
            staged: &[],
        };
        let rows = build_rows(diffs, Side::Unstaged);
        let split = split_rows(diffs, Side::Unstaged);
        (rows, file, split)
    }

    fn diffs(file: &FileDiff) -> Diffs<'_> {
        Diffs {
            unstaged: &file.hunks,
            staged: &[],
        }
    }

    fn none() -> BTreeSet<i64> {
        BTreeSet::new()
    }

    #[test]
    fn a_comment_is_drawn_under_the_row_of_the_last_line_it_covers() {
        let (rows, file, _) = fixture();
        // Lines 2 and 3 are the two added rows; the card goes under line 3.
        let at = anchor_row(
            &rows,
            diffs(&file),
            ReviewLocation {
                start_line: 2,
                end_line: 3,
            },
        );
        assert_eq!(at, Some(4));
        // A range that ends on context is drawn under that context row.
        let at = anchor_row(
            &rows,
            diffs(&file),
            ReviewLocation {
                start_line: 1,
                end_line: 4,
            },
        );
        assert_eq!(at, Some(5));
    }

    #[test]
    fn the_row_of_a_working_tree_line_is_found_and_a_removed_or_missing_one_is_not() {
        let (rows, file, _) = fixture();
        assert_eq!(row_of_line(&rows, diffs(&file), 1), Some(1));
        assert_eq!(row_of_line(&rows, diffs(&file), 2), Some(3));
        assert_eq!(row_of_line(&rows, diffs(&file), 3), Some(4));
        assert_eq!(row_of_line(&rows, diffs(&file), 40), None);
    }

    #[test]
    fn a_range_with_no_line_in_a_hunk_has_no_anchor_row() {
        let (rows, file, _) = fixture();
        let at = anchor_row(
            &rows,
            diffs(&file),
            ReviewLocation {
                start_line: 40,
                end_line: 50,
            },
        );
        assert_eq!(at, None);
    }

    #[test]
    fn a_range_ending_past_the_hunk_is_drawn_under_its_last_line_in_the_hunk() {
        let (rows, file, _) = fixture();
        let at = anchor_row(
            &rows,
            diffs(&file),
            ReviewLocation {
                start_line: 4,
                end_line: 9,
            },
        );
        assert_eq!(at, Some(5));
    }

    #[test]
    fn a_comment_card_is_one_head_row_and_one_row_per_body_line() {
        let (rows, file, split) = fixture();
        let mut body = comment(1, "a.txt", Some((2, 3)), "TWO\nthree");
        body.body = "first line\n\nlast".into();
        let notes = notes(
            &rows,
            diffs(&file),
            &split,
            &[body],
            "a.txt",
            Opened {
                outdated: false,
                comments: &none(),
            },
        );
        let card: Vec<&Entry> = notes
            .unified
            .entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Card { .. }))
            .collect();
        assert_eq!(card.len(), 4);
        assert!(matches!(
            card[1],
            Entry::Card { part: Part::Text(text), .. } if text == "first line"
        ));
        assert!(matches!(
            card[2],
            Entry::Card { part: Part::Text(text), .. } if text.is_empty()
        ));
    }

    #[test]
    fn unified_listing_puts_the_card_right_after_its_row_and_maps_each_row() {
        let (rows, file, split) = fixture();
        let note = comment(1, "a.txt", Some((3, 3)), "three");
        let notes = notes(
            &rows,
            diffs(&file),
            &split,
            &[note],
            "a.txt",
            Opened {
                outdated: false,
                comments: &none(),
            },
        );
        let listing = &notes.unified;
        assert_eq!(listing.at.len(), rows.len());
        // Row 4 is the line "three"; its card starts right after it.
        assert_eq!(listing.entries[listing.at[4]], Entry::Diff(4));
        assert!(matches!(
            listing.entries[listing.at[4] + 1],
            Entry::Card {
                id: 1,
                part: Part::Head { .. }
            }
        ));
        assert_eq!(listing.entries[listing.at[5]], Entry::Diff(5));
        assert_eq!(listing.entries.len(), rows.len() + 2);
    }

    #[test]
    fn split_listing_puts_the_card_under_the_split_row_that_holds_the_line() {
        let (rows, file, split) = fixture();
        // Line 2 is the added "TWO" (row 3), paired with the removed "two" (row 2) in one split row.
        let note = comment(1, "a.txt", Some((2, 2)), "TWO");
        let notes = notes(
            &rows,
            diffs(&file),
            &split,
            &[note],
            "a.txt",
            Opened {
                outdated: false,
                comments: &none(),
            },
        );
        let listing = &notes.split;
        let pair = split.of_unified[3];
        assert_eq!(pair, split.of_unified[2]);
        assert_eq!(listing.at.len(), split.rows.len());
        assert!(matches!(
            listing.entries[listing.at[pair] + 1],
            Entry::Card { id: 1, .. }
        ));
        // The unified listing keeps its own mapping.
        assert!(matches!(
            notes.unified.entries[notes.unified.at[3] + 1],
            Entry::Card {
                id: 1,
                part: Part::Head {
                    badge: Badge::Open,
                    ..
                }
            }
        ));
    }

    #[test]
    fn with_no_comments_the_listing_is_the_rows_in_order() {
        let (rows, file, split) = fixture();
        let notes = notes(
            &rows,
            diffs(&file),
            &split,
            &[],
            "a.txt",
            Opened {
                outdated: false,
                comments: &none(),
            },
        );
        assert_eq!(notes.unified.at, (0..rows.len()).collect::<Vec<_>>());
        assert!(notes.marked.is_empty());
        assert!(
            notes
                .unified
                .entries
                .iter()
                .all(|entry| matches!(entry, Entry::Diff(_)))
        );
    }

    #[test]
    fn comments_of_other_paths_are_left_out() {
        let (rows, file, split) = fixture();
        let other = comment(1, "b.txt", Some((3, 3)), "three");
        let notes = notes(
            &rows,
            diffs(&file),
            &split,
            &[other],
            "a.txt",
            Opened {
                outdated: false,
                comments: &none(),
            },
        );
        assert!(notes.marked.is_empty());
        assert_eq!(notes.unified.entries.len(), rows.len());
    }

    #[test]
    fn an_outdated_comment_is_a_strip_above_the_file_that_opens_to_its_cards() {
        let (rows, file, split) = fixture();
        let stale = comment(1, "a.txt", None, "gone\nand gone");
        let closed = notes(
            &rows,
            diffs(&file),
            &split,
            std::slice::from_ref(&stale),
            "a.txt",
            Opened {
                outdated: false,
                comments: &none(),
            },
        );
        assert_eq!(
            closed.unified.entries[0],
            Entry::Heading(Heading::Outdated {
                count: 1,
                open: false
            })
        );
        assert_eq!(closed.unified.entries.len(), rows.len() + 1);
        let open = notes(
            &rows,
            diffs(&file),
            &split,
            &[stale],
            "a.txt",
            Opened {
                outdated: true,
                comments: &none(),
            },
        );
        assert_eq!(
            open.unified.entries[0],
            Entry::Heading(Heading::Outdated {
                count: 1,
                open: true
            })
        );
        assert!(matches!(
            open.unified.entries[1],
            Entry::Card {
                id: 1,
                part: Part::Head {
                    badge: Badge::Outdated,
                    ..
                }
            }
        ));
        // The strip's cards are above the first row, so the first row is still row 0.
        assert_eq!(
            open.unified.at[0],
            open.unified
                .entries
                .iter()
                .position(|e| *e == Entry::Diff(0))
                .unwrap()
        );
    }

    #[test]
    fn a_comment_outside_the_hunks_goes_to_the_strip_under_its_heading() {
        let (rows, file, split) = fixture();
        let far = comment(2, "a.txt", Some((40, 41)), "elsewhere");
        let notes = notes(
            &rows,
            diffs(&file),
            &split,
            &[far],
            "a.txt",
            Opened {
                outdated: false,
                comments: &none(),
            },
        );
        assert_eq!(notes.unified.entries[0], Entry::Heading(Heading::Outside));
        assert!(matches!(
            notes.unified.entries[1],
            Entry::Card { id: 2, .. }
        ));
        assert!(notes.marked.contains(&40));
    }

    #[test]
    fn an_open_comment_marks_its_lines_and_a_resolved_one_does_not() {
        let (rows, file, split) = fixture();
        let open = comment(1, "a.txt", Some((2, 3)), "TWO");
        let done = resolved(comment(2, "a.txt", Some((4, 4)), "four"));
        let notes = notes(
            &rows,
            diffs(&file),
            &split,
            &[open, done],
            "a.txt",
            Opened {
                outdated: false,
                comments: &none(),
            },
        );
        assert_eq!(notes.marked, BTreeSet::from([2, 3]));
    }

    #[test]
    fn a_resolved_comment_is_one_folded_line_until_it_is_opened() {
        let (rows, file, split) = fixture();
        let done = resolved(comment(1, "a.txt", Some((3, 3)), "three"));
        let folded = notes(
            &rows,
            diffs(&file),
            &split,
            std::slice::from_ref(&done),
            "a.txt",
            Opened {
                outdated: false,
                comments: &none(),
            },
        );
        let cards: Vec<&Entry> = folded
            .unified
            .entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Card { .. }))
            .collect();
        assert_eq!(cards.len(), 1);
        assert!(
            matches!(cards[0], Entry::Card { part: Part::Folded(text), .. } if text.starts_with("Resolved · line 3 · "))
        );
        let opened = BTreeSet::from([1]);
        let open = notes(
            &rows,
            diffs(&file),
            &split,
            &[done],
            "a.txt",
            Opened {
                outdated: false,
                comments: &opened,
            },
        );
        assert!(matches!(
            open.unified
                .entries
                .iter()
                .find(|e| matches!(e, Entry::Card { .. })),
            Some(Entry::Card {
                part: Part::Head {
                    badge: Badge::Resolved,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn picked_is_the_file_text_of_the_rows_without_the_removed_lines() {
        let (rows, file, _) = fixture();
        let all = picked(&rows, diffs(&file), 5, 1);
        assert_eq!(all.lines, ["one", "TWO", "three", "four"]);
        assert!(all.contiguous);
        assert_eq!(all.text(), "one\nTWO\nthree\nfour");
        // The removed line alone is not in the file.
        assert!(picked(&rows, diffs(&file), 2, 2).lines.is_empty());
        // The header row adds nothing.
        assert_eq!(picked(&rows, diffs(&file), 0, 1).lines, ["one"]);
    }

    #[test]
    fn picked_lines_across_two_hunks_are_not_contiguous() {
        let mut file = file();
        let mut second = file.hunks[0].clone();
        second.id = HunkId("h2".into());
        second.lines = vec![line(LineKind::Context, Some(10), Some(10), "ten")];
        file.hunks.push(second);
        let diffs = diffs(&file);
        let rows = build_rows(diffs, Side::Unstaged);
        let last = rows.len() - 1;
        let across = picked(&rows, diffs, 5, last);
        assert_eq!(across.lines, ["four", "ten"]);
        assert!(!across.contiguous);
    }

    #[test]
    fn covered_gives_the_working_tree_lines_of_the_rows_and_whether_any_is_a_line() {
        let (rows, file, _) = fixture();
        // Rows 2 and 3 are the removed "two" and the added "TWO" (line 2).
        let both = covered(&rows, diffs(&file), 2, 3);
        assert_eq!(
            both,
            Covered {
                has_line: true,
                working: Some((2, 2))
            }
        );
        // The removed line alone has no working-tree line.
        assert_eq!(
            covered(&rows, diffs(&file), 2, 2),
            Covered {
                has_line: true,
                working: None
            }
        );
        // The hunk header alone covers no line.
        assert_eq!(
            covered(&rows, diffs(&file), 0, 0),
            Covered {
                has_line: false,
                working: None
            }
        );
        // Visual selection in either direction, from the removed line to the context.
        assert_eq!(covered(&rows, diffs(&file), 5, 1).working, Some((1, 4)));
    }

    #[test]
    fn the_selection_range_is_from_the_lowest_to_the_highest_working_line() {
        let (rows, file, _) = fixture();
        assert_eq!(covered(&rows, diffs(&file), 2, 4).working, Some((2, 3)));
    }

    #[test]
    fn a_range_label_says_line_or_lines_with_an_en_dash() {
        assert_eq!(range_label(12, 12), "line 12");
        assert_eq!(range_label(12, 15), "lines 12–15");
    }

    #[test]
    fn a_stale_comment_is_shown_at_its_old_range() {
        let stale = comment(1, "a.txt", None, "x\ny\nz");
        assert_eq!(shown_range(&stale), (1, 3));
        assert_eq!(badge(&stale), Badge::Outdated);
    }

    #[test]
    fn a_resolved_comment_wins_its_badge_over_outdated() {
        let stale = resolved(comment(1, "a.txt", None, "x"));
        assert_eq!(badge(&stale), Badge::Resolved);
    }

    #[test]
    fn wrap_breaks_at_spaces_and_cuts_a_word_that_is_too_long() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("ab cdefghijk", 4), ["ab", "cdef", "ghij", "k"]);
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        assert!(wrap("", 10).is_empty());
    }

    #[test]
    fn folded_summary_cuts_the_first_line_with_an_ellipsis() {
        let mut long = resolved(comment(1, "a.txt", Some((3, 3)), "three"));
        long.body = "x".repeat(70) + "\nsecond";
        let summary = folded_summary(&long);
        assert_eq!(summary, format!("Resolved · line 3 · {}…", "x".repeat(60)));
    }

    #[test]
    fn group_by_path_keeps_each_path_in_listing_order() {
        let list = vec![
            comment(1, "b.txt", Some((1, 1)), "x"),
            comment(2, "a.txt", Some((1, 1)), "x"),
            comment(3, "b.txt", Some((2, 2)), "x"),
        ];
        let groups = group_by_path(&list);
        assert_eq!(
            groups.keys().copied().collect::<Vec<_>>(),
            ["a.txt", "b.txt"]
        );
        assert_eq!(
            groups["b.txt"].iter().map(|c| c.id).collect::<Vec<_>>(),
            [1, 3]
        );
    }

    #[test]
    fn the_list_is_none_until_it_is_shown_and_a_superseded_answer_is_dropped() {
        let mut list = ReviewList::default();
        assert_eq!(list.comments(), None);
        list.request(OperationId(3));
        assert!(list.owns(OperationId(3)));
        assert!(list.listed(OperationId(5), vec![comment(1, "a.txt", Some((1, 1)), "x")]));
        assert_eq!(list.comments().map(<[_]>::len), Some(1));
        // An answer to an older read, after a newer one was shown, changes nothing.
        assert!(!list.listed(OperationId(4), Vec::new()));
        assert_eq!(list.comments().map(<[_]>::len), Some(1));
        assert!(list.comment(1).is_some());
        assert!(list.comment(2).is_none());
    }

    #[test]
    fn a_read_that_is_overtaken_by_a_newer_one_is_dropped() {
        let mut list = ReviewList::default();
        list.request(OperationId(7));
        // A listing answered before the read in flight is superseded.
        assert!(!list.listed(OperationId(6), Vec::new()));
        assert_eq!(list.comments(), None);
        assert!(list.listed(OperationId(7), Vec::new()));
        assert!(!list.owns(OperationId(7)));
    }

    #[test]
    fn a_failed_read_keeps_the_list_and_says_why() {
        let mut list = ReviewList::default();
        list.request(OperationId(2));
        list.ended(
            OperationId(2),
            OperationState::Failed,
            Some("no repo".into()),
        );
        assert_eq!(list.failure(), Some("no repo"));
        list.request(OperationId(3));
        assert_eq!(list.failure(), None);
        list.ended(OperationId(3), OperationState::Failed, None);
        assert_eq!(list.failure(), Some("The comments could not be read."));
    }

    #[test]
    fn a_write_settles_only_its_own_operation() {
        let mut write = ReviewWrite::default();
        write.start(OperationId(4));
        assert!(write.is_pending());
        assert!(!write.owns(OperationId(3)));
        write.settle(OperationState::Executing, None);
        assert_eq!(write.take(), None);
        write.settle(OperationState::Failed, Some("nope".into()));
        assert_eq!(write.take(), Some(Err("nope".to_owned())));
        assert!(!write.is_pending());
        write.start(OperationId(5));
        write.settle(OperationState::Done, None);
        assert_eq!(write.take(), Some(Ok(())));
    }

    #[test]
    fn the_second_press_of_delete_within_three_seconds_on_the_same_comment_deletes() {
        let t0 = Instant::now();
        let mut arm = DeleteArm::default();
        assert_eq!(arm.press(1, t0), Delete::Ask);
        assert!(arm.is_armed(1, t0));
        assert!(!arm.is_armed(2, t0));
        assert_eq!(arm.press(1, t0 + Duration::from_secs(2)), Delete::Confirmed);
        assert!(!arm.is_armed(1, t0 + Duration::from_secs(2)));
    }

    #[test]
    fn a_press_after_the_window_asks_again_and_another_comment_asks_too() {
        let t0 = Instant::now();
        let mut arm = DeleteArm::default();
        assert_eq!(arm.press(1, t0), Delete::Ask);
        assert!(!arm.is_armed(1, t0 + DELETE_CONFIRM));
        assert_eq!(arm.press(1, t0 + DELETE_CONFIRM), Delete::Ask);
        assert_eq!(arm.press(2, t0 + DELETE_CONFIRM), Delete::Ask);
        assert!(!arm.is_armed(1, t0 + DELETE_CONFIRM));
        arm.clear();
        assert!(!arm.is_armed(2, t0 + DELETE_CONFIRM));
    }

    #[test]
    fn a_made_time_reads_as_the_list_reads_times() {
        assert_eq!(made_label(1_000_000, (1_000_000 - 30) * 1000), "just now");
        assert_eq!(made_label(1_000_000, (1_000_000 - 7_200) * 1000), "2 h ago");
    }

    proptest! {
        #[test]
        fn wrapped_lines_fit_the_columns_and_keep_every_character(
            body in "[a-z ]{0,200}",
            columns in 1usize..20,
        ) {
            let lines = wrap(&body, columns);
            prop_assert!(lines.iter().all(|line| line.chars().count() <= columns));
            let joined: String = lines.concat();
            let kept: String = body.chars().filter(|c| *c != ' ').collect();
            let kept_joined: String = joined.chars().filter(|c| *c != ' ').collect();
            prop_assert_eq!(kept, kept_joined);
        }

        #[test]
        fn a_card_has_one_row_per_wrapped_line_plus_its_head(
            body in "[a-z ]{1,300}",
        ) {
            let mut note = comment(1, "a.txt", Some((1, 1)), "x");
            note.body = body.clone();
            let entries = card_entries(&note, false);
            prop_assert_eq!(entries.len(), 1 + wrap(&body, WRAP_COLUMNS).len());
        }
    }
}
