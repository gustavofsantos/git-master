//! Hooks of the app (docs/03). Slice 2 has one: `prepare-commit`, which turns
//! the staged diff and the current message into a suggested message. Native
//! Git hooks are not here; they stay with Git (D29).
//!
//! # Engine API for the UI
//!
//! Everything goes through `EngineHandle::dispatch` and `subscribe`; the
//! calls return at once and the answers arrive as events.
//!
//! - `ListHooks` → `HooksListed { hooks }`. Each hook has `state`:
//!   `enabled`, `needs_approval` (a repository hook not yet approved),
//!   `disabled`, or `invalid { reason }`. `applies_to_branch` says whether its
//!   `match` fits the current branch.
//! - `ApproveHook { name, command_hash }` → `HooksListed` again. The hash must
//!   be the one from the last listing; a command changed since then is refused.
//! - `RunPrepareCommit { message }` → returns the run's id `R`. The run
//!   refuses to start (`OperationChanged` failed, with the reason) when a
//!   matching hook needs approval or is invalid, or when none applies.
//! - `CancelHook { run: R }` → kills the running step's process group.
//!
//! Events for a run `R`, in this order:
//!
//! 1. `OperationChanged { R, requested }`, `{ R, gating }`, `{ R, executing }`.
//! 2. For each chain step `k` of `n`: `HookStarted { R, hook, step: k, of: n }`.
//!    A failure in step `k` ends the run with `HookFailed { R, hook, failure }`.
//! 3. Then exactly one of:
//!    - `HookFinished { R, message, repo_changed }`, then `OperationChanged { R, done }`.
//!      `message` is the last step's output; `repo_changed` is true when HEAD
//!      or the staged diff moved while the chain ran, so the UI shows the
//!      message as a suggestion (D05/D06 reconciliation).
//!    - `HookFailed` (timeout, non-zero exit, invalid output, another
//!      invocation's id, cannot start), then `OperationChanged { R, failed }`
//!      whose `detail` is one line for the user.
//!    - `HookCancelled { R, hook }`, then `OperationChanged { R, cancelled }`.
//!
//! Chains run in config order, each step receiving the previous output. Repo
//! hooks run only after `ApproveHook`; approval is stored outside the repo,
//! keyed by hook name and the sha256 of its command.

pub mod approval;
pub mod config;
pub mod run;

use std::path::Path;

use gm_schema::{HookInfo, HookScope, HookState, OperationId};
use sha2::{Digest, Sha256};

pub use approval::ApprovalStore;
pub use config::ConfigValue;
pub use run::{Plan, Report, Step};

/// One line of the engine's hook log: a step that ran, with the hash of its
/// command and its exit code. Kept in memory with the operation log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookRecord {
    pub run: OperationId,
    pub hook: String,
    pub command_hash: String,
    /// `None` when the step never exited (cancelled, timed out, not started).
    pub exit_code: Option<i32>,
    pub outcome: HookOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookOutcome {
    Ok,
    /// The one-line reason the user sees.
    Failed(String),
    Cancelled,
}

use crate::{GitError, GitPort};

/// What the repository looked like when a run started. Compared again when
/// the run answers: the `repo_changed` flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    /// The commit HEAD points at; `None` on an unborn branch.
    pub head: Option<String>,
    /// sha256 of the staged diff. The index is compared through its diff, so
    /// a restat that changes nothing staged does not count as a change.
    pub staged: String,
}

#[must_use]
pub fn fingerprint(head: Option<&str>, staged_diff: &str) -> Fingerprint {
    Fingerprint {
        head: head.map(str::to_owned),
        staged: sha256_hex(staged_diff.as_bytes()),
    }
}

/// Lowercase hex sha256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Every hook the repository sees, user hooks first, each with its state.
/// `branch` is `None` when HEAD is detached.
pub fn list(
    git: &dyn GitPort,
    root: &Path,
    branch: Option<&str>,
    approvals: &ApprovalStore,
) -> Result<Vec<HookInfo>, GitError> {
    let user = config::parse(&git.hook_config(root, HookScope::User)?, HookScope::User);
    let repo = config::parse(&git.hook_config(root, HookScope::Repo)?, HookScope::Repo);
    Ok(config::merge(user, repo)
        .iter()
        .map(|section| info(section, branch, approvals))
        .collect())
}

fn info(section: &config::Section, branch: Option<&str>, approvals: &ApprovalStore) -> HookInfo {
    let command_hash = if section.command.is_empty() {
        String::new()
    } else {
        sha256_hex(section.command.as_bytes())
    };
    let applies_to_branch = match section.branch_glob() {
        None => true,
        Some(glob) => branch.is_some_and(|branch| config::glob_match(glob, branch)),
    };
    let state = if let Some(reason) = &section.problem {
        HookState::Invalid {
            reason: reason.clone(),
        }
    } else if !section.enabled {
        HookState::Disabled
    } else if section.scope == HookScope::Repo
        && !approvals.is_approved(&section.name, &command_hash)
    {
        HookState::NeedsApproval
    } else {
        HookState::Enabled
    };
    HookInfo {
        name: section.name.clone(),
        scope: section.scope,
        command: section.command.clone(),
        command_hash,
        match_rule: section.match_rule.clone(),
        timeout_ms: u64::try_from(section.timeout.as_millis()).unwrap_or(u64::MAX),
        applies_to_branch,
        state,
    }
}

/// The hooks a `prepare-commit` run executes, in order. A hook that applies to
/// the branch but is not runnable (needs approval, invalid) stops the run:
/// skipping it would change the message without saying so. Disabled hooks and
/// hooks whose `match` does not fit are skipped.
pub fn chain(hooks: &[HookInfo]) -> Result<Vec<&HookInfo>, String> {
    let mut steps = Vec::new();
    for hook in hooks.iter().filter(|hook| hook.applies_to_branch) {
        match &hook.state {
            HookState::Enabled => steps.push(hook),
            HookState::NeedsApproval => {
                return Err(format!(
                    "hook {} needs approval: list the hooks and approve it",
                    hook.name
                ));
            }
            HookState::Invalid { reason } => {
                return Err(format!("hook {} is invalid: {reason}", hook.name));
            }
            HookState::Disabled => {}
        }
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fingerprint_moves_with_head_or_the_staged_diff() {
        let base = fingerprint(Some("a1"), "diff one");
        assert_eq!(base, fingerprint(Some("a1"), "diff one"));
        assert_ne!(base, fingerprint(Some("b2"), "diff one"));
        assert_ne!(base, fingerprint(Some("a1"), "diff two"));
        assert_ne!(base, fingerprint(None, "diff one"));
    }

    #[test]
    fn sha256_matches_the_standard_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn hook(name: &str, state: HookState, applies: bool) -> HookInfo {
        HookInfo {
            name: name.into(),
            scope: HookScope::User,
            command: "x".into(),
            command_hash: String::new(),
            match_rule: None,
            timeout_ms: 1,
            applies_to_branch: applies,
            state,
        }
    }

    #[test]
    fn the_chain_skips_disabled_and_non_matching_hooks() {
        let hooks = [
            hook("a", HookState::Enabled, true),
            hook("b", HookState::Disabled, true),
            hook("c", HookState::Enabled, false),
            hook("d", HookState::Enabled, true),
        ];
        let names: Vec<_> = chain(&hooks)
            .unwrap()
            .iter()
            .map(|h| h.name.as_str())
            .collect();
        assert_eq!(names, ["a", "d"]);
    }

    #[test]
    fn an_applicable_unapproved_or_invalid_hook_stops_the_chain() {
        let approval = [hook("a", HookState::NeedsApproval, true)];
        assert!(chain(&approval).unwrap_err().contains("needs approval"));
        let invalid = [hook(
            "b",
            HookState::Invalid {
                reason: "no command".into(),
            },
            true,
        )];
        assert_eq!(
            chain(&invalid).unwrap_err(),
            "hook b is invalid: no command"
        );
        // Not applicable, so it cannot stop anything.
        let elsewhere = [hook("c", HookState::NeedsApproval, false)];
        assert!(chain(&elsewhere).unwrap().is_empty());
    }
}
