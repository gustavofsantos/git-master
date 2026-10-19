//! The window's view of the `prepare-commit` hooks: what the engine listed,
//! the run in flight or the last one, and the suggestion it left. No GPUI in
//! here. The view reads the composer and passes its text in, so the rule that
//! protects typed text is a plain function.

use gm_schema::{HookInfo, HookScope, HookState, OperationId, OperationState};

/// Where the current run is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Sent; the engine has not started the first step yet.
    Starting,
    /// A step is running; `step` counts from 1 of `of`.
    Running {
        hook: String,
        step: u32,
        of: u32,
    },
    /// The chain answered. Waits for `Hooks::take_fill`, which decides what
    /// happens to the message.
    Answered {
        hook: String,
        message: String,
        repo_changed: bool,
    },
    /// The answer was applied or offered. `hook` is the last step that ran.
    Ready {
        hook: String,
    },
    /// The run ended without an answer; the text is the one-line reason.
    Failed(String),
    Cancelled,
}

impl Phase {
    /// The text the composer's chip shows for this phase.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Phase::Starting => "Starting the hooks...".to_owned(),
            Phase::Running { hook, step, of } => format!("Running {hook} ({step} of {of})"),
            Phase::Answered { .. } => "Hook answered".to_owned(),
            Phase::Ready { hook } => format!("{hook} done"),
            Phase::Failed(reason) => reason.clone(),
            Phase::Cancelled => "Hook cancelled".to_owned(),
        }
    }
}

/// One `RunPrepareCommit`, with the message it was started from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub id: OperationId,
    /// The composer text when the run was sent. The answer may replace the
    /// composer only while the text still equals this.
    pub sent: String,
    pub phase: Phase,
}

/// What to do with an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fill {
    /// Put the message in the composer: nothing the user typed is lost.
    Apply(String),
    /// Offer the message; the user chooses Replace or Discard.
    Suggest(String),
}

/// The rule that keeps typed text. The answer applies only when the repository
/// is as the run saw it and the composer still holds what was sent. Otherwise
/// it is a suggestion. An answer equal to the composer text is applied, as it
/// changes nothing.
#[must_use]
pub fn fill_for(sent: &str, typed: &str, repo_changed: bool, message: &str) -> Fill {
    let untouched = !repo_changed && typed == sent;
    if untouched || typed == message {
        Fill::Apply(message.to_owned())
    } else {
        Fill::Suggest(message.to_owned())
    }
}

/// The hook state the window keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hooks {
    /// Every hook the engine listed, or `None` until the first listing.
    pub listed: Option<Vec<HookInfo>>,
    /// The current run, or the last one, until the repository changes.
    pub run: Option<Run>,
    /// A message the last run offered and the user has not used or dropped.
    pub suggestion: Option<String>,
    /// The command hash of a repository hook the user denied this session.
    denied: Option<String>,
}

impl Hooks {
    /// Forgets everything that belonged to the repository being left.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn listed(&mut self, hooks: Vec<HookInfo>) {
        self.listed = Some(hooks);
    }

    /// A run was sent with `sent` in the composer. It replaces any run and
    /// any suggestion from before.
    pub fn start(&mut self, id: OperationId, sent: String) {
        self.run = Some(Run {
            id,
            sent,
            phase: Phase::Starting,
        });
        self.suggestion = None;
    }

    /// Whether `id` is the current run. Events of any other operation are not
    /// the hooks' to read.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.run.as_ref().is_some_and(|run| run.id == id)
    }

    /// The current run's id while it is still running.
    #[must_use]
    pub fn in_flight(&self) -> Option<OperationId> {
        self.run
            .as_ref()
            .filter(|run| matches!(run.phase, Phase::Starting | Phase::Running { .. }))
            .map(|run| run.id)
    }

    pub fn step_started(&mut self, id: OperationId, hook: String, step: u32, of: u32) {
        if let Some(run) = self.run.as_mut().filter(|run| run.id == id) {
            run.phase = Phase::Running { hook, step, of };
        }
    }

    /// The chain answered. The last step's name comes from the phase before.
    pub fn answered(&mut self, id: OperationId, message: String, repo_changed: bool) {
        if let Some(run) = self.run.as_mut().filter(|run| run.id == id) {
            let hook = match &run.phase {
                Phase::Running { hook, .. } => hook.clone(),
                _ => String::new(),
            };
            run.phase = Phase::Answered {
                hook,
                message,
                repo_changed,
            };
        }
    }

    /// The run ended in `state`. `Done` needs nothing: the answer already
    /// arrived and `take_fill` handles it.
    pub fn ended(&mut self, id: OperationId, state: OperationState, detail: Option<String>) {
        let Some(run) = self.run.as_mut().filter(|run| run.id == id) else {
            return;
        };
        match state {
            OperationState::Failed => {
                run.phase =
                    Phase::Failed(detail.unwrap_or_else(|| "the hook run failed".to_owned()));
            }
            OperationState::Cancelled => run.phase = Phase::Cancelled,
            _ => {}
        }
    }

    /// Turns an answer that is waiting into the fill it calls for, given the
    /// composer text as it is now. A suggestion is kept here. Returns `None`
    /// when no answer waits.
    pub fn take_fill(&mut self, typed: &str) -> Option<Fill> {
        let run = self.run.as_mut()?;
        let Phase::Answered {
            hook,
            message,
            repo_changed,
        } = &run.phase
        else {
            return None;
        };
        let fill = fill_for(&run.sent, typed, *repo_changed, message);
        let hook = hook.clone();
        run.phase = Phase::Ready { hook };
        if let Fill::Suggest(text) = &fill {
            self.suggestion = Some(text.clone());
        }
        Some(fill)
    }

    /// Offers `message` as the suggestion, in place of any one waiting. The
    /// workflows use it too: a suggested message has one place to wait.
    pub fn offer(&mut self, message: String) {
        self.suggestion = Some(message);
    }

    pub fn drop_suggestion(&mut self) {
        self.suggestion = None;
    }

    /// Hides the approval prompt for `hash` until the window closes.
    pub fn deny(&mut self, hash: &str) {
        self.denied = Some(hash.to_owned());
    }

    /// How many hooks apply to this branch and are not switched off; `None`
    /// until the first listing.
    #[must_use]
    pub fn applicable(&self) -> Option<usize> {
        self.listed.as_ref().map(|hooks| {
            hooks
                .iter()
                .filter(|hook| hook.applies_to_branch && hook.state != HookState::Disabled)
                .count()
        })
    }

    /// The first repository hook that waits for approval and is not denied.
    #[must_use]
    pub fn pending_approval(&self) -> Option<&HookInfo> {
        self.listed.as_ref()?.iter().find(|hook| {
            hook.scope == HookScope::Repo
                && hook.applies_to_branch
                && hook.state == HookState::NeedsApproval
                && self.denied.as_deref() != Some(hook.command_hash.as_str())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> OperationId {
        OperationId(n)
    }

    #[test]
    fn an_untouched_composer_on_the_same_state_takes_the_answer() {
        assert_eq!(
            fill_for("draft", "draft", false, "feat: x"),
            Fill::Apply("feat: x".into())
        );
    }

    #[test]
    fn text_typed_during_the_run_is_never_overwritten() {
        assert_eq!(
            fill_for("draft", "draft plus", false, "feat: x"),
            Fill::Suggest("feat: x".into())
        );
        assert_eq!(
            fill_for("", "fix", false, "feat: x"),
            Fill::Suggest("feat: x".into())
        );
    }

    #[test]
    fn a_moved_repository_turns_the_answer_into_a_suggestion() {
        assert_eq!(
            fill_for("", "", true, "feat: x"),
            Fill::Suggest("feat: x".into())
        );
    }

    #[test]
    fn an_answer_equal_to_the_typed_text_changes_nothing_and_applies() {
        assert_eq!(
            fill_for("draft", "same", true, "same"),
            Fill::Apply("same".into())
        );
    }

    #[test]
    fn a_run_moves_from_starting_to_ready_through_its_steps() {
        let mut hooks = Hooks::default();
        hooks.start(id(7), String::new());
        assert_eq!(hooks.in_flight(), Some(id(7)));
        hooks.step_started(id(7), "llm".into(), 1, 2);
        hooks.answered(id(7), "feat: x".into(), false);
        assert_eq!(hooks.take_fill(""), Some(Fill::Apply("feat: x".into())));
        assert_eq!(hooks.in_flight(), None);
        assert_eq!(
            hooks.run.as_ref().map(|run| run.phase.clone()),
            Some(Phase::Ready { hook: "llm".into() })
        );
        assert_eq!(hooks.take_fill(""), None, "an answer is taken once");
    }

    #[test]
    fn an_event_of_another_operation_is_ignored() {
        let mut hooks = Hooks::default();
        hooks.start(id(7), String::new());
        hooks.step_started(id(3), "old".into(), 1, 1);
        hooks.ended(id(3), OperationState::Failed, Some("old".into()));
        assert!(matches!(
            hooks.run.as_ref().map(|run| &run.phase),
            Some(Phase::Starting)
        ));
        assert!(!hooks.owns(id(3)));
    }

    #[test]
    fn a_failure_keeps_the_reason_and_a_missing_reason_says_so() {
        let mut hooks = Hooks::default();
        hooks.start(id(1), String::new());
        hooks.ended(
            id(1),
            OperationState::Failed,
            Some("hook llm timed out".into()),
        );
        assert_eq!(
            hooks.run.as_ref().map(|run| run.phase.clone()),
            Some(Phase::Failed("hook llm timed out".into()))
        );
        hooks.start(id(2), String::new());
        hooks.ended(id(2), OperationState::Failed, None);
        assert_eq!(
            hooks.run.as_ref().map(|run| run.phase.clone()),
            Some(Phase::Failed("the hook run failed".into()))
        );
    }

    #[test]
    fn a_cancelled_run_is_cancelled_and_never_fills() {
        let mut hooks = Hooks::default();
        hooks.start(id(4), String::new());
        hooks.ended(id(4), OperationState::Cancelled, None);
        assert_eq!(
            hooks.run.as_ref().map(|run| run.phase.clone()),
            Some(Phase::Cancelled)
        );
        assert_eq!(hooks.take_fill(""), None);
    }

    #[test]
    fn a_suggestion_is_kept_and_a_new_run_clears_it() {
        let mut hooks = Hooks::default();
        hooks.start(id(1), "draft".into());
        hooks.answered(id(1), "feat: x".into(), true);
        assert_eq!(
            hooks.take_fill("draft"),
            Some(Fill::Suggest("feat: x".into()))
        );
        assert_eq!(hooks.suggestion.as_deref(), Some("feat: x"));
        hooks.start(id(2), "draft".into());
        assert_eq!(hooks.suggestion, None);
    }

    #[test]
    fn the_chip_says_what_the_run_is_doing() {
        let running = Phase::Running {
            hook: "llm".into(),
            step: 1,
            of: 2,
        };
        assert_eq!(running.label(), "Running llm (1 of 2)");
        assert_eq!(Phase::Ready { hook: "llm".into() }.label(), "llm done");
        assert_eq!(
            Phase::Failed("hook llm timed out after 30s".into()).label(),
            "hook llm timed out after 30s"
        );
        assert_eq!(Phase::Cancelled.label(), "Hook cancelled");
    }

    #[test]
    fn a_reset_forgets_the_run_and_the_listing() {
        let mut hooks = Hooks::default();
        hooks.listed(Vec::new());
        hooks.start(id(1), String::new());
        hooks.reset();
        assert_eq!(hooks, Hooks::default());
    }

    fn hook(name: &str, scope: HookScope, state: HookState, applies: bool) -> HookInfo {
        HookInfo {
            name: name.into(),
            scope,
            command: format!("run-{name}"),
            command_hash: format!("hash-{name}"),
            match_rule: None,
            timeout_ms: 1000,
            applies_to_branch: applies,
            state,
        }
    }

    #[test]
    fn applicable_counts_hooks_that_apply_and_are_not_disabled() {
        let mut hooks = Hooks::default();
        assert_eq!(hooks.applicable(), None);
        hooks.listed(vec![
            hook("a", HookScope::User, HookState::Enabled, true),
            hook("b", HookScope::User, HookState::Disabled, true),
            hook("c", HookScope::User, HookState::Enabled, false),
            hook("d", HookScope::Repo, HookState::NeedsApproval, true),
        ]);
        assert_eq!(hooks.applicable(), Some(2));
    }

    #[test]
    fn a_repository_hook_awaiting_approval_is_pending_until_denied() {
        let mut hooks = Hooks::default();
        hooks.listed(vec![
            hook("user", HookScope::User, HookState::NeedsApproval, true),
            hook("team", HookScope::Repo, HookState::NeedsApproval, true),
        ]);
        assert_eq!(
            hooks.pending_approval().map(|h| h.name.as_str()),
            Some("team")
        );
        hooks.deny("hash-team");
        assert_eq!(hooks.pending_approval().map(|h| h.name.as_str()), None);
    }

    #[test]
    fn a_hook_that_does_not_match_the_branch_is_not_pending() {
        let mut hooks = Hooks::default();
        hooks.listed(vec![hook(
            "team",
            HookScope::Repo,
            HookState::NeedsApproval,
            false,
        )]);
        assert!(hooks.pending_approval().is_none());
    }
}
