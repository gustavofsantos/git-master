use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The most matching lines a text search lists. It stops there (N3).
pub const GREP_TOTAL_LIMIT: usize = 1000;

/// The most matching lines of one file a text search lists. The rest of the
/// file's matches are counted in `GrepFile::more`.
pub const GREP_FILE_LIMIT: usize = 100;

/// The most bytes of a matching line a result shows, around the match.
pub const GREP_LINE_LIMIT: usize = 300;

/// How a text search reads its pattern. The default is a fixed string, any case,
/// in the tracked files and the new ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GrepOptions {
    /// Letters must match in case (`git grep` without `-i`).
    pub case_sensitive: bool,
    /// The match must be a whole word (`-w`).
    pub whole_word: bool,
    /// The pattern is an extended regular expression (`-E`), not a fixed string (`-F`).
    pub regex: bool,
    /// Which files are searched.
    pub scope: GrepScope,
}

impl Default for GrepOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            whole_word: false,
            regex: false,
            scope: GrepScope::TrackedAndNew,
        }
    }
}

/// Which files of the worktree a text search reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrepScope {
    /// The tracked files and the new ones `.gitignore` does not hide
    /// (`--untracked`). Git has to walk the whole worktree to find the new
    /// ones, which costs about as much as `git status`: well under a second in a
    /// repository of 100 thousand files, and the reason to use `TrackedOnly` in a
    /// larger one.
    TrackedAndNew,
    /// The tracked files only. Needs no walk of the worktree.
    TrackedOnly,
}

/// One matching line, shaped for a result row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GrepLine {
    /// 1-based line number in the file.
    pub line: u32,
    /// The line without its indentation, cut around the match when it is
    /// longer than `GREP_LINE_LIMIT` bytes.
    pub text: String,
    /// Byte ranges (start, end) of `text` to highlight.
    pub ranges: Vec<(usize, usize)>,
    /// `text` does not start at the start of the line's content.
    pub cut_start: bool,
    /// `text` does not reach the end of the line.
    pub cut_end: bool,
}

/// The matching lines of one file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GrepFile {
    /// Path from the root of the worktree.
    pub path: String,
    /// At most `GREP_FILE_LIMIT` lines, in line order.
    pub lines: Vec<GrepLine>,
    /// Matching lines past the limit, counted and not listed.
    pub more: usize,
}

/// The answer to `SearchText`: files by path, lines by line number, whatever
/// order Git printed them in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GrepResult {
    pub query: String,
    pub files: Vec<GrepFile>,
    /// Matching lines listed, over all files.
    pub listed: usize,
    /// The search stopped at `GREP_TOTAL_LIMIT` lines, so there are more.
    pub truncated: bool,
}
