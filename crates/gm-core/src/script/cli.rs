//! `gitmaster workflow check | dry-run | test` (docs/reference/cli.md, D102): one-shot commands
//! with no resident process and no state. They never apply an effect: `check` only compiles,
//! `dry-run` records and simulates, `test` runs against a fake `ctx`.

use std::path::Path;

use super::port::ScriptEngine;

/// What a subcommand prints and how it exits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliResult {
    pub code: i32,
    pub out: String,
    pub err: String,
}

/// Runs `gitmaster workflow <args…>`; `args` are the words after `workflow`. `cwd` resolves
/// relative paths. Exit codes: 0 success, 1 the script has errors or a test differs, 2 usage.
pub fn run(_args: &[String], _engine: &dyn ScriptEngine, _cwd: &Path) -> CliResult {
    todo!()
}
