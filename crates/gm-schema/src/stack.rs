//! Branch stacks (D03, D13, D138): each local branch has at most one parent,
//! recorded in `.git/config`, so the branches form trees. `StackSnapshot` is the
//! answer to `LoadStacks`: every tree, walked parents first, with the state each
//! branch derives from its recorded base.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a branch with a parent stands against it (docs/guide/stacks.md, Derived
/// states). When several apply, the first in this order is given: parent gone,
/// parent merged, diverged base, stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StackState {
    /// The root of a stack: it has children and no recorded parent.
    Root,
    /// The parent's tip is the recorded base.
    Current,
    /// The parent's tip moved away from the recorded base: a restack is due.
    Stale,
    /// The recorded base no longer marks where the branch's own commits start.
    DivergedBase,
    /// The recorded parent is not a local branch any more.
    ParentGone,
    /// The parent's changes are already in the root of the stack.
    ParentMerged,
}

impl StackState {
    /// Whether the branch is offered Reparent to Grandparent.
    #[must_use]
    pub fn parent_left(self) -> bool {
        matches!(self, Self::ParentGone | Self::ParentMerged)
    }
}

/// One branch of a stack, in the order of a walk that visits each parent before
/// its children.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StackBranch {
    pub name: String,
    /// The recorded parent; `None` for the root.
    pub parent: Option<String>,
    /// 0 for the root, 1 for its children, and so on.
    pub depth: u32,
    pub state: StackState,
    /// Commits the branch has that its parent does not (`↑N`). 0 for the root
    /// and when the parent is gone.
    pub ahead: u32,
    /// Commits the parent has that the branch does not.
    pub behind: u32,
    /// The branches a restack from this branch would replay: its stale children
    /// and every branch under them (`restack N`).
    pub restack: u32,
    /// The branch's own commits, `stackParentBase..tip`, newest first, at most
    /// `OWN_COMMITS_LIMIT` (D139). Empty for a root and when the parent is gone.
    #[serde(default)]
    pub commits: Vec<String>,
    /// The recorded base (`stackParentBase`); `None` for a root, when the parent is gone and
    /// when the key is missing. The `stack.parent_moved` payload carries it (`WF3f`).
    #[serde(default)]
    pub base: Option<String>,
    /// The full id of the parent's tip; `None` for a root and when the parent is gone. The
    /// `stack.parent_moved` payload carries it (`WF3f`).
    #[serde(default)]
    pub parent_tip: Option<String>,
}

/// The most commits `StackBranch::commits` lists for one branch.
pub const OWN_COMMITS_LIMIT: usize = 1_000;

/// One tree of branches: its root first, then the rest parents first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Stack {
    pub branches: Vec<StackBranch>,
}

impl Stack {
    /// The name of the branch at the root.
    #[must_use]
    pub fn root(&self) -> &str {
        self.branches
            .first()
            .map_or("", |branch| branch.name.as_str())
    }
}

/// Every stack of the repository, by root name, and the branches whose recorded
/// parents form a loop (they belong to no tree until one parent is changed).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StackSnapshot {
    pub stacks: Vec<Stack>,
    pub looped: Vec<String>,
}

impl StackSnapshot {
    /// The stack entry of `name`, if it is in a stack.
    #[must_use]
    pub fn branch(&self, name: &str) -> Option<&StackBranch> {
        self.stacks
            .iter()
            .flat_map(|stack| &stack.branches)
            .find(|branch| branch.name == name)
    }
}
