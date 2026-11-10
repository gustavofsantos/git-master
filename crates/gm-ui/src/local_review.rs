//! The Local review page (B4d, D96, D97), with no GPUI in it: the filter and the
//! counts it shows, the list grouped by file, the comment selected and which of
//! its two texts is shown, the reads of those texts, and the lines that differ
//! between them. The page draws what this module says.

use std::collections::BTreeSet;
use std::time::Instant;

use gm_schema::{OperationId, ReviewChange, ReviewComment, ReviewStatus};

use crate::review::{self, Badge, Chips, Delete, DeleteArm};

const NO_SNAPSHOT: &str = "No snapshot was taken for this comment.";
const NOT_IN_SNAPSHOT: &str = "The commented text is not in the snapshot.";
const NOT_IN_FILE: &str = "The commented text is no longer in the file.";
const NOT_IN_AFTER: &str = "The commented text was not in the file when the agent handled it.";
const HANDLED_CAPTION: &str = "As it was when the agent marked it handled";

/// Which comments the list shows. Open is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    All,
    #[default]
    Open,
    Resolved,
    /// The comments whose text is not in the file now, whatever their status.
    Outdated,
    /// The questions an agent asked that no person has answered yet (D200).
    NeedsAnswer,
    /// The threads an agent started (D200), whatever their status.
    Agent,
}

impl Filter {
    /// The filters, in the order their chips show.
    pub const ALL: [Filter; 6] = [
        Filter::All,
        Filter::Open,
        Filter::Resolved,
        Filter::Outdated,
        Filter::NeedsAnswer,
        Filter::Agent,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Open => "Open",
            Filter::Resolved => "Resolved",
            Filter::Outdated => "Outdated",
            Filter::NeedsAnswer => review::NEEDS_ANSWER,
            Filter::Agent => "Agent",
        }
    }

    /// Whether the filter shows `comment`. Open is what still waits for the person,
    /// so it keeps a handled comment too. An outdated open comment is shown by
    /// Open, by Outdated and by All.
    #[must_use]
    pub fn keeps(self, comment: &ReviewComment) -> bool {
        match self {
            Filter::All => true,
            Filter::Open => comment.status != ReviewStatus::Resolved,
            Filter::Resolved => comment.status == ReviewStatus::Resolved,
            Filter::Outdated => is_outdated(comment),
            Filter::NeedsAnswer => comment.needs_answer,
            Filter::Agent => review::is_agent_root(comment),
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
    pub needs_answer: usize,
    pub agent: usize,
}

impl Counts {
    #[must_use]
    pub fn of(self, filter: Filter) -> usize {
        match filter {
            Filter::All => self.all,
            Filter::Open => self.open,
            Filter::Resolved => self.resolved,
            Filter::Outdated => self.outdated,
            Filter::NeedsAnswer => self.needs_answer,
            Filter::Agent => self.agent,
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
        needs_answer: kept(Filter::NeedsAnswer),
        agent: kept(Filter::Agent),
    }
}

/// The count on the sidebar's Local review row: the open comments the person started,
/// and no pill at all while there are none or the listing is not read yet. An agent's
/// note or flag is not work waiting for the agent, so it is not counted (D200).
#[must_use]
pub fn open_pill(comments: Option<&[ReviewComment]>) -> Option<String> {
    let open = comments?
        .iter()
        .filter(|comment| !review::is_agent_root(comment) && Filter::Open.keeps(comment))
        .count();
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
        chips: Chips,
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
        chips: review::chips(comment),
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
    /// The commented lines as the "after" snapshot holds them once an agent has
    /// handled the comment (D140), else as the file holds them now.
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

/// Whether the texts of `comment` come from its two snapshots in one read (D140):
/// once an agent has handled it, After is its "after" snapshot, not the file now.
#[must_use]
pub fn reads_change(comment: &ReviewComment) -> bool {
    comment.after_snapshot.is_some()
}

/// The caption over the text of `version` for `comment`: After names the agent's
/// snapshot when it comes from one, else the file now.
#[must_use]
pub fn caption(version: Version, comment: &ReviewComment) -> &'static str {
    match version {
        Version::After if reads_change(comment) => HANDLED_CAPTION,
        _ => version.caption(),
    }
}

/// What the page says for a loaded text that has no lines.
#[must_use]
pub fn empty_text(version: Version, comment: &ReviewComment) -> &'static str {
    match version {
        Version::Before if comment.snapshot.is_none() => NO_SNAPSHOT,
        Version::Before => NOT_IN_SNAPSHOT,
        Version::After if reads_change(comment) => NOT_IN_AFTER,
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
    /// The number of the first line, when the answer said where the lines are in
    /// their snapshot (D140); otherwise `first_line` of the comment applies.
    first: Option<u32>,
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
    /// The comments whose threads show all their replies (D198).
    threads: BTreeSet<i64>,
}

impl LocalReview {
    #[must_use]
    pub fn filter(&self) -> Filter {
        self.filter
    }

    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
    }

    /// Whether the thread of comment `id` shows all its replies.
    #[must_use]
    pub fn thread_open(&self, id: i64) -> bool {
        self.threads.contains(&id)
    }

    /// Shows all of a thread's replies, or folds the earlier ones again.
    pub fn toggle_thread(&mut self, id: i64) {
        if !self.threads.remove(&id) {
            self.threads.insert(id);
        }
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

    /// Records the one read that answers both texts of a handled comment (D140).
    /// The lines already shown stay until its answer comes.
    pub fn expect_change(&mut self, id: OperationId) {
        self.before.pending = Some(id);
        self.after.pending = Some(id);
    }

    /// Takes both texts from the change that `id` answered for comment `comment`:
    /// Before from its "before" snapshot, After from its "after" one. A change the
    /// engine could not read (`None`) leaves both texts empty, as a missing snapshot
    /// does. False when the answer is superseded or is for another comment.
    pub fn received_change(
        &mut self,
        id: OperationId,
        comment: i64,
        change: Option<ReviewChange>,
    ) -> bool {
        let wanted = self.selected_id();
        let starts = change
            .as_ref()
            .map(|change| (change.before_start_line, change.after_start_line));
        let (before, after) =
            change.map_or_else(Default::default, |change| (change.before, change.after));
        let took_before = settle(&mut self.before, wanted, id, comment, before);
        let took_after = settle(&mut self.after, wanted, id, comment, after);
        if took_before && took_after {
            self.before.first = starts.map(|(before, _)| before);
            self.after.first = starts.map(|(_, after)| after);
        }
        took_before && took_after
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

    /// The number of the first line of the `version` text of `comment`: where its
    /// snapshot put it when the answer said so, else `first_line`.
    #[must_use]
    pub fn first_line(&self, comment: &ReviewComment, version: Version) -> u32 {
        self.read(version)
            .first
            .unwrap_or_else(|| first_line(comment, version))
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
    read.first = None;
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
    use gm_schema::{ReviewAnchor, ReviewKind, ReviewLocation, ReviewSeverity};
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
            replies: Vec::new(),
            snapshot: Some("abc".into()),
            after_snapshot: None,
            author: None,
            kind: None,
            severity: None,
            needs_answer: false,
            suggested_fix: None,
            label: None,
            decorations: Vec::new(),
            scorecard: None,
            location: line.map(|start_line| ReviewLocation {
                start_line,
                end_line: start_line,
            }),
            outdated: line.is_none(),
            workspace: None,
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
                needs_answer: 0,
                agent: 0,
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
                chips: Chips::default(),
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

    fn handled(id: i64) -> ReviewComment {
        let mut handled = comment(id, "a", ReviewStatus::Handled, Some(1));
        handled.after_snapshot = Some("def".into());
        handled
    }

    fn change(before: &[&str], after: &[&str]) -> ReviewChange {
        ReviewChange {
            before: words(before),
            after: words(after),
            changed: before != after,
            before_start_line: 1,
            after_start_line: 1,
        }
    }

    #[test]
    fn a_change_numbers_its_lines_where_its_snapshots_put_them() {
        let mut page = LocalReview::default();
        let comment = handled(1);
        page.select(&comment);
        let read = OperationId(30);
        page.expect_change(read);
        let mut moved = change(&["b"], &["c"]);
        moved.before_start_line = 4;
        moved.after_start_line = 9;
        assert!(page.received_change(read, 1, Some(moved)));
        assert_eq!(page.first_line(&comment, Version::Before), 4);
        assert_eq!(page.first_line(&comment, Version::After), 9);
    }

    #[test]
    fn texts_read_from_the_file_number_their_lines_from_the_comment() {
        let mut page = LocalReview::default();
        let open = comment(1, "a", ReviewStatus::Open, Some(7));
        page.select(&open);
        page.expect(id(10), id(11));
        page.received_before(id(10), 1, words(&["b"]));
        page.received_after(id(11), 1, words(&["b"]));
        assert_eq!(
            page.first_line(&open, Version::Before),
            first_line(&open, Version::Before)
        );
        assert_eq!(
            page.first_line(&open, Version::After),
            first_line(&open, Version::After)
        );
    }

    #[test]
    fn a_handled_comment_reads_its_change_and_an_unhandled_one_the_file() {
        assert!(reads_change(&handled(1)));
        assert!(!reads_change(&comment(1, "a", ReviewStatus::Open, Some(1))));
    }

    #[test]
    fn both_texts_of_a_handled_comment_fill_from_its_change() {
        let mut page = LocalReview::default();
        page.select(&handled(1));
        page.expect_change(id(20));
        assert!(page.received_change(id(20), 1, Some(change(&["a", "old"], &["a", "new"]))));
        assert_eq!(
            page.lines(Version::Before),
            Some(words(&["a", "old"]).as_slice())
        );
        assert_eq!(
            page.lines(Version::After),
            Some(words(&["a", "new"]).as_slice())
        );
        assert_eq!(page.marks(Version::After), vec![false, true]);
    }

    #[test]
    fn a_change_the_engine_could_not_read_leaves_both_texts_empty() {
        let mut page = LocalReview::default();
        page.select(&handled(1));
        page.expect_change(id(20));
        assert!(page.received_change(id(20), 1, None));
        assert_eq!(page.lines(Version::Before), Some([].as_slice()));
        assert_eq!(page.lines(Version::After), Some([].as_slice()));
    }

    #[test]
    fn a_late_change_for_a_superseded_read_or_another_comment_is_dropped() {
        let mut page = LocalReview::default();
        page.select(&handled(1));
        page.expect_change(id(20));
        page.expect_change(id(21));
        assert!(!page.received_change(id(20), 1, Some(change(&["stale"], &["stale"]))));
        assert!(!page.received_change(id(21), 2, Some(change(&["other"], &["other"]))));
        assert_eq!(page.lines(Version::Before), None);
        assert_eq!(page.lines(Version::After), None);
        page.select(&handled(2));
        assert!(!page.received_change(id(21), 1, Some(change(&["old"], &["old"]))));
        assert_eq!(page.lines(Version::After), None);
    }

    #[test]
    fn a_text_answer_for_the_two_reads_is_refused_once_a_change_is_expected() {
        let mut page = LocalReview::default();
        page.select(&handled(1));
        page.expect(id(10), id(11));
        page.expect_change(id(12));
        assert!(!page.received_before(id(10), 1, words(&["b"])));
        assert!(!page.received_after(id(11), 1, words(&["c"])));
        assert!(page.received_change(id(12), 1, Some(change(&["b"], &["c"]))));
    }

    #[test]
    fn after_names_the_agent_snapshot_once_the_comment_is_handled() {
        let open = comment(1, "a", ReviewStatus::Open, Some(1));
        assert_eq!(caption(Version::After, &open), "Now");
        assert_eq!(
            caption(Version::After, &handled(1)),
            "As it was when the agent marked it handled"
        );
        assert_eq!(
            caption(Version::Before, &handled(1)),
            "As it was when you commented"
        );
        assert_eq!(
            empty_text(Version::After, &handled(1)),
            "The commented text was not in the file when the agent handled it."
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

    fn agent_comment(id: i64, kind: ReviewKind, line: Option<u32>) -> ReviewComment {
        let mut agent = comment(id, "a", ReviewStatus::Open, line);
        agent.author = Some("agent:claude-code".into());
        agent.kind = Some(kind);
        agent
    }

    #[test]
    fn the_needs_answer_and_agent_filters_keep_their_threads() {
        let mut question = agent_comment(1, ReviewKind::Question, Some(1));
        question.needs_answer = true;
        let note = agent_comment(2, ReviewKind::Note, Some(2));
        let person = comment(3, "a", ReviewStatus::Open, Some(3));
        assert!(Filter::NeedsAnswer.keeps(&question));
        assert!(!Filter::NeedsAnswer.keeps(&note));
        assert!(!Filter::NeedsAnswer.keeps(&person));
        assert!(Filter::Agent.keeps(&question));
        assert!(Filter::Agent.keeps(&note));
        assert!(!Filter::Agent.keeps(&person));
        assert!(Filter::All.keeps(&person));
    }

    #[test]
    fn counts_say_how_many_need_an_answer_and_how_many_an_agent_started() {
        let mut question = agent_comment(1, ReviewKind::Question, Some(1));
        question.needs_answer = true;
        let all = [
            question,
            agent_comment(2, ReviewKind::Note, Some(2)),
            comment(3, "a", ReviewStatus::Open, Some(3)),
        ];
        let counted = counts(&all);
        assert_eq!(counted.needs_answer, 1);
        assert_eq!(counted.agent, 2);
        assert_eq!(counted.of(Filter::NeedsAnswer), 1);
        assert_eq!(counted.of(Filter::Agent), 2);
        assert_eq!(
            counted.open, 3,
            "the Open filter still keeps every open thread"
        );
    }

    #[test]
    fn the_pill_counts_the_open_comments_a_person_started_only() {
        let all = [
            comment(1, "a", ReviewStatus::Open, Some(1)),
            agent_comment(2, ReviewKind::Note, Some(2)),
            agent_comment(3, ReviewKind::Flag, Some(3)),
            agent_comment(4, ReviewKind::Question, Some(4)),
        ];
        assert_eq!(open_pill(Some(&all)), Some("1".to_owned()));
        let agent_only = [agent_comment(2, ReviewKind::Note, Some(2))];
        assert_eq!(open_pill(Some(&agent_only)), None);
    }

    #[test]
    fn a_row_of_an_agent_root_carries_its_chips() {
        let mut flag = agent_comment(5, ReviewKind::Flag, Some(2));
        flag.severity = Some(ReviewSeverity::High);
        let listed = rows(&[flag], Filter::Agent);
        let Row::Comment { chips, .. } = &listed[1] else {
            panic!("the second row is the comment");
        };
        assert_eq!(chips.kind, ["Flag", "high"]);
        assert!(chips.agent);
    }

    #[test]
    fn the_filter_labels_and_the_version_captions_are_the_page_words() {
        let labels: Vec<&str> = Filter::ALL.into_iter().map(Filter::label).collect();
        assert_eq!(
            labels,
            [
                "All",
                "Open",
                "Resolved",
                "Outdated",
                "Needs answer",
                "Agent"
            ]
        );
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
