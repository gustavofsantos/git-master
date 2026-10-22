//! When an `apply` call must ask the human (design/workflow-scripts.md, D64, D101). Pure: the
//! runtime gives it the facts and acts on the answer, so every rule has a test and the journal can
//! say why a call asked.

/// Who started a run (D64).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartedBy {
    /// The person pressed Run.
    Human,
    /// An engine event, such as `commit.created`.
    Event(String),
    /// An MCP client.
    Agent(String),
}

impl StartedBy {
    /// The text stored in the journal: `human`, `event:<name>` or `agent:<client>`.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Human => "human".to_owned(),
            Self::Event(name) => format!("event:{name}"),
            Self::Agent(client) => format!("agent:{client}"),
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text == "human" {
            return Some(Self::Human);
        }
        let (kind, name) = text.split_once(':')?;
        match kind {
            "event" => Some(Self::Event(name.to_owned())),
            "agent" => Some(Self::Agent(name.to_owned())),
            _ => None,
        }
    }
}

/// Why an `apply` call asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskReason {
    /// A run started by an event, without automatic approval of this version (D64(b)).
    EventRun,
    /// A run started by an agent always waits (D64(c)).
    AgentRun,
    /// `push.lease` asks in every mode (D64(d)).
    AlwaysAsks,
    /// An `exec` ran earlier in this run (D101): its output may have shaped these arguments.
    AfterExec,
}

impl AskReason {
    /// One line for the approval dialog.
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::EventRun => "This run was started by an event.",
            Self::AgentRun => "This run was started by an agent.",
            Self::AlwaysAsks => "Pushing always asks.",
            Self::AfterExec => {
                "A program ran earlier in this run, and its output may have shaped this call."
            }
        }
    }
}

/// What the runtime does with an `apply` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Run,
    Ask(AskReason),
}

/// The rule for an `apply` call. `auto_approve` is the grant tied to the reviewed hash of the
/// script (D101); `exec_seen` is whether this run already started an `exec`.
#[must_use]
pub fn gate_apply(func: &str, started_by: &StartedBy, auto_approve: bool, exec_seen: bool) -> Gate {
    if func == "push.lease" {
        return Gate::Ask(AskReason::AlwaysAsks);
    }
    if exec_seen {
        return Gate::Ask(AskReason::AfterExec);
    }
    match started_by {
        StartedBy::Human => Gate::Run,
        StartedBy::Event(_) if auto_approve => Gate::Run,
        StartedBy::Event(_) => Gate::Ask(AskReason::EventRun),
        StartedBy::Agent(_) => Gate::Ask(AskReason::AgentRun),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> StartedBy {
        StartedBy::Event("commit.created".into())
    }

    fn agent() -> StartedBy {
        StartedBy::Agent("claude".into())
    }

    #[test]
    fn a_manual_run_applies() {
        assert_eq!(
            gate_apply("branch.create", &StartedBy::Human, false, false),
            Gate::Run
        );
    }

    #[test]
    fn an_event_run_asks_unless_the_version_has_the_grant() {
        assert_eq!(
            gate_apply("stage.all", &event(), false, false),
            Gate::Ask(AskReason::EventRun)
        );
        assert_eq!(gate_apply("stage.all", &event(), true, false), Gate::Run);
    }

    #[test]
    fn an_agent_run_always_asks_even_with_the_grant() {
        assert_eq!(
            gate_apply("stage.all", &agent(), true, false),
            Gate::Ask(AskReason::AgentRun)
        );
    }

    #[test]
    fn push_asks_in_every_mode() {
        for who in [StartedBy::Human, event(), agent()] {
            assert_eq!(
                gate_apply("push.lease", &who, true, false),
                Gate::Ask(AskReason::AlwaysAsks)
            );
        }
    }

    #[test]
    fn after_an_exec_every_apply_asks_even_a_manual_one() {
        for who in [StartedBy::Human, event()] {
            assert_eq!(
                gate_apply("commit.create", &who, true, true),
                Gate::Ask(AskReason::AfterExec)
            );
        }
    }

    #[test]
    fn the_journal_label_round_trips() {
        for who in [StartedBy::Human, event(), agent()] {
            assert_eq!(StartedBy::parse(&who.label()), Some(who));
        }
        assert_eq!(StartedBy::parse("robot"), None);
    }
}
