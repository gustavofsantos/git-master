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

use std::collections::BTreeSet;

use gm_schema::threads::{
    Context, Memory, MemoryBasis, MemoryContext, MemoryMark, MemoryMarkSummary, MemorySourceInput,
};
use gm_schema::{
    CommandKind, FileChange, FileChangeState, MemoryKind, MemoryStatus, OperationId, OperationState,
};

/// Memories per page of the list.
pub const PAGE: u32 = 50;
/// The longest memory text, in characters (D190).
pub const TEXT_CHARS: usize = 1_000;
/// The scope the engine gives a memory of this repository.
const REPOSITORY: &str = "repository";
/// The scope the engine reads as the contexts the repository is bound to (D223 (1)).
const BOUND: &str = "bound";
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
const NO_SUPERSEDED: &str = "Choose the memory this one supersedes.";
const SELF_SUPERSEDE: &str = "A memory cannot supersede itself.";
const NO_SELECTION: &str = "Choose proposed memories to accept.";
const NO_AUTHOR: &str = "Choose an author whose proposals to accept.";
const NOT_COMPLETE: &str = "Clear the filters and search, and load the whole list, to accept every proposal from an author.";
const NO_AUTHOR_PROPOSALS: &str = "No proposal from that author is shown.";
const BAD_RANGE: &str = "A thread range starts after it ends.";
const EMPTY_REF: &str = "A source needs a thread, a commit or a file.";
const NOT_KNOWN: &str = "Choose a context of the tree.";
const NOT_MOVABLE: &str = "Only a proposed or accepted memory can move to a context.";
const LINKED: &str = "A memory with supersedes links cannot move to a context.";
const MOVE_FIRST: &str = "This memory is in this repository only: move it to a context first.";
const NOT_ACCEPTED_TO_ATTACH: &str = "Only an accepted memory can be attached to another context.";
const NO_CONTEXT_OF_ITS_OWN: &str =
    "This memory is in this repository only, so it has no context to detach from.";
const NOT_ATTACHED_TO_IT: &str = "The memory is not attached to that context.";
const NOT_ACCEPTED_TO_MARK: &str = "Only an accepted memory can be marked.";
const REASON_LINE: &str = "A reason is one line.";
const REASON_TOO_LONG: &str = "A reason holds at most 200 characters.";
/// The longest reason a mark carries, in characters (D202 (1)).
pub const REASON_CHARS: usize = 200;
/// The actor of a mark the person makes in the app.
pub const HUMAN_ACTOR: &str = "human";

/// Which memories the list shows by scope (D223 (1)): the contexts this repository is bound to and
/// those above them, by default, or every memory of one context, bound or not, by its path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// The memories of the contexts the repository is bound to and of every context above them.
    #[default]
    Bound,
    /// The memories of one context, by its path such as `Platform / Billing`, with what it inherits.
    Context(String),
}

impl Scope {
    /// The `scope` the list command takes: `bound` for the repository's contexts, else a path.
    fn wire(&self) -> String {
        match self {
            Self::Bound => BOUND.to_owned(),
            Self::Context(path) => path.clone(),
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
    /// The exact number of proposed memories the repository sees, whatever the request's filters,
    /// scope, status, query and page (the engine's `proposed_total`).
    pub proposed_total: u32,
    /// The accepted memories of the scope and the characters of their text (the engine's
    /// `accepted_total` and `accepted_characters`): the size line (D201 (5)).
    pub accepted_total: u32,
    pub accepted_characters: u64,
}

/// A memory the human writes. It is accepted at once (D190).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewMemory {
    pub kind: MemoryKind,
    /// The context the memory goes to, by its path. `None` leaves the choice to the engine, which puts
    /// it in the one context the repository is bound to, or in the context of the repository (D223 (5)).
    pub context: Option<String>,
    pub text: String,
    /// Optional sources: a thread range, a commit or a file.
    pub sources: Vec<MemorySourceInput>,
    /// How the memory is known (D201 (1)). The form starts at `observed`.
    pub basis: MemoryBasis,
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
    /// The size of the accepted memories of the scope, from the last list applied (D201 (5)).
    accepted: Option<(u32, u64)>,
    /// The list read in flight: its id, whether it appends, and the generation it was sent under.
    list_pending: Option<(OperationId, bool, u64)>,
    selected: Option<String>,
    detail: Option<Memory>,
    detail_pending: Option<(OperationId, String)>,
    contexts: Vec<Context>,
    contexts_pending: Option<OperationId>,
    /// The id of the open repository, when it has one; the contexts answer gives it (D193 (4)).
    repo_id: Option<String>,
    /// The writes in flight, each with what it changes.
    writes: Vec<(OperationId, Read)>,
    /// The proposals chosen for a bulk accept.
    chosen: BTreeSet<String>,
    /// The mark history of the memory `uid`, read with its detail (D202 (2)).
    history: Option<(String, Vec<MemoryMarkSummary>)>,
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

    /// Whether any filter or the search narrows the list.
    #[must_use]
    pub fn filter_active(&self) -> bool {
        self.kind.is_some()
            || self.status.is_some()
            || self.path.is_some()
            || self.author.is_some()
            || self.scope != Scope::Bound
            || !self.query.trim().is_empty()
    }

    /// The memories loaded, proposed first, in the order the engine gave within each group.
    #[must_use]
    pub fn memories(&self) -> &[Memory] {
        &self.memories
    }

    /// How many proposed memories the badge on the tab shows: the engine's exact `proposed_total`
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

    /// The size line of the tab: the accepted memories of the scope and their text (D201 (5)). `None`
    /// before a list is applied.
    #[must_use]
    pub fn size_line(&self) -> Option<String> {
        self.accepted
            .map(|(total, characters)| size_line(total, characters))
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

    /// Whether the contexts the list reads could be read (false when the engine says why not).
    #[must_use]
    pub fn contexts_read(&self) -> bool {
        self.contexts_read
    }

    /// The id of the open repository, when it has one. A source whose repository is another one
    /// is shown, not opened.
    #[must_use]
    pub fn repo_id(&self) -> Option<&str> {
        self.repo_id.as_deref()
    }

    /// Keeps the id of the open repository the contexts answer gave.
    pub fn set_repo_id(&mut self, repo_id: Option<String>) {
        self.repo_id = repo_id;
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

    /// The full mark history of the selected memory, oldest first, once its detail is read (D202 (2)).
    #[must_use]
    pub fn mark_history(&self) -> Option<&[MemoryMarkSummary]> {
        let (uid, history) = self.history.as_ref()?;
        (Some(uid.as_str()) == self.selected.as_deref()).then_some(history.as_slice())
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
            scope: Some(self.scope.wire()),
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
            kind: CommandKind::ListContexts {
                repo: true,
                all: true,
            },
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
        self.history = None;
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
        self.accepted = Some((page.accepted_total, page.accepted_characters));
        self.listed = true;
        self.prune_chosen();
        true
    }

    /// Applies the detail of memory `uid` answered by `id`, when it is the one selected and read
    /// in flight. The memory's row is updated too. Returns false for any other answer.
    pub fn read(
        &mut self,
        id: OperationId,
        memory: Memory,
        history: Vec<MemoryMarkSummary>,
    ) -> bool {
        let Some((pending, uid)) = self.detail_pending.clone() else {
            return false;
        };
        if pending != id || uid != memory.uid || self.selected.as_deref() != Some(uid.as_str()) {
            return false;
        }
        self.detail_pending = None;
        self.upsert(memory.clone());
        self.history = Some((uid, history));
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

    /// Records the person's `mark` on memory `uid` (D202 (1), (2)). Refused unless `uid` is loaded and
    /// accepted, or when the reason is not one line of at most 200 characters. `Ok(None)` when the
    /// person's latest mark already is `mark`: nothing is sent. A blank reason is none.
    pub fn plan_mark(
        &self,
        uid: &str,
        mark: MemoryMark,
        reason: &str,
    ) -> Result<Option<Request>, &'static str> {
        let memory = self.find(uid).ok_or(NOT_SHOWN)?;
        check_mark(Placement::of(memory))?;
        let reason = checked_reason(reason)?;
        if own_mark(&memory.marks) == Some(mark) {
            return Ok(None);
        }
        Ok(Some(Request {
            kind: CommandKind::MarkMemory {
                uid: uid.to_owned(),
                mark,
                reason,
            },
            read: Read::Change {
                uid: Some(uid.to_owned()),
                removes: false,
            },
        }))
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
            && self.scope == Scope::Bound
            && self.query.trim().is_empty()
            && matches!(self.status, None | Some(MemoryStatus::Proposed))
            && self.author.as_deref().is_none_or(|shown| shown == author)
    }

    /// Creates a memory of the human's, accepted at once. Refused for a text that is empty or too
    /// long, a scope that is not a place to write to, or a source that names nothing or a range
    /// that runs backwards.
    pub fn plan_new(new: &NewMemory) -> Result<Request, &'static str> {
        let text = checked_text(&new.text)?;
        let context = new.context.clone();
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
                basis: new.basis,
                member: None,
            },
            read: Read::Change {
                uid: None,
                removes: false,
            },
        })
    }

    /// Moves memory `uid` into context `context` (D193 (8), D222 (1)). Refused unless `uid` is loaded
    /// and movable, and `context` is one of the tree. The engine refuses a duplicate of the text in
    /// that context; its reason is the write's failure, shown as it comes.
    pub fn plan_move(&self, uid: &str, context: i64) -> Result<Request, &'static str> {
        let memory = self.find(uid).ok_or(NOT_SHOWN)?;
        check_move(Placement::of(memory))?;
        self.known_context(context)?;
        Ok(place(
            CommandKind::MoveMemoryToContext {
                uid: uid.to_owned(),
                context,
            },
            uid,
        ))
    }

    /// Attaches the accepted context memory `uid` to context `context`, any context of the tree.
    /// A proposed memory is refused, as the engine refuses it.
    pub fn plan_attach(&self, uid: &str, context: i64) -> Result<Request, &'static str> {
        let memory = self.find(uid).ok_or(NOT_SHOWN)?;
        check_attach(Placement::of(memory))?;
        self.known_context(context)?;
        Ok(place(
            CommandKind::AttachMemory {
                uid: uid.to_owned(),
                context,
            },
            uid,
        ))
    }

    /// Detaches the context memory `uid` from `context`, one of the contexts the memory is attached
    /// to (`detach_choices`). The engine refuses the last context of a memory, and that reason is
    /// shown as it comes.
    pub fn plan_detach(&self, uid: &str, context: i64) -> Result<Request, &'static str> {
        let memory = self.find(uid).ok_or(NOT_SHOWN)?;
        check_detach(Placement::of(memory))?;
        if !memory
            .contexts
            .iter()
            .any(|attached| attached.id == context)
        {
            return Err(NOT_ATTACHED_TO_IT);
        }
        Ok(place(
            CommandKind::DetachMemory {
                uid: uid.to_owned(),
                context,
            },
            uid,
        ))
    }

    /// The visible context with `id`, or the reason it cannot be a target.
    fn known_context(&self, id: i64) -> Result<&Context, &'static str> {
        self.contexts
            .iter()
            .find(|context| context.id == id)
            .ok_or(NOT_KNOWN)
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

/// Where a memory sits, for the moves between the repository and its contexts (D193 (8)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The memory is a context's, not the repository's own.
    pub in_context: bool,
    pub status: MemoryStatus,
    /// The memory has a supersedes link in either direction.
    pub linked: bool,
    /// How many visible contexts the memory is attached to (D193 (1)).
    pub contexts: usize,
}

impl Placement {
    /// The placement of a memory as the engine listed or read it.
    #[must_use]
    pub fn of(memory: &Memory) -> Self {
        Self {
            in_context: memory.scope != REPOSITORY,
            status: memory.status,
            linked: !memory.supersedes.is_empty() || !memory.superseded_by.is_empty(),
            contexts: memory.contexts.len(),
        }
    }
}

/// Whether a memory can move to another context: the reasons are the engine's. A memory of the
/// repository or of a context moves (D222 (1)).
pub fn check_move(place: Placement) -> Result<(), &'static str> {
    if !matches!(
        place.status,
        MemoryStatus::Proposed | MemoryStatus::Accepted
    ) {
        Err(NOT_MOVABLE)
    } else if place.linked {
        Err(LINKED)
    } else {
        Ok(())
    }
}

/// Whether a memory can be attached to another context: only an accepted context memory.
pub fn check_attach(place: Placement) -> Result<(), &'static str> {
    if !place.in_context {
        Err(MOVE_FIRST)
    } else if place.status != MemoryStatus::Accepted {
        Err(NOT_ACCEPTED_TO_ATTACH)
    } else {
        Ok(())
    }
}

/// Whether a memory can be detached from a context: only a memory attached to at least one visible
/// context has one to leave.
pub fn check_detach(place: Placement) -> Result<(), &'static str> {
    if place.contexts > 0 {
        Ok(())
    } else {
        Err(NO_CONTEXT_OF_ITS_OWN)
    }
}

/// Whether a memory can take a mark: only an accepted one (D202 (1)). The engine refuses the rest.
pub fn check_mark(place: Placement) -> Result<(), &'static str> {
    if place.status == MemoryStatus::Accepted {
        Ok(())
    } else {
        Err(NOT_ACCEPTED_TO_MARK)
    }
}

/// The reason a mark carries: trimmed, none when blank, one line and at most 200 characters.
pub fn checked_reason(text: &str) -> Result<Option<String>, &'static str> {
    let reason = text.trim();
    if reason.is_empty() {
        Ok(None)
    } else if reason.lines().count() > 1 {
        Err(REASON_LINE)
    } else if reason.chars().count() > REASON_CHARS {
        Err(REASON_TOO_LONG)
    } else {
        Ok(Some(reason.to_owned()))
    }
}

/// The person's latest mark of a memory, if she has marked it (D202 (2)).
#[must_use]
pub fn own_mark(marks: &[MemoryMarkSummary]) -> Option<MemoryMark> {
    marks
        .iter()
        .find(|mark| mark.actor == HUMAN_ACTOR)
        .map(|mark| mark.mark)
}

/// Whether a mark's actor is an agent (`agent:<client>`).
#[must_use]
pub fn is_agent_actor(actor: &str) -> bool {
    actor.starts_with(AGENT_ORIGIN)
}

/// The name of a mark's actor in the marks: "You", or "Agent <client>" for an agent.
#[must_use]
pub fn actor_label(actor: &str) -> String {
    if actor == HUMAN_ACTOR {
        "You".to_owned()
    } else if let Some(client) = actor.strip_prefix(AGENT_ORIGIN) {
        format!("Agent {client}")
    } else {
        actor.to_owned()
    }
}

/// The word of a mark on its line: "Useful" or "Wrong".
#[must_use]
pub fn mark_label(mark: MemoryMark) -> &'static str {
    match mark {
        MemoryMark::Useful => "Useful",
        MemoryMark::Wrong => "Wrong",
    }
}

/// The contexts the detach picker lists for a memory, one row each, by path (D193 (1)). Empty when
/// the memory is in no visible context, which `check_detach` refuses.
#[must_use]
pub fn detach_choices(memory: &Memory) -> Vec<MemoryContext> {
    let mut choices = memory.contexts.clone();
    choices.sort_by(|a, b| a.path.cmp(&b.path).then(a.id.cmp(&b.id)));
    choices
}

/// How a detach starts from the memory shown: nothing when it is offered no detach, the one-step
/// confirmation when it is in exactly one visible context, else the picker of its contexts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetachStart {
    Nothing,
    Confirm(MemoryContext),
    Pick(Vec<MemoryContext>),
}

/// The detach a memory starts with (D193 (1)).
#[must_use]
pub fn detach_start(memory: &Memory) -> DetachStart {
    if check_detach(Placement::of(memory)).is_err() {
        return DetachStart::Nothing;
    }
    let choices = detach_choices(memory);
    match choices.as_slice() {
        [only] => DetachStart::Confirm(only.clone()),
        _ => DetachStart::Pick(choices),
    }
}

/// A move, attach or detach write of `uid`: its answer is the memory as it now is.
fn place(kind: CommandKind, uid: &str) -> Request {
    Request {
        kind,
        read: Read::Change {
            uid: Some(uid.to_owned()),
            removes: false,
        },
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

/// The label of a memory's basis: how it is known (D201 (1)).
#[must_use]
pub fn basis_label(basis: MemoryBasis) -> &'static str {
    match basis {
        MemoryBasis::Observed => "Observed",
        MemoryBasis::Tested => "Tested",
        MemoryBasis::Unconfirmed => "Unconfirmed",
    }
}

/// The size line of the tab (D201 (5)): "12 accepted, 3.1k characters".
#[must_use]
pub fn size_line(accepted: u32, characters: u64) -> String {
    format!("{accepted} accepted, {}", characters_text(characters))
}

/// The characters of the memories' text, as the size line says them: a count below a thousand, then
/// thousands with one decimal, cut rather than rounded ("3.1k characters").
#[must_use]
pub fn characters_text(characters: u64) -> String {
    match characters {
        1 => "1 character".to_owned(),
        0..=999 => format!("{characters} characters"),
        _ => format!(
            "{}.{}k characters",
            characters / 1_000,
            characters / 100 % 10
        ),
    }
}

/// The line about a memory's retrieval in its detail (D201 (4)): "Never retrieved", or how often it
/// was and when last, such as "Retrieved 3 times, last 2 h ago". Only the app shows it.
#[must_use]
pub fn retrieval_line(count: u32, last_at_ms: Option<i64>, now_secs: i64) -> String {
    let when = last_at_ms.map(|at| format!(", last {}", crate::threads::relative(now_secs, at)));
    match (count, when) {
        (0, _) => "Never retrieved".to_owned(),
        (1, when) => format!("Retrieved 1 time{}", when.unwrap_or_default()),
        (n, when) => format!("Retrieved {n} times{}", when.unwrap_or_default()),
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

/// The chips of a memory: one per context it belongs to, each with its full path (D223 (2)). A memory
/// with no context in the list (a repository's own, not yet moved) shows its scope as before.
#[must_use]
pub fn scope_chips(memory: &Memory) -> Vec<String> {
    if memory.contexts.is_empty() {
        vec![scope_chip(&memory.scope)]
    } else {
        memory
            .contexts
            .iter()
            .map(|context| context.path.clone())
            .collect()
    }
}

/// The chip of a memory's scope when it names no context: `Repository` for this repository, else the
/// context path it names.
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
            contexts: Vec::new(),
            member: None,
            basis: MemoryBasis::Observed,
            retrieval_count: 0,
            last_retrieved_at: None,
            marks: Vec::new(),
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
            accepted_total: 0,
            accepted_characters: 0,
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
        assert_eq!(tab.set_scope(Scope::Bound), None);
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
        tab.set_scope(Scope::Bound);
        assert!(matches!(
            kind_of(&tab.plan_list()),
            CommandKind::ListMemories { scope: Some(s), .. } if s == "bound"
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
            memory("p1", MemoryStatus::Proposed, "agent:x", "one"),
            Vec::new(),
        ));
        assert!(tab.detail().is_none());
        assert!(tab.read(
            OperationId(6),
            memory("a", MemoryStatus::Accepted, "human", "first"),
            vec![mark_summary("human", MemoryMark::Useful, 1_000, None)],
        ));
        assert_eq!(tab.mark_history().map(<[_]>::len), Some(1));
        assert_eq!(tab.detail().map(|m| m.uid.as_str()), Some("a"));
        assert_eq!(
            tab.selected_memory().map(|m| m.text.as_str()),
            Some("first")
        );
    }

    fn mark_summary(
        actor: &str,
        mark: MemoryMark,
        at: i64,
        reason: Option<&str>,
    ) -> MemoryMarkSummary {
        MemoryMarkSummary {
            actor: actor.to_owned(),
            mark,
            at,
            reason: reason.map(str::to_owned),
        }
    }

    #[test]
    fn the_mark_history_is_kept_for_the_selected_memory_only() {
        let mut tab = loaded();
        let request = tab.select("a");
        tab.sent(&request.read, OperationId(5));
        assert!(tab.read(
            OperationId(5),
            memory("a", MemoryStatus::Accepted, "human", "first"),
            vec![mark_summary("human", MemoryMark::Wrong, 9, None)],
        ));
        assert_eq!(tab.mark_history().map(<[_]>::len), Some(1));
        let other = tab.select("b");
        tab.sent(&other.read, OperationId(6));
        assert_eq!(tab.mark_history(), None);
        tab.deselect();
        assert_eq!(tab.mark_history(), None);
    }

    #[test]
    fn a_mark_is_planned_only_for_a_loaded_accepted_memory() {
        let tab = loaded();
        let request = tab
            .plan_mark("a", MemoryMark::Wrong, "  it misled me  ")
            .expect("an accepted memory takes a mark")
            .expect("a mark the person does not have yet");
        assert!(matches!(
            kind_of(&request),
            CommandKind::MarkMemory { uid, mark: MemoryMark::Wrong, reason: Some(reason) }
                if uid == "a" && reason == "it misled me"
        ));
        assert_eq!(
            request.read,
            Read::Change {
                uid: Some("a".to_owned()),
                removes: false
            }
        );
        let blank = tab
            .plan_mark("a", MemoryMark::Useful, "   ")
            .expect("a blank reason is none")
            .expect("a request");
        assert!(matches!(
            kind_of(&blank),
            CommandKind::MarkMemory { reason: None, .. }
        ));
        assert_eq!(
            tab.plan_mark("p1", MemoryMark::Useful, "").err(),
            Some("Only an accepted memory can be marked.")
        );
        assert_eq!(
            tab.plan_mark("nope", MemoryMark::Useful, "").err(),
            Some(NOT_SHOWN)
        );
    }

    #[test]
    fn a_mark_the_person_already_has_sends_nothing() {
        let mut tab = MemoriesPage::default();
        let read = tab.plan_list();
        tab.sent(&read.read, OperationId(1));
        let mut marked = memory("a", MemoryStatus::Accepted, "human", "first");
        marked.marks = vec![
            mark_summary("agent:claude-code", MemoryMark::Wrong, 1, None),
            mark_summary("human", MemoryMark::Useful, 2, None),
        ];
        tab.listed(OperationId(1), page(vec![marked], None));
        let same = tab
            .plan_mark("a", MemoryMark::Useful, "")
            .expect("not refused");
        assert!(same.is_none(), "the person's own latest mark sends nothing");
        let other = tab
            .plan_mark("a", MemoryMark::Wrong, "")
            .expect("not refused");
        assert!(other.is_some());
    }

    #[test]
    fn a_reason_is_one_line_of_at_most_200_characters_or_none() {
        assert_eq!(checked_reason(" \t "), Ok(None));
        assert_eq!(checked_reason(" why "), Ok(Some("why".to_owned())));
        assert_eq!(checked_reason("one\ntwo"), Err(REASON_LINE));
        assert_eq!(
            checked_reason(&"x".repeat(REASON_CHARS)).map(|r| r.is_some()),
            Ok(true)
        );
        assert_eq!(
            checked_reason(&"x".repeat(REASON_CHARS + 1)),
            Err(REASON_TOO_LONG)
        );
        let tab = loaded();
        assert_eq!(
            tab.plan_mark("a", MemoryMark::Wrong, "one\ntwo").err(),
            Some(REASON_LINE)
        );
    }

    #[test]
    fn the_person_is_named_and_an_agent_is_named_by_its_client() {
        assert_eq!(actor_label("human"), "You");
        assert_eq!(actor_label("agent:claude-code"), "Agent claude-code");
        assert_eq!(mark_label(MemoryMark::Useful), "Useful");
        assert_eq!(mark_label(MemoryMark::Wrong), "Wrong");
        let marks = [
            mark_summary("human", MemoryMark::Useful, 1, None),
            mark_summary("agent:claude-code", MemoryMark::Wrong, 1, None),
        ];
        assert_eq!(own_mark(&marks), Some(MemoryMark::Useful));
        assert_eq!(own_mark(&marks[1..]), None);
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

    fn new_memory(text: &str, context: Option<&str>) -> NewMemory {
        NewMemory {
            kind: MemoryKind::Decision,
            context: context.map(str::to_owned),
            text: text.to_owned(),
            sources: Vec::new(),
            basis: MemoryBasis::Observed,
        }
    }

    #[test]
    fn a_new_memory_sends_the_basis_the_form_chose() {
        for basis in [
            MemoryBasis::Observed,
            MemoryBasis::Tested,
            MemoryBasis::Unconfirmed,
        ] {
            let request = MemoriesPage::plan_new(&NewMemory {
                basis,
                ..new_memory("A belief.", None)
            })
            .expect("a memory with text and a scope is planned");
            assert!(matches!(
                request.kind,
                CommandKind::ProposeMemory { basis: sent, .. } if sent == basis
            ));
        }
    }

    #[test]
    fn a_basis_has_a_plain_label() {
        assert_eq!(basis_label(MemoryBasis::Observed), "Observed");
        assert_eq!(basis_label(MemoryBasis::Tested), "Tested");
        assert_eq!(basis_label(MemoryBasis::Unconfirmed), "Unconfirmed");
    }

    #[test]
    fn the_size_line_says_how_many_are_accepted_and_how_much_text_they_hold() {
        assert_eq!(size_line(12, 3_100), "12 accepted, 3.1k characters");
        assert_eq!(size_line(0, 0), "0 accepted, 0 characters");
        assert_eq!(size_line(1, 1), "1 accepted, 1 character");
        assert_eq!(characters_text(999), "999 characters");
        assert_eq!(characters_text(1_000), "1.0k characters");
        assert_eq!(
            characters_text(12_345),
            "12.3k characters",
            "cut, not rounded"
        );
    }

    #[test]
    fn the_size_line_follows_the_page_and_is_absent_before_a_list() {
        let mut tab = MemoriesPage::default();
        assert_eq!(tab.size_line(), None);
        let read = tab.plan_list();
        tab.sent(&read.read, OperationId(1));
        tab.listed(
            OperationId(1),
            MemoryPage {
                accepted_total: 2,
                accepted_characters: 1_500,
                ..page(Vec::new(), None)
            },
        );
        assert_eq!(
            tab.size_line().as_deref(),
            Some("2 accepted, 1.5k characters")
        );
    }

    #[test]
    fn the_retrieval_line_says_never_or_how_often_and_when_last() {
        let now = 10_000;
        assert_eq!(retrieval_line(0, None, now), "Never retrieved");
        assert_eq!(
            retrieval_line(1, Some(now * 1_000), now),
            "Retrieved 1 time, last just now"
        );
        assert_eq!(
            retrieval_line(3, Some((now - 7_200) * 1_000), now),
            "Retrieved 3 times, last 2 h ago"
        );
        assert_eq!(retrieval_line(2, None, now), "Retrieved 2 times");
    }

    #[test]
    fn a_new_memory_needs_text_of_at_most_a_thousand_characters() {
        assert_eq!(
            (MemoriesPage::plan_new(&new_memory("   ", None))).err(),
            Some(EMPTY_TEXT)
        );
        let long = "x".repeat(TEXT_CHARS + 1);
        assert_eq!(
            (MemoriesPage::plan_new(&new_memory(&long, None))).err(),
            Some(TOO_LONG)
        );
        let exact = "é".repeat(TEXT_CHARS);
        let request = MemoriesPage::plan_new(&new_memory(&exact, None))
            .expect("exactly 1,000 characters is allowed");
        assert!(matches!(
            kind_of(&request),
            CommandKind::ProposeMemory { text, context: None, .. } if text.chars().count() == TEXT_CHARS
        ));
    }

    #[test]
    fn a_new_memory_goes_to_the_context_it_names_or_to_the_engines_choice() {
        let request =
            MemoriesPage::plan_new(&new_memory("a rule", Some("Platform"))).expect("context");
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
        let unnamed = MemoriesPage::plan_new(&new_memory("a rule", None)).expect("no context");
        assert!(matches!(
            kind_of(&unnamed),
            CommandKind::ProposeMemory { context: None, .. }
        ));
    }

    #[test]
    fn a_new_memory_checks_its_sources() {
        let mut memory = new_memory("a fact", None);
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
    fn a_memory_shows_one_chip_per_context_with_its_full_path() {
        let mut stored = memory("m1", MemoryStatus::Accepted, "human", "A rule.");
        stored.scope = "Platform / Billing".to_owned();
        stored.contexts = vec![
            MemoryContext {
                id: 2,
                path: "Platform / Billing".to_owned(),
            },
            MemoryContext {
                id: 9,
                path: "Accounting".to_owned(),
            },
        ];
        assert_eq!(scope_chips(&stored), ["Platform / Billing", "Accounting"]);
        let mut unmoved = memory("m2", MemoryStatus::Accepted, "human", "A rule.");
        unmoved.scope = "repository".to_owned();
        assert_eq!(scope_chips(&unmoved), ["Repository"]);
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

    /// A memory of a context, as the engine lists it: its scope names the context.
    fn in_context(uid: &str, status: MemoryStatus, scope: &str) -> Memory {
        Memory {
            scope: scope.to_owned(),
            ..memory(uid, status, "human", "a context memory")
        }
    }

    fn context_at(id: i64, path: &str) -> Context {
        Context {
            id,
            uid: format!("uid-{id}"),
            name: path.rsplit(" / ").next().unwrap_or(path).to_owned(),
            parent: None,
            path: path.to_owned(),
            repos: Vec::new(),
            accepted_memories: 0,
            sub_contexts: 0,
            members: Vec::new(),
            bound: false,
            inherited_memories: 0,
        }
    }

    /// A page of the repository's memories and context memories, with the contexts it sees.
    fn with_contexts(memories: Vec<Memory>, contexts: Vec<Context>) -> MemoriesPage {
        let mut tab = MemoriesPage::default();
        let read = tab.plan_list();
        tab.sent(&read.read, OperationId(1));
        tab.listed(OperationId(1), page(memories, None));
        tab.sent(&Read::Contexts, OperationId(2));
        assert!(tab.contexts_listed(OperationId(2), contexts));
        tab
    }

    #[test]
    fn a_repository_memory_moves_into_a_context_unless_it_is_done_or_linked() {
        let accepted = Placement::of(&memory("a", MemoryStatus::Accepted, "human", "x"));
        let proposed = Placement::of(&memory("p", MemoryStatus::Proposed, "agent:x", "x"));
        assert_eq!(check_move(accepted), Ok(()));
        assert_eq!(check_move(proposed), Ok(()));
        let rejected = Placement::of(&memory("r", MemoryStatus::Rejected, "human", "x"));
        assert_eq!(check_move(rejected), Err(NOT_MOVABLE));
        let mut linked = memory("l", MemoryStatus::Accepted, "human", "x");
        linked.superseded_by = vec!["n".to_owned()];
        assert_eq!(check_move(Placement::of(&linked)), Err(LINKED));
        let inside = Placement::of(&in_context("c", MemoryStatus::Accepted, "Platform"));
        assert_eq!(check_move(inside), Ok(()), "a context memory moves too");
    }

    #[test]
    fn attach_is_offered_only_for_an_accepted_context_memory_as_the_engine_allows() {
        let accepted = Placement::of(&in_context("c", MemoryStatus::Accepted, "Platform"));
        assert_eq!(check_attach(accepted), Ok(()));
        let proposed = Placement::of(&in_context("p", MemoryStatus::Proposed, "Platform"));
        assert_eq!(check_attach(proposed), Err(NOT_ACCEPTED_TO_ATTACH));
        let own = Placement::of(&memory("a", MemoryStatus::Accepted, "human", "x"));
        assert_eq!(check_attach(own), Err(MOVE_FIRST));
    }

    /// A context a memory is attached to, as the engine lists it.
    fn attached(id: i64, path: &str) -> MemoryContext {
        MemoryContext {
            id,
            path: path.to_owned(),
        }
    }

    /// A memory of a context, attached to every context of `contexts`.
    fn attached_in(uid: &str, status: MemoryStatus, contexts: &[(i64, &str)]) -> Memory {
        let mut memory = in_context(uid, status, contexts[0].1);
        memory.contexts = contexts
            .iter()
            .map(|&(id, path)| attached(id, path))
            .collect();
        memory
    }

    #[test]
    fn detach_needs_a_memory_attached_to_a_visible_context_of_any_status() {
        let proposed = Placement::of(&attached_in(
            "p",
            MemoryStatus::Proposed,
            &[(7, "Platform")],
        ));
        assert_eq!(check_detach(proposed), Ok(()));
        let own = Placement::of(&memory("a", MemoryStatus::Accepted, "human", "x"));
        assert_eq!(check_detach(own), Err(NO_CONTEXT_OF_ITS_OWN));
        let unattached = Placement::of(&in_context("u", MemoryStatus::Accepted, "Platform"));
        assert_eq!(check_detach(unattached), Err(NO_CONTEXT_OF_ITS_OWN));
    }

    #[test]
    fn the_picker_lists_every_attached_context_once_by_path() {
        let memory = attached_in(
            "c",
            MemoryStatus::Accepted,
            &[(9, "Platform / Billing"), (7, "Partners"), (4, "Platform")],
        );
        assert_eq!(
            detach_choices(&memory),
            vec![
                attached(7, "Partners"),
                attached(4, "Platform"),
                attached(9, "Platform / Billing"),
            ]
        );
    }

    #[test]
    fn a_detach_is_confirmed_for_one_context_and_picked_for_several_or_refused() {
        let one = attached_in("o", MemoryStatus::Accepted, &[(7, "Platform")]);
        assert_eq!(
            detach_start(&one),
            DetachStart::Confirm(attached(7, "Platform"))
        );
        let two = attached_in(
            "t",
            MemoryStatus::Accepted,
            &[(9, "Platform / Billing"), (7, "Platform")],
        );
        assert_eq!(
            detach_start(&two),
            DetachStart::Pick(vec![
                attached(7, "Platform"),
                attached(9, "Platform / Billing")
            ])
        );
        let own = memory("a", MemoryStatus::Accepted, "human", "x");
        assert_eq!(detach_start(&own), DetachStart::Nothing);
    }

    #[test]
    fn a_move_plans_the_engine_command_for_a_context_of_the_tree_only() {
        let tab = with_contexts(
            vec![memory("a", MemoryStatus::Accepted, "human", "first")],
            vec![context_at(7, "Platform")],
        );
        let request = tab.plan_move("a", 7).expect("planned");
        assert_eq!(
            kind_of(&request),
            &CommandKind::MoveMemoryToContext {
                uid: "a".to_owned(),
                context: 7,
            }
        );
        assert_eq!(
            request.read,
            Read::Change {
                uid: Some("a".to_owned()),
                removes: false,
            }
        );
        assert_eq!(tab.plan_move("a", 8).err(), Some(NOT_KNOWN));
        assert_eq!(tab.plan_move("missing", 7).err(), Some(NOT_SHOWN));
    }

    #[test]
    fn an_attach_is_refused_for_a_proposal_and_planned_for_an_accepted_context_memory() {
        let tab = with_contexts(
            vec![
                in_context("c", MemoryStatus::Accepted, "Platform"),
                in_context("p", MemoryStatus::Proposed, "Platform"),
            ],
            vec![context_at(7, "Platform"), context_at(8, "Partners")],
        );
        assert_eq!(
            kind_of(&tab.plan_attach("c", 8).expect("planned")),
            &CommandKind::AttachMemory {
                uid: "c".to_owned(),
                context: 8,
            }
        );
        assert_eq!(tab.plan_attach("p", 8).err(), Some(NOT_ACCEPTED_TO_ATTACH));
    }

    #[test]
    fn a_detach_names_an_attached_context_and_is_refused_for_a_repository_memory() {
        let tab = with_contexts(
            vec![
                attached_in("c", MemoryStatus::Accepted, &[(9, "Platform / Billing")]),
                memory("a", MemoryStatus::Accepted, "human", "own"),
            ],
            vec![
                context_at(7, "Platform"),
                context_at(9, "Platform / Billing"),
            ],
        );
        assert_eq!(
            kind_of(&tab.plan_detach("c", 9).expect("planned")),
            &CommandKind::DetachMemory {
                uid: "c".to_owned(),
                context: 9,
            }
        );
        assert_eq!(tab.plan_detach("a", 9).err(), Some(NO_CONTEXT_OF_ITS_OWN));
        assert_eq!(tab.plan_detach("c", 7).err(), Some(NOT_ATTACHED_TO_IT));
        let two = with_contexts(
            vec![attached_in(
                "t",
                MemoryStatus::Accepted,
                &[(7, "Platform"), (9, "Platform / Billing")],
            )],
            vec![
                context_at(7, "Platform"),
                context_at(9, "Platform / Billing"),
            ],
        );
        assert_eq!(
            kind_of(&two.plan_detach("t", 7).expect("planned")),
            &CommandKind::DetachMemory {
                uid: "t".to_owned(),
                context: 7,
            }
        );
    }

    #[test]
    fn a_refused_move_onto_a_duplicate_shows_the_engine_reason_as_it_comes() {
        let mut tab = with_contexts(
            vec![memory("a", MemoryStatus::Accepted, "human", "first")],
            vec![context_at(7, "Platform")],
        );
        let request = tab.plan_move("a", 7).expect("planned");
        tab.sent(&request.read, OperationId(30));
        let reason = "memory m9 in this context already says this, so nothing was moved";
        tab.ended(
            OperationId(30),
            OperationState::Failed,
            Some(reason.to_owned()),
        );
        assert_eq!(tab.failure(), Some(reason));
    }
}
