//! The graph and reference rules of a parsed workflow (docs/10, D63, D78).
//! Errors keep the workflow from running; warnings are shown on the page.

use super::Diagnostic;
use super::model::Definition;

/// Every problem in the definition, errors and warnings, ordered by line.
pub fn validate(_definition: &Definition) -> Vec<Diagnostic> {
    todo!("WF0: validate")
}
