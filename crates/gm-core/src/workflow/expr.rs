//! The expression language of workflows (docs/10, D62, D78): `${path}`
//! references inside templates, and conditions with comparisons,
//! `and`/`or`/`not`, parentheses and `len()`. Pure; no I/O, no variables.

use std::collections::BTreeMap;

use serde_json::Value;

/// What expressions can read while a run is evaluated.
#[derive(Clone, Copy, Debug)]
pub struct Scope<'a> {
    pub inputs: &'a Value,
    pub trigger: &'a Value,
    pub repo: &'a Value,
    /// Output of each state that has run, by state id (`states.<id>.output`).
    pub states: &'a BTreeMap<String, Value>,
    /// The current `for_each` item: its name and value.
    pub item: Option<(&'a str, &'a Value)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExprError(pub String);

impl std::fmt::Display for ExprError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExprError {}

/// Every `${...}` path in a template or a condition, split on dots, in
/// order of appearance: `${states.plan.output.plan_id}` gives
/// `["states", "plan", "output", "plan_id"]`.
pub fn references(_source: &str) -> Result<Vec<Vec<String>>, ExprError> {
    todo!("WF0: expr::references")
}

/// Renders one template string. A string that is exactly one `${path}`
/// yields the referenced value with its type; any other string yields a
/// string with each reference written in.
pub fn render(_template: &str, _scope: &Scope<'_>) -> Result<Value, ExprError> {
    todo!("WF0: expr::render")
}

/// Renders every string inside a `with` value, keeping its shape.
pub fn render_value(_with: &Value, _scope: &Scope<'_>) -> Result<Value, ExprError> {
    todo!("WF0: expr::render_value")
}

/// Checks the syntax of a condition without evaluating it.
pub fn check_condition(_source: &str) -> Result<(), ExprError> {
    todo!("WF0: expr::check_condition")
}

/// Evaluates a condition to a boolean.
pub fn eval_condition(_source: &str, _scope: &Scope<'_>) -> Result<bool, ExprError> {
    todo!("WF0: expr::eval_condition")
}
