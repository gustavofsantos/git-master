//! Proposed reviewers and gates (D214 (3)). An agent proposes a definition; it is not a reviewer
//! or a gate until a person accepts it, so a proposal is never read by the gate check or listed
//! as a reviewer. The text in a proposal is written by an agent: untrusted.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::gates::GateDef;
use crate::verdict::{Reviewer, ReviewerDirection, ReviewerSignal, ReviewerSubject};

/// A reviewer as proposed: the fields of a reviewer file (D212 (1)) without its source, which the
/// layer it is accepted into decides.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewerProposal {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub signal: ReviewerSignal,
    pub direction: ReviewerDirection,
    pub subject: ReviewerSubject,
    /// An optional model hint for the run.
    #[serde(default)]
    pub model: Option<String>,
    /// Globs the change must touch for the reviewer to apply (D214 (1)).
    #[serde(default)]
    pub paths: Vec<String>,
    /// Globs the branch must match for the reviewer to apply (D214 (1)).
    #[serde(default)]
    pub branches: Vec<String>,
    /// The workspace members the reviewer applies to (D214 (5)). Only a workspace definition
    /// may name members, so a proposal with any is accepted into the workspace layer alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
}

/// What a proposal defines: a reviewer or a gate (D214 (3)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    Reviewer,
    Gate,
}

/// Where a proposal stands. Only `Proposed` is open; `Denied` and `Accepted` are kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Proposed,
    Denied,
    Accepted,
}

/// A proposed reviewer or gate, as the person sees it (D214 (3)). `reviewer` is set for a
/// reviewer proposal and `gate` for a gate proposal. `updates` is true when an accepted definition
/// with the same name or id exists in the user or repository layer, and `current_reviewer` or
/// `current_gate` is that definition, so the window can show before and after.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Proposal {
    pub id: i64,
    pub kind: ProposalKind,
    pub name: String,
    pub reviewer: Option<ReviewerProposal>,
    pub gate: Option<GateDef>,
    /// Why the agent proposes it, untrusted text.
    pub reason: String,
    /// The author of the proposal: `agent:<client>`.
    pub origin: String,
    pub status: ProposalStatus,
    /// Milliseconds since the Unix epoch, when it was proposed.
    pub at: i64,
    /// When the person accepted or denied it; `None` while it is proposed.
    pub decided_at: Option<i64>,
    pub updates: bool,
    pub current_reviewer: Option<Reviewer>,
    pub current_gate: Option<GateDef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proposal_kind_and_status_are_written_in_snake_case() {
        assert_eq!(
            serde_json::to_value(ProposalKind::Reviewer).unwrap(),
            serde_json::json!("reviewer")
        );
        assert_eq!(
            serde_json::to_value(ProposalStatus::Denied).unwrap(),
            serde_json::json!("denied")
        );
        let back: ProposalStatus = serde_json::from_str("\"accepted\"").unwrap();
        assert_eq!(back, ProposalStatus::Accepted);
    }

    #[test]
    fn a_reviewer_proposal_reads_without_its_optional_lists() {
        let json = serde_json::json!({
            "name": "billing",
            "description": "Knows the billing rules.",
            "instructions": "Check the invoice rounding.",
            "signal": "level",
            "direction": "lower_is_better",
            "subject": "branch"
        });
        let proposal: ReviewerProposal = serde_json::from_value(json).unwrap();
        assert_eq!(proposal.model, None);
        assert!(proposal.paths.is_empty());
        assert!(proposal.branches.is_empty());
        assert!(proposal.members.is_empty());
    }

    #[test]
    fn members_are_written_only_when_a_proposal_names_some() {
        let mut proposal: ReviewerProposal = serde_json::from_value(serde_json::json!({
            "name": "billing",
            "description": "Knows the billing rules.",
            "instructions": "Check the invoice rounding.",
            "signal": "level",
            "direction": "lower_is_better",
            "subject": "branch"
        }))
        .unwrap();
        let bare = serde_json::to_value(&proposal).unwrap();
        assert!(bare.get("members").is_none(), "{bare}");
        proposal.members = vec!["api".into()];
        let named = serde_json::to_value(&proposal).unwrap();
        assert_eq!(named["members"], serde_json::json!(["api"]));
        let back: ReviewerProposal = serde_json::from_value(named).unwrap();
        assert_eq!(back, proposal);
    }
}
