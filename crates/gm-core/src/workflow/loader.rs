//! Finds and loads the workflow files of the user (docs/10, D61, D73): the
//! user's workflows directory and the directories in `gitmaster.workflowPath`.
//! Nothing is ever read from a repository's working tree.

use std::path::{Path, PathBuf};

use super::Diagnostic;
use super::model::Definition;

/// Files larger than this are refused.
pub const MAX_FILE: u64 = 256 * 1024;

/// One file and what loading it gave.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    /// The workflow's `name`, or the file stem when the file does not parse.
    pub name: String,
    /// sha256 of the file's bytes, lowercase hex: the key of approvals.
    pub hash: String,
    pub source: String,
    pub result: Result<(Definition, Vec<Diagnostic>), Vec<Diagnostic>>,
}

/// A directory that was not read, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ignored {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    pub entries: Vec<Entry>,
    pub ignored: Vec<Ignored>,
}

/// `$XDG_CONFIG_HOME/gitmaster/workflows` (else `~/.config/...`) on Linux,
/// `~/Library/Application Support/gitmaster/workflows` on macOS. A relative
/// `XDG_CONFIG_HOME` is ignored; no home, no directory.
pub fn user_dir(
    _xdg_config_home: Option<std::ffi::OsString>,
    _home: Option<std::ffi::OsString>,
    _macos: bool,
) -> Option<PathBuf> {
    todo!("WF1: loader::user_dir")
}

/// `user_dir` with this process's environment.
pub fn default_user_dir() -> Option<PathBuf> {
    todo!("WF1: loader::default_user_dir")
}

/// Loads every `*.toml` directly inside each directory, directories in the
/// given order and files by name. A directory inside `repo_root` is ignored
/// (D73); a missing directory is skipped without a note.
pub fn load_dirs(_dirs: &[PathBuf], _repo_root: Option<&Path>) -> Loaded {
    todo!("WF1: loader::load_dirs")
}
