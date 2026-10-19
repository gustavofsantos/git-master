//! Reads a workflow file into the typed model (docs/10, D61, D78). Only the
//! shape is checked here: TOML syntax, known keys, types, one kind per state.
//! Graph and reference rules live in `validate`.

use super::Diagnostic;
use super::model::Definition;

/// Parses one file. The error points at the line of the first problem.
pub fn parse(_source: &str) -> Result<Definition, Diagnostic> {
    todo!("WF0: parse")
}
