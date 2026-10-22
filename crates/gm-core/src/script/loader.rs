//! Finds and loads the workflow scripts of the user (design/workflow-scripts.md, D61, D73, D98):
//! `*.lua` files in the user's workflows directory and in the directories of
//! `gitmaster.workflowPath`. Nothing is read from a repository's working tree. `M.meta` is read
//! in the sandbox without `ctx`, so a script is listed without running any of its logic.

use std::path::{Path, PathBuf};

use super::meta::Meta;
use super::port::{ScriptDiagnostic, ScriptEngine};

/// Files larger than this are refused.
pub const MAX_FILE: u64 = 256 * 1024;

/// One script file and what loading it gave.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptEntry {
    pub path: PathBuf,
    /// The file name, such as `branch-from-ticket.lua`.
    pub file_name: String,
    /// The workflow's `name`, or the file stem when the file does not load.
    pub name: String,
    /// sha256 of the file's bytes, lowercase hex: the key of reviews (D101).
    pub hash: String,
    /// Empty for a file that was refused before it was read (too large, not UTF-8).
    pub source: String,
    /// The checked `M.meta`, or every problem found, each with the line it points at.
    pub meta: Result<Meta, Vec<ScriptDiagnostic>>,
}

/// A directory or file that was not read as a workflow, and why. The page shows these as warnings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ignored {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    pub entries: Vec<ScriptEntry>,
    pub ignored: Vec<Ignored>,
}

/// `$XDG_CONFIG_HOME/gitmaster/workflows` (else `~/.config/...`) on Linux,
/// `~/Library/Application Support/gitmaster/workflows` on macOS. A relative `XDG_CONFIG_HOME` is
/// ignored; no home, no directory.
pub fn user_dir(
    _xdg_config_home: Option<std::ffi::OsString>,
    _home: Option<std::ffi::OsString>,
    _macos: bool,
) -> Option<PathBuf> {
    todo!()
}

/// `user_dir` with this process's environment.
pub fn default_user_dir() -> Option<PathBuf> {
    todo!()
}

/// Loads every `*.lua` directly inside each directory, directories in the given order and files
/// by name. See the module doc and the tests for the rules.
pub fn load_dirs(
    _engine: &dyn ScriptEngine,
    _dirs: &[PathBuf],
    _repo_root: Option<&Path>,
) -> Loaded {
    todo!()
}

/// Loads one file by path, with the same checks, for `check`, `dry-run` and `test`.
pub fn load_file(_engine: &dyn ScriptEngine, _path: &Path) -> ScriptEntry {
    todo!()
}

/// sha256 of `bytes`, lowercase hex.
#[must_use]
pub fn sha256_hex(_bytes: &[u8]) -> String {
    todo!()
}
