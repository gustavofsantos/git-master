use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How many files a search lists (N1).
pub const FILE_HIT_LIMIT: usize = 50;

/// The most bytes of a file the viewer reads; a larger file is shown cut (N2).
pub const FILE_VIEW_LIMIT: usize = 2 * 1024 * 1024;

/// One file a search found: its path and the parts of it the query matched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileHit {
    /// Path from the root of the worktree, with `/` separators.
    pub path: String,
    /// Byte ranges (start, end) of `path` to highlight, in order. Empty for an
    /// empty query.
    pub ranges: Vec<(usize, usize)>,
}

/// The answer to `SearchFiles`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileSearch {
    /// The query as typed, `:line` suffix included.
    pub query: String,
    /// The best matches, best first, at most `FILE_HIT_LIMIT`. The same query
    /// over the same files always gives the same list.
    pub hits: Vec<FileHit>,
    /// How many files matched, listed or not.
    pub matched: usize,
    /// How many files the list holds.
    pub files: usize,
    /// The list is being (re)read: the answer may miss files, and
    /// `FilesIndexed` follows when it is complete.
    pub indexing: bool,
    /// The `:line` the query ended with (`foo.rs:120`), if any.
    pub line: Option<u32>,
}

/// One entry of a folder in the file tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DirEntry {
    /// The name inside the folder. A folder whose only content is another
    /// folder is shown joined (`a/b/c`), as hosting services do.
    pub name: String,
    pub is_dir: bool,
}

/// The answer to `ListDirectory`: folders first, then files, each by name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DirListing {
    /// The folder listed, `""` for the root of the worktree.
    pub dir: String,
    pub entries: Vec<DirEntry>,
    /// How many files the list holds.
    pub files: usize,
    /// The list is being (re)read; see `FileSearch::indexing`.
    pub indexing: bool,
}

/// What a file holds, as the viewer shows it (N2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileBody {
    /// The text, lossy where it is not UTF-8. Cut at a line end when the file
    /// is larger than `FILE_VIEW_LIMIT`.
    Text { text: String },
    /// A NUL byte among the first bytes: Git's own test for a binary file.
    Binary,
    /// There is no such file in the worktree now.
    Missing,
    /// A symbolic link, which is shown and never followed.
    Symlink { target: String },
    /// The path is a folder.
    Directory,
}

/// The answer to `ReadFile`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileView {
    pub path: String,
    /// The size on disk, in bytes. Zero when there is no file.
    pub size: u64,
    pub body: FileBody,
    /// The text is the first part of a larger file.
    pub truncated: bool,
}
