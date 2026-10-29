//! The Claude Code harness (D129, Q26).
//!
//! Every format in this module comes from the Claude Code documentation and is UNVERIFIED
//! against a real session: the hook payloads, the transcript lines and the plugin layout.
//! Verifying them is open question Q26 in `design/threads.md`. The parser is lenient: a
//! missing optional field never fails a payload, unknown fields are ignored, and an unknown
//! event or record is reported in `unrecognized`, not as an error.
//!
//! The integration route (a local marketplace holding a plugin whose `hooks.json` runs
//! `gitmaster thread hook`, added and installed with `claude plugin`) is unverified too.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use gm_schema::threads::{
    CanonicalEvent, EditChange, EditIntent, EditPhase, EventHeader, SessionOrigin, ToolKind,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    Capabilities, EditPhaseInfo, HarnessAdapter, HookInput, InstallContext, Integration, LogChunk,
    MAX_LOG_READ, ParseError, SessionLog, read_complete_lines, stable_source_id,
};

/// The harness id, stored with every thread.
pub const ID: &str = "claude-code";

/// The plugin and the local marketplace the integration installs under.
const PLUGIN: &str = "gitmaster";
const MARKETPLACE: &str = "gitmaster-local";

/// Seconds a hook may run before Claude Code gives up on it.
const HOOK_TIMEOUT_SECS: u64 = 5;

/// The Claude Code adapter. Stateless.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClaudeCodeAdapter;

impl HarnessAdapter for ClaudeCodeAdapter {
    fn id(&self) -> &'static str {
        ID
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            session_log: true,
            pre_edit_event: true,
            tool_results: true,
            subagents: true,
            resume_same_id: true,
            plugin_install: true,
        }
    }

    fn parse_hook(&self, payload: &[u8]) -> Result<HookInput, ParseError> {
        parse_hook(payload)
    }

    fn read_session_log(&self, path: &Path, from: u64) -> Result<LogChunk, ParseError> {
        read_transcript(path, from)
    }

    fn integration(&self, ctx: &InstallContext) -> Integration {
        integration(ctx)
    }

    fn discover(&self, worktree: &Path) -> Vec<SessionLog> {
        let Some(home) = std::env::var_os("HOME") else {
            return Vec::new();
        };
        discover_in(Path::new(&home), worktree)
    }
}

/// One hook invocation. Hook payloads carry no time, so every event gets `at` 0; the pipeline
/// stamps it.
///
/// Common fields are `session_id`, `transcript_path`, `cwd` and `hook_event_name`. A
/// `UserPromptSubmit` yields no event: the prompt comes from the transcript, so recording it
/// here too would store every prompt twice.
fn parse_hook(payload: &[u8]) -> Result<HookInput, ParseError> {
    let value: Value = serde_json::from_slice(payload)
        .map_err(|err| ParseError(format!("payload is not JSON: {err}")))?;
    if !value.is_object() {
        return Err(ParseError("payload is not a JSON object".to_owned()));
    }
    let session = field(&value, "session_id")
        .ok_or_else(|| ParseError("payload has no session_id".to_owned()))?;
    let event = field(&value, "hook_event_name")
        .ok_or_else(|| ParseError("payload has no hook_event_name".to_owned()))?;
    let mut input = HookInput {
        session_id: session.clone(),
        cwd: field(&value, "cwd"),
        log_path: field(&value, "transcript_path"),
        ..HookInput::default()
    };

    match event.as_str() {
        "SessionStart" => input.events.push(session_started(&value, &session)),
        "UserPromptSubmit" => {}
        "PreToolUse" => {
            let call = hook_tool(&value, &session);
            input.edit_phase = call.edit_phase(EditPhase::Before);
            input.events.push(call.event(&session, 0));
        }
        "PostToolUse" => {
            // The call is repeated so a post without its pre still creates the call row.
            let call = hook_tool(&value, &session);
            input.edit_phase = call.edit_phase(EditPhase::After);
            input.events.push(call.event(&session, 0));
            let response = value.get("tool_response");
            input.events.push(CanonicalEvent::ToolReturned {
                header: header(&session, format!("{}:result", call.source_id), 0),
                call_source_id: call.source_id.clone(),
                text: response_text(response),
                is_error: response_is_error(response),
            });
        }
        "Stop" => input.events.push(CanonicalEvent::TurnEnded {
            header: header(&session, format!("stop:{}", payload_digest(payload)), 0),
            parent_call: None,
        }),
        "SubagentStop" => input.events.push(CanonicalEvent::TurnEnded {
            header: header(&session, format!("substop:{}", payload_digest(payload)), 0),
            parent_call: Some(session.clone()),
        }),
        "SessionEnd" => {
            let reason = field(&value, "reason").unwrap_or_else(|| "unknown".to_owned());
            input.events.push(CanonicalEvent::SessionEnded {
                header: header(&session, format!("session-end:{reason}"), 0),
                reason,
            });
        }
        other => input.unrecognized.push(other.to_owned()),
    }
    Ok(input)
}

fn session_started(value: &Value, session: &str) -> CanonicalEvent {
    let source = field(value, "source");
    let origin = match source.as_deref() {
        Some("startup") => SessionOrigin::New,
        Some("resume") => SessionOrigin::Resumed,
        Some("clear") => SessionOrigin::Cleared,
        Some("compact") => SessionOrigin::Compacted,
        _ => SessionOrigin::Unknown,
    };
    CanonicalEvent::SessionStarted {
        header: header(
            session,
            format!("session-start:{}", source.as_deref().unwrap_or("unknown")),
            0,
        ),
        cwd: field(value, "cwd").unwrap_or_default(),
        model: field(value, "model"),
        harness_version: None,
        log_path: field(value, "transcript_path"),
        origin,
    }
}

/// A tool call as either a hook or a transcript block names it. Native names are the
/// documented Claude Code ones.
struct ToolUse {
    source_id: String,
    native: String,
    tool: ToolKind,
    args: Value,
    edit: Option<EditIntent>,
}

impl ToolUse {
    fn new(source_id: String, native: String, args: Value) -> Self {
        let tool = tool_kind(&native);
        let edit = edit_intent(&native, &args);
        Self {
            source_id,
            native,
            tool,
            args,
            edit,
        }
    }

    /// The edit phase of this call, when it edits a file.
    fn edit_phase(&self, phase: EditPhase) -> Option<EditPhaseInfo> {
        self.edit.as_ref().map(|edit| EditPhaseInfo {
            phase,
            call_source_id: self.source_id.clone(),
            path: edit.path.clone(),
        })
    }

    fn event(&self, session: &str, at: i64) -> CanonicalEvent {
        CanonicalEvent::ToolCalled {
            header: header(session, self.source_id.clone(), at),
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
    let args = value.get("tool_input").cloned().unwrap_or(Value::Null);
    let source_id = field(value, "tool_use_id")
        .unwrap_or_else(|| stable_source_id(ID, session, tool_position(&native, &args)));
    ToolUse::new(source_id, native, args)
}

/// The canonical class of a native tool name. Unknown names, including `mcp__` tools, are
/// `Other` with the name kept.
fn tool_kind(native: &str) -> ToolKind {
    match native {
        "Read" => ToolKind::Read,
        "Edit" | "MultiEdit" | "NotebookEdit" => ToolKind::Edit,
        "Write" => ToolKind::Write,
        "Bash" => ToolKind::Shell,
        "Grep" | "Glob" => ToolKind::Search,
        "WebFetch" | "WebSearch" => ToolKind::Web,
        "Task" => ToolKind::Agent,
        _ => ToolKind::Other,
    }
}

/// The edit a file-changing tool makes, from its input. `None` for other tools, or when the
/// input names no file.
fn edit_intent(native: &str, args: &Value) -> Option<EditIntent> {
    let (old_text, new_text) = match native {
        "Edit" => (raw_str(args, "old_string"), raw_str(args, "new_string")),
        "Write" => (None, raw_str(args, "content")),
        "MultiEdit" | "NotebookEdit" => (None, None),
        _ => return None,
    };
    let path = field(args, "file_path").or_else(|| field(args, "notebook_path"))?;
    Some(EditIntent {
        path,
        change: EditChange::Modify,
        old_path: None,
        old_text,
        new_text,
    })
}

/// The text of a tool result. A string is itself; an object with `stdout` is the output
/// followed by a newline and `stderr`, when there is stderr; anything else is its compact JSON.
fn response_text(response: Option<&Value>) -> String {
    match response {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(value) => match raw_str(value, "stdout") {
            Some(stdout) => match (stdout.is_empty(), field(value, "stderr")) {
                (true, Some(stderr)) => stderr,
                (false, Some(stderr)) => format!("{stdout}\n{stderr}"),
                _ => stdout,
            },
            None => value.to_string(),
        },
    }
}

fn response_is_error(response: Option<&Value>) -> bool {
    response.is_some_and(|value| flag(value, "is_error") || present(value, "error"))
}

/// The plugin files and install steps. Unverified (Q26): the marketplace and plugin layout
/// follow the documented `claude plugin` format, not a session run.
fn integration(ctx: &InstallContext) -> Integration {
    let marketplace = ctx.data_dir.join("agents").join(ID).join("marketplace");
    let command = format!(
        "{} thread hook --harness {ID}",
        shell_quote(&ctx.binary.to_string_lossy())
    );
    Integration {
        files: vec![
            (
                PathBuf::from("marketplace/.claude-plugin/marketplace.json"),
                pretty(&marketplace_manifest()),
            ),
            (
                PathBuf::from("marketplace/plugin/.claude-plugin/plugin.json"),
                pretty(&plugin_manifest()),
            ),
            (
                PathBuf::from("marketplace/plugin/hooks/hooks.json"),
                pretty(&hooks_file(&command)),
            ),
        ],
        steps: vec![
            format!(
                "claude plugin marketplace add {}",
                shell_quote(&marketplace.to_string_lossy())
            ),
            format!("claude plugin install {PLUGIN}@{MARKETPLACE}"),
        ],
        uninstall_steps: vec![
            format!("claude plugin uninstall {PLUGIN}@{MARKETPLACE}"),
            format!("claude plugin marketplace remove {MARKETPLACE}"),
        ],
    }
}

fn marketplace_manifest() -> Value {
    json!({
        "name": MARKETPLACE,
        "owner": {"name": "GitMaster"},
        "plugins": [{
            "name": PLUGIN,
            "source": "./plugin",
            "description": "Captures agent sessions as GitMaster threads",
        }],
    })
}

fn plugin_manifest() -> Value {
    json!({
        "name": PLUGIN,
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Captures agent sessions as GitMaster threads",
    })
}

/// One command hook per event. Only the tool events carry a matcher.
fn hooks_file(command: &str) -> Value {
    let hook = json!([{
        "type": "command",
        "command": command,
        "timeout": HOOK_TIMEOUT_SECS,
    }]);
    json!({"hooks": {
        "SessionStart": [{"hooks": hook.clone()}],
        "UserPromptSubmit": [{"hooks": hook.clone()}],
        "PreToolUse": [{"matcher": "Edit|MultiEdit|Write|NotebookEdit", "hooks": hook.clone()}],
        "PostToolUse": [{"matcher": "*", "hooks": hook.clone()}],
        "Stop": [{"hooks": hook.clone()}],
        "SubagentStop": [{"hooks": hook.clone()}],
        "SessionEnd": [{"hooks": hook}],
    }})
}

fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text
}

/// Single-quotes a path for the shell, escaping any single quote inside it.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The transcripts of a worktree: `$HOME/.claude/projects/<path with non-alphanumerics as
/// '-'>/*.jsonl`, by file name. Missing directories give nothing.
fn discover_in(home: &Path, worktree: &Path) -> Vec<SessionLog> {
    let dir = home
        .join(".claude")
        .join("projects")
        .join(project_dir_name(worktree));
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut logs: Vec<SessionLog> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .filter(|path| path.is_file())
        .filter_map(|path| {
            let session_id = path.file_stem()?.to_str()?.to_owned();
            Some(SessionLog { path, session_id })
        })
        .collect();
    logs.sort_by(|a, b| a.path.cmp(&b.path));
    logs
}

fn project_dir_name(worktree: &Path) -> String {
    worktree
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Complete records of a transcript from `from`. Each line is one record; a record's session
/// is its own `sessionId`, or the file stem when a line has none, since a read that starts
/// mid-file never sees the first line. Lines and block types the adapter does not read are
/// named in `unrecognized`.
fn read_transcript(path: &Path, from: u64) -> Result<LogChunk, ParseError> {
    let (lines, next_offset) = read_complete_lines(path, from, MAX_LOG_READ)?;
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    let mut chunk = LogChunk {
        events: Vec::new(),
        next_offset,
        unrecognized: Vec::new(),
    };
    for (offset, line) in lines {
        transcript_line(&line, offset, stem, &mut chunk);
    }
    Ok(chunk)
}

/// What every event of one transcript record shares.
struct Record {
    session: String,
    at: i64,
    /// The record's `uuid`, when it has one.
    uuid: Option<String>,
    /// `uuid`, or a hash of the line position when the record has none.
    base: String,
    model: Option<String>,
}

impl Record {
    fn header(&self, source_id: String) -> EventHeader {
        header(&self.session, source_id, self.at)
    }
}

fn transcript_line(line: &str, offset: u64, fallback_session: &str, out: &mut LogChunk) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        out.unrecognized.push("invalid-json".to_owned());
        return;
    };
    let Some(kind) = field(&value, "type") else {
        out.unrecognized.push("invalid-json".to_owned());
        return;
    };
    let session = field(&value, "sessionId").unwrap_or_else(|| fallback_session.to_owned());
    let uuid = field(&value, "uuid");
    let record = Record {
        at: value
            .get("timestamp")
            .and_then(Value::as_str)
            .map_or(0, parse_timestamp_ms),
        base: uuid
            .clone()
            .unwrap_or_else(|| stable_source_id(ID, &session, offset)),
        model: value
            .get("message")
            .and_then(|message| field(message, "model")),
        session,
        uuid,
    };
    let content = value
        .get("message")
        .and_then(|message| message.get("content"));

    match kind.as_str() {
        "user" => {
            if !flag(&value, "isMeta") {
                user_message(&record, content, out);
            }
        }
        "assistant" => assistant_message(&record, content, out),
        "summary" | "compaction" => {
            let source = record
                .uuid
                .clone()
                .unwrap_or_else(|| format!("summary:{offset}"));
            out.events.push(CanonicalEvent::Notice {
                header: record.header(source),
                text: field(&value, "summary").unwrap_or_default(),
            });
        }
        other => out.unrecognized.push(other.to_owned()),
    }
}

fn user_message(record: &Record, content: Option<&Value>, out: &mut LogChunk) {
    match content {
        Some(Value::String(text)) if !text.is_empty() => {
            out.events.push(prompt(record, text.clone()));
        }
        Some(Value::Array(blocks)) => {
            let mut texts = Vec::new();
            for block in blocks {
                match block_type(block) {
                    Some("tool_result") => tool_result(record, block, out),
                    Some("text") => texts.extend(field(block, "text")),
                    None => {}
                    Some(other) => out.unrecognized.push(format!("block:{other}")),
                }
            }
            if !texts.is_empty() {
                out.events.push(prompt(record, texts.join("\n")));
            }
        }
        _ => {}
    }
}

fn prompt(record: &Record, text: String) -> CanonicalEvent {
    CanonicalEvent::PromptSubmitted {
        header: record.header(record.base.clone()),
        text,
    }
}

fn tool_result(record: &Record, block: &Value, out: &mut LogChunk) {
    let Some(call) = field(block, "tool_use_id") else {
        out.unrecognized
            .push("tool_result without tool_use_id".to_owned());
        return;
    };
    out.events.push(CanonicalEvent::ToolReturned {
        header: record.header(format!("{call}:result")),
        text: block_text(block.get("content")),
        is_error: flag(block, "is_error"),
        call_source_id: call,
    });
}

fn assistant_message(record: &Record, content: Option<&Value>, out: &mut LogChunk) {
    match content {
        Some(Value::String(text)) if !text.is_empty() => {
            out.events.push(assistant_text(
                record,
                format!("{}:0", record.base),
                text.clone(),
            ));
        }
        Some(Value::Array(blocks)) => {
            for (index, block) in blocks.iter().enumerate() {
                match block_type(block) {
                    Some("text") => {
                        if let Some(text) = field(block, "text") {
                            let source = format!("{}:{index}", record.base);
                            out.events.push(assistant_text(record, source, text));
                        }
                    }
                    Some("tool_use") => tool_use(record, block, index, out),
                    // Thinking is not captured (Q31).
                    Some("thinking" | "redacted_thinking") | None => {}
                    Some(other) => out.unrecognized.push(format!("block:{other}")),
                }
            }
        }
        _ => {}
    }
}

fn assistant_text(record: &Record, source_id: String, text: String) -> CanonicalEvent {
    CanonicalEvent::AssistantMessage {
        header: record.header(source_id),
        text,
        model: record.model.clone(),
    }
}

fn tool_use(record: &Record, block: &Value, index: usize, out: &mut LogChunk) {
    let source_id = field(block, "id").unwrap_or_else(|| format!("{}:{index}", record.base));
    let native = field(block, "name").unwrap_or_else(|| "unknown".to_owned());
    let args = block.get("input").cloned().unwrap_or(Value::Null);
    out.events
        .push(ToolUse::new(source_id, native, args).event(&record.session, record.at));
}

/// The text of a transcript tool result: a string, or the text blocks of an array joined by
/// newlines.
fn block_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|block| block_type(block) == Some("text"))
            .filter_map(|block| raw_str(block, "text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn block_type(block: &Value) -> Option<&str> {
    block.get("type").and_then(Value::as_str)
}

fn header(session: &str, source_id: String, at: i64) -> EventHeader {
    EventHeader {
        harness: ID.to_owned(),
        session_id: session.to_owned(),
        source_id,
        at,
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

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool) == Some(true)
}

fn present(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(|field| !field.is_null())
}

/// Hex of the first 8 bytes of a digest of the tool name and input, as a position.
fn tool_position(native: &str, args: &Value) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(native.as_bytes());
    hasher.update([0u8]);
    hasher.update(args.to_string().as_bytes());
    let digest = hasher.finalize();
    let mut eight = [0u8; 8];
    eight.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(eight)
}

/// The first 16 hex digits of the SHA-256 of a whole payload.
fn payload_digest(payload: &[u8]) -> String {
    let mut full = hex(&Sha256::digest(payload));
    full.truncate(16);
    full
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Milliseconds since the Unix epoch of an RFC 3339 timestamp. Unparsable text gives 0.
fn parse_timestamp_ms(text: &str) -> i64 {
    parse_rfc3339(text).unwrap_or(0)
}

/// `YYYY-MM-DD` then `T` (or `t` or a space), `hh:mm:ss`, an optional fraction of which the
/// first three digits count, then `Z` or `+hh:mm` or `-hh:mm`. No zone is not accepted.
fn parse_rfc3339(text: &str) -> Option<i64> {
    let year = num(text.get(0..4)?)?;
    separator(text, 4, "-")?;
    let month = num(text.get(5..7)?)?;
    separator(text, 7, "-")?;
    let day = num(text.get(8..10)?)?;
    if !matches!(text.get(10..11)?, "T" | "t" | " ") {
        return None;
    }
    let hour = num(text.get(11..13)?)?;
    separator(text, 13, ":")?;
    let minute = num(text.get(14..16)?)?;
    separator(text, 16, ":")?;
    let second = num(text.get(17..19)?)?;
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }

    let mut rest = text.get(19..)?;
    let mut millis = 0_i64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let mut scale = 100_i64;
        for digit in fraction.bytes().take(digits.min(3)) {
            millis += i64::from(digit - b'0') * scale;
            scale /= 10;
        }
        rest = &fraction[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => offset_seconds(rest)?,
    };
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second - offset;
    Some(seconds * 1_000 + millis)
}

fn offset_seconds(text: &str) -> Option<i64> {
    if text.len() != 6 {
        return None;
    }
    let sign = match text.get(0..1)? {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let hours = num(text.get(1..3)?)?;
    separator(text, 3, ":")?;
    let minutes = num(text.get(4..6)?)?;
    Some(sign * (hours * 3_600 + minutes * 60))
}

fn separator(text: &str, at: usize, want: &str) -> Option<()> {
    (text.get(at..at + want.len())? == want).then_some(())
}

fn num(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::threads::harness::conformance;

    #[test]
    fn claude_code_adapter_conforms() {
        conformance::run(
            &ClaudeCodeAdapter,
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/threads/claude-code"
            )),
        );
    }

    /// Writes the expected JSON of every fixture. Run once after changing the parser, then
    /// review the diff by eye: `BLESS=1 cargo test -p gm-core bless_claude_code -- --ignored`.
    #[test]
    #[ignore = "writes the golden files; run with BLESS=1"]
    fn bless_claude_code_fixtures() {
        if std::env::var("BLESS").as_deref() != Ok("1") {
            return;
        }
        conformance::bless(
            &ClaudeCodeAdapter,
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/threads/claude-code"
            )),
        );
    }

    #[test]
    fn timestamps_become_epoch_milliseconds() {
        assert_eq!(parse_timestamp_ms("1970-01-01T00:00:00Z"), 0);
        assert_eq!(
            parse_timestamp_ms("2025-01-02T03:04:05.678Z"),
            1_735_787_045_678
        );
        assert_eq!(
            parse_timestamp_ms("2025-01-02T03:04:05Z"),
            1_735_787_045_000
        );
        assert_eq!(
            parse_timestamp_ms("2025-01-02T04:04:05+01:00"),
            1_735_787_045_000
        );
        assert_eq!(
            parse_timestamp_ms("2025-01-02T02:34:05.5-00:30"),
            1_735_787_045_500
        );
        // Digits past the third are cut, not rounded.
        assert_eq!(
            parse_timestamp_ms("2025-01-02T03:04:05.678912Z"),
            1_735_787_045_678
        );
        assert_eq!(
            parse_timestamp_ms("2024-02-29T00:00:00Z"),
            1_709_164_800_000
        );
    }

    #[test]
    fn unparsable_timestamps_are_zero() {
        for text in [
            "",
            "yesterday",
            "2025-01-02T03:04:05",
            "2025-02-30T00:00:00Z",
            "2025-01-02T03:04:05.Z",
            "2025-01-02T25:00:00Z",
            "2025-01-02T03:04:05+0100",
        ] {
            assert_eq!(parse_timestamp_ms(text), 0, "{text}");
        }
    }

    #[test]
    fn a_payload_without_a_session_or_event_name_is_an_error() {
        assert!(parse_hook(br#"{"hook_event_name":"Stop"}"#).is_err());
        assert!(parse_hook(br#"{"session_id":"s1"}"#).is_err());
        assert!(parse_hook(b"[]").is_err());
        assert!(parse_hook(b"{nope").is_err());
    }

    #[test]
    fn a_prompt_hook_sets_the_session_but_yields_no_event() {
        let input = parse_hook(
            br#"{"session_id":"s1","cwd":"/r","transcript_path":"/t.jsonl","hook_event_name":"UserPromptSubmit","prompt":"hi"}"#,
        )
        .expect("parse");
        assert!(input.events.is_empty());
        assert!(input.unrecognized.is_empty());
        assert_eq!(input.session_id, "s1");
        assert_eq!(input.cwd.as_deref(), Some("/r"));
        assert_eq!(input.log_path.as_deref(), Some("/t.jsonl"));
    }

    #[test]
    fn integration_writes_the_marketplace_plugin_and_hooks() {
        let ctx = InstallContext {
            binary: PathBuf::from("/opt/it's/gitmaster"),
            data_dir: PathBuf::from("/data"),
        };
        let integration = ClaudeCodeAdapter.integration(&ctx);
        let file = |name: &str| -> Value {
            let (_, text) = integration
                .files
                .iter()
                .find(|(path, _)| path == Path::new(name))
                .unwrap_or_else(|| panic!("no file {name}"));
            serde_json::from_str(text).expect("generated JSON")
        };

        assert_eq!(
            file("marketplace/.claude-plugin/marketplace.json"),
            json!({
                "name": "gitmaster-local",
                "owner": {"name": "GitMaster"},
                "plugins": [{
                    "name": "gitmaster",
                    "source": "./plugin",
                    "description": "Captures agent sessions as GitMaster threads",
                }],
            })
        );
        assert_eq!(
            file("marketplace/plugin/.claude-plugin/plugin.json")["version"],
            env!("CARGO_PKG_VERSION")
        );

        let hooks = file("marketplace/plugin/hooks/hooks.json");
        let mut events: Vec<&str> = hooks["hooks"]
            .as_object()
            .expect("hooks object")
            .keys()
            .map(String::as_str)
            .collect();
        events.sort_unstable();
        assert_eq!(
            events,
            [
                "PostToolUse",
                "PreToolUse",
                "SessionEnd",
                "SessionStart",
                "Stop",
                "SubagentStop",
                "UserPromptSubmit",
            ]
        );
        let command = "'/opt/it'\\''s/gitmaster' thread hook --harness claude-code";
        for event in events {
            let group = &hooks["hooks"][event][0];
            assert_eq!(group["hooks"][0]["command"], command, "{event}");
            assert_eq!(group["hooks"][0]["timeout"], 5, "{event}");
        }
        assert_eq!(
            hooks["hooks"]["PreToolUse"][0]["matcher"],
            "Edit|MultiEdit|Write|NotebookEdit"
        );
        assert_eq!(hooks["hooks"]["PostToolUse"][0]["matcher"], "*");
        assert!(hooks["hooks"]["Stop"][0].get("matcher").is_none());

        assert_eq!(
            integration.steps,
            vec![
                "claude plugin marketplace add '/data/agents/claude-code/marketplace'".to_owned(),
                "claude plugin install gitmaster@gitmaster-local".to_owned(),
            ]
        );
        assert_eq!(
            integration.uninstall_steps,
            vec![
                "claude plugin uninstall gitmaster@gitmaster-local".to_owned(),
                "claude plugin marketplace remove gitmaster-local".to_owned(),
            ]
        );
    }

    #[test]
    fn discover_lists_the_worktree_transcripts_in_name_order() {
        let home = tempfile::tempdir().expect("tempdir");
        let project = home.path().join(".claude/projects/-work-my-repo-1");
        fs::create_dir_all(&project).expect("project dir");
        for name in ["b.jsonl", "a.jsonl", "notes.txt"] {
            fs::write(project.join(name), "").expect("write");
        }

        let logs = discover_in(home.path(), Path::new("/work/my.repo_1"));
        let ids: Vec<&str> = logs.iter().map(|log| log.session_id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(logs[0].path, project.join("a.jsonl"));

        assert!(discover_in(home.path(), Path::new("/elsewhere")).is_empty());
    }
}
