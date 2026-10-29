//! Conformance kit (design/threads.md, "Conformance kit"). Every harness adapter runs through
//! `run` over its fixture directory before it is registered. Test-only.
//!
//! A fixture directory holds, per case, `NAME.hook` (a payload) with `NAME.hook.expected.json`
//! (its `Vec<CanonicalEvent>`) and an optional `NAME.hook.edit.json` (the expected
//! `EditPhaseInfo` as `{phase, call_source_id, path}`); and `NAME.log` (a session log) with
//! `NAME.log.expected.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use gm_schema::threads::{CanonicalEvent, EditPhase, ToolKind};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestCaseError, TestRunner};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{EditPhaseInfo, HarnessAdapter, HookInput, InstallContext, LogChunk};

/// The expected `EditPhaseInfo` of a hook fixture, as stored in `NAME.hook.edit.json`.
#[derive(Serialize, Deserialize)]
struct EditJson {
    phase: EditPhase,
    call_source_id: String,
    path: String,
}

impl EditJson {
    fn from_info(info: &EditPhaseInfo) -> Self {
        Self {
            phase: info.phase,
            call_source_id: info.call_source_id.clone(),
            path: info.path.clone(),
        }
    }

    fn into_info(self) -> EditPhaseInfo {
        EditPhaseInfo {
            phase: self.phase,
            call_source_id: self.call_source_id,
            path: self.path,
        }
    }
}

struct HookFixture {
    name: String,
    bytes: Vec<u8>,
    expected: Vec<CanonicalEvent>,
    edit: Option<EditPhaseInfo>,
}

struct LogFixture {
    name: String,
    bytes: Vec<u8>,
    expected: Vec<CanonicalEvent>,
}

/// What the fixtures showed the adapter do, for the capability checks.
#[derive(Default)]
struct Observed {
    before: bool,
    tool_results: bool,
    subagents: bool,
    /// Each native tool name must map to one `ToolKind` everywhere it appears.
    tools: BTreeMap<String, ToolKind>,
}

impl Observed {
    fn observe_edit(&mut self, edit: Option<&EditPhaseInfo>) {
        if edit.is_some_and(|info| info.phase == EditPhase::Before) {
            self.before = true;
        }
    }

    fn observe_events(&mut self, events: &[CanonicalEvent]) {
        for event in events {
            match event {
                CanonicalEvent::ToolCalled {
                    tool,
                    native_tool,
                    parent_call,
                    ..
                } => {
                    if let Some(previous) = self.tools.insert(native_tool.clone(), *tool) {
                        assert!(
                            previous == *tool,
                            "native tool `{native_tool}` maps to both {previous:?} and {tool:?}"
                        );
                    }
                    self.subagents |= parent_call.is_some();
                }
                CanonicalEvent::ToolReturned { .. } => self.tool_results = true,
                CanonicalEvent::TurnEnded { parent_call, .. } => {
                    self.subagents |= parent_call.is_some();
                }
                _ => {}
            }
        }
    }
}

/// Runs every check over the fixture directory `fixtures`.
///
/// # Panics
///
/// Panics with the first violated check: a golden mismatch, a broken generic property, an
/// honesty mismatch between the declared capabilities and the fixtures, or a missing file.
pub fn run(adapter: &dyn HarnessAdapter, fixtures: &Path) {
    let listing = list(fixtures);
    assert!(
        !(listing.hooks.is_empty() && listing.logs.is_empty()),
        "{}: no fixtures found",
        fixtures.display()
    );
    let hooks: Vec<HookFixture> = listing
        .hooks
        .iter()
        .map(|path| load_hook(fixtures, path))
        .collect();
    let logs: Vec<LogFixture> = listing
        .logs
        .iter()
        .map(|path| load_log(fixtures, path))
        .collect();

    let scratch = tempfile::tempdir().expect("scratch directory");
    let scratch_log = scratch.path().join("session.log");
    let mut observed = Observed::default();

    for hook in &hooks {
        let parsed = match check_hook(adapter, &hook.bytes) {
            Ok(Some(parsed)) => parsed,
            Ok(None) => panic!("{}: the payload does not parse", hook.name),
            Err(problem) => panic!("{}: {problem}", hook.name),
        };
        assert_eq!(
            parsed.events, hook.expected,
            "{}: events differ from {}.expected.json",
            hook.name, hook.name
        );
        assert_eq!(
            parsed.edit_phase, hook.edit,
            "{}: edit phase differs from {}.edit.json",
            hook.name, hook.name
        );
        observed.observe_edit(parsed.edit_phase.as_ref());
        observed.observe_events(&parsed.events);
    }

    for log in &logs {
        let chunk = match check_log(adapter, &scratch_log, &log.bytes, true) {
            Ok(Some(chunk)) => chunk,
            Ok(None) => panic!("{}: the log does not read", log.name),
            Err(problem) => panic!("{}: {problem}", log.name),
        };
        assert_eq!(
            chunk.events, log.expected,
            "{}: events differ from {}.expected.json",
            log.name, log.name
        );
        observed.observe_events(&chunk.events);
    }

    let payloads: Vec<Vec<u8>> = hooks.iter().map(|hook| hook.bytes.clone()).collect();
    check_arbitrary_bytes(adapter, &payloads, scratch.path());
    check_capabilities(adapter, &observed, logs.len());
    check_integration(adapter, scratch.path());
}

/// Writes the expected files of every fixture from the adapter's current output. Review the
/// diff by eye before committing it.
///
/// # Panics
///
/// Panics when a fixture does not read, or a file cannot be written.
pub fn bless(adapter: &dyn HarnessAdapter, fixtures: &Path) {
    let listing = list(fixtures);
    for path in &listing.hooks {
        let name = name_of(path);
        let bytes = read_bytes(path);
        let parsed = adapter
            .parse_hook(&bytes)
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        write_json(
            &fixtures.join(format!("{name}.expected.json")),
            &parsed.events,
        );
        let edit_path = fixtures.join(format!("{name}.edit.json"));
        match parsed.edit_phase {
            Some(info) => write_json(&edit_path, &EditJson::from_info(&info)),
            None => {
                let _ = fs::remove_file(&edit_path);
            }
        }
    }
    for path in &listing.logs {
        let name = name_of(path);
        let chunk = adapter
            .read_session_log(path, 0)
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        write_json(
            &fixtures.join(format!("{name}.expected.json")),
            &chunk.events,
        );
    }
}

struct Listing {
    hooks: Vec<PathBuf>,
    logs: Vec<PathBuf>,
}

fn list(dir: &Path) -> Listing {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .collect();
    entries.sort();
    let mut listing = Listing {
        hooks: Vec::new(),
        logs: Vec::new(),
    };
    for path in entries {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("hook") => listing.hooks.push(path),
            Some("log") => listing.logs.push(path),
            _ => {}
        }
    }
    listing
}

fn load_hook(dir: &Path, path: &Path) -> HookFixture {
    let name = name_of(path);
    let edit_path = dir.join(format!("{name}.edit.json"));
    let edit = edit_path
        .exists()
        .then(|| read_json::<EditJson>(&edit_path).into_info());
    HookFixture {
        bytes: read_bytes(path),
        expected: read_json(&dir.join(format!("{name}.expected.json"))),
        edit,
        name,
    }
}

fn load_log(dir: &Path, path: &Path) -> LogFixture {
    let name = name_of(path);
    LogFixture {
        bytes: read_bytes(path),
        expected: read_json(&dir.join(format!("{name}.expected.json"))),
        name,
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| panic!("fixture name is not UTF-8: {}", path.display()))
        .to_owned()
}

fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> T {
    let text = fs::read_to_string(path).unwrap_or_else(|err| {
        panic!(
            "missing or unreadable {} ({err}); generate it with BLESS=1",
            path.display()
        )
    });
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) {
    let mut text = serde_json::to_string_pretty(value).expect("serialize fixture");
    text.push('\n');
    fs::write(path, text).unwrap_or_else(|err| panic!("cannot write {}: {err}", path.display()));
}

/// Checks the generic properties of one hook payload. `Ok(None)` when it does not parse,
/// which is allowed for arbitrary bytes; `Err` when a property is broken.
fn check_hook(adapter: &dyn HarnessAdapter, payload: &[u8]) -> Result<Option<HookInput>, String> {
    let first = adapter.parse_hook(payload);
    if first != adapter.parse_hook(payload) {
        return Err("parse_hook is not deterministic".to_owned());
    }
    let Ok(input) = first else {
        return Ok(None);
    };
    if input.session_id.is_empty() {
        return Err("empty session_id".to_owned());
    }
    check_events(adapter, &input.events)?;
    if input
        .events
        .iter()
        .any(|event| event.header().session_id != input.session_id)
    {
        return Err("an event's session_id differs from HookInput.session_id".to_owned());
    }
    if input
        .edit_phase
        .as_ref()
        .is_some_and(|edit| edit.call_source_id.is_empty() || edit.path.is_empty())
    {
        return Err("edit phase with an empty call id or path".to_owned());
    }
    Ok(Some(input))
}

/// Checks a session log in one pass and in chunks: the union of the chunks must equal the
/// one-pass result for every cut, so a log read from a stored offset loses nothing.
/// `exhaustive` tries every byte as a cut; otherwise only a few cuts. `Ok(None)` when the log
/// does not read in one pass.
fn check_log(
    adapter: &dyn HarnessAdapter,
    scratch: &Path,
    bytes: &[u8],
    exhaustive: bool,
) -> Result<Option<LogChunk>, String> {
    write_scratch(scratch, bytes)?;
    let whole = adapter.read_session_log(scratch, 0);
    if whole != adapter.read_session_log(scratch, 0) {
        return Err("read_session_log is not deterministic".to_owned());
    }
    let Ok(whole) = whole else {
        return Ok(None);
    };
    check_events(adapter, &whole.events)?;
    for cut in cut_points(bytes, exhaustive) {
        split_read(adapter, scratch, bytes, cut, &whole)?;
    }
    Ok(Some(whole))
}

fn split_read(
    adapter: &dyn HarnessAdapter,
    scratch: &Path,
    bytes: &[u8],
    cut: usize,
    whole: &LogChunk,
) -> Result<(), String> {
    write_scratch(scratch, &bytes[..cut])?;
    let head = adapter
        .read_session_log(scratch, 0)
        .map_err(|err| format!("reading the first {cut} bytes failed: {err}"))?;
    write_scratch(scratch, bytes)?;
    let tail = adapter
        .read_session_log(scratch, head.next_offset)
        .map_err(|err| format!("reading after cut {cut} failed: {err}"))?;

    let events: Vec<&CanonicalEvent> = head.events.iter().chain(&tail.events).collect();
    let expected: Vec<&CanonicalEvent> = whole.events.iter().collect();
    if events != expected {
        return Err(format!(
            "cut at byte {cut}: chunked events differ from one pass"
        ));
    }
    let unrecognized: Vec<&String> = head.unrecognized.iter().chain(&tail.unrecognized).collect();
    let expected_unrecognized: Vec<&String> = whole.unrecognized.iter().collect();
    if unrecognized != expected_unrecognized {
        return Err(format!(
            "cut at byte {cut}: chunked unrecognized names differ"
        ));
    }
    if tail.next_offset != whole.next_offset {
        return Err(format!(
            "cut at byte {cut}: final offset {} differs from one pass {}",
            tail.next_offset, whole.next_offset
        ));
    }
    Ok(())
}

fn write_scratch(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|err| format!("cannot write scratch log: {err}"))
}

/// Every byte offset when exhaustive; otherwise the start, the middle, the end and every line
/// boundary.
fn cut_points(bytes: &[u8], exhaustive: bool) -> Vec<usize> {
    if exhaustive {
        return (0..=bytes.len()).collect();
    }
    let mut cuts: BTreeSet<usize> = BTreeSet::from([0, bytes.len() / 2, bytes.len()]);
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            cuts.insert(index + 1);
        }
    }
    cuts.into_iter().collect()
}

/// Generic event invariants: the adapter's harness id, a session id, a `source_id`, and no
/// event of one kind twice with the same `source_id`.
fn check_events(adapter: &dyn HarnessAdapter, events: &[CanonicalEvent]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for event in events {
        let header = event.header();
        if header.harness != adapter.id() {
            return Err(format!(
                "event `{}` has harness `{}`, expected `{}`",
                header.source_id,
                header.harness,
                adapter.id()
            ));
        }
        if header.session_id.is_empty() {
            return Err(format!(
                "event `{}` has an empty session_id",
                header.source_id
            ));
        }
        if header.source_id.is_empty() {
            return Err("an event has an empty source_id".to_owned());
        }
        if !seen.insert((kind_name(event), header.source_id.as_str())) {
            return Err(format!(
                "{} `{}` is produced twice",
                kind_name(event),
                header.source_id
            ));
        }
    }
    Ok(())
}

fn kind_name(event: &CanonicalEvent) -> &'static str {
    match event {
        CanonicalEvent::SessionStarted { .. } => "session_started",
        CanonicalEvent::PromptSubmitted { .. } => "prompt_submitted",
        CanonicalEvent::AssistantMessage { .. } => "assistant_message",
        CanonicalEvent::ToolCalled { .. } => "tool_called",
        CanonicalEvent::ToolReturned { .. } => "tool_returned",
        CanonicalEvent::TurnEnded { .. } => "turn_ended",
        CanonicalEvent::SessionEnded { .. } => "session_ended",
        CanonicalEvent::Notice { .. } => "notice",
    }
}

/// Feeds random bytes, and truncated fixture payloads, through the hook and log checks.
fn check_arbitrary_bytes(adapter: &dyn HarnessAdapter, payloads: &[Vec<u8>], dir: &Path) {
    let scratch = dir.join("arbitrary.log");
    let arbitrary = proptest::collection::vec(any::<u8>(), 0..512);
    let strategy: BoxedStrategy<Vec<u8>> = if payloads.is_empty() {
        arbitrary.boxed()
    } else {
        let fixtures = payloads.to_vec();
        let truncated = (0..fixtures.len(), any::<u16>()).prop_map(move |(index, cut)| {
            let payload = &fixtures[index];
            payload[..usize::from(cut).min(payload.len())].to_vec()
        });
        prop_oneof![arbitrary, truncated].boxed()
    };
    let mut runner = TestRunner::new(Config {
        failure_persistence: None,
        ..Config::default()
    });
    let result = runner.run(&strategy, |bytes| {
        check_hook(adapter, &bytes).map_err(TestCaseError::fail)?;
        check_log(adapter, &scratch, &bytes, false).map_err(TestCaseError::fail)?;
        Ok(())
    });
    if let Err(err) = result {
        panic!("{}: arbitrary input broke a property: {err}", adapter.id());
    }
}

/// An adapter must not claim a capability its fixtures do not show, and must not show a
/// capability it does not claim where that would mislead the pipeline.
fn check_capabilities(adapter: &dyn HarnessAdapter, observed: &Observed, log_fixtures: usize) {
    let id = adapter.id();
    let caps = adapter.capabilities();
    assert_eq!(
        caps.pre_edit_event,
        observed.before,
        "{id}: pre_edit_event is {} but the fixtures {} EditPhase::Before",
        caps.pre_edit_event,
        if observed.before {
            "yield"
        } else {
            "do not yield"
        }
    );
    if caps.session_log {
        assert!(
            log_fixtures > 0,
            "{id}: session_log is declared but no .log fixture exists"
        );
    }
    if caps.tool_results {
        assert!(
            observed.tool_results,
            "{id}: tool_results is declared but no fixture yields ToolReturned"
        );
    }
    if caps.subagents {
        assert!(
            observed.subagents,
            "{id}: subagents is declared but no fixture yields a parent call"
        );
    }
}

fn check_integration(adapter: &dyn HarnessAdapter, dir: &Path) {
    let id = adapter.id();
    let ctx = InstallContext {
        binary: PathBuf::from("/opt/gitmaster/bin/gitmaster"),
        data_dir: dir.to_path_buf(),
    };
    let integration = adapter.integration(&ctx);
    assert_eq!(
        integration,
        adapter.integration(&ctx),
        "{id}: integration is not deterministic"
    );
    for (path, _) in &integration.files {
        let escapes = path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        });
        assert!(
            path.is_relative() && !escapes,
            "{id}: integration file {} must be relative to the agent's directory",
            path.display()
        );
    }
    if adapter.capabilities().plugin_install {
        assert!(
            !integration.files.is_empty(),
            "{id}: plugin_install is declared but no file is generated"
        );
        assert!(
            !integration.steps.is_empty(),
            "{id}: plugin_install is declared but no step is given"
        );
    }
    // Discovery on a worktree with no sessions must answer, not panic.
    adapter.discover(dir);
}
