//! The stack calls of a workflow run (`WF4a`, D68, D142, D154): `ctx.stack.*`, `ctx.restack.*`,
//! `ctx.reparent.*`, `ctx.fetch` and `ctx.push.lease`, carried out by the engine over the same
//! operations as the window. A plan is remembered by its id for the open repository, so an apply
//! moves exactly the plan the script was shown: a plan whose id changed, or that was never made in
//! this session, ends `stale`. Every Git write goes through `Actions`, so it is logged under the run
//! (D81). Planning and reading write nothing (D28).

use std::collections::BTreeMap;
use std::path::Path;

use gm_schema::{CommandKind, RestackBlock, RestackPlan, StackSnapshot, StackState};
use serde_json::{Value, json};

use crate::GitPort;
use crate::branch;
use crate::port::ScriptRemote;
use crate::restack_session;
use crate::script::run::EffectResult;
use crate::stack_session;
use crate::workflow_session::Actions;

/// What a plan id was made for, so an apply can check it is the kind of plan it applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Planned {
    /// `ctx.restack.plan({ root })`.
    Restack { root: String },
    /// `ctx.reparent.plan({ branch, parent })`.
    Reparent { branch: String, parent: String },
}

/// The plans of the open repository by id. It lives as long as the script session does.
#[derive(Debug, Default)]
pub(crate) struct PlanBook {
    plans: BTreeMap<String, Planned>,
}

impl PlanBook {
    fn remember(&mut self, plan_id: &str, planned: Planned) {
        self.plans.insert(plan_id.to_owned(), planned);
    }

    fn get(&self, plan_id: &str) -> Option<&Planned> {
        self.plans.get(plan_id)
    }
}

/// Where an apply of `plan_id` goes, given what the book knows of it. Pure.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// A restack of this root.
    Restack(String),
    /// A reparent of this branch onto this parent.
    Reparent { branch: String, parent: String },
    /// The plan is not the id the apply was given, or the book does not know it.
    Unknown,
    /// The id belongs to the other kind of plan.
    OtherKind(&'static str),
}

/// Routes an apply of a restack (`want_restack`) or of a reparent. Pure.
pub(crate) fn route(entry: Option<&Planned>, want_restack: bool) -> Route {
    match (entry, want_restack) {
        (None, _) => Route::Unknown,
        (Some(Planned::Restack { root }), true) => Route::Restack(root.clone()),
        (Some(Planned::Reparent { branch, parent }), false) => Route::Reparent {
            branch: branch.clone(),
            parent: parent.clone(),
        },
        (Some(Planned::Reparent { .. }), true) => Route::OtherKind("ctx.reparent.apply"),
        (Some(Planned::Restack { .. }), false) => Route::OtherKind("ctx.restack.apply"),
    }
}

/// Why a remote name is refused: an empty one, or one that starts with `-` and could be read as an
/// option of Git. Pure.
#[must_use]
pub(crate) fn remote_problem(remote: &str) -> Option<&'static str> {
    if remote.is_empty() {
        Some("A remote name cannot be empty.")
    } else if remote.starts_with('-') {
        Some("A remote name cannot start with '-'.")
    } else {
        None
    }
}

/// `ctx.stack.tree()`: every branch of every stack, with its parent and whether it is stale.
#[must_use]
pub(crate) fn tree_value(snapshot: &StackSnapshot) -> Value {
    let branches: Vec<Value> = snapshot
        .stacks
        .iter()
        .flat_map(|stack| &stack.branches)
        .map(|entry| {
            json!({
                "name": entry.name,
                "parent": entry.parent,
                "stale": entry.state == StackState::Stale,
            })
        })
        .collect();
    json!({ "branches": branches })
}

/// `ctx.stack.stale()`: the names of the branches whose parent moved.
#[must_use]
pub(crate) fn stale_value(snapshot: &StackSnapshot) -> Value {
    let names: Vec<&str> = snapshot
        .stacks
        .iter()
        .flat_map(|stack| &stack.branches)
        .filter(|entry| entry.state == StackState::Stale)
        .map(|entry| entry.name.as_str())
        .collect();
    json!({ "branches": names })
}

/// The branches a plan moves: unblocked, with a new tip. Pure.
#[must_use]
pub(crate) fn branch_count(plan: &RestackPlan) -> usize {
    plan.branches
        .iter()
        .filter(|entry| entry.block.is_none() && entry.new_tip.is_some())
        .count()
}

/// The answer of a plan call: its id and how many branches it moves.
#[must_use]
pub(crate) fn plan_value(plan: &RestackPlan) -> Value {
    json!({ "plan_id": plan.plan_id, "branch_count": branch_count(plan) })
}

/// What blocks a plan, as text for the script: the conflicts and the refusals. Pure.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Blocks {
    pub(crate) conflicts: Vec<String>,
    pub(crate) refusals: Vec<String>,
}

#[must_use]
pub(crate) fn blocks(plan: &RestackPlan) -> Blocks {
    let mut found = Blocks::default();
    for entry in &plan.branches {
        match &entry.block {
            Some(RestackBlock::Conflict { commit, .. }) => {
                let short = commit.get(..7).unwrap_or(commit);
                found.conflicts.push(format!(
                    "{} conflicts at {short}; it and the branches under it wait.",
                    entry.name
                ));
            }
            Some(RestackBlock::Refused { reason }) => found.refusals.push(reason.clone()),
            _ => {}
        }
    }
    found
}

/// The same-folder test of the window, for a plan: a branch checked out in the open folder is
/// refused when that folder is dirty, as the restack dialog does.
fn same_folder(root: &Path) -> impl Fn(&str) -> bool + '_ {
    move |path| crate::engine::same_folder(root, path)
}

fn ended(outcome: &str, message: impl Into<String>) -> EffectResult {
    EffectResult {
        outcome: outcome.to_owned(),
        value: Value::Null,
        message: message.into(),
    }
}

fn done() -> EffectResult {
    EffectResult::done(Value::Null)
}

/// `ctx.stack.tree()` and `ctx.stack.stale()` read the snapshot; nothing is logged.
pub(crate) fn tree(git: &dyn GitPort, root: &Path) -> Result<Value, String> {
    Ok(tree_value(&stack_session::snapshot(git, root)?))
}

pub(crate) fn stale(git: &dyn GitPort, root: &Path) -> Result<Value, String> {
    Ok(stale_value(&stack_session::snapshot(git, root)?))
}

/// `ctx.restack.plan({ root })`: plans the restack of `root` and its subtree and remembers it.
pub(crate) fn plan_restack(
    git: &dyn GitPort,
    root: &Path,
    book: &mut PlanBook,
    branch: &str,
) -> Result<Value, String> {
    let plan = restack_session::plan_with(git, root, branch, &[], None, &same_folder(root))?;
    book.remember(
        &plan.plan_id,
        Planned::Restack {
            root: branch.to_owned(),
        },
    );
    Ok(plan_value(&plan))
}

/// `ctx.reparent.plan({ branch, parent })`: plans moving `branch` and its subtree onto `parent`
/// and remembers it. Writes no config.
pub(crate) fn plan_reparent(
    git: &dyn GitPort,
    root: &Path,
    book: &mut PlanBook,
    branch: &str,
    parent: &str,
) -> Result<Value, String> {
    let plan = restack_session::plan_with(
        git,
        root,
        branch,
        &[],
        Some((branch, parent)),
        &same_folder(root),
    )?;
    book.remember(
        &plan.plan_id,
        Planned::Reparent {
            branch: branch.to_owned(),
            parent: parent.to_owned(),
        },
    );
    Ok(plan_value(&plan))
}

/// `restack.preview({ plan_id })`: the plan an apply of `plan_id` would move, made again for its
/// root now, so the approval of the apply can show it (`WF4c`). It reads only: nothing is logged
/// and nothing moves. An id the run did not make ends `failed`.
pub(crate) fn preview(
    git: &dyn GitPort,
    root: &Path,
    book: &PlanBook,
    plan_id: &str,
) -> EffectResult {
    let made = match book.get(plan_id) {
        None => return ended("failed", "The plan is not known to this run."),
        Some(Planned::Restack { root: from }) => {
            restack_session::plan_with(git, root, from, &[], None, &same_folder(root))
        }
        Some(Planned::Reparent { branch, parent }) => restack_session::plan_with(
            git,
            root,
            branch,
            &[],
            Some((branch.as_str(), parent.as_str())),
            &same_folder(root),
        ),
    };
    match made {
        Ok(plan) => EffectResult::done(json!(plan)),
        Err(message) => EffectResult::failed(message),
    }
}

/// `ctx.restack.apply({ plan_id })`. The plan is made again for its root and must have the same
/// id, else `stale`. Its unblocked branches move in one transaction; a conflict ends `conflict`
/// after they moved; a plan that moves nothing and was refused ends `failed` with the reasons.
///
pub(crate) fn apply_restack(
    actions: &mut Actions<'_>,
    book: &PlanBook,
    plan_id: &str,
) -> EffectResult {
    let from = match route(book.get(plan_id), true) {
        Route::Restack(root) => root,
        Route::OtherKind(call) => {
            return ended(
                "failed",
                format!("{plan_id} is a reparent plan; apply it with {call}."),
            );
        }
        Route::Unknown | Route::Reparent { .. } => {
            return ended("stale", "The plan is not known to this run. Plan again.");
        }
    };
    let (git, root) = (actions.git(), actions.root());
    let plan = match restack_session::plan_with(git, root, &from, &[], None, &same_folder(root)) {
        Ok(plan) => plan,
        Err(message) => return ended("failed", message),
    };
    if plan.plan_id != plan_id {
        return ended(
            "stale",
            "The stacks changed since the plan was made. Plan again.",
        );
    }
    if let Some(reason) = plan.stopped.first() {
        return ended("failed", reason.clone());
    }
    let found = blocks(&plan);
    if branch_count(&plan) > 0 {
        let kind = CommandKind::ApplyRestack {
            branch: from.clone(),
            plan_id: plan_id.to_owned(),
            skip: Vec::new(),
        };
        match actions.record(kind, restack_session::apply_plan(git, root, &plan)) {
            Ok((_, remaps)) => {
                actions.moved_by_restack();
                actions.remap(remaps);
            }
            Err(message) => return ended("failed", message),
        }
    }
    if !found.conflicts.is_empty() {
        // The plan after the moves, with the blocked branch, for the window (WF4c). The plan
        // made before them stands in when the stacks cannot be read again.
        let after = restack_session::plan_with(git, root, &from, &[], None, &same_folder(root))
            .unwrap_or_else(|_| plan.clone());
        actions.stopped_on_conflict(after);
        return ended("conflict", found.conflicts.join(" "));
    }
    if branch_count(&plan) == 0 && !found.refusals.is_empty() {
        return ended("failed", found.refusals.join(" "));
    }
    done()
}

/// `ctx.reparent.apply({ plan_id })`. The plan is made again and must have the same id, else
/// `failed` ("stale plan"). A reparent moves all of its branches or none: a blocked branch fails
/// it, since the outcomes of the call have no `conflict`. The stack parent is recorded with the
/// refs, so undo puts the old parent back.
pub(crate) fn apply_reparent(
    actions: &mut Actions<'_>,
    book: &PlanBook,
    plan_id: &str,
) -> EffectResult {
    let (branch, parent) = match route(book.get(plan_id), false) {
        Route::Reparent { branch, parent } => (branch, parent),
        Route::OtherKind(call) => {
            return ended(
                "failed",
                format!("{plan_id} is a restack plan; apply it with {call}."),
            );
        }
        Route::Unknown | Route::Restack(_) => {
            return ended(
                "failed",
                "stale plan: the plan is not known to this run. Plan again.",
            );
        }
    };
    let (git, root) = (actions.git(), actions.root());
    let plan = match restack_session::plan_with(
        git,
        root,
        &branch,
        &[],
        Some((&branch, &parent)),
        &same_folder(root),
    ) {
        Ok(plan) => plan,
        Err(message) => return ended("failed", message),
    };
    if plan.plan_id != plan_id {
        return ended(
            "failed",
            "stale plan: the stacks changed since the plan was made. Plan again.",
        );
    }
    if let Some(reason) = plan.stopped.first() {
        return ended("failed", reason.clone());
    }
    let found = blocks(&plan);
    if !found.conflicts.is_empty() || !found.refusals.is_empty() {
        let reasons: Vec<String> = found.conflicts.into_iter().chain(found.refusals).collect();
        return ended("failed", reasons.join(" "));
    }
    if branch_count(&plan) > 0 {
        let kind = CommandKind::ApplyReparent {
            branch,
            parent,
            plan_id: plan_id.to_owned(),
        };
        match actions.record(kind, restack_session::apply_plan(git, root, &plan)) {
            Ok((_, remaps)) => {
                actions.moved_by_restack();
                actions.remap(remaps);
            }
            Err(message) => return ended("failed", message),
        }
    }
    done()
}

/// `ctx.restack.undo({ plan_id })`: puts back the last restack, when its record has that id.
pub(crate) fn undo(actions: &mut Actions<'_>, plan_id: &str) -> EffectResult {
    let (git, root) = (actions.git(), actions.root());
    let last = match restack_session::last(git, root) {
        Ok(last) => last,
        Err(message) => return ended("failed", message),
    };
    match last {
        None => ended("failed", "There is no restack to undo."),
        Some(record) if record.plan_id != plan_id => ended(
            "failed",
            format!(
                "The last restack is {}, not {plan_id}; only the last restack can be undone.",
                record.plan_id
            ),
        ),
        Some(_) => match actions.record(CommandKind::UndoRestack, restack_session::undo(git, root))
        {
            Ok(()) => {
                actions.moved_by_restack();
                done()
            }
            Err(message) => ended("failed", message),
        },
    }
}

/// How a call that runs Git starts: its answer at once when it is refused, or the Git command to
/// run off the engine thread, with the call the run waits for (`WF4a`).
pub(crate) enum Started {
    Ended(EffectResult),
    Waiting { remote: ScriptRemote },
}

/// `ctx.fetch({ remote })`: `git fetch <remote> --prune`, off the engine thread.
pub(crate) fn fetch(git: &dyn GitPort, root: &Path, remote: &str) -> Started {
    if let Some(problem) = remote_problem(remote) {
        return Started::Ended(ended("failed", problem));
    }
    match git.fetch_remote(root, remote) {
        Ok(remote) => Started::Waiting { remote },
        Err(e) => Started::Ended(ended("failed", e.0)),
    }
}

/// `ctx.push.lease({ branch, remote })`: `git push --force-with-lease`, off the engine thread. The
/// approval is asked by the run before the call gets here.
pub(crate) fn push_lease(git: &dyn GitPort, root: &Path, branch: &str, remote: &str) -> Started {
    if let Some(problem) = remote_problem(remote) {
        return Started::Ended(ended("failed", problem));
    }
    if let Some(problem) = branch::name_problem(branch) {
        return Started::Ended(ended("failed", problem));
    }
    match git.push_branch_with_lease(root, branch, remote) {
        Ok(remote) => Started::Waiting { remote },
        Err(e) => Started::Ended(ended("failed", e.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{PlannedBranch, PlannedCommit, ReplayStep, Stack, StackBranch};

    fn book_with(id: &str, planned: Planned) -> PlanBook {
        let mut book = PlanBook::default();
        book.remember(id, planned);
        book
    }

    fn stack_branch(name: &str, parent: Option<&str>, state: StackState) -> StackBranch {
        StackBranch {
            name: name.into(),
            parent: parent.map(Into::into),
            depth: 0,
            state,
            ahead: 0,
            behind: 0,
            restack: 0,
            commits: Vec::new(),
            base: None,
            parent_tip: None,
        }
    }

    fn planned(name: &str, new_tip: Option<&str>, block: Option<RestackBlock>) -> PlannedBranch {
        PlannedBranch {
            name: name.into(),
            parent: "main".into(),
            old_tip: "old".into(),
            old_base: None,
            onto: "onto".into(),
            commits: vec![PlannedCommit {
                oid: "abcdef0123".into(),
                subject: "s".into(),
                step: ReplayStep::Clean { new: "n".into() },
            }],
            new_tip: new_tip.map(Into::into),
            block,
        }
    }

    #[test]
    fn a_remote_name_that_starts_with_a_dash_is_refused() {
        assert!(remote_problem("-x").is_some());
        assert!(remote_problem("--upload-pack=evil").is_some());
        assert!(remote_problem("").is_some());
        assert_eq!(remote_problem("origin"), None);
        assert_eq!(remote_problem("upstream-2"), None);
    }

    #[test]
    fn an_unknown_plan_id_routes_nowhere_so_the_apply_ends_stale() {
        assert_eq!(route(None, true), Route::Unknown);
        assert_eq!(route(None, false), Route::Unknown);
    }

    #[test]
    fn an_apply_routes_only_the_kind_of_plan_it_applies() {
        let restack = Planned::Restack { root: "a".into() };
        assert_eq!(route(Some(&restack), true), Route::Restack("a".into()));
        assert_eq!(
            route(Some(&restack), false),
            Route::OtherKind("ctx.restack.apply")
        );
        let reparent = Planned::Reparent {
            branch: "b".into(),
            parent: "main".into(),
        };
        assert_eq!(
            route(Some(&reparent), false),
            Route::Reparent {
                branch: "b".into(),
                parent: "main".into()
            }
        );
        assert_eq!(
            route(Some(&reparent), true),
            Route::OtherKind("ctx.reparent.apply")
        );
    }

    #[test]
    fn the_book_knows_only_the_plans_it_was_given() {
        let book = book_with("abc", Planned::Restack { root: "a".into() });
        assert!(book.get("abc").is_some());
        assert!(book.get("unknown").is_none());
    }

    #[test]
    fn the_tree_gives_each_branch_its_parent_and_stale_flag() {
        let snapshot = StackSnapshot {
            stacks: vec![Stack {
                branches: vec![
                    stack_branch("main", None, StackState::Root),
                    stack_branch("a", Some("main"), StackState::Stale),
                    stack_branch("b", Some("a"), StackState::Current),
                ],
            }],
            looped: Vec::new(),
        };
        let tree = tree_value(&snapshot);
        assert_eq!(
            tree,
            json!({"branches": [
                {"name": "main", "parent": null, "stale": false},
                {"name": "a", "parent": "main", "stale": true},
                {"name": "b", "parent": "a", "stale": false},
            ]})
        );
        assert_eq!(stale_value(&snapshot), json!({"branches": ["a"]}));
    }

    #[test]
    fn branch_count_is_the_unblocked_branches_with_a_new_tip() {
        let plan = RestackPlan {
            plan_id: "p".into(),
            from: "a".into(),
            branches: vec![
                planned("a", Some("new-a"), None),
                planned("b", Some("new-b"), None),
                planned("c", None, Some(RestackBlock::Skipped)),
                planned("d", None, None),
            ],
            stopped: Vec::new(),
        };
        assert_eq!(branch_count(&plan), 2);
        assert_eq!(
            plan_value(&plan),
            json!({"plan_id": "p", "branch_count": 2})
        );
    }

    #[test]
    fn blocks_name_the_conflict_by_its_short_commit_and_keep_the_refusals() {
        let plan = RestackPlan {
            plan_id: "p".into(),
            from: "a".into(),
            branches: vec![
                planned(
                    "a",
                    None,
                    Some(RestackBlock::Conflict {
                        commit: "abcdef0123".into(),
                        onto: "o".into(),
                    }),
                ),
                planned(
                    "c",
                    None,
                    Some(RestackBlock::Refused {
                        reason: "checked out elsewhere".into(),
                    }),
                ),
            ],
            stopped: Vec::new(),
        };
        let found = blocks(&plan);
        assert_eq!(
            found.conflicts,
            ["a conflicts at abcdef0; it and the branches under it wait."]
        );
        assert_eq!(found.refusals, ["checked out elsewhere"]);
    }
}
