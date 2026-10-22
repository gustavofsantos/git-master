//! The Local review page (B4d, D96, D97), with no GPUI in it: the filter and the
//! counts it shows, the list grouped by file, the comment selected and which of
//! its two texts is shown, the reads of those texts, and the lines that differ
//! between them. The page draws what this module says.

use std::time::Instant;

use gm_schema::{OperationId, ReviewComment, ReviewStatus};

use crate::review::{self, Badge, Delete, DeleteArm};

const NO_SNAPSHOT: &str = "No snapshot was taken for this comment.";
const NOT_IN_SNAPSHOT: &str = "The commented text is not in the snapshot.";
const NOT_IN_FILE: &str = "The commented text is no longer in the file.";

/// Which comments the list shows. Open is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    All,
    #[default]
    Open,
    Resolved,
    /// The comments whose text is not in the file now, whatever their status.
    Outdated,
}

impl Filter {
    /// The filters, in the order their chips show.
    pub const ALL: [Filter; 4] = [
        Filter::All,
        Filter::Open,
        Filter::Resolved,
        Filter::Outdated,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Open => "Open",
            Filter::Resolved => "Resolved",
            Filter::Outdated => "Outdated",
        }
    }

    /// Whether the filter shows `comment`. An outdated open comment is shown by
    /// Open, by Outdated and by All.
    #[must_use]
    pub fn keeps(self, comment: &ReviewComment) -> bool {
        match self {
            Filter::All => true,
            Filter::Open => comment.status == ReviewStatus::Open,
            Filter::Resolved => comment.status == ReviewStatus::Resolved,
            Filter::Outdated => is_outdated(comment),
        }
    }
}

/// Whether the comment's text is not in the file now, exactly once.
#[must_use]
pub fn is_outdated(comment: &ReviewComment) -> bool {
    comment.location.is_none()
}

/// How many comments each filter keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub all: usize,
    pub open: usize,
    pub resolved: usize,
    pub outdated: usize,
}

impl Counts {
    #[must_use]
    pub fn of(self, filter: Filter) -> usize {
        match filter {
            Filter::All => self.all,
            Filter::Open => self.open,
            Filter::Resolved => self.resolved,
            Filter::Outdated => self.outdated,
        }
    }
}

#[must_use]
pub fn counts(comments: &[ReviewComment]) -> Counts {
    let kept = |filter: Filter| {
        comments
            .iter()
            .filter(|comment| filter.keeps(comment))
            .count()
    };
    Counts {
        all: comments.len(),
        open: kept(Filter::Open),
        resolved: kept(Filter::Resolved),
        outdated: kept(Filter::Outdated),
    }
}

/// The count on the sidebar's Local review row: the open comments, and no pill at all
/// while there are none or the listing is not read yet.
#[must_use]
pub fn open_pill(comments: Option<&[ReviewComment]>) -> Option<String> {
    let open = counts(comments?).open;
    (open > 0).then(|| open.to_string())
}

/// One row of the list: the heading of a file, then one of its comments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    File {
        path: String,
        count: usize,
    },
    Comment {
        id: i64,
        lines: String,
        summary: String,
        badge: Badge,
        created_at: i64,
    },
}

/// The rows of the list: the comments the filter keeps, grouped by file in path
/// order. Within a file they follow the line they show (the recorded line when
/// outdated), then their id. A file with no comment kept has no row.
#[must_use]
pub fn rows(comments: &[ReviewComment], filter: Filter) -> Vec<Row> {
    let mut out = Vec::new();
    for (path, mut group) in review::group_by_path(comments) {
        group.retain(|comment| filter.keeps(comment));
        if group.is_empty() {
            continue;
        }
        group.sort_by_key(|comment| (review::shown_range(comment).0, comment.id));
        out.push(Row::File {
            path: path.to_owned(),
            count: group.len(),
        });
        out.extend(group.into_iter().map(comment_row));
    }
    out
}

fn comment_row(comment: &ReviewComment) -> Row {
    let (start, end) = review::shown_range(comment);
    Row::Comment {
        id: comment.id,
        lines: review::range_label(start, end),
        summary: summary(&comment.body),
        badge: review::badge(comment),
        created_at: comment.created_at,
    }
}

/// The first line of a body that has any text, trimmed.
#[must_use]
pub fn summary(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned()
}

/// Which of the two texts of the selected comment the page shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Version {
    /// The commented lines as the snapshot held them, from when the comment was made.
    #[default]
    Before,
    /// The commented lines as the file holds them now.
    After,
}

impl Version {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Version::Before => "Before",
            Version::After => "After",
        }
    }

    #[must_use]
    pub fn caption(self) -> &'static str {
        match self {
            Version::Before => "As it was when you commented",
            Version::After => "Now",
        }
    }
}

/// The version a comment opens on: Before when it has a snapshot, else After.
#[must_use]
pub fn default_version(comment: &ReviewComment) -> Version {
    if comment.snapshot.is_some() {
        Version::Before
    } else {
        Version::After
    }
}

/// What the page says for a loaded text that has no lines.
#[must_use]
pub fn empty_text(version: Version, comment: &ReviewComment) -> &'static str {
    match version {
        Version::Before if comment.snapshot.is_none() => NO_SNAPSHOT,
        Version::Before => NOT_IN_SNAPSHOT,
        Version::After => NOT_IN_FILE,
    }
}

/// The number of the first line of a text: the recorded line for the snapshot, the
/// line the comment has now for the file (its recorded line when outdated).
#[must_use]
pub fn first_line(comment: &ReviewComment, version: Version) -> u32 {
    match version {
        Version::Before => comment.anchor.start_line,
        Version::After => comment
            .location
            .map_or(comment.anchor.start_line, |location| location.start_line),
    }
}

/// The text the list shows when no comment exists yet, naming the key that starts one.
#[must_use]
pub fn empty_hint(shortcut: &str) -> String {
    format!("Select lines in the Working Copy diff and press {shortcut} to add one.")
}

/// A text the page asked for: the read in flight, and the lines of the last answer
/// that was kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Read {
    pending: Option<OperationId>,
    lines: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selected {
    id: i64,
    version: Version,
}

/// The Local review page's state: the filter, the selected comment with the version
/// it shows, the two texts of that comment and the delete waiting for its second click.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalReview {
    filter: Filter,
    selected: Option<Selected>,
    before: Read,
    after: Read,
    delete: DeleteArm,
}

impl LocalReview {
    #[must_use]
    pub fn filter(&self) -> Filter {
        self.filter
    }

    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
    }

    #[must_use]
    pub fn selected_id(&self) -> Option<i64> {
        self.selected.map(|selected| selected.id)
    }

    /// The version the selected comment shows; Before when none is selected.
    #[must_use]
    pub fn version(&self) -> Version {
        self.selected
            .map_or(Version::Before, |selected| selected.version)
    }

    pub fn set_version(&mut self, version: Version) {
        if let Some(selected) = self.selected.as_mut() {
            selected.version = version;
        }
    }

    /// Selects `comment`, which opens on its default version and forgets the texts of
    /// the comment before. False when it was already selected, so nothing needs asking.
    pub fn select(&mut self, comment: &ReviewComment) -> bool {
        if self.selected_id() == Some(comment.id) {
            return false;
        }
        self.selected = Some(Selected {
            id: comment.id,
            version: default_version(comment),
        });
        self.before = Read::default();
        self.after = Read::default();
        true
    }

    /// Drops the selection when `comments` no longer has its comment. A listing that
    /// still has it changes nothing.
    pub fn keep_listed(&mut self, comments: &[ReviewComment]) {
        let Some(id) = self.selected_id() else {
            return;
        };
        if !comments.iter().any(|comment| comment.id == id) {
            self.selected = None;
            self.before = Read::default();
            self.after = Read::default();
        }
    }

    /// Records the two reads the window sent for the selected comment. The lines
    /// already shown stay until their answers come.
    pub fn expect(&mut self, before: OperationId, after: OperationId) {
        self.before.pending = Some(before);
        self.after.pending = Some(after);
    }

    /// Takes the Before text that `id` answered for comment `comment`. False when the
    /// answer is superseded or is for another comment.
    pub fn received_before(&mut self, id: OperationId, comment: i64, lines: Vec<String>) -> bool {
        let wanted = self.selected_id();
        settle(&mut self.before, wanted, id, comment, lines)
    }

    /// Takes the After text that `id` answered for comment `comment`, as
    /// `received_before` does for Before.
    pub fn received_after(&mut self, id: OperationId, comment: i64, lines: Vec<String>) -> bool {
        let wanted = self.selected_id();
        settle(&mut self.after, wanted, id, comment, lines)
    }

    /// The lines of `version` as they were last answered; `None` until an answer comes.
    #[must_use]
    pub fn lines(&self, version: Version) -> Option<&[String]> {
        self.read(version).lines.as_deref()
    }

    fn read(&self, version: Version) -> &Read {
        match version {
            Version::Before => &self.before,
            Version::After => &self.after,
        }
    }

    /// Which lines of `version` are marked in the diff between the two texts: the
    /// removed lines of Before and the added lines of After. Nothing is marked unless
    /// both texts have lines.
    #[must_use]
    pub fn marks(&self, version: Version) -> Vec<bool> {
        match (self.before.lines.as_deref(), self.after.lines.as_deref()) {
            (Some(before), Some(after)) if !before.is_empty() && !after.is_empty() => {
                let (removed, added) = changed_lines(before, after);
                match version {
                    Version::Before => removed,
                    Version::After => added,
                }
            }
            _ => vec![false; self.lines(version).map_or(0, <[String]>::len)],
        }
    }

    /// The first click of delete asks, a second one on the same comment within the
    /// confirmation window deletes, as on the cards.
    pub fn press_delete(&mut self, id: i64, now: Instant) -> Delete {
        self.delete.press(id, now)
    }

    /// Whether the button of `id` reads "Click again to delete" at `now`.
    #[must_use]
    pub fn delete_armed(&self, id: i64, now: Instant) -> bool {
        self.delete.is_armed(id, now)
    }
}

/// Takes `lines` into `read` when `id` is its read in flight and `comment` is the
/// selected comment. False otherwise, and the read is left as it was.
fn settle(
    read: &mut Read,
    wanted: Option<i64>,
    id: OperationId,
    comment: i64,
    lines: Vec<String>,
) -> bool {
    if read.pending != Some(id) || wanted != Some(comment) {
        return false;
    }
    read.pending = None;
    read.lines = Some(lines);
    true
}

/// Which lines of `before` and of `after` are not in their longest common run, as
/// `(removed, added)`: a `true` for a line of `before` that is gone from `after`, and
/// for a line of `after` that is new. Both texts come from one comment, so they are
/// at most a few hundred lines each.
#[must_use]
pub fn changed_lines(before: &[String], after: &[String]) -> (Vec<bool>, Vec<bool>) {
    let (n, m) = (before.len(), after.len());
    // common[i][j] is the length of the longest common run of before[i..] and after[j..].
    let mut common = vec![vec![0_usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            common[i][j] = if before[i] == after[j] {
                common[i + 1][j + 1] + 1
            } else {
                common[i + 1][j].max(common[i][j + 1])
            };
        }
    }
    let mut removed = vec![true; n];
    let mut added = vec![true; m];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if before[i] == after[j] {
            removed[i] = false;
            added[j] = false;
            i += 1;
            j += 1;
        } else if common[i + 1][j] >= common[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    (removed, added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{ReviewAnchor, ReviewLocation};
    use proptest::prelude::*;

    fn comment(id: i64, path: &str, status: ReviewStatus, line: Option<u32>) -> ReviewComment {
        ReviewComment {
            id,
            path: path.into(),
            body: format!("note {id}"),
            status,
            anchor: ReviewAnchor {
                text: "x".into(),
                before: Vec::new(),
                after: Vec::new(),
                start_line: line.unwrap_or(1),
            },
            created_at: 1_000 * id,
            resolved_at: None,
            snapshot: Some("abc".into()),
            location: line.map(|start_line| ReviewLocation {
                start_line,
                end_line: start_line,
            }),
            outdated: line.is_none(),
        }
    }

    fn words(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_owned()).collect()
    }

    fn id(value: u64) -> OperationId {
        OperationId(value)
    }

    #[test]
    fn open_is_the_default_filter_and_outdated_counts_on_its_own() {
        assert_eq!(Filter::default(), Filter::Open);
        let open_outdated = comment(1, "a", ReviewStatus::Open, None);
        let resolved_outdated = comment(2, "a", ReviewStatus::Resolved, None);
        let open = comment(3, "a", ReviewStatus::Open, Some(4));
        assert!(Filter::Open.keeps(&open_outdated));
        assert!(Filter::Outdated.keeps(&open_outdated));
        assert!(Filter::All.keeps(&open_outdated));
        assert!(!Filter::Resolved.keeps(&open_outdated));
        assert!(Filter::Outdated.keeps(&resolved_outdated));
        assert!(!Filter::Outdated.keeps(&open));
        assert!(!Filter::Resolved.keeps(&open));
    }

    #[test]
    fn counts_say_how_many_each_filter_keeps() {
        let all = [
            comment(1, "a", ReviewStatus::Open, Some(1)),
            comment(2, "a", ReviewStatus::Open, None),
            comment(3, "b", ReviewStatus::Resolved, Some(2)),
            comment(4, "b", ReviewStatus::Resolved, None),
        ];
        let counted = counts(&all);
        assert_eq!(
            counted,
            Counts {
                all: 4,
                open: 2,
                resolved: 2,
                outdated: 2,
            }
        );
        assert_eq!(counted.of(Filter::Resolved), 2);
        assert_eq!(counts(&[]), Counts::default());
    }

    #[test]
    fn the_sidebar_pill_shows_open_comments_and_is_hidden_at_zero() {
        let open = [comment(1, "a", ReviewStatus::Open, Some(1))];
        assert_eq!(open_pill(Some(&open)), Some("1".to_owned()));
        let done = [comment(1, "a", ReviewStatus::Resolved, Some(1))];
        assert_eq!(open_pill(Some(&done)), None);
        assert_eq!(open_pill(Some(&[])), None);
        assert_eq!(open_pill(None), None);
    }

    #[test]
    fn rows_group_by_file_in_path_order_and_by_the_line_each_comment_shows() {
        let all = [
            comment(1, "src/b.rs", ReviewStatus::Open, Some(9)),
            comment(2, "src/a.rs", ReviewStatus::Open, Some(30)),
            comment(3, "src/a.rs", ReviewStatus::Open, Some(4)),
            comment(4, "src/c.rs", ReviewStatus::Resolved, Some(1)),
        ];
        let listed = rows(&all, Filter::Open);
        let ids: Vec<&str> = listed
            .iter()
            .map(|row| match row {
                Row::File { path, .. } => path.as_str(),
                Row::Comment { .. } => "comment",
            })
            .collect();
        assert_eq!(
            ids,
            ["src/a.rs", "comment", "comment", "src/b.rs", "comment"]
        );
        let order: Vec<i64> = listed
            .iter()
            .filter_map(|row| match row {
                Row::Comment { id, .. } => Some(*id),
                Row::File { .. } => None,
            })
            .collect();
        assert_eq!(order, [3, 2, 1]);
        assert_eq!(
            listed[0],
            Row::File {
                path: "src/a.rs".into(),
                count: 2,
            }
        );
    }

    #[test]
    fn a_file_with_no_comment_kept_has_no_row() {
        let all = [comment(1, "a.rs", ReviewStatus::Resolved, Some(1))];
        assert!(rows(&all, Filter::Open).is_empty());
        assert_eq!(rows(&all, Filter::Resolved).len(), 2);
    }

    #[test]
    fn a_row_gives_the_range_the_badge_and_the_first_line_with_text() {
        let mut first = comment(7, "a.rs", ReviewStatus::Open, Some(12));
        first.body = "\n   \n  rename this  \nand this".into();
        first.location = Some(ReviewLocation {
            start_line: 12,
            end_line: 15,
        });
        let listed = rows(&[first], Filter::All);
        assert_eq!(
            listed[1],
            Row::Comment {
                id: 7,
                lines: "lines 12–15".into(),
                summary: "rename this".into(),
                badge: Badge::Open,
                created_at: 7_000,
            }
        );
    }

    #[test]
    fn the_rows_show_each_kept_comment_once_whatever_the_listing() {
        proptest!(|(lines in prop::collection::vec((0_u8..3, prop::option::of(1_u32..40)), 0..20))| {
            let all: Vec<ReviewComment> = lines
                .iter()
                .enumerate()
                .map(|(index, (path, line))| {
                    let status = if index % 2 == 0 { ReviewStatus::Open } else { ReviewStatus::Resolved };
                    comment(i64::try_from(index).unwrap(), &format!("f{path}"), status, *line)
                })
                .collect();
            for filter in Filter::ALL {
                let kept = all.iter().filter(|c| filter.keeps(c)).count();
                let shown: Vec<i64> = rows(&all, filter)
                    .into_iter()
                    .filter_map(|row| match row {
                        Row::Comment { id, .. } => Some(id),
                        Row::File { .. } => None,
                    })
                    .collect();
                prop_assert_eq!(shown.len(), kept);
                let mut unique = shown.clone();
                unique.sort_unstable();
                unique.dedup();
                prop_assert_eq!(unique.len(), shown.len());
            }
        });
    }

    #[test]
    fn the_snapshot_is_the_default_version_and_no_snapshot_opens_on_after() {
        let mut with = comment(1, "a", ReviewStatus::Open, Some(1));
        assert_eq!(default_version(&with), Version::Before);
        with.snapshot = None;
        assert_eq!(default_version(&with), Version::After);
    }

    #[test]
    fn selecting_a_comment_opens_its_default_version_and_forgets_its_texts() {
        let first = comment(1, "a", ReviewStatus::Open, Some(1));
        let second = comment(2, "a", ReviewStatus::Open, Some(2));
        let mut page = LocalReview::default();
        assert_eq!(page.version(), Version::Before);
        assert!(page.select(&first));
        assert!(!page.select(&first), "the same comment needs no new read");
        page.set_version(Version::After);
        assert_eq!(page.version(), Version::After);
        assert!(page.select(&second));
        assert_eq!(page.version(), Version::Before);
        assert_eq!(page.selected_id(), Some(2));
    }

    #[test]
    fn set_version_does_nothing_without_a_selection() {
        let mut page = LocalReview::default();
        page.set_version(Version::After);
        assert_eq!(page.version(), Version::Before);
        assert_eq!(page.selected_id(), None);
    }

    #[test]
    fn the_selection_ends_when_its_comment_is_no_longer_listed() {
        let kept = comment(1, "a", ReviewStatus::Open, Some(1));
        let mut page = LocalReview::default();
        page.select(&kept);
        page.keep_listed(std::slice::from_ref(&kept));
        assert_eq!(page.selected_id(), Some(1));
        page.keep_listed(&[comment(2, "a", ReviewStatus::Open, Some(2))]);
        assert_eq!(page.selected_id(), None);
    }

    #[test]
    fn the_filter_is_kept_when_the_selection_ends() {
        let mut page = LocalReview::default();
        page.set_filter(Filter::Resolved);
        page.select(&comment(1, "a", ReviewStatus::Open, Some(1)));
        page.keep_listed(&[]);
        assert_eq!(page.filter(), Filter::Resolved);
    }

    #[test]
    fn a_text_answer_is_kept_only_for_the_read_in_flight_and_the_selected_comment() {
        let mut page = LocalReview::default();
        page.select(&comment(1, "a", ReviewStatus::Open, Some(1)));
        page.expect(id(10), id(11));
        assert_eq!(page.lines(Version::Before), None);

        assert!(
            !page.received_before(id(9), 1, words(&["old"])),
            "superseded"
        );
        assert!(
            !page.received_before(id(10), 2, words(&["other"])),
            "another comment"
        );
        assert!(page.received_before(id(10), 1, words(&["b"])));
        assert_eq!(page.lines(Version::Before), Some(words(&["b"]).as_slice()));

        assert!(page.received_after(id(11), 1, words(&["c"])));
        assert_eq!(page.lines(Version::After), Some(words(&["c"]).as_slice()));
    }

    #[test]
    fn a_selection_change_drops_the_texts_and_a_late_answer_for_the_old_one_is_refused() {
        let mut page = LocalReview::default();
        page.select(&comment(1, "a", ReviewStatus::Open, Some(1)));
        page.expect(id(10), id(11));
        page.received_before(id(10), 1, words(&["b"]));
        page.select(&comment(2, "a", ReviewStatus::Open, Some(2)));
        assert_eq!(page.lines(Version::Before), None);
        assert!(!page.received_before(id(10), 1, words(&["b"])));
        assert!(!page.received_after(id(11), 1, words(&["c"])));
    }

    #[test]
    fn a_reload_keeps_the_lines_on_screen_until_the_new_answer_comes() {
        let mut page = LocalReview::default();
        page.select(&comment(1, "a", ReviewStatus::Open, Some(1)));
        page.expect(id(10), id(11));
        page.received_after(id(11), 1, words(&["first"]));
        page.expect(id(12), id(13));
        assert_eq!(
            page.lines(Version::After),
            Some(words(&["first"]).as_slice())
        );
    }

    #[test]
    fn empty_texts_say_why_there_is_nothing_to_show() {
        let mut with = comment(1, "a", ReviewStatus::Open, Some(1));
        assert_eq!(
            empty_text(Version::Before, &with),
            "The commented text is not in the snapshot."
        );
        assert_eq!(
            empty_text(Version::After, &with),
            "The commented text is no longer in the file."
        );
        with.snapshot = None;
        assert_eq!(
            empty_text(Version::Before, &with),
            "No snapshot was taken for this comment."
        );
    }

    #[test]
    fn the_first_line_of_each_text_is_the_recorded_line_or_the_current_one() {
        let mut moved = comment(1, "a", ReviewStatus::Open, Some(20));
        moved.anchor.start_line = 12;
        assert_eq!(first_line(&moved, Version::Before), 12);
        assert_eq!(first_line(&moved, Version::After), 20);
        moved.location = None;
        assert_eq!(first_line(&moved, Version::After), 12);
    }

    #[test]
    fn marks_appear_only_when_both_texts_have_lines() {
        let mut page = LocalReview::default();
        page.select(&comment(1, "a", ReviewStatus::Open, Some(1)));
        page.expect(id(1), id(2));
        page.received_before(id(1), 1, words(&["a", "gone", "c"]));
        assert_eq!(page.marks(Version::Before), vec![false, false, false]);
        page.received_after(id(2), 1, words(&["a", "c", "new"]));
        assert_eq!(page.marks(Version::Before), vec![false, true, false]);
        assert_eq!(page.marks(Version::After), vec![false, false, true]);
        page.expect(id(3), id(4));
        page.received_after(id(4), 1, Vec::new());
        assert_eq!(page.marks(Version::Before), vec![false, false, false]);
    }

    #[test]
    fn changed_lines_marks_what_only_one_side_has() {
        let before = words(&["a", "b", "c"]);
        assert_eq!(
            changed_lines(&before, &before),
            (vec![false; 3], vec![false; 3])
        );
        assert_eq!(
            changed_lines(&before, &words(&["a", "B", "c"])),
            (vec![false, true, false], vec![false, true, false])
        );
        assert_eq!(
            changed_lines(&before, &words(&["x", "a", "b", "c", "y"])),
            (vec![false; 3], vec![true, false, false, false, true])
        );
        assert_eq!(changed_lines(&before, &[]), (vec![true; 3], Vec::new()));
        assert_eq!(changed_lines(&[], &before), (Vec::new(), vec![true; 3]));
    }

    #[test]
    fn a_summary_is_the_first_non_blank_line_trimmed() {
        assert_eq!(summary("  \n  first  \nsecond"), "first");
        assert_eq!(summary(""), "");
    }

    #[test]
    fn the_empty_hint_names_the_shortcut() {
        assert_eq!(
            empty_hint("⇧⌘M"),
            "Select lines in the Working Copy diff and press ⇧⌘M to add one."
        );
    }

    #[test]
    fn the_filter_labels_and_the_version_captions_are_the_page_words() {
        let labels: Vec<&str> = Filter::ALL.into_iter().map(Filter::label).collect();
        assert_eq!(labels, ["All", "Open", "Resolved", "Outdated"]);
        assert_eq!(Version::Before.label(), "Before");
        assert_eq!(Version::After.caption(), "Now");
    }

    #[test]
    fn delete_here_is_the_two_click_confirm_of_the_cards() {
        let mut page = LocalReview::default();
        let now = Instant::now();
        assert_eq!(page.press_delete(3, now), Delete::Ask);
        assert!(page.delete_armed(3, now));
        assert_eq!(page.press_delete(3, now), Delete::Confirmed);
    }
}
