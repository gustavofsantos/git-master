//! The `exec` action (docs/10, D74): runs a command line directly, never
//! through a shell. Each argument is one argv element, so repository content
//! interpolated into it cannot inject flags or commands.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::Value;

/// Used when `with.timeout` is not set.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Stdout beyond this is cut and `truncated` is set in the output.
pub const STDOUT_LIMIT: usize = 1024 * 1024;
/// Stderr kept for the error message.
pub const STDERR_LIMIT: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecSpec {
    pub argv: Vec<String>,
    pub stdin: Option<String>,
    pub timeout: Duration,
    /// Where the process runs: the repository root.
    pub cwd: PathBuf,
    /// `format = "json"`: stdout must be JSON, exposed as `output.json`.
    pub json: bool,
}

/// Reads the rendered `with` of an `exec` state: `argv` (non-empty list of
/// strings), `stdin` (string), `timeout` (`"<n>s|m|h"`), `format` (`"json"`).
/// A leading `~/` in `argv[0]` is the home directory.
pub fn spec(_input: &Value, _cwd: &Path) -> Result<ExecSpec, String> {
    todo!("WF1: exec::spec")
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExecResult {
    /// `done`, `failed` or `timeout` (catalog outcomes of `exec`).
    pub outcome: &'static str,
    /// `{ "text", "exit_code", "stderr", "truncated" }`, plus `"json"` in
    /// JSON format, plus `"error"` when the process could not start or was
    /// cancelled.
    pub output: Value,
}

/// Runs the process in its own process group, feeds `stdin`, reads stdout
/// and stderr, and kills the group on timeout or when `cancel` is set.
pub fn run(_spec: &ExecSpec, _cancel: &AtomicBool) -> ExecResult {
    todo!("WF1: exec::run")
}
