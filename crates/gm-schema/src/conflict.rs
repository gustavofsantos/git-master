use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The Git operation that stopped on conflicts (D114). `Unknown` when none of
/// the state files of a known operation is present, as after a stash apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConflictOperation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Unknown,
}

/// One side of a conflict, named by its role (D22) rather than ours or theirs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConflictSide {
    /// The branch name, or the short id when no branch names the commit. It is
    /// the label the conflict markers carry.
    pub branch: String,
    /// What the side is in this operation, such as `current branch`,
    /// `merged branch`, `new parent`, `commit being replayed`, `picked commit`,
    /// `reverted commit` or `other side`.
    pub role: String,
    /// The subject line of the side's commit.
    pub subject: String,
}

/// A conflicted file read for the resolver (D114). `text` is the file merged from
/// the three stages, in zdiff3 form with its markers. It is not the working tree
/// file: edits already made there are not in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConflictFileRead {
    pub path: String,
    pub operation: ConflictOperation,
    /// The current side: stage 2.
    pub left: ConflictSide,
    /// The other side: stage 3.
    pub right: ConflictSide,
    pub text: String,
    /// Whether the common ancestor (stage 1) exists. An add/add conflict has none.
    pub has_base: bool,
}
