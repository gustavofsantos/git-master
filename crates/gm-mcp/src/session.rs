//! One engine on one repository, and the actor every command it runs is recorded as.

use gm_core::Engine;
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

/// What the server runs commands on. The actor is the MCP client's name, known after
/// `initialize`.
pub struct Session {
    engine: Engine,
    client: String,
    next_id: u64,
}

impl Session {
    /// Opens the repository that contains `path`.
    pub fn open(engine: Engine, path: &str) -> Result<Self, String> {
        let mut session = Self {
            engine,
            client: "unknown".to_owned(),
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

    /// Runs `kind` as the agent and returns its events, or the reason it failed.
    pub fn run(&mut self, kind: CommandKind) -> Result<Vec<Event>, String> {
        let id = OperationId(self.next_id);
        self.next_id += 1;
        let command = Command {
            actor: Actor::Agent {
                name: self.client.clone(),
            },
            kind,
        };
        let mut events = self.engine.handle(id, &command);
        // A provenance read answers after its blame runs as a job (D144). This server has no
        // runner thread, so each job runs to its end here, as the app's runner does.
        for job in self.engine.take_provenance_jobs() {
            events.extend(self.engine.provenance_report(job.run()));
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
