use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One stash of `git stash list` (B2, D94). `index` is `n` of `stash@{n}`, so 0
/// is the newest. `oid` is the full id of the stash commit: the actions name
/// both, and the engine refuses one whose index no longer holds that commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StashEntry {
    pub index: u32,
    /// Full id of the stash commit.
    pub oid: String,
    /// The reflog subject after `stash@{n}: `, as `git stash list` shows it:
    /// `On <branch>: <message>` for a message given with `-m`, else
    /// `WIP on <branch>: <short id> <subject>`.
    pub message: String,
    /// The branch the stash was made on, read from `message`; `None` for a
    /// detached HEAD or when the subject has another shape.
    pub branch: Option<String>,
    /// Committer time of the stash commit, in Unix seconds.
    pub time: i64,
}
