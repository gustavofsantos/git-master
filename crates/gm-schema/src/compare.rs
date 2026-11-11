//! The changes between two revisions (D231): the Compare and Branch tabs of the
//! Changes page read them. Read with `git`, never logged (D28).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::history::CommitFile;

/// What a range compares (D231 (3)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RangeMode {
    /// The tree at `from` against the tree at `to`.
    #[default]
    Direct,
    /// The merge base of `from` and `to` against `to`: what `to` brings over `from`.
    SinceCommonAncestor,
}

/// The changes between two revisions, as the Compare tab shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RevisionRange {
    /// Full id of the `from` revision as it resolved when read.
    pub from_oid: String,
    /// Full id of the `to` revision as it resolved when read.
    pub to_oid: String,
    /// The abbreviated `from_oid`.
    pub from_short: String,
    /// The abbreviated `to_oid`.
    pub to_short: String,
    /// The full id the diff starts from: `from_oid` for `Direct`, the merge base for
    /// `SinceCommonAncestor`.
    pub base: String,
    /// The full id the diff ends at: `to_oid`.
    pub tip: String,
    /// The changed files in Git's order, at most `DETAIL_FILE_LIMIT`.
    pub files: Vec<CommitFile>,
    /// Changed files past the limit. They are counted, not listed.
    pub more_files: usize,
    /// Lines added over all changed files; zero for a binary file.
    pub added: u32,
    /// Lines removed over all changed files; zero for a binary file.
    pub deleted: u32,
}
