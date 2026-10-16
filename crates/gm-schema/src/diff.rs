use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Which comparison a diff or a staging action is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiffSide {
    /// Index against HEAD.
    Staged,
    /// Working tree against index. Untracked files diff against nothing.
    Unstaged,
}

/// Deterministic id of a hunk: a hash of its path and content, so a hunk that
/// changed gets a new id and a stale id fails instead of applying in the wrong
/// place (D21).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct HunkId(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number on the old side; `None` for added lines.
    pub old_no: Option<u32>,
    /// Line number on the new side; `None` for removed lines.
    pub new_no: Option<u32>,
    /// The line without its line terminator.
    pub text: String,
    /// The line is the last of its file and has no terminator
    /// (`\ No newline at end of file`).
    pub no_newline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hunk {
    pub id: HunkId,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// The text after the second `@@` (git's function context), possibly empty.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

/// What a stage or unstage action covers. `Lines` holds indices into
/// `Hunk::lines`; only added and removed lines may be selected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StageTarget {
    File {
        path: String,
    },
    Hunk {
        path: String,
        hunk: HunkId,
    },
    Lines {
        path: String,
        hunk: HunkId,
        lines: Vec<u32>,
    },
}
