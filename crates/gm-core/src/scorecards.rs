//! The pure rules of scorecards (D219): when a scorecard is stale, and which scorecards a listing
//! returns. No Git and no store here; the engine reads the branch tips and the rows, and calls
//! these.

use std::collections::BTreeMap;

use gm_schema::Scorecard;

/// The most rows the engine reads from the store before it filters the stale ones out and cuts
/// the list to the caller's limit. Retention keeps 20 per name and branch, so this is ample.
pub const FETCH_LIMIT: usize = 1_000;

/// Whether a scorecard that judged `head` is stale: the branch's tip is not that commit, or the
/// branch no longer exists (`tip` is `None`). Derived on read; a stale scorecard never changes.
#[must_use]
pub fn is_stale(head: &str, tip: Option<&str>) -> bool {
    tip != Some(head)
}

/// Sets `stale` on each row against the tip of its branch in `tips`. A branch missing from `tips`
/// is read as gone, so its rows are stale.
#[must_use]
pub fn with_staleness(
    mut rows: Vec<Scorecard>,
    tips: &BTreeMap<String, Option<String>>,
) -> Vec<Scorecard> {
    for row in &mut rows {
        let tip = tips.get(&row.branch).and_then(|tip| tip.as_deref());
        row.stale = is_stale(&row.head, tip);
    }
    rows
}

/// The rows a listing returns, newest first (the order they came in): stale rows only when
/// `include_stale`, and at most `limit`.
#[must_use]
pub fn listed(rows: Vec<Scorecard>, include_stale: bool, limit: usize) -> Vec<Scorecard> {
    rows.into_iter()
        .filter(|row| include_stale || !row.stale)
        .take(limit)
        .collect()
}

/// The longest run id a scorecard keeps (D219).
pub const WORKFLOW_RUN_MAX: usize = 64;

/// The run id a scorecard is linked to, from the `GITMASTER_WORKFLOW_RUN` a process was started
/// with (D219): the text, trimmed of its blank space, when it is 1 to 64 characters of `a-z`,
/// `A-Z`, `0-9`, `_` or `-`. Anything else is dropped, so nothing else is stored. This is a link
/// for reading, not a check: a run id that names no run is kept as it was given.
#[must_use]
pub fn workflow_run_link(text: &str) -> Option<String> {
    let text = text.trim();
    let valid = (1..=WORKFLOW_RUN_MAX).contains(&text.chars().count())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    valid.then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use gm_schema::{CommentLabel, ScorecardStatus};

    use super::*;

    fn row(id: i64, branch: &str, head: &str) -> Scorecard {
        Scorecard {
            id,
            name: "security".into(),
            branch: branch.into(),
            head: head.into(),
            status: ScorecardStatus::Pass,
            stale: false,
            blocking_open: 0,
            label: CommentLabel::Note,
            decorations: Vec::new(),
            subject: "s".into(),
            body: String::new(),
            origin: "agent:test".into(),
            model: None,
            run_thread: None,
            workflow_run: None,
            created_at: id,
        }
    }

    #[test]
    fn a_scorecard_is_stale_unless_its_head_is_the_tip() {
        assert!(!is_stale("abc", Some("abc")));
        assert!(is_stale("abc", Some("def")));
        assert!(is_stale("abc", None), "a branch that is gone is stale");
    }

    #[test]
    fn rows_are_marked_against_their_branch_tip() {
        let rows = vec![
            row(3, "feat", "new"),
            row(2, "feat", "old"),
            row(1, "gone", "x"),
        ];
        let mut tips = BTreeMap::new();
        tips.insert("feat".to_owned(), Some("new".to_owned()));
        tips.insert("gone".to_owned(), None);
        let marked = with_staleness(rows, &tips);
        let stale: Vec<(i64, bool)> = marked.iter().map(|row| (row.id, row.stale)).collect();
        assert_eq!(stale, [(3, false), (2, true), (1, true)]);
    }

    #[test]
    fn a_listing_hides_stale_rows_unless_asked_and_cuts_to_the_limit() {
        let mut rows = vec![
            row(4, "feat", "new"),
            row(3, "feat", "old"),
            row(2, "feat", "new"),
        ];
        rows[1].stale = true;
        let hidden: Vec<i64> = listed(rows.clone(), false, 20)
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(hidden, [4, 2]);
        let all: Vec<i64> = listed(rows.clone(), true, 20)
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(all, [4, 3, 2]);
        let cut: Vec<i64> = listed(rows, true, 2).iter().map(|r| r.id).collect();
        assert_eq!(cut, [4, 3]);
    }

    #[test]
    fn a_run_id_is_kept_when_it_is_a_short_plain_word_and_dropped_otherwise() {
        assert_eq!(workflow_run_link("abc123").as_deref(), Some("abc123"));
        assert_eq!(
            workflow_run_link("  run-7_x \n").as_deref(),
            Some("run-7_x")
        );
        assert_eq!(workflow_run_link(&"a".repeat(64)), Some("a".repeat(64)));
        assert_eq!(
            workflow_run_link(&"a".repeat(65)),
            None,
            "over 64 characters"
        );
        assert_eq!(workflow_run_link(""), None);
        assert_eq!(workflow_run_link("   "), None);
        assert_eq!(
            workflow_run_link("a b"),
            None,
            "a space inside is not allowed"
        );
        assert_eq!(workflow_run_link("a;rm -rf"), None);
        assert_eq!(
            workflow_run_link("ü"),
            None,
            "only ASCII letters and digits"
        );
    }
}
