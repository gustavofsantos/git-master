//! MCP tools over the same engine handle the UI uses. Slice 0 has one Read
//! tool, `repo_status`, and no transport; the socket and shim come in slice 7.

use gm_core::EngineHandle;
use gm_schema::{Actor, CommandKind, Event, OperationState, RepoStatus};
use serde_json::{Value, json};

/// What an MCP server lists for a tool.
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub output_schema: Value,
}

/// # Panics
///
/// Never in practice: a derived schema always serializes to JSON.
pub fn tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "repo_status",
        description: "Read the branch and the staged, unstaged and untracked changes of the open repository.",
        input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        output_schema: serde_json::to_value(schemars::schema_for!(RepoStatus))
            .expect("a schema is JSON"),
    }]
}

/// Runs `repo_status` as an agent: a normal operation, recorded with its actor.
/// Blocks until the operation ends, so call it off the UI thread.
pub fn repo_status(engine: &EngineHandle, agent: &str) -> Result<RepoStatus, String> {
    let events = engine.subscribe();
    let id = engine.dispatch(
        Actor::Agent {
            name: agent.to_owned(),
        },
        CommandKind::RefreshStatus,
    );
    let mut loaded = None;
    while let Ok(event) = events.recv_blocking() {
        match event {
            Event::StatusLoaded { id: got, status } if got == id => loaded = Some(status),
            Event::OperationChanged {
                id: got,
                state: OperationState::Done,
                ..
            } if got == id => {
                return loaded.ok_or_else(|| "operation finished without a status".to_owned());
            }
            Event::OperationChanged {
                id: got,
                state,
                detail,
                ..
            } if got == id && state.is_terminal() => {
                return Err(detail.unwrap_or_else(|| format!("operation ended {state:?}")));
            }
            _ => {}
        }
    }
    Err("the engine stopped".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_core::fake::FakeGit;
    use gm_schema::BranchInfo;

    fn engine() -> EngineHandle {
        let status = RepoStatus {
            root: "/r".into(),
            branch: BranchInfo {
                head: Some("main".into()),
                ..BranchInfo::default()
            },
            entries: vec![],
        };
        let handle = gm_core::spawn(Box::new(FakeGit::new("/r", status)));
        let events = handle.subscribe();
        handle.dispatch(Actor::Human, CommandKind::OpenRepo { path: "/r".into() });
        while let Ok(event) = events.recv_blocking() {
            if matches!(
                event,
                Event::OperationChanged {
                    state: OperationState::Done,
                    ..
                }
            ) {
                break;
            }
        }
        handle
    }

    #[test]
    fn repo_status_returns_the_open_repository() {
        let status = repo_status(&engine(), "claude").unwrap();
        assert_eq!(status.branch.head.as_deref(), Some("main"));
    }

    #[test]
    fn repo_status_without_a_repository_reports_why() {
        let handle = gm_core::spawn(Box::new(FakeGit::new("/r", RepoStatus::default())));
        assert_eq!(
            repo_status(&handle, "claude"),
            Err("no repository is open".to_owned())
        );
    }

    #[test]
    fn output_schema_describes_the_status_fields() {
        let tool = &tools()[0];
        assert_eq!(tool.name, "repo_status");
        let properties = &tool.output_schema["properties"];
        for field in ["root", "branch", "entries"] {
            assert!(properties.get(field).is_some(), "missing {field}");
        }
    }
}
