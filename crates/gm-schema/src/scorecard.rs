//! Scorecards and conventional-comment labels (D219). A scorecard is one review result: a status,
//! a summary with a label and decorations, and the review comments that point at lines of the
//! change. The app never calls a model; an agent publishes a scorecard through MCP, and a workflow
//! reads it back. The rules here are pure: what a label and its decorations mean, and how a
//! comment's kind and severity follow from its label when they are not given.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::review::{ReviewKind, ReviewSeverity};

/// The longest scorecard name, in characters (D219 (4)).
pub const NAME_MAX_CHARS: usize = 40;
/// The longest subject of a scorecard summary, in characters (D219 (6)).
pub const SUBJECT_MAX_CHARS: usize = 120;
/// The longest body of a scorecard summary, in characters (D219 (6)).
pub const BODY_MAX_CHARS: usize = 4_000;
/// The longest body of a scorecard comment, in characters.
pub const COMMENT_BODY_MAX_CHARS: usize = 1_000;
/// The longest suggested fix of a scorecard comment, in characters (D215 (3)).
pub const SUGGESTED_FIX_MAX_CHARS: usize = 1_000;
/// The most comments one scorecard publish carries (D219 (6)).
pub const COMMENTS_MAX: usize = 30;
/// The newest scorecards kept per name and branch (D219 (4)).
pub const RETAINED_PER_NAME_AND_BRANCH: usize = 20;
/// The newest results of each name the Review page shows as trend dots (D219 (11)).
pub const TREND_LEN: usize = 5;
/// The scorecards a listing returns when it names no limit, and the most it returns.
pub const LIST_DEFAULT: u32 = 20;
pub const LIST_MAX: u32 = 20;
/// The most decorations of the agent's own (free words) one comment or summary carries.
pub const FREE_DECORATIONS_MAX: usize = 3;
/// The longest free decoration word, in characters.
pub const DECORATION_WORD_MAX_CHARS: usize = 20;

/// The decoration that makes a comment block (D219 (5)).
pub const BLOCKING: &str = "blocking";
/// The decoration that says a comment does not block.
pub const NON_BLOCKING: &str = "non-blocking";
/// The decoration for a comment that matters only if the change is small.
pub const IF_MINOR: &str = "if-minor";

/// The status of a scorecard: what the reviewer judged of the commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScorecardStatus {
    Pass,
    NeedsWork,
    Fail,
}

impl ScorecardStatus {
    /// The status a tool argument names, in its lower-case wire form.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "pass" => Some(Self::Pass),
            "needs_work" => Some(Self::NeedsWork),
            "fail" => Some(Self::Fail),
            _ => None,
        }
    }

    /// The wire form, as the store keeps it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::NeedsWork => "needs_work",
            Self::Fail => "fail",
        }
    }
}

/// A conventional-comment label (D219 (5)): what kind of remark a summary or a comment makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommentLabel {
    Praise,
    Nitpick,
    Suggestion,
    Issue,
    Todo,
    Question,
    Thought,
    Chore,
    Note,
}

impl CommentLabel {
    /// The label a tool argument names, in its lower-case wire form.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "praise" => Some(Self::Praise),
            "nitpick" => Some(Self::Nitpick),
            "suggestion" => Some(Self::Suggestion),
            "issue" => Some(Self::Issue),
            "todo" => Some(Self::Todo),
            "question" => Some(Self::Question),
            "thought" => Some(Self::Thought),
            "chore" => Some(Self::Chore),
            "note" => Some(Self::Note),
            _ => None,
        }
    }

    /// The wire form, as the store keeps it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Praise => "praise",
            Self::Nitpick => "nitpick",
            Self::Suggestion => "suggestion",
            Self::Issue => "issue",
            Self::Todo => "todo",
            Self::Question => "question",
            Self::Thought => "thought",
            Self::Chore => "chore",
            Self::Note => "note",
        }
    }
}

/// The kind a label gives a comment when no kind is given (D219 (5)): a question is a question,
/// praise, a note and a thought are notes, and the rest are flags.
#[must_use]
pub fn kind_of_label(label: CommentLabel) -> ReviewKind {
    match label {
        CommentLabel::Question => ReviewKind::Question,
        CommentLabel::Praise | CommentLabel::Note | CommentLabel::Thought => ReviewKind::Note,
        CommentLabel::Issue
        | CommentLabel::Nitpick
        | CommentLabel::Suggestion
        | CommentLabel::Todo
        | CommentLabel::Chore => ReviewKind::Flag,
    }
}

/// The severity a flag takes when none is given (D219 (5)): `blocking` is high whatever the
/// label; issue, suggestion and todo are medium; nitpick and chore are low; any other label, and
/// no label, gives none.
#[must_use]
pub fn severity_of(label: Option<CommentLabel>, decorations: &[String]) -> Option<ReviewSeverity> {
    if decorations.iter().any(|item| item == BLOCKING) {
        return Some(ReviewSeverity::High);
    }
    match label? {
        CommentLabel::Issue | CommentLabel::Suggestion | CommentLabel::Todo => {
            Some(ReviewSeverity::Medium)
        }
        CommentLabel::Nitpick | CommentLabel::Chore => Some(ReviewSeverity::Low),
        CommentLabel::Praise
        | CommentLabel::Question
        | CommentLabel::Thought
        | CommentLabel::Note => None,
    }
}

/// The kind of a comment an agent starts: the given kind, else the label's (D219 (5)). Refused
/// when neither is given.
pub fn resolve_kind(
    kind: Option<ReviewKind>,
    label: Option<CommentLabel>,
) -> Result<ReviewKind, String> {
    match (kind, label) {
        (Some(kind), _) => Ok(kind),
        (None, Some(label)) => Ok(kind_of_label(label)),
        (None, None) => {
            Err("a comment needs a kind (note, question or flag) or a label".to_owned())
        }
    }
}

/// The severity of a comment of `kind`: the given one, else, for a flag, the one its label and
/// decorations give (D219 (5)). Only a flag has a severity, so any other kind keeps what was
/// given, and the store refuses a severity on it.
#[must_use]
pub fn resolve_severity(
    kind: ReviewKind,
    severity: Option<ReviewSeverity>,
    label: Option<CommentLabel>,
    decorations: &[String],
) -> Option<ReviewSeverity> {
    match kind {
        ReviewKind::Flag => severity.or_else(|| severity_of(label, decorations)),
        ReviewKind::Note | ReviewKind::Question => severity,
    }
}

/// Checks a scorecard name (D219 (4)): 1 to 40 characters of `a-z`, `0-9` and `-`.
pub fn check_name(name: &str) -> Result<(), String> {
    let length = name.chars().count();
    let plain = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if (1..=NAME_MAX_CHARS).contains(&length) && plain {
        Ok(())
    } else {
        Err(format!(
            "a scorecard name is 1 to {NAME_MAX_CHARS} characters of a to z, 0 to 9 and -, not {name:?}"
        ))
    }
}

/// Checks the decorations of a comment or summary (D219 (5)). Each is `blocking`, `non-blocking`
/// or `if-minor`, or one of at most three free words of `a-z` and `-`, 1 to 20 characters long.
/// `blocking` and `non-blocking` exclude each other, and a decoration is given once.
pub fn check_decorations(items: &[String]) -> Result<(), String> {
    let mut free = 0usize;
    let mut seen: Vec<&str> = Vec::with_capacity(items.len());
    for item in items {
        let known = matches!(item.as_str(), BLOCKING | NON_BLOCKING | IF_MINOR);
        if !known {
            if !is_free_word(item) {
                return Err(format!(
                    "a decoration is blocking, non-blocking, if-minor or a word of a to z and - \
                     of 1 to {DECORATION_WORD_MAX_CHARS} characters, not {item:?}"
                ));
            }
            free += 1;
        }
        if seen.contains(&item.as_str()) {
            return Err(format!("the decoration {item:?} is given twice"));
        }
        seen.push(item);
    }
    if free > FREE_DECORATIONS_MAX {
        return Err(format!(
            "at most {FREE_DECORATIONS_MAX} decorations of your own, not {free}"
        ));
    }
    if seen.contains(&BLOCKING) && seen.contains(&NON_BLOCKING) {
        return Err("blocking and non-blocking exclude each other".to_owned());
    }
    Ok(())
}

/// Whether `text` is a free decoration word: `a-z` and `-`, 1 to 20 characters.
fn is_free_word(text: &str) -> bool {
    let length = text.chars().count();
    (1..=DECORATION_WORD_MAX_CHARS).contains(&length)
        && text.chars().all(|c| c.is_ascii_lowercase() || c == '-')
}

/// Whether `decorations` carries `blocking`.
#[must_use]
pub fn is_blocking(decorations: &[String]) -> bool {
    decorations.iter().any(|item| item == BLOCKING)
}

/// The summary a scorecard publish carries (D219 (6)), as the engine takes it: the label, the
/// decorations, a subject and a body. Text that an agent wrote: it is redacted at ingest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScorecardSummary {
    pub label: CommentLabel,
    #[serde(default)]
    pub decorations: Vec<String>,
    pub subject: String,
    pub body: String,
}

/// A review comment a scorecard publish carries (D219 (6)): lines of a file as it is now, with
/// a label and decorations, and an optional suggested fix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScorecardCommentInput {
    /// Repository-relative path.
    pub path: String,
    /// The first line, 1-based.
    pub start_line: u32,
    /// The last line, 1-based and inclusive.
    pub end_line: u32,
    pub label: CommentLabel,
    #[serde(default)]
    pub decorations: Vec<String>,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_fix: Option<String>,
}

/// One scorecard as the store holds it (D219 (4)), with the two values derived on read:
/// `stale` (the branch has moved since `head`) and `blocking_open` (the open comments of this
/// scorecard that block). The text is written by an agent: untrusted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Scorecard {
    pub id: i64,
    pub name: String,
    pub branch: String,
    /// The commit judged.
    pub head: String,
    pub status: ScorecardStatus,
    /// True when the branch's tip is no longer `head`, or the branch is gone. Derived on read.
    pub stale: bool,
    /// The open comments of this scorecard that are blocking or of high severity. Derived.
    pub blocking_open: u32,
    pub label: CommentLabel,
    pub decorations: Vec<String>,
    pub subject: String,
    pub body: String,
    /// Who published it: `agent:<client>`.
    pub origin: String,
    pub model: Option<String>,
    /// The uid of the run's own thread, when the agent reported one.
    pub run_thread: Option<String>,
    /// The id of the workflow run that started the review, when there is one.
    pub workflow_run: Option<String>,
    /// Milliseconds since the Unix epoch, as the engine's clock gave them.
    pub created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn labels_and_statuses_parse_only_their_wire_forms() {
        for label in [
            "praise",
            "nitpick",
            "suggestion",
            "issue",
            "todo",
            "question",
            "thought",
            "chore",
            "note",
        ] {
            assert_eq!(
                CommentLabel::parse(label).map(CommentLabel::as_str),
                Some(label)
            );
        }
        assert_eq!(CommentLabel::parse("Issue"), None);
        assert_eq!(CommentLabel::parse("nit"), None);
        assert_eq!(
            ScorecardStatus::parse("needs_work"),
            Some(ScorecardStatus::NeedsWork)
        );
        assert_eq!(ScorecardStatus::parse("needs-work"), None);
        assert_eq!(ScorecardStatus::parse("PASS"), None);
        assert_eq!(ScorecardStatus::Fail.as_str(), "fail");
    }

    #[test]
    fn a_label_gives_its_kind() {
        let table = [
            (CommentLabel::Question, ReviewKind::Question),
            (CommentLabel::Praise, ReviewKind::Note),
            (CommentLabel::Note, ReviewKind::Note),
            (CommentLabel::Thought, ReviewKind::Note),
            (CommentLabel::Issue, ReviewKind::Flag),
            (CommentLabel::Nitpick, ReviewKind::Flag),
            (CommentLabel::Suggestion, ReviewKind::Flag),
            (CommentLabel::Todo, ReviewKind::Flag),
            (CommentLabel::Chore, ReviewKind::Flag),
        ];
        for (label, kind) in table {
            assert_eq!(kind_of_label(label), kind, "{label:?}");
        }
    }

    #[test]
    fn a_label_and_decoration_give_a_severity_to_a_flag_only_by_the_table() {
        let none: Vec<String> = Vec::new();
        let blocking = words(&["blocking"]);
        let table = [
            (
                Some(CommentLabel::Issue),
                none.clone(),
                Some(ReviewSeverity::Medium),
            ),
            (
                Some(CommentLabel::Suggestion),
                none.clone(),
                Some(ReviewSeverity::Medium),
            ),
            (
                Some(CommentLabel::Todo),
                none.clone(),
                Some(ReviewSeverity::Medium),
            ),
            (
                Some(CommentLabel::Nitpick),
                none.clone(),
                Some(ReviewSeverity::Low),
            ),
            (
                Some(CommentLabel::Chore),
                none.clone(),
                Some(ReviewSeverity::Low),
            ),
            (
                Some(CommentLabel::Nitpick),
                blocking.clone(),
                Some(ReviewSeverity::High),
            ),
            (
                Some(CommentLabel::Issue),
                blocking.clone(),
                Some(ReviewSeverity::High),
            ),
            (Some(CommentLabel::Praise), none.clone(), None),
            (Some(CommentLabel::Question), none.clone(), None),
            (Some(CommentLabel::Thought), none.clone(), None),
            (Some(CommentLabel::Note), none.clone(), None),
            (None, blocking.clone(), Some(ReviewSeverity::High)),
            (None, none.clone(), None),
        ];
        for (label, decorations, severity) in table {
            assert_eq!(
                severity_of(label, &decorations),
                severity,
                "{label:?} {decorations:?}"
            );
        }
    }

    #[test]
    fn a_given_kind_or_severity_wins_over_the_label() {
        assert_eq!(
            resolve_kind(Some(ReviewKind::Note), Some(CommentLabel::Issue)).unwrap(),
            ReviewKind::Note
        );
        assert_eq!(
            resolve_kind(None, Some(CommentLabel::Issue)).unwrap(),
            ReviewKind::Flag
        );
        assert!(resolve_kind(None, None).is_err());
        let issue = Some(CommentLabel::Issue);
        assert_eq!(
            resolve_severity(ReviewKind::Flag, Some(ReviewSeverity::Low), issue, &[]),
            Some(ReviewSeverity::Low)
        );
        assert_eq!(
            resolve_severity(ReviewKind::Flag, None, issue, &[]),
            Some(ReviewSeverity::Medium)
        );
        assert_eq!(resolve_severity(ReviewKind::Flag, None, None, &[]), None);
        assert_eq!(
            resolve_severity(ReviewKind::Note, None, Some(CommentLabel::Issue), &[]),
            None,
            "only a flag takes a severity"
        );
    }

    #[test]
    fn names_are_one_to_forty_lowercase_letters_digits_and_dashes() {
        let longest = "a".repeat(NAME_MAX_CHARS);
        let too_long = "a".repeat(NAME_MAX_CHARS + 1);
        for name in ["security", "code-review", "a", "v2-check", longest.as_str()] {
            assert!(check_name(name).is_ok(), "{name}");
        }
        for name in [
            "",
            "Security",
            "code_review",
            "a b",
            "a/b",
            too_long.as_str(),
        ] {
            assert!(check_name(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn decorations_follow_the_rules_of_the_label_table() {
        assert!(check_decorations(&[]).is_ok());
        assert!(check_decorations(&words(&["blocking", "security"])).is_ok());
        assert!(check_decorations(&words(&["non-blocking", "if-minor"])).is_ok());
        assert!(check_decorations(&words(&["security", "auth", "api"])).is_ok());
        assert!(check_decorations(&words(&["security", "auth", "api", "db"])).is_err());
        assert!(check_decorations(&words(&["blocking", "non-blocking"])).is_err());
        assert!(check_decorations(&words(&["security", "security"])).is_err());
        assert!(check_decorations(&words(&["Security"])).is_err());
        assert!(check_decorations(&words(&["sec urity"])).is_err());
        assert!(check_decorations(&words(&[""])).is_err());
        assert!(check_decorations(&[format!("{}x", "a".repeat(20))]).is_err());
    }

    #[test]
    fn blocking_is_read_from_the_decorations() {
        assert!(is_blocking(&words(&["security", "blocking"])));
        assert!(!is_blocking(&words(&["non-blocking"])));
        assert!(!is_blocking(&[]));
    }

    #[test]
    fn a_scorecard_row_reads_its_status_in_snake_case() {
        let json = serde_json::to_value(ScorecardStatus::NeedsWork).unwrap();
        assert_eq!(json, "needs_work");
        let label: CommentLabel = serde_json::from_str("\"todo\"").unwrap();
        assert_eq!(label, CommentLabel::Todo);
    }
}
