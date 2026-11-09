//! Gates (D189 (5), D212 (2), D214, D215 (2)): the types a gate file and a gate check share. A
//! gate is a deterministic rule over recorded verdicts and Git facts; the engine evaluates it
//! and nothing here runs a model or reads a file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::review::ReviewSeverity;
use crate::verdict::ReviewerSubject;

/// The operations a gate can apply to (D212 (3)): a push, a restack, or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateOp {
    Push,
    Restack,
}

/// The conditions under which a gate or a reviewer applies (D214 (1)). An empty list means
/// every branch, or every change, for that field.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct When {
    /// Globs the branch name must match, such as `feat/billing-*`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<String>,
    /// Globs one of the change's paths must match, such as `services/billing/**`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

impl When {
    /// True when the condition applies to every change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.branches.is_empty() && self.paths.is_empty()
    }
}

/// What a gate checks (D189 (5), D215 (2)). The `kind` tag is written flat in the gate object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GateKind {
    /// Each named reviewer has a verdict for the current head.
    FreshVerdict { reviewers: Vec<String> },
    /// No unresolved finding at or above the severity.
    Findings { severity: ReviewSeverity },
    /// The reviewer's latest value is within the bound. A score takes a whole number from 0 to
    /// 100, a level one of `low`, `medium` or `high`; the reviewer's own signal decides which.
    Signal {
        reviewer: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<serde_json::Value>,
    },
    /// The branch is not behind its parent in the stack model.
    UpToDate,
    /// A person approved the tip of the branch (D215 (2)).
    HumanApproval,
}

/// One gate of a gate file (D212 (2), D214 (2)).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GateDef {
    /// The name, of the same form as a reviewer name. A gate with the same id in a more
    /// specific layer replaces this one whole (D214 (2)).
    pub id: String,
    #[serde(flatten)]
    pub kind: GateKind,
    /// The operations the gate checks.
    pub on: Vec<GateOp>,
    #[serde(default, skip_serializing_if = "When::is_empty")]
    pub when: When,
}

/// Where a gate definition was read from (D214 (2)). The more specific layer wins by id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateLayer {
    User,
    Repository,
    Workspace,
}

/// A gate with the layer it was read from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LayeredGate {
    pub gate: GateDef,
    pub layer: GateLayer,
}

/// What one gate said for the branch (D189 (5)).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GateOutcome {
    pub id: String,
    /// The kind's tag: `fresh_verdict`, `findings`, `signal`, `up_to_date` or `human_approval`.
    pub kind: String,
    pub layer: GateLayer,
    /// False when the gate's conditions do not match the change (D214 (1)).
    pub applicable: bool,
    /// Why the gate does not apply, when it does not.
    pub not_applicable: Option<String>,
    /// True when the gate does not block. A gate that does not apply always passes.
    pub passed: bool,
    /// Why the gate blocks, when it does.
    pub reason: Option<String>,
    /// The gate's own configuration: the fields of its kind, `on` and `when`, as the gate file
    /// writes them (D212 (5)). It holds reviewer names and patterns, so it is untrusted text.
    pub settings: serde_json::Value,
}

/// Why a reviewer has no verdict for the current head.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MissingReason {
    /// The reviewer has never recorded a verdict for this branch.
    NoVerdict,
    /// The reviewer has verdicts, but none for the current head.
    Stale,
}

/// A reviewer a gate needs and that has no verdict for the current head (D212 (5)). The skill
/// dispatches one sub-agent per entry, so it holds no logic of its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MissingReviewer {
    pub reviewer: String,
    pub subject: ReviewerSubject,
    pub model: Option<String>,
    pub reason: MissingReason,
}

/// The gate check for one operation on one branch (D189 (5), D212 (5)).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatesStatus {
    pub branch: String,
    pub head: String,
    /// True when every gate that applies to the operation passes.
    pub ready: bool,
    /// The gates the operation considers, sorted by id.
    pub gates: Vec<GateOutcome>,
    /// The reviewers a gate needs that have no verdict for the head, sorted by name.
    pub missing: Vec<MissingReviewer>,
    /// The gate files that could not be read; a gate check that has any fails closed.
    pub errors: Vec<String>,
}

/// One bypass of a gate check, as the audit keeps it (D212 (4), R2h): a row written before the
/// bypassed command runs, so a bypass is still known after the app closes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Bypass {
    pub id: i64,
    pub branch: String,
    /// The tip of the branch when the bypass ran.
    pub head: String,
    pub operation: GateOp,
    /// The ids of the gates the person named.
    pub gates: Vec<String>,
    /// The reason, trimmed.
    pub reason: String,
    /// Always `human`: an agent cannot bypass a gate.
    pub actor: String,
    /// Milliseconds since the Unix epoch.
    pub at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gate_kind_is_written_flat_with_its_tag() {
        let gate = GateDef {
            id: "no-high".to_owned(),
            kind: GateKind::Findings {
                severity: ReviewSeverity::High,
            },
            on: vec![GateOp::Push],
            when: When::default(),
        };
        let json = serde_json::to_value(&gate).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"id": "no-high", "kind": "findings", "severity": "high", "on": ["push"]})
        );
        let back: GateDef = serde_json::from_value(json).unwrap();
        assert_eq!(back, gate);
    }

    #[test]
    fn an_empty_when_is_not_written() {
        let when = When {
            branches: vec!["feat/*".to_owned()],
            paths: Vec::new(),
        };
        let json = serde_json::to_value(&when).unwrap();
        assert_eq!(json, serde_json::json!({"branches": ["feat/*"]}));
        assert!(When::default().is_empty());
        assert!(!when.is_empty());
    }

    #[test]
    fn a_reviewer_without_conditions_reads_with_none() {
        let json = serde_json::json!({
            "name": "security",
            "description": "d",
            "instructions": "i",
            "signal": "level",
            "direction": "lower_is_better",
            "subject": "branch",
            "model": null,
            "source": "preset"
        });
        let reviewer: crate::Reviewer = serde_json::from_value(json).unwrap();
        assert!(reviewer.paths.is_empty());
        assert!(reviewer.branches.is_empty());
    }
}
