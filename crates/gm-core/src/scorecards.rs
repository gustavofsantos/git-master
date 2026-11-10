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
}
