//! `threads::pipeline`: captures one hook invocation into `threads.db` (design/threads.md,
//! "Capture pipeline", "Edits (D123)", "Redaction (D121)").
//!
//! `ingest` is the whole path. It parses the payload through the adapter, reads the new lines
//! of the harness's session log, turns the canonical events into one batch, redacts and caps
//! every text field before anything is stored, measures the file of an edit, and applies the
//! batch in one transaction. A batch that cannot be stored (a busy database, a disk error) is
//! spooled as redacted JSON and drained by the next run. Nothing is printed, and nothing outside
//! `<common dir>/gitmaster/` is written.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use gm_schema::threads::{
    CanonicalEvent, EditChange, EditIntent, EditPhase, EntryKind, EventHeader, LinkKind,
    LinkOrigin, ThreadLink, ToolKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::config::ThreadsConfig;
use super::harness::{EditPhaseInfo, HarnessAdapter, HookInput};
use super::redact::{self, PatternProblem, Redactor};
use super::store::{Applied, Batch, NewEdit, NewEntry, StoreError, ThreadStore};

/// The largest text stored for an entry that is not a tool result, in bytes.
const TEXT_CAP: usize = 64 * 1024;
/// The largest stored patch, in bytes.
const PATCH_CAP: usize = 256 * 1024;
/// The largest file read for a pre-image or a post-image, in bytes.
const FILE_LIMIT: u64 = 2 * 1024 * 1024;
/// The largest hook document read; the CLI reads stdin up to the same size.
const PAYLOAD_LIMIT: usize = 8 * 1024 * 1024;
/// A pre-image left behind longer than this is removed by the next run.
const PRE_IMAGE_MAX_AGE_MS: i64 = 10 * 60 * 1000;
/// The largest spool file, in bytes; a batch that does not fit is dropped.
const SPOOL_FILE_LIMIT: usize = 8 * 1024 * 1024;
/// Spool files kept; the oldest go first.
const SPOOL_KEEP: usize = 100;
/// The log is renamed to `threads.log.1` once it reaches this size.
const LOG_ROTATE_BYTES: u64 = 256 * 1024;
/// The longest message written to the log.
const LOG_MESSAGE_CHARS: usize = 300;
/// The longest summary of a tool call, in characters.
const SUMMARY_CHARS: usize = 200;
/// Commits linked per turn at most.
const COMMITS_PER_TURN: usize = 50;
/// How far into a file the binary check looks, as Git does.
const BINARY_SCAN_BYTES: usize = 8000;
/// The start of every redaction marker, as `redact` writes it.
const MARKER_OPEN: &str = "[REDACTED:";
/// The argument names that carry a file path, in the order they are tried.
const PATH_KEYS: [&str; 3] = ["file_path", "path", "notebook_path"];
/// The argument names whose value summarizes a tool call, in the order they are tried.
const DETAIL_KEYS: [&str; 7] = [
    "file_path",
    "path",
    "notebook_path",
    "command",
    "pattern",
    "url",
    "query",
];
const BINARY_NOTE: &str = "Binary file changed\n";
const UNMEASURED_NOTE: &str = "Edit recorded from the tool input; the file was not measured.\n";
const UNRECOGNIZED: &str = "unrecognized event";

/// The repository a hook runs in, as `locate_repo` found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoInfo {
    /// The Git common directory; the store lives under it.
    pub common_dir: PathBuf,
    /// The root of the worktree the hook ran in.
    pub worktree: PathBuf,
    /// The branch checked out; `None` when detached.
    pub branch: Option<String>,
    /// The commit `HEAD` names; `None` on an unborn branch.
    pub head: Option<String>,
}

/// What one hook invocation did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The batch is stored; `inserted` entries were new.
    Stored { uid: String, inserted: u32 },
    /// The database could not take the batch now; it waits in the spool.
    Spooled,
    /// Nothing was stored.
    Dropped(DropReason),
}

/// Why an invocation stored nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropReason {
    CaptureOff,
    /// A user pattern does not build, so capture is paused everywhere (D121). Holds the names.
    InvalidPattern(Vec<String>),
    /// The payload is not one the adapter reads.
    Unparseable,
    NoRepo,
    UnknownHarness,
    /// Neither the database nor the spool could take the batch.
    Storage,
}

/// What one invocation needs besides the payload.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    pub config: &'a ThreadsConfig,
    /// The time of this invocation, in milliseconds since the Unix epoch.
    pub now_ms: i64,
}

/// One edit to attach to an entry of the batch, once the batch is stored.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PendingEdit {
    /// The `source_id` of the tool call the edit belongs to.
    call: String,
    edit: NewEdit,
}

/// What one ingest stores. It is also the spool file's content, so it holds only redacted data.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    batch: Batch,
    edits: Vec<PendingEdit>,
    /// Whether the batch closed a turn, which is when commit links are read.
    turn_ended: bool,
}

/// Finds the repository `cwd` is in. `None` outside a repository or when `git` is missing.
#[must_use]
pub fn locate_repo(cwd: &Path) -> Option<RepoInfo> {
    let (paths, head) = match git_output(
        cwd,
        &["rev-parse", "--git-common-dir", "--show-toplevel", "HEAD"],
    ) {
        Some(lines) if lines.len() == 3 => (lines[..2].to_vec(), Some(lines[2].clone())),
        // An unborn branch has no HEAD to name; the two paths still answer.
        _ => (
            git_output(cwd, &["rev-parse", "--git-common-dir", "--show-toplevel"])?,
            None,
        ),
    };
    let common = paths.first()?;
    let toplevel = paths.get(1)?;
    let branch = git_output(cwd, &["symbolic-ref", "--short", "-q", "HEAD"])
        .and_then(|lines| lines.into_iter().next());
    Some(RepoInfo {
        common_dir: absolute(cwd, common),
        worktree: PathBuf::from(toplevel),
        branch,
        head: head.filter(|head| is_commit_id(head)),
    })
}

/// Captures one hook payload. Never prints; the outcome is for the caller to exit with.
pub fn ingest(
    adapter: &dyn HarnessAdapter,
    payload: &[u8],
    repo: &RepoInfo,
    ctx: &Context<'_>,
    store: &mut ThreadStore,
) -> Outcome {
    if !ctx.config.capture {
        return Outcome::Dropped(DropReason::CaptureOff);
    }
    drain_spool(&repo.common_dir, store);
    match prepare(adapter, payload, repo, ctx, store) {
        Prepared::Done(outcome) => outcome,
        Prepared::Record(record) => persist(store, &record, repo, ctx.now_ms),
        Prepared::Refused { record, names } => {
            if let Some(record) = record {
                let _ = persist(store, &record, repo, ctx.now_ms);
            }
            Outcome::Dropped(DropReason::InvalidPattern(names))
        }
    }
}

/// Captures one hook payload without opening the database for writes. The hook uses this when
/// the database needs a migration too large for the hook to run (`ThreadStore::open_for_hook`):
/// the redacted batch is spooled for the app to store. `reader` is a read-only handle; it
/// supplies what the batch needs from earlier invocations, and is never written to.
pub fn ingest_deferred(
    adapter: &dyn HarnessAdapter,
    payload: &[u8],
    repo: &RepoInfo,
    ctx: &Context<'_>,
    reader: &ThreadStore,
) -> Outcome {
    if !ctx.config.capture {
        return Outcome::Dropped(DropReason::CaptureOff);
    }
    match prepare(adapter, payload, repo, ctx, reader) {
        Prepared::Done(outcome) => outcome,
        Prepared::Record(record) => spool_record(repo, &record, ctx.now_ms),
        Prepared::Refused { record, names } => {
            if let Some(record) = record {
                let _ = spool_record(repo, &record, ctx.now_ms);
            }
            Outcome::Dropped(DropReason::InvalidPattern(names))
        }
    }
}

/// What `prepare` leaves to the caller: a batch to store, a refusal (a batch with only a gap
/// row, when the session can be named), or an outcome already decided.
enum Prepared {
    Record(Record),
    Refused {
        record: Option<Record>,
        names: Vec<String>,
    },
    Done(Outcome),
}

/// Reads, converts and redacts one payload into a record. Reads the store, never writes it.
fn prepare(
    adapter: &dyn HarnessAdapter,
    payload: &[u8],
    repo: &RepoInfo,
    ctx: &Context<'_>,
    store: &ThreadStore,
) -> Prepared {
    let redactor = match Redactor::new(&ctx.config.redact) {
        Ok(redactor) => redactor,
        Err(problems) => return refuse_invalid(adapter, payload, repo, ctx, &problems),
    };
    if payload.len() > PAYLOAD_LIMIT {
        append_log(
            &repo.common_dir,
            &format!("{}: payload over the size limit not read", adapter.id()),
        );
        return Prepared::Done(Outcome::Dropped(DropReason::Unparseable));
    }
    clean_tmp(&repo.common_dir, ctx.now_ms);

    let mut hook = match adapter.parse_hook(payload) {
        Ok(hook) => hook,
        Err(error) => {
            append_log(
                &repo.common_dir,
                &format!("{}: payload not read: {error}", adapter.id()),
            );
            return Prepared::Done(Outcome::Dropped(DropReason::Unparseable));
        }
    };
    let (events, log) = take_events(adapter, &mut hook, store, &repo.common_dir, ctx.now_ms);
    let calls = index_calls(&events, &repo.worktree);
    let skip_intent = hook
        .edit_phase
        .as_ref()
        .filter(|info| info.phase == EditPhase::Before)
        .map(|info| info.call_source_id.as_str());
    let scope = Scope {
        redactor: &redactor,
        worktree: &repo.worktree,
        store,
        harness: adapter.id(),
        session: &hook.session_id,
        max_result: ctx.config.max_result_bytes,
        calls: &calls,
        skip_intent,
    };
    let converted = convert_events(&events, scope);
    let mut edits = converted.intents;
    if let Some(info) = &hook.edit_phase
        && let Some(edit) = measure_phase(repo, &redactor, &calls, info)
    {
        edits.push(edit);
    }

    let (started_at, at) = span(&events, ctx.now_ms);
    let mut batch = base_batch(adapter.id(), &hook.session_id, repo, ctx.now_ms);
    batch.started_at = started_at;
    batch.at = at;
    batch.model = converted.meta.model;
    batch.harness_version = converted.meta.harness_version;
    batch.transcript = log
        .path
        .map(|path| redactor.redact(&path).text)
        .or(converted.meta.transcript);
    batch.cursor = log.cursor;
    batch.ended = converted.meta.ended;
    batch.entries = converted.entries;
    batch.gaps = hook
        .unrecognized
        .iter()
        .chain(&log.unrecognized)
        .map(|name| (ctx.now_ms, unrecognized_reason(name)))
        .collect();
    Prepared::Record(Record {
        batch,
        edits,
        turn_ended: converted.meta.turn_ended,
    })
}

/// Stores a record, or spools it when the store cannot take it now.
fn persist(store: &mut ThreadStore, record: &Record, repo: &RepoInfo, now_ms: i64) -> Outcome {
    match store_record(store, record, Some(repo)) {
        Ok(applied) => Outcome::Stored {
            uid: applied.uid,
            inserted: applied.inserted,
        },
        Err(error) => {
            append_log(&repo.common_dir, &format!("store: {error}"));
            spool_record(repo, record, now_ms)
        }
    }
}

/// Spools a record for the next run that can store it.
fn spool_record(repo: &RepoInfo, record: &Record, now_ms: i64) -> Outcome {
    match spool(&repo.common_dir, record, now_ms) {
        Ok(()) => Outcome::Spooled,
        Err(reason) => {
            append_log(&repo.common_dir, &format!("spool: {reason}"));
            Outcome::Dropped(DropReason::Storage)
        }
    }
}

/// What the session log contributed to one invocation.
#[derive(Default)]
struct LogPart {
    cursor: Option<u64>,
    path: Option<String>,
    unrecognized: Vec<String>,
}

/// The events of one invocation: the hook's, then the new lines of the session log. An event
/// the adapter gave no time for takes the invocation's time.
fn take_events(
    adapter: &dyn HarnessAdapter,
    hook: &mut HookInput,
    store: &ThreadStore,
    common_dir: &Path,
    now_ms: i64,
) -> (Vec<CanonicalEvent>, LogPart) {
    fill_times(&mut hook.events, now_ms);
    let mut events = std::mem::take(&mut hook.events);
    let part = match read_log(adapter, hook, store, common_dir) {
        Some(log) => {
            let mut log_events = log.events;
            fill_times(&mut log_events, now_ms);
            events.extend(log_events);
            LogPart {
                cursor: Some(log.next_offset),
                path: Some(log.path),
                unrecognized: log.unrecognized,
            }
        }
        None => LogPart::default(),
    };
    (events, part)
}

/// The edit of one edit phase of the hook. Only edit calls (or calls not seen in this
/// invocation) are measured. A pre-image yields nothing; a post-image yields the measured edit.
fn measure_phase(
    repo: &RepoInfo,
    redactor: &Redactor,
    calls: &HashMap<String, CallInfo>,
    info: &EditPhaseInfo,
) -> Option<PendingEdit> {
    let measurable = calls
        .get(&info.call_source_id)
        .is_none_or(|call| matches!(call.kind, ToolKind::Edit | ToolKind::Write));
    if !measurable {
        return None;
    }
    match info.phase {
        EditPhase::Before => {
            before_edit(&repo.common_dir, &repo.worktree, redactor, info);
            None
        }
        EditPhase::After => {
            after_edit(&repo.common_dir, &repo.worktree, redactor, info).map(|edit| PendingEdit {
                call: info.call_source_id.clone(),
                edit,
            })
        }
    }
}

/// Stores the spooled batches, oldest first, and removes the files it stored. Stops at the
/// first busy database and keeps the rest. Returns how many batches were stored.
pub fn drain_spool(common_dir: &Path, store: &mut ThreadStore) -> u32 {
    let mut stored = 0;
    for path in spool_files(&spool_dir(common_dir)) {
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let record = if bytes.len() > SPOOL_FILE_LIMIT {
            None
        } else {
            serde_json::from_slice::<Record>(&bytes).ok()
        };
        let Some(record) = record else {
            append_log(common_dir, "corrupt spool file removed");
            let _ = fs::remove_file(&path);
            continue;
        };
        match store_record(store, &record, None) {
            Ok(_) => {
                let _ = fs::remove_file(&path);
                stored += 1;
            }
            Err(error) if error.is_busy() || matches!(error, StoreError::Io(_)) => break,
            Err(error) => {
                append_log(common_dir, &format!("spool record dropped: {error}"));
                let _ = fs::remove_file(&path);
            }
        }
    }
    stored
}

/// Removes pre-images older than ten minutes: the post event that should have used them did
/// not come.
pub fn clean_tmp(common_dir: &Path, now_ms: i64) {
    let Ok(entries) = fs::read_dir(tmp_dir(common_dir)) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        if now_ms.saturating_sub(system_ms(modified)) > PRE_IMAGE_MAX_AGE_MS {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Appends one line to `<common dir>/gitmaster/threads.log`, rotated at 256 KiB. The message
/// must never carry payload content; control characters are replaced and it is cut short.
pub fn append_log(common_dir: &Path, message: &str) {
    let dir = gitmaster_dir(common_dir);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("threads.log");
    if fs::metadata(&path).is_ok_and(|meta| meta.len() >= LOG_ROTATE_BYTES) {
        let _ = fs::rename(&path, dir.join("threads.log.1"));
    }
    let message: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(LOG_MESSAGE_CHARS)
        .collect();
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    if let Ok(mut file) = options.open(&path) {
        let _ = writeln!(file, "{} {message}", system_ms(SystemTime::now()));
    }
}

/// An invalid pattern pauses capture: nothing from the payload is stored, only a content-free
/// gap row when the session can be named.
fn refuse_invalid(
    adapter: &dyn HarnessAdapter,
    payload: &[u8],
    repo: &RepoInfo,
    ctx: &Context<'_>,
    problems: &[PatternProblem],
) -> Prepared {
    let names: Vec<String> = problems
        .iter()
        .map(|problem| problem.name.clone())
        .collect();
    let reason = format!("invalid redaction pattern: {}", names.join(", "));
    append_log(&repo.common_dir, &reason);
    let record = adapter.parse_hook(payload).ok().map(|hook| {
        let mut batch = base_batch(adapter.id(), &hook.session_id, repo, ctx.now_ms);
        batch.gaps = vec![(ctx.now_ms, reason)];
        Record {
            batch,
            edits: Vec::new(),
            turn_ended: false,
        }
    });
    Prepared::Refused { record, names }
}

/// Stores a record: the batch, then its edits, then the links it earns. Links are facts: the
/// branch when the thread is new, each file an edit touched, and the commits made during a
/// turn (the last one only when a repository is given, since a spooled record is drained
/// without one).
fn store_record(
    store: &mut ThreadStore,
    record: &Record,
    repo: Option<&RepoInfo>,
) -> Result<Applied, StoreError> {
    let batch = &record.batch;
    let is_new = store.cursor(&batch.harness, &batch.session_id)?.is_none();
    let applied = store.apply(batch)?;
    if let (true, Some(branch)) = (is_new, batch.branch.as_deref()) {
        observe(store, &applied.uid, LinkKind::Branch, branch)?;
    }
    for pending in &record.edits {
        match store.attach_edit(&applied.uid, &pending.call, &pending.edit) {
            Ok(true) => observe(store, &applied.uid, LinkKind::File, &pending.edit.path)?,
            Ok(false) | Err(StoreError::NotFound) => {}
            Err(error) => return Err(error),
        }
    }
    if let (true, Some(repo)) = (record.turn_ended, repo) {
        link_commits(store, &applied.uid, repo);
    }
    Ok(applied)
}

fn observe(
    store: &mut ThreadStore,
    uid: &str,
    kind: LinkKind,
    reference: &str,
) -> Result<(), StoreError> {
    store.link(
        uid,
        &ThreadLink {
            kind,
            reference: reference.to_owned(),
            origin: LinkOrigin::Observed,
            resolved: None,
            state: None,
        },
    )
}

/// Commits between the thread's first `HEAD` and the current one, at most `COMMITS_PER_TURN`.
/// Failures are ignored: a link is a convenience, the entries are the record.
fn link_commits(store: &mut ThreadStore, uid: &str, repo: &RepoInfo) {
    let Some(head) = repo.head.as_deref() else {
        return;
    };
    let Ok((Some(start), _)) = store.heads(uid) else {
        return;
    };
    if start == head || !is_commit_id(&start) {
        return;
    }
    let range = format!("{start}..{head}");
    let limit = format!("--max-count={COMMITS_PER_TURN}");
    let Some(commits) = git_output(
        &repo.worktree,
        &["rev-list", limit.as_str(), range.as_str()],
    ) else {
        return;
    };
    for commit in commits.iter().filter(|commit| is_commit_id(commit)) {
        let _ = observe(store, uid, LinkKind::Commit, commit);
    }
}

/// The batch fields every ingest starts from; the caller fills the events in.
fn base_batch(harness: &str, session: &str, repo: &RepoInfo, now: i64) -> Batch {
    Batch {
        harness: harness.to_owned(),
        session_id: session.to_owned(),
        worktree: repo.worktree.to_string_lossy().into_owned(),
        branch: repo.branch.clone(),
        head: repo.head.clone(),
        model: None,
        harness_version: None,
        transcript: None,
        cursor: None,
        started_at: now,
        at: now,
        ended: None,
        entries: Vec::new(),
        gaps: Vec::new(),
    }
}

/// The earliest and the latest time of the events; the invocation's time when there are none.
fn span(events: &[CanonicalEvent], now: i64) -> (i64, i64) {
    let times = events.iter().map(|event| event.header().at);
    (
        times.clone().min().unwrap_or(now),
        times.max().unwrap_or(now),
    )
}

/// Events the adapter gave no time for (time zero) get the invocation's time.
fn fill_times(events: &mut [CanonicalEvent], now: i64) {
    for event in events {
        let header = header_mut(event);
        if header.at == 0 {
            header.at = now;
        }
    }
}

fn header_mut(event: &mut CanonicalEvent) -> &mut EventHeader {
    match event {
        CanonicalEvent::SessionStarted { header, .. }
        | CanonicalEvent::PromptSubmitted { header, .. }
        | CanonicalEvent::AssistantMessage { header, .. }
        | CanonicalEvent::ToolCalled { header, .. }
        | CanonicalEvent::ToolReturned { header, .. }
        | CanonicalEvent::TurnEnded { header, .. }
        | CanonicalEvent::SessionEnded { header, .. }
        | CanonicalEvent::Notice { header, .. } => header,
    }
}

/// The complete lines of the harness's session log that the store has not read yet.
struct LogRead {
    events: Vec<CanonicalEvent>,
    unrecognized: Vec<String>,
    next_offset: u64,
    path: String,
}

/// Reads the session log from the stored offset. A log that shrank (rewritten, compacted) is
/// read again from the start; the store's `source_id` rule makes that safe.
fn read_log(
    adapter: &dyn HarnessAdapter,
    hook: &HookInput,
    store: &ThreadStore,
    common_dir: &Path,
) -> Option<LogRead> {
    if !adapter.capabilities().session_log {
        return None;
    }
    let path = hook.log_path.as_deref()?;
    let offset = match store.cursor(adapter.id(), &hook.session_id) {
        Ok(Some((offset, _))) => offset,
        Ok(None) => 0,
        Err(error) => {
            append_log(common_dir, &format!("cursor: {error}"));
            0
        }
    };
    let mut chunk = read_chunk(adapter, path, offset, common_dir)?;
    if chunk.events.is_empty() && chunk.next_offset == 0 && offset > 0 {
        chunk = read_chunk(adapter, path, 0, common_dir)?;
    }
    Some(LogRead {
        events: chunk.events,
        unrecognized: chunk.unrecognized,
        next_offset: chunk.next_offset,
        path: path.to_owned(),
    })
}

fn read_chunk(
    adapter: &dyn HarnessAdapter,
    path: &str,
    from: u64,
    common_dir: &Path,
) -> Option<super::harness::LogChunk> {
    match adapter.read_session_log(Path::new(path), from) {
        Ok(chunk) => Some(chunk),
        Err(error) => {
            append_log(
                common_dir,
                &format!("{}: session log not read: {error}", adapter.id()),
            );
            None
        }
    }
}

fn unrecognized_reason(name: &str) -> String {
    let short: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(64)
        .collect();
    format!("{UNRECOGNIZED}: {short}")
}

/// A tool call as the batch knows it: what it is, and the repository path it touches.
#[derive(Clone, Debug)]
struct CallInfo {
    native: String,
    kind: ToolKind,
    path: Option<String>,
}

/// Every tool call in the invocation, so a result can find its call's path and kind.
fn index_calls(events: &[CanonicalEvent], worktree: &Path) -> HashMap<String, CallInfo> {
    let mut calls = HashMap::new();
    for event in events {
        if let CanonicalEvent::ToolCalled {
            header,
            tool,
            native_tool,
            args,
            edit,
            ..
        } = event
        {
            calls.insert(
                header.source_id.clone(),
                CallInfo {
                    native: native_tool.clone(),
                    kind: *tool,
                    path: call_path(worktree, args, edit.as_ref()),
                },
            );
        }
    }
    calls
}

/// What the converter learns along the way and the batch needs at the end.
#[derive(Debug, Default)]
struct Meta {
    model: Option<String>,
    harness_version: Option<String>,
    transcript: Option<String>,
    ended: Option<i64>,
    turn_ended: bool,
}

/// What the conversion reads: the redaction rules, the repository, the store (for calls an
/// earlier invocation stored) and the calls of this invocation.
struct Scope<'a> {
    redactor: &'a Redactor,
    worktree: &'a Path,
    store: &'a ThreadStore,
    harness: &'a str,
    session: &'a str,
    max_result: usize,
    calls: &'a HashMap<String, CallInfo>,
    /// The call whose pre-image is being taken: its edit comes from measuring, never from input.
    skip_intent: Option<&'a str>,
}

/// Turns canonical events into entries. Every text goes through `clean_text` (redact, then cap),
/// and the edit intents of the calls are collected for the attach step.
struct Converter<'a> {
    scope: Scope<'a>,
    meta: Meta,
    intents: Vec<PendingEdit>,
}

/// The entries of one invocation, and what its events add to the batch.
struct Converted {
    entries: Vec<NewEntry>,
    intents: Vec<PendingEdit>,
    meta: Meta,
}

fn convert_events(events: &[CanonicalEvent], scope: Scope<'_>) -> Converted {
    let mut converter = Converter {
        scope,
        meta: Meta::default(),
        intents: Vec::new(),
    };
    let entries: Vec<NewEntry> = events
        .iter()
        .filter_map(|event| converter.convert(event))
        .collect();
    Converted {
        entries,
        intents: converter.intents,
        meta: converter.meta,
    }
}

impl Converter<'_> {
    fn convert(&mut self, event: &CanonicalEvent) -> Option<NewEntry> {
        match event {
            CanonicalEvent::SessionStarted {
                model,
                harness_version,
                log_path,
                ..
            } => {
                keep(&mut self.meta.model, model.as_deref());
                keep(&mut self.meta.harness_version, harness_version.as_deref());
                if self.meta.transcript.is_none() {
                    self.meta.transcript = log_path
                        .as_deref()
                        .map(|path| self.scope.redactor.redact(path).text);
                }
                None
            }
            CanonicalEvent::PromptSubmitted { header, text } => Some(entry_of(
                header,
                EntryKind::User,
                clean_text(self.scope.redactor, text, TEXT_CAP),
                None,
                None,
                None,
                false,
            )),
            CanonicalEvent::AssistantMessage {
                header,
                text,
                model,
            } => {
                keep(&mut self.meta.model, model.as_deref());
                Some(entry_of(
                    header,
                    EntryKind::Assistant,
                    clean_text(self.scope.redactor, text, TEXT_CAP),
                    None,
                    None,
                    None,
                    false,
                ))
            }
            CanonicalEvent::ToolCalled {
                header,
                tool,
                native_tool,
                args,
                parent_call,
                edit,
            } => Some(self.tool_call(
                header,
                *tool,
                native_tool,
                args,
                parent_call.as_deref(),
                edit.as_ref(),
            )),
            CanonicalEvent::ToolReturned {
                header,
                call_source_id,
                text,
                is_error,
            } => Some(self.tool_result(header, call_source_id, text, *is_error)),
            CanonicalEvent::TurnEnded { .. } => {
                self.meta.turn_ended = true;
                None
            }
            CanonicalEvent::SessionEnded { header, .. } => {
                self.meta.ended = Some(header.at);
                None
            }
            CanonicalEvent::Notice { header, text } => Some(entry_of(
                header,
                EntryKind::Notice,
                clean_text(self.scope.redactor, text, TEXT_CAP),
                None,
                None,
                None,
                false,
            )),
        }
    }

    fn tool_call(
        &mut self,
        header: &EventHeader,
        tool: ToolKind,
        native: &str,
        args: &Value,
        parent: Option<&str>,
        intent: Option<&EditIntent>,
    ) -> NewEntry {
        let path = call_path(self.scope.worktree, args, intent);
        let sensitive = path
            .as_deref()
            .is_some_and(|path| self.scope.redactor.is_sensitive_path(path));
        let editing = matches!(tool, ToolKind::Edit | ToolKind::Write);
        let args_text = match path.as_deref() {
            Some(path) if sensitive && editing => marker_args(path),
            _ => clean_args(self.scope.redactor, args),
        };
        if let Some(intent) = intent.filter(|_| editing)
            && self.scope.skip_intent != Some(header.source_id.as_str())
        {
            let edit = self.intent_edit(intent, sensitive);
            self.intents.push(PendingEdit {
                call: header.source_id.clone(),
                edit,
            });
        }
        let summary = self.summary(native, args);
        entry_of(
            header,
            EntryKind::ToolCall,
            summary,
            Some(args_text),
            Some(native.to_owned()),
            parent.map(str::to_owned),
            false,
        )
    }

    fn tool_result(
        &self,
        header: &EventHeader,
        call: &str,
        text: &str,
        is_error: bool,
    ) -> NewEntry {
        let (tool, path) = match self.scope.calls.get(call) {
            Some(info) => (Some(info.native.clone()), info.path.clone()),
            None => (self.stored_tool(call), self.stored_path(call)),
        };
        let sensitive = path
            .as_deref()
            .is_some_and(|path| self.scope.redactor.is_sensitive_path(path));
        let body = if sensitive {
            marker_text()
        } else {
            clean_text(self.scope.redactor, text, self.scope.max_result)
        };
        entry_of(
            header,
            EntryKind::ToolResult,
            body,
            None,
            tool,
            None,
            is_error,
        )
    }

    /// The native tool name of a call stored by an earlier invocation.
    fn stored_tool(&self, call: &str) -> Option<String> {
        self.scope
            .store
            .entry_tool(self.scope.harness, self.scope.session, call)
            .ok()
            .flatten()
    }

    /// The path a call stored by an earlier invocation touched.
    fn stored_path(&self, call: &str) -> Option<String> {
        let json = self
            .scope
            .store
            .entry_args(self.scope.harness, self.scope.session, call)
            .ok()
            .flatten()?;
        let args: Value = serde_json::from_str(&json).ok()?;
        call_path(self.scope.worktree, &args, None)
    }

    /// The tool name and its argument summary, redacted, then cut to `SUMMARY_CHARS`. The cut
    /// comes after redaction, so a secret split by it cannot survive as a fragment.
    fn summary(&self, native: &str, args: &Value) -> Cleaned {
        let detail = DETAIL_KEYS.iter().find_map(|key| args.get(*key)?.as_str());
        let full = match detail {
            Some(detail) => format!("{native} {detail}"),
            None => native.to_owned(),
        };
        let redacted = self.scope.redactor.redact(&full);
        let mut shown: String = redacted
            .text
            .chars()
            .take(SUMMARY_CHARS)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let truncated = redacted.text.chars().count() > SUMMARY_CHARS;
        if truncated {
            cut_open_marker(&mut shown);
        }
        Cleaned {
            text: shown,
            hits: hits_of(&redacted.hits),
            truncated,
            orig_bytes: truncated.then(|| len_u64(&redacted.text)),
        }
    }

    /// The edit an intent describes, from the tool input (D123's fallback).
    fn intent_edit(&self, intent: &EditIntent, sensitive: bool) -> NewEdit {
        let path = repo_relative(&intent.path, self.scope.worktree);
        let (body, added, removed) = match (intent.old_text.as_deref(), intent.new_text.as_deref())
        {
            (Some(old), Some(new)) => text_patch(Path::new(&path), old.as_bytes(), new.as_bytes()),
            (None, Some(new)) => plus_lines(new),
            _ => (UNMEASURED_NOTE.to_owned(), 0, 0),
        };
        let edit = NewEdit {
            path,
            old_path: intent
                .old_path
                .as_deref()
                .map(|old| repo_relative(old, self.scope.worktree)),
            change: intent.change,
            patch: body,
            added,
            removed,
            pre_oid: None,
            post_oid: None,
            from_input: true,
            hits: Vec::new(),
        };
        clean_edit(self.scope.redactor, edit, sensitive)
    }
}

/// A text after redaction and the cap: what is stored and how it was made.
#[derive(Debug, Default)]
struct Cleaned {
    text: String,
    hits: Vec<(String, u32)>,
    truncated: bool,
    orig_bytes: Option<u64>,
}

/// Redacts, then caps: a secret cut by the cap cannot survive as an unmatched prefix.
fn clean_text(redactor: &Redactor, text: &str, cap: usize) -> Cleaned {
    let redacted = redactor.redact(text);
    let (kept, truncated, _) = redact::truncate(&redacted.text, cap);
    Cleaned {
        text: kept,
        hits: hits_of(&redacted.hits),
        truncated,
        orig_bytes: truncated.then(|| len_u64(&redacted.text)),
    }
}

/// The arguments as compact JSON, redacted string by string and then capped. The strings are
/// redacted before they are escaped: a JSON escape such as `\n` in front of a token would
/// otherwise hide it from a pattern's word boundary.
fn clean_args(redactor: &Redactor, args: &Value) -> Cleaned {
    let mut hits = Vec::new();
    let json = redact_json(redactor, args, &mut hits).to_string();
    let (text, truncated, _) = redact::truncate(&json, TEXT_CAP);
    Cleaned {
        text,
        hits,
        truncated,
        orig_bytes: truncated.then(|| len_u64(&json)),
    }
}

fn redact_json(redactor: &Redactor, value: &Value, hits: &mut Vec<(String, u32)>) -> Value {
    match value {
        Value::String(text) => {
            let redacted = redactor.redact(text);
            merge_hits(hits, &hits_of(&redacted.hits));
            Value::String(redacted.text)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact_json(redactor, item, hits))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| {
                    let key = redactor.redact(key);
                    merge_hits(hits, &hits_of(&key.hits));
                    (key.text, redact_json(redactor, item, hits))
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The text that replaces the content of a sensitive file (design/threads.md, "Redaction").
fn marker_text() -> Cleaned {
    Cleaned {
        text: Redactor::sensitive_file_marker().to_owned(),
        ..Cleaned::default()
    }
}

/// The arguments of an edit to a sensitive file: the path stays, the content does not.
fn marker_args(path: &str) -> Cleaned {
    Cleaned {
        text: json!({ "path": path, "content": Redactor::sensitive_file_marker() }).to_string(),
        ..Cleaned::default()
    }
}

/// The entry of one event. Hits of the text and of the arguments are merged, so the entry's
/// total is the sum of its redactions.
fn entry_of(
    header: &EventHeader,
    kind: EntryKind,
    text: Cleaned,
    args: Option<Cleaned>,
    tool: Option<String>,
    parent_id: Option<String>,
    is_error: bool,
) -> NewEntry {
    let (args_json, args_hits, args_truncated, args_orig) = match args {
        Some(args) => (Some(args.text), args.hits, args.truncated, args.orig_bytes),
        None => (None, Vec::new(), false, None),
    };
    let mut hits = text.hits;
    merge_hits(&mut hits, &args_hits);
    NewEntry {
        source_id: header.source_id.clone(),
        kind,
        parent_id,
        tool,
        text: text.text,
        args_json,
        is_error,
        truncated: text.truncated || args_truncated,
        orig_bytes: text.orig_bytes.or(args_orig),
        redactions: hits
            .iter()
            .fold(0_u32, |sum, (_, count)| sum.saturating_add(*count)),
        hits,
        at: header.at,
        edit: None,
    }
}

/// Redacts an edit's path and patch and caps the patch. A sensitive file's patch is the marker.
fn clean_edit(redactor: &Redactor, mut edit: NewEdit, sensitive: bool) -> NewEdit {
    let path = redactor.redact(&edit.path);
    let mut hits = hits_of(&path.hits);
    edit.path = path.text;
    if let Some(old) = edit.old_path.take() {
        let old = redactor.redact(&old);
        merge_hits(&mut hits, &hits_of(&old.hits));
        edit.old_path = Some(old.text);
    }
    if sensitive {
        Redactor::sensitive_file_marker().clone_into(&mut edit.patch);
    } else {
        let cleaned = clean_text(redactor, &edit.patch, PATCH_CAP);
        merge_hits(&mut hits, &cleaned.hits);
        edit.patch = cleaned.text;
    }
    edit.hits = hits;
    edit
}

/// A repeated pattern name adds up; the result is sorted by name.
fn merge_hits(into: &mut Vec<(String, u32)>, more: &[(String, u32)]) {
    for (name, count) in more {
        match into.iter_mut().find(|(known, _)| known == name) {
            Some((_, total)) => *total = total.saturating_add(*count),
            None => into.push((name.clone(), *count)),
        }
    }
    into.sort_by(|a, b| a.0.cmp(&b.0));
}

fn hits_of(hits: &[redact::Hit]) -> Vec<(String, u32)> {
    hits.iter()
        .map(|hit| {
            (
                hit.pattern.clone(),
                u32::try_from(hit.count).unwrap_or(u32::MAX),
            )
        })
        .collect()
}

fn keep(slot: &mut Option<String>, value: Option<&str>) {
    if let Some(value) = value {
        *slot = Some(value.to_owned());
    }
}

/// Cuts a `[REDACTED:` that the cut left open, so no fragment of a marker is stored.
fn cut_open_marker(text: &mut String) {
    if let Some(open) = text.rfind(MARKER_OPEN)
        && !text[open..].contains(']')
    {
        text.truncate(open);
    }
}

/// The repository path a call touches: the intent's, else the first path argument's. Paths
/// are repository-relative when they lie inside the worktree.
fn call_path(worktree: &Path, args: &Value, intent: Option<&EditIntent>) -> Option<String> {
    let raw = intent
        .map(|intent| intent.path.clone())
        .or_else(|| path_of_args(args))?;
    Some(repo_relative(&raw, worktree))
}

fn path_of_args(args: &Value) -> Option<String> {
    PATH_KEYS
        .iter()
        .find_map(|key| args.get(*key)?.as_str())
        .map(str::to_owned)
}

/// `path` without the worktree prefix; a path outside the worktree is kept as given.
fn repo_relative(path: &str, worktree: &Path) -> String {
    Path::new(path).strip_prefix(worktree).map_or_else(
        |_| path.to_owned(),
        |relative| relative.to_string_lossy().into_owned(),
    )
}

/// The absolute path of a file under the worktree, or `None` when it would leave it. Paths are
/// data: a `..` or an absolute path outside the worktree is never opened.
fn resolve(worktree: &Path, path: &str) -> Option<PathBuf> {
    let candidate = Path::new(path);
    let full = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        worktree.join(candidate)
    };
    let relative = full.strip_prefix(worktree).ok()?;
    let inside = relative
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    (inside && relative.components().next().is_some()).then_some(full)
}

/// The pre-image directory of the worktree's common directory.
fn gitmaster_dir(common_dir: &Path) -> PathBuf {
    common_dir.join("gitmaster")
}

fn tmp_dir(common_dir: &Path) -> PathBuf {
    gitmaster_dir(common_dir).join("threads-tmp")
}

fn spool_dir(common_dir: &Path) -> PathBuf {
    gitmaster_dir(common_dir).join("threads-spool")
}

/// The name of a call's pre-image: the first 32 hex digits of SHA-256 of its `source_id`.
fn tmp_key(call: &str) -> String {
    let mut key = String::with_capacity(32);
    for byte in &Sha256::digest(call.as_bytes())[..16] {
        let _ = write!(key, "{byte:02x}");
    }
    key
}

/// Reads the file's content before an edit, when it is a regular file of a measurable size.
/// A file that does not exist is recorded as an `.absent` marker. A sensitive path is never
/// read: its content must not reach the disk even briefly.
fn before_edit(common_dir: &Path, worktree: &Path, redactor: &Redactor, info: &EditPhaseInfo) {
    if redactor.is_sensitive_path(&repo_relative(&info.path, worktree)) {
        return;
    }
    let Some(file) = resolve(worktree, &info.path) else {
        return;
    };
    let dir = tmp_dir(common_dir);
    let key = tmp_key(&info.call_source_id);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    match fs::symlink_metadata(&file) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let _ = write_private(&dir.join(format!("{key}.absent")), &[]);
        }
        Ok(meta) if meta.is_file() && meta.len() <= FILE_LIMIT => {
            if let Ok(bytes) = fs::read(&file) {
                let _ = write_private(&dir.join(&key), &bytes);
            }
        }
        _ => {}
    }
}

/// Takes the pre-image of a call (removing it from the disk) and measures the edit against the
/// file as it is now. `None` when there is no pre-image, the file cannot be measured, or the
/// path is sensitive; the caller's intent fallback covers those.
fn after_edit(
    common_dir: &Path,
    worktree: &Path,
    redactor: &Redactor,
    info: &EditPhaseInfo,
) -> Option<NewEdit> {
    let pre = take_pre_image(&tmp_dir(common_dir), &tmp_key(&info.call_source_id));
    let relative = repo_relative(&info.path, worktree);
    if redactor.is_sensitive_path(&relative) {
        return None;
    }
    let file = resolve(worktree, &info.path)?;
    let pre = match pre? {
        PreImage::Absent => None,
        PreImage::Content(bytes) => Some(bytes),
    };
    let post = match fs::symlink_metadata(&file) {
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Ok(meta) if meta.is_file() && meta.len() <= FILE_LIMIT => Some(fs::read(&file).ok()?),
        _ => return None,
    };
    measure(redactor, &relative, pre.as_deref(), post.as_deref())
}

/// The state of a file before an edit, as `before_edit` left it.
enum PreImage {
    Absent,
    Content(Vec<u8>),
}

/// Reads and removes a pre-image; `None` when there is none.
fn take_pre_image(dir: &Path, key: &str) -> Option<PreImage> {
    let absent = dir.join(format!("{key}.absent"));
    if absent.exists() {
        let _ = fs::remove_file(&absent);
        return Some(PreImage::Absent);
    }
    let present = dir.join(key);
    let bytes = fs::read(&present).ok();
    let _ = fs::remove_file(&present);
    bytes.map(PreImage::Content)
}

/// The edit between two images of a file. `None` when neither side exists.
fn measure(
    redactor: &Redactor,
    path: &str,
    pre: Option<&[u8]>,
    post: Option<&[u8]>,
) -> Option<NewEdit> {
    let change = match (pre, post) {
        (None, None) => return None,
        (None, Some(_)) => EditChange::Create,
        (Some(_), None) => EditChange::Delete,
        (Some(_), Some(_)) => EditChange::Modify,
    };
    let (body, added, removed) = text_patch(
        Path::new(path),
        pre.unwrap_or_default(),
        post.unwrap_or_default(),
    );
    let edit = NewEdit {
        path: path.to_owned(),
        old_path: None,
        change,
        patch: body,
        added,
        removed,
        pre_oid: pre.and_then(blob_id),
        post_oid: post.and_then(blob_id),
        from_input: false,
        hits: Vec::new(),
    };
    Some(clean_edit(redactor, edit, false))
}

/// The blob id `git hash-object` gives the bytes, without writing anything.
fn blob_id(bytes: &[u8]) -> Option<String> {
    git2::Oid::hash_object(git2::ObjectType::Blob, bytes)
        .ok()
        .map(|oid| oid.to_string())
}

/// A unified diff of two texts with its added and removed line counts. Binary content gets a
/// note and no counts; a diff that cannot be built gets the note for unmeasured edits.
fn text_patch(path: &Path, old: &[u8], new: &[u8]) -> (String, u32, u32) {
    if is_binary(old) || is_binary(new) {
        return (BINARY_NOTE.to_owned(), 0, 0);
    }
    diff(path, old, new).unwrap_or_else(|_| (UNMEASURED_NOTE.to_owned(), 0, 0))
}

fn diff(path: &Path, old: &[u8], new: &[u8]) -> Result<(String, u32, u32), git2::Error> {
    let mut built = git2::Patch::from_buffers(old, Some(path), new, Some(path), None)?;
    let (_, added, removed) = built.line_stats()?;
    let text = String::from_utf8_lossy(&built.to_buf()?).into_owned();
    Ok((text, count(added), count(removed)))
}

/// A patch of only added lines, for an edit whose old text the harness did not give.
fn plus_lines(new: &str) -> (String, u32, u32) {
    let mut patch = String::new();
    let mut added = 0_usize;
    for line in new.lines() {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
        added += 1;
    }
    (patch, count(added), 0)
}

/// Git's rule: a NUL byte near the start makes a file binary.
fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SCAN_BYTES).any(|byte| *byte == 0)
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn len_u64(text: &str) -> u64 {
    u64::try_from(text.len()).unwrap_or(u64::MAX)
}

fn is_commit_id(text: &str) -> bool {
    text.len() >= 7 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Runs `git` in `cwd` and returns its output lines, or `None` when it fails or is missing.
fn git_output(cwd: &Path, args: &[&str]) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Some(text.lines().map(str::to_owned).collect())
}

fn absolute(cwd: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn system_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// Writes a file that only this user can read.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

/// Writes the record to the spool under a name that sorts by time, then keeps the newest files.
fn spool(common_dir: &Path, record: &Record, now_ms: i64) -> Result<(), String> {
    let bytes = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    if bytes.len() > SPOOL_FILE_LIMIT {
        return Err("the batch is larger than the spool file limit".to_owned());
    }
    let dir = spool_dir(common_dir);
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let name = format!("{:013}-{:08x}.json", now_ms.max(0), random_u32());
    write_private(&dir.join(name), &bytes).map_err(|error| error.to_string())?;
    prune_spool(&dir);
    Ok(())
}

fn prune_spool(dir: &Path) {
    let files = spool_files(dir);
    let excess = files.len().saturating_sub(SPOOL_KEEP);
    for path in files.iter().take(excess) {
        let _ = fs::remove_file(path);
    }
}

/// The spool files, oldest first.
fn spool_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    files
}

/// Random enough to keep two spool files of one millisecond apart.
fn random_u32() -> u32 {
    let bits = RandomState::new().build_hasher().finish();
    u32::try_from(bits & 0xffff_ffff).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads::config::RedactConfig;
    use crate::threads::harness::fake::{self, FakeAdapter};
    use crate::threads::harness::{Capabilities, LogChunk, ParseError, SessionLog};
    use gm_schema::threads::ThreadDetail;

    const NOW: i64 = 1_700_000_000_000;
    const TOKEN_PREFIX: &str = "ghp_";

    fn token() -> String {
        [TOKEN_PREFIX, &"a".repeat(36)].concat()
    }

    /// A temporary worktree with its common directory, and the store and settings for it.
    struct Rig {
        dir: tempfile::TempDir,
        store: ThreadStore,
        config: ThreadsConfig,
    }

    impl Rig {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let store = ThreadStore::open(&dir.path().join(".git")).expect("store");
            Self {
                dir,
                store,
                config: ThreadsConfig::default(),
            }
        }

        fn repo(&self) -> RepoInfo {
            RepoInfo {
                common_dir: self.dir.path().join(".git"),
                worktree: self.dir.path().to_path_buf(),
                branch: Some("main".to_owned()),
                head: Some("1111111aaaaaaa".to_owned()),
            }
        }

        fn send(&mut self, adapter: &dyn HarnessAdapter, payload: &Value) -> Outcome {
            let bytes = serde_json::to_vec(payload).expect("json");
            self.send_bytes(adapter, &bytes)
        }

        fn send_bytes(&mut self, adapter: &dyn HarnessAdapter, bytes: &[u8]) -> Outcome {
            let repo = self.repo();
            let ctx = Context {
                config: &self.config,
                now_ms: NOW,
            };
            ingest(adapter, bytes, &repo, &ctx, &mut self.store)
        }

        fn ingest(&mut self, payload: &Value) -> Outcome {
            self.send(&FakeAdapter, payload)
        }

        fn thread(&self, session: &str) -> ThreadDetail {
            self.store
                .read_thread(&ThreadStore::thread_uid(fake::ID, session), 0, 1000)
                .expect("thread")
        }
    }

    fn start(session: &str, at: i64) -> Value {
        json!({"event": "session_start", "session": session, "id": "start", "at": at, "cwd": "/x"})
    }

    fn prompt(session: &str, id: &str, at: i64, text: &str) -> Value {
        json!({"event": "prompt", "session": session, "id": id, "at": at, "text": text})
    }

    #[test]
    fn each_event_kind_becomes_one_entry_of_its_kind() {
        let mut rig = Rig::new();
        let session = "s";
        let payloads = [
            start(session, 10),
            prompt(session, "u1", 11, "fix the parser\nmore"),
            json!({"event": "assistant", "session": session, "id": "a1", "at": 12, "text": "ok"}),
            json!({"event": "tool_call", "session": session, "id": "t1", "at": 13, "tool": "read", "args": {"file_path": "src/lib.rs"}}),
            json!({"event": "tool_result", "session": session, "id": "r1", "at": 14, "call": "t1", "text": "fn main() {}"}),
            json!({"event": "tool_call", "session": session, "id": "t2", "at": 15, "tool": "bash", "args": {"command": "cargo test"}, "parent": "t1"}),
            json!({"event": "tool_result", "session": session, "id": "r2", "at": 16, "call": "t2", "text": "failed", "error": true}),
            json!({"event": "notice", "session": session, "id": "n1", "at": 17, "text": "compacted"}),
            json!({"event": "turn_end", "session": session, "id": "e1", "at": 18}),
            json!({"event": "session_end", "session": session, "id": "x1", "at": 19, "text": "exit"}),
        ];
        for payload in &payloads {
            assert!(matches!(rig.ingest(payload), Outcome::Stored { .. }));
        }
        let detail = rig.thread(session);
        let shape: Vec<(EntryKind, Option<&str>)> = detail
            .entries
            .iter()
            .map(|entry| (entry.kind, entry.tool.as_deref()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (EntryKind::User, None),
                (EntryKind::Assistant, None),
                (EntryKind::ToolCall, Some("read")),
                (EntryKind::ToolResult, Some("read")),
                (EntryKind::ToolCall, Some("bash")),
                (EntryKind::ToolResult, Some("bash")),
                (EntryKind::Notice, None),
            ]
        );
        assert_eq!(detail.entries[0].text, "fix the parser\nmore");
        assert_eq!(detail.entries[2].text, "read src/lib.rs");
        assert_eq!(
            detail.entries[2].args_json.as_deref(),
            Some("{\"file_path\":\"src/lib.rs\"}")
        );
        assert_eq!(detail.entries[4].parent_id.as_deref(), Some("t1"));
        assert!(detail.entries[5].is_error);
        assert_eq!(detail.summary.title, "fix the parser");
        assert_eq!(detail.summary.ended_at, Some(19));
        assert_eq!(detail.summary.branch.as_deref(), Some("main"));
        assert_eq!(detail.summary.last_event_at, 19);
    }

    #[test]
    fn a_zero_time_takes_the_invocation_time() {
        let mut rig = Rig::new();
        rig.ingest(&prompt("s", "u1", 0, "hello"));
        let detail = rig.thread("s");
        assert_eq!(detail.entries[0].at, NOW);
    }

    #[test]
    fn long_text_is_capped_after_redaction_and_the_cut_is_flagged() {
        let mut rig = Rig::new();
        rig.config.max_result_bytes = 1000;
        let long = "a".repeat(100 * 1024);
        rig.ingest(&prompt("s", "u1", 1, &long));
        rig.ingest(&json!({"event": "tool_call", "session": "s", "id": "t1", "at": 2, "tool": "read", "args": {"file_path": "x.txt"}}));
        rig.ingest(&json!({"event": "tool_result", "session": "s", "id": "r1", "at": 3, "call": "t1", "text": long}));
        let detail = rig.thread("s");
        let prompt_entry = &detail.entries[0];
        assert!(prompt_entry.truncated);
        assert_eq!(prompt_entry.orig_bytes, Some(100 * 1024));
        assert!(prompt_entry.text.len() <= TEXT_CAP + 100);
        let result = &detail.entries[2];
        assert!(result.truncated);
        assert!(result.text.len() <= 1000 + 100);
    }

    #[test]
    fn a_secret_straddling_the_cap_leaves_no_fragment() {
        let mut rig = Rig::new();
        rig.config.max_result_bytes = 200;
        let text = format!("{} {}", "x".repeat(190), token());
        rig.ingest(&json!({"event": "tool_call", "session": "s", "id": "t1", "at": 1, "tool": "read", "args": {"file_path": "x.txt"}}));
        rig.ingest(&json!({"event": "tool_result", "session": "s", "id": "r1", "at": 2, "call": "t1", "text": text}));
        let detail = rig.thread("s");
        let result = &detail.entries[1];
        assert!(!result.text.contains(TOKEN_PREFIX));
        assert!(!result.text.contains("aaaa"));
    }

    #[test]
    fn a_long_argument_summary_is_cut_after_redaction() {
        let mut rig = Rig::new();
        let command = format!("{}{}", "echo ".repeat(38), token());
        rig.ingest(&json!({"event": "tool_call", "session": "s", "id": "t1", "at": 1, "tool": "bash", "args": {"command": command}}));
        let detail = rig.thread("s");
        let call = &detail.entries[0];
        assert!(call.text.chars().count() <= SUMMARY_CHARS);
        assert!(!call.text.contains(TOKEN_PREFIX));
        assert!(call.truncated);
    }

    #[test]
    fn a_sensitive_file_result_is_not_stored_across_two_invocations() {
        let mut rig = Rig::new();
        rig.ingest(&json!({"event": "tool_call", "session": "s", "id": "t1", "at": 1, "tool": "read", "args": {"file_path": ".env"}}));
        rig.ingest(&json!({"event": "tool_result", "session": "s", "id": "r1", "at": 2, "call": "t1", "text": "API_URL=https://example.test"}));
        rig.ingest(&json!({"event": "tool_call", "session": "s", "id": "t2", "at": 3, "tool": "read", "args": {"file_path": "src/a.rs"}}));
        rig.ingest(&json!({"event": "tool_result", "session": "s", "id": "r2", "at": 4, "call": "t2", "text": "fn a() {}"}));
        let detail = rig.thread("s");
        assert_eq!(detail.entries[1].text, Redactor::sensitive_file_marker());
        assert_eq!(detail.entries[3].text, "fn a() {}");
    }

    #[test]
    fn a_sensitive_result_is_recognized_from_the_store_when_the_call_is_older() {
        let mut rig = Rig::new();
        rig.ingest(&json!({"event": "tool_call", "session": "s", "id": "t1", "at": 1, "tool": "read", "args": {"path": "config/.env"}}));
        rig.ingest(&json!({"event": "tool_result", "session": "s", "id": "r1", "at": 2, "call": "t1", "text": "DEBUG=1"}));
        let detail = rig.thread("s");
        assert_eq!(detail.entries[1].text, Redactor::sensitive_file_marker());
    }

    #[test]
    fn redaction_counts_are_kept_per_entry_and_not_added_twice() {
        let mut rig = Rig::new();
        let payload = prompt("s", "u1", 1, &format!("use {} now", token()));
        rig.ingest(&payload);
        rig.ingest(&payload);
        let detail = rig.thread("s");
        assert!(!detail.entries[0].text.contains(TOKEN_PREFIX));
        assert_eq!(detail.entries[0].redactions, 1);
        assert_eq!(detail.summary.redaction_count, 1);
        assert_eq!(detail.redactions, vec![("github-token".to_owned(), 1)]);
    }

    #[test]
    fn a_measured_edit_has_a_patch_and_the_blob_ids_of_both_images() {
        let mut rig = Rig::new();
        let file = rig.dir.path().join("a.txt");
        fs::write(&file, "one\n").expect("write");
        rig.ingest(&json!({"event": "tool_call", "session": "s", "id": "t1", "at": 1, "tool": "edit", "args": {"path": "a.txt", "old": "one\n", "new": "one\ntwo\n"}}));
        rig.send(
            &FakeAdapter,
            &json!({"event": "edit_before", "session": "s", "call": "t1", "path": "a.txt"}),
        );
        fs::write(&file, "one\ntwo\n").expect("write");
        rig.send(
            &FakeAdapter,
            &json!({"event": "edit_after", "session": "s", "call": "t1", "path": "a.txt"}),
        );
        let detail = rig.thread("s");
        assert_eq!(detail.edits.len(), 1);
        let edit = &detail.edits[0];
        assert!(!edit.from_input);
        assert_eq!(edit.change, EditChange::Modify);
        assert_eq!((edit.added, edit.removed), (1, 0));
        assert!(edit.patch.contains("+two\n"), "{}", edit.patch);
        assert_eq!(edit.pre_oid.as_deref(), blob_id(b"one\n").as_deref());
        assert_eq!(edit.post_oid.as_deref(), blob_id(b"one\ntwo\n").as_deref());
        assert!(
            !tmp_dir(&rig.dir.path().join(".git"))
                .join(tmp_key("t1"))
                .exists()
        );
    }

    #[test]
    fn create_delete_and_binary_edits_are_measured_by_their_images() {
        let redactor = Redactor::new(&RedactConfig::default()).expect("redactor");
        let created = measure(&redactor, "n.txt", None, Some(&b"hi\n"[..])).expect("create");
        assert_eq!(created.change, EditChange::Create);
        assert_eq!((created.added, created.removed), (1, 0));
        assert_eq!(created.pre_oid, None);
        let deleted = measure(&redactor, "n.txt", Some(&b"hi\n"[..]), None).expect("delete");
        assert_eq!(deleted.change, EditChange::Delete);
        assert_eq!((deleted.added, deleted.removed), (0, 1));
        assert_eq!(deleted.post_oid, None);
        let binary =
            measure(&redactor, "b.bin", Some(&b"\0a"[..]), Some(&b"\0b"[..])).expect("binary");
        assert_eq!(binary.patch, BINARY_NOTE);
        assert_eq!((binary.added, binary.removed), (0, 0));
        assert!(measure(&redactor, "x", None, None).is_none());
    }

    #[test]
    fn an_edit_without_a_measurement_falls_back_to_the_tool_input() {
        let mut rig = Rig::new();
        rig.ingest(&json!({"event": "tool_call", "session": "s", "id": "t1", "at": 1, "tool": "edit", "args": {"path": "a.txt", "old": "one\n", "new": "two\n"}}));
        let detail = rig.thread("s");
        assert_eq!(detail.edits.len(), 1);
        assert!(detail.edits[0].from_input);
        assert!(detail.edits[0].patch.contains("+two"));
        assert_eq!(detail.edits[0].pre_oid, None);
    }

    #[test]
    fn a_new_text_without_old_text_is_all_added_lines() {
        let (patch, added, removed) = plus_lines("a\nb\n");
        assert_eq!(patch, "+a\n+b\n");
        assert_eq!((added, removed), (2, 0));
    }

    #[test]
    fn an_input_edit_with_a_secret_stores_no_secret() {
        let mut rig = Rig::new();
        let new = format!("token = {}\n", token());
        rig.ingest(&json!({"event": "tool_call", "session": "s", "id": "t1", "at": 1, "tool": "write", "args": {"path": "a.txt", "new": new}}));
        let detail = rig.thread("s");
        assert_eq!(detail.edits.len(), 1);
        assert!(!detail.edits[0].patch.contains(TOKEN_PREFIX));
        assert_eq!(detail.edits[0].change, EditChange::Create);
    }

    #[test]
    fn an_edit_to_a_sensitive_file_stores_the_marker_as_its_patch() {
        let mut rig = Rig::new();
        rig.ingest(&json!({"event": "tool_call", "session": "s", "id": "t1", "at": 1, "tool": "edit", "args": {"path": ".env", "old": "A=1\n", "new": "B=secretvalue\n"}}));
        let detail = rig.thread("s");
        assert_eq!(detail.edits[0].patch, Redactor::sensitive_file_marker());
        assert!(
            !detail.entries[0]
                .args_json
                .as_deref()
                .unwrap_or_default()
                .contains("secretvalue")
        );
    }

    #[test]
    fn a_before_event_never_stores_an_input_edit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("session.jsonl");
        let line = json!({"event": "tool_call", "session": "s", "id": "t9", "at": 1, "tool": "edit", "args": {"path": "a.txt", "old": "a", "new": "b"}});
        fs::write(&log, format!("{line}\n")).expect("log");
        let adapter = WithLog(log);
        let mut rig = Rig::new();
        rig.send(
            &adapter,
            &json!({"event": "edit_before", "session": "s", "call": "t9", "path": "a.txt"}),
        );
        assert!(!rig.store.has_edit(fake::ID, "s", "t9").expect("has"));
    }

    /// The fake adapter with a session log: the hook names the log it was given.
    struct WithLog(PathBuf);

    impl HarnessAdapter for WithLog {
        fn id(&self) -> &'static str {
            FakeAdapter.id()
        }
        fn display_name(&self) -> &'static str {
            "fake with log"
        }
        fn capabilities(&self) -> Capabilities {
            FakeAdapter.capabilities()
        }
        fn parse_hook(&self, payload: &[u8]) -> Result<HookInput, ParseError> {
            let mut hook = FakeAdapter.parse_hook(payload)?;
            hook.log_path = Some(self.0.display().to_string());
            Ok(hook)
        }
        fn read_session_log(&self, path: &Path, from: u64) -> Result<LogChunk, ParseError> {
            FakeAdapter.read_session_log(path, from)
        }
        fn discover(&self, worktree: &Path) -> Vec<SessionLog> {
            FakeAdapter.discover(worktree)
        }
    }

    #[test]
    fn the_session_log_is_read_from_the_stored_offset_and_a_shrunk_log_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("session.jsonl");
        let first = json!({"event": "prompt", "session": "s", "id": "lu1", "at": 5, "text": "hi"})
            .to_string();
        let second =
            json!({"event": "assistant", "session": "s", "id": "la1", "at": 6, "text": "hello"})
                .to_string();
        fs::write(&log, format!("{first}\n{second}\n")).expect("log");
        let adapter = WithLog(log.clone());
        let mut rig = Rig::new();
        rig.send(&adapter, &start("s", 4));
        assert_eq!(rig.thread("s").entries.len(), 2);
        let cursor = rig
            .store
            .cursor(fake::ID, "s")
            .expect("cursor")
            .expect("thread");
        assert_eq!(
            cursor.0,
            u64::try_from(first.len() + second.len() + 2).expect("len")
        );

        let third =
            json!({"event": "assistant", "session": "s", "id": "la2", "at": 7, "text": "more"})
                .to_string();
        fs::write(&log, format!("{first}\n{second}\n{third}\n")).expect("append");
        rig.send(&adapter, &start("s", 8));
        assert_eq!(rig.thread("s").entries.len(), 3);

        fs::write(&log, format!("{first}\n")).expect("shrink");
        rig.send(&adapter, &start("s", 9));
        let detail = rig.thread("s");
        assert_eq!(detail.entries.len(), 3);
        let cursor = rig
            .store
            .cursor(fake::ID, "s")
            .expect("cursor")
            .expect("thread");
        assert_eq!(cursor.0, u64::try_from(first.len() + 1).expect("len"));
    }

    #[test]
    fn a_batch_that_cannot_be_stored_waits_in_the_spool_and_the_next_run_stores_it() {
        let mut rig = Rig::new();
        let db = rig
            .dir
            .path()
            .join(".git")
            .join("gitmaster")
            .join("threads.db");
        let other = rusqlite::Connection::open(&db).expect("second connection");
        other
            .busy_timeout(std::time::Duration::from_millis(1))
            .expect("timeout");
        other.execute_batch("BEGIN IMMEDIATE").expect("lock");
        let outcome = rig.ingest(&prompt("s", "u1", 1, "spooled prompt"));
        assert_eq!(outcome, Outcome::Spooled);
        let spooled = spool_files(&spool_dir(&rig.dir.path().join(".git")));
        assert_eq!(spooled.len(), 1);
        other.execute_batch("COMMIT").expect("release");
        drop(other);

        let outcome = rig.ingest(&prompt("s", "u2", 2, "second prompt"));
        assert!(matches!(outcome, Outcome::Stored { inserted: 1, .. }));
        let detail = rig.thread("s");
        let texts: Vec<&str> = detail.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, vec!["spooled prompt", "second prompt"]);
        assert!(spool_files(&spool_dir(&rig.dir.path().join(".git"))).is_empty());
    }

    #[test]
    fn a_deferred_hook_spools_its_batch_and_the_next_run_stores_it() {
        let mut rig = Rig::new();
        let common = rig.dir.path().join(".git");
        let reader = ThreadStore::open_read_only(&common).expect("reader");
        let repo = rig.repo();
        let ctx = Context {
            config: &rig.config,
            now_ms: NOW,
        };
        let bytes = serde_json::to_vec(&prompt("s", "u1", 1, "deferred prompt")).expect("json");
        let outcome = ingest_deferred(&FakeAdapter, &bytes, &repo, &ctx, &reader);
        assert_eq!(outcome, Outcome::Spooled);
        assert_eq!(rig.store.total_threads().expect("count"), 0);
        assert_eq!(spool_files(&spool_dir(&common)).len(), 1);

        let outcome = rig.ingest(&prompt("s", "u2", 2, "second prompt"));
        assert!(matches!(outcome, Outcome::Stored { inserted: 1, .. }));
        let texts: Vec<String> = rig
            .thread("s")
            .entries
            .iter()
            .map(|entry| entry.text.clone())
            .collect();
        assert_eq!(texts, vec!["deferred prompt", "second prompt"]);
        assert!(spool_files(&spool_dir(&common)).is_empty());
    }

    #[test]
    fn a_spool_record_that_cannot_be_read_is_removed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let common = dir.path().join(".git");
        fs::create_dir_all(spool_dir(&common)).expect("dir");
        fs::write(
            spool_dir(&common).join("0000000000001-00000001.json"),
            b"{not json",
        )
        .expect("write");
        let mut store = ThreadStore::open(&common).expect("store");
        assert_eq!(drain_spool(&common, &mut store), 0);
        assert!(spool_files(&spool_dir(&common)).is_empty());
    }

    #[test]
    fn the_spool_keeps_at_most_its_limit_of_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let common = dir.path().join(".git");
        let record = Record {
            batch: base_batch(
                fake::ID,
                "s",
                &RepoInfo {
                    common_dir: common.clone(),
                    worktree: dir.path().to_path_buf(),
                    branch: None,
                    head: None,
                },
                NOW,
            ),
            edits: Vec::new(),
            turn_ended: false,
        };
        for n in 0..(SPOOL_KEEP + 5) {
            spool(&common, &record, NOW + i64::try_from(n).expect("n")).expect("spool");
        }
        assert_eq!(spool_files(&spool_dir(&common)).len(), SPOOL_KEEP);
    }

    #[test]
    fn an_invalid_pattern_stores_only_a_gap_and_names_the_pattern() {
        let mut rig = Rig::new();
        rig.config = ThreadsConfig::from_settings(&json!({
            "threads": {"redact": {"patterns": [{"name": "bad", "regex": "("}]}}
        }));
        let outcome = rig.ingest(&prompt("s", "u1", 1, "a prompt with text"));
        assert_eq!(
            outcome,
            Outcome::Dropped(DropReason::InvalidPattern(vec!["bad".to_owned()]))
        );
        let detail = rig.thread("s");
        assert!(detail.entries.is_empty());
        let gaps = rig
            .store
            .gaps(&ThreadStore::thread_uid(fake::ID, "s"))
            .expect("gaps");
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].reason, "invalid redaction pattern: bad");
    }

    #[test]
    fn capture_off_stores_nothing() {
        let mut rig = Rig::new();
        rig.config = ThreadsConfig::from_settings(&json!({"threads": {"capture": false}}));
        assert_eq!(
            rig.ingest(&start("s", 1)),
            Outcome::Dropped(DropReason::CaptureOff)
        );
        assert_eq!(rig.store.total_threads().expect("count"), 0);
    }

    #[test]
    fn an_unparseable_payload_is_dropped_and_logged_without_its_content() {
        let mut rig = Rig::new();
        let secret = token();
        let bytes = format!("{{\"event\": \"prompt\", \"text\": \"{secret}\"").into_bytes();
        assert_eq!(
            rig.send_bytes(&FakeAdapter, &bytes),
            Outcome::Dropped(DropReason::Unparseable)
        );
        let log =
            fs::read_to_string(rig.dir.path().join(".git/gitmaster/threads.log")).expect("log");
        assert!(log.contains("payload not read"));
        assert!(!log.contains(TOKEN_PREFIX));
        assert_eq!(rig.store.total_threads().expect("count"), 0);
    }

    #[test]
    fn a_payload_over_the_size_limit_is_not_read() {
        let mut rig = Rig::new();
        let bytes = vec![b' '; PAYLOAD_LIMIT + 1];
        assert_eq!(
            rig.send_bytes(&FakeAdapter, &bytes),
            Outcome::Dropped(DropReason::Unparseable)
        );
    }

    #[test]
    fn a_path_that_leaves_the_worktree_is_never_resolved() {
        let worktree = Path::new("/repo");
        assert_eq!(
            resolve(worktree, "src/lib.rs"),
            Some(PathBuf::from("/repo/src/lib.rs"))
        );
        assert_eq!(
            resolve(worktree, "/repo/a.txt"),
            Some(PathBuf::from("/repo/a.txt"))
        );
        assert_eq!(resolve(worktree, "../secret"), None);
        assert_eq!(resolve(worktree, "a/../../secret"), None);
        assert_eq!(resolve(worktree, "/etc/passwd"), None);
        assert_eq!(resolve(worktree, ""), None);
        assert_eq!(repo_relative("/repo/a/b.txt", worktree), "a/b.txt");
        assert_eq!(
            repo_relative("/elsewhere/b.txt", worktree),
            "/elsewhere/b.txt"
        );
    }

    #[test]
    fn clean_tmp_removes_only_pre_images_older_than_ten_minutes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let common = dir.path().join(".git");
        fs::create_dir_all(tmp_dir(&common)).expect("dir");
        let file = tmp_dir(&common).join("abc");
        fs::write(&file, b"old").expect("write");
        let now = system_ms(SystemTime::now());
        clean_tmp(&common, now + 60_000);
        assert!(file.exists());
        clean_tmp(&common, now + PRE_IMAGE_MAX_AGE_MS + 1_000);
        assert!(!file.exists());
    }

    #[test]
    fn the_log_rotates_at_its_size_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let common = dir.path().join(".git");
        let message = "x".repeat(LOG_MESSAGE_CHARS);
        for _ in 0..(LOG_ROTATE_BYTES / 200 + 2) {
            append_log(&common, &message);
        }
        append_log(&common, "after the rotation");
        assert!(gitmaster_dir(&common).join("threads.log.1").exists());
        let current = fs::read_to_string(gitmaster_dir(&common).join("threads.log")).expect("log");
        assert!(current.contains("after the rotation"));
    }

    #[test]
    fn locate_repo_is_none_outside_a_repository() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(locate_repo(dir.path()), None);
    }

    #[test]
    fn a_secret_in_a_prompt_is_not_in_the_database_file() {
        let mut rig = Rig::new();
        rig.ingest(&prompt("s", "u1", 1, &format!("secret {}", token())));
        rig.store.checkpoint().expect("checkpoint");
        let db = rig.dir.path().join(".git/gitmaster/threads.db");
        let bytes = fs::read(db).expect("db");
        assert!(
            !bytes
                .windows(TOKEN_PREFIX.len())
                .any(|w| w == TOKEN_PREFIX.as_bytes())
        );
    }
}
