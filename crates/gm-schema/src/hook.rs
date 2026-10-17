//! The hook contract (docs/03): JSON in and out of a user script, version 1.
//! Also the hook descriptions and failures the front-ends show. Only
//! `prepare-commit` exists in this slice (D29).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The only contract version so far. Input always carries it; output may omit it.
pub const CONTRACT_VERSION: u32 = 1;

/// The event name a `prepare-commit` script receives.
pub const PREPARE_COMMIT: &str = "prepare-commit";

fn contract_version() -> u32 {
    CONTRACT_VERSION
}

/// What a script reads on stdin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookInput {
    pub schema_version: u32,
    pub event: String,
    /// Echoed back by the script; a different id is a hook error.
    pub invocation_id: String,
    pub repo: HookRepo,
    pub input: HookPayload,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookRepo {
    pub root: String,
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    /// `None` on an unborn branch.
    pub head: Option<String>,
}

/// Exactly one of `staged_diff` and `staged_diff_path` is set: a diff over the
/// inline limit goes to a temporary file, so the contract stays the same shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookPayload {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_diff: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_diff_path: Option<String>,
}

/// What a script writes on stdout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookOutput {
    #[serde(default = "contract_version")]
    pub schema_version: u32,
    pub invocation_id: String,
    pub output: HookResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookResult {
    pub message: String,
}

/// Where a hook was declared. Repository hooks run only after approval (D17).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookScope {
    /// Global or user git config.
    User,
    /// The repository's own config.
    Repo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookState {
    Enabled,
    /// A repository hook whose command has no approval yet.
    NeedsApproval,
    /// `enabled = false` in its config section.
    Disabled,
    /// The section cannot run as written; `reason` says why.
    Invalid { reason: String },
}

/// One `[gitmaster "prepare-commit.<name>"]` section, as the UI lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookInfo {
    pub name: String,
    pub scope: HookScope,
    /// Empty when the section has no `command`.
    pub command: String,
    /// sha256 of `command`, hex. Approval names this value.
    pub command_hash: String,
    /// The raw `match` value, such as `branch:feat/*`.
    #[serde(rename = "match")]
    pub match_rule: Option<String>,
    pub timeout_ms: u64,
    /// Whether `match` fits the current branch (true when there is no `match`).
    pub applies_to_branch: bool,
    pub state: HookState,
}

/// Why a running hook step ended without a usable answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookFailure {
    Timeout { timeout_ms: u64 },
    /// `code` is `None` when the process was killed by a signal. `stderr`
    /// holds the first lines only.
    Exited { code: Option<i32>, stderr: String },
    InvalidOutput { detail: String },
    InvocationMismatch { expected: String, got: String },
    CannotStart { detail: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_wire_format_is_versioned_and_inline_by_default() {
        let input = HookInput {
            schema_version: CONTRACT_VERSION,
            event: PREPARE_COMMIT.into(),
            invocation_id: "prep-7-1".into(),
            repo: HookRepo {
                root: "/work/ledger".into(),
                branch: Some("ledger-ui".into()),
                head: None,
            },
            input: HookPayload {
                message: String::new(),
                staged_diff: Some("diff --git".into()),
                staged_diff_path: None,
            },
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["event"], "prepare-commit");
        assert_eq!(json["repo"]["branch"], "ledger-ui");
        assert!(json["repo"]["head"].is_null());
        assert_eq!(json["input"]["staged_diff"], "diff --git");
        assert!(json["input"].get("staged_diff_path").is_none());
    }

    #[test]
    fn a_large_diff_travels_as_a_path_and_not_inline() {
        let payload = HookPayload {
            message: "m".into(),
            staged_diff: None,
            staged_diff_path: Some("/tmp/x.diff".into()),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert!(json.get("staged_diff").is_none());
        assert_eq!(json["staged_diff_path"], "/tmp/x.diff");
    }

    #[test]
    fn output_may_omit_the_schema_version() {
        let text = r#"{"invocation_id":"prep-7-1","output":{"message":"feat: x"}}"#;
        let output: HookOutput = serde_json::from_str(text).unwrap();
        assert_eq!(output.schema_version, CONTRACT_VERSION);
        assert_eq!(output.output.message, "feat: x");
    }

    #[test]
    fn hook_info_uses_match_as_the_key() {
        let info = HookInfo {
            name: "llm".into(),
            scope: HookScope::User,
            command: "suggest".into(),
            command_hash: "ab".into(),
            match_rule: Some("branch:feat/*".into()),
            timeout_ms: 30_000,
            applies_to_branch: true,
            state: HookState::Invalid {
                reason: "x".into(),
            },
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["match"], "branch:feat/*");
        assert_eq!(json["scope"], "user");
        assert_eq!(json["state"]["type"], "invalid");
        assert_eq!(json["state"]["reason"], "x");
    }

    #[test]
    fn failures_are_tagged() {
        let failure = HookFailure::Timeout { timeout_ms: 5 };
        let json = serde_json::to_value(&failure).unwrap();
        assert_eq!(json["type"], "timeout");
        assert_eq!(json["timeout_ms"], 5);
    }
}
