//! One engine on one repository, and the actor every command it runs is recorded as.

use std::time::{Duration, Instant};

use gm_core::Engine;
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

/// How long one agent's code health call reads history before it answers with what the index
/// holds (docs/guide/code-health.md, "Building the index"; D229 (4)).
pub const HEALTH_BUDGET: Duration = Duration::from_secs(20);

/// What the server runs commands on. The actor is the MCP client's name, known after
/// `initialize`, and the agent's own label for the call in hand, when it gave one (D200).
pub struct Session {
    engine: Engine,
    client: String,
    label: Option<String>,
    /// The workflow run that started this connection, from `GITMASTER_WORKFLOW_RUN` (D219): a link
    /// that scorecards keep, never a check.
    workflow_run: Option<String>,
    /// The budget of each code health read (`HEALTH_BUDGET`); tests shorten it.
    health_budget: Duration,
    next_id: u64,
}

impl Session {
    /// Opens the repository that contains `path`.
    pub fn open(engine: Engine, path: &str) -> Result<Self, String> {
        let mut session = Self {
            engine,
            client: "unknown".to_owned(),
            label: None,
            workflow_run: None,
            health_budget: HEALTH_BUDGET,
            next_id: 1,
        };
        session.run(CommandKind::OpenRepo {
            path: path.to_owned(),
        })?;
        Ok(session)
    }

    /// Names the MCP client; blank names are ignored.
    pub fn set_client(&mut self, name: &str) {
        let name = name.trim();
        if !name.is_empty() {
            name.clone_into(&mut self.client);
        }
    }

    /// Sets the run that started this connection, already checked by `gm_core` (D219), or clears it.
    pub fn set_workflow_run(&mut self, run: Option<String>) {
        self.workflow_run = run;
    }

    /// The run that started this connection, if any.
    pub fn workflow_run(&self) -> Option<String> {
        self.workflow_run.clone()
    }

    /// Sets how long each code health read may take before it answers partially (`HEALTH_BUDGET`
    /// by default). Zero answers at once, with what the index holds.
    pub fn set_health_budget(&mut self, budget: Duration) {
        self.health_budget = budget;
    }

    /// Sets the label the agent gave for the call in hand, or clears it. The label is shown with
    /// the client's name (`agent:<client>/<label>`) and decides nothing (D200).
    pub fn set_label(&mut self, label: Option<String>) {
        self.label = label;
    }

    /// The actor name of the call in hand: the client, then the label when the call has one.
    fn actor_name(&self) -> String {
        match &self.label {
            Some(label) => format!("{}/{label}", self.client),
            None => self.client.clone(),
        }
    }

    /// Runs `kind` as the agent and returns its events, or the reason it failed.
    pub fn run(&mut self, kind: CommandKind) -> Result<Vec<Event>, String> {
        let id = OperationId(self.next_id);
        self.next_id += 1;
        let command = Command {
            actor: Actor::Agent {
                name: self.actor_name(),
            },
            kind,
        };
        let mut events = self.engine.handle(id, &command);
        // A provenance read answers after its blame runs as a job (D144). This server has no
        // runner thread, so each job runs to its end here, as the app's runner does.
        for job in self.engine.take_provenance_jobs() {
            events.extend(self.engine.provenance_report(job.run()));
        }
        // A code health read runs its index and indicators as a job too (D225).
        for mut job in self.engine.take_health_jobs() {
            job.deadline = Some(Instant::now() + self.health_budget);
            let outcome = job.run(&mut |_| {});
            events.extend(self.engine.health_report(outcome));
        }
        for event in &events {
            if let Event::OperationChanged {
                id: got,
                state,
                detail,
                ..
            } = event
                && *got == id
                && state.is_terminal()
                && *state != OperationState::Done
            {
                return Err(detail
                    .clone()
                    .unwrap_or_else(|| format!("the operation ended {state:?}")));
            }
        }
        Ok(events)
    }
}
