//! The engine's side of threads (design/threads.md, "Engine surface"): the store of
//! the open repository, the reads and writes of threads, the Markdown export and the
//! scrub. The commands are declared in `gm-schema`; `engine` routes them here.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use gm_schema::threads::{
    EditChange, EditState, EntryKind, ThreadDetail, ThreadEdit, ThreadFilter,
};
use gm_schema::{CommandKind, Event, OperationId};

use crate::GitPort;
use crate::threads::config::ThreadsConfig;
use crate::threads::redact::Redactor;
use crate::threads::store::{MetaUpdate, StoreError, ThreadStore};

/// The most rows one read returns; a larger `limit` is cut to this.
pub(crate) const MAX_PAGE: u32 = 500;

const THREAD_MISSING: &str = "Thread not found.";

/// The store of one open repository: opened on the first thread command, and
/// replaced when another repository opens.
pub(crate) struct ThreadsSession {
    pub(crate) root: PathBuf,
    pub(crate) store: ThreadStore,
}

/// The store of the open repository: opened on first use, and reopened when
/// another repository is open. It lives in `.git/gitmaster/threads.db`.
pub(crate) fn store<'a>(
    slot: &'a mut Option<ThreadsSession>,
    git: &dyn GitPort,
    root: &Path,
) -> Result<&'a mut ThreadStore, String> {
    if slot.as_ref().is_none_or(|session| session.root != root) {
        let common = git.common_dir(root).map_err(|e| e.0)?;
        let store = ThreadStore::open(&common).map_err(|e| e.to_string())?;
        *slot = Some(ThreadsSession {
            root: root.to_path_buf(),
            store,
        });
    }
    match slot {
        Some(session) => Ok(&mut session.store),
        None => Err("the threads store is not open".to_owned()),
    }
}

/// What a command reads besides the store: the repository, and the settings file
/// whose `threads` key holds the redaction patterns.
pub(crate) struct Context<'a> {
    pub(crate) git: &'a dyn GitPort,
    pub(crate) root: &'a Path,
    /// `None` takes the default configuration.
    pub(crate) settings: Option<&'a Path>,
}

/// What a thread command needs a human for, or `None` when an agent may run it.
/// The reads are open to an agent (design/threads.md, "Engine surface").
pub(crate) fn human_only(kind: &CommandKind) -> Option<&'static str> {
    Some(match kind {
        CommandKind::ExportThread { .. } => "export a thread",
        CommandKind::UpdateThread { .. } => "change a thread",
        CommandKind::LinkThread { .. } => "link a thread",
        CommandKind::UnlinkThread { .. } => "unlink a thread",
        CommandKind::DeleteThread { .. } => "delete a thread",
        CommandKind::DeleteThreadEntry { .. } => "delete a thread entry",
        CommandKind::ScrubThreads => "scrub the threads",
        _ => return None,
    })
}

/// Whether `kind` is one of the thread commands.
pub(crate) fn is_threads_command(kind: &CommandKind) -> bool {
    matches!(
        kind,
        CommandKind::ListThreads { .. }
            | CommandKind::SearchThreads { .. }
            | CommandKind::ReadThread { .. }
            | CommandKind::ReadThreadEdit { .. }
            | CommandKind::ExportThread { .. }
            | CommandKind::UpdateThread { .. }
            | CommandKind::LinkThread { .. }
            | CommandKind::UnlinkThread { .. }
            | CommandKind::DeleteThread { .. }
            | CommandKind::DeleteThreadEntry { .. }
            | CommandKind::ScrubThreads
    )
}

/// Runs one thread command against `store`. Writes return `ThreadsChanged` after
/// their own answer; the caller has checked the actor.
pub(crate) fn run(
    id: OperationId,
    store: &mut ThreadStore,
    ctx: &Context<'_>,
    kind: &CommandKind,
) -> Result<Vec<Event>, String> {
    match kind {
        CommandKind::ListThreads {
            filter,
            before_at,
            before_uid,
            limit,
        } => {
            let before = cursor(*before_at, before_uid.as_deref())?;
            let threads = store
                .list_threads(filter, before, page(*limit)?)
                .map_err(|e| e.to_string())?;
            let total = store.total_threads().map_err(|e| e.to_string())?;
            Ok(vec![Event::ThreadsListed { id, threads, total }])
        }
        CommandKind::SearchThreads {
            query,
            filter,
            limit,
        } => {
            let hits = store
                .search(query, filter, page(*limit)?)
                .map_err(|e| e.to_string())?;
            Ok(vec![Event::ThreadSearchResults { id, hits }])
        }
        CommandKind::ReadThread {
            uid,
            from_seq,
            limit,
        } => {
            let detail = store
                .read_thread(uid, *from_seq, page(*limit)?)
                .map_err(|e| named(e, THREAD_MISSING))?;
            Ok(vec![Event::ThreadRead { id, detail }])
        }
        CommandKind::ReadThreadEdit { id: edit_id } => {
            let edit = store
                .read_edit(*edit_id)
                .map_err(|e| named(e, "Edit not found."))?;
            let state = edit_state(ctx, &edit);
            Ok(vec![Event::ThreadEditRead { id, edit, state }])
        }
        CommandKind::ExportThread { uid, path } => export(id, store, ctx, uid, path),
        CommandKind::UpdateThread { .. }
        | CommandKind::LinkThread { .. }
        | CommandKind::UnlinkThread { .. }
        | CommandKind::DeleteThread { .. }
        | CommandKind::DeleteThreadEntry { .. }
        | CommandKind::ScrubThreads => write(id, store, ctx, kind),
        _ => Err("not a thread command".to_owned()),
    }
}

/// Writes thread `uid` as Markdown to `path`, redacted again.
fn export(
    id: OperationId,
    store: &mut ThreadStore,
    ctx: &Context<'_>,
    uid: &str,
    path: &str,
) -> Result<Vec<Event>, String> {
    let target = export_target(ctx.root, path)?;
    let redactor = redactor(ctx)?;
    let detail = store
        .read_thread(uid, 0, u32::MAX)
        .map_err(|e| named(e, THREAD_MISSING))?;
    let text = redactor.redact(&markdown(&detail)).text;
    std::fs::write(&target, text).map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    Ok(vec![Event::ThreadExported {
        id,
        path: target.to_string_lossy().into_owned(),
    }])
}

/// The commands that change the store. Each answers `ThreadsChanged`.
fn write(
    id: OperationId,
    store: &mut ThreadStore,
    ctx: &Context<'_>,
    kind: &CommandKind,
) -> Result<Vec<Event>, String> {
    match kind {
        CommandKind::UpdateThread {
            uid,
            title,
            note,
            starred,
            tags,
        } => {
            let meta = MetaUpdate {
                title: title.clone(),
                note: note.clone(),
                starred: *starred,
                tags: tags.clone(),
            };
            store
                .update_meta(uid, &meta)
                .map_err(|e| named(e, THREAD_MISSING))?;
            Ok(vec![Event::ThreadsChanged])
        }
        CommandKind::LinkThread { uid, link } => {
            store
                .link(uid, link)
                .map_err(|e| named(e, THREAD_MISSING))?;
            Ok(vec![Event::ThreadsChanged])
        }
        CommandKind::UnlinkThread {
            uid,
            kind,
            reference,
        } => {
            store
                .unlink(uid, *kind, reference)
                .map_err(|e| e.to_string())?;
            Ok(vec![Event::ThreadsChanged])
        }
        CommandKind::DeleteThread { uid } => {
            store
                .delete_thread(uid)
                .map_err(|e| named(e, THREAD_MISSING))?;
            Ok(vec![Event::ThreadsChanged])
        }
        CommandKind::DeleteThreadEntry { id: entry } => {
            store
                .delete_entry(*entry)
                .map_err(|e| named(e, "Entry not found."))?;
            Ok(vec![Event::ThreadsChanged])
        }
        CommandKind::ScrubThreads => scrub(id, store, ctx),
        _ => Err("not a write command".to_owned()),
    }
}

/// The message of a store error, where `NotFound` names what was missing.
fn named(error: StoreError, missing: &str) -> String {
    match error {
        StoreError::NotFound => missing.to_owned(),
        other => other.to_string(),
    }
}

/// The `(last_event_at, uid)` cursor of a page: both or neither.
fn cursor(at: Option<i64>, uid: Option<&str>) -> Result<Option<(i64, String)>, String> {
    match (at, uid) {
        (None, None) => Ok(None),
        (Some(at), Some(uid)) => Ok(Some((at, uid.to_owned()))),
        _ => Err("a page needs both before_at and before_uid, or neither".to_owned()),
    }
}

/// A read's limit: at least one row, at most `MAX_PAGE`.
fn page(limit: u32) -> Result<u32, String> {
    if limit == 0 {
        Err("the limit must be at least 1".to_owned())
    } else {
        Ok(limit.min(MAX_PAGE))
    }
}

/// The redaction patterns of the settings file, compiled. Invalid patterns refuse
/// the command, as capture does (design/threads.md, "Redaction").
fn redactor(ctx: &Context<'_>) -> Result<Redactor, String> {
    let config = ctx
        .settings
        .map_or_else(ThreadsConfig::default, ThreadsConfig::load);
    Redactor::new(&config.redact).map_err(|problems| {
        let listed: Vec<String> = problems
            .iter()
            .map(|problem| format!("{}: {}", problem.name, problem.message))
            .collect();
        format!("the redaction patterns are invalid: {}", listed.join("; "))
    })
}

/// Applies the patterns to every stored text, then to every thread's title and
/// note, and checkpoints the database so the old text leaves the WAL too.
fn scrub(
    id: OperationId,
    store: &mut ThreadStore,
    ctx: &Context<'_>,
) -> Result<Vec<Event>, String> {
    let redactor = redactor(ctx)?;
    let mut changed = store
        .rewrite_texts(&|text: &str| {
            let out = redactor.redact(text);
            (out.text != text).then_some(out.text)
        })
        .map_err(|e| e.to_string())?;
    let mut before = None;
    loop {
        let page = store
            .list_threads(&ThreadFilter::default(), before.clone(), MAX_PAGE)
            .map_err(|e| e.to_string())?;
        let Some(last) = page.last() else {
            break;
        };
        before = Some((last.last_event_at, last.uid.clone()));
        for thread in &page {
            let title = redactor.redact(&thread.title).text;
            let note = redactor.redact(&thread.note).text;
            let title_changed = title != thread.title;
            let note_changed = note != thread.note;
            if title_changed || note_changed {
                let meta = MetaUpdate {
                    title: title_changed.then_some(title),
                    note: note_changed.then_some(note),
                    ..MetaUpdate::default()
                };
                store
                    .update_meta(&thread.uid, &meta)
                    .map_err(|e| e.to_string())?;
                changed += 1;
            }
        }
    }
    store.checkpoint().map_err(|e| e.to_string())?;
    Ok(vec![
        Event::ThreadsScrubbed { id, changed },
        Event::ThreadsChanged,
    ])
}

/// Where an export may be written: an absolute path whose folder exists and is
/// outside the open worktree. A folder at the path is refused.
fn export_target(root: &Path, path: &str) -> Result<PathBuf, String> {
    let target = Path::new(path);
    if !target.is_absolute() {
        return Err(format!("the export path must be absolute: {path}"));
    }
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(format!("not a file path: {path}"));
    };
    let real_parent = parent
        .canonicalize()
        .map_err(|_| format!("the folder {} does not exist", parent.display()))?;
    if let Ok(base) = root.canonicalize()
        && real_parent.starts_with(&base)
    {
        return Err(format!(
            "the export must not be inside the worktree: {path}"
        ));
    }
    let full = real_parent.join(name);
    if full.is_dir() {
        return Err(format!("{} is a folder", full.display()));
    }
    Ok(full)
}

/// The derived state of an edit (design/threads.md, "Links to Git"). `Unknown`
/// when the edit was not measured or a file cannot be read.
fn edit_state(ctx: &Context<'_>, edit: &ThreadEdit) -> EditState {
    if edit.post_oid.is_none() {
        return EditState::Unknown;
    }
    let working = working_blob(ctx.root, &edit.path);
    let head = head_blob(ctx.git, ctx.root, &edit.path);
    derive_state(edit.post_oid.as_deref(), working, head)
}

/// The state from the post blob id and the two blob ids read: `working` is the
/// file in the working tree (`None` when it is absent) and `head` the blob at
/// `HEAD`. An `Err` is a read that failed.
pub(crate) fn derive_state(
    post: Option<&str>,
    working: Result<Option<String>, ()>,
    head: Result<Option<String>, ()>,
) -> EditState {
    let Some(post) = post else {
        return EditState::Unknown;
    };
    let Ok(working) = working else {
        return EditState::Unknown;
    };
    if working.as_deref() == Some(post) {
        return EditState::InWorkingTree;
    }
    let Ok(head) = head else {
        return EditState::Unknown;
    };
    if head.as_deref() == Some(post) {
        EditState::InHead
    } else {
        EditState::ChangedSince
    }
}

fn blob_id(bytes: &[u8]) -> Result<String, ()> {
    git2::Oid::hash_object(git2::ObjectType::Blob, bytes)
        .map(|oid| oid.to_string())
        .map_err(|_| ())
}

/// The blob id of `path` in the working tree; `None` when there is no such file.
fn working_blob(root: &Path, path: &str) -> Result<Option<String>, ()> {
    let relative = Path::new(path);
    if path.is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(());
    }
    match std::fs::read(root.join(relative)) {
        Ok(bytes) => blob_id(&bytes).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

/// The blob id of `path` at `HEAD`; `None` when `HEAD` has no such file.
fn head_blob(git: &dyn GitPort, root: &Path, path: &str) -> Result<Option<String>, ()> {
    match git.file_at_commit(root, "HEAD", path) {
        Ok(Some(bytes)) => blob_id(&bytes).map(Some),
        Ok(None) => Ok(None),
        Err(_) => Err(()),
    }
}

/// The thread as Markdown: a title, a metadata list, then each turn with the
/// prompt, the agent's text, its tool calls, and the edits as fenced diffs.
pub(crate) fn markdown(detail: &ThreadDetail) -> String {
    let summary = &detail.summary;
    let mut out = format!("# {}\n\n", summary.title.replace('\n', " "));
    let meta = [
        ("Agent", summary.harness.clone()),
        ("Session", summary.session_id.clone()),
        ("Worktree", summary.worktree.clone()),
        (
            "Branch",
            summary
                .branch
                .clone()
                .unwrap_or_else(|| "detached".to_owned()),
        ),
        (
            "Model",
            summary
                .model
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
        ),
        ("Started", utc(summary.started_at)),
        ("Last event", utc(summary.last_event_at)),
        (
            "Ended",
            summary
                .ended_at
                .map_or_else(|| "still open".to_owned(), utc),
        ),
    ];
    for (label, value) in meta {
        let _ = writeln!(out, "- {label}: {value}");
    }
    if !summary.tags.is_empty() {
        let _ = writeln!(out, "- Tags: {}", summary.tags.join(", "));
    }
    if !summary.note.trim().is_empty() {
        let _ = writeln!(out, "- Note: {}", summary.note.trim().replace('\n', " "));
    }
    out.push('\n');

    let mut edits: BTreeMap<i64, Vec<&ThreadEdit>> = BTreeMap::new();
    for edit in &detail.edits {
        edits.entry(edit.entry_id).or_default().push(edit);
    }
    let mut turn = 0;
    // A list item is followed by a blank line before any other block, or the
    // block would read as part of the item.
    let mut after_item = false;
    for entry in &detail.entries {
        if entry.turn != turn {
            turn = entry.turn;
            let _ = write!(out, "\n## Turn {turn}\n\n");
            after_item = false;
        }
        let item = matches!(entry.kind, EntryKind::ToolCall | EntryKind::ToolResult);
        if !item && after_item {
            out.push('\n');
        }
        match entry.kind {
            EntryKind::User => {
                let _ = write!(out, "**You:**\n\n{}\n\n", entry.text.trim_end());
            }
            EntryKind::Assistant => {
                let _ = write!(out, "**Agent:**\n\n{}\n\n", entry.text.trim_end());
            }
            EntryKind::ToolCall => {
                let tool = entry.tool.as_deref().unwrap_or("tool");
                let source = if entry.text.trim().is_empty() {
                    entry.args_json.as_deref().unwrap_or("")
                } else {
                    &entry.text
                };
                let _ = writeln!(out, "- {tool}: {}", first_line(source, 200));
            }
            EntryKind::ToolResult => {
                let failed = if entry.is_error { " (error)" } else { "" };
                let _ = writeln!(out, "  - result{failed}: {}", first_line(&entry.text, 200));
            }
            EntryKind::Notice => {
                let _ = write!(out, "> {}\n\n", entry.text.trim().replace('\n', "\n> "));
            }
        }
        after_item = item;
        if let Some(list) = edits.get(&entry.id) {
            for edit in list {
                if after_item {
                    out.push('\n');
                }
                out.push_str(&edit_block(edit));
                after_item = false;
            }
        }
    }
    out
}

/// One edit: a line naming the file and its counts, then the patch fenced so that
/// no backtick run in the patch closes the fence.
fn edit_block(edit: &ThreadEdit) -> String {
    let change = match edit.change {
        EditChange::Create => "created",
        EditChange::Modify => "modified",
        EditChange::Delete => "deleted",
        EditChange::Rename => "renamed",
    };
    let measured = if edit.from_input {
        " (from the tool input, not measured)"
    } else {
        ""
    };
    let fence = "`".repeat((longest_backtick_run(&edit.patch) + 1).max(3));
    format!(
        "**Edit** `{}` {change}: +{} -{}{measured}\n\n{fence}diff\n{}\n{fence}\n\n",
        edit.path,
        edit.added,
        edit.removed,
        edit.patch.trim_end_matches('\n')
    )
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

/// The first non-blank line of `text`, cut to `max` characters.
fn first_line(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut short: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        short.push_str("...");
    }
    short
}

/// Milliseconds since the Unix epoch, as `YYYY-MM-DD HH:MM:SS UTC`.
fn utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let in_day = secs.rem_euclid(86_400);
    // Civil date from a day count since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let (hour, minute, second) = (in_day / 3600, in_day % 3600 / 60, in_day % 60);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use crate::fake::FakeGit;
    use crate::git::SubprocessGit;
    use crate::threads::store::{Batch, NewEdit, NewEntry};
    use gm_schema::threads::{LinkKind, LinkOrigin, ThreadLink, ThreadSummary};
    use gm_schema::{Actor, Command, OperationState, RepoStatus};

    fn human() -> Actor {
        Actor::Human
    }

    fn agent() -> Actor {
        Actor::Agent {
            name: "agent".into(),
        }
    }

    fn send(engine: &mut Engine, actor: &Actor, kind: CommandKind) -> Vec<Event> {
        engine.handle(
            OperationId(1),
            &Command {
                actor: actor.clone(),
                kind,
            },
        )
    }

    fn state_of(events: &[Event]) -> OperationState {
        events
            .iter()
            .rev()
            .find_map(|event| match event {
                Event::OperationChanged { state, .. } => Some(*state),
                _ => None,
            })
            .expect("an operation state")
    }

    /// The detail of the terminal state, when the operation failed.
    fn failure(events: &[Event]) -> Option<String> {
        events.iter().rev().find_map(|event| match event {
            Event::OperationChanged { detail, .. } => detail.clone(),
            _ => None,
        })
    }

    /// The first event that is not an operation state.
    fn answer(events: &[Event]) -> &Event {
        events
            .iter()
            .find(|event| !matches!(event, Event::OperationChanged { .. }))
            .expect("an answer")
    }

    fn has_changed(events: &[Event]) -> bool {
        events
            .iter()
            .any(|event| matches!(event, Event::ThreadsChanged))
    }

    fn text_entry(source: &str, kind: EntryKind, text: &str, at: i64) -> NewEntry {
        NewEntry {
            source_id: source.into(),
            kind,
            parent_id: None,
            tool: None,
            text: text.into(),
            args_json: None,
            is_error: false,
            truncated: false,
            orig_bytes: None,
            redactions: 0,
            hits: Vec::new(),
            at,
            edit: None,
        }
    }

    fn edit_entry(source: &str, path: &str, post: Option<String>, at: i64) -> NewEntry {
        let mut entry = text_entry(source, EntryKind::ToolCall, "Edit", at);
        entry.tool = Some("Edit".into());
        entry.edit = Some(NewEdit {
            path: path.into(),
            old_path: None,
            change: EditChange::Modify,
            patch: "@@ -1 +1 @@\n-one\n+two\n".into(),
            added: 1,
            removed: 1,
            pre_oid: None,
            post_oid: post,
            from_input: false,
            hits: Vec::new(),
        });
        entry
    }

    fn batch(session: &str, branch: &str, started: i64, entries: Vec<NewEntry>) -> Batch {
        Batch {
            harness: "claude-code".into(),
            session_id: session.into(),
            worktree: "/r".into(),
            branch: Some(branch.into()),
            head: None,
            model: Some("claude".into()),
            harness_version: None,
            transcript: None,
            cursor: None,
            started_at: started,
            at: started + 10,
            ended: None,
            entries,
            gaps: Vec::new(),
        }
    }

    /// Three threads: `a` on main with an edit and a secret-looking ticket, `b` on
    /// a feature branch, `c` on main and the newest. Returns their uids in that order.
    fn fill(store: &mut ThreadStore) -> [String; 3] {
        let mut a_entries = vec![
            text_entry(
                "u1",
                EntryKind::User,
                "retry with backoff for the client",
                1000,
            ),
            text_entry(
                "a1",
                EntryKind::Assistant,
                "Added backoff in client.rs",
                1001,
            ),
            edit_entry("t1", "src/client.rs", None, 1002),
        ];
        let mut result = text_entry("r1", EntryKind::ToolResult, "ok", 1003);
        result.tool = Some("Edit".into());
        a_entries.push(result);
        a_entries.push(text_entry(
            "u2",
            EntryKind::User,
            "now write the test",
            1004,
        ));
        a_entries.push(text_entry(
            "a2",
            EntryKind::Assistant,
            "Test added for TICKET-42",
            1005,
        ));
        let a = store
            .apply(&batch("a", "main", 1000, a_entries))
            .expect("apply a")
            .uid;
        let b = store
            .apply(&batch(
                "b",
                "feature",
                2000,
                vec![
                    text_entry(
                        "u3",
                        EntryKind::User,
                        "fix the parser crash on empty input",
                        2000,
                    ),
                    text_entry("a3", EntryKind::Assistant, "Fixed", 2001),
                ],
            ))
            .expect("apply b")
            .uid;
        let c = store
            .apply(&batch(
                "c",
                "main",
                3000,
                vec![text_entry("u4", EntryKind::User, "rename helper", 3000)],
            ))
            .expect("apply c")
            .uid;
        [a, b, c]
    }

    /// A repository root with an open store and the threads of `fill`.
    fn repository() -> (tempfile::TempDir, PathBuf, [String; 3]) {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().to_path_buf();
        let mut store = ThreadStore::open(&root.join(".git")).expect("open");
        let uids = fill(&mut store);
        (dir, root, uids)
    }

    fn engine_over(root: &Path) -> Engine {
        let status = RepoStatus {
            root: root.to_string_lossy().into_owned(),
            ..RepoStatus::default()
        };
        let mut engine = Engine::new(Box::new(FakeGit::new(root, status)));
        let open = Command {
            actor: human(),
            kind: CommandKind::OpenRepo {
                path: root.to_string_lossy().into_owned(),
            },
        };
        engine.handle(OperationId(1), &open);
        engine
    }

    fn list(engine: &mut Engine, filter: ThreadFilter) -> Vec<ThreadSummary> {
        let events = send(
            engine,
            &human(),
            CommandKind::ListThreads {
                filter,
                before_at: None,
                before_uid: None,
                limit: 50,
            },
        );
        match answer(&events) {
            Event::ThreadsListed { threads, .. } => threads.clone(),
            other => panic!("expected threads, got {other:?}"),
        }
    }

    fn read(engine: &mut Engine, uid: &str) -> ThreadDetail {
        let events = send(
            engine,
            &human(),
            CommandKind::ReadThread {
                uid: uid.into(),
                from_seq: 0,
                limit: 100,
            },
        );
        match answer(&events) {
            Event::ThreadRead { detail, .. } => detail.clone(),
            other => panic!("expected a thread, got {other:?}"),
        }
    }

    #[test]
    fn listing_pages_and_filters_follow_the_store() {
        let (_dir, root, [a, b, c]) = repository();
        let mut engine = engine_over(&root);

        let first = send(
            &mut engine,
            &human(),
            CommandKind::ListThreads {
                filter: ThreadFilter::default(),
                before_at: None,
                before_uid: None,
                limit: 2,
            },
        );
        let Event::ThreadsListed { threads, total, .. } = answer(&first) else {
            panic!("expected threads");
        };
        assert_eq!(*total, 3);
        assert_eq!(
            threads.iter().map(|t| t.uid.clone()).collect::<Vec<_>>(),
            vec![c.clone(), b.clone()]
        );

        let last = threads.last().expect("a last row");
        let next = send(
            &mut engine,
            &human(),
            CommandKind::ListThreads {
                filter: ThreadFilter::default(),
                before_at: Some(last.last_event_at),
                before_uid: Some(last.uid.clone()),
                limit: 2,
            },
        );
        let Event::ThreadsListed { threads, .. } = answer(&next) else {
            panic!("expected threads");
        };
        assert_eq!(
            threads.iter().map(|t| t.uid.clone()).collect::<Vec<_>>(),
            vec![a.clone()]
        );

        let feature = ThreadFilter {
            branch: Some("feature".into()),
            ..ThreadFilter::default()
        };
        let branch_uids: Vec<String> = list(&mut engine, feature)
            .into_iter()
            .map(|thread| thread.uid)
            .collect();
        assert_eq!(branch_uids, vec![b.clone()]);
        let edited = ThreadFilter {
            has_edits: Some(true),
            ..ThreadFilter::default()
        };
        assert_eq!(list(&mut engine, edited).len(), 1);
        let touched = ThreadFilter {
            path: Some("src/client.rs".into()),
            ..ThreadFilter::default()
        };
        assert_eq!(list(&mut engine, touched)[0].uid, a);
        let other_harness = ThreadFilter {
            harness: Some("codex".into()),
            ..ThreadFilter::default()
        };
        assert!(list(&mut engine, other_harness).is_empty());
    }

    #[test]
    fn a_cursor_needs_both_halves() {
        let (_dir, root, _) = repository();
        let mut engine = engine_over(&root);
        let events = send(
            &mut engine,
            &human(),
            CommandKind::ListThreads {
                filter: ThreadFilter::default(),
                before_at: Some(5),
                before_uid: None,
                limit: 2,
            },
        );
        assert_eq!(state_of(&events), OperationState::Failed);
        assert_eq!(
            failure(&events).as_deref(),
            Some("a page needs both before_at and before_uid, or neither")
        );
    }

    #[test]
    fn search_finds_entries_and_a_thread_reads_in_pages() {
        let (_dir, root, [a, _, _]) = repository();
        let mut engine = engine_over(&root);
        let found = send(
            &mut engine,
            &human(),
            CommandKind::SearchThreads {
                query: "backoff".into(),
                filter: ThreadFilter::default(),
                limit: 10,
            },
        );
        let Event::ThreadSearchResults { hits, .. } = answer(&found) else {
            panic!("expected hits");
        };
        assert_eq!(
            hits.first().map(|hit| hit.thread_uid.clone()),
            Some(a.clone())
        );

        let page = send(
            &mut engine,
            &human(),
            CommandKind::ReadThread {
                uid: a.clone(),
                from_seq: 0,
                limit: 2,
            },
        );
        let Event::ThreadRead { detail, .. } = answer(&page) else {
            panic!("expected a thread");
        };
        assert_eq!(detail.entries.len(), 2);
        assert!(detail.has_more);

        let whole = read(&mut engine, &a);
        assert_eq!(whole.entries.len(), 6);
        assert!(!whole.has_more);
        assert_eq!(whole.edits.len(), 1);
        assert_eq!(whole.edits[0].path, "src/client.rs");

        let missing = send(
            &mut engine,
            &human(),
            CommandKind::ReadThread {
                uid: "0000000000000000".into(),
                from_seq: 0,
                limit: 2,
            },
        );
        assert_eq!(failure(&missing).as_deref(), Some("Thread not found."));
        let no_limit = send(
            &mut engine,
            &human(),
            CommandKind::ReadThread {
                uid: a,
                from_seq: 0,
                limit: 0,
            },
        );
        assert_eq!(
            failure(&no_limit).as_deref(),
            Some("the limit must be at least 1")
        );
    }

    #[test]
    fn an_agent_reads_and_is_refused_every_write() {
        let (_dir, root, [a, _, _]) = repository();
        let mut engine = engine_over(&root);
        let agent = agent();
        let listed = send(
            &mut engine,
            &agent,
            CommandKind::ListThreads {
                filter: ThreadFilter::default(),
                before_at: None,
                before_uid: None,
                limit: 5,
            },
        );
        assert_eq!(state_of(&listed), OperationState::Done);

        let writes = [
            (
                CommandKind::ExportThread {
                    uid: a.clone(),
                    path: "/tmp/out.md".into(),
                },
                "only a human can export a thread",
            ),
            (
                CommandKind::UpdateThread {
                    uid: a.clone(),
                    title: Some("x".into()),
                    note: None,
                    starred: None,
                    tags: None,
                },
                "only a human can change a thread",
            ),
            (
                CommandKind::LinkThread {
                    uid: a.clone(),
                    link: ThreadLink {
                        kind: LinkKind::Branch,
                        reference: "main".into(),
                        origin: LinkOrigin::Manual,
                    },
                },
                "only a human can link a thread",
            ),
            (
                CommandKind::UnlinkThread {
                    uid: a.clone(),
                    kind: LinkKind::Branch,
                    reference: "main".into(),
                },
                "only a human can unlink a thread",
            ),
            (
                CommandKind::DeleteThread { uid: a.clone() },
                "only a human can delete a thread",
            ),
            (
                CommandKind::DeleteThreadEntry { id: 1 },
                "only a human can delete a thread entry",
            ),
            (
                CommandKind::ScrubThreads,
                "only a human can scrub the threads",
            ),
        ];
        for (kind, message) in writes {
            let events = send(&mut engine, &agent, kind);
            assert_eq!(state_of(&events), OperationState::Failed);
            assert_eq!(failure(&events).as_deref(), Some(message));
            assert!(!has_changed(&events));
        }
        assert_eq!(list(&mut engine, ThreadFilter::default()).len(), 3);
    }

    #[test]
    fn human_writes_change_the_store_and_say_so() {
        let (_dir, root, [a, b, _]) = repository();
        let mut engine = engine_over(&root);
        let human = human();

        let update = send(
            &mut engine,
            &human,
            CommandKind::UpdateThread {
                uid: a.clone(),
                title: Some("Backoff".into()),
                note: Some("keep".into()),
                starred: Some(true),
                tags: Some(vec!["flaky".into()]),
            },
        );
        assert_eq!(state_of(&update), OperationState::Done);
        assert!(has_changed(&update));
        let row = list(&mut engine, ThreadFilter::default())
            .into_iter()
            .find(|t| t.uid == a)
            .expect("a is listed");
        assert_eq!(row.title, "Backoff");
        assert_eq!(row.note, "keep");
        assert!(row.starred);
        assert_eq!(row.tags, vec!["flaky".to_owned()]);

        let link = ThreadLink {
            kind: LinkKind::Commit,
            reference: "abc123".into(),
            origin: LinkOrigin::Manual,
        };
        let linked = send(
            &mut engine,
            &human,
            CommandKind::LinkThread {
                uid: a.clone(),
                link: link.clone(),
            },
        );
        assert!(has_changed(&linked));
        assert_eq!(read(&mut engine, &a).links, vec![link.clone()]);
        let unlinked = send(
            &mut engine,
            &human,
            CommandKind::UnlinkThread {
                uid: a.clone(),
                kind: LinkKind::Commit,
                reference: "abc123".into(),
            },
        );
        assert!(has_changed(&unlinked));
        assert!(read(&mut engine, &a).links.is_empty());
        let again = send(
            &mut engine,
            &human,
            CommandKind::UnlinkThread {
                uid: a.clone(),
                kind: LinkKind::Commit,
                reference: "abc123".into(),
            },
        );
        assert_eq!(state_of(&again), OperationState::Failed);

        let first_entry = read(&mut engine, &a).entries[1].id;
        let deleted = send(
            &mut engine,
            &human,
            CommandKind::DeleteThreadEntry { id: first_entry },
        );
        assert!(has_changed(&deleted));
        assert_eq!(read(&mut engine, &a).entries.len(), 5);

        let gone = send(&mut engine, &human, CommandKind::DeleteThread { uid: b });
        assert!(has_changed(&gone));
        let total = send(
            &mut engine,
            &human,
            CommandKind::ListThreads {
                filter: ThreadFilter::default(),
                before_at: None,
                before_uid: None,
                limit: 5,
            },
        );
        let Event::ThreadsListed { total, .. } = answer(&total) else {
            panic!("expected threads");
        };
        assert_eq!(*total, 2);
    }

    #[test]
    fn reads_are_not_logged_and_writes_are_() {
        let (_dir, root, [a, _, _]) = repository();
        let mut engine = engine_over(&root);
        let agent = agent();
        send(
            &mut engine,
            &agent,
            CommandKind::ListThreads {
                filter: ThreadFilter::default(),
                before_at: None,
                before_uid: None,
                limit: 5,
            },
        );
        send(
            &mut engine,
            &human(),
            CommandKind::ReadThread {
                uid: a.clone(),
                from_seq: 0,
                limit: 5,
            },
        );
        send(&mut engine, &human(), CommandKind::ReadThreadEdit { id: 1 });
        send(
            &mut engine,
            &human(),
            CommandKind::SearchThreads {
                query: "backoff".into(),
                filter: ThreadFilter::default(),
                limit: 5,
            },
        );
        assert!(
            engine
                .log()
                .iter()
                .all(|entry| !is_thread_read(&entry.kind))
        );

        send(
            &mut engine,
            &human(),
            CommandKind::UpdateThread {
                uid: a,
                title: None,
                note: Some("n".into()),
                starred: None,
                tags: None,
            },
        );
        assert!(
            engine
                .log()
                .iter()
                .any(|entry| matches!(entry.kind, CommandKind::UpdateThread { .. }))
        );
    }

    fn is_thread_read(kind: &CommandKind) -> bool {
        matches!(
            kind,
            CommandKind::ListThreads { .. }
                | CommandKind::SearchThreads { .. }
                | CommandKind::ReadThread { .. }
                | CommandKind::ReadThreadEdit { .. }
                | CommandKind::ExportThread { .. }
        )
    }

    /// A settings file whose redaction patterns match `TICKET-<digits>`.
    fn ticket_settings(dir: &Path) -> PathBuf {
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"threads":{"redact":{"patterns":[{"name":"ticket","regex":"TICKET-[0-9]+","group":0}]}}}"#,
        )
        .expect("write settings");
        path
    }

    fn context<'a>(git: &'a dyn GitPort, root: &'a Path, settings: &'a Path) -> Context<'a> {
        Context {
            git,
            root,
            settings: Some(settings),
        }
    }

    #[test]
    fn export_writes_markdown_with_the_redaction_applied() {
        let (_dir, root, [a, _, _]) = repository();
        let out = tempfile::tempdir().expect("out dir");
        let settings = ticket_settings(out.path());
        let fake = FakeGit::new(&root, RepoStatus::default());
        let mut store = ThreadStore::open(&root.join(".git")).expect("open");
        let target = out.path().join("thread.md");
        let ctx = context(&fake, &root, &settings);
        let kind = CommandKind::ExportThread {
            uid: a,
            path: target.to_string_lossy().into_owned(),
        };
        let events = run(OperationId(2), &mut store, &ctx, &kind).expect("export");
        assert!(
            matches!(&events[0], Event::ThreadExported { path, .. } if path.ends_with("thread.md"))
        );

        let text = std::fs::read_to_string(&target).expect("read export");
        assert!(text.starts_with("# retry with backoff for the client\n"));
        assert!(text.contains("- Branch: main"));
        assert!(text.contains("\n## Turn 1\n\n**You:**\n\nretry with backoff for the client\n"));
        assert!(text.contains("\n## Turn 2\n"));
        assert!(text.contains("- Edit: "));
        assert!(text.contains("**Edit** `src/client.rs` modified: +1 -1\n\n```diff\n@@ -1 +1 @@"));
        assert!(
            !text.contains("TICKET-42"),
            "the redaction is applied: {text}"
        );
    }

    #[test]
    fn export_refuses_paths_and_invalid_patterns() {
        let (_dir, root, [a, _, _]) = repository();
        let out = tempfile::tempdir().expect("out dir");
        let settings = ticket_settings(out.path());
        let fake = FakeGit::new(&root, RepoStatus::default());
        let mut store = ThreadStore::open(&root.join(".git")).expect("open");
        let export = |store: &mut ThreadStore, settings: &Path, path: PathBuf| {
            let ctx = context(&fake, &root, settings);
            run(
                OperationId(3),
                store,
                &ctx,
                &CommandKind::ExportThread {
                    uid: a.clone(),
                    path: path.to_string_lossy().into_owned(),
                },
            )
        };

        let relative = export(&mut store, &settings, PathBuf::from("thread.md"));
        assert!(relative.unwrap_err().contains("must be absolute"));
        let inside = export(&mut store, &settings, root.join("thread.md"));
        assert!(
            inside
                .unwrap_err()
                .contains("must not be inside the worktree")
        );
        let missing = export(&mut store, &settings, out.path().join("no/such/t.md"));
        assert!(missing.unwrap_err().contains("does not exist"));
        assert!(!out.path().join("thread.md").exists());

        let broken = out.path().join("broken.json");
        std::fs::write(
            &broken,
            r#"{"threads":{"redact":{"patterns":[{"name":"bad","regex":"(","group":0}]}}}"#,
        )
        .expect("write settings");
        let refused = export(&mut store, &broken, out.path().join("t.md"));
        let message = refused.unwrap_err();
        assert!(
            message.starts_with("the redaction patterns are invalid"),
            "{message}"
        );
        assert!(message.contains("bad"));
    }

    #[test]
    fn scrub_redacts_texts_titles_and_notes() {
        let (_dir, root, [a, _, c]) = repository();
        let out = tempfile::tempdir().expect("out dir");
        let settings = ticket_settings(out.path());
        let fake = FakeGit::new(&root, RepoStatus::default());
        let mut store = ThreadStore::open(&root.join(".git")).expect("open");
        store
            .update_meta(
                &c,
                &MetaUpdate {
                    title: Some("Fix TICKET-9".into()),
                    ..MetaUpdate::default()
                },
            )
            .expect("title");
        store
            .update_meta(
                &a,
                &MetaUpdate {
                    note: Some("see TICKET-7".into()),
                    ..MetaUpdate::default()
                },
            )
            .expect("note");

        // A bad pattern refuses the scrub and changes nothing.
        let broken = out.path().join("broken.json");
        std::fs::write(
            &broken,
            r#"{"threads":{"redact":{"patterns":[{"name":"bad","regex":"(","group":0}]}}}"#,
        )
        .expect("write settings");
        let ctx = context(&fake, &root, &broken);
        assert!(run(OperationId(4), &mut store, &ctx, &CommandKind::ScrubThreads).is_err());
        assert!(read_back(&mut store, &a).contains("TICKET-42"));

        let ctx = context(&fake, &root, &settings);
        let events =
            run(OperationId(4), &mut store, &ctx, &CommandKind::ScrubThreads).expect("scrub");
        let Event::ThreadsScrubbed { changed, .. } = &events[0] else {
            panic!("expected the scrub answer, got {events:?}");
        };
        assert!(*changed >= 3, "entry, title and note changed: {changed}");
        assert_eq!(events[1], Event::ThreadsChanged);

        let detail = store.read_thread(&a, 0, 100).expect("read a");
        assert!(detail.entries.iter().all(|e| !e.text.contains("TICKET-42")));
        assert!(!detail.summary.note.contains("TICKET-7"));
        let c_title = store
            .list_threads(&ThreadFilter::default(), None, 10)
            .expect("list")
            .into_iter()
            .find(|t| t.uid == c)
            .expect("c listed")
            .title;
        assert!(!c_title.contains("TICKET-9"), "{c_title}");
    }

    fn read_back(store: &mut ThreadStore, uid: &str) -> String {
        store
            .read_thread(uid, 0, 100)
            .expect("read")
            .entries
            .iter()
            .map(|entry| entry.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Commits `a.txt` with the text `one`, then leaves `two` in the working tree.
    fn git_repository() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().to_path_buf();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .status()
                .expect("run git");
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["init", "-q"]);
        std::fs::write(root.join("a.txt"), "one\n").expect("write");
        git(&["add", "a.txt"]);
        git(&[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "-m",
            "init",
        ]);
        std::fs::write(root.join("a.txt"), "two\n").expect("write");
        (dir, root)
    }

    fn blob(text: &str) -> String {
        git2::Oid::hash_object(git2::ObjectType::Blob, text.as_bytes())
            .expect("hash")
            .to_string()
    }

    #[test]
    fn edit_state_follows_the_working_tree_and_head() {
        let (_dir, root) = git_repository();
        let mut store = ThreadStore::open(&root.join(".git")).expect("open");
        let entries = vec![
            edit_entry("e-work", "a.txt", Some(blob("two\n")), 1),
            edit_entry("e-head", "a.txt", Some(blob("one\n")), 2),
            edit_entry("e-gone", "a.txt", Some(blob("three\n")), 3),
            edit_entry("e-none", "a.txt", None, 4),
        ];
        let uid = store
            .apply(&batch("g", "main", 10, entries))
            .expect("apply")
            .uid;
        let detail = store.read_thread(&uid, 0, 100).expect("read");
        let ids: Vec<i64> = detail.edits.iter().map(|edit| edit.id).collect();
        assert_eq!(ids.len(), 4);

        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.handle(
            OperationId(1),
            &Command {
                actor: human(),
                kind: CommandKind::OpenRepo {
                    path: root.to_string_lossy().into_owned(),
                },
            },
        );
        let states: Vec<EditState> = ids
            .iter()
            .map(|id| {
                let events = send(
                    &mut engine,
                    &human(),
                    CommandKind::ReadThreadEdit { id: *id },
                );
                match answer(&events) {
                    Event::ThreadEditRead { state, .. } => *state,
                    other => panic!("expected an edit, got {other:?}"),
                }
            })
            .collect();
        assert_eq!(
            states,
            vec![
                EditState::InWorkingTree,
                EditState::InHead,
                EditState::ChangedSince,
                EditState::Unknown,
            ]
        );
        let missing = send(
            &mut engine,
            &human(),
            CommandKind::ReadThreadEdit { id: 999 },
        );
        assert_eq!(failure(&missing).as_deref(), Some("Edit not found."));
    }

    #[test]
    fn the_derivation_names_each_state() {
        let post = "p";
        let some = |s: &str| Ok(Some(s.to_owned()));
        assert_eq!(
            derive_state(Some(post), some("p"), some("x")),
            EditState::InWorkingTree
        );
        assert_eq!(
            derive_state(Some(post), some("x"), some("p")),
            EditState::InHead
        );
        assert_eq!(
            derive_state(Some(post), Ok(None), Ok(None)),
            EditState::ChangedSince
        );
        assert_eq!(
            derive_state(Some(post), Err(()), some("p")),
            EditState::Unknown
        );
        assert_eq!(
            derive_state(Some(post), some("x"), Err(())),
            EditState::Unknown
        );
        assert_eq!(derive_state(None, some("p"), some("p")), EditState::Unknown);
    }

    #[test]
    fn utc_formats_epoch_milliseconds() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(1_700_000_000_000), "2023-11-14 22:13:20 UTC");
        assert_eq!(utc(951_782_400_000), "2000-02-29 00:00:00 UTC");
    }

    #[test]
    fn first_line_and_fences_handle_edges() {
        assert_eq!(first_line("\n  \nfirst\nsecond", 200), "first");
        assert_eq!(first_line("abcdef", 3), "abc...");
        assert_eq!(longest_backtick_run("a ``` b `` c"), 3);
        assert_eq!(longest_backtick_run("none"), 0);
    }
}
