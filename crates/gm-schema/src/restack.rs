//! The restack plan (D14, D142): what `PlanRestack` answers. Each branch of the
//! subtree that needs it is replayed commit by commit onto its parent's planned
//! tip, with the algorithm apply will use, so the plan shows exactly which commits
//! conflict and what the new tips are.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What replaying one commit of a branch onto its new base gives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ReplayStep {
    /// Replays cleanly as `new`, a commit not yet referenced by any ref.
    Clean { new: String },
    /// Its change is already in the new base, so it is dropped.
    Dropped,
    /// It conflicts in `paths`; the branch stops here.
    Conflict { paths: Vec<String> },
    /// It conflicted when replayed onto this base, and the person resolved it: the
    /// commit replays as `new`, the resolution (D154). The walk goes on from `new`.
    Resolved { new: String },
}

/// One commit of a branch and what its replay gives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlannedCommit {
    pub oid: String,
    pub subject: String,
    pub step: ReplayStep,
}

/// Why a branch is not replayed, or is waiting for another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestackBlock {
    /// A commit of the branch conflicts: the branch and the ones under it wait.
    /// `onto` is the base the commit was replayed onto (the parent's planned tip plus
    /// the commits of the branch already replayed); a resolution is keyed on it (D154).
    Conflict { commit: String, onto: String },
    /// The person left the branch out; the branches under it wait with it.
    Skipped,
    /// The parent is blocked or refused, so the branch cannot be replayed.
    ParentBlocked { parent: String },
    /// The branch cannot be restacked now; `reason` is a sentence for the person.
    Refused { reason: String },
}

/// One branch of the plan, listed parents first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlannedBranch {
    pub name: String,
    pub parent: String,
    /// The tip the branch has now.
    pub old_tip: String,
    /// The recorded base now, to restore on undo.
    pub old_base: Option<String>,
    /// The parent's tip after its own restack (its current tip when it is not
    /// restacked): the base the branch is replayed onto and will record.
    pub onto: String,
    /// The branch's own commits, oldest first, up to the first conflict.
    pub commits: Vec<PlannedCommit>,
    /// The tip after the restack; `None` when the branch is blocked.
    pub new_tip: Option<String>,
    pub block: Option<RestackBlock>,
}

impl PlannedBranch {
    /// Commits that are replayed (not dropped): clean ones and resolved ones (D154).
    #[must_use]
    pub fn replayed(&self) -> usize {
        self.commits
            .iter()
            .filter(|commit| {
                matches!(
                    commit.step,
                    ReplayStep::Clean { .. } | ReplayStep::Resolved { .. }
                )
            })
            .count()
    }
}

/// The answer to `PlanRestack`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestackPlan {
    /// Tied to the tips, bases and new commits of the plan: it changes when any
    /// of them does (D66).
    pub plan_id: String,
    /// The branch the plan starts from.
    pub from: String,
    pub branches: Vec<PlannedBranch>,
    /// Why nothing can run at all (a dirty working copy on a branch of the plan);
    /// empty when apply may start.
    pub stopped: Vec<String>,
}

impl RestackPlan {
    /// Whether apply may start: nothing stops the plan and at least one branch is
    /// replayed.
    #[must_use]
    pub fn can_apply(&self) -> bool {
        self.stopped.is_empty()
            && self
                .branches
                .iter()
                .any(|branch| branch.block.is_none() && branch.new_tip.is_some())
    }
}

/// One branch a restack moved, with what undo needs to put it back (D142).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestackMove {
    pub name: String,
    pub parent: String,
    pub old_tip: String,
    pub new_tip: String,
    /// The recorded base before; `None` when the branch had none.
    pub old_base: Option<String>,
    /// The parent's tip the branch now sits on, recorded as its base.
    pub new_base: String,
    /// The stack parent before the move, restored by undo. `None` in records written before
    /// `reparent.apply` (`WF4a`); undo then keeps `parent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_parent: Option<String>,
}

/// A conflict of a restack being resolved (D154): what `StartRestackResolution` opened,
/// and what the resolver shows. The sides are named as in a rebase: `parent` is the new
/// parent, `commit` the commit being replayed onto `onto`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestackResolution {
    /// The branch the restack starts from (the plan's `from`).
    pub from: String,
    /// The blocked branch whose commit conflicts.
    pub conflicted: String,
    /// The parent of `conflicted`, the new parent of the replay.
    pub parent: String,
    /// The commit being replayed.
    pub commit: String,
    /// The subject line of `commit`.
    pub subject: String,
    /// The base `commit` is replayed onto.
    pub onto: String,
    /// The conflicted paths, sorted. Empty when rerere resolved every path already.
    pub paths: Vec<String>,
}

/// How far a restack got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RestackPhase {
    /// Saved before the refs move; the files and bases may still be owed.
    Applying,
    /// Refs, files and bases are all written.
    #[default]
    Applied,
}

/// A restack that ran, kept in `.git/gitmaster/restack.json` until it is undone
/// or the next one replaces it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestackRecord {
    pub plan_id: String,
    pub from: String,
    pub moves: Vec<RestackMove>,
    #[serde(default)]
    pub phase: RestackPhase,
}
