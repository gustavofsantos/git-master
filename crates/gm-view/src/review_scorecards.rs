//! The Review page's scorecards and conversation filters (S5, D219), with no GPUI in them: the
//! table of one row per scorecard name on the branch, its status and trend, the blocking count,
//! the chips of a label and its decorations, the filters Blocking, Scorecard and Label over the
//! conversations, and the read that feeds them. The page draws what this module says.

use std::collections::{BTreeMap, BTreeSet};

use gm_schema::{
    CommentLabel, OperationId, ReviewComment, ReviewSeverity, ReviewStatus, Scorecard,
    ScorecardStatus, TREND_LEN, is_blocking,
};

pub const NO_RUN_TITLE: &str = "No review has run on this branch.";
pub const NO_RUN_HINT: &str = "Choose Run review… to start one.";
pub const READING: &str = "Reading scorecards…";
pub const STALE: &str = "Stale";
pub const BLOCKING: &str = "Blocking";
pub const SCORECARD: &str = "Scorecard";
pub const LABEL: &str = "Label";
/// The length of a short commit id in the expanded row.
const SHORT_HEAD: usize = 7;

/// The colour family of a status chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    Pass,
    NeedsWork,
    Fail,
}

/// A status chip: its words and colour family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusChip {
    pub word: &'static str,
    pub tone: StatusTone,
}

#[must_use]
pub fn status_chip(status: ScorecardStatus) -> StatusChip {
    match status {
        ScorecardStatus::Pass => StatusChip {
            word: "Pass",
            tone: StatusTone::Pass,
        },
        ScorecardStatus::NeedsWork => StatusChip {
            word: "Needs work",
            tone: StatusTone::NeedsWork,
        },
        ScorecardStatus::Fail => StatusChip {
            word: "Fail",
            tone: StatusTone::Fail,
        },
    }
}

/// One dot of the trend: a result, or an empty circle where the name has fewer results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot {
    Result(StatusTone),
    Empty,
}

/// The trend of a name: its last `TREND_LEN` statuses, oldest to newest, so the newest dot is the
/// rightmost. A name with fewer results gets empty circles on the left. `newest_first` is the
/// name's statuses in the order the engine lists them.
#[must_use]
pub fn trend(newest_first: &[ScorecardStatus]) -> Vec<Dot> {
    let shown = newest_first.len().min(TREND_LEN);
    let mut dots = vec![Dot::Empty; TREND_LEN - shown];
    dots.extend(
        newest_first[..shown]
            .iter()
            .rev()
            .map(|status| Dot::Result(status_chip(*status).tone)),
    );
    dots
}

/// One row of the Scorecards table: the newest scorecard of one name on the branch, with its
/// trend and what the expanded row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScorecardRow {
    pub name: String,
    pub status: StatusChip,
    pub stale: bool,
    pub label: CommentLabel,
    pub decorations: Vec<String>,
    pub subject: String,
    pub trend: Vec<Dot>,
    pub blocking_open: u32,
    pub body: String,
    pub head: String,
    pub model: Option<String>,
    pub workflow_run: Option<String>,
}

/// The table for `branch`: one row per name, the newest scorecard of each name, sorted by name.
/// A stale scorecard is still a row, marked `stale`. Rows of other branches are left out.
#[must_use]
pub fn table_of(rows: &[Scorecard], branch: &str) -> Vec<ScorecardRow> {
    let mut by_name: BTreeMap<&str, Vec<&Scorecard>> = BTreeMap::new();
    for row in rows.iter().filter(|row| row.branch == branch) {
        by_name.entry(row.name.as_str()).or_default().push(row);
    }
    by_name
        .into_values()
        .filter_map(|mut group| {
            group.sort_by_key(|row| std::cmp::Reverse((row.created_at, row.id)));
            let newest = *group.first()?;
            let statuses: Vec<ScorecardStatus> = group.iter().map(|row| row.status).collect();
            Some(ScorecardRow {
                name: newest.name.clone(),
                status: status_chip(newest.status),
                stale: newest.stale,
                label: newest.label,
                decorations: newest.decorations.clone(),
                subject: newest.subject.clone(),
                trend: trend(&statuses),
                blocking_open: newest.blocking_open,
                body: newest.body.clone(),
                head: newest.head.chars().take(SHORT_HEAD).collect(),
                model: newest.model.clone(),
                workflow_run: newest.workflow_run.clone(),
            })
        })
        .collect()
}

/// The names on `branch`, sorted: the choices of the Scorecard filter.
#[must_use]
pub fn scorecard_names(rows: &[Scorecard], branch: &str) -> Vec<String> {
    rows.iter()
        .filter(|row| row.branch == branch)
        .map(|row| row.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The name of each scorecard that is listed, by its id: how a comment shows where it came from.
#[must_use]
pub fn names_by_id(rows: &[Scorecard]) -> BTreeMap<i64, String> {
    rows.iter().map(|row| (row.id, row.name.clone())).collect()
}

/// How a label's chip is drawn: by its weight (D219 (5)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    /// Praise, note and thought: a remark, not a request.
    Muted,
    /// Nitpick, suggestion, todo and chore.
    Normal,
    /// Issue: a problem to fix.
    Strong,
    /// Question: waiting for the person.
    Accent,
}

#[must_use]
pub fn label_weight(label: CommentLabel) -> Weight {
    match label {
        CommentLabel::Praise | CommentLabel::Note | CommentLabel::Thought => Weight::Muted,
        CommentLabel::Nitpick
        | CommentLabel::Suggestion
        | CommentLabel::Todo
        | CommentLabel::Chore => Weight::Normal,
        CommentLabel::Issue => Weight::Strong,
        CommentLabel::Question => Weight::Accent,
    }
}

/// A decoration chip: its word, and whether it is `blocking`, drawn as a danger chip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoration {
    pub word: String,
    pub danger: bool,
}

#[must_use]
pub fn decoration_chips(decorations: &[String]) -> Vec<Decoration> {
    decorations
        .iter()
        .map(|word| Decoration {
            word: word.clone(),
            danger: word == "blocking",
        })
        .collect()
}

/// Whether a comment blocks: an open comment that is `blocking` or of high severity. The count
/// of a scorecard counts the same comments (`blocking_open`), so the filter and the table agree.
#[must_use]
pub fn is_blocking_open(comment: &ReviewComment) -> bool {
    comment.status == ReviewStatus::Open
        && (is_blocking(&comment.decorations) || comment.severity == Some(ReviewSeverity::High))
}

/// The Blocking, Scorecard and Label filters over the conversations. They narrow the list
/// together with the filter that is already there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Narrow {
    pub blocking: bool,
    pub scorecard: Option<String>,
    pub label: Option<CommentLabel>,
}

impl Narrow {
    /// Whether the narrowing keeps `comment`. `names` maps a scorecard's id to its name.
    #[must_use]
    pub fn keeps(&self, comment: &ReviewComment, names: &BTreeMap<i64, String>) -> bool {
        (!self.blocking || is_blocking_open(comment))
            && self.scorecard.as_ref().is_none_or(|wanted| {
                comment
                    .scorecard
                    .and_then(|id| names.get(&id))
                    .is_some_and(|name| name == wanted)
            })
            && self
                .label
                .is_none_or(|wanted| comment.label == Some(wanted))
    }
}

/// The next choice of a cycling filter: none, then each choice in order, then none again. An
/// unknown current choice starts again at the first.
#[must_use]
pub fn next_choice<T: PartialEq + Clone>(current: Option<&T>, choices: &[T]) -> Option<T> {
    let next = current
        .and_then(|now| choices.iter().position(|choice| choice == now))
        .map_or(0, |index| index + 1);
    choices.get(next).cloned()
}

/// The labels the conversations carry, in the order of the label list.
#[must_use]
pub fn labels_in(comments: &[ReviewComment]) -> Vec<CommentLabel> {
    let present: BTreeSet<&str> = comments
        .iter()
        .filter_map(|comment| comment.label.map(CommentLabel::as_str))
        .collect();
    LABELS
        .into_iter()
        .filter(|label| present.contains(label.as_str()))
        .collect()
}

const LABELS: [CommentLabel; 9] = [
    CommentLabel::Praise,
    CommentLabel::Nitpick,
    CommentLabel::Suggestion,
    CommentLabel::Issue,
    CommentLabel::Todo,
    CommentLabel::Question,
    CommentLabel::Thought,
    CommentLabel::Chore,
    CommentLabel::Note,
];

/// The number of open blocking comments among `comments`: the Blocking chip's count.
#[must_use]
pub fn blocking_count(comments: &[ReviewComment]) -> usize {
    comments.iter().filter(|c| is_blocking_open(c)).count()
}

/// The scorecards of the page: the last read of the branch's recent scorecards, and which
/// rows are expanded. A read that a newer one has superseded is dropped by its id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScorecardsList {
    read: Option<OperationId>,
    rows: Option<Vec<Scorecard>>,
    expanded: BTreeSet<String>,
}

impl ScorecardsList {
    /// The read asked for now, which supersedes one in flight.
    pub fn request(&mut self, id: OperationId) {
        self.read = Some(id);
    }

    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.read == Some(id)
    }

    /// Keeps the answer of the read `id`; an answer of any other read is dropped.
    pub fn listed(&mut self, id: OperationId, rows: Vec<Scorecard>) -> bool {
        if !self.owns(id) {
            return false;
        }
        self.read = None;
        self.rows = Some(rows);
        true
    }

    /// The rows last listed, or `None` until the first answer.
    #[must_use]
    pub fn rows(&self) -> Option<&[Scorecard]> {
        self.rows.as_deref()
    }

    /// Flips a row between folded and expanded.
    pub fn toggle(&mut self, name: &str) {
        if !self.expanded.remove(name) {
            self.expanded.insert(name.to_owned());
        }
    }

    #[must_use]
    pub fn is_expanded(&self, name: &str) -> bool {
        self.expanded.contains(name)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The text of the expanded row's last line: the commit, the agent, its model and the run.
#[must_use]
pub fn origin_line(row: &ScorecardRow) -> String {
    let mut parts = vec![format!("head {}", row.head)];
    parts.push(crate::review::AGENT_WRITTEN.to_owned());
    if let Some(model) = &row.model {
        parts.push(model.clone());
    }
    if let Some(run) = &row.workflow_run {
        parts.push(format!("run {run}"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{ReviewAnchor, ReviewKind};

    fn scorecard(id: i64, name: &str, branch: &str, status: ScorecardStatus) -> Scorecard {
        Scorecard {
            id,
            name: name.to_owned(),
            branch: branch.to_owned(),
            head: format!("abcdef{id:02}1234"),
            status,
            stale: false,
            blocking_open: 0,
            label: CommentLabel::Note,
            decorations: Vec::new(),
            subject: format!("subject {id}"),
            body: format!("body {id}"),
            origin: "agent:claude-code".to_owned(),
            model: None,
            run_thread: None,
            workflow_run: None,
            created_at: id * 10,
        }
    }

    fn comment(id: i64) -> ReviewComment {
        ReviewComment {
            id,
            path: "a.rs".to_owned(),
            body: "text".to_owned(),
            status: ReviewStatus::Open,
            anchor: ReviewAnchor {
                text: String::new(),
                before: Vec::new(),
                after: Vec::new(),
                start_line: 1,
            },
            created_at: 0,
            resolved_at: None,
            snapshot: None,
            after_snapshot: None,
            location: None,
            outdated: false,
            replies: Vec::new(),
            author: Some("agent:claude-code".to_owned()),
            kind: Some(ReviewKind::Flag),
            severity: None,
            needs_answer: false,
            suggested_fix: None,
            label: None,
            decorations: Vec::new(),
            scorecard: None,
            workspace: None,
        }
    }

    use ScorecardStatus::{Fail, NeedsWork, Pass};

    #[test]
    fn each_status_has_its_words_and_tone() {
        assert_eq!(
            status_chip(Pass),
            StatusChip {
                word: "Pass",
                tone: StatusTone::Pass
            }
        );
        assert_eq!(status_chip(NeedsWork).word, "Needs work");
        assert_eq!(status_chip(NeedsWork).tone, StatusTone::NeedsWork);
        assert_eq!(status_chip(Fail).tone, StatusTone::Fail);
    }

    #[test]
    fn the_trend_is_always_five_dots_oldest_to_newest_with_empty_ones_first() {
        let none = trend(&[]);
        assert_eq!(none, vec![Dot::Empty; 5]);
        let one = trend(&[Fail]);
        assert_eq!(one[..4], [Dot::Empty; 4]);
        assert_eq!(one[4], Dot::Result(StatusTone::Fail));
        // Newest first in, so the newest is the rightmost dot.
        let three = trend(&[Pass, NeedsWork, Fail]);
        assert_eq!(
            three,
            vec![
                Dot::Empty,
                Dot::Empty,
                Dot::Result(StatusTone::Fail),
                Dot::Result(StatusTone::NeedsWork),
                Dot::Result(StatusTone::Pass),
            ]
        );
        for len in 0..=7 {
            let statuses = vec![Pass; len];
            assert_eq!(trend(&statuses).len(), 5, "{len} results");
        }
    }

    #[test]
    fn the_trend_keeps_the_last_five_of_more_results() {
        // Newest first: the newest is a fail, the seventh (oldest) is left out.
        let seven = [Fail, Pass, Pass, Pass, Pass, Pass, NeedsWork];
        let dots = trend(&seven);
        assert_eq!(dots.len(), 5);
        assert!(dots.iter().all(|dot| matches!(dot, Dot::Result(_))));
        assert_eq!(dots[0], Dot::Result(StatusTone::Pass));
        assert_eq!(dots[4], Dot::Result(StatusTone::Fail));
        assert!(
            !dots.contains(&Dot::Result(StatusTone::NeedsWork)),
            "only the newest five results are shown"
        );
    }

    #[test]
    fn the_table_has_the_newest_of_each_name_and_marks_a_stale_one() {
        let mut stale = scorecard(3, "security", "feat", Pass);
        stale.stale = true;
        let rows = vec![
            stale,
            scorecard(2, "security", "feat", Fail),
            scorecard(1, "lint", "feat", NeedsWork),
        ];
        let table = table_of(&rows, "feat");
        assert_eq!(
            table
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            ["lint", "security"]
        );
        let security = &table[1];
        assert_eq!(security.status.word, "Pass");
        assert!(security.stale);
        assert_eq!(security.subject, "subject 3");
        assert_eq!(security.trend.len(), 5);
        assert_eq!(security.trend[4], Dot::Result(StatusTone::Pass));
        assert_eq!(security.trend[3], Dot::Result(StatusTone::Fail));
        assert_eq!(security.head, "abcdef0");
    }

    #[test]
    fn the_table_is_per_branch_so_another_branch_does_not_mix_in() {
        let rows = vec![
            scorecard(1, "security", "other", Fail),
            scorecard(2, "security", "feat", Pass),
        ];
        let table = table_of(&rows, "feat");
        assert_eq!(table.len(), 1);
        assert_eq!(table[0].status.tone, StatusTone::Pass);
        assert_eq!(
            table[0].trend.iter().filter(|d| **d == Dot::Empty).count(),
            4
        );
        assert!(table_of(&rows, "none").is_empty());
    }

    #[test]
    fn the_names_of_a_branch_are_sorted_and_distinct() {
        let rows = vec![
            scorecard(1, "security", "feat", Pass),
            scorecard(2, "lint", "feat", Pass),
            scorecard(3, "security", "feat", Pass),
            scorecard(4, "other", "elsewhere", Pass),
        ];
        assert_eq!(scorecard_names(&rows, "feat"), ["lint", "security"]);
        assert_eq!(
            names_by_id(&rows).get(&4).map(String::as_str),
            Some("other")
        );
    }

    #[test]
    fn a_label_weight_maps_each_label_once() {
        use Weight::{Accent, Muted, Normal, Strong};
        let table = [
            (CommentLabel::Praise, Muted),
            (CommentLabel::Note, Muted),
            (CommentLabel::Thought, Muted),
            (CommentLabel::Nitpick, Normal),
            (CommentLabel::Suggestion, Normal),
            (CommentLabel::Todo, Normal),
            (CommentLabel::Chore, Normal),
            (CommentLabel::Issue, Strong),
            (CommentLabel::Question, Accent),
        ];
        for (label, weight) in table {
            assert_eq!(label_weight(label), weight, "{label:?}");
        }
    }

    #[test]
    fn blocking_is_a_blocking_decoration_or_high_severity_and_open_only() {
        let mut blocking = comment(1);
        blocking.decorations = vec!["blocking".to_owned()];
        assert!(is_blocking_open(&blocking));
        let mut high = comment(2);
        high.severity = Some(ReviewSeverity::High);
        assert!(is_blocking_open(&high));
        assert!(
            !is_blocking_open(&comment(3)),
            "a plain flag does not block"
        );
        let mut handled = blocking.clone();
        handled.status = ReviewStatus::Handled;
        assert!(!is_blocking_open(&handled), "only an open comment counts");
        let mut resolved = high.clone();
        resolved.status = ReviewStatus::Resolved;
        assert!(!is_blocking_open(&resolved));
    }

    #[test]
    fn the_blocking_chip_is_a_danger_chip_and_other_decorations_are_plain() {
        let chips = decoration_chips(&["blocking".to_owned(), "security".to_owned()]);
        assert_eq!(chips[0].word, "blocking");
        assert!(chips[0].danger);
        assert!(!chips[1].danger);
    }

    #[test]
    fn the_narrowing_keeps_what_each_chosen_filter_keeps() {
        let names: BTreeMap<i64, String> = [(7, "security".to_owned())].into();
        let mut from_security = comment(1);
        from_security.scorecard = Some(7);
        from_security.label = Some(CommentLabel::Issue);
        let mut blocking = comment(2);
        blocking.decorations = vec!["blocking".to_owned()];
        let plain = comment(3);

        assert!(Narrow::default().keeps(&plain, &names));
        let only_blocking = Narrow {
            blocking: true,
            ..Narrow::default()
        };
        assert!(only_blocking.keeps(&blocking, &names));
        assert!(!only_blocking.keeps(&plain, &names));

        let security = Narrow {
            scorecard: Some("security".to_owned()),
            ..Narrow::default()
        };
        assert!(security.keeps(&from_security, &names));
        assert!(!security.keeps(&blocking, &names), "not from a scorecard");
        let other = Narrow {
            scorecard: Some("lint".to_owned()),
            ..Narrow::default()
        };
        assert!(!other.keeps(&from_security, &names));

        let issues = Narrow {
            label: Some(CommentLabel::Issue),
            ..Narrow::default()
        };
        assert!(issues.keeps(&from_security, &names));
        assert!(!issues.keeps(&plain, &names));
    }

    #[test]
    fn a_cycling_filter_goes_none_then_each_choice_then_none() {
        let names = vec!["lint".to_owned(), "security".to_owned()];
        assert_eq!(next_choice(None, &names), Some("lint".to_owned()));
        assert_eq!(
            next_choice(Some(&"lint".to_owned()), &names),
            Some("security".to_owned())
        );
        assert_eq!(next_choice(Some(&"security".to_owned()), &names), None);
        assert_eq!(next_choice::<String>(None, &[]), None);
        assert_eq!(
            next_choice(Some(&"gone".to_owned()), &names),
            Some("lint".to_owned()),
            "a choice that is no longer listed starts again"
        );
    }

    #[test]
    fn the_labels_of_the_conversations_are_listed_once_in_label_order() {
        let mut issue = comment(1);
        issue.label = Some(CommentLabel::Issue);
        let mut praise = comment(2);
        praise.label = Some(CommentLabel::Praise);
        let mut another_issue = comment(3);
        another_issue.label = Some(CommentLabel::Issue);
        assert_eq!(
            labels_in(&[issue, praise, another_issue, comment(4)]),
            [CommentLabel::Praise, CommentLabel::Issue]
        );
    }

    #[test]
    fn the_blocking_count_counts_the_same_comments_as_the_filter() {
        let mut blocking = comment(1);
        blocking.decorations = vec!["blocking".to_owned()];
        assert_eq!(blocking_count(&[blocking, comment(2)]), 1);
        assert_eq!(blocking_count(&[]), 0);
    }

    #[test]
    fn the_list_drops_a_read_that_a_newer_one_superseded() {
        let mut list = ScorecardsList::default();
        assert!(list.rows().is_none(), "nothing until the first answer");
        list.request(OperationId(1));
        list.request(OperationId(2));
        assert!(!list.listed(OperationId(1), Vec::new()));
        assert!(list.rows().is_none());
        assert!(list.listed(OperationId(2), vec![scorecard(1, "a", "b", Pass)]));
        assert_eq!(list.rows().map(<[Scorecard]>::len), Some(1));
        assert!(!list.owns(OperationId(2)), "an answer is taken once");
    }

    #[test]
    fn a_row_folds_and_expands_by_its_name() {
        let mut list = ScorecardsList::default();
        assert!(!list.is_expanded("security"));
        list.toggle("security");
        assert!(list.is_expanded("security"));
        list.toggle("security");
        assert!(!list.is_expanded("security"));
    }

    #[test]
    fn the_origin_line_names_the_head_the_agent_and_the_run() {
        let mut row = scorecard(1, "security", "feat", Pass);
        row.model = Some("sonnet".to_owned());
        row.workflow_run = Some("42".to_owned());
        let table = table_of(&[row], "feat");
        assert_eq!(
            origin_line(&table[0]),
            "head abcdef0 · Agent-written · sonnet · run 42"
        );
    }
}
