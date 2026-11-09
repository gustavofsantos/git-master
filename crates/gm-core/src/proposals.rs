//! Proposed reviewers and gates (D214 (3)): the checks a proposal passes before it is stored,
//! the definition a stored proposal holds, and the view a person sees with what it would replace.
//! Pure: no Git, no store, no files are written here. A proposal is never a file, so nothing in
//! this module makes one a reviewer or a gate: the engine writes a definition only when a person
//! accepts it.

use gm_schema::{
    GateDef, GateLayer, LayeredGate, Proposal, ProposalKind, ProposalStatus, Reviewer,
    ReviewerProposal, ReviewerSource,
};
use serde::Serialize;

use crate::{gates, reviewers};

/// The most open (`proposed`) proposals one repository holds (D214 (3)).
pub const OPEN_LIMIT: usize = 10;
/// The proposals `ListProposals` returns when it names no limit, and the most it returns.
pub const LIST_DEFAULT: u32 = 20;
pub const LIST_MAX: u32 = 100;
/// The longest reason, in characters, once trimmed.
const REASON_LIMIT: usize = 300;

/// The definitions an accepted reviewer or gate has in the repository and the user's layers,
/// read from their files only: presets are not files, so they are not here.
pub struct Current {
    pub reviewers: Vec<Reviewer>,
    pub gates: Vec<GateDef>,
}

/// The reason of a proposal: trimmed, 1 to 300 characters, with what the redactor takes out.
/// The redacted text is checked again, since a redaction can make it longer.
pub fn check_reason(reason: &str, redact: &dyn Fn(&str) -> String) -> Result<String, String> {
    let trimmed = bounded(reason.trim(), REASON_LIMIT, "the reason")?;
    bounded(&redact(&trimmed), REASON_LIMIT, "the reason")
}

/// The text of a field, refused when it is empty or longer than `limit` characters.
fn bounded(text: &str, limit: usize, what: &str) -> Result<String, String> {
    let count = text.chars().count();
    if text.is_empty() {
        Err(format!("{what} is empty"))
    } else if count > limit {
        Err(format!(
            "{what} is {count} characters, at most {limit} are allowed"
        ))
    } else {
        Ok(text.to_owned())
    }
}

/// A copy of `reviewer` with its free text (`description` and `instructions`) redacted.
pub fn redact_reviewer(
    reviewer: &ReviewerProposal,
    redact: &dyn Fn(&str) -> String,
) -> ReviewerProposal {
    ReviewerProposal {
        description: redact(&reviewer.description),
        instructions: redact(&reviewer.instructions),
        ..reviewer.clone()
    }
}

/// The reviewer as it is written: trimmed text, an empty model is none, and each pattern is
/// trimmed, as a reviewer file reads them back.
fn normalized(reviewer: &ReviewerProposal) -> ReviewerProposal {
    ReviewerProposal {
        name: reviewer.name.clone(),
        description: reviewer.description.trim().to_owned(),
        instructions: reviewer.instructions.trim().to_owned(),
        signal: reviewer.signal,
        direction: reviewer.direction,
        subject: reviewer.subject,
        model: reviewer
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_owned),
        paths: trimmed(&reviewer.paths),
        branches: trimmed(&reviewer.branches),
    }
}

fn trimmed(patterns: &[String]) -> Vec<String> {
    patterns
        .iter()
        .map(|pattern| pattern.trim().to_owned())
        .collect()
}

/// The reviewer a proposal defines in `source`.
pub fn to_reviewer(reviewer: &ReviewerProposal, source: ReviewerSource) -> Reviewer {
    Reviewer {
        name: reviewer.name.clone(),
        description: reviewer.description.clone(),
        instructions: reviewer.instructions.clone(),
        signal: reviewer.signal,
        direction: reviewer.direction,
        subject: reviewer.subject,
        model: reviewer.model.clone(),
        source,
        paths: reviewer.paths.clone(),
        branches: reviewer.branches.clone(),
    }
}

/// Checks a reviewer proposal as a reviewer file: it is rendered and read back, and must read
/// back as the same reviewer, so the name rule, the front matter limits and the patterns are all
/// checked. Returns the reviewer as it is written.
pub fn check_reviewer(reviewer: &ReviewerProposal) -> Result<ReviewerProposal, String> {
    let normalized = normalized(reviewer);
    let written = to_reviewer(&normalized, ReviewerSource::Repository);
    let text = reviewers::render(&written);
    let back = reviewers::parse(&written.name, &text, ReviewerSource::Repository)
        .map_err(|reason| format!("not a valid reviewer: {reason}"))?;
    if back != written {
        return Err(
            "not a valid reviewer: a field would not be read back as it was written".to_owned(),
        );
    }
    Ok(normalized)
}

/// Checks a gate proposal as a gate file: a file of this one gate is read back by the gate
/// reader, which checks the id, the kind, the settings, `on` and `when`, and must give the gate
/// back unchanged.
pub fn check_gate(gate: &GateDef) -> Result<(), String> {
    let text = serde_json::json!({ "gates": [gate] }).to_string();
    let parsed = gates::parse(&text, GateLayer::Repository)
        .map_err(|errors| format!("not a valid gate: {}", errors.join("; ")))?;
    match parsed.as_slice() {
        [only] if only.gate == *gate => Ok(()),
        _ => Err("not a valid gate: a field would not be read back as it was written".to_owned()),
    }
}

/// The JSON a proposal's body holds.
pub fn encode<T: Serialize>(definition: &T) -> Result<String, String> {
    serde_json::to_string(definition).map_err(|e| format!("cannot encode the proposal: {e}"))
}

/// The reviewer a stored reviewer proposal holds.
pub fn decode_reviewer(body: &str) -> Result<ReviewerProposal, String> {
    serde_json::from_str(body).map_err(|e| format!("cannot read the stored proposal: {e}"))
}

/// The gate a stored gate proposal holds.
pub fn decode_gate(body: &str) -> Result<GateDef, String> {
    serde_json::from_str(body).map_err(|e| format!("cannot read the stored proposal: {e}"))
}

/// The gates of one layer's file, read alone. A file with any problem refuses the write that
/// would change it: the person fixes the file first (D212 (2)).
pub fn layer_gates(layer: GateLayer, dir: &std::path::Path) -> Result<Vec<GateDef>, String> {
    let loaded = match layer {
        GateLayer::Repository => gates::load(Some(dir), None),
        GateLayer::User => gates::load(None, Some(dir)),
        GateLayer::Workspace => return Err(workspace_not_yet()),
    };
    if !loaded.errors.is_empty() {
        return Err(format!(
            "fix gates.json first: {}",
            loaded.errors.join("; ")
        ));
    }
    Ok(loaded
        .gates
        .into_iter()
        .map(|layered: LayeredGate| layered.gate)
        .collect())
}

/// The refusal of a workspace layer, which is not built yet (D214 (5), R2c).
pub fn workspace_not_yet() -> String {
    "accepting into the workspace is not available yet: accept into the repository or your configuration"
        .to_owned()
}

/// The proposal as the person sees it, with the accepted definition it would replace, if any.
pub fn view(
    stored: &crate::review_store::StoredProposal,
    current: &Current,
) -> Result<Proposal, String> {
    let (reviewer, gate, current_reviewer, current_gate) = match stored.kind {
        ProposalKind::Reviewer => {
            let reviewer = decode_reviewer(&stored.body)?;
            let current = current
                .reviewers
                .iter()
                .find(|defined| defined.name == stored.name)
                .cloned();
            (Some(reviewer), None, current, None)
        }
        ProposalKind::Gate => {
            let gate = decode_gate(&stored.body)?;
            let current = current
                .gates
                .iter()
                .find(|defined| defined.id == stored.name)
                .cloned();
            (None, Some(gate), None, current)
        }
    };
    let updates = current_reviewer.is_some() || current_gate.is_some();
    Ok(Proposal {
        id: stored.id,
        kind: stored.kind,
        name: stored.name.clone(),
        reviewer,
        gate,
        reason: stored.reason.clone(),
        origin: stored.origin.clone(),
        status: stored.status,
        at: stored.at,
        decided_at: stored.decided_at,
        updates,
        current_reviewer,
        current_gate,
    })
}

/// A proposal's status as the words a refusal uses.
pub fn status_word(status: ProposalStatus) -> &'static str {
    match status {
        ProposalStatus::Proposed => "open",
        ProposalStatus::Denied => "denied",
        ProposalStatus::Accepted => "accepted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{ReviewerDirection, ReviewerSignal, ReviewerSubject};

    fn reviewer() -> ReviewerProposal {
        ReviewerProposal {
            name: "billing".to_owned(),
            description: "Knows the billing rules.".to_owned(),
            instructions: "Check the rounding of invoices.".to_owned(),
            signal: ReviewerSignal::Level,
            direction: ReviewerDirection::LowerIsBetter,
            subject: ReviewerSubject::Branch,
            model: None,
            paths: vec!["services/billing/**".to_owned()],
            branches: Vec::new(),
        }
    }

    #[test]
    fn a_valid_reviewer_is_checked_and_comes_back_trimmed() {
        let mut padded = reviewer();
        padded.description = "  Knows the billing rules.  ".to_owned();
        padded.model = Some("  ".to_owned());
        let checked = check_reviewer(&padded).expect("valid");
        assert_eq!(checked.description, "Knows the billing rules.");
        assert_eq!(checked.model, None);
    }

    #[test]
    fn a_reviewer_that_is_not_a_valid_file_is_refused() {
        let mut bad_name = reviewer();
        bad_name.name = "Billing Rules".to_owned();
        assert!(
            check_reviewer(&bad_name)
                .unwrap_err()
                .contains("not a valid reviewer")
        );
        let mut newline = reviewer();
        newline.description = "two\nlines".to_owned();
        assert!(check_reviewer(&newline).is_err());
        let mut bad_pattern = reviewer();
        bad_pattern.paths = vec!["../outside".to_owned()];
        assert!(check_reviewer(&bad_pattern).is_err());
        let mut empty = reviewer();
        empty.instructions = "   ".to_owned();
        assert!(check_reviewer(&empty).is_err());
    }

    #[test]
    fn a_reason_is_trimmed_and_bounded() {
        let keep = |text: &str| text.to_owned();
        assert_eq!(check_reason("  why  ", &keep).unwrap(), "why");
        assert!(check_reason("   ", &keep).is_err());
        assert!(check_reason(&"x".repeat(301), &keep).is_err());
        assert!(check_reason(&"x".repeat(300), &keep).is_ok());
    }

    #[test]
    fn a_gate_is_checked_as_a_gate_file_would_read_it() {
        let gate = GateDef {
            id: "billing".to_owned(),
            kind: gm_schema::GateKind::UpToDate,
            on: vec![gm_schema::GateOp::Push],
            when: gm_schema::When {
                branches: Vec::new(),
                paths: vec!["services/billing/**".to_owned()],
            },
        };
        assert!(check_gate(&gate).is_ok());
        let mut no_ops = gate.clone();
        no_ops.on = Vec::new();
        assert!(check_gate(&no_ops).is_err());
        let mut bad_id = gate;
        bad_id.id = "Bad_Id".to_owned();
        assert!(check_gate(&bad_id).is_err());
    }
}
