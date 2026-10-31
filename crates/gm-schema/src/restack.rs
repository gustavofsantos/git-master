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
    Conflict { commit: String },
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
    /// Commits that are replayed (not dropped).
    #[must_use]
    pub fn replayed(&self) -> usize {
        self.commits
            .iter()
            .filter(|commit| matches!(commit.step, ReplayStep::Clean { .. }))
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
