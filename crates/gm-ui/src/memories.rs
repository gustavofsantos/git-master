//! The Memories tab's model (D190, D193), with no GPUI in it: the memories the
//! repository sees (its own and those of its contexts), read a page at a time, the
//! memory selected and its detail, the filters, the proposals chosen for a bulk
//! accept, and the human actions on a memory. Each read and write is sent with a
//! `Read` tag, and an answer is applied only when its `OperationId` is the one in
//! flight for that part of the tab, as `threads.rs` does. A filter change ends the
//! list read in flight, so its late answer is dropped.
//!
//! The memory text is untrusted data: this module carries it and never reads it as
//! an instruction.

// The tab is not drawn yet (M3b); the model is exercised by its tests until then.
#![allow(dead_code)]

use std::collections::BTreeSet;

use gm_schema::threads::{Context, Memory, MemorySourceInput};
use gm_schema::{
    CommandKind, FileChange, FileChangeState, MemoryKind, MemoryStatus, OperationId, OperationState,
};

/// Memories per page of the list.
pub const PAGE: u32 = 50;
/// The longest memory text, in characters (D190).
pub const TEXT_CHARS: usize = 1_000;
/// The scope the engine gives a memory of this repository.
const REPOSITORY: &str = "repository";
/// The origin prefix of a memory an agent proposed (`agent:<client>`).
const AGENT_ORIGIN: &str = "agent:";
/// The origin of a memory the human wrote or edited.
const HUMAN_ORIGIN: &str = "human";
/// The origin prefix of an imported memory (`imported:<author>`).
const IMPORTED_ORIGIN: &str = "imported:";
/// The message of a failed read when the engine gave none.
const NO_MESSAGE: &str = "The memories could not be read.";

const NOT_SHOWN: &str = "That memory is not shown.";
const NOT_PROPOSED: &str = "Only a proposed memory can be accepted or rejected.";
const NOT_ACCEPTED: &str = "Only an accepted memory can be superseded.";
const EMPTY_TEXT: &str = "A memory needs some text.";
const TOO_LONG: &str = "A memory holds at most 1,000 characters.";
const NO_SCOPE: &str = "Choose this repository or a context for the memory.";
const NO_SUPERSEDED: &str = "Choose the memory this one supersedes.";
const SELF_SUPERSEDE: &str = "A memory cannot supersede itself.";
const NO_SELECTION: &str = "Choose proposed memories to accept.";
const NO_AUTHOR: &str = "Choose an author whose proposals to accept.";
const NOT_COMPLETE: &str = "Clear the filters and search, and load the whole list, to accept every proposal from an author.";
const NO_AUTHOR_PROPOSALS: &str = "No proposal from that author is shown.";
const BAD_RANGE: &str = "A thread range starts after it ends.";
const EMPTY_REF: &str = "A source needs a thread, a commit or a file.";

/// Which memories the list shows by scope (D193 (5)).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// Everything the repository sees: its own memories and those of its contexts.
    #[default]
    Everything,
    /// The memories of this repository only.
    Repository,
    /// The memories of one context, by its path such as `Platform / Billing`.
    Context(String),
}

impl Scope {
    /// The `scope` the list command takes: none for everything, else the repository or a path.
    fn wire(&self) -> Option<String> {
        match self {
            Self::Everything => None,
            Self::Repository => Some(REPOSITORY.to_owned()),
            Self::Context(path) => Some(path.clone()),
        }
    }
}

/// What a read or a write was sent for. Its answer is applied only while it is in flight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read {
    /// A page of the list. `append` adds it after the rows shown; `generation` is the filter
    /// generation it was sent under, so an answer to an older filter is dropped.
    List { append: bool, generation: u64 },
    /// The memory `uid`, with its sources and relations.
    Detail { uid: String },
    /// The contexts the repository belongs to, for the scope filter.
    Contexts,
    /// A change to one memory (`uid`, or none for a new memory). `removes` is true for a delete,
    /// which answers with no memory.
    Change { uid: Option<String>, removes: bool },
}

/// A command to send, with the read or write its answer is matched by.
#[derive(Clone, Debug)]
pub struct Request {
    pub kind: CommandKind,
    pub read: Read,
}

/// A page of the list as the engine answered it, with the notes on the contexts it read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryPage {
    pub memories: Vec<Memory>,
    /// The cursor of the next page; none when no more follow.
    pub next_cursor: Option<String>,
    /// False when the contexts could not be read (`note` says why).
    pub contexts_read: bool,
    pub note: Option<String>,
    /// The exact number of proposed memories the request's scope, kind, path and author select,
    /// ignoring the status, the query and the page (the engine's `proposed_total`).
    pub proposed_total: u32,
}

/// A memory the human writes. It is accepted at once (D190).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewMemory {
    pub kind: MemoryKind,
    /// The repository or one context. `Everything` is not a place to write to.
    pub scope: Scope,
    pub text: String,
    /// Optional sources: a thread range, a commit or a file.
    pub sources: Vec<MemorySourceInput>,
}

/// The state of the Memories tab: the list, its filters, the memory selected and its detail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoriesPage {
    kind: Option<MemoryKind>,
    status: Option<MemoryStatus>,
    path: Option<String>,
    author: Option<String>,
    scope: Scope,
    query: String,
    /// Counts the changes of the filters and the search. A list read is applied only for the
    /// latest one.
    generation: u64,
    /// The memories loaded, proposed first.
    memories: Vec<Memory>,
    listed: bool,
    next_cursor: Option<String>,
    contexts_read: bool,
    note: Option<String>,
    /// The engine's exact count of proposed memories, from the last list applied, moved by the
    /// writes that change a loaded proposal; `None` before a list is applied.
    proposed_total: Option<u32>,
    /// The list read in flight: its id, whether it appends, and the generation it was sent under.
    list_pending: Option<(OperationId, bool, u64)>,
    selected: Option<String>,
    detail: Option<Memory>,
    detail_pending: Option<(OperationId, String)>,
    contexts: Vec<Context>,
    contexts_pending: Option<OperationId>,
    /// The writes in flight, each with what it changes.
    writes: Vec<(OperationId, Read)>,
    /// The proposals chosen for a bulk accept.
    chosen: BTreeSet<String>,
    failure: Option<String>,
}

impl MemoriesPage {
    /// The kind the list is filtered to.
    #[must_use]
    pub fn kind(&self) -> Option<MemoryKind> {
        self.kind
    }

    /// The status the list is filtered to.
    #[must_use]
    pub fn status(&self) -> Option<MemoryStatus> {
        self.status
    }

    /// The file the list is filtered to: the memories that name that path or a path under it.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// The origin the list is filtered to, such as `human` or `agent:claude-code`.
    #[must_use]
    pub fn author(&self) -> Option<&str> {
        self.author.as_deref()
    }

    /// The scope the list is filtered to.
    #[must_use]
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    /// The search text as it is.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether any filter or the search narrows the list.
    #[must_use]
    pub fn filter_active(&self) -> bool {
        self.kind.is_some()
            || self.status.is_some()
            || self.path.is_some()
            || self.author.is_some()
            || self.scope != Scope::Everything
            || !self.query.trim().is_empty()
    }

    /// The memories loaded, proposed first, in the order the engine gave within each group.
    #[must_use]
    pub fn memories(&self) -> &[Memory] {
        &self.memories
    }

    /// How many proposed memories the count on the tab shows: the engine's exact `proposed_total`
    /// of the last list applied. Before a list is applied it counts the proposals loaded.
    #[must_use]
    pub fn proposed_count(&self) -> usize {
        match self.proposed_total {
            Some(total) => usize::try_from(total).unwrap_or(usize::MAX),
            None => self
                .memories
                .iter()
                .filter(|memory| memory.status == MemoryStatus::Proposed)
                .count(),
        }
    }

    /// Whether a list has been applied at least once.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.listed
    }

    /// Whether another page of the list follows the rows shown.
    #[must_use]
    pub fn has_more(&self) -> bool {
        self.next_cursor.is_some()
    }

    /// Whether a page of the list is in flight.
    #[must_use]
    pub fn is_loading_list(&self) -> bool {
        self.list_pending.is_some()
    }

    /// Whether a memory's detail is in flight.
    #[must_use]
    pub fn is_loading_detail(&self) -> bool {
        self.detail_pending.is_some()
    }

    /// The failure of the last read or write, when it failed.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// The note the engine gave on the list, such as contexts that could not be read.
    #[must_use]
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// The contexts the repository belongs to, for the scope filter.
    #[must_use]
    pub fn contexts(&self) -> &[Context] {
        &self.contexts
    }

    /// The uid of the selected memory.
    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The detail of the selected memory, once it is read.
    #[must_use]
    pub fn detail(&self) -> Option<&Memory> {
        self.detail
            .as_ref()
            .filter(|detail| Some(detail.uid.as_str()) == self.selected.as_deref())
    }

    /// The memory `uid` as the page holds it: its detail, or its row in the list.
    #[must_use]
    pub fn find(&self, uid: &str) -> Option<&Memory> {
        self.detail
            .as_ref()
            .filter(|detail| detail.uid == uid)
            .or_else(|| self.memories.iter().find(|memory| memory.uid == uid))
    }

    /// The memory selected, as the page holds it.
    #[must_use]
    pub fn selected_memory(&self) -> Option<&Memory> {
        self.find(self.selected.as_deref()?)
    }

    /// Whether the proposal `uid` is chosen for a bulk accept.
    #[must_use]
    pub fn is_chosen(&self, uid: &str) -> bool {
        self.chosen.contains(uid)
    }

    /// How many proposals are chosen for a bulk accept.
    #[must_use]
    pub fn chosen_count(&self) -> usize {
        self.chosen.len()
    }

    /// Changes the search text. Returns the generation of the change, or `None` when the text is
    /// the same.
    pub fn set_query(&mut self, text: &str) -> Option<u64> {
        if text == self.query {
            return None;
        }
        text.clone_into(&mut self.query);
        Some(self.invalidate())
    }

    /// Changes the kind filter. Returns the generation of the change, or `None` when unchanged.
    pub fn set_kind(&mut self, kind: Option<MemoryKind>) -> Option<u64> {
        if self.kind == kind {
            return None;
        }
        self.kind = kind;
        Some(self.invalidate())
    }

    /// Changes the status filter. Returns the generation of the change, or `None` when unchanged.
    pub fn set_status(&mut self, status: Option<MemoryStatus>) -> Option<u64> {
        if self.status == status {
            return None;
        }
        self.status = status;
        Some(self.invalidate())
    }

    /// Changes the file filter. Returns the generation of the change, or `None` when unchanged.
    pub fn set_path(&mut self, path: Option<String>) -> Option<u64> {
        if self.path == path {
            return None;
        }
        self.path = path;
        Some(self.invalidate())
    }

    /// Changes the author filter. Returns the generation of the change, or `None` when unchanged.
    pub fn set_author(&mut self, author: Option<String>) -> Option<u64> {
        if self.author == author {
            return None;
        }
        self.author = author;
        Some(self.invalidate())
    }

    /// Changes the scope filter. Returns the generation of the change, or `None` when unchanged.
    pub fn set_scope(&mut self, scope: Scope) -> Option<u64> {
        if self.scope == scope {
            return None;
        }
        self.scope = scope;
        Some(self.invalidate())
    }

    /// A filter or the search changed: the list read in flight is no longer wanted, and the
    /// cursor of the old list is not valid. The rows shown stay until the new list arrives.
    fn invalidate(&mut self) -> u64 {
        self.generation += 1;
        self.list_pending = None;
        self.next_cursor = None;
        self.generation
    }

    /// Whether `generation` is the latest change of the filters or the search.
    #[must_use]
    pub fn is_latest(&self, generation: u64) -> bool {
        self.generation == generation
    }

    /// The list from its first page, under the filters as they are now. A reload keeps as many
    /// rows as are shown.
    #[must_use]
    pub fn plan_list(&self) -> Request {
        let limit = u32::try_from(self.memories.len())
            .unwrap_or(u32::MAX)
            .max(PAGE);
        Request {
            kind: self.listing(None, limit),
            read: Read::List {
                append: false,
                generation: self.generation,
            },
        }
    }

    /// The page of the list after the rows shown, when one follows and no list read is in flight.
    #[must_use]
    pub fn plan_more(&self) -> Option<Request> {
        let cursor = self.next_cursor.clone()?;
        if !self.listed || self.list_pending.is_some() || self.failure.is_some() {
            return None;
        }
        Some(Request {
            kind: self.listing(Some(cursor), PAGE),
            read: Read::List {
                append: true,
                generation: self.generation,
            },
        })
    }

    fn listing(&self, cursor: Option<String>, limit: u32) -> CommandKind {
        CommandKind::ListMemories {
            query: self.query.trim().to_owned(),
            kind: self.kind,
            status: self.status,
            path: self.path.clone(),
            author: self.author.clone(),
            scope: self.scope.wire(),
            limit,
            cursor,
        }
    }

    /// The read of memory `uid` with its sources and relations.
    #[must_use]
    pub fn plan_read(uid: &str) -> Request {
        Request {
            kind: CommandKind::ReadMemory {
                uid: uid.to_owned(),
            },
            read: Read::Detail {
                uid: uid.to_owned(),
            },
        }
    }

    /// The read of the contexts the repository belongs to, for the scope filter.
    #[must_use]
    pub fn plan_contexts() -> Request {
        Request {
            kind: CommandKind::ListContexts { repo: true },
            read: Read::Contexts,
        }
    }

    /// Selects memory `uid` and returns the read of its detail. The detail shown is the one
    /// of the memory selected: another memory's is dropped.
    pub fn select(&mut self, uid: &str) -> Request {
        if self.selected.as_deref() != Some(uid) {
            self.selected = Some(uid.to_owned());
            if self.detail.as_ref().is_some_and(|detail| detail.uid != uid) {
                self.detail = None;
            }
        }
        Self::plan_read(uid)
    }

    /// Clears the selection and the detail shown.
    pub fn deselect(&mut self) {
        self.selected = None;
        self.detail = None;
        self.detail_pending = None;
    }

    /// Chooses or unchooses a proposal for a bulk accept. Only a proposed memory can be chosen.
    /// Returns whether the choice changed.
    pub fn toggle_chosen(&mut self, uid: &str) -> bool {
        if self.chosen.remove(uid) {
            return true;
        }
        let proposed = self
            .find(uid)
            .is_some_and(|memory| memory.status == MemoryStatus::Proposed);
        if proposed {
            self.chosen.insert(uid.to_owned());
        }
        proposed
    }

    /// Records that `read` was sent as operation `id`. A list read replaces the one in flight,
    /// whose late answer then does not match.
    pub fn sent(&mut self, read: &Read, id: OperationId) {
        self.failure = None;
        match read {
            Read::List { append, generation } => {
                self.list_pending = Some((id, *append, *generation));
            }
            Read::Detail { uid } => self.detail_pending = Some((id, uid.clone())),
            Read::Contexts => self.contexts_pending = Some(id),
            Read::Change { .. } => self.writes.push((id, read.clone())),
        }
    }

    /// Whether `id` is one of the reads or writes in flight.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.list_pending
            .is_some_and(|(pending, _, _)| pending == id)
            || self
                .detail_pending
                .as_ref()
                .is_some_and(|(pending, _)| *pending == id)
            || self.contexts_pending == Some(id)
            || self.writes.iter().any(|(pending, _)| *pending == id)
    }

    /// Applies a page of the list answered by `id`. Returns false for an answer that is not the
    /// list read in flight, or whose filters have changed since it was sent.
    pub fn listed(&mut self, id: OperationId, page: MemoryPage) -> bool {
        let Some((pending, append, generation)) = self.list_pending else {
            return false;
        };
        if pending != id || generation != self.generation {
            return false;
        }
        self.list_pending = None;
        if append {
            for memory in page.memories {
                if !self.memories.iter().any(|shown| shown.uid == memory.uid) {
                    self.memories.push(memory);
                }
            }
        } else {
            self.memories = page.memories;
        }
        order(&mut self.memories);
        self.next_cursor = page.next_cursor;
        self.contexts_read = page.contexts_read;
        self.note = page.note;
        self.proposed_total = Some(page.proposed_total);
        self.listed = true;
        self.prune_chosen();
        true
    }

    /// Applies the detail of memory `uid` answered by `id`, when it is the one selected and read
    /// in flight. The memory's row is updated too. Returns false for any other answer.
    pub fn read(&mut self, id: OperationId, memory: Memory) -> bool {
        let Some((pending, uid)) = self.detail_pending.clone() else {
            return false;
        };
        if pending != id || uid != memory.uid || self.selected.as_deref() != Some(uid.as_str()) {
            return false;
        }
        self.detail_pending = None;
        self.upsert(memory.clone());
        self.detail = Some(memory);
        true
    }

    /// Applies the contexts answered by `id`. Returns false for an answer that is not the read in flight.
    pub fn contexts_listed(&mut self, id: OperationId, contexts: Vec<Context>) -> bool {
        if self.contexts_pending != Some(id) {
            return false;
        }
        self.contexts_pending = None;
        self.contexts = contexts;
        true
    }

    /// Applies the answer of a write `id`: the memory as it now is, or none after a delete.
    /// Returns false for an answer that is not a write in flight.
    pub fn changed(&mut self, id: OperationId, memory: Option<Memory>) -> bool {
        let Some(index) = self.writes.iter().position(|(pending, _)| *pending == id) else {
            return false;
        };
        let (_, read) = self.writes.remove(index);
        let Read::Change { uid, removes } = read else {
            return false;
        };
        let shown = match &memory {
            Some(memory) => Some(memory.uid.clone()),
            None if removes => uid.clone(),
            None => None,
        };
        let was = shown.as_deref().and_then(|uid| self.loaded_proposed(uid));
        match memory {
            Some(memory) => self.upsert(memory),
            None if removes => {
                if let Some(uid) = uid {
                    self.forget(&uid);
                }
            }
            None => {}
        }
        let now = shown.as_deref().and_then(|uid| self.loaded_proposed(uid));
        self.move_total(was, now);
        true
    }

    /// Whether memory `uid` is loaded and proposed; `None` when it is not loaded.
    fn loaded_proposed(&self, uid: &str) -> Option<bool> {
        self.memories
            .iter()
            .find(|memory| memory.uid == uid)
            .map(|memory| memory.status == MemoryStatus::Proposed)
    }

    /// Moves the engine's total by a write that changed a loaded row between proposed and not
    /// (an accept, a reject, a delete). A row that was not loaded before leaves the total as the
    /// next list sets it.
    fn move_total(&mut self, was: Option<bool>, now: Option<bool>) {
        let Some(total) = self.proposed_total.as_mut() else {
            return;
        };
        match (was, now) {
            (Some(true), Some(false) | None) => *total = total.saturating_sub(1),
            (Some(false), Some(true)) => *total = total.saturating_add(1),
            _ => {}
        }
    }

    /// A read or write `id` ended without an answer. A failure is kept for the tab to show; a
    /// stopped read leaves the tab as it was.
    pub fn ended(&mut self, id: OperationId, state: OperationState, detail: Option<String>) {
        if !self.owns(id) || !state.is_terminal() {
            return;
        }
        if self
            .list_pending
            .is_some_and(|(pending, _, _)| pending == id)
        {
            self.list_pending = None;
        }
        if self
            .detail_pending
            .as_ref()
            .is_some_and(|(pending, _)| *pending == id)
        {
            self.detail_pending = None;
        }
        if self.contexts_pending == Some(id) {
            self.contexts_pending = None;
        }
        self.writes.retain(|(pending, _)| *pending != id);
        if state == OperationState::Failed {
            self.failure = Some(detail.unwrap_or_else(|| NO_MESSAGE.to_owned()));
        }
    }

    /// Puts a memory the engine answered into the list: replacing its row, or adding it. The
    /// detail is kept in step, and a row that the status filter no longer matches is taken out.
    fn upsert(&mut self, memory: Memory) {
        if self.status.is_some_and(|status| status != memory.status) {
            self.forget(&memory.uid);
            return;
        }
        if let Some(slot) = self
            .memories
            .iter_mut()
            .find(|shown| shown.uid == memory.uid)
        {
            *slot = memory.clone();
        } else {
            self.memories.push(memory.clone());
        }
        if let Some(detail) = self
            .detail
            .as_mut()
            .filter(|detail| detail.uid == memory.uid)
        {
            *detail = memory;
        }
        order(&mut self.memories);
        self.prune_chosen();
    }

    /// Drops memory `uid` from the page: its row, its choice, its detail and its selection.
    fn forget(&mut self, uid: &str) {
        self.memories.retain(|memory| memory.uid != uid);
        self.chosen.remove(uid);
        if self.detail.as_ref().is_some_and(|detail| detail.uid == uid) {
            self.detail = None;
        }
        if self.selected.as_deref() == Some(uid) {
            self.deselect();
        }
    }

    /// Keeps only the chosen proposals that are still loaded and still proposed.
    fn prune_chosen(&mut self) {
        let memories = &self.memories;
        self.chosen.retain(|uid| {
            memories
                .iter()
                .any(|memory| memory.uid == *uid && memory.status == MemoryStatus::Proposed)
        });
    }

    /// Accepts proposal `uid` as it is. Refused unless it is a loaded proposal.
    pub fn plan_accept(&self, uid: &str) -> Result<Request, &'static str> {
        self.proposal(uid)?;
        Ok(accept(uid, None, Vec::new()))
    }

    /// Accepts proposal `uid` with `text` in place of its text. The human edit makes the memory
    /// the human's (the engine sets the origin). Text the same as it is accepts as it is, since
    /// nothing was edited. Refused for a text that is empty or too long.
    pub fn plan_accept_edited(&self, uid: &str, text: &str) -> Result<Request, &'static str> {
        let memory = self.proposal(uid)?;
        let text = checked_text(text)?;
        let edited = (text != memory.text).then_some(text);
        Ok(accept(uid, edited, Vec::new()))
    }

    /// Rejects proposal `uid`. Refused unless it is a loaded proposal.
    pub fn plan_reject(&self, uid: &str) -> Result<Request, &'static str> {
        self.proposal(uid)?;
        Ok(Request {
            kind: CommandKind::RejectMemory {
                uid: uid.to_owned(),
            },
            read: Read::Change {
                uid: Some(uid.to_owned()),
                removes: false,
            },
        })
    }

    /// Deletes memory `uid` in any status. Refused unless it is loaded.
    pub fn plan_delete(&self, uid: &str) -> Result<Request, &'static str> {
        self.find(uid).ok_or(NOT_SHOWN)?;
        Ok(Request {
            kind: CommandKind::DeleteMemory {
                uid: uid.to_owned(),
            },
            read: Read::Change {
                uid: Some(uid.to_owned()),
                removes: true,
            },
        })
    }

    /// Accepts proposal `uid` and marks the accepted memories `supersedes` as superseded in the
    /// same write. Refused unless `uid` is a loaded proposal, `supersedes` names at least one
    /// loaded accepted memory and none of them is `uid`.
    pub fn plan_supersede(
        &self,
        uid: &str,
        supersedes: &[String],
    ) -> Result<Request, &'static str> {
        self.proposal(uid)?;
        if supersedes.is_empty() {
            return Err(NO_SUPERSEDED);
        }
        for old in supersedes {
            if old == uid {
                return Err(SELF_SUPERSEDE);
            }
            let memory = self.find(old).ok_or(NOT_SHOWN)?;
            if memory.status != MemoryStatus::Accepted {
                return Err(NOT_ACCEPTED);
            }
        }
        Ok(accept(uid, None, supersedes.to_vec()))
    }

    /// Accepts every chosen proposal, one write each, and clears the choice. Refused when none
    /// of the chosen memories is a loaded proposal.
    pub fn plan_bulk_accept(&mut self) -> Result<Vec<Request>, &'static str> {
        let uids: Vec<String> = self
            .chosen
            .iter()
            .filter(|uid| {
                self.memories
                    .iter()
                    .any(|memory| memory.uid == **uid && memory.status == MemoryStatus::Proposed)
            })
            .cloned()
            .collect();
        if uids.is_empty() {
            return Err(NO_SELECTION);
        }
        self.chosen.clear();
        Ok(uids
            .iter()
            .map(|uid| accept(uid, None, Vec::new()))
            .collect())
    }

    /// Accepts every proposal from `author` (the origin, such as `agent:claude-code`). The
    /// proposals are those of the whole list, so the list must be complete and show proposals
    /// under no narrowing filter but the author's. Refused otherwise, and when none is shown.
    pub fn plan_accept_author(&mut self, author: &str) -> Result<Vec<Request>, &'static str> {
        let author = author.trim();
        if author.is_empty() {
            return Err(NO_AUTHOR);
        }
        if !self.covers_every_proposal(author) {
            return Err(NOT_COMPLETE);
        }
        let uids: Vec<String> = self
            .memories
            .iter()
            .filter(|memory| memory.status == MemoryStatus::Proposed && memory.origin == author)
            .map(|memory| memory.uid.clone())
            .collect();
        if uids.is_empty() {
            return Err(NO_AUTHOR_PROPOSALS);
        }
        self.chosen.retain(|uid| !uids.contains(uid));
        Ok(uids
            .iter()
            .map(|uid| accept(uid, None, Vec::new()))
            .collect())
    }

    /// Whether the rows shown hold every proposal of `author`: the list is complete, its filters
    /// narrow by nothing but the author and the status (`None` or proposed), and no search runs.
    fn covers_every_proposal(&self, author: &str) -> bool {
        self.listed
            && self.list_pending.is_none()
            && self.next_cursor.is_none()
            && self.kind.is_none()
            && self.path.is_none()
            && self.scope == Scope::Everything
            && self.query.trim().is_empty()
            && matches!(self.status, None | Some(MemoryStatus::Proposed))
            && self.author.as_deref().is_none_or(|shown| shown == author)
    }

    /// Creates a memory of the human's, accepted at once. Refused for a text that is empty or too
    /// long, a scope that is not a place to write to, or a source that names nothing or a range
    /// that runs backwards.
    pub fn plan_new(new: &NewMemory) -> Result<Request, &'static str> {
        let text = checked_text(&new.text)?;
        let context = match &new.scope {
            Scope::Everything => return Err(NO_SCOPE),
            Scope::Repository => None,
            Scope::Context(path) => Some(path.clone()),
        };
        let mut sources = Vec::with_capacity(new.sources.len());
        for source in &new.sources {
            sources.push(checked_source(source)?);
        }
        Ok(Request {
            kind: CommandKind::ProposeMemory {
                kind: new.kind,
                text,
                sources,
                context,
            },
            read: Read::Change {
                uid: None,
                removes: false,
            },
        })
    }

    /// The loaded proposal `uid`, or the reason it cannot be acted on.
    fn proposal(&self, uid: &str) -> Result<&Memory, &'static str> {
        let memory = self.find(uid).ok_or(NOT_SHOWN)?;
        if memory.status != MemoryStatus::Proposed {
            return Err(NOT_PROPOSED);
        }
        Ok(memory)
    }
}

/// An accept write for `uid`, with the text it replaces and the memories it supersedes.
fn accept(uid: &str, text: Option<String>, supersedes: Vec<String>) -> Request {
    Request {
        kind: CommandKind::AcceptMemory {
            uid: uid.to_owned(),
            text,
            supersedes,
        },
        read: Read::Change {
            uid: Some(uid.to_owned()),
            removes: false,
        },
    }
}

/// Trims a memory text and checks its length: it must hold something and at most 1,000 characters.
pub fn checked_text(text: &str) -> Result<String, &'static str> {
    let text = text.trim();
    if text.is_empty() {
        Err(EMPTY_TEXT)
    } else if text.chars().count() > TEXT_CHARS {
        Err(TOO_LONG)
    } else {
        Ok(text.to_owned())
    }
}

/// Checks a source a human names for a memory: it names something, and a thread range runs forward.
fn checked_source(source: &MemorySourceInput) -> Result<MemorySourceInput, &'static str> {
    let reference = source.reference.trim();
    if reference.is_empty() {
        return Err(EMPTY_REF);
    }
    if let (Some(from), Some(to)) = (source.entry_from, source.entry_to)
        && from > to
    {
        return Err(BAD_RANGE);
    }
    Ok(MemorySourceInput {
        kind: source.kind,
        reference: reference.to_owned(),
        entry_from: source.entry_from,
        entry_to: source.entry_to,
    })
}

/// Proposals first, then the rest. The sort is stable, so the engine's order holds within each group.
fn order(memories: &mut [Memory]) {
    memories.sort_by_key(|memory| memory.status != MemoryStatus::Proposed);
}

/// The label of a memory's kind on its row.
#[must_use]
pub fn kind_label(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Decision => "Decision",
        MemoryKind::Rule => "Rule",
        MemoryKind::Lesson => "Lesson",
        MemoryKind::Fact => "Fact",
    }
}

/// The label of a memory's status on its row.
#[must_use]
pub fn status_label(status: MemoryStatus) -> &'static str {
    match status {
        MemoryStatus::Proposed => "Proposed",
        MemoryStatus::Accepted => "Accepted",
        MemoryStatus::Rejected => "Rejected",
        MemoryStatus::Superseded => "Superseded",
    }
}

/// Whether a memory's origin is an agent's: the `Agent-written` flag. It follows the origin, so a
/// proposal the human edited keeps no flag once its origin is the human's (D190).
#[must_use]
pub fn agent_written(origin: &str) -> bool {
    origin.starts_with(AGENT_ORIGIN)
}

/// The label of a memory's origin: who wrote it.
#[must_use]
pub fn origin_label(origin: &str) -> String {
    if origin == HUMAN_ORIGIN {
        "You".to_owned()
    } else if let Some(client) = origin.strip_prefix(AGENT_ORIGIN) {
        format!("Agent: {client}")
    } else if let Some(author) = origin.strip_prefix(IMPORTED_ORIGIN) {
        format!("Imported from {author}")
    } else {
        origin.to_owned()
    }
}

/// The meta line of a memory's row: its kind, its status and its origin.
#[must_use]
pub fn row_label(memory: &Memory) -> String {
    format!(
        "{} · {} · {}",
        kind_label(memory.kind),
        status_label(memory.status),
        origin_label(&memory.origin)
    )
}

/// The chip of a memory's scope: `Repository` for this repository, else the context path.
#[must_use]
pub fn scope_chip(scope: &str) -> String {
    if scope.is_empty() || scope == REPOSITORY {
        "Repository".to_owned()
    } else {
        scope.to_owned()
    }
}

/// The files a memory names that differ from when it was accepted, split by what is known about
/// them. `unavailable` and `unknown` files are never in `changed` (D193 (9)).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilesChanged {
    /// Files whose blob at `HEAD` differs from the one accepted.
    pub changed: Vec<String>,
    /// Files gone from `HEAD`.
    pub removed: Vec<String>,
    /// Files Git could not answer for: not known to be changed or unchanged.
    pub unknown: Vec<String>,
    /// Files nothing can be read for (the repository is not here, or the blob was not read).
    pub unavailable: Vec<String>,
}

/// Splits the `files_changed_since` of a memory by state.
#[must_use]
pub fn files_changed(changes: &[FileChange]) -> FilesChanged {
    let mut split = FilesChanged::default();
    for change in changes {
        let path = change.path.clone();
        match change.state {
            FileChangeState::Changed => split.changed.push(path),
            FileChangeState::Missing => split.removed.push(path),
            FileChangeState::Unknown => split.unknown.push(path),
            FileChangeState::Unavailable => split.unavailable.push(path),
        }
    }
    split
}

impl FilesChanged {
    /// Whether a file the memory names changed or went: the memory may be stale (D192 (6)).
    /// Unknown and unavailable files do not make it stale.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        !self.changed.is_empty() || !self.removed.is_empty()
    }

    /// The lines the memory shows about its files, one per state, none for a state with no file.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.changed.is_empty() {
            lines.push(format!(
                "Changed since accepted: {}",
                self.changed.join(", ")
            ));
        }
        if !self.removed.is_empty() {
            lines.push(format!(
                "Removed since accepted: {}",
                self.removed.join(", ")
            ));
        }
        if !self.unknown.is_empty() {
            lines.push(format!("Git could not check: {}", self.unknown.join(", ")));
        }
        if !self.unavailable.is_empty() {
            lines.push(format!(
                "Unavailable, not checked: {}",
                self.unavailable.join(", ")
            ));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::threads::{MemorySource, MemorySourceKind};

    fn memory(uid: &str, status: MemoryStatus, origin: &str, text: &str) -> Memory {
        Memory {
            id: 0,
            uid: uid.to_owned(),
            kind: MemoryKind::Rule,
            text: text.to_owned(),
            status,
            origin: origin.to_owned(),
            proposed_by: None,
            model: None,
            created_at: 1_000,
            decided_at: None,
            decided_by: None,
            reviewed: status != MemoryStatus::Proposed,
            sources: Vec::<MemorySource>::new(),
            supersedes: Vec::new(),
            superseded_by: Vec::new(),
            files_changed_since: Vec::new(),
            scope: REPOSITORY.to_owned(),
        }
    }

    fn page(memories: Vec<Memory>, cursor: Option<&str>) -> MemoryPage {
        let proposed_total = u32::try_from(
            memories
                .iter()
                .filter(|memory| memory.status == MemoryStatus::Proposed)
                .count(),
        )
        .unwrap_or(u32::MAX);
        MemoryPage {
            memories,
            next_cursor: cursor.map(str::to_owned),
            contexts_read: true,
            note: None,
            proposed_total,
        }
    }

    /// A page as `page`, with the engine's exact `proposed_total` given.
    fn page_with_total(memories: Vec<Memory>, proposed_total: u32) -> MemoryPage {
        MemoryPage {
            proposed_total,
            ..page(memories, None)
        }
    }

    /// A page with a proposal and two accepted memories, the proposal last in the engine's order.
    fn loaded() -> MemoriesPage {
        let mut tab = MemoriesPage::default();
        let read = tab.plan_list();
        tab.sent(&read.read, OperationId(1));
        tab.listed(
            OperationId(1),
            page(
                vec![
                    memory("a", MemoryStatus::Accepted, "human", "first"),
                    memory("p1", MemoryStatus::Proposed, "agent:claude-code", "one"),
                    memory("b", MemoryStatus::Accepted, "human", "second"),
                    memory("p2", MemoryStatus::Proposed, "agent:claude-code", "two"),
                ],
                None,
            ),
        );
        tab
    }

    fn kind_of(request: &Request) -> &CommandKind {
        &request.kind
    }

    #[test]
    fn proposals_come_first_and_count_for_the_tab() {
        let tab = loaded();
        let statuses: Vec<MemoryStatus> = tab.memories().iter().map(|m| m.status).collect();
        assert_eq!(
            statuses,
            vec![
                MemoryStatus::Proposed,
                MemoryStatus::Proposed,
                MemoryStatus::Accepted,
                MemoryStatus::Accepted,
            ]
        );
        let uids: Vec<&str> = tab.memories().iter().map(|m| m.uid.as_str()).collect();
        assert_eq!(
            uids,
            ["p1", "p2", "a", "b"],
            "engine order holds within each group"
        );
        assert_eq!(tab.proposed_count(), 2);
        assert!(tab.is_loaded());
        assert!(!tab.has_more());
    }

    #[test]
    fn the_count_is_the_engine_total_once_a_list_is_applied() {
        // One proposal is loaded, but the engine counts five on the whole list (a page of one).
        let mut tab = MemoriesPage::default();
        assert_eq!(tab.proposed_count(), 0, "nothing is counted before a list");
        let read = tab.plan_list();
        tab.sent(&read.read, OperationId(1));
        assert!(tab.listed(
            OperationId(1),
            page_with_total(
                vec![memory(
                    "p1",
                    MemoryStatus::Proposed,
                    "agent:claude-code",
                    "one"
                )],
                5,
            ),
        ));
        assert_eq!(tab.proposed_count(), 5);
        assert_eq!(tab.memories().len(), 1);
    }

    #[test]
    fn a_write_moves_the_total_only_for_a_loaded_proposal() {
        let mut tab = MemoriesPage::default();
        let read = tab.plan_list();
        tab.sent(&read.read, OperationId(1));
        tab.listed(
            OperationId(1),
            page_with_total(
                vec![
                    memory("p1", MemoryStatus::Proposed, "agent:claude-code", "one"),
                    memory("p2", MemoryStatus::Proposed, "agent:claude-code", "two"),
                ],
                5,
            ),
        );
        let accept = tab.plan_accept("p1").expect("accept");
        tab.sent(&accept.read, OperationId(2));
        assert!(tab.changed(
            OperationId(2),
            Some(memory("p1", MemoryStatus::Accepted, "human", "one")),
        ));
        assert_eq!(tab.proposed_count(), 4);
        let delete = tab.plan_delete("p2").expect("delete");
        tab.sent(&delete.read, OperationId(3));
        assert!(tab.changed(OperationId(3), None));
        assert_eq!(tab.proposed_count(), 3);
    }

    #[test]
    fn a_later_list_replaces_the_total() {
        let mut tab = MemoriesPage::default();
        let read = tab.plan_list();
        tab.sent(&read.read, OperationId(1));
        tab.listed(OperationId(1), page_with_total(Vec::new(), 3));
        assert_eq!(tab.proposed_count(), 3);
        let read = tab.plan_list();
        tab.sent(&read.read, OperationId(2));
        tab.listed(OperationId(2), page_with_total(Vec::new(), 0));
        assert_eq!(tab.proposed_count(), 0);
    }

    #[test]
    fn a_page_after_the_first_appends_and_keeps_proposals_first() {
        let mut tab = MemoriesPage::default();
        let first = tab.plan_list();
        tab.sent(&first.read, OperationId(1));
        tab.listed(
            OperationId(1),
            page(
                vec![memory("a", MemoryStatus::Accepted, "human", "x")],
                Some("c1"),
            ),
        );
        assert!(tab.has_more());
        let more = tab.plan_more().expect("a page follows");
        assert!(matches!(
            kind_of(&more),
            CommandKind::ListMemories { cursor: Some(c), .. } if c == "c1"
        ));
        tab.sent(&more.read, OperationId(2));
        assert!(tab.listed(
            OperationId(2),
            page(
                vec![
                    memory("a", MemoryStatus::Accepted, "human", "x"),
                    memory("p", MemoryStatus::Proposed, "agent:x", "y"),
                ],
                None,
            ),
        ));
        let uids: Vec<&str> = tab.memories().iter().map(|m| m.uid.as_str()).collect();
        assert_eq!(uids, ["p", "a"]);
        assert!(!tab.has_more());
        assert!(tab.plan_more().is_none());
    }

    #[test]
    fn a_late_list_answer_is_dropped_after_a_filter_change() {
        let mut tab = loaded();
        let stale = tab.plan_list();
        tab.sent(&stale.read, OperationId(10));
        let generation = tab
            .set_status(Some(MemoryStatus::Proposed))
            .expect("changed");
        assert!(tab.is_latest(generation));
        assert!(!tab.is_latest(generation - 1));
        // The answer to the list read before the change is dropped, and the rows stay as they were.
        assert!(!tab.listed(
            OperationId(10),
            page(
                vec![memory("old", MemoryStatus::Accepted, "human", "stale")],
                None
            ),
        ));
        assert!(tab.memories().iter().all(|m| m.uid != "old"));
        // A read sent after the change is applied.
        let fresh = tab.plan_list();
        tab.sent(&fresh.read, OperationId(11));
        assert!(tab.listed(
            OperationId(11),
            page(
                vec![memory("p1", MemoryStatus::Proposed, "agent:x", "one")],
                None
            ),
        ));
        assert_eq!(tab.memories().len(), 1);
    }

    #[test]
    fn a_filter_change_ends_a_page_in_flight_and_its_cursor() {
        let mut tab = MemoriesPage::default();
        let first = tab.plan_list();
        tab.sent(&first.read, OperationId(1));
        tab.listed(OperationId(1), page(vec![], Some("c1")));
        let more = tab.plan_more().expect("a page follows");
        tab.sent(&more.read, OperationId(2));
        assert!(tab.set_query("retry").is_some());
        assert!(!tab.listed(OperationId(2), page(vec![], None)));
        assert!(
            !tab.has_more(),
            "the old cursor is not valid for the new search"
        );
    }

    #[test]
    fn a_setter_that_changes_nothing_bumps_no_generation() {
        let mut tab = MemoriesPage::default();
        assert_eq!(tab.set_query(""), None);
        assert_eq!(tab.set_kind(None), None);
        assert_eq!(tab.set_scope(Scope::Everything), None);
        let generation = tab.set_kind(Some(MemoryKind::Rule)).expect("changed");
        assert_eq!(tab.set_kind(Some(MemoryKind::Rule)), None);
        assert!(tab.is_latest(generation));
    }

    #[test]
    fn the_list_command_carries_the_filters_and_the_scope() {
        let mut tab = MemoriesPage::default();
        tab.set_query("  retry  ");
        tab.set_status(Some(MemoryStatus::Proposed));
        tab.set_scope(Scope::Context("Platform / Billing".to_owned()));
        let request = tab.plan_list();
        match kind_of(&request) {
            CommandKind::ListMemories {
                query,
                status,
                scope,
                cursor,
                ..
            } => {
                assert_eq!(query, "retry");
                assert_eq!(*status, Some(MemoryStatus::Proposed));
                assert_eq!(scope.as_deref(), Some("Platform / Billing"));
                assert!(cursor.is_none());
            }
            other => panic!("not a list: {other:?}"),
        }
        tab.set_scope(Scope::Repository);
        assert!(matches!(
            kind_of(&tab.plan_list()),
            CommandKind::ListMemories { scope: Some(s), .. } if s == REPOSITORY
        ));
    }

    #[test]
    fn a_detail_is_applied_only_for_the_selected_memory() {
        let mut tab = loaded();
        let request = tab.select("p1");
        tab.sent(&request.read, OperationId(5));
        assert!(tab.is_loading_detail());
        // Another memory is selected before the answer arrives: the answer is dropped.
        let other = tab.select("a");
        tab.sent(&other.read, OperationId(6));
        assert!(!tab.read(
            OperationId(5),
            memory("p1", MemoryStatus::Proposed, "agent:x", "one")
        ));
        assert!(tab.detail().is_none());
        assert!(tab.read(
            OperationId(6),
            memory("a", MemoryStatus::Accepted, "human", "first")
        ));
        assert_eq!(tab.detail().map(|m| m.uid.as_str()), Some("a"));
        assert_eq!(
            tab.selected_memory().map(|m| m.text.as_str()),
            Some("first")
        );
    }

    #[test]
    fn accept_is_planned_only_for_a_loaded_proposal() {
        let tab = loaded();
        let request = tab.plan_accept("p1").expect("a proposal can be accepted");
        assert!(matches!(
            kind_of(&request),
            CommandKind::AcceptMemory { uid, text: None, supersedes } if uid == "p1" && supersedes.is_empty()
        ));
        assert_eq!((tab.plan_accept("a")).err(), Some(NOT_PROPOSED));
        assert_eq!((tab.plan_accept("missing")).err(), Some(NOT_SHOWN));
    }

    #[test]
    fn accept_with_an_edit_sends_the_new_text_and_no_text_when_it_is_the_same() {
        let tab = loaded();
        let edited = tab
            .plan_accept_edited("p1", "  a better wording  ")
            .expect("edit accepted");
        assert!(matches!(
            kind_of(&edited),
            CommandKind::AcceptMemory { text: Some(t), .. } if t == "a better wording"
        ));
        let same = tab
            .plan_accept_edited("p1", "one")
            .expect("unchanged accepted");
        assert!(matches!(
            kind_of(&same),
            CommandKind::AcceptMemory { text: None, .. }
        ));
        assert_eq!(
            (tab.plan_accept_edited("p1", "   ")).err(),
            Some(EMPTY_TEXT)
        );
        assert_eq!((tab.plan_accept_edited("a", "x")).err(), Some(NOT_PROPOSED));
    }

    #[test]
    fn reject_is_planned_only_for_a_loaded_proposal() {
        let tab = loaded();
        let request = tab.plan_reject("p2").expect("rejected");
        assert!(matches!(kind_of(&request), CommandKind::RejectMemory { uid } if uid == "p2"));
        assert_eq!((tab.plan_reject("b")).err(), Some(NOT_PROPOSED));
    }

    #[test]
    fn delete_is_planned_for_any_loaded_memory() {
        let tab = loaded();
        assert!(matches!(
            kind_of(&tab.plan_delete("b").expect("accepted memories can be deleted")),
            CommandKind::DeleteMemory { uid } if uid == "b"
        ));
        assert_eq!((tab.plan_delete("gone")).err(), Some(NOT_SHOWN));
        let request = tab.plan_delete("p1").expect("delete");
        assert!(matches!(request.read, Read::Change { removes: true, .. }));
    }

    #[test]
    fn supersede_accepts_a_proposal_and_names_the_accepted_memories_it_replaces() {
        let tab = loaded();
        let request = tab
            .plan_supersede("p1", &["a".to_owned()])
            .expect("supersedes an accepted memory");
        assert!(matches!(
            kind_of(&request),
            CommandKind::AcceptMemory { uid, supersedes, .. } if uid == "p1" && supersedes == &["a"]
        ));
        assert_eq!((tab.plan_supersede("p1", &[])).err(), Some(NO_SUPERSEDED));
        assert_eq!(
            (tab.plan_supersede("a", &["b".to_owned()])).err(),
            Some(NOT_PROPOSED)
        );
        assert_eq!(
            tab.plan_supersede("p1", &["p2".to_owned()]).err(),
            Some(NOT_ACCEPTED),
            "only an accepted memory can be superseded"
        );
        assert_eq!(
            (tab.plan_supersede("p1", &["p1".to_owned()])).err(),
            Some(SELF_SUPERSEDE)
        );
        assert_eq!(
            (tab.plan_supersede("p1", &["nope".to_owned()])).err(),
            Some(NOT_SHOWN)
        );
    }

    #[test]
    fn bulk_accept_sends_one_write_per_chosen_proposal_and_clears_the_choice() {
        let mut tab = loaded();
        assert_eq!((tab.plan_bulk_accept()).err(), Some(NO_SELECTION));
        assert!(tab.toggle_chosen("p1"));
        assert!(tab.toggle_chosen("p2"));
        assert!(
            !tab.toggle_chosen("a"),
            "an accepted memory cannot be chosen"
        );
        assert_eq!(tab.chosen_count(), 2);
        let requests = tab.plan_bulk_accept().expect("two accepts");
        assert_eq!(requests.len(), 2);
        assert_eq!(tab.chosen_count(), 0);
        assert!(tab.toggle_chosen("p1"));
        assert_eq!(tab.chosen_count(), 1);
    }

    #[test]
    fn accept_by_author_takes_every_proposal_from_that_author_in_a_complete_list() {
        let mut tab = MemoriesPage::default();
        let read = tab.plan_list();
        tab.sent(&read.read, OperationId(1));
        tab.listed(
            OperationId(1),
            page(
                vec![
                    memory("p1", MemoryStatus::Proposed, "agent:claude-code", "one"),
                    memory("p2", MemoryStatus::Proposed, "agent:other", "two"),
                    memory("p3", MemoryStatus::Proposed, "agent:claude-code", "three"),
                    memory("a", MemoryStatus::Accepted, "agent:claude-code", "four"),
                ],
                None,
            ),
        );
        let requests = tab
            .plan_accept_author("agent:claude-code")
            .expect("two proposals from the author");
        let uids: Vec<String> = requests
            .iter()
            .filter_map(|request| match kind_of(request) {
                CommandKind::AcceptMemory { uid, .. } => Some(uid.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(uids, ["p1", "p3"]);
        assert_eq!((tab.plan_accept_author("  ")).err(), Some(NO_AUTHOR));
        assert_eq!(
            (tab.plan_accept_author("agent:nobody")).err(),
            Some(NO_AUTHOR_PROPOSALS)
        );
    }

    #[test]
    fn accept_by_author_is_refused_while_the_list_is_narrowed_or_incomplete() {
        let mut tab = loaded();
        tab.set_kind(Some(MemoryKind::Rule));
        assert_eq!(
            (tab.plan_accept_author("agent:claude-code")).err(),
            Some(NOT_COMPLETE)
        );
        let mut paged = MemoriesPage::default();
        let read = paged.plan_list();
        paged.sent(&read.read, OperationId(1));
        paged.listed(
            OperationId(1),
            page(
                vec![memory("p1", MemoryStatus::Proposed, "agent:x", "one")],
                Some("more"),
            ),
        );
        assert_eq!(
            (paged.plan_accept_author("agent:x")).err(),
            Some(NOT_COMPLETE)
        );
    }

    fn new_memory(text: &str, scope: Scope) -> NewMemory {
        NewMemory {
            kind: MemoryKind::Decision,
            scope,
            text: text.to_owned(),
            sources: Vec::new(),
        }
    }

    #[test]
    fn a_new_memory_needs_text_of_at_most_a_thousand_characters() {
        assert_eq!(
            (MemoriesPage::plan_new(&new_memory("   ", Scope::Repository))).err(),
            Some(EMPTY_TEXT)
        );
        let long = "x".repeat(TEXT_CHARS + 1);
        assert_eq!(
            (MemoriesPage::plan_new(&new_memory(&long, Scope::Repository))).err(),
            Some(TOO_LONG)
        );
        let exact = "é".repeat(TEXT_CHARS);
        let request = MemoriesPage::plan_new(&new_memory(&exact, Scope::Repository))
            .expect("exactly 1,000 characters is allowed");
        assert!(matches!(
            kind_of(&request),
            CommandKind::ProposeMemory { text, context: None, .. } if text.chars().count() == TEXT_CHARS
        ));
    }

    #[test]
    fn a_new_memory_takes_a_scope_and_refuses_everything() {
        assert_eq!(
            (MemoriesPage::plan_new(&new_memory("a rule", Scope::Everything))).err(),
            Some(NO_SCOPE)
        );
        let request =
            MemoriesPage::plan_new(&new_memory("a rule", Scope::Context("Platform".to_owned())))
                .expect("context");
        assert!(matches!(
            kind_of(&request),
            CommandKind::ProposeMemory { context: Some(c), .. } if c == "Platform"
        ));
        assert_eq!(
            request.read,
            Read::Change {
                uid: None,
                removes: false
            }
        );
    }

    #[test]
    fn a_new_memory_checks_its_sources() {
        let mut memory = new_memory("a fact", Scope::Repository);
        memory.sources = vec![MemorySourceInput {
            kind: MemorySourceKind::File,
            reference: "  ".to_owned(),
            entry_from: None,
            entry_to: None,
        }];
        assert_eq!((MemoriesPage::plan_new(&memory)).err(), Some(EMPTY_REF));
        memory.sources = vec![MemorySourceInput {
            kind: MemorySourceKind::Thread,
            reference: "t1".to_owned(),
            entry_from: Some(9),
            entry_to: Some(3),
        }];
        assert_eq!((MemoriesPage::plan_new(&memory)).err(), Some(BAD_RANGE));
        memory.sources = vec![MemorySourceInput {
            kind: MemorySourceKind::File,
            reference: " src/lib.rs ".to_owned(),
            entry_from: None,
            entry_to: None,
        }];
        let request = MemoriesPage::plan_new(&memory).expect("a file source");
        assert!(matches!(
            kind_of(&request),
            CommandKind::ProposeMemory { sources, .. } if sources[0].reference == "src/lib.rs"
        ));
    }

    #[test]
    fn an_answer_to_a_write_puts_the_memory_in_place_or_removes_it() {
        let mut tab = loaded();
        let request = tab.plan_accept("p1").expect("accept");
        tab.sent(&request.read, OperationId(20));
        assert!(tab.owns(OperationId(20)));
        assert!(tab.changed(
            OperationId(20),
            Some(memory("p1", MemoryStatus::Accepted, "human", "one")),
        ));
        assert_eq!(tab.proposed_count(), 1);
        let delete = tab.plan_delete("b").expect("delete");
        tab.sent(&delete.read, OperationId(21));
        assert!(tab.changed(OperationId(21), None));
        assert!(tab.find("b").is_none());
        assert!(
            !tab.changed(OperationId(99), None),
            "an unknown answer is dropped"
        );
    }

    #[test]
    fn a_status_filter_drops_a_row_the_status_no_longer_matches() {
        let mut tab = loaded();
        tab.set_status(Some(MemoryStatus::Proposed));
        let request = tab.plan_accept("p1").expect("accept");
        tab.sent(&request.read, OperationId(30));
        tab.changed(
            OperationId(30),
            Some(memory("p1", MemoryStatus::Accepted, "human", "one")),
        );
        assert!(tab.find("p1").is_none());
        assert_eq!(tab.proposed_count(), 1);
    }

    #[test]
    fn a_failed_read_is_kept_and_a_stopped_one_leaves_the_tab() {
        let mut tab = MemoriesPage::default();
        let read = tab.plan_list();
        tab.sent(&read.read, OperationId(1));
        tab.ended(OperationId(1), OperationState::Failed, None);
        assert_eq!(tab.failure(), Some(NO_MESSAGE));
        assert!(!tab.is_loading_list());
        let again = tab.plan_list();
        tab.sent(&again.read, OperationId(2));
        assert_eq!(
            tab.failure(),
            None,
            "a new read clears the failure it replaces"
        );
        tab.ended(
            OperationId(2),
            OperationState::Cancelled,
            Some("stopped".to_owned()),
        );
        assert_eq!(tab.failure(), None, "a stopped read reports no failure");
    }

    #[test]
    fn contexts_are_applied_only_for_the_read_in_flight() {
        let mut tab = MemoriesPage::default();
        let request = MemoriesPage::plan_contexts();
        tab.sent(&request.read, OperationId(4));
        assert!(!tab.contexts_listed(OperationId(3), Vec::new()));
        assert!(tab.contexts_listed(OperationId(4), Vec::new()));
        assert!(tab.contexts().is_empty());
    }

    #[test]
    fn the_scope_chip_names_the_repository_or_the_context_path() {
        assert_eq!(scope_chip("repository"), "Repository");
        assert_eq!(scope_chip(""), "Repository");
        assert_eq!(scope_chip("Platform / Billing"), "Platform / Billing");
    }

    #[test]
    fn an_agent_origin_is_flagged_and_labelled_and_a_human_one_is_not() {
        assert!(agent_written("agent:claude-code"));
        assert!(!agent_written("human"));
        assert!(!agent_written("imported:someone"));
        assert_eq!(origin_label("human"), "You");
        assert_eq!(origin_label("agent:claude-code"), "Agent: claude-code");
        assert_eq!(origin_label("imported:alice"), "Imported from alice");
        let row = memory("p", MemoryStatus::Proposed, "agent:claude-code", "x");
        assert_eq!(row_label(&row), "Rule · Proposed · Agent: claude-code");
    }

    #[test]
    fn a_changed_file_is_stale_and_an_unavailable_one_is_never_shown_as_changed() {
        let changes = vec![
            FileChange {
                path: "src/a.rs".to_owned(),
                state: FileChangeState::Changed,
            },
            FileChange {
                path: "src/gone.rs".to_owned(),
                state: FileChangeState::Missing,
            },
            FileChange {
                path: "src/b.rs".to_owned(),
                state: FileChangeState::Unavailable,
            },
            FileChange {
                path: "src/c.rs".to_owned(),
                state: FileChangeState::Unknown,
            },
        ];
        let split = files_changed(&changes);
        assert_eq!(split.changed, ["src/a.rs"]);
        assert_eq!(split.removed, ["src/gone.rs"]);
        assert_eq!(split.unavailable, ["src/b.rs"]);
        assert_eq!(split.unknown, ["src/c.rs"]);
        assert!(split.is_stale());
        let lines = split.lines();
        assert_eq!(lines[0], "Changed since accepted: src/a.rs");
        assert!(lines.iter().any(|line| line.starts_with("Unavailable")));
        assert!(!lines[0].contains("src/b.rs"));
    }

    #[test]
    fn only_unavailable_or_unknown_files_do_not_make_a_memory_stale() {
        let changes = vec![
            FileChange {
                path: "src/b.rs".to_owned(),
                state: FileChangeState::Unavailable,
            },
            FileChange {
                path: "src/c.rs".to_owned(),
                state: FileChangeState::Unknown,
            },
        ];
        let split = files_changed(&changes);
        assert!(!split.is_stale());
        assert!(split.changed.is_empty());
        assert_eq!(split.lines().len(), 2);
        assert!(files_changed(&[]).lines().is_empty());
    }
}
