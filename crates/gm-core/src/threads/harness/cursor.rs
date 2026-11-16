//! The Cursor harness (D256).
//!
//! Every format in this module comes from the Cursor documentation (Hooks, Plugins) and is
//! UNVERIFIED against a real session, like the Claude Code formats were (Q26). The parser is
//! lenient: a missing optional field never fails a payload, unknown fields are ignored, and an
//! unknown event is reported in `unrecognized`, not as an error.
//!
//! Cursor's hooks carry the prompt (`beforeSubmitPrompt`) and the assistant's reply
//! (`afterAgentResponse`) themselves, so this adapter captures from hooks alone and reads no
//! transcript. Every payload names the conversation (`conversation_id`), which is the thread's
//! session id, and the workspace (`workspace_roots`); tool events also carry `cwd`.
//!
//! Cursor reads a hook's standard output as JSON, and a permission hook (`preToolUse`,
//! `beforeSubmitPrompt`) whose output is not JSON blocks the action. `HOOK_REPLY` is what
//! `gitmaster thread hook --harness cursor` prints instead of nothing: an empty object, which
//! decides nothing.
//!
//! The plugin is source in the `git-master-ai` repository, the `ai/` submodule here
//! (`plugins/cursor/`). Its hooks and MCP settings call `gitmaster` from `PATH`;
//! `crates/gitmaster/tests/cursor_plugin.rs` checks the files against the events and commands
//! this adapter expects.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use gm_schema::threads::{
    CanonicalEvent, EditChange, EditIntent, EditPhase, EventHeader, SessionOrigin, ToolKind,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    Capabilities, EditPhaseInfo, HarnessAdapter, HookInput, LogChunk, ParseError, SessionLog,
    stable_source_id,
};

/// The harness id, stored with every thread.
pub const ID: &str = "cursor";

/// What the hook prints for Cursor: an empty JSON object, which allows and asks nothing.
pub const HOOK_REPLY: &str = "{}";

/// The Cursor adapter. Stateless.
#[derive(Clone, Copy, Debug, Default)]
pub struct CursorAdapter;

impl HarnessAdapter for CursorAdapter {
    fn id(&self) -> &'static str {
        ID
    }

    fn display_name(&self) -> &'static str {
        "Cursor"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            session_log: false,
            pre_edit_event: true,
            tool_results: true,
            subagents: false,
            resume_same_id: true,
        }
    }

    fn parse_hook(&self, payload: &[u8]) -> Result<HookInput, ParseError> {
        parse_hook(payload)
    }

    fn read_session_log(&self, path: &Path, _from: u64) -> Result<LogChunk, ParseError> {
        Err(ParseError(format!(
            "the cursor adapter reads no session log ({})",
            path.display()
        )))
    }

    fn discover(&self, _worktree: &Path) -> Vec<SessionLog> {
        Vec::new()
    }

    fn hook_cwd(&self, payload: &[u8]) -> Option<PathBuf> {
        let value: Value = serde_json::from_slice(payload).ok()?;
        payload_cwd(&value).map(PathBuf::from)
    }

    fn hook_reply(&self) -> Option<&'static str> {
        Some(HOOK_REPLY)
    }
}

/// The directory a payload runs in: its `cwd` (tool events), else its first workspace root.
fn payload_cwd(value: &Value) -> Option<String> {
    field(value, "cwd").or_else(|| {
        value
            .get("workspace_roots")
            .and_then(Value::as_array)
            .and_then(|roots| {
                roots
                    .iter()
                    .filter_map(Value::as_str)
                    .find(|root| !root.is_empty())
            })
            .map(str::to_owned)
    })
}

/// One hook invocation. Hook payloads carry no time, so every event gets `at` 0; the pipeline
/// stamps it.
fn parse_hook(payload: &[u8]) -> Result<HookInput, ParseError> {
    let value: Value = serde_json::from_slice(payload)
        .map_err(|err| ParseError(format!("payload is not JSON: {err}")))?;
    if !value.is_object() {
        return Err(ParseError("payload is not a JSON object".to_owned()));
    }
    let session = field(&value, "conversation_id")
        .or_else(|| field(&value, "session_id"))
        .ok_or_else(|| ParseError("payload has no conversation_id".to_owned()))?;
    let event = field(&value, "hook_event_name")
        .ok_or_else(|| ParseError("payload has no hook_event_name".to_owned()))?;
    // The user message this event belongs to; a new one starts with each prompt.
    let generation = field(&value, "generation_id").unwrap_or_else(|| payload_digest(payload));
    let mut input = HookInput {
        session_id: session.clone(),
        cwd: payload_cwd(&value),
        log_path: None,
        ..HookInput::default()
    };

    match event.as_str() {
        "sessionStart" => input.events.push(CanonicalEvent::SessionStarted {
            // A resumed conversation starts a new session under the same conversation id.
            header: header(
                &session,
                format!(
                    "session-start:{}",
                    field(&value, "session_id").unwrap_or_else(|| generation.clone())
                ),
            ),
            cwd: input.cwd.clone().unwrap_or_default(),
            model: model(&value),
            harness_version: field(&value, "cursor_version"),
            log_path: None,
            origin: SessionOrigin::New,
        }),
        "beforeSubmitPrompt" => input.events.push(CanonicalEvent::PromptSubmitted {
            header: header(&session, format!("prompt:{generation}")),
            text: raw_str(&value, "prompt").unwrap_or_default(),
        }),
        "afterAgentResponse" => {
            let text = raw_str(&value, "text").unwrap_or_default();
            input.events.push(CanonicalEvent::AssistantMessage {
                header: header(
                    &session,
                    format!("response:{generation}:{}", payload_digest(text.as_bytes())),
                ),
                text,
                model: model(&value),
            });
        }
        "preToolUse" => {
            let call = hook_tool(&value, &session);
            input.edit_phase = call.edit_phase(EditPhase::Before);
            input.events.push(call.event(&session));
        }
        "postToolUse" | "postToolUseFailure" => {
            // The call is repeated so a post without its pre still creates the call row.
            let call = hook_tool(&value, &session);
            input.edit_phase = call.edit_phase(EditPhase::After);
            input.events.push(call.event(&session));
            let failed = event == "postToolUseFailure";
            let text = if failed {
                failure_text(&value)
            } else {
                output_text(value.get("tool_output"))
            };
            input.events.push(CanonicalEvent::ToolReturned {
                header: header(&session, format!("{}:result", call.source_id)),
                call_source_id: call.source_id.clone(),
                text,
                is_error: failed,
            });
        }
        "subagentStop" => input.events.push(CanonicalEvent::Notice {
            header: header(
                &session,
                format!("subagent-stop:{}", payload_digest(payload)),
            ),
            text: subagent_notice(&value),
        }),
        "stop" => {
            let round = value.get("loop_count").and_then(Value::as_u64).unwrap_or(0);
            input.events.push(CanonicalEvent::TurnEnded {
                header: header(&session, format!("stop:{generation}:{round}")),
                parent_call: None,
            });
        }
        "sessionEnd" => {
            let reason = field(&value, "reason").unwrap_or_else(|| "unknown".to_owned());
            input.events.push(CanonicalEvent::SessionEnded {
                header: header(&session, format!("session-end:{reason}")),
                reason,
            });
        }
        other => input.unrecognized.push(other.to_owned()),
    }
    Ok(input)
}

/// The model of a payload: the structured `model_id`, else the legacy `model` slug.
fn model(value: &Value) -> Option<String> {
    field(value, "model_id").or_else(|| field(value, "model"))
}

/// A tool call as a hook payload names it.
struct ToolUse {
    source_id: String,
    native: String,
    tool: ToolKind,
    args: Value,
    edit: Option<EditIntent>,
}

impl ToolUse {
    /// The edit phase of this call, when it edits a file.
    fn edit_phase(&self, phase: EditPhase) -> Option<EditPhaseInfo> {
        self.edit.as_ref().map(|edit| EditPhaseInfo {
            phase,
            call_source_id: self.source_id.clone(),
            path: edit.path.clone(),
        })
    }

    fn event(&self, session: &str) -> CanonicalEvent {
        CanonicalEvent::ToolCalled {
            header: header(session, self.source_id.clone()),
            tool: self.tool,
            native_tool: self.native.clone(),
            args: self.args.clone(),
            parent_call: None,
            edit: self.edit.clone(),
        }
    }
}

/// The tool call of a hook payload. Without `tool_use_id` the id is a hash of the tool name
/// and its input, so a pre and a post of the same call still agree.
fn hook_tool(value: &Value, session: &str) -> ToolUse {
    let native = field(value, "tool_name").unwrap_or_else(|| "unknown".to_owned());
    let args = tool_input(value.get("tool_input"));
    let source_id = field(value, "tool_use_id")
        .unwrap_or_else(|| stable_source_id(ID, session, tool_position(&native, &args)));
    let edit = edit_intent(&native, &args);
    let tool = match (&edit, tool_kind(&native)) {
        // A `Write` that replaces text changes an existing file.
        (Some(intent), ToolKind::Write) if intent.old_text.is_some() => ToolKind::Edit,
        (_, kind) => kind,
    };
    ToolUse {
        source_id,
        native,
        tool,
        args,
        edit,
    }
}

/// `tool_input` as an object. Cursor documents it as an object, and as a JSON string for MCP
/// tools; a string that holds JSON is decoded.
fn tool_input(input: Option<&Value>) -> Value {
    match input {
        None => Value::Null,
        Some(Value::String(text)) => {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
        }
        Some(value) => value.clone(),
    }
}

/// The canonical class of a native tool name. `MCP:<tool>` and unknown names are `Other` with
/// the name kept.
fn tool_kind(native: &str) -> ToolKind {
    match native {
        "Read" => ToolKind::Read,
        "Write" | "Delete" => ToolKind::Write,
        "Shell" => ToolKind::Shell,
        "Grep" | "Glob" => ToolKind::Search,
        "WebSearch" | "WebFetch" => ToolKind::Web,
        "Task" => ToolKind::Agent,
        _ => ToolKind::Other,
    }
}

/// The edit a file-changing tool makes, from its input. `None` for other tools, or when the
/// input names no file. Cursor does not document the input of `Write` and `Delete`, so the
/// path is read from the names its tools are known to use.
fn edit_intent(native: &str, args: &Value) -> Option<EditIntent> {
    let change = match native {
        "Write" => EditChange::Modify,
        "Delete" => EditChange::Delete,
        _ => return None,
    };
    let path = ["file_path", "path", "target_file", "filePath"]
        .iter()
        .find_map(|key| field(args, key))?;
    let (old_text, new_text) = if change == EditChange::Delete {
        (None, None)
    } else {
        (
            raw_str(args, "old_string"),
            raw_str(args, "new_string")
                .or_else(|| raw_str(args, "contents"))
                .or_else(|| raw_str(args, "content")),
        )
    };
    Some(EditIntent {
        path,
        change,
        old_path: None,
        old_text,
        new_text,
    })
}

/// The text of a tool result. Cursor sends `tool_output` as a string holding JSON: an object
/// with `output` (a shell) or `content` is that text, any other JSON is kept as it is.
fn output_text(output: Option<&Value>) -> String {
    let decoded = match output {
        None | Some(Value::Null) => return String::new(),
        Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
            Ok(value @ Value::Object(_)) => value,
            _ => return text.clone(),
        },
        Some(value) => value.clone(),
    };
    ["output", "content", "result"]
        .iter()
        .find_map(|key| raw_str(&decoded, key))
        .unwrap_or_else(|| decoded.to_string())
}

/// The text of a failed call: its error message, with the kind of failure when it is not a
/// plain error.
fn failure_text(value: &Value) -> String {
    let message = raw_str(value, "error_message").unwrap_or_default();
    match field(value, "failure_type").as_deref() {
        None | Some("error") => message,
        Some(kind) if message.is_empty() => kind.to_owned(),
        Some(kind) => format!("{kind}: {message}"),
    }
}

/// The notice of a finished sub-agent: its type, status, task and summary.
fn subagent_notice(value: &Value) -> String {
    let kind = field(value, "subagent_type").unwrap_or_else(|| "subagent".to_owned());
    let status = field(value, "status").unwrap_or_else(|| "finished".to_owned());
    let mut text = format!("Sub-agent {kind} {status}");
    if let Some(task) = field(value, "task").or_else(|| field(value, "description")) {
        let _ = write!(text, ": {task}");
    }
    if let Some(summary) = field(value, "summary") {
        let _ = write!(text, "\n\n{summary}");
    }
    text
}

/// A stable position for a call without an id: a hash of its name and input.
fn tool_position(native: &str, args: &Value) -> u64 {
    let digest = Sha256::digest(format!("{native}\0{args}").as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes)
}

fn header(session: &str, source_id: String) -> EventHeader {
    EventHeader {
        harness: ID.to_owned(),
        session_id: session.to_owned(),
        source_id,
        at: 0,
    }
}

/// A string field, when it is present and not empty.
fn field(value: &Value, key: &str) -> Option<String> {
    raw_str(value, key).filter(|text| !text.is_empty())
}

/// A string field, empty or not.
fn raw_str(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn payload_digest(payload: &[u8]) -> String {
    let mut out = String::with_capacity(16);
    for byte in &Sha256::digest(payload)[..8] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::threads::harness::conformance;

    fn fixtures() -> &'static Path {
        Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/threads/cursor"
        ))
    }

    #[test]
    fn cursor_adapter_conforms() {
        conformance::run(&CursorAdapter, fixtures());
    }

    /// Writes the expected JSON of every fixture. Run once after changing the parser, then
    /// review the diff by eye: `BLESS=1 cargo test -p gm-core bless_cursor -- --ignored`.
    #[test]
    #[ignore = "writes the golden files; run with BLESS=1"]
    fn bless_cursor_fixtures() {
        if std::env::var("BLESS").as_deref() != Ok("1") {
            return;
        }
        conformance::bless(&CursorAdapter, fixtures());
    }

    fn parse(value: &Value) -> HookInput {
        parse_hook(value.to_string().as_bytes()).expect("parses")
    }

    #[test]
    fn the_working_directory_is_cwd_else_the_first_workspace_root() {
        let adapter = CursorAdapter;
        let with_cwd = json!({"cwd": "/w/sub", "workspace_roots": ["/w"]}).to_string();
        assert_eq!(
            adapter.hook_cwd(with_cwd.as_bytes()),
            Some(PathBuf::from("/w/sub"))
        );
        let roots = json!({"workspace_roots": ["", "/a", "/b"]}).to_string();
        assert_eq!(
            adapter.hook_cwd(roots.as_bytes()),
            Some(PathBuf::from("/a"))
        );
        assert_eq!(adapter.hook_cwd(b"{}"), None);
        assert_eq!(adapter.hook_cwd(b"not json"), None);
    }

    #[test]
    fn the_hook_replies_with_an_empty_json_object() {
        let reply = CursorAdapter.hook_reply().expect("a reply");
        assert_eq!(
            serde_json::from_str::<Value>(reply).expect("JSON"),
            json!({})
        );
    }

    #[test]
    fn a_payload_without_a_conversation_is_an_error() {
        let err = parse_hook(br#"{"hook_event_name":"stop"}"#).expect_err("no session");
        assert!(err.0.contains("conversation_id"), "{err}");
        assert!(parse_hook(b"[]").is_err());
    }

    #[test]
    fn two_replies_in_one_generation_get_two_ids_and_a_replay_the_same_one() {
        let first = json!({"conversation_id": "c", "generation_id": "g",
            "hook_event_name": "afterAgentResponse", "text": "one"});
        let second = json!({"conversation_id": "c", "generation_id": "g",
            "hook_event_name": "afterAgentResponse", "text": "two"});
        let id = |input: &HookInput| input.events[0].header().source_id.clone();
        assert_ne!(id(&parse(&first)), id(&parse(&second)));
        assert_eq!(id(&parse(&first)), id(&parse(&first)));
    }

    #[test]
    fn a_call_without_an_id_agrees_between_pre_and_post() {
        let pre = json!({"conversation_id": "c", "hook_event_name": "preToolUse",
            "tool_name": "Shell", "tool_input": {"command": "ls"}});
        let post = json!({"conversation_id": "c", "hook_event_name": "postToolUse",
            "tool_name": "Shell", "tool_input": {"command": "ls"}, "tool_output": "{}"});
        assert_eq!(
            parse(&pre).events[0].header().source_id,
            parse(&post).events[0].header().source_id
        );
    }

    #[test]
    fn tool_output_is_unwrapped_from_its_json_string() {
        assert_eq!(output_text(Some(&json!("{\"output\":\"hi\"}"))), "hi");
        assert_eq!(output_text(Some(&json!("plain"))), "plain");
        assert_eq!(output_text(Some(&json!("[1,2]"))), "[1,2]");
        assert_eq!(output_text(Some(&json!("{\"a\":1}"))), "{\"a\":1}");
        assert_eq!(output_text(None), "");
    }

    #[test]
    fn a_write_with_old_text_is_an_edit_and_a_delete_has_no_text() {
        let edit = json!({"conversation_id": "c", "hook_event_name": "preToolUse",
            "tool_name": "Write", "tool_use_id": "t",
            "tool_input": {"file_path": "/w/a.rs", "old_string": "a", "new_string": "b"}});
        let CanonicalEvent::ToolCalled { tool, .. } = &parse(&edit).events[0] else {
            panic!("a call");
        };
        assert_eq!(*tool, ToolKind::Edit);

        let delete = json!({"conversation_id": "c", "hook_event_name": "preToolUse",
            "tool_name": "Delete", "tool_use_id": "t", "tool_input": {"path": "/w/a.rs"}});
        let CanonicalEvent::ToolCalled { tool, edit, .. } = &parse(&delete).events[0] else {
            panic!("a call");
        };
        assert_eq!(*tool, ToolKind::Write);
        let edit = edit.as_ref().expect("an edit");
        assert_eq!(
            (edit.change, edit.new_text.as_deref()),
            (EditChange::Delete, None)
        );
    }
}
