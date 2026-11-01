//! Phases of the operations (design/workflow-scripts.md, `WF3c`, D157): which operations have
//! `before.<op>` gates and `after.<op>` observers, what each phase reads as `input.trigger`, which
//! scripts run for it, and the one-line texts a veto shows. Pure: the engine reads the scripts,
//! runs them and acts on the answers, so every rule has a test without a repository or a window.

use gm_schema::{CommandKind, RestackPlan, RestackRecord};
use serde_json::{Value, json};

use super::journal::RunStatus;
use super::loader::ScriptEntry;
use super::triggers::{self, TriggerEvent};

/// When a phase runs: before the operation (a gate) or after it succeeded (an observer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    Before,
    After,
}

impl Timing {
    #[must_use]
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
        }
    }
}

/// The operation a command is, as a phase names it: `commit`, `restack`, `branch_create` or
/// `conflict_resolve`. `None` for a command that has no phases.
#[must_use]
pub fn operation(kind: &CommandKind) -> Option<&'static str> {
    Some(match kind {
        CommandKind::Commit { .. } => "commit",
        CommandKind::ApplyRestack { .. } => "restack",
        CommandKind::CreateBranch { .. } | CommandKind::CreateChildBranch { .. } => "branch_create",
        CommandKind::ResolveConflict { .. } => "conflict_resolve",
        _ => return None,
    })
}

/// What a phase knows about its operation. Before the operation runs: `head` and `planned`.
/// After it succeeded: `oid` and `moved` as well.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The branch HEAD is on, `None` when detached.
    pub head: Option<String>,
    /// The branches a restack plan moves, in plan order.
    pub planned: Vec<String>,
    /// The branches a restack moved, from its record.
    pub moved: Vec<String>,
    /// The commit a commit made, or the tip of a new branch, when there is one.
    pub oid: Option<String>,
}

/// The branch a `match` filter looks at for the phase of `kind`. `None` when the operation has no
/// branch of its own, which only a script without `match` can run for.
#[must_use]
pub fn branch_of(kind: &CommandKind, facts: &Facts) -> Option<String> {
    match kind {
        CommandKind::Commit { .. } => facts.head.clone(),
        CommandKind::ApplyRestack { branch, .. } => Some(branch.clone()),
        CommandKind::CreateBranch { name, .. } | CommandKind::CreateChildBranch { name, .. } => {
            Some(name.clone())
        }
        _ => None,
    }
}

/// The fields a phase reads as `input.trigger`, besides `on` (which `TriggerEvent` adds). A
/// `before` payload has no `oid` and no `moved`; an `after` payload adds them where the operation
/// makes them.
#[must_use]
pub fn payload(kind: &CommandKind, facts: &Facts, timing: Timing) -> Option<Value> {
    let mut fields = match kind {
        CommandKind::Commit { message } => json!({ "message": message, "branch": facts.head }),
        CommandKind::ApplyRestack { branch, .. } => {
            json!({ "root": branch, "branches": facts.planned })
        }
        CommandKind::CreateBranch {
            name, start_point, ..
        } => json!({ "name": name, "start_point": start_point }),
        CommandKind::CreateChildBranch { name, parent, .. } => {
            json!({ "name": name, "start_point": parent, "parent": parent })
        }
        CommandKind::ResolveConflict { path, site, .. } => json!({ "path": path, "site": site }),
        _ => return None,
    };
    if timing == Timing::After {
        let map = fields.as_object_mut()?;
        match kind {
            CommandKind::Commit { .. }
            | CommandKind::CreateBranch { .. }
            | CommandKind::CreateChildBranch { .. } => {
                map.insert("oid".to_owned(), json!(facts.oid));
            }
            CommandKind::ApplyRestack { .. } => {
                map.insert("moved".to_owned(), json!(facts.moved));
            }
            _ => {}
        }
    }
    Some(fields)
}

/// The event a phase of `kind` fires: `before.<op>` or `after.<op>`, with its branch and payload.
#[must_use]
pub fn trigger(kind: &CommandKind, facts: &Facts, timing: Timing) -> Option<TriggerEvent> {
    let op = operation(kind)?;
    Some(TriggerEvent {
        on: format!("{}.{op}", timing.prefix()),
        branch: branch_of(kind, facts),
        payload: payload(kind, facts, timing)?,
    })
}

/// The scripts that run for `event`: each loads, names the event as its trigger and passes its
/// `match`. In file-name order, as the gates run (D157(1)). Returns the name and the hash of each.
#[must_use]
pub fn select(entries: &[ScriptEntry], event: &TriggerEvent) -> Vec<(String, String)> {
    let mut chosen: Vec<&ScriptEntry> = entries
        .iter()
        .filter(|entry| {
            entry
                .meta
                .as_ref()
                .is_ok_and(|meta| triggers::matches(meta, event))
        })
        .collect();
    chosen.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    chosen
        .into_iter()
        .map(|entry| (entry.name.clone(), entry.hash.clone()))
        .collect()
}

/// The branches a restack plan moves: the ones `ApplyRestack` replays, in plan order.
#[must_use]
pub fn moved_by_plan(plan: &RestackPlan) -> Vec<String> {
    plan.branches
        .iter()
        .filter(|branch| branch.block.is_none() && branch.new_tip.is_some())
        .map(|branch| branch.name.clone())
        .collect()
}

/// The branches a restack record says it moved.
#[must_use]
pub fn moved_by_record(record: &RestackRecord) -> Vec<String> {
    record
        .moves
        .iter()
        .map(|moved| moved.name.clone())
        .collect()
}

/// The noun of the operation in the sentences the window shows.
#[must_use]
pub fn subject(op: &str) -> &'static str {
    match op {
        "commit" => "the commit",
        "restack" => "the restack",
        "branch_create" => "the branch",
        _ => "the conflict resolution",
    }
}

/// What a gate's run ended with (D157(1)): only a clean finish lets the operation go on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    Passed,
    /// The operation does not run; the text is the one line the window shows.
    Vetoed(String),
}

/// The answer of a gate `gate` (the workflow's name) that ended `status` on `op`. A block or a
/// failure names the workflow and its reason; a cancelled or interrupted run says the operation
/// did not run.
#[must_use]
pub fn gate_answer(gate: &str, op: &str, status: RunStatus, reason: Option<&str>) -> Gate {
    let subject = subject(op);
    let reason = one_line(reason.unwrap_or(""));
    let reason = if reason.is_empty() {
        "no reason was given"
    } else {
        reason.as_str()
    };
    Gate::Vetoed(match status {
        RunStatus::Done => return Gate::Passed,
        RunStatus::Blocked => format!("{gate} blocked {subject}: {reason}"),
        RunStatus::Cancelled => format!("{gate} was cancelled: {subject} did not run."),
        RunStatus::Interrupted => format!("{gate} was interrupted: {subject} did not run."),
        RunStatus::Running
        | RunStatus::AwaitingApproval
        | RunStatus::Waiting
        | RunStatus::Failed => {
            format!("{gate} failed: {reason}")
        }
    })
}

/// The veto of a gate that is not reviewed: it does not run (D157(2)).
#[must_use]
pub fn unreviewed(gate: &str) -> String {
    format!("{gate} is not reviewed: review it, then try again.")
}

/// The warning an observer's failure shows (D157(4)). `None` when the observer finished cleanly.
#[must_use]
pub fn observer_failure(
    name: &str,
    op: &str,
    status: RunStatus,
    reason: Option<&str>,
) -> Option<String> {
    if status == RunStatus::Done {
        return None;
    }
    let reason = one_line(reason.unwrap_or(""));
    let reason = if reason.is_empty() {
        "no reason was given"
    } else {
        reason.as_str()
    };
    Some(format!("{name} failed after {}: {reason}", subject(op)))
}

/// The text on one line: a reason that spans lines reads as one.
pub(super) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gm_schema::{ConflictSite, RestackPhase};
    use gm_schema::{PlannedBranch, RestackBlock, RestackMove, RestackPlan, RestackRecord};
    use serde_json::json;

    use super::*;
    use crate::script::meta::parse;

    fn commit(message: &str) -> CommandKind {
        CommandKind::Commit {
            message: message.to_owned(),
        }
    }

    fn restack(branch: &str) -> CommandKind {
        CommandKind::ApplyRestack {
            branch: branch.to_owned(),
            plan_id: "plan".to_owned(),
            skip: Vec::new(),
        }
    }

    fn entry(file: &str, source_on: &str, filter: Option<&str>) -> ScriptEntry {
        let mut trigger = json!({ "on": source_on });
        if let Some(rule) = filter {
            trigger["match"] = json!(rule);
        }
        ScriptEntry {
            path: PathBuf::from(file),
            file_name: file.to_owned(),
            name: file.trim_end_matches(".lua").to_owned(),
            hash: format!("hash-{file}"),
            source: String::new(),
            meta: parse(&json!({ "name": file, "trigger": trigger })).map_err(|_| Vec::new()),
        }
    }

    #[test]
    fn four_operations_have_phases_and_the_rest_do_not() {
        assert_eq!(operation(&commit("m")), Some("commit"));
        assert_eq!(operation(&restack("main")), Some("restack"));
        assert_eq!(
            operation(&CommandKind::CreateBranch {
                name: "b".into(),
                start_point: None,
                checkout: false,
            }),
            Some("branch_create")
        );
        assert_eq!(
            operation(&CommandKind::CreateChildBranch {
                name: "b".into(),
                parent: "main".into(),
                checkout: false,
            }),
            Some("branch_create")
        );
        assert_eq!(
            operation(&CommandKind::ResolveConflict {
                path: "a".into(),
                text: String::new(),
                origins: Vec::new(),
                site: ConflictSite::Restack,
            }),
            Some("conflict_resolve")
        );
        assert_eq!(operation(&CommandKind::StageAll), None);
        assert_eq!(operation(&CommandKind::ListBranches), None);
        assert_eq!(
            operation(&CommandKind::CancelOperation {
                op: gm_schema::OperationId(1),
            }),
            None
        );
    }

    #[test]
    fn a_commit_gate_reads_its_message_and_branch_and_the_observer_its_oid() {
        let facts = Facts {
            head: Some("feat".into()),
            oid: Some("abc".into()),
            ..Facts::default()
        };
        assert_eq!(
            payload(&commit("fix"), &facts, Timing::Before),
            Some(json!({ "message": "fix", "branch": "feat" }))
        );
        assert_eq!(
            payload(&commit("fix"), &facts, Timing::After),
            Some(json!({ "message": "fix", "branch": "feat", "oid": "abc" }))
        );
    }

    #[test]
    fn a_detached_commit_has_no_branch_and_no_oid_before_it_is_made() {
        let facts = Facts::default();
        assert_eq!(
            payload(&commit("fix"), &facts, Timing::Before),
            Some(json!({ "message": "fix", "branch": null }))
        );
        assert_eq!(
            payload(&commit("fix"), &facts, Timing::After),
            Some(json!({ "message": "fix", "branch": null, "oid": null }))
        );
    }

    #[test]
    fn a_restack_gate_reads_its_root_and_the_branches_it_moves() {
        let facts = Facts {
            planned: vec!["a".into(), "b".into()],
            moved: vec!["a".into()],
            ..Facts::default()
        };
        assert_eq!(
            payload(&restack("main"), &facts, Timing::Before),
            Some(json!({ "root": "main", "branches": ["a", "b"] }))
        );
        assert_eq!(
            payload(&restack("main"), &facts, Timing::After),
            Some(json!({ "root": "main", "branches": ["a", "b"], "moved": ["a"] }))
        );
    }

    #[test]
    fn a_child_branch_starts_at_its_parent_and_a_plain_one_has_no_parent() {
        let facts = Facts::default();
        let plain = CommandKind::CreateBranch {
            name: "x".into(),
            start_point: Some("main".into()),
            checkout: true,
        };
        assert_eq!(
            payload(&plain, &facts, Timing::Before),
            Some(json!({ "name": "x", "start_point": "main" }))
        );
        let child = CommandKind::CreateChildBranch {
            name: "x".into(),
            parent: "a".into(),
            checkout: false,
        };
        assert_eq!(
            payload(&child, &facts, Timing::Before),
            Some(json!({ "name": "x", "start_point": "a", "parent": "a" }))
        );
        let facts = Facts {
            oid: Some("tip".into()),
            ..Facts::default()
        };
        assert_eq!(
            payload(&child, &facts, Timing::After),
            Some(json!({ "name": "x", "start_point": "a", "parent": "a", "oid": "tip" }))
        );
    }

    #[test]
    fn a_conflict_phase_reads_the_same_before_and_after() {
        let kind = CommandKind::ResolveConflict {
            path: "src/a.rs".into(),
            text: String::new(),
            origins: Vec::new(),
            site: ConflictSite::Restack,
        };
        let facts = Facts {
            oid: Some("unused".into()),
            ..Facts::default()
        };
        let before = payload(&kind, &facts, Timing::Before);
        assert_eq!(
            before,
            Some(json!({ "path": "src/a.rs", "site": "restack" }))
        );
        assert_eq!(payload(&kind, &facts, Timing::After), before);
    }

    #[test]
    fn a_phase_event_is_named_by_its_timing_and_operation() {
        let facts = Facts {
            head: Some("main".into()),
            ..Facts::default()
        };
        let before = trigger(&commit("m"), &facts, Timing::Before).unwrap();
        assert_eq!(before.on, "before.commit");
        assert_eq!(before.branch.as_deref(), Some("main"));
        let after = trigger(&restack("feat"), &facts, Timing::After).unwrap();
        assert_eq!(after.on, "after.restack");
        assert_eq!(after.branch.as_deref(), Some("feat"));
        let conflict = CommandKind::ResolveConflict {
            path: "a".into(),
            text: String::new(),
            origins: Vec::new(),
            site: ConflictSite::WorkingCopy,
        };
        assert_eq!(
            trigger(&conflict, &facts, Timing::Before).unwrap().branch,
            None
        );
        assert!(trigger(&CommandKind::StageAll, &facts, Timing::Before).is_none());
    }

    #[test]
    fn a_branch_create_phase_matches_the_branch_it_creates() {
        let kind = CommandKind::CreateBranch {
            name: "feat/x".into(),
            start_point: None,
            checkout: false,
        };
        assert_eq!(
            branch_of(&kind, &Facts::default()).as_deref(),
            Some("feat/x")
        );
    }

    #[test]
    fn scripts_run_in_file_name_order_and_only_when_they_match() {
        let entries = [
            entry("c.lua", "before.commit", None),
            entry("a.lua", "before.commit", None),
            entry("b.lua", "before.commit", Some("branch:feat/*")),
            entry("d.lua", "after.commit", None),
            entry("e.lua", "before.restack", None),
        ];
        let event = TriggerEvent {
            on: "before.commit".into(),
            branch: Some("main".into()),
            payload: json!({}),
        };
        let chosen: Vec<String> = select(&entries, &event)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(chosen, ["a", "c"]);
        let on_feat = TriggerEvent {
            branch: Some("feat/one".into()),
            ..event
        };
        let chosen: Vec<String> = select(&entries, &on_feat)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(chosen, ["a", "b", "c"]);
    }

    #[test]
    fn a_script_that_does_not_load_is_never_chosen() {
        let mut broken = entry("a.lua", "before.commit", None);
        broken.meta = Err(Vec::new());
        let event = TriggerEvent {
            on: "before.commit".into(),
            branch: None,
            payload: json!({}),
        };
        assert!(select(&[broken], &event).is_empty());
    }

    #[test]
    fn a_gate_that_blocks_or_fails_vetoes_with_its_reason() {
        assert_eq!(
            gate_answer(
                "freeze",
                "commit",
                RunStatus::Blocked,
                Some("Commits are frozen")
            ),
            Gate::Vetoed("freeze blocked the commit: Commits are frozen".to_owned())
        );
        assert_eq!(
            gate_answer("freeze", "restack", RunStatus::Blocked, Some("no")),
            Gate::Vetoed("freeze blocked the restack: no".to_owned())
        );
        assert_eq!(
            gate_answer("freeze", "branch_create", RunStatus::Blocked, Some("no")),
            Gate::Vetoed("freeze blocked the branch: no".to_owned())
        );
        assert_eq!(
            gate_answer("lint", "commit", RunStatus::Failed, Some("boom")),
            Gate::Vetoed("lint failed: boom".to_owned())
        );
        assert_eq!(
            gate_answer("lint", "commit", RunStatus::Failed, None),
            Gate::Vetoed("lint failed: no reason was given".to_owned())
        );
    }

    #[test]
    fn a_multi_line_reason_reads_as_one_line() {
        assert_eq!(
            gate_answer(
                "freeze",
                "commit",
                RunStatus::Blocked,
                Some("No\n  commits\tnow")
            ),
            Gate::Vetoed("freeze blocked the commit: No commits now".to_owned())
        );
    }

    #[test]
    fn a_cancelled_or_interrupted_gate_says_the_operation_did_not_run() {
        assert_eq!(
            gate_answer("freeze", "commit", RunStatus::Cancelled, Some("cancelled")),
            Gate::Vetoed("freeze was cancelled: the commit did not run.".to_owned())
        );
        assert_eq!(
            gate_answer("freeze", "restack", RunStatus::Interrupted, None),
            Gate::Vetoed("freeze was interrupted: the restack did not run.".to_owned())
        );
    }

    #[test]
    fn a_clean_gate_passes() {
        assert_eq!(
            gate_answer("freeze", "commit", RunStatus::Done, None),
            Gate::Passed
        );
    }

    #[test]
    fn an_unreviewed_gate_vetoes_with_the_review_text() {
        assert_eq!(
            unreviewed("freeze"),
            "freeze is not reviewed: review it, then try again."
        );
    }

    #[test]
    fn an_observer_failure_is_a_warning_and_a_clean_run_is_silent() {
        assert_eq!(
            observer_failure("notify", "commit", RunStatus::Failed, Some("boom")).as_deref(),
            Some("notify failed after the commit: boom")
        );
        assert_eq!(
            observer_failure("notify", "commit", RunStatus::Done, None),
            None
        );
    }

    #[test]
    fn a_plan_moves_only_the_branches_that_are_not_blocked() {
        let planned =
            |name: &str, block: Option<RestackBlock>, new_tip: Option<&str>| PlannedBranch {
                name: name.into(),
                parent: "main".into(),
                old_tip: "old".into(),
                old_base: None,
                onto: "main".into(),
                commits: Vec::new(),
                new_tip: new_tip.map(str::to_owned),
                block,
            };
        let plan = RestackPlan {
            plan_id: "p".into(),
            from: "main".into(),
            branches: vec![
                planned("a", None, Some("new-a")),
                planned("b", Some(RestackBlock::Skipped), None),
                planned("c", None, Some("new-c")),
            ],
            stopped: Vec::new(),
        };
        assert_eq!(moved_by_plan(&plan), ["a", "c"]);
        let record = RestackRecord {
            plan_id: "p".into(),
            from: "main".into(),
            moves: vec![RestackMove {
                name: "a".into(),
                parent: "main".into(),
                old_tip: "old".into(),
                new_tip: "new".into(),
                old_base: None,
                new_base: "main".into(),
            }],
            phase: RestackPhase::Applied,
        };
        assert_eq!(moved_by_record(&record), ["a"]);
    }
}
