//! The curation of a thread on the Threads page (T6): what each dialog reads
//! and what its command carries. Rename, note, tags, link, export, delete and
//! scrub are Human-only engine commands (D119), so the window sends them with
//! `Actor::Human` once the dialog's confirm button is pressed. The rules here are
//! pure and tested: the text the dialogs prefill, the validation of what was
//! typed, the commands, the words the dialogs and notices say, and the tracker
//! that holds a write until the engine settles it. The window side is in
//! `thread_dialogs.rs`.

use std::path::Path;

use gm_schema::{
    AttachKind, CommandKind, EntryKind, LinkKind, LinkOrigin, OperationId, OperationState,
    ThreadFilter, ThreadLink, ThreadRelKind, ThreadSummary,
};

use crate::action_registry::Reason;
use crate::threads;

/// The longest slug of an export's file name, in characters.
const SLUG_CHARS: usize = 60;
/// The longest snippet of an entry the delete dialog quotes.
const SNIPPET_CHARS: usize = 80;
const TITLE_EMPTY: Reason = "A thread needs a title.";
const LINK_EMPTY: Reason = "Enter a commit id or a ref.";
/// The longest tag, in characters: the store's limit on a label name (D145).
const TAG_CHARS: usize = 64;
const TAG_LONG: Reason = "A tag is at most 64 characters.";
const TAG_CONTROL: Reason = "A tag cannot contain control characters.";
const LINK_INVALID: Reason = "Use 4 to 64 hex digits for a commit id, or a ref without spaces.";
const PATH_EMPTY: Reason = "Enter the path of the file to write.";
/// The line every delete says: the text is gone for good.
pub(crate) const DELETE_LOST: &str =
    "The text is overwritten in the database file and cannot be restored.";
/// What a scrub does, in the confirmation.
pub(crate) const SCRUB_LINES: [&str; 2] = [
    "Applies the current redaction patterns to every stored thread: its messages, tool calls, titles and notes.",
    "A scrub cannot be undone.",
];

/// The field a dialog edits, or the file it exports, of the thread it is opened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThreadField {
    Rename,
    Note,
    Tags,
    LinkCommit,
    Export,
}

impl ThreadField {
    /// The dialog's title.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Rename => "Rename Thread",
            Self::Note => "Add Note",
            Self::Tags => "Tags",
            Self::LinkCommit => "Link to Commit",
            Self::Export => "Export Thread",
        }
    }

    /// The label above the field.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Rename => "TITLE",
            Self::Note => "NOTE",
            Self::Tags => "TAGS",
            Self::LinkCommit => "COMMIT",
            Self::Export => "PATH",
        }
    }

    /// The text the empty field shows.
    pub(crate) fn placeholder(self) -> &'static str {
        match self {
            Self::Rename => "Thread title",
            Self::Note => "A note on this thread",
            Self::Tags => "Comma-separated, for example bug, auth",
            Self::LinkCommit => "Commit id or ref",
            Self::Export => "Absolute path of the Markdown file",
        }
    }

    /// The button that sends the change.
    pub(crate) fn confirm(self) -> &'static str {
        match self {
            Self::Rename => "Rename",
            Self::Note | Self::Tags => "Save",
            Self::LinkCommit => "Link",
            Self::Export => "Export",
        }
    }

    /// Whether the field takes several lines. Only a note does; Enter then adds a line.
    pub(crate) fn multiline(self) -> bool {
        self == Self::Note
    }
}

/// The title, trimmed. A blank title is refused: a thread needs one.
pub(crate) fn title_of(text: &str) -> Result<String, Reason> {
    let title = text.trim();
    if title.is_empty() {
        Err(TITLE_EMPTY)
    } else {
        Ok(title.to_owned())
    }
}

/// The note, trimmed. A blank note clears the note, so it is returned as empty.
#[must_use]
pub(crate) fn note_of(text: &str) -> String {
    text.trim().to_owned()
}

/// The tags of a comma-separated field: each trimmed, empty entries dropped and
/// repeats dropped after their first. The case is kept, so `Bug` and `bug` are two tags.
#[must_use]
pub(crate) fn tags_of(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for part in text.split(',') {
        let tag = part.trim();
        if !tag.is_empty() && !tags.iter().any(|known| known == tag) {
            tags.push(tag.to_owned());
        }
    }
    tags
}

/// The commit id or ref a link names, trimmed. It must not be blank, and must be
/// a commit id of 4 to 64 hex digits or a ref-looking string with no spaces. A
/// leading `-` is refused, so the ref cannot be read as an option by Git.
pub(crate) fn link_of(text: &str) -> Result<String, Reason> {
    let reference = text.trim();
    if reference.is_empty() {
        return Err(LINK_EMPTY);
    }
    let hex = reference.chars().all(|c| c.is_ascii_hexdigit());
    let commit_id = hex && (4..=64).contains(&reference.len());
    let ref_like = !reference.starts_with('-')
        && !reference
            .chars()
            .any(|c| c.is_whitespace() || c.is_control());
    if commit_id || ref_like {
        Ok(reference.to_owned())
    } else {
        Err(LINK_INVALID)
    }
}

/// The path an export writes, trimmed. Whether it is absolute and writable is the engine's call.
pub(crate) fn export_path_of(text: &str) -> Result<String, Reason> {
    let path = text.trim();
    if path.is_empty() {
        Err(PATH_EMPTY)
    } else {
        Ok(path.to_owned())
    }
}

/// A file-name slug of a title: lowercase ASCII letters and digits, runs of
/// anything else as one `-`, at most `SLUG_CHARS` long. A title with none is `thread`.
#[must_use]
pub(crate) fn slug(title: &str) -> String {
    let mut out = String::new();
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(SLUG_CHARS);
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "thread".to_owned()
    } else {
        out
    }
}

/// The export path a new export proposes: `<home>/<slug of the title>.md`.
#[must_use]
pub(crate) fn default_export_path(home: &str, title: &str) -> String {
    Path::new(home)
        .join(format!("{}.md", slug(title)))
        .to_string_lossy()
        .into_owned()
}

/// The text a dialog starts with: the current value of the field, or the default path.
#[must_use]
pub(crate) fn prefill(field: ThreadField, summary: &ThreadSummary, export_path: &str) -> String {
    match field {
        ThreadField::Rename => summary.title.clone(),
        ThreadField::Note => summary.note.clone(),
        ThreadField::Tags => summary.tags.join(", "),
        ThreadField::LinkCommit => String::new(),
        ThreadField::Export => export_path.to_owned(),
    }
}

/// The commands a dialog sends for `field` on thread `uid`, or the reason the text is refused.
/// Tags are the one field that takes several commands: `current` is the tags the thread has now
/// (see `tag_commands`).
pub(crate) fn field_command(
    field: ThreadField,
    uid: &str,
    text: &str,
    current: &[String],
) -> Result<Vec<CommandKind>, Reason> {
    let uid = uid.to_owned();
    Ok(match field {
        ThreadField::Rename => vec![CommandKind::UpdateThread {
            uid,
            title: Some(title_of(text)?),
            note: None,
            starred: None,
        }],
        ThreadField::Note => vec![CommandKind::UpdateThread {
            uid,
            title: None,
            note: Some(note_of(text)),
            starred: None,
        }],
        ThreadField::Tags => {
            // Each tag is checked before any command is sent, so a refused tag sends nothing.
            let tags = tags_of(text);
            if tags.iter().any(|tag| tag.chars().count() > TAG_CHARS) {
                return Err(TAG_LONG);
            }
            if tags.iter().any(|tag| tag.chars().any(char::is_control)) {
                return Err(TAG_CONTROL);
            }
            tag_commands(&uid, current, text)
        }
        ThreadField::LinkCommit => vec![CommandKind::LinkThread {
            uid,
            link: ThreadLink {
                kind: LinkKind::Commit,
                reference: link_of(text)?,
                origin: LinkOrigin::Human,
                resolved: None,
                state: None,
            },
        }],
        ThreadField::Export => vec![CommandKind::ExportThread {
            uid,
            path: export_path_of(text)?,
        }],
    })
}

/// The label commands that move a thread from the tags `current` to the tags `text` names (D145):
/// each tag no longer named is detached, then each new tag is attached. Attaching creates a label
/// that does not exist yet, so no create is sent. Nothing when the two are the same.
#[must_use]
pub(crate) fn tag_commands(uid: &str, current: &[String], text: &str) -> Vec<CommandKind> {
    let wanted = tags_of(text);
    let detach = current
        .iter()
        .filter(|tag| !wanted.contains(tag))
        .map(|tag| CommandKind::DetachLabel {
            label: tag.clone(),
            kind: AttachKind::Thread,
            reference: uid.to_owned(),
        });
    let attach = wanted
        .iter()
        .filter(|tag| !current.contains(tag))
        .map(|tag| CommandKind::AttachLabel {
            label: tag.clone(),
            kind: AttachKind::Thread,
            reference: uid.to_owned(),
        });
    detach.chain(attach).collect()
}

/// The command that sets the star to the opposite of `starred`.
#[must_use]
pub(crate) fn star_command(uid: &str, starred: bool) -> CommandKind {
    CommandKind::UpdateThread {
        uid: uid.to_owned(),
        title: None,
        note: None,
        starred: Some(!starred),
    }
}

/// The command that removes a manual link. The window offers it only for those.
#[must_use]
pub(crate) fn unlink_command(uid: &str, link: &ThreadLink) -> CommandKind {
    CommandKind::UnlinkThread {
        uid: uid.to_owned(),
        kind: link.kind,
        reference: link.reference.clone(),
    }
}

/// The title of the delete confirmation: the thread's title, or its placeholder.
#[must_use]
pub(crate) fn delete_title(summary: &ThreadSummary) -> String {
    format!("Delete {}?", threads::title(summary))
}

/// The body of the delete confirmation: what goes, then that it cannot come back.
#[must_use]
pub(crate) fn delete_lines(entries: u32, edits: u32) -> Vec<String> {
    vec![
        format!(
            "{} and {} are removed, with its links.",
            count(entries, "entry", "entries"),
            count(edits, "edit", "edits")
        ),
        DELETE_LOST.to_owned(),
    ]
}

/// The name of an entry kind, as the delete confirmation quotes it.
#[must_use]
pub(crate) fn entry_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::User => "Prompt",
        EntryKind::Assistant => "Agent message",
        EntryKind::ToolCall => "Tool call",
        EntryKind::ToolResult => "Tool result",
        EntryKind::Notice => "Notice",
    }
}

/// The body of the delete-entry confirmation: the entry, the edits it carries, then the cost.
#[must_use]
pub(crate) fn delete_entry_lines(kind: EntryKind, text: &str, edits: usize) -> Vec<String> {
    let mut lines = vec![format!("{}: {}", entry_label(kind), snippet(text))];
    if edits > 0 {
        let edits = u32::try_from(edits).unwrap_or(u32::MAX);
        lines.push(format!(
            "Its {} is removed too.",
            count(edits, "edit", "edits")
        ));
    }
    lines.push(DELETE_LOST.to_owned());
    lines
}

/// The first line of an entry's text, cut to `SNIPPET_CHARS`, or a placeholder when it is blank.
fn snippet(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty());
    match line {
        None => "(no text)".to_owned(),
        Some(line) if line.chars().count() > SNIPPET_CHARS => {
            let cut: String = line.chars().take(SNIPPET_CHARS).collect();
            format!("{cut}…")
        }
        Some(line) => line.to_owned(),
    }
}

/// The notice when a scrub ended: how many entries it changed.
#[must_use]
pub(crate) fn scrub_notice(changed: u32) -> String {
    format!("Scrub changed {changed} entries")
}

/// The notice when an export was written.
#[must_use]
pub(crate) fn exported_notice(path: &str) -> String {
    format!("Exported to {path}")
}

/// The notice of a write that succeeded, by what it was.
#[must_use]
pub(crate) fn done_notice(work: Work) -> &'static str {
    match work {
        Work::Field(ThreadField::Rename) => "Thread renamed",
        Work::Field(ThreadField::Note) => "Note saved",
        Work::Field(ThreadField::Tags) => "Tags saved",
        Work::Field(ThreadField::LinkCommit) => "Commit linked",
        Work::Field(ThreadField::Export) => "Thread exported",
        Work::Link => "Threads linked",
        Work::Delete => "Thread deleted",
        Work::DeleteEntry => "Entry deleted",
        Work::Scrub => "Threads scrubbed",
    }
}

/// `1 edit`, `3 edits`, and the same for any noun given its plural.
fn count(n: u32, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// What a write in flight is for, so its outcome goes to the dialog that asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Work {
    Field(ThreadField),
    /// A link from the Link to Thread picker (D143).
    Link,
    Delete,
    DeleteEntry,
    Scrub,
}

/// How a write ended, until the window takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The change was made.
    Done(Work),
    /// The export was written to this path.
    Exported(String),
    /// The scrub changed this many entries.
    Scrubbed(u32),
    /// The engine refused, with its message.
    Failed(String),
}

/// The write in flight, and its outcome until the window takes it. The engine
/// answers an export and a scrub with a payload event before the operation ends,
/// so their `Done` waits for that payload.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ThreadWork {
    pending: Option<(OperationId, Work)>,
    outcome: Option<Outcome>,
}

impl ThreadWork {
    pub(crate) fn start(&mut self, id: OperationId, work: Work) {
        self.pending = Some((id, work));
        self.outcome = None;
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Whether the write in flight is `work`, so its dialog shows it is saving.
    pub(crate) fn is_working(&self, work: Work) -> bool {
        self.pending.is_some_and(|(_, pending)| pending == work)
    }

    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending.is_some_and(|(pending, _)| pending == id)
    }

    /// Ends the write with the operation's terminal `state`.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, work)) = self.pending else {
            return;
        };
        let waits_for_payload = matches!(work, Work::Field(ThreadField::Export) | Work::Scrub);
        if state == OperationState::Done && waits_for_payload {
            return;
        }
        self.pending = None;
        self.outcome = Some(if state == OperationState::Done {
            Outcome::Done(work)
        } else {
            let message = detail.unwrap_or_else(|| "The change was not saved.".to_owned());
            Outcome::Failed(message)
        });
    }

    /// Ends the write with its payload, when the engine answers an export or a scrub.
    pub(crate) fn payload(&mut self, id: OperationId, outcome: Outcome) {
        if self.owns(id) {
            self.pending = None;
            self.outcome = Some(outcome);
        }
    }

    pub(crate) fn take(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }
}

/// The newest threads the Link to Thread picker reads. A full page says that older threads are
/// not listed, and the picker matches its query against these only.
pub(crate) const PICK_LIMIT: u32 = 200;
/// The rows the picker draws at once; a longer match list is cut and says so.
pub(crate) const PICK_SHOWN: usize = 8;
const PICK_NONE: Reason = "Choose the thread to link.";

/// The read of the picker's threads: the newest `PICK_LIMIT` threads of the store.
#[must_use]
pub(crate) fn pick_plan() -> CommandKind {
    CommandKind::ListThreads {
        filter: ThreadFilter::default(),
        before_at: None,
        before_uid: None,
        limit: PICK_LIMIT,
    }
}

/// The words the kind choice shows, in the order the buttons draw them (D143).
pub(crate) const LINK_KINDS: [ThreadRelKind; 3] = [
    ThreadRelKind::Continues,
    ThreadRelKind::Relates,
    ThreadRelKind::Supersedes,
];

/// The button label of a thread link kind.
#[must_use]
pub(crate) fn kind_label(kind: ThreadRelKind) -> &'static str {
    match kind {
        ThreadRelKind::Continues => "Continues",
        ThreadRelKind::Relates => "Relates",
        ThreadRelKind::Supersedes => "Supersedes",
    }
}

/// The state of the Link to Thread picker (D143): the thread it links from, the threads
/// the store lists (the thread itself left out), the target chosen and the kind. The read of
/// the threads is answered by `pending`; the model routes that answer here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinkPicker {
    from: String,
    kind: ThreadRelKind,
    target: Option<String>,
    candidates: Vec<ThreadSummary>,
    pending: Option<OperationId>,
}

impl Default for LinkPicker {
    fn default() -> Self {
        Self::new("")
    }
}

impl LinkPicker {
    /// A picker for thread `from`. The kind starts as `relates`, the one that claims least.
    #[must_use]
    pub(crate) fn new(from: &str) -> Self {
        Self {
            from: from.to_owned(),
            kind: ThreadRelKind::Relates,
            target: None,
            candidates: Vec::new(),
            pending: None,
        }
    }

    /// Notes the read of the threads in flight, answered by `id`.
    pub(crate) fn request(&mut self, id: OperationId) {
        self.pending = Some(id);
    }

    /// Whether `id` is the threads read in flight.
    #[must_use]
    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending == Some(id)
    }

    /// Whether the threads are still being read.
    #[must_use]
    pub(crate) fn is_loading(&self) -> bool {
        self.pending.is_some()
    }

    /// Keeps the answer when it is the read in flight, without the thread it links from. Returns
    /// false for any other answer. A target that is no longer listed is dropped.
    pub(crate) fn listed(&mut self, id: OperationId, threads: Vec<ThreadSummary>) -> bool {
        if !self.owns(id) {
            return false;
        }
        self.pending = None;
        self.candidates = threads
            .into_iter()
            .filter(|thread| thread.uid != self.from)
            .collect();
        if self
            .target
            .as_ref()
            .is_some_and(|target| !self.candidates.iter().any(|thread| &thread.uid == target))
        {
            self.target = None;
        }
        true
    }

    /// The read `id` ended without an answer: it is no longer in flight.
    pub(crate) fn ended(&mut self, id: OperationId) {
        if self.owns(id) {
            self.pending = None;
        }
    }

    /// The threads the query matches, in the store's order: a case-insensitive word of the
    /// plain title. A blank query matches all.
    #[must_use]
    pub(crate) fn matches(&self, query: &str) -> Vec<&ThreadSummary> {
        let needle = query.trim().to_lowercase();
        self.candidates
            .iter()
            .filter(|thread| {
                needle.is_empty() || threads::title(thread).to_lowercase().contains(&needle)
            })
            .collect()
    }

    /// Chooses the listed thread `uid`. Returns false when no listed thread has that id.
    pub(crate) fn choose(&mut self, uid: &str) -> bool {
        if self.candidates.iter().any(|thread| thread.uid == uid) {
            self.target = Some(uid.to_owned());
            true
        } else {
            false
        }
    }

    /// The thread chosen, if any.
    #[must_use]
    pub(crate) fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// Chooses the kind of link.
    pub(crate) fn set_kind(&mut self, kind: ThreadRelKind) {
        self.kind = kind;
    }

    /// The kind of link chosen.
    #[must_use]
    pub(crate) fn kind(&self) -> ThreadRelKind {
        self.kind
    }

    /// Whether the answer is a full page, so older threads may exist outside the picker.
    #[must_use]
    pub(crate) fn is_full(&self) -> bool {
        u32::try_from(self.candidates.len()).is_ok_and(|len| len >= PICK_LIMIT)
    }

    /// The `LinkThreads` command that links the thread to the chosen one, or the reason none is chosen.
    pub(crate) fn command(&self) -> Result<CommandKind, Reason> {
        let to = self.target.clone().ok_or(PICK_NONE)?;
        Ok(CommandKind::LinkThreads {
            from: self.from.clone(),
            to,
            kind: self.kind,
        })
    }
}

/// The most threads per kind the relations of the selected thread read (D143).
pub(crate) const RELATION_LIMIT: u32 = 50;

/// The threads linked to the selected thread, by kind (D143). Each kind is one `ListThreads`
/// read with `related_to` and `related_kind`, so a row has the other thread's kind and title.
/// The listing does not give the link's direction or who made it: `ThreadRelation` is on no
/// read yet, so these rows are read-only and carry no Remove.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ThreadRelations {
    uid: Option<String>,
    pending: Vec<(OperationId, ThreadRelKind)>,
    rows: Vec<(ThreadRelKind, ThreadSummary)>,
}

impl ThreadRelations {
    /// The reads of thread `uid`, one per kind of link, in the order of `LINK_KINDS`.
    #[must_use]
    pub(crate) fn plan(uid: &str) -> Vec<(ThreadRelKind, CommandKind)> {
        LINK_KINDS
            .iter()
            .map(|kind| {
                (
                    *kind,
                    CommandKind::ListThreads {
                        filter: ThreadFilter {
                            related_to: Some(uid.to_owned()),
                            related_kind: Some(*kind),
                            ..ThreadFilter::default()
                        },
                        before_at: None,
                        before_uid: None,
                        limit: RELATION_LIMIT,
                    },
                )
            })
            .collect()
    }

    /// Forgets the rows and the reads of the thread shown, and takes `uid` as the thread shown.
    pub(crate) fn reset(&mut self, uid: &str) {
        *self = Self {
            uid: Some(uid.to_owned()),
            ..Self::default()
        };
    }

    /// Notes a read of `kind` in flight, answered by `id`.
    pub(crate) fn sent(&mut self, id: OperationId, kind: ThreadRelKind) {
        self.pending.push((id, kind));
    }

    /// Whether `id` is one of the reads in flight.
    #[must_use]
    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending.iter().any(|(pending, _)| *pending == id)
    }

    /// Keeps the answer to a read in flight: the rows of its kind are replaced. Returns false for any other.
    pub(crate) fn listed(&mut self, id: OperationId, threads: Vec<ThreadSummary>) -> bool {
        let Some(index) = self.pending.iter().position(|(pending, _)| *pending == id) else {
            return false;
        };
        let (_, kind) = self.pending.remove(index);
        self.rows.retain(|(known, _)| *known != kind);
        self.rows
            .extend(threads.into_iter().map(|thread| (kind, thread)));
        true
    }

    /// The read `id` ended without an answer: it is no longer in flight.
    pub(crate) fn ended(&mut self, id: OperationId) {
        self.pending.retain(|(pending, _)| *pending != id);
    }

    /// The thread the rows belong to.
    #[must_use]
    pub(crate) fn uid(&self) -> Option<&str> {
        self.uid.as_deref()
    }

    /// The linked threads, by kind, in the order the reads answered.
    #[must_use]
    pub(crate) fn rows(&self) -> &[(ThreadRelKind, ThreadSummary)] {
        &self.rows
    }
}

#[cfg(test)]
mod tests {
    use gm_schema::{EntryKind, LinkKind, LinkOrigin, ThreadLink};

    use super::*;

    fn summary(title: &str, note: &str, tags: &[&str]) -> ThreadSummary {
        ThreadSummary {
            uid: "0123456789abcdef".into(),
            harness: "claude-code".into(),
            harness_version: None,
            session_id: "s".into(),
            worktree: "/repo".into(),
            branch: Some("main".into()),
            model: None,
            title: title.into(),
            note: note.into(),
            starred: false,
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            started_at: 0,
            last_event_at: 0,
            ended_at: None,
            entry_count: 0,
            edit_count: 0,
            redaction_count: 0,
        }
    }

    #[test]
    fn a_blank_title_is_refused_and_the_rest_is_trimmed() {
        assert_eq!(title_of("  \t "), Err("A thread needs a title."));
        assert_eq!(title_of("  Fix login  "), Ok("Fix login".to_owned()));
    }

    #[test]
    fn a_blank_note_clears_the_note_and_the_rest_is_trimmed() {
        assert_eq!(note_of("   \n"), "");
        assert_eq!(note_of("  first line\nsecond  \n"), "first line\nsecond");
    }

    #[test]
    fn tags_are_trimmed_deduplicated_and_keep_their_case() {
        assert_eq!(
            tags_of(" bug, auth ,, bug ,Bug,  , auth"),
            ["bug", "auth", "Bug"]
        );
        assert!(tags_of(" , ,").is_empty());
        assert!(tags_of("").is_empty());
    }

    #[test]
    fn a_commit_link_takes_hex_of_4_to_64_digits_or_a_ref_without_spaces() {
        assert_eq!(link_of(" a1b2c3d "), Ok("a1b2c3d".to_owned()));
        assert_eq!(link_of("abcd"), Ok("abcd".to_owned()));
        assert_eq!(link_of(&"f".repeat(64)), Ok("f".repeat(64)));
        assert_eq!(link_of("HEAD~2"), Ok("HEAD~2".to_owned()));
        assert_eq!(link_of("main"), Ok("main".to_owned()));
        assert_eq!(link_of("  "), Err("Enter a commit id or a ref."));
        assert_eq!(
            link_of("fix login"),
            Err("Use 4 to 64 hex digits for a commit id, or a ref without spaces.")
        );
        assert!(link_of("--output=x").is_err());
        assert!(link_of("a\tb").is_err());
    }

    #[test]
    fn an_export_needs_a_path() {
        assert_eq!(
            export_path_of("  "),
            Err("Enter the path of the file to write.")
        );
        assert_eq!(export_path_of(" /tmp/a.md "), Ok("/tmp/a.md".to_owned()));
    }

    #[test]
    fn the_slug_is_lowercase_and_cut_at_the_first_run_of_other_characters() {
        assert_eq!(slug("Fix login: bug!"), "fix-login-bug");
        assert_eq!(slug("  --Ação 2 --  "), "a-o-2");
        assert_eq!(slug("!!!"), "thread");
        let long = slug(&"a ".repeat(100));
        assert!(long.len() <= SLUG_CHARS && long.len() > SLUG_CHARS - 2);
        assert!(!long.ends_with('-'));
    }

    #[test]
    fn the_default_export_path_is_the_home_folder_and_the_slug() {
        assert_eq!(
            default_export_path("/home/ana", "Fix login: bug!"),
            "/home/ana/fix-login-bug.md"
        );
        assert_eq!(
            default_export_path("/home/ana/", "Thread"),
            "/home/ana/thread.md"
        );
        assert_eq!(default_export_path("", "Thread"), "thread.md");
    }

    #[test]
    fn each_dialog_starts_with_the_current_value_of_its_field() {
        let thread = summary("Fix login", "check the cookie", &["bug", "auth"]);
        assert_eq!(prefill(ThreadField::Rename, &thread, "/x"), "Fix login");
        assert_eq!(
            prefill(ThreadField::Note, &thread, "/x"),
            "check the cookie"
        );
        assert_eq!(prefill(ThreadField::Tags, &thread, "/x"), "bug, auth");
        assert_eq!(prefill(ThreadField::LinkCommit, &thread, "/x"), "");
        assert_eq!(prefill(ThreadField::Export, &thread, "/x/a.md"), "/x/a.md");
    }

    #[test]
    fn only_the_note_takes_several_lines() {
        assert!(ThreadField::Note.multiline());
        for field in [
            ThreadField::Rename,
            ThreadField::Tags,
            ThreadField::LinkCommit,
            ThreadField::Export,
        ] {
            assert!(!field.multiline(), "{field:?}");
        }
    }

    #[test]
    fn each_field_sends_its_own_engine_command() {
        assert_eq!(
            field_command(ThreadField::Rename, "u1", " New ", &[]),
            Ok(vec![CommandKind::UpdateThread {
                uid: "u1".into(),
                title: Some("New".into()),
                note: None,
                starred: None,
            }])
        );
        assert_eq!(
            field_command(ThreadField::Note, "u1", "", &[]),
            Ok(vec![CommandKind::UpdateThread {
                uid: "u1".into(),
                title: None,
                note: Some(String::new()),
                starred: None,
            }])
        );
        assert_eq!(
            field_command(ThreadField::LinkCommit, "u1", "a1b2c3d", &[]),
            Ok(vec![CommandKind::LinkThread {
                uid: "u1".into(),
                link: ThreadLink {
                    kind: LinkKind::Commit,
                    reference: "a1b2c3d".into(),
                    origin: LinkOrigin::Human,
                    resolved: None,
                    state: None,
                },
            }])
        );
        assert_eq!(
            field_command(ThreadField::Export, "u1", " /tmp/t.md ", &[]),
            Ok(vec![CommandKind::ExportThread {
                uid: "u1".into(),
                path: "/tmp/t.md".into(),
            }])
        );
    }

    #[test]
    fn tags_send_labels_for_what_changed_only() {
        let current = vec!["bug".to_owned(), "api".to_owned()];
        assert_eq!(
            tag_commands("u1", &[], "a, b"),
            vec![
                CommandKind::AttachLabel {
                    label: "a".into(),
                    kind: AttachKind::Thread,
                    reference: "u1".into(),
                },
                CommandKind::AttachLabel {
                    label: "b".into(),
                    kind: AttachKind::Thread,
                    reference: "u1".into(),
                },
            ]
        );
        assert_eq!(
            tag_commands("u1", &current, "api, ui"),
            vec![
                CommandKind::DetachLabel {
                    label: "bug".into(),
                    kind: AttachKind::Thread,
                    reference: "u1".into(),
                },
                CommandKind::AttachLabel {
                    label: "ui".into(),
                    kind: AttachKind::Thread,
                    reference: "u1".into(),
                },
            ]
        );
        assert!(tag_commands("u1", &current, "bug,api").is_empty());
    }

    #[test]
    fn a_tag_the_store_would_refuse_sends_nothing() {
        let long = "t".repeat(65);
        assert_eq!(
            field_command(ThreadField::Tags, "u1", &long, &[]),
            Err("A tag is at most 64 characters.")
        );
        assert_eq!(
            field_command(ThreadField::Tags, "u1", "a\tb", &[]),
            Err("A tag cannot contain control characters.")
        );
    }

    #[test]
    fn a_refused_text_sends_no_command() {
        assert_eq!(
            field_command(ThreadField::Rename, "u1", " ", &[]),
            Err("A thread needs a title.")
        );
        assert_eq!(
            field_command(ThreadField::LinkCommit, "u1", "", &[]),
            Err("Enter a commit id or a ref.")
        );
    }

    #[test]
    fn the_star_sends_the_opposite_of_the_state_shown() {
        assert_eq!(
            star_command("u1", false),
            CommandKind::UpdateThread {
                uid: "u1".into(),
                title: None,
                note: None,
                starred: Some(true),
            }
        );
        assert!(matches!(
            star_command("u1", true),
            CommandKind::UpdateThread {
                starred: Some(false),
                ..
            }
        ));
    }

    #[test]
    fn an_unlink_names_the_link_it_removes() {
        let link = ThreadLink {
            kind: LinkKind::Commit,
            reference: "a1b2".into(),
            origin: LinkOrigin::Human,
            resolved: None,
            state: None,
        };
        assert_eq!(
            unlink_command("u1", &link),
            CommandKind::UnlinkThread {
                uid: "u1".into(),
                kind: LinkKind::Commit,
                reference: "a1b2".into(),
            }
        );
    }

    #[test]
    fn the_delete_confirmation_says_what_goes_and_that_it_is_final() {
        let thread = summary("Fix login", "", &[]);
        assert_eq!(delete_title(&thread), "Delete Fix login?");
        assert_eq!(
            delete_title(&summary("  ", "", &[])),
            "Delete Untitled thread?"
        );
        let lines = delete_lines(1, 0);
        assert_eq!(lines[0], "1 entry and 0 edits are removed, with its links.");
        assert_eq!(lines[1], DELETE_LOST);
        assert_eq!(
            delete_lines(12, 3)[0],
            "12 entries and 3 edits are removed, with its links."
        );
        assert!(DELETE_LOST.contains("cannot be restored"));
    }

    #[test]
    fn the_delete_entry_confirmation_quotes_the_entry_and_its_edits() {
        assert_eq!(
            delete_entry_lines(EntryKind::User, "  fix the login\nthen test", 0),
            ["Prompt: fix the login", DELETE_LOST]
        );
        assert_eq!(
            delete_entry_lines(EntryKind::ToolCall, "Edit", 2),
            [
                "Tool call: Edit",
                "Its 2 edits is removed too.",
                DELETE_LOST
            ]
        );
        assert_eq!(
            delete_entry_lines(EntryKind::Assistant, "   \n ", 1)[0],
            "Agent message: (no text)"
        );
        let long = "x".repeat(200);
        let quoted = &delete_entry_lines(EntryKind::Notice, &long, 0)[0];
        assert_eq!(quoted.chars().count(), "Notice: ".len() + SNIPPET_CHARS + 1);
        assert!(quoted.ends_with('…'));
    }

    #[test]
    fn the_scrub_confirmation_explains_the_patterns_and_the_cost() {
        assert!(SCRUB_LINES[0].contains("redaction patterns"));
        assert!(SCRUB_LINES[1].contains("cannot be undone"));
    }

    #[test]
    fn the_notices_name_what_happened() {
        assert_eq!(scrub_notice(3), "Scrub changed 3 entries");
        assert_eq!(scrub_notice(0), "Scrub changed 0 entries");
        assert_eq!(exported_notice("/tmp/a.md"), "Exported to /tmp/a.md");
        assert_eq!(done_notice(Work::Delete), "Thread deleted");
        assert_eq!(done_notice(Work::Field(ThreadField::Tags)), "Tags saved");
    }

    #[test]
    fn a_write_is_tracked_only_while_its_operation_runs() {
        let mut work = ThreadWork::default();
        work.start(OperationId(7), Work::Field(ThreadField::Rename));
        assert!(work.is_pending());
        assert!(work.is_working(Work::Field(ThreadField::Rename)));
        assert!(!work.is_working(Work::Delete));
        assert!(!work.owns(OperationId(6)));
        assert!(work.owns(OperationId(7)));
        work.settle(OperationState::Executing, None);
        assert!(work.is_pending());
        assert_eq!(work.take(), None);
        work.settle(OperationState::Done, None);
        assert!(!work.is_pending());
        assert_eq!(
            work.take(),
            Some(Outcome::Done(Work::Field(ThreadField::Rename)))
        );
        assert_eq!(work.take(), None);
    }

    #[test]
    fn a_refused_write_keeps_the_engines_message() {
        let mut work = ThreadWork::default();
        work.start(OperationId(2), Work::Delete);
        work.settle(OperationState::Failed, Some("Thread not found.".into()));
        assert_eq!(
            work.take(),
            Some(Outcome::Failed("Thread not found.".into()))
        );
        work.start(OperationId(3), Work::Delete);
        work.settle(OperationState::Vetoed, None);
        assert_eq!(
            work.take(),
            Some(Outcome::Failed("The change was not saved.".into()))
        );
    }

    #[test]
    fn an_export_or_scrub_ends_with_its_payload_not_with_done() {
        let mut work = ThreadWork::default();
        work.start(OperationId(4), Work::Field(ThreadField::Export));
        work.settle(OperationState::Done, None);
        assert!(work.is_pending());
        assert_eq!(work.take(), None);
        work.payload(OperationId(9), Outcome::Exported("/x".into()));
        assert!(
            work.is_pending(),
            "a payload for another operation is ignored"
        );
        work.payload(OperationId(4), Outcome::Exported("/tmp/t.md".into()));
        assert!(!work.is_pending());
        assert_eq!(work.take(), Some(Outcome::Exported("/tmp/t.md".into())));

        work.start(OperationId(5), Work::Scrub);
        work.settle(OperationState::Done, None);
        work.payload(OperationId(5), Outcome::Scrubbed(2));
        assert_eq!(work.take(), Some(Outcome::Scrubbed(2)));
    }

    #[test]
    fn an_export_that_fails_says_why() {
        let mut work = ThreadWork::default();
        work.start(OperationId(4), Work::Field(ThreadField::Export));
        work.settle(
            OperationState::Failed,
            Some("the export path must be absolute".into()),
        );
        assert_eq!(
            work.take(),
            Some(Outcome::Failed("the export path must be absolute".into()))
        );
    }

    fn picked(uid: &str, title: &str) -> ThreadSummary {
        ThreadSummary {
            title: title.to_owned(),
            uid: uid.to_owned(),
            ..summary("", "", &[])
        }
    }

    #[test]
    fn the_picker_lists_every_thread_but_its_own_and_matches_titles_in_any_case() {
        let mut picker = LinkPicker::new("me");
        picker.request(OperationId(3));
        assert!(picker.is_loading());
        assert!(!picker.listed(OperationId(4), vec![picked("other", "Fix")]));
        assert!(picker.listed(
            OperationId(3),
            vec![
                picked("me", "Mine"),
                picked("a", "Auth refactor"),
                picked("b", "fix login")
            ],
        ));
        assert!(!picker.is_loading());
        let all: Vec<&str> = picker.matches("").iter().map(|t| t.uid.as_str()).collect();
        assert_eq!(all, vec!["a", "b"]);
        let found: Vec<&str> = picker
            .matches("  FIX ")
            .iter()
            .map(|t| t.uid.as_str())
            .collect();
        assert_eq!(found, vec!["b"]);
    }

    #[test]
    fn the_picker_sends_the_kind_it_is_set_to_and_refuses_without_a_target() {
        let mut picker = LinkPicker::new("me");
        picker.request(OperationId(1));
        picker.listed(OperationId(1), vec![picked("a", "A")]);
        assert_eq!(picker.command(), Err(PICK_NONE));
        assert!(!picker.choose("missing"));
        assert!(picker.choose("a"));
        picker.set_kind(ThreadRelKind::Supersedes);
        assert_eq!(
            picker.command(),
            Ok(CommandKind::LinkThreads {
                from: "me".to_owned(),
                to: "a".to_owned(),
                kind: ThreadRelKind::Supersedes,
            })
        );
        assert_eq!(picker.kind(), ThreadRelKind::Supersedes);
    }

    #[test]
    fn a_target_that_is_no_longer_listed_is_dropped_and_an_ended_read_is_not_in_flight() {
        let mut picker = LinkPicker::new("me");
        picker.request(OperationId(1));
        picker.listed(OperationId(1), vec![picked("a", "A"), picked("b", "B")]);
        assert!(picker.choose("b"));
        picker.request(OperationId(2));
        picker.listed(OperationId(2), vec![picked("a", "A")]);
        assert_eq!(picker.target(), None);
        picker.request(OperationId(5));
        picker.ended(OperationId(4));
        assert!(picker.is_loading());
        picker.ended(OperationId(5));
        assert!(!picker.is_loading());
    }

    #[test]
    fn the_kinds_are_offered_in_order_with_their_words() {
        assert_eq!(
            LINK_KINDS,
            [
                ThreadRelKind::Continues,
                ThreadRelKind::Relates,
                ThreadRelKind::Supersedes,
            ]
        );
        assert_eq!(kind_label(ThreadRelKind::Continues), "Continues");
        assert_eq!(kind_label(ThreadRelKind::Supersedes), "Supersedes");
        assert_eq!(LinkPicker::new("me").kind(), ThreadRelKind::Relates);
        assert_eq!(done_notice(Work::Link), "Threads linked");
    }

    #[test]
    fn the_relation_reads_are_one_per_kind_for_the_thread_shown() {
        let plan = ThreadRelations::plan("me");
        let kinds: Vec<ThreadRelKind> = plan.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(kinds, LINK_KINDS.to_vec());
        for (kind, command) in plan {
            assert_eq!(
                command,
                CommandKind::ListThreads {
                    filter: ThreadFilter {
                        related_to: Some("me".to_owned()),
                        related_kind: Some(kind),
                        ..ThreadFilter::default()
                    },
                    before_at: None,
                    before_uid: None,
                    limit: RELATION_LIMIT,
                }
            );
        }
    }

    #[test]
    fn a_kind_answer_replaces_only_its_own_rows_and_a_stale_one_is_dropped() {
        let mut relations = ThreadRelations::default();
        relations.reset("me");
        relations.sent(OperationId(1), ThreadRelKind::Continues);
        relations.sent(OperationId(2), ThreadRelKind::Relates);
        assert!(relations.owns(OperationId(2)));
        assert!(relations.listed(OperationId(1), vec![picked("a", "A")]));
        assert!(relations.listed(OperationId(2), vec![picked("b", "B")]));
        assert!(!relations.listed(OperationId(2), vec![picked("c", "C")]));
        assert_eq!(relations.uid(), Some("me"));
        let shown: Vec<(&str, ThreadRelKind)> = relations
            .rows()
            .iter()
            .map(|(kind, thread)| (thread.uid.as_str(), *kind))
            .collect();
        assert_eq!(
            shown,
            vec![
                ("a", ThreadRelKind::Continues),
                ("b", ThreadRelKind::Relates)
            ]
        );
        relations.sent(OperationId(3), ThreadRelKind::Continues);
        relations.ended(OperationId(3));
        assert!(!relations.owns(OperationId(3)));
        relations.reset("other");
        assert!(relations.rows().is_empty());
        assert_eq!(relations.uid(), Some("other"));
    }
}
