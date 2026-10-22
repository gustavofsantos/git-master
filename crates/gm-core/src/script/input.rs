//! The `input.repo` value of a run (docs/reference/workflow-api.md, D105): a read-only snapshot of
//! the repository when the run starts. The run, the dry run and the tests build it the same way.

use std::path::Path;

use serde_json::Value;

use crate::GitPort;

/// `staged_diff` is the diff text up to this many bytes. A longer diff is cut at a line break
/// and ends with `[diff truncated at 1 MiB]`.
pub const STAGED_DIFF_INLINE: usize = 1024 * 1024;

/// The snapshot, and the temporary file that holds the whole diff when it was cut. Dropping this
/// deletes the file, so keep it until the run ends.
pub struct RepoInput {
    /// `{ root, branch, head, staged_diff }`, plus `staged_diff_path` (the whole diff, in a
    /// temporary file) when `staged_diff` was cut. A piece that cannot be read is `null`.
    pub value: Value,
    pub diff_file: Option<tempfile::NamedTempFile>,
}

/// Reads the snapshot of the repository at `root` through `git`.
#[must_use]
pub fn repo_input(_git: &dyn GitPort, _root: &Path) -> RepoInput {
    todo!()
}

/// `value` for a directory that is not a repository: every piece `null`.
#[must_use]
pub fn no_repo() -> Value {
    todo!()
}
