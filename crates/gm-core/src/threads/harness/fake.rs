//! A harness for tests: no agent, one JSON object per hook call and one per log line.
//!
//! Object fields: `event`, `session` (required), `id` (the item's `source_id`, required on
//! events that produce one), `at` (ms), `text`, `cwd`, `tool`, `args`, `call` (the call a
//! result answers), `error`, `path` and `parent` (the parent call of a sub-agent). A
//! `tool_call`'s `id` is the call id that its `tool_result` names in `call`.

use std::path::Path;

use gm_schema::threads::{
    CanonicalEvent, EditChange, EditIntent, EditPhase, EventHeader, SessionOrigin, ToolKind,
};
use serde_json::{Map, Value};

use super::{
    Capabilities, EditPhaseInfo, HarnessAdapter, HookInput, LogChunk, MAX_LOG_READ, ParseError,
    SessionLog, read_complete_lines,
};

/// The harness id of the fake.
pub const ID: &str = "fake";

/// The test harness. It is not in the registry: tests pass it to the conformance kit directly.
#[derive(Clone, Copy, Debug, Default)]
pub struct FakeAdapter;

impl HarnessAdapter for FakeAdapter {
    fn id(&self) -> &'static str {
        ID
    }

    fn display_name(&self) -> &'static str {
        "Fake harness (tests)"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            session_log: true,
            pre_edit_event: true,
            tool_results: true,
            subagents: true,
            resume_same_id: true,
        }
    }

    fn parse_hook(&self, payload: &[u8]) -> Result<HookInput, ParseError> {
        let record = decode(payload)?;
        let mut batch = Batch::default();
        let session_id = record.session_id.clone();
        let cwd = record.cwd.clone();
        batch.add(record);
        Ok(HookInput {
            session_id,
            cwd,
            log_path: None,
            events: batch.events,
            edit_phase: batch.edit_phase,
            unrecognized: batch.unrecognized,
        })
    }

    fn read_session_log(&self, path: &Path, from: u64) -> Result<LogChunk, ParseError> {
        let (lines, next_offset) = read_complete_lines(path, from, MAX_LOG_READ)?;
        let mut batch = Batch::default();
        for (start, line) in lines {
            let record = decode(line.as_bytes())
                .map_err(|err| ParseError(format!("line at byte {start}: {err}")))?;
            batch.add(record);
        }
        Ok(LogChunk {
            events: batch.events,
            next_offset,
            unrecognized: batch.unrecognized,
        })
    }

    fn discover(&self, _worktree: &Path) -> Vec<SessionLog> {
        Vec::new()
    }
}

/// One decoded object: its session, its cwd and what it means.
struct Record {
    session_id: String,
    cwd: Option<String>,
    outcome: Outcome,
}

enum Outcome {
    Event(Box<CanonicalEvent>),
    Edit(EditPhaseInfo),
    Unrecognized(String),
}

/// Everything a hook or a log chunk produced, in order.
#[derive(Default)]
struct Batch {
    events: Vec<CanonicalEvent>,
    edit_phase: Option<EditPhaseInfo>,
    unrecognized: Vec<String>,
}

impl Batch {
    fn add(&mut self, record: Record) {
        match record.outcome {
            Outcome::Event(event) => self.events.push(*event),
            Outcome::Edit(info) => self.edit_phase = Some(info),
            Outcome::Unrecognized(name) => self.unrecognized.push(name),
        }
    }
}

fn decode(bytes: &[u8]) -> Result<Record, ParseError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|err| ParseError(format!("payload is not JSON: {err}")))?;
    let Value::Object(object) = value else {
        return Err(ParseError("payload is not a JSON object".to_owned()));
    };
    let event = req_str(&object, "event")?;
    let session_id = req_str(&object, "session")?;
    let cwd = opt_str(&object, "cwd");
    let outcome = match event.as_str() {
        "edit_before" | "edit_after" => {
            let phase = if event == "edit_before" {
                EditPhase::Before
            } else {
                EditPhase::After
            };
            Outcome::Edit(EditPhaseInfo {
                phase,
                call_source_id: req_str(&object, "call")?,
                path: req_str(&object, "path")?,
            })
        }
        "session_start" | "prompt" | "assistant" | "tool_call" | "tool_result" | "turn_end"
        | "session_end" | "notice" => {
            let header = header(&object, &session_id)?;
            Outcome::Event(Box::new(known_event(
                &event,
                header,
                &object,
                cwd.as_deref(),
            )?))
        }
        other => Outcome::Unrecognized(other.to_owned()),
    };
    Ok(Record {
        session_id,
        cwd,
        outcome,
    })
}

fn known_event(
    event: &str,
    header: EventHeader,
    object: &Map<String, Value>,
    cwd: Option<&str>,
) -> Result<CanonicalEvent, ParseError> {
    Ok(match event {
        "session_start" => CanonicalEvent::SessionStarted {
            header,
            cwd: cwd
                .ok_or_else(|| ParseError("session_start has no cwd".to_owned()))?
                .to_owned(),
            model: None,
            harness_version: None,
            log_path: None,
            origin: SessionOrigin::New,
        },
        "prompt" => CanonicalEvent::PromptSubmitted {
            header,
            text: text(object),
        },
        "assistant" => CanonicalEvent::AssistantMessage {
            header,
            text: text(object),
            model: None,
        },
        "tool_call" => {
            let native_tool = req_str(object, "tool")?;
            let tool = tool_kind(&native_tool);
            let args = object.get("args").cloned().unwrap_or(Value::Null);
            let edit = edit_intent(tool, &args, opt_str(object, "path").as_deref());
            CanonicalEvent::ToolCalled {
                header,
                tool,
                native_tool,
                args,
                parent_call: opt_str(object, "parent"),
                edit,
            }
        }
        "tool_result" => CanonicalEvent::ToolReturned {
            header,
            call_source_id: req_str(object, "call")?,
            text: text(object),
            is_error: object
                .get("error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "turn_end" => CanonicalEvent::TurnEnded {
            header,
            parent_call: opt_str(object, "parent"),
        },
        "session_end" => CanonicalEvent::SessionEnded {
            header,
            reason: text(object),
        },
        _ => CanonicalEvent::Notice {
            header,
            text: text(object),
        },
    })
}

fn header(object: &Map<String, Value>, session_id: &str) -> Result<EventHeader, ParseError> {
    let at = object
        .get("at")
        .and_then(Value::as_i64)
        .ok_or_else(|| ParseError("field `at` must be an integer".to_owned()))?;
    Ok(EventHeader {
        harness: ID.to_owned(),
        session_id: session_id.to_owned(),
        source_id: req_str(object, "id")?,
        at,
    })
}

/// The canonical class of a native tool name. Unknown names are `Other`, with the name kept.
fn tool_kind(native: &str) -> ToolKind {
    match native {
        "read" => ToolKind::Read,
        "edit" => ToolKind::Edit,
        "write" => ToolKind::Write,
        "bash" => ToolKind::Shell,
        "grep" => ToolKind::Search,
        "web" => ToolKind::Web,
        "task" => ToolKind::Agent,
        _ => ToolKind::Other,
    }
}

/// The edit intent of an `edit` or `write` call: `args.path` (or the top-level `path`), with
/// `args.old` and `args.new` as the replaced and written text.
fn edit_intent(tool: ToolKind, args: &Value, fallback_path: Option<&str>) -> Option<EditIntent> {
    let change = match tool {
        ToolKind::Edit => EditChange::Modify,
        ToolKind::Write => EditChange::Create,
        _ => return None,
    };
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| fallback_path.map(str::to_owned))?;
    Some(EditIntent {
        path,
        change,
        old_path: None,
        old_text: args.get("old").and_then(Value::as_str).map(str::to_owned),
        new_text: args.get("new").and_then(Value::as_str).map(str::to_owned),
    })
}

fn req_str(object: &Map<String, Value>, key: &str) -> Result<String, ParseError> {
    match object.get(key).and_then(Value::as_str) {
        Some(value) if !value.is_empty() => Ok(value.to_owned()),
        _ => Err(ParseError(format!(
            "field `{key}` must be a non-empty string"
        ))),
    }
}

fn opt_str(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn text(object: &Map<String, Value>) -> String {
    object
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads::harness::conformance;

    const AT: i64 = 1_700_000_000_000;

    #[test]
    fn fake_adapter_conforms() {
        conformance::run(
            &FakeAdapter,
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/threads/fake"
            )),
        );
    }

    /// Writes the expected JSON of every fixture. Run once after changing the parser, then
    /// review the diff by eye: `BLESS=1 cargo test -p gm-core bless_fake -- --ignored`.
    #[test]
    #[ignore = "writes the golden files; run with BLESS=1"]
    fn bless_fake_fixtures() {
        if std::env::var("BLESS").as_deref() != Ok("1") {
            return;
        }
        conformance::bless(
            &FakeAdapter,
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/threads/fake"
            )),
        );
    }

    fn hook(json: &str) -> Result<HookInput, ParseError> {
        FakeAdapter.parse_hook(json.as_bytes())
    }

    #[test]
    fn invalid_json_and_non_objects_are_errors() {
        assert!(hook("{not json").is_err());
        assert!(hook("[1, 2]").is_err());
        assert!(hook(r#"{"event":"prompt","session":"s1","id":"e1","at":1}"#).is_ok());
    }

    #[test]
    fn a_missing_or_empty_session_is_an_error() {
        assert!(hook(r#"{"event":"prompt","id":"e1","at":1}"#).is_err());
        assert!(hook(r#"{"event":"prompt","session":"","id":"e1","at":1}"#).is_err());
    }

    #[test]
    fn an_unknown_event_is_recorded_not_an_error() {
        let input = hook(r#"{"event":"mystery","session":"s1","id":"m1","at":1}"#)
            .expect("unknown events parse");
        assert!(input.events.is_empty());
        assert_eq!(input.unrecognized, vec!["mystery".to_owned()]);
        assert_eq!(input.session_id, "s1");
    }

    #[test]
    fn tool_names_map_to_kinds_and_unknown_names_to_other() {
        let input = hook(
            r#"{"event":"tool_call","session":"s1","id":"c1","at":1,"tool":"custom","args":{}}"#,
        )
        .expect("parse");
        match &input.events[0] {
            CanonicalEvent::ToolCalled {
                tool, native_tool, ..
            } => {
                assert_eq!(*tool, ToolKind::Other);
                assert_eq!(native_tool, "custom");
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    #[test]
    fn an_edit_call_carries_its_intent_and_a_read_does_not() {
        let edit = hook(
            r#"{"event":"tool_call","session":"s1","id":"c1","at":1,"tool":"edit","args":{"path":"a.txt","old":"x","new":"y"}}"#,
        )
        .expect("parse");
        let CanonicalEvent::ToolCalled { edit, .. } = &edit.events[0] else {
            panic!("not a tool call");
        };
        assert_eq!(
            edit.clone(),
            Some(EditIntent {
                path: "a.txt".to_owned(),
                change: EditChange::Modify,
                old_path: None,
                old_text: Some("x".to_owned()),
                new_text: Some("y".to_owned()),
            })
        );

        let read = hook(
            r#"{"event":"tool_call","session":"s1","id":"c2","at":1,"tool":"read","args":{"path":"a.txt"}}"#,
        )
        .expect("parse");
        let CanonicalEvent::ToolCalled { edit, .. } = &read.events[0] else {
            panic!("not a tool call");
        };
        assert!(edit.is_none());
    }

    #[test]
    fn an_edit_phase_sets_edit_phase_and_no_event() {
        let input = hook(r#"{"event":"edit_before","session":"s1","call":"c1","path":"a.txt"}"#)
            .expect("parse");
        assert!(input.events.is_empty());
        assert_eq!(
            input.edit_phase,
            Some(EditPhaseInfo {
                phase: EditPhase::Before,
                call_source_id: "c1".to_owned(),
                path: "a.txt".to_owned(),
            })
        );
    }

    #[test]
    fn a_log_reads_complete_lines_and_leaves_a_partial_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.log");
        let line = format!(r#"{{"event":"prompt","session":"s1","id":"p1","at":{AT}}}"#);
        std::fs::write(&path, format!("{line}\n{{\"event\":\"prom")).expect("write");
        let chunk = FakeAdapter.read_session_log(&path, 0).expect("read");
        assert_eq!(chunk.events.len(), 1);
        assert_eq!(
            chunk.next_offset,
            u64::try_from(line.len() + 1).expect("fits")
        );
    }

    #[test]
    fn a_bad_log_line_is_an_error_with_its_offset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.log");
        std::fs::write(&path, "{broken\n").expect("write");
        let err = FakeAdapter
            .read_session_log(&path, 0)
            .expect_err("bad line");
        assert!(err.0.contains("byte 0"), "{err}");
    }

    #[test]
    fn discover_finds_nothing() {
        assert!(FakeAdapter.discover(Path::new("/nowhere")).is_empty());
    }
}
