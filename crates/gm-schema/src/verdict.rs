//! Reviewers and verdicts (D189, D212). A reviewer is a named definition a person or a preset
//! provides; a verdict is what one run recorded against it. The app never calls a model: an
//! agent records a verdict through MCP, and the engine reads these types back.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::review::{ReviewSeverity, ReviewStatus};

/// What a reviewer's verdict measures (D189): a score from 0 to 100, a level, or a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerSignal {
    Score,
    Level,
    Pass,
}

/// Which way is good for a signal (D189). Ignored for `pass`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerDirection {
    HigherIsBetter,
    LowerIsBetter,
}

/// What a reviewer judges (D189): the branch against its parent, the staged changes, or one commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerSubject {
    Branch,
    Staged,
    Commit,
}

/// Where a reviewer definition was read from (D212 (1)). A preset is a template, never a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerSource {
    Repository,
    User,
    /// A workspace repository's reviewer (D214 (2), D174): it applies to the members it names.
    Workspace,
    Preset,
}

/// A reviewer definition (D189, D212 (1)): what to look for, how to read its answer, and on what.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Reviewer {
    /// The name, which is also the file stem: 1 to 40 characters of `a-z`, `0-9` and `-`.
    pub name: String,
    pub description: String,
    /// The body of the file: the instructions the agent follows.
    pub instructions: String,
    pub signal: ReviewerSignal,
    pub direction: ReviewerDirection,
    pub subject: ReviewerSubject,
    /// An optional model hint for the run.
    pub model: Option<String>,
    pub source: ReviewerSource,
    /// Globs the change must touch for the reviewer to apply (D214 (1)); empty means any change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// Globs the branch must match for the reviewer to apply (D214 (1)); empty means any branch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<String>,
    /// The workspace members the reviewer applies to, by folder name (D214 (5)); empty means
    /// every member.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
}

/// A level, ordered `Low` < `Medium` < `High` (D189).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Low,
    Medium,
    High,
}

/// What one run of a reviewer recorded (D189 (2)). `value` is the signal as text: an integer
/// from 0 to 100 for a score, `low`, `medium` or `high` for a level, `pass` or `fail`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Verdict {
    pub id: i64,
    /// The name of the reviewer that produced it.
    pub reviewer: String,
    pub branch: String,
    /// The commit that was judged.
    pub head: String,
    pub signal: ReviewerSignal,
    pub value: String,
    pub rationale: String,
    /// Who recorded it: `agent:<client>`.
    pub origin: String,
    pub model: Option<String>,
    /// The uid of the run's own thread, when the agent reported one.
    pub run_thread: Option<String>,
    /// Milliseconds since the Unix epoch, as the engine's clock gave them.
    pub at: i64,
    /// True when the branch has moved since the verdict was recorded. Derived on read; the
    /// store never sets it.
    #[serde(default)]
    pub stale: bool,
    /// The findings the verdict recorded: review comments, with their status now.
    #[serde(default)]
    pub findings: Vec<VerdictFinding>,
}

/// A person's approval of a branch's tip (D215 (2)): the commit approved, who approved it and
/// when. `stale` is derived on read, as for a verdict; the store never sets it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Approval {
    pub branch: String,
    /// The commit that was approved.
    pub head: String,
    /// What the person wrote with the approval, at most 500 characters.
    pub note: Option<String>,
    /// Who approved it: `human`.
    pub origin: String,
    /// Milliseconds since the Unix epoch, as the engine's clock gave them.
    pub at: i64,
    /// True when the branch has moved since the approval. Derived on read.
    #[serde(default)]
    pub stale: bool,
}

/// One finding of a verdict: a review comment with the verdict's origin (D189 (3)).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerdictFinding {
    pub comment_id: i64,
    pub severity: ReviewSeverity,
    pub path: String,
    pub status: ReviewStatus,
    /// The line the finding's anchor is on now, 1-based; `None` when it is outdated (D215 (3)).
    #[serde(default)]
    pub start_line: Option<u32>,
    /// What the finding says, redacted. Written by the reviewer: untrusted text (D215 (3)).
    #[serde(default)]
    pub body: String,
    /// The fix the reviewer suggested, redacted, when it gave one. Untrusted text (D215 (3)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_fix: Option<String>,
}

/// A finding an agent records with its verdict (D212 (6)): a flag on lines of a file, with
/// 1-based inclusive line numbers. The engine checks the lines against the file as it is now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FindingInput {
    pub severity: ReviewSeverity,
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub body: String,
    /// The fix the reviewer suggests, as plain text, at most 1,000 characters (D215 (3)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_fix: Option<String>,
}

/// A reviewer file that could not be read as a reviewer (D212 (1)), with the reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewerFileError {
    pub file: String,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reviewer_signal_is_written_in_snake_case() {
        let json = serde_json::to_value(ReviewerSignal::Score).unwrap();
        assert_eq!(json, "score");
        let direction: ReviewerDirection = serde_json::from_str("\"lower_is_better\"").unwrap();
        assert_eq!(direction, ReviewerDirection::LowerIsBetter);
    }

    #[test]
    fn levels_are_ordered_low_to_high() {
        assert!(Level::Low < Level::Medium && Level::Medium < Level::High);
        let json = serde_json::to_value(Level::High).unwrap();
        assert_eq!(json, "high");
    }

    #[test]
    fn a_finding_written_before_its_body_and_fix_reads_with_defaults() {
        let json = serde_json::json!({
            "comment_id": 9,
            "severity": "high",
            "path": "a.rs",
            "status": "open"
        });
        let finding: VerdictFinding = serde_json::from_value(json).unwrap();
        assert_eq!(finding.start_line, None);
        assert_eq!(finding.body, "");
        assert_eq!(finding.suggested_fix, None);
        let input: FindingInput = serde_json::from_value(serde_json::json!({
            "severity": "low",
            "path": "a.rs",
            "start_line": 1,
            "end_line": 1,
            "body": "x"
        }))
        .unwrap();
        assert_eq!(input.suggested_fix, None);
        let back = serde_json::to_value(&input).unwrap();
        assert!(back.get("suggested_fix").is_none());
    }

    #[test]
    fn a_verdict_without_stale_or_findings_reads_as_not_stale_and_empty() {
        let json = serde_json::json!({
            "id": 1,
            "reviewer": "security",
            "branch": "feat",
            "head": "abc",
            "signal": "level",
            "value": "low",
            "rationale": "fine",
            "origin": "agent:claude-code",
            "model": null,
            "run_thread": null,
            "at": 5
        });
        let verdict: Verdict = serde_json::from_value(json).unwrap();
        assert!(!verdict.stale);
        assert!(verdict.findings.is_empty());
    }
}
