//! Harness adapters (D129): the only code that knows a harness's shapes.
//!
//! An adapter turns one hook payload or one chunk of a harness's own session log into
//! `CanonicalEvent`s. Parsing is pure; redaction, storage and the window never see a harness
//! shape. See `design/threads.md`, "Harnesses and the canonical model".

pub mod claude_code;
#[cfg(test)]
pub mod conformance;
pub mod fake;

use std::fmt::{self, Write as _};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use gm_schema::threads::{CanonicalEvent, EditPhase, EntryKind};
use sha2::{Digest, Sha256};

use crate::threads::store::Cleaned;

/// The most bytes one `read_session_log` call reads from a transcript.
pub const MAX_LOG_READ: usize = 8 * 1024 * 1024;

/// What a harness can provide. The pipeline degrades per harness instead of failing.
#[allow(clippy::struct_excessive_bools)] // one flag per capability in design/threads.md
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// A transcript the harness writes, to read assistant text and reconcile from.
    pub session_log: bool,
    /// A hook fires before an edit, so a pre-image is possible.
    pub pre_edit_event: bool,
    /// Hooks carry tool results.
    pub tool_results: bool,
    /// Sub-agent calls are reported with their parent call.
    pub subagents: bool,
    /// A resumed session keeps its harness session id.
    pub resume_same_id: bool,
}

/// A payload or log line that the adapter cannot read. The message is for the capture log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// An edit phase reported by a hook: before or after the tool ran, for one call and path.
#[derive(Clone, Debug, PartialEq)]
pub struct EditPhaseInfo {
    pub phase: EditPhase,
    /// The `source_id` of the tool call the edit belongs to.
    pub call_source_id: String,
    pub path: String,
}

/// One hook invocation, parsed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HookInput {
    pub session_id: String,
    pub cwd: Option<String>,
    /// The harness's session log, when the payload names it.
    pub log_path: Option<String>,
    pub events: Vec<CanonicalEvent>,
    pub edit_phase: Option<EditPhaseInfo>,
    /// Native event or record type names that were skipped.
    pub unrecognized: Vec<String>,
}

/// The complete records read from a session log, and where the next read starts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogChunk {
    pub events: Vec<CanonicalEvent>,
    /// Byte offset just after the last complete line read.
    pub next_offset: u64,
    /// Native record type names that were skipped.
    pub unrecognized: Vec<String>,
}

/// An existing session transcript found for import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionLog {
    pub path: PathBuf,
    pub session_id: String,
}

/// One harness. Implementations live beside this file, one module each.
pub trait HarnessAdapter: Send + Sync {
    /// Stable id stored with every thread, such as `claude-code`.
    fn id(&self) -> &'static str;

    /// Human-readable name for the UI and the CLI.
    fn display_name(&self) -> &'static str;

    fn capabilities(&self) -> Capabilities;

    /// One hook invocation: the stdin document to canonical events. Pure.
    fn parse_hook(&self, payload: &[u8]) -> Result<HookInput, ParseError>;

    /// Complete records of the harness's own session log from byte offset `from`. Reads the
    /// file, bounded to `MAX_LOG_READ` per call; a trailing partial line is left for next time.
    fn read_session_log(&self, path: &Path, from: u64) -> Result<LogChunk, ParseError>;

    /// Existing sessions for a repository's worktree, for import. May be empty.
    fn discover(&self, worktree: &Path) -> Vec<SessionLog>;

    /// The cleaned text of a stored entry, for the schema 4 repair (D133): the harness markers
    /// removed from a `user` or `tool_result` entry. `None` when there is no rule for the kind.
    fn clean_stored_text(&self, _kind: EntryKind, _text: &str) -> Option<Cleaned> {
        None
    }
}

/// The known adapters. Empty until a harness is added: a harness is one line here.
static REGISTRY: &[&dyn HarnessAdapter] = &[&claude_code::ClaudeCodeAdapter];

/// Every registered harness id, sorted.
#[must_use]
pub fn known_ids() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = REGISTRY.iter().map(|adapter| adapter.id()).collect();
    ids.sort_unstable();
    ids
}

/// The adapter registered under `id`, if any.
#[must_use]
pub fn lookup(id: &str) -> Option<&'static dyn HarnessAdapter> {
    REGISTRY.iter().copied().find(|adapter| adapter.id() == id)
}

/// The cleaner the store runs over stored entries (D133): dispatches on the harness id to its
/// adapter. An unknown harness has no rule, so its entries are left as they are.
#[must_use]
pub fn clean_stored(harness: &str, kind: EntryKind, text: &str) -> Option<Cleaned> {
    lookup(harness)?.clean_stored_text(kind, text)
}

/// The `source_id` of an item the harness gives no id for: the first 24 hex digits of
/// `sha256(harness NUL session NUL position)`, so a replay of the same item is idempotent.
#[must_use]
pub fn stable_source_id(harness: &str, session_id: &str, position: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(harness.as_bytes());
    hasher.update([0u8]);
    hasher.update(session_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(position.to_string().as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(24);
    for byte in &digest[..12] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Reads up to `max` bytes of `path` from `from` and returns its complete lines, each with
/// its start offset, and the offset just after the last complete line.
///
/// A trailing line without its `\n` is left out, so the next call from the returned offset
/// reads it whole. Empty lines are skipped; a trailing `\r` is removed; bytes are decoded
/// lossily as UTF-8. When `from` is past the end of the file the log has shrunk: the result is
/// no lines and offset 0, so the caller starts again.
///
/// # Errors
///
/// Fails when the file cannot be opened or read, or when a line is longer than `max` bytes.
pub fn read_complete_lines(
    path: &Path,
    from: u64,
    max: usize,
) -> Result<(Vec<(u64, String)>, u64), ParseError> {
    let describe = |what: &str, err: std::io::Error| {
        ParseError(format!("cannot {what} {}: {err}", path.display()))
    };
    let mut file = File::open(path).map_err(|err| describe("open", err))?;
    let len = file.metadata().map_err(|err| describe("stat", err))?.len();
    if from > len {
        return Ok((Vec::new(), 0));
    }
    file.seek(SeekFrom::Start(from))
        .map_err(|err| describe("seek", err))?;
    let mut buf = Vec::new();
    file.take(to_u64(max))
        .read_to_end(&mut buf)
        .map_err(|err| describe("read", err))?;

    let Some(last_newline) = buf.iter().rposition(|&byte| byte == b'\n') else {
        if buf.len() >= max {
            return Err(ParseError(format!(
                "line at byte {from} of {} is longer than {max} bytes",
                path.display()
            )));
        }
        return Ok((Vec::new(), from));
    };

    let complete = &buf[..=last_newline];
    let mut lines = Vec::new();
    let mut offset = from;
    for raw in complete.split(|&byte| byte == b'\n') {
        let start = offset;
        offset += to_u64(raw.len()) + 1;
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        if !line.is_empty() {
            lines.push((start, String::from_utf8_lossy(line).into_owned()));
        }
    }
    Ok((lines, from + to_u64(complete.len())))
}

fn to_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).expect("write fixture file");
        path
    }

    #[test]
    fn stable_source_id_is_deterministic_and_position_sensitive() {
        let first = stable_source_id("claude-code", "s1", 7);
        assert_eq!(first, stable_source_id("claude-code", "s1", 7));
        assert_eq!(first.len(), 24);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(first, stable_source_id("claude-code", "s1", 8));
        assert_ne!(first, stable_source_id("claude-code", "s2", 7));
        assert_ne!(first, stable_source_id("other", "s1", 7));
    }

    #[test]
    fn stable_source_id_separates_fields_so_they_cannot_run_together() {
        // "ab" + "c" must not collide with "a" + "bc".
        assert_ne!(
            stable_source_id("ab", "c", 0),
            stable_source_id("a", "bc", 0)
        );
    }

    #[test]
    fn known_ids_is_sorted_and_lookup_misses_unknown_ids() {
        let ids = known_ids();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
        assert!(lookup("no-such-harness").is_none());
    }

    #[test]
    fn partial_last_line_is_left_for_the_next_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "a.log", b"a\nb\nc");
        let (lines, next) = read_complete_lines(&path, 0, 1024).expect("read");
        assert_eq!(lines, vec![(0, "a".to_owned()), (2, "b".to_owned())]);
        assert_eq!(next, 4);
    }

    #[test]
    fn offsets_count_empty_lines_and_resume_from_the_middle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "a.log", b"one\n\ntwo\n");
        let (lines, next) = read_complete_lines(&path, 0, 1024).expect("read");
        assert_eq!(lines, vec![(0, "one".to_owned()), (5, "two".to_owned())]);
        assert_eq!(next, 9);

        let (lines, next) = read_complete_lines(&path, 4, 1024).expect("read");
        assert_eq!(lines, vec![(5, "two".to_owned())]);
        assert_eq!(next, 9);
    }

    #[test]
    fn reading_at_the_end_returns_nothing_and_keeps_the_offset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "a.log", b"x\n");
        assert_eq!(
            read_complete_lines(&path, 2, 1024).expect("read"),
            (vec![], 2)
        );
    }

    #[test]
    fn a_shrunk_log_restarts_from_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "a.log", b"x\n");
        assert_eq!(
            read_complete_lines(&path, 500, 1024).expect("read"),
            (vec![], 0)
        );
    }

    #[test]
    fn carriage_returns_are_removed_and_invalid_utf8_is_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "a.log", b"a\xff\r\n\r\n");
        let (lines, next) = read_complete_lines(&path, 0, 1024).expect("read");
        assert_eq!(lines, vec![(0, "a\u{FFFD}".to_owned())]);
        assert_eq!(next, 6);
    }

    #[test]
    fn one_call_never_reads_past_the_bound() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "a.log", b"aaaa\nbbbb\ncccc\n");
        let (lines, next) = read_complete_lines(&path, 0, 10).expect("read");
        assert_eq!(lines, vec![(0, "aaaa".to_owned()), (5, "bbbb".to_owned())]);
        assert_eq!(next, 10);
    }

    #[test]
    fn a_line_longer_than_the_bound_is_an_error_not_a_stall() {
        let dir = tempfile::tempdir().expect("tempdir");
        let line = vec![b'x'; MAX_LOG_READ];
        let path = write(&dir, "huge.log", &line);
        let err = read_complete_lines(&path, 0, MAX_LOG_READ).expect_err("too long");
        assert!(err.0.contains("longer than"), "{err}");
    }

    #[test]
    fn a_missing_file_is_a_parse_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(read_complete_lines(&dir.path().join("none.log"), 0, 16).is_err());
    }
}
