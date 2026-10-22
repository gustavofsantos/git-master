//! Hooks of the app (design/hooks-transitional.md). Slice 2 has one: `prepare-commit`, which turns
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
//! - `ApproveHook { name, command_hash }` → `HooksListed` again. Only a human
//!   may approve. The hash must be the one from the last listing; a command
//!   changed since then is refused.
//! - `RunPrepareCommit { message }` → returns the run's id `R`. The run
//!   refuses to start (`OperationChanged` failed, with the reason) when no
//!   hook applies, or when the only applicable hook is a repository hook still
//!   awaiting approval.
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
//! Chains run in config order, each step receiving the previous output. User
//! hooks run when enabled and matching; an invalid user hook stops the run.
//! Repository hooks run only after `ApproveHook`: until then they are skipped
//! and listed as `needs_approval`, so a repository cannot switch off the
//! user's own hooks. Approval is stored outside the repository, keyed by the
//! repository root, the hook name and the sha256 of its command.

pub mod approval;
pub mod config;
pub mod run;

use std::path::Path;

use gm_schema::{HookInfo, HookScope, HookState, OperationId};
use sha2::{Digest, Sha256};

pub use approval::ApprovalStore;
pub use config::ConfigValue;
pub use run::{Plan, Report, Step};

use crate::{GitError, GitPort};

/// One line of the engine's hook log: a step that ended, with its command and
/// the hash of it, and its exit code. Kept in memory with the operation log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookRecord {
    pub run: OperationId,
    pub hook: String,
    pub command: String,
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
    let repo_key = root.to_string_lossy();
    Ok(config::merge(user, repo)
        .iter()
        .map(|section| info(section, branch, approvals, &repo_key))
        .collect())
}

fn info(
    section: &config::Section,
    branch: Option<&str>,
    approvals: &ApprovalStore,
    repo: &str,
) -> HookInfo {
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
        && !approvals.is_approved(repo, &section.name, &command_hash)
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

/// What a run executes, and the repository hooks it leaves out.
#[derive(Debug, PartialEq, Eq)]
pub struct Chain<'a> {
    /// Enabled hooks that apply to the branch, in config order.
    pub steps: Vec<&'a HookInfo>,
    /// Names of applicable repository hooks skipped until approved.
    pub awaiting_approval: Vec<&'a str>,
}

/// The chain for a run. Repository hooks that are not enabled are skipped, so
/// an unapproved or broken repository section cannot change the message. An
/// invalid user section stops the run: it is the user's own config, and a
/// silent skip would hide a typo.
pub fn chain(hooks: &[HookInfo]) -> Result<Chain<'_>, String> {
    let mut steps = Vec::new();
    let mut awaiting_approval = Vec::new();
    for hook in hooks.iter().filter(|hook| hook.applies_to_branch) {
        match &hook.state {
            HookState::Enabled => steps.push(hook),
            HookState::NeedsApproval => awaiting_approval.push(hook.name.as_str()),
            HookState::Invalid { reason } if hook.scope == HookScope::User => {
                return Err(format!("hook {} is invalid: {reason}", hook.name));
            }
            HookState::Invalid { .. } | HookState::Disabled => {}
        }
    }
    Ok(Chain {
        steps,
        awaiting_approval,
    })
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

    fn hook(name: &str, scope: HookScope, state: HookState, applies: bool) -> HookInfo {
        HookInfo {
            name: name.into(),
            scope,
            command: "x".into(),
            command_hash: String::new(),
            match_rule: None,
            timeout_ms: 1,
            applies_to_branch: applies,
            state,
        }
    }

    fn names(chain: &Chain<'_>) -> Vec<String> {
        chain.steps.iter().map(|h| h.name.clone()).collect()
    }

    #[test]
    fn the_chain_skips_disabled_and_non_matching_hooks() {
        let hooks = [
            hook("a", HookScope::User, HookState::Enabled, true),
            hook("b", HookScope::User, HookState::Disabled, true),
            hook("c", HookScope::User, HookState::Enabled, false),
            hook("d", HookScope::User, HookState::Enabled, true),
        ];
        assert_eq!(names(&chain(&hooks).unwrap()), ["a", "d"]);
    }

    #[test]
    fn an_unapproved_repo_hook_is_skipped_and_reported() {
        let hooks = [
            hook("user", HookScope::User, HookState::Enabled, true),
            hook("team", HookScope::Repo, HookState::NeedsApproval, true),
        ];
        let chain = chain(&hooks).unwrap();
        assert_eq!(names(&chain), ["user"]);
        assert_eq!(chain.awaiting_approval, ["team"]);
    }

    #[test]
    fn an_invalid_repo_hook_cannot_stop_the_user_chain() {
        let hooks = [
            hook("user", HookScope::User, HookState::Enabled, true),
            hook(
                "team",
                HookScope::Repo,
                HookState::Invalid {
                    reason: "name clash".into(),
                },
                true,
            ),
        ];
        assert_eq!(names(&chain(&hooks).unwrap()), ["user"]);
    }

    #[test]
    fn an_invalid_user_hook_stops_the_run() {
        let hooks = [hook(
            "b",
            HookScope::User,
            HookState::Invalid {
                reason: "no command".into(),
            },
            true,
        )];
        assert_eq!(chain(&hooks).unwrap_err(), "hook b is invalid: no command");
    }
}
