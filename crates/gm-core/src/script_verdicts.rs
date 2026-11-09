//! `ctx.verdicts({ reviewer, branch })` (D212 (3), (5)): the verdicts a reviewer recorded on a
//! branch, newest first, read from the review store. A read: nothing is written and nothing is
//! logged. A verdict's text is data (the workflow API, Verdicts): a script decides by `value`,
//! `stale` and the severities, and does not run text from a verdict.

use std::collections::BTreeMap;
use std::path::Path;

use gm_schema::{ReviewComment, Verdict, VerdictFinding};
use serde::Serialize;
use serde_json::{Value, json};

use crate::GitPort;
use crate::review_session::{self, ReviewSession};
use crate::script::run::EffectResult;
use crate::verdicts;

/// The most verdicts one call reads: the bound the gate check reads too (D212 (5)).
const LIMIT: usize = 1_000;

/// `ctx.verdicts(args)`: the verdicts of `args.reviewer` (every reviewer when absent) on
/// `args.branch`. The branch defaults to the branch of the run, then to the branch HEAD is on.
pub(crate) fn read(
    git: &dyn GitPort,
    root: &Path,
    args: &Value,
    run_branch: Option<String>,
) -> EffectResult {
    match listed_for_call(git, root, args, run_branch) {
        Ok(value) => EffectResult::done(value),
        Err(message) => EffectResult::failed(message),
    }
}

fn listed_for_call(
    git: &dyn GitPort,
    root: &Path,
    args: &Value,
    run_branch: Option<String>,
) -> Result<Value, String> {
    let named = text_arg(args, "branch");
    let head = match (&named, &run_branch) {
        (None, None) => git.status(root).map_err(|e| e.0)?.branch.head,
        _ => None,
    };
    let branch = pick_branch(named, run_branch, head)?;
    list(git, root, &branch, text_arg(args, "reviewer").as_deref())
}

fn text_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// The branch a call reads: the one it names, else the run's, else HEAD's.
fn pick_branch(
    named: Option<String>,
    run_branch: Option<String>,
    head: Option<String>,
) -> Result<String, String> {
    named.or(run_branch).or(head).ok_or_else(|| {
        "there is no branch to read verdicts for: HEAD is detached. Name one with `branch`."
            .to_owned()
    })
}

/// The verdicts of `branch` (of `reviewer` when named), newest first, each with its staleness and
/// the current line of each finding.
fn list(
    git: &dyn GitPort,
    root: &Path,
    branch: &str,
    reviewer: Option<&str>,
) -> Result<Value, String> {
    let session = ReviewSession::open(git, root)?;
    let found = session
        .store
        .list_verdicts(Some(branch), reviewer, LIMIT)
        .map_err(|e| e.0)?;
    let tip = git.branch_tip(root, branch).map_err(|e| e.0)?;
    let lines = start_lines(&review_session::listed(&session.store, root)?);
    Ok(Value::Array(
        found
            .iter()
            .map(|verdict| verdict_value(verdict, tip.as_deref(), &lines))
            .collect(),
    ))
}

/// The current start line of each comment that is located in its file. An outdated comment has
/// none, so its finding reads `nil`.
fn start_lines(comments: &[ReviewComment]) -> BTreeMap<i64, u32> {
    comments
        .iter()
        .filter_map(|comment| {
            comment
                .location
                .as_ref()
                .map(|location| (comment.id, location.start_line))
        })
        .collect()
}

fn verdict_value(verdict: &Verdict, tip: Option<&str>, lines: &BTreeMap<i64, u32>) -> Value {
    json!({
        "reviewer": verdict.reviewer,
        "signal": name(&verdict.signal),
        "value": verdict.value,
        "stale": verdicts::is_stale(&verdict.head, tip),
        "head": verdict.head,
        "rationale": verdict.rationale,
        "findings": verdict
            .findings
            .iter()
            .map(|finding| finding_value(finding, lines))
            .collect::<Vec<Value>>(),
    })
}

fn finding_value(finding: &VerdictFinding, lines: &BTreeMap<i64, u32>) -> Value {
    json!({
        "severity": name(&finding.severity),
        "path": finding.path,
        "start_line": lines.get(&finding.comment_id).copied(),
        "status": name(&finding.status),
    })
}

/// The wire name of a unit enum, such as `high` or `open`.
fn name<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use gm_schema::{ReviewAnchor, ReviewLocation, ReviewSeverity, ReviewStatus, ReviewerSignal};

    use super::*;

    fn verdict(head: &str, findings: Vec<VerdictFinding>) -> Verdict {
        Verdict {
            id: 7,
            reviewer: "security".into(),
            branch: "feat".into(),
            head: head.into(),
            signal: ReviewerSignal::Level,
            value: "high".into(),
            rationale: "Unchecked input.".into(),
            origin: "agent:test".into(),
            model: None,
            run_thread: None,
            at: 0,
            stale: false,
            findings,
        }
    }

    fn finding(comment_id: i64, severity: ReviewSeverity) -> VerdictFinding {
        VerdictFinding {
            comment_id,
            severity,
            path: "src/a.rs".into(),
            status: ReviewStatus::Open,
            start_line: None,
            body: String::new(),
            suggested_fix: None,
        }
    }

    #[test]
    fn the_named_branch_wins_then_the_runs_then_head() {
        assert_eq!(
            pick_branch(Some("x".into()), Some("run".into()), Some("head".into())).unwrap(),
            "x"
        );
        assert_eq!(
            pick_branch(None, Some("run".into()), Some("head".into())).unwrap(),
            "run"
        );
        assert_eq!(
            pick_branch(None, None, Some("head".into())).unwrap(),
            "head"
        );
        let detached = pick_branch(None, None, None).unwrap_err();
        assert!(detached.contains("Name one with `branch`"), "{detached}");
    }

    #[test]
    fn a_verdict_at_the_tip_is_not_stale_and_one_elsewhere_is() {
        let lines = BTreeMap::new();
        let current = verdict_value(&verdict("abc", Vec::new()), Some("abc"), &lines);
        assert_eq!(current["stale"], json!(false));
        let moved = verdict_value(&verdict("abc", Vec::new()), Some("def"), &lines);
        assert_eq!(moved["stale"], json!(true));
        let gone = verdict_value(&verdict("abc", Vec::new()), None, &lines);
        assert_eq!(gone["stale"], json!(true));
    }

    #[test]
    fn a_verdict_has_the_fields_the_workflow_api_names() {
        let lines = BTreeMap::new();
        let value = verdict_value(&verdict("abc", Vec::new()), Some("abc"), &lines);
        assert_eq!(value["reviewer"], json!("security"));
        assert_eq!(value["signal"], json!("level"));
        assert_eq!(value["value"], json!("high"));
        assert_eq!(value["head"], json!("abc"));
        assert_eq!(value["rationale"], json!("Unchecked input."));
        assert_eq!(value["findings"], json!([]));
    }

    #[test]
    fn a_finding_carries_its_severity_path_status_and_current_line() {
        let lines = BTreeMap::from([(11, 42)]);
        let value = verdict_value(
            &verdict(
                "abc",
                vec![
                    finding(11, ReviewSeverity::High),
                    finding(12, ReviewSeverity::Low),
                ],
            ),
            Some("abc"),
            &lines,
        );
        assert_eq!(
            value["findings"],
            json!([
                { "severity": "high", "path": "src/a.rs", "start_line": 42, "status": "open" },
                { "severity": "low", "path": "src/a.rs", "start_line": null, "status": "open" }
            ])
        );
    }

    #[test]
    fn only_located_comments_have_a_start_line() {
        let located = ReviewComment {
            id: 3,
            location: Some(ReviewLocation {
                start_line: 9,
                end_line: 10,
            }),
            ..blank_comment()
        };
        let outdated = ReviewComment {
            id: 4,
            location: None,
            outdated: true,
            ..blank_comment()
        };
        let lines = start_lines(&[located, outdated]);
        assert_eq!(lines.get(&3), Some(&9));
        assert_eq!(lines.get(&4), None);
    }

    fn blank_comment() -> ReviewComment {
        ReviewComment {
            id: 0,
            path: "src/a.rs".into(),
            body: String::new(),
            status: ReviewStatus::Open,
            anchor: ReviewAnchor {
                text: "x".into(),
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
            author: None,
            kind: None,
            severity: None,
            needs_answer: false,
            suggested_fix: None,
            finding: false,
            workspace: None,
        }
    }
}
