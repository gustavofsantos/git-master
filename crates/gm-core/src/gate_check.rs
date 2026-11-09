//! The gate check of a push and a restack (D212 (2), (3), (5), D214, D215), without the engine, so
//! the native check and the workflow calls `ctx.push.lease` and `ctx.restack.apply` judge a
//! branch the same way. It reads the gate files of the repository and the user, the reviewers,
//! the verdicts, the findings and the approval of each branch, then calls the pure
//! `gates::evaluate`. It runs no gate and writes nothing.
//!
//! A workflow can never bypass a gate: `BypassGates` is a human command and no `ctx` call names
//! one, so the workflow calls run this check and stop at a block.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gm_schema::{Approval, GateOp, GateOutcome, GatesStatus, RestackPlan};

use crate::GitPort;
use crate::change::{self, Change};
use crate::gates::{self, FindingFact, GateFacts, LoadedGates};
use crate::review_session::ReviewSession;
use crate::review_store::ReviewStore;
use crate::reviewers;
use crate::script::phases::moved_by_plan;
use crate::verdicts;
use crate::workspace_defs::{self, WorkspaceDefs};

/// The most verdicts one gate check reads for a branch (D212 (5)). A reviewer keeps five, so
/// this holds two hundred reviewers; a branch past it reads as missing verdicts, which blocks.
const GATE_VERDICT_LIMIT: usize = 1000;

/// The review stores one engine keeps open at once (D214 (5)): the open repository and the
/// members a workspace window judges, each by its root. The oldest one is dropped past this.
pub(crate) const REVIEW_SESSION_LIMIT: usize = 4;

/// The most distinct commits one gate check tests for ancestry (R2h). A finding whose commit is
/// past the limit counts, which blocks: the check fails closed.
const FINDING_HEAD_LIMIT: usize = 200;

/// The id a gate file problem has in a bypass (D212 (4)).
pub(crate) const GATES_FILE_ID: &str = "gates-file";

/// The directories the gate check reads the user's definitions from (D212 (1), (2)). The engine
/// holds the default ones; tests point them at temporary folders.
#[derive(Clone, Debug, Default)]
pub(crate) struct GateConfig {
    /// The directory of the user's `gates.json`.
    pub(crate) gates_dir: Option<PathBuf>,
    /// The directory of the user's reviewers (`reviewers/`).
    pub(crate) reviewers_dir: Option<PathBuf>,
    /// The workspace layer a check of one member reads, named by the workspace window that
    /// judges it (D214 (5)). `None` finds the workspaces the checkout's clone belongs to.
    pub(crate) explicit: Option<WorkspaceDefs>,
}

/// The tip of a branch a gate check judged. A push checks that the branch is still there, at this
/// tip, before it sends anything (R2h).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Seen {
    pub(crate) branch: String,
    pub(crate) head: String,
}

/// What the gates of one branch found: the gates that block it, and the gate file problems.
#[derive(Debug, Default)]
pub(crate) struct GateBlock {
    pub(crate) gates: Vec<GateOutcome>,
    pub(crate) errors: Vec<String>,
}

/// A branch the check found blocked, with what blocks it.
#[derive(Debug)]
pub(crate) struct BranchBlock {
    pub(crate) branch: String,
    pub(crate) block: GateBlock,
}

/// What a check of one or more branches for one operation found (D212 (3)): every branch judged,
/// with the tip it was judged at, and the branches that do not pass.
#[derive(Debug, Default)]
pub(crate) struct Checked {
    pub(crate) seen: Vec<Seen>,
    pub(crate) blocks: Vec<BranchBlock>,
}

impl Checked {
    /// True when some gate of some branch blocks, or a gate file has a problem.
    pub(crate) fn blocked(&self) -> bool {
        !self.blocks.is_empty()
    }

    /// The blocking gates of every branch, in order, and the gate file problems, each once.
    pub(crate) fn outcomes(&self) -> (Vec<GateOutcome>, Vec<String>) {
        let gates = self
            .blocks
            .iter()
            .flat_map(|blocked| blocked.block.gates.iter().cloned())
            .collect();
        (gates, self.errors())
    }

    /// The gate file problems of every branch, each once.
    pub(crate) fn errors(&self) -> Vec<String> {
        let mut errors: Vec<String> = Vec::new();
        for error in self
            .blocks
            .iter()
            .flat_map(|blocked| blocked.block.errors.iter())
        {
            if !errors.contains(error) {
                errors.push(error.clone());
            }
        }
        errors
    }

    /// The ids a bypass names for this block (D212 (4)): each blocking gate once, then
    /// `gates-file` when a gate file has a problem.
    pub(crate) fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for gate in self
            .blocks
            .iter()
            .flat_map(|blocked| blocked.block.gates.iter())
        {
            if !ids.contains(&gate.id) {
                ids.push(gate.id.clone());
            }
        }
        if !self.errors().is_empty() {
            ids.push(GATES_FILE_ID.to_owned());
        }
        ids
    }

    /// The veto text: one part per blocking gate, then one per gate file problem (D212 (3)). When
    /// more than one branch was judged, each gate part starts with its branch.
    pub(crate) fn text(&self) -> String {
        let named = self.seen.len() > 1;
        let mut parts = Vec::new();
        for blocked in &self.blocks {
            for gate in &blocked.block.gates {
                let reason = gate.reason.as_deref().unwrap_or("the gate does not pass");
                let part = format!("Blocked by gate {}: {reason}", gate.id);
                parts.push(if named {
                    format!("{}: {part}", blocked.branch)
                } else {
                    part
                });
            }
        }
        parts.extend(
            self.errors()
                .into_iter()
                .map(|error| format!("Gate file problem: {error}")),
        );
        parts.join("; ")
    }
}

/// What the gates of a branch need from the repository (D215, D214 (1)): the tip, the change,
/// whether a person approved the tip, and the intent. It gathers and never runs a gate.
pub struct GateInputs {
    /// The tip of the branch: the commit the gates judge.
    pub head: String,
    /// The change of the branch, `None` when it has no base (D214 (1)).
    pub change: Option<Change>,
    /// True when a person approved `head` and the approval is not stale (D215 (2)).
    pub approved: bool,
    /// The intent of the branch, its description; text from the repository, so untrusted.
    pub intent: Option<String>,
}

/// What `gates_report` says about a branch (D212 (5), D215): the gate check, the intent (text
/// from the repository, so untrusted) and the approval, with whether it is stale.
pub struct GatesReport {
    pub status: GatesStatus,
    pub intent: Option<String>,
    pub approval: Option<Approval>,
}

/// The gate files of the workspaces, the repository and the user, as read (D212 (2), D214 (5)).
/// Nothing else is read.
pub(crate) fn load(
    git: &dyn GitPort,
    root: &Path,
    config: &GateConfig,
) -> Result<LoadedGates, String> {
    let common = git.common_dir(root).map_err(|e| e.0)?;
    let layers = workspace_defs::layers_for(git, &common, config.explicit.as_ref());
    Ok(gates::load_layers(
        Some(&common.join("gitmaster")),
        config.gates_dir.as_deref(),
        &layers,
    ))
}

/// True when there is no gate and no gate file problem: nothing is checked, and a user without
/// gates pays nothing (D212 (3)).
pub(crate) fn is_idle(loaded: &LoadedGates) -> bool {
    loaded.gates.is_empty() && loaded.errors.is_empty()
}

/// The text of a native or workflow check that could not run: it fails closed (D212 (3)).
pub(crate) fn unchecked(reason: &str) -> String {
    format!("gates could not be checked: {reason}")
}

/// The store of the checkout at `root`: opened on first use and kept, so a window that alternates
/// between the workspace and a member does not reopen it each time. Each root has its own store,
/// which is the one of its clone (D214 (5)). Past `REVIEW_SESSION_LIMIT` the oldest is dropped.
pub(crate) fn review_store<'a>(
    slot: &'a mut Vec<ReviewSession>,
    git: &dyn GitPort,
    root: &Path,
) -> Result<&'a mut ReviewStore, String> {
    if let Some(index) = slot.iter().position(|session| session.root == root) {
        return Ok(&mut slot[index].store);
    }
    if slot.len() >= REVIEW_SESSION_LIMIT {
        slot.remove(0);
    }
    slot.push(ReviewSession::open(git, root)?);
    let last = slot.len() - 1;
    Ok(&mut slot[last].store)
}

/// Whether `approval` of a branch is stale against `tip` now (D215 (2)); the store never sets it.
pub(crate) fn approval_now(
    store: &ReviewStore,
    branch: &str,
    tip: Option<&str>,
) -> Result<Option<Approval>, String> {
    let approval = store.approval(branch).map_err(|e| e.0)?;
    Ok(approval.map(|mut approval| {
        approval.stale = verdicts::is_stale(&approval.head, tip);
        approval
    }))
}

/// What a branch's gates need, gathered from Git and the store (D215, D214 (1)).
pub(crate) fn facts_inputs(
    git: &dyn GitPort,
    store: &ReviewStore,
    root: &Path,
    branch: &str,
) -> Result<GateInputs, String> {
    let head = git
        .branch_tip(root, branch)
        .map_err(|e| e.0)?
        .ok_or_else(|| format!("there is no branch named {branch}"))?;
    let change = change::change_of(git, root, branch).map_err(|e| e.0)?;
    let intent = git.branch_description(root, branch).map_err(|e| e.0)?;
    let approved = store
        .approval(branch)
        .map_err(|e| e.0)?
        .is_some_and(|approval| !verdicts::is_stale(&approval.head, Some(&head)));
    Ok(GateInputs {
        head,
        change,
        approved,
        intent,
    })
}

/// How many commits `branch` is behind its parent in the stack model (D214 (1)). `None` when it
/// has no parent, the parent is not a local branch any more, or the stack cannot be read.
fn behind_parent(git: &dyn GitPort, root: &Path, branch: &str) -> Option<u32> {
    let link = git
        .stack_links(root)
        .ok()?
        .into_iter()
        .find(|link| link.branch == branch && link.parent != branch)?;
    git.stack_edge(root, branch, &link.parent, link.base.as_deref())
        .ok()
        .map(|edge| edge.behind)
}

/// The findings that count for `branch` at its tip `tip` (R2h, D212 (6), D214 (1)): a finding
/// counts when its branch is `branch`, or when the commit it was made on is an ancestor of the tip,
/// so a rename of the branch does not let it escape. Each distinct commit is tested once. A test
/// that fails, and a commit past `FINDING_HEAD_LIMIT`, count the finding: the check fails closed.
fn counted_findings(
    git: &dyn GitPort,
    store: &ReviewStore,
    root: &Path,
    branch: &str,
    tip: &str,
) -> Result<Vec<FindingFact>, String> {
    let rows = store.branch_findings_with_heads(branch).map_err(|e| e.0)?;
    let mut ancestors: BTreeMap<String, bool> = BTreeMap::new();
    let mut facts = Vec::new();
    for (_, severity, status, finding_branch, finding_head) in rows {
        let counts = if finding_branch.as_deref() == Some(branch) {
            true
        } else {
            match finding_head {
                Some(head) => match ancestors.get(&head) {
                    Some(known) => *known,
                    None if ancestors.len() >= FINDING_HEAD_LIMIT => true,
                    None => {
                        let answer = git.is_ancestor(root, &head, tip).unwrap_or(true);
                        ancestors.insert(head, answer);
                        answer
                    }
                },
                None => false,
            }
        };
        if counts {
            facts.push(FindingFact { severity, status });
        }
    }
    Ok(facts)
}

/// The gate report of `branch` for `op` (every gate when `None`) (D212 (5), D214, D215). It reads
/// the reviewers of the repository and the user, the verdicts and findings of the branch, the
/// approval and the stack parent's distance, then calls the pure `gates::evaluate`. A file that
/// cannot be read is an error of the report, and a report with any error is never ready (fail
/// closed). It blocks nothing by itself.
pub(crate) fn report(
    git: &dyn GitPort,
    store: &ReviewStore,
    root: &Path,
    branch: &str,
    op: Option<GateOp>,
    config: &GateConfig,
    loaded: &LoadedGates,
) -> Result<GatesReport, String> {
    let common = git.common_dir(root).map_err(|e| e.0)?;
    let inputs = facts_inputs(git, store, root, branch)?;
    let layers = workspace_defs::layers_for(git, &common, config.explicit.as_ref());
    let defined = reviewers::load_layers(
        Some(&reviewers::repo_dir(&common)),
        config.reviewers_dir.as_deref(),
        &layers,
    );
    // Every verdict the store keeps for the branch: `evaluate` picks the newest one for each
    // reviewer whose head is the tip, so an older verdict of an earlier commit still counts
    // after a reset back to it. Retention keeps five per reviewer, so the limit is a bound.
    let verdicts = store
        .list_verdicts(Some(branch), None, GATE_VERDICT_LIMIT)
        .map_err(|e| e.0)?;
    let findings = counted_findings(git, store, root, branch, &inputs.head)?;
    let approval = approval_now(store, branch, Some(&inputs.head))?;
    let preset_names: Vec<String> = reviewers::presets()
        .into_iter()
        .map(|preset| preset.name)
        .collect();
    let facts = GateFacts {
        branch,
        head: &inputs.head,
        changed_paths: inputs
            .change
            .as_ref()
            .and_then(|change| change.paths.as_deref()),
        reviewers: &defined.reviewers,
        preset_names: &preset_names,
        verdicts: &verdicts,
        findings: &findings,
        behind_parent: behind_parent(git, root, branch),
        approved: inputs.approved,
        member: None,
    };
    let mut status = gates::evaluate(&loaded.gates, op, &facts);
    let mut errors: Vec<String> = defined
        .errors
        .into_iter()
        .map(|(file, reason)| format!("{file}: {reason}"))
        .collect();
    errors.extend(loaded.errors.iter().cloned());
    status.ready = status.ready && errors.is_empty();
    status.errors = errors;
    Ok(GatesReport {
        status,
        intent: inputs.intent,
        approval,
    })
}

/// The block of a branch's report: the gates that apply and do not pass, and the file problems.
fn block_of(status: GatesStatus) -> GateBlock {
    GateBlock {
        gates: status
            .gates
            .into_iter()
            .filter(|gate| gate.applicable && !gate.passed)
            .collect(),
        errors: status.errors,
    }
}

/// Checks `branches` for `op` against the gates in `loaded` (D212 (3)): each branch's report, the
/// tip it was judged at, and the branches that do not pass. The review store opens on first use,
/// through `slot`.
pub(crate) fn check(
    git: &dyn GitPort,
    slot: &mut Vec<ReviewSession>,
    root: &Path,
    loaded: &LoadedGates,
    op: GateOp,
    branches: &[String],
    config: &GateConfig,
) -> Result<Checked, String> {
    let store = review_store(slot, git, root)?;
    let mut checked = Checked::default();
    for branch in branches {
        let report = report(git, store, root, branch, Some(op), config, loaded)?;
        checked.seen.push(Seen {
            branch: branch.clone(),
            head: report.status.head.clone(),
        });
        if !report.status.ready {
            checked.blocks.push(BranchBlock {
                branch: branch.clone(),
                block: block_of(report.status),
            });
        }
    }
    Ok(checked)
}

/// The refusal of a workflow call that pushes or restacks (D212 (3), R2h): `Err` with the veto
/// text when a gate blocks any of `branches`, or with the `gates could not be checked` text when
/// the check cannot run. Nothing is checked when no gate exists.
pub(crate) fn refuse(
    git: &dyn GitPort,
    root: &Path,
    branches: &[String],
    op: GateOp,
    config: &GateConfig,
) -> Result<(), String> {
    let loaded = load(git, root, config).map_err(|reason| unchecked(&reason))?;
    if is_idle(&loaded) {
        return Ok(());
    }
    let mut slot = Vec::new();
    let checked = check(git, &mut slot, root, &loaded, op, branches, config)
        .map_err(|reason| unchecked(&reason))?;
    if checked.blocked() {
        Err(checked.text())
    } else {
        Ok(())
    }
}

/// The branches a restack check judges: its root, then every branch the plan moves (D212 (2)).
pub(crate) fn restack_branches(root: &str, plan: &RestackPlan) -> Vec<String> {
    let mut branches = vec![root.to_owned()];
    for name in moved_by_plan(plan) {
        if !branches.contains(&name) {
            branches.push(name);
        }
    }
    branches
}

/// Why a push may not go on, when its branch moved since the gates judged it (R2h): HEAD left the
/// branch, or the branch's tip is no longer the one judged. `None` when the push may go on. Pure.
pub(crate) fn moved_veto(seen: &Seen, head: Option<&str>, tip: Option<&str>) -> Option<String> {
    if head != Some(seen.branch.as_str()) {
        return Some(format!(
            "HEAD moved off {} while the gates ran; run it again",
            seen.branch
        ));
    }
    if tip == Some(seen.head.as_str()) {
        return None;
    }
    let now = tip.map_or_else(|| "nothing".to_owned(), |tip| short(tip).to_owned());
    Some(format!(
        "the branch moved while the gates ran: {} is now {now}; run it again",
        short(&seen.head)
    ))
}

/// The first seven characters of a commit id, as a message shows it.
fn short(oid: &str) -> &str {
    oid.get(..7).unwrap_or(oid)
}

#[cfg(test)]
mod tests {
    use gm_schema::{GateLayer, PlannedBranch, RestackBlock};

    use super::*;

    fn outcome(id: &str, reason: &str) -> GateOutcome {
        GateOutcome {
            id: id.to_owned(),
            kind: "human_approval".to_owned(),
            layer: GateLayer::Repository,
            applicable: true,
            not_applicable: None,
            passed: false,
            reason: Some(reason.to_owned()),
            settings: serde_json::Value::Null,
        }
    }

    fn blocked(branch: &str, gates: Vec<GateOutcome>, errors: Vec<&str>) -> BranchBlock {
        BranchBlock {
            branch: branch.to_owned(),
            block: GateBlock {
                gates,
                errors: errors.into_iter().map(str::to_owned).collect(),
            },
        }
    }

    fn seen(branches: &[&str]) -> Vec<Seen> {
        branches
            .iter()
            .map(|branch| Seen {
                branch: (*branch).to_owned(),
                head: "abcdef0123".to_owned(),
            })
            .collect()
    }

    #[test]
    fn one_branch_is_named_by_no_prefix() {
        let checked = Checked {
            seen: seen(&["feat"]),
            blocks: vec![blocked(
                "feat",
                vec![outcome("approved", "no approval")],
                vec![],
            )],
        };
        assert_eq!(checked.text(), "Blocked by gate approved: no approval");
    }

    #[test]
    fn several_branches_prefix_each_gate_and_the_file_problem_comes_once() {
        let checked = Checked {
            seen: seen(&["a", "b", "c"]),
            blocks: vec![
                blocked("a", vec![], vec!["gates.json: bad"]),
                blocked(
                    "b",
                    vec![outcome("approved", "no approval")],
                    vec!["gates.json: bad"],
                ),
            ],
        };
        assert_eq!(
            checked.text(),
            "b: Blocked by gate approved: no approval; Gate file problem: gates.json: bad"
        );
        assert_eq!(checked.ids(), ["approved", GATES_FILE_ID]);
    }

    #[test]
    fn a_moved_tip_or_a_moved_head_vetoes_a_push() {
        let judged = Seen {
            branch: "feat".to_owned(),
            head: "abcdef0123".to_owned(),
        };
        assert_eq!(moved_veto(&judged, Some("feat"), Some("abcdef0123")), None);
        assert_eq!(
            moved_veto(&judged, Some("feat"), Some("9999999aaa")).as_deref(),
            Some("the branch moved while the gates ran: abcdef0 is now 9999999; run it again")
        );
        assert_eq!(
            moved_veto(&judged, Some("other"), Some("abcdef0123")).as_deref(),
            Some("HEAD moved off feat while the gates ran; run it again")
        );
    }

    fn planned(name: &str, moved: bool) -> PlannedBranch {
        PlannedBranch {
            name: name.to_owned(),
            parent: "main".to_owned(),
            old_tip: "old".to_owned(),
            old_base: None,
            onto: "main".to_owned(),
            commits: Vec::new(),
            new_tip: moved.then(|| "new".to_owned()),
            block: (!moved).then_some(RestackBlock::Refused {
                reason: "dirty".to_owned(),
            }),
        }
    }

    #[test]
    fn a_restack_checks_its_root_and_each_branch_it_moves_once() {
        let plan = RestackPlan {
            plan_id: "p".to_owned(),
            from: "a".to_owned(),
            branches: vec![planned("a", true), planned("b", true), planned("c", false)],
            stopped: Vec::new(),
        };
        assert_eq!(restack_branches("a", &plan), ["a", "b"]);
        assert_eq!(restack_branches("x", &plan), ["x", "a", "b"]);
    }
}
