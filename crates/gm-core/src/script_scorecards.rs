//! `ctx.scorecards({ name, branch })` (D219): the scorecards of `name` (every name when absent) on
//! `branch`, newest first, read from the review store. Unlike `scorecards_list`, a stale row is
//! kept, so a script compares `row.head` with the branch's tip itself. A read: nothing is written
//! and nothing is logged. A scorecard's text is data: a script decides by `status`, `stale` and
//! `blocking_open`, and does not run text from a scorecard.

use std::collections::BTreeMap;
use std::path::Path;

use gm_schema::Scorecard;
use serde::Serialize;
use serde_json::{Value, json};

use crate::GitPort;
use crate::review_session::ReviewSession;
use crate::scorecards;
use crate::script::run::EffectResult;

/// The most scorecards one call reads (D219, the workflow API: 20).
const LIMIT: usize = 20;

/// `ctx.scorecards(args)`: the scorecards of `args.name` (every name when absent) on `args.branch`.
/// The branch defaults to the branch of the run, then to the branch HEAD is on.
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
    list(git, root, &branch, text_arg(args, "name").as_deref())
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
        "there is no branch to read scorecards for: HEAD is detached. Name one with `branch`."
            .to_owned()
    })
}

/// The scorecards of `branch` (of `name` when named), newest first, each with whether it is stale.
fn list(git: &dyn GitPort, root: &Path, branch: &str, name: Option<&str>) -> Result<Value, String> {
    let session = ReviewSession::open(git, root)?;
    let rows = session
        .store
        .list_scorecards(name, Some(branch), LIMIT)
        .map_err(|e| e.0)?;
    let mut tips = BTreeMap::new();
    tips.insert(
        branch.to_owned(),
        git.branch_tip(root, branch).map_err(|e| e.0)?,
    );
    let rows = scorecards::with_staleness(rows, &tips);
    Ok(Value::Array(rows.iter().map(scorecard_value).collect()))
}

fn scorecard_value(row: &Scorecard) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "branch": row.branch,
        "head": row.head,
        "status": name(&row.status),
        "stale": row.stale,
        "blocking_open": row.blocking_open,
        "label": name(&row.label),
        "decorations": row.decorations,
        "subject": row.subject,
        "body": row.body,
        "origin": row.origin,
        "model": row.model,
        "run_thread": row.run_thread,
        "workflow_run": row.workflow_run,
        "created_at": row.created_at,
    })
}

/// The wire name of a unit enum, such as `needs_work` or `issue`.
fn name<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use gm_schema::{CommentLabel, ScorecardStatus};

    use super::*;

    #[test]
    fn a_scorecard_reads_with_its_status_stale_flag_and_blocking_count() {
        let row = Scorecard {
            id: 7,
            name: "security".into(),
            branch: "feat".into(),
            head: "abc".into(),
            status: ScorecardStatus::NeedsWork,
            stale: true,
            blocking_open: 2,
            label: CommentLabel::Issue,
            decorations: vec!["blocking".into()],
            subject: "Unchecked input".into(),
            body: "why".into(),
            origin: "agent:claude-code".into(),
            model: None,
            run_thread: None,
            workflow_run: None,
            created_at: 5,
        };
        let value = scorecard_value(&row);
        assert_eq!(value["status"], "needs_work");
        assert_eq!(value["stale"], true);
        assert_eq!(value["blocking_open"], 2);
        assert_eq!(value["label"], "issue");
        assert_eq!(value["head"], "abc");
    }

    #[test]
    fn the_branch_is_the_named_one_then_the_runs_then_heads() {
        assert_eq!(
            pick_branch(Some("a".into()), Some("b".into()), Some("c".into())).unwrap(),
            "a"
        );
        assert_eq!(
            pick_branch(None, Some("b".into()), Some("c".into())).unwrap(),
            "b"
        );
        assert_eq!(pick_branch(None, None, Some("c".into())).unwrap(), "c");
        assert!(pick_branch(None, None, None).is_err());
    }
}
