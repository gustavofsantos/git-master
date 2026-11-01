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
    /// The resolution Git recorded for this conflict (rerere), when there is one.
    /// Git has already applied it to the working tree file, so the file holds it
    /// now. `None` when nothing is recorded.
    pub recorded: Option<RecordedResolution>,
}

/// A resolution Git recorded for a conflict (rerere) and applied to the working
/// tree file when the conflict was read. `text` is that file as it is now, not
/// the zdiff3 text of `ConflictFileRead::text`, which stays the conflict so the
/// engineer can compare the two.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordedResolution {
    pub text: String,
}

/// Who resolved one hunk of a conflict (the operation log records it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HunkOrigin {
    /// Chosen by the engineer in the resolver.
    Human,
    /// Taken from a resolution Git recorded (rerere).
    Rerere,
    /// Taken from a workflow script's suggestion. No script produces one yet.
    Script,
}

/// Which worktree a conflict command reads or writes (D154). `WorkingCopy` is the open
/// checkout, the default; `Restack` is the resolution worktree of a restack conflict, the
/// only other one a command can reach.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConflictSite {
    #[default]
    WorkingCopy,
    Restack,
}
