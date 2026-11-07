//! The memories of the contexts a repository sees, beside its own (design/threads.md, "Contexts";
//! D191, D193). `threads.db` holds the repository's memories and `contexts.db` the memories of the
//! contexts; this module joins them: which contexts the repository sees (`Reach`), the merged and
//! paged list (`list_command`), the scope a memory is labelled with, the files a memory names in
//! other repositories, and the commands that act on a context memory or move one into a context.
//! A read never creates `contexts.db`, and a failure reading it never fails a read: the repository's
//! memories answer, and the answer says why (D193 (2), (4), (6)).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gm_schema::threads::{Memory, MemoryFilter, MemorySource, MemorySourceKind, MemoryStatus};
use gm_schema::{Actor, Event, OperationId};

use crate::GitPort;
use crate::contexts::store::ContextStore;
use crate::contexts::visibility::{self, ContextNode, Scope};
use crate::contexts_session::{RepoState, repo_identity};
use crate::threads::memories::{self, Found, Lookup, MemoryAction, NewMemory};
use crate::threads::store::{StoreError, ThreadStore};
use crate::threads_session::{
    Context, Proposal, named, now_ms, origin_of, redacted, stored_source,
};

/// What a memory command answers when no memory of that uid is visible.
pub(crate) const MEMORY_MISSING: &str = "Memory not found.";
/// The scope label of the repository's own memories (D193 (5)).
const REPOSITORY: &str = "repository";
const NOT_VISIBLE: &str = "The context is not one this repository belongs to.";
const NOT_ATTACHED: &str = "The memory is not attached to that context.";
const ALREADY_IN_CONTEXT: &str = "The memory is already in a context.";
const MOVE_FIRST: &str = "The memory is in this repository only: move it to a context first.";
const NO_CONTEXT_OF_ITS_OWN: &str =
    "The memory is in this repository only, so it has no context to detach from.";
const NOT_ACCEPTED: &str = "only an accepted memory can be attached to another context";
const NOT_MOVABLE: &str = "only a proposed or accepted memory can move to a context";
const LINKED: &str = "a memory with supersedes links cannot move to a context";

/// What the open clone's identity says about its contexts (D193 (4)): its id when its contexts are
/// used, the contexts it sees, the note of a clash, and every context's node.
struct Joined {
    repo_id: Option<String>,
    visible: Vec<(i64, u32)>,
    note: Option<String>,
    nodes: Vec<ContextNode>,
}

/// The contexts a repository sees, as a memory command reads them (D191 (3), D193 (4), (5)).
pub(crate) struct Reach {
    /// The repository's id, when its contexts are used: `None` for a clone with no id in a
    /// context, or a clash.
    repo_id: Option<String>,
    /// The visible contexts with their distance, nearest first (`visibility::visible_contexts`).
    visible: Vec<(i64, u32)>,
    /// Every context, for names and paths.
    nodes: Vec<ContextNode>,
    /// `contexts.db`, when it exists and opened.
    store: Option<ContextStore>,
    /// False when `contexts.db` exists but could not be opened or read.
    read: bool,
    /// Why the contexts are not used, when they are not (D193 (4)).
    note: Option<String>,
}

impl Reach {
    /// What the repository at `root` sees in `db`. A missing file gives no contexts; a file that
    /// cannot be read gives none and says so in `note`.
    pub(crate) fn load(git: &dyn GitPort, root: &Path, db: Option<&Path>) -> Self {
        let Some(path) = db else {
            return Self::none();
        };
        let mut store = match ContextStore::open_existing(path) {
            Ok(Some(store)) => store,
            Ok(None) => return Self::none(),
            Err(error) => return Self::unreadable(&error.to_string()),
        };
        match Self::join(git, root, &mut store) {
            Ok(joined) => Self {
                repo_id: joined.repo_id,
                visible: joined.visible,
                nodes: joined.nodes,
                store: Some(store),
                read: true,
                note: joined.note,
            },
            Err(error) => Self::unreadable(&error),
        }
    }

    fn none() -> Self {
        Self {
            repo_id: None,
            visible: Vec::new(),
            nodes: Vec::new(),
            store: None,
            read: true,
            note: None,
        }
    }

    fn unreadable(error: &str) -> Self {
        Self {
            read: false,
            note: Some(format!(
                "contexts.db could not be read ({error}), so the answer holds only this repository's memories"
            )),
            ..Self::none()
        }
    }

    /// What the clone at `root` sees, from its identity in `store` (D193 (4)).
    fn join(git: &dyn GitPort, root: &Path, store: &mut ContextStore) -> Result<Joined, String> {
        let common = git.common_dir(root).map_err(|e| e.0)?;
        let identity = repo_identity(store, &common)?;
        let nodes = store.nodes().map_err(|e| e.to_string())?;
        let (repo_id, visible, note) = match identity {
            RepoState::None => (None, Vec::new(), None),
            RepoState::Ok(id) => {
                let member_of = store.repo_contexts(&id).map_err(|e| e.to_string())?;
                let visible = visibility::visible_contexts(&nodes, &member_of);
                (Some(id), visible, None)
            }
            RepoState::Clash { other_path, .. } => (
                None,
                Vec::new(),
                Some(format!(
                    "this clone shares its repository id with the clone at {other_path}, so its contexts are not used here"
                )),
            ),
        };
        Ok(Joined {
            repo_id,
            visible,
            note,
            nodes,
        })
    }

    /// The contexts file, when it opened.
    pub(crate) fn store(&self) -> Option<&ContextStore> {
        self.store.as_ref()
    }

    fn store_mut(&mut self) -> Option<&mut ContextStore> {
        self.store.as_mut()
    }

    /// The repository's id, when its contexts are used.
    pub(crate) fn repo_id(&self) -> Option<&str> {
        self.repo_id.as_deref()
    }

    /// The last known folder of repository `repo_id`, when `contexts.db` has one.
    pub(crate) fn path_of(&self, repo_id: &str) -> Option<String> {
        self.store.as_ref()?.repo_path(repo_id).ok().flatten()
    }

    fn sees_context(&self, context: i64) -> bool {
        self.visible.iter().any(|&(id, _)| id == context)
    }

    /// Whether the repository sees one of `contexts`.
    fn sees_any(&self, contexts: &[i64]) -> bool {
        contexts.iter().any(|&context| self.sees_context(context))
    }

    /// The distance of the nearest visible context among `contexts`; `u32::MAX` when none is.
    fn nearest(&self, contexts: &[i64]) -> u32 {
        self.visible
            .iter()
            .filter(|(id, _)| contexts.contains(id))
            .map(|&(_, distance)| distance)
            .min()
            .unwrap_or(u32::MAX)
    }

    /// The scope label of a context memory attached to `contexts`: the path of its nearest visible
    /// context (D193 (5)). Empty when none is visible, which a memory the engine found never is.
    fn label_of(&self, contexts: &[i64]) -> String {
        self.visible
            .iter()
            .find(|(id, _)| contexts.contains(id))
            .map_or_else(String::new, |&(id, _)| {
                visibility::context_path(&self.nodes, id)
            })
    }

    /// The scope a request names, `None` meaning every scope (D193 (5)).
    fn scope(&self, text: Option<&str>) -> Result<Scope, String> {
        match text {
            None => Ok(Scope::All),
            Some(text) => visibility::resolve_scope(&self.nodes, &self.visible, text),
        }
    }

    /// The id a memory proposed at a context carries on its sources: the repository's own (D193
    /// (7)). Refused with the reason when the repository has none, since it belongs to no context.
    fn writer_id(&self) -> Result<String, String> {
        self.repo_id.clone().ok_or_else(|| {
            self.note.clone().unwrap_or_else(|| {
                "this repository belongs to no context, so no memory can be proposed at one"
                    .to_owned()
            })
        })
    }

    /// The context a proposal names, by path or unique name, among the visible ones (D193 (7)).
    fn context_named(&self, name: &str) -> Result<i64, String> {
        if !self.read {
            return Err(self.note.clone().unwrap_or_default());
        }
        match self.scope(Some(name)) {
            Ok(Scope::Context(id)) => Ok(id),
            Ok(Scope::All | Scope::Repository) => Err(format!(
                "\"{name}\" is not a context: leave the context out to propose for this repository."
            )),
            Err(message) if message.starts_with("No context") => Err(format!(
                "\"{name}\" is not a context this repository belongs to."
            )),
            Err(message) => Err(message),
        }
    }

    /// The contexts a request's scope covers among the visible ones (`All`), or the named one.
    fn scope_ids(&self, scope: Scope) -> Vec<i64> {
        match scope {
            Scope::Context(id) => vec![id],
            Scope::All | Scope::Repository => self.visible.iter().map(|&(id, _)| id).collect(),
        }
    }
}

/// What the memory commands read and write: the repository's store and the contexts it sees.
pub(crate) struct Stores<'a> {
    pub(crate) threads: &'a mut ThreadStore,
    pub(crate) reach: Reach,
}

impl<'a> Stores<'a> {
    /// The stores of one memory command: the repository's and the contexts it sees now.
    pub(crate) fn new(threads: &'a mut ThreadStore, ctx: &Context<'_>) -> Self {
        Self {
            threads,
            reach: Reach::load(ctx.git, ctx.root, ctx.contexts_db),
        }
    }
}

/// Where a memory the repository can act on is stored (D193 (1), (7)).
pub(crate) enum Home {
    /// The repository's own store, with the row id.
    Repository(i64),
    /// `contexts.db`, attached to a context this repository sees.
    Context,
}

/// Finds memory `uid` in the repository's store, or else among the context memories a visible
/// context holds. Any other memory is not found (D193 (7)).
pub(crate) fn home_of(stores: &Stores<'_>, uid: &str) -> Result<Home, String> {
    match stores.threads.memory_id_of(uid) {
        Ok(id) => return Ok(Home::Repository(id)),
        Err(StoreError::NotFound) => {}
        Err(error) => return Err(error.to_string()),
    }
    let contexts = stores
        .reach
        .store()
        .ok_or_else(|| MEMORY_MISSING.to_owned())?;
    match contexts.memory_contexts(uid) {
        Ok(attached) if stores.reach.sees_any(&attached) => Ok(Home::Context),
        Ok(_) | Err(StoreError::NotFound) => Err(MEMORY_MISSING.to_owned()),
        Err(error) => Err(error.to_string()),
    }
}

/// Whether `uid` is a memory of `contexts.db`, visible or not: the scope a supersedes link must
/// share (D193 (1)).
pub(crate) fn is_in_contexts(stores: &Stores<'_>, uid: &str) -> bool {
    stores
        .reach
        .store()
        .is_some_and(|contexts| contexts.memory_contexts(uid).is_ok())
}

/// The refusal of a supersedes link across stores (D193 (1)).
pub(crate) fn other_scope(uid: &str) -> String {
    format!("memory {uid} is in another scope: a memory supersedes only memories of its own store")
}

/// The scope label and the files changed of a memory, set once it is answered (D193 (5), (9)).
/// `attached` holds the contexts a context memory is in and is empty for the repository's own.
fn finish(ctx: &Context<'_>, reach: &Reach, memory: &mut Memory, attached: &[i64]) {
    memory.scope = if attached.is_empty() {
        REPOSITORY.to_owned()
    } else {
        reach.label_of(attached)
    };
    fill_changes(ctx, reach, memory);
}

/// Derives `files_changed_since` of `memory` from `HEAD` now (D192 (6), D193 (9)). Never stored.
pub(crate) fn fill_changes(ctx: &Context<'_>, reach: &Reach, memory: &mut Memory) {
    let current = |source: &MemorySource| -> Result<Option<String>, Lookup> {
        let root = source_root(ctx, reach, source.repo_id.as_deref())?;
        blob_under(ctx, &root, &source.reference).map_err(|_| Lookup::Failed)
    };
    memory.files_changed_since = memories::files_changed_since(memory, &current);
}

/// The folder a file source of repository `repo_id` is read from (D193 (9)): this repository's for
/// its own sources, the recorded folder of another, which must still exist. Otherwise the source is
/// unavailable, and nothing is guessed.
fn source_root(ctx: &Context<'_>, reach: &Reach, repo_id: Option<&str>) -> Result<PathBuf, Lookup> {
    match repo_id {
        None => Ok(ctx.root.to_path_buf()),
        Some(id) if reach.repo_id() == Some(id) => Ok(ctx.root.to_path_buf()),
        Some(id) => reach
            .path_of(id)
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .ok_or(Lookup::Unavailable),
    }
}

/// The blob of `path` at `HEAD` of the repository at `root`, with `root` a work tree or its Git
/// directory. A failure of Git is an error; a missing file is `Ok(None)`.
fn blob_under(ctx: &Context<'_>, root: &Path, path: &str) -> Result<Option<String>, String> {
    ctx.git.blob_at_commit(root, "HEAD", path).map_err(|e| e.0)
}

/// The answer of a memory write: the memory as it now is, with its scope and changes.
fn changed(
    id: OperationId,
    ctx: &Context<'_>,
    reach: &Reach,
    mut memory: Memory,
    created: bool,
    attached: &[i64],
) -> Vec<Event> {
    finish(ctx, reach, &mut memory, attached);
    vec![Event::MemoryChanged {
        id,
        memory: Some(memory),
        created,
    }]
}

/// The memory a write answers: a repository memory (`repository`) or a memory of a context.
pub(crate) fn repository_changed(
    id: OperationId,
    ctx: &Context<'_>,
    reach: &Reach,
    memory: Memory,
    created: bool,
) -> Vec<Event> {
    changed(id, ctx, reach, memory, created, &[])
}

/// What `ListMemories` asks for, as the engine reads it.
pub(crate) struct Ask<'a> {
    pub(crate) query: &'a str,
    pub(crate) filter: &'a MemoryFilter,
    pub(crate) scope: Option<&'a str>,
    pub(crate) limit: u32,
    pub(crate) cursor: Option<&'a str>,
}

/// The answer of `ListMemories`.
pub(crate) struct Listed {
    pub(crate) memories: Vec<Memory>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) contexts_read: bool,
    pub(crate) note: Option<String>,
}

/// `ListMemories` (D193 (5), (6)): the repository's memories and those of the contexts it sees,
/// each once, merged and paged. A scope naming a context that cannot be resolved while `contexts.db`
/// is unreadable answers nothing and says why.
pub(crate) fn list_command(
    stores: &Stores<'_>,
    ctx: &Context<'_>,
    ask: &Ask<'_>,
) -> Result<Listed, String> {
    let reach = &stores.reach;
    let (contexts_read, note) = (reach.read, reach.note.clone());
    let scope = match reach.scope(ask.scope) {
        Ok(scope) => scope,
        Err(_) if !reach.read => {
            return Ok(Listed {
                memories: Vec::new(),
                next_cursor: None,
                contexts_read,
                note,
            });
        }
        Err(message) => return Err(message),
    };
    let (mut memories, next_cursor) = list(stores, ask, scope)?;
    for memory in &mut memories {
        fill_changes(ctx, reach, memory);
    }
    Ok(Listed {
        memories,
        next_cursor,
        contexts_read,
        note,
    })
}

/// One memory a list finds, before its page is read.
struct Entry {
    hit: Found,
    /// Whether it is in `contexts.db` (else in the repository's store).
    in_contexts: bool,
    /// The contexts it is attached to (empty for the repository's own).
    attached: Vec<i64>,
    /// The distance of its nearest visible context, 0 for the repository (D193 (6)).
    distance: u32,
}

/// Orders the entries as a list shows them (D193 (6)): by `bm25` ascending with a query, otherwise
/// newest first; then the nearer scope; then the uid.
fn order(entries: &mut [Entry], with_query: bool) {
    entries.sort_by(|a, b| {
        let rank = if with_query {
            a.hit.score.total_cmp(&b.hit.score)
        } else {
            b.hit.created_at.cmp(&a.hit.created_at)
        };
        rank.then(a.distance.cmp(&b.distance))
            .then_with(|| a.hit.uid.cmp(&b.hit.uid))
    });
}

/// The scope a cursor names: the resolved scope, so `Billing` and `Platform / Billing` agree.
fn scope_key(scope: Scope) -> String {
    match scope {
        Scope::All => "all".to_owned(),
        Scope::Repository => REPOSITORY.to_owned(),
        Scope::Context(id) => format!("context:{id}"),
    }
}

/// The merged list of the scopes `scope` names, one page of it (D193 (6)).
fn list(
    stores: &Stores<'_>,
    ask: &Ask<'_>,
    scope: Scope,
) -> Result<(Vec<Memory>, Option<String>), String> {
    let digest = memories::list_digest(ask.query, ask.filter, &scope_key(scope));
    let offset = match ask.cursor {
        Some(cursor) => {
            let offset = memories::decode_offset(cursor, &digest).map_err(|e| e.to_string())?;
            usize::try_from(offset).unwrap_or(usize::MAX)
        }
        None => 0,
    };
    let mut entries = Vec::new();
    if matches!(scope, Scope::All | Scope::Repository) {
        let found = stores
            .threads
            .find_memories(ask.query, ask.filter)
            .map_err(|e| e.to_string())?;
        entries.extend(found.into_iter().map(|hit| Entry {
            hit,
            in_contexts: false,
            attached: Vec::new(),
            distance: 0,
        }));
    }
    if let Some(contexts) = stores.reach.store().filter(|_| scope != Scope::Repository) {
        let ids = stores.reach.scope_ids(scope);
        let found = contexts
            .find_memories_in(&ids, ask.query, ask.filter)
            .map_err(|e| e.to_string())?;
        for hit in found {
            let attached = contexts
                .memory_contexts(&hit.uid)
                .map_err(|e| e.to_string())?;
            let distance = stores.reach.nearest(&attached);
            entries.push(Entry {
                hit,
                in_contexts: true,
                attached,
                distance,
            });
        }
    }
    order(&mut entries, !ask.query.trim().is_empty());
    // A memory held by both stores (a move interrupted after its insert) is listed once, nearest first.
    let mut seen = HashSet::new();
    entries.retain(|entry| seen.insert(entry.hit.uid.clone()));
    let total = entries.len();
    let start = offset.min(total);
    let end = start
        .saturating_add(usize::try_from(ask.limit.max(1)).unwrap_or(usize::MAX))
        .min(total);
    let next = (end < total).then(|| format!("{digest}.{end}"));
    let mut memories = Vec::with_capacity(end - start);
    for entry in &entries[start..end] {
        let mut memory = if entry.in_contexts {
            let contexts = stores
                .reach
                .store()
                .ok_or_else(|| MEMORY_MISSING.to_owned())?;
            contexts.read_memory_by_uid(&entry.hit.uid)
        } else {
            stores.threads.read_memory(entry.hit.id)
        }
        .map_err(|e| e.to_string())?;
        memory.scope = if entry.in_contexts {
            stores.reach.label_of(&entry.attached)
        } else {
            REPOSITORY.to_owned()
        };
        memories.push(memory);
    }
    Ok((memories, next))
}

/// A memory of `contexts.db` by uid, with the contexts it is attached to.
fn context_memory(reach: &Reach, uid: &str) -> Result<(Memory, Vec<i64>), String> {
    let contexts = reach.store().ok_or_else(|| MEMORY_MISSING.to_owned())?;
    let memory = contexts
        .read_memory_by_uid(uid)
        .map_err(|e| named(e, MEMORY_MISSING))?;
    let attached = contexts
        .memory_contexts(uid)
        .map_err(|e| named(e, MEMORY_MISSING))?;
    Ok((memory, attached))
}

/// `ReadMemory` for a memory the repository sees, of its own store or of a context.
pub(crate) fn read(stores: &Stores<'_>, ctx: &Context<'_>, uid: &str) -> Result<Memory, String> {
    let (mut memory, attached) = match home_of(stores, uid)? {
        Home::Repository(id) => (
            stores
                .threads
                .read_memory(id)
                .map_err(|e| named(e, MEMORY_MISSING))?,
            Vec::new(),
        ),
        Home::Context => context_memory(&stores.reach, uid)?,
    };
    finish(ctx, &stores.reach, &mut memory, &attached);
    Ok(memory)
}

/// `ProposeMemory` with a context (D193 (7)): the memory is stored once, in a context the repository
/// sees, and every source names this repository. An agent's is `proposed`, a human's `accepted`.
pub(crate) fn propose_in_context(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    actor: &Actor,
    name: &str,
    proposal: &Proposal<'_>,
) -> Result<Vec<Event>, String> {
    let is_human = matches!(actor, Actor::Human);
    memories::authorize(
        is_human,
        &MemoryAction::Propose {
            sources: proposal.sources.len(),
        },
    )?;
    let repo_id = stores.reach.writer_id()?;
    let context = stores.reach.context_named(name)?;
    let text = redacted(ctx, proposal.text)?;
    let mut sources = proposal
        .sources
        .iter()
        .map(|source| stored_source(ctx, source, is_human))
        .collect::<Result<Vec<_>, _>>()?;
    for source in &mut sources {
        source.repo_id = Some(repo_id.clone());
    }
    let status = if is_human {
        MemoryStatus::Accepted
    } else {
        MemoryStatus::Proposed
    };
    let origin = origin_of(actor);
    let contexts = stores
        .reach
        .store_mut()
        .ok_or_else(|| NOT_VISIBLE.to_owned())?;
    let (memory, created) = contexts
        .propose_context_memory(
            &NewMemory {
                kind: proposal.kind,
                text: &text,
                status,
                origin: &origin,
                sources: &sources,
                at: now_ms(),
            },
            &[context],
            None,
        )
        .map_err(|e| e.to_string())?;
    let attached = contexts
        .memory_contexts(&memory.uid)
        .map_err(|e| e.to_string())?;
    Ok(changed(id, ctx, &stores.reach, memory, created, &attached))
}

/// `AcceptMemory` of a context memory (D192 (3), (4), D193 (1), (7)). The blob of each file source
/// is recorded when the memory first becomes accepted, read from its own repository, or from another
/// one whose folder is known and still exists; a source with no readable folder records nothing.
pub(crate) fn accept_in_context(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    is_human: bool,
    uid: &str,
    text: Option<&str>,
    supersedes: &[String],
) -> Result<Vec<Event>, String> {
    memories::authorize(is_human, &MemoryAction::Accept)?;
    for other in supersedes {
        match home_of(stores, other) {
            Ok(Home::Context) => {}
            Ok(Home::Repository(_)) => return Err(other_scope(other)),
            Err(_) => return Err(format!("memory {other} does not exist")),
        }
    }
    let (memory, _) = context_memory(&stores.reach, uid)?;
    let text = text.map(|text| redacted(ctx, text)).transpose()?;
    let blobs = if memory.status == MemoryStatus::Accepted {
        Vec::new()
    } else {
        file_blobs(ctx, &stores.reach, &memory)
    };
    let contexts = stores
        .reach
        .store_mut()
        .ok_or_else(|| MEMORY_MISSING.to_owned())?;
    let accepted = contexts
        .accept_memory(uid, text.as_deref(), supersedes, &blobs, now_ms())
        .map_err(|e| named(e, MEMORY_MISSING))?;
    let attached = contexts
        .memory_contexts(uid)
        .map_err(|e| named(e, MEMORY_MISSING))?;
    Ok(changed(id, ctx, &stores.reach, accepted, false, &attached))
}

/// The blob at `HEAD` of each file source whose folder is known (D193 (9)), as the acceptance
/// records it.
fn file_blobs(ctx: &Context<'_>, reach: &Reach, memory: &Memory) -> Vec<(String, Option<String>)> {
    memory
        .sources
        .iter()
        .filter(|source| source.kind == MemorySourceKind::File)
        .filter_map(|source| {
            let root = source_root(ctx, reach, source.repo_id.as_deref()).ok()?;
            let blob = blob_under(ctx, &root, &source.reference).ok().flatten();
            Some((source.reference.clone(), blob))
        })
        .collect()
}

/// `RejectMemory` of a context memory.
pub(crate) fn reject_in_context(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    is_human: bool,
    uid: &str,
) -> Result<Vec<Event>, String> {
    memories::authorize(is_human, &MemoryAction::Reject)?;
    let contexts = stores
        .reach
        .store_mut()
        .ok_or_else(|| MEMORY_MISSING.to_owned())?;
    let memory = contexts
        .reject_memory(uid, now_ms())
        .map_err(|e| named(e, MEMORY_MISSING))?;
    let attached = contexts
        .memory_contexts(uid)
        .map_err(|e| named(e, MEMORY_MISSING))?;
    Ok(changed(id, ctx, &stores.reach, memory, false, &attached))
}

/// `DeleteMemory` of a context memory: it leaves every context.
pub(crate) fn delete_in_context(
    id: OperationId,
    stores: &mut Stores<'_>,
    is_human: bool,
    uid: &str,
) -> Result<Vec<Event>, String> {
    memories::authorize(is_human, &MemoryAction::Delete)?;
    let contexts = stores
        .reach
        .store_mut()
        .ok_or_else(|| MEMORY_MISSING.to_owned())?;
    contexts
        .delete_memory(uid)
        .map_err(|e| named(e, MEMORY_MISSING))?;
    Ok(vec![Event::MemoryChanged {
        id,
        memory: None,
        created: false,
    }])
}

/// `MoveMemoryToContext` (D193 (8)): repository memory `uid` is stored in `context` under the same
/// uid, with its history and its sources, and then deleted here. A retry after a crash between the
/// two writes finds the copy and only deletes. A different memory of the same text in the context
/// refuses the move, and nothing is deleted.
pub(crate) fn move_to_context(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    uid: &str,
    context: i64,
) -> Result<Vec<Event>, String> {
    let Ok(row) = stores.threads.memory_id_of(uid) else {
        return Err(match home_of(stores, uid) {
            Ok(_) => ALREADY_IN_CONTEXT.to_owned(),
            Err(message) => message,
        });
    };
    let mut memory = stores
        .threads
        .read_memory(row)
        .map_err(|e| named(e, MEMORY_MISSING))?;
    if !matches!(
        memory.status,
        MemoryStatus::Proposed | MemoryStatus::Accepted
    ) {
        return Err(NOT_MOVABLE.to_owned());
    }
    if !memory.supersedes.is_empty() || !memory.superseded_by.is_empty() {
        return Err(LINKED.to_owned());
    }
    if !stores.reach.sees_context(context) {
        return Err(NOT_VISIBLE.to_owned());
    }
    let repo_id = stores.reach.writer_id()?;
    for source in &mut memory.sources {
        source.repo_id = Some(repo_id.clone());
    }
    let contexts = stores
        .reach
        .store_mut()
        .ok_or_else(|| NOT_VISIBLE.to_owned())?;
    let (moved, created) = contexts
        .move_memory_in(&memory, &[context])
        .map_err(|e| e.to_string())?;
    if moved.uid != uid {
        return Err(format!(
            "memory {} in this context already says this, so nothing was moved",
            moved.uid
        ));
    }
    if !created {
        contexts
            .attach_memory(uid, context)
            .map_err(|e| e.to_string())?;
    }
    let attached = contexts.memory_contexts(uid).map_err(|e| e.to_string())?;
    stores
        .threads
        .delete_memory(row)
        .map_err(|e| named(e, MEMORY_MISSING))?;
    Ok(changed(id, ctx, &stores.reach, moved, false, &attached))
}

/// `AttachMemory` (D193 (8)): accepted context memory `uid` also joins `context`.
pub(crate) fn attach(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    uid: &str,
    context: i64,
) -> Result<Vec<Event>, String> {
    if let Home::Repository(_) = home_of(stores, uid)? {
        return Err(MOVE_FIRST.to_owned());
    }
    let (memory, _) = context_memory(&stores.reach, uid)?;
    if memory.status != MemoryStatus::Accepted {
        return Err(NOT_ACCEPTED.to_owned());
    }
    if !stores.reach.sees_context(context) {
        return Err(NOT_VISIBLE.to_owned());
    }
    let contexts = stores
        .reach
        .store_mut()
        .ok_or_else(|| MEMORY_MISSING.to_owned())?;
    contexts
        .attach_memory(uid, context)
        .map_err(|e| named(e, NOT_VISIBLE))?;
    let (memory, attached) = context_memory(&stores.reach, uid)?;
    Ok(changed(id, ctx, &stores.reach, memory, false, &attached))
}

/// `DetachMemory` (D193 (8)): context memory `uid` leaves `context`. The last context is refused by
/// the store.
pub(crate) fn detach(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    uid: &str,
    context: i64,
) -> Result<Vec<Event>, String> {
    if let Home::Repository(_) = home_of(stores, uid)? {
        return Err(NO_CONTEXT_OF_ITS_OWN.to_owned());
    }
    if !stores.reach.sees_context(context) {
        return Err(NOT_VISIBLE.to_owned());
    }
    let contexts = stores
        .reach
        .store_mut()
        .ok_or_else(|| MEMORY_MISSING.to_owned())?;
    contexts
        .detach_memory(uid, context)
        .map_err(|e| named(e, NOT_ATTACHED))?;
    let (memory, attached) = context_memory(&stores.reach, uid)?;
    Ok(changed(id, ctx, &stores.reach, memory, false, &attached))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(uid: &str, in_contexts: bool, distance: u32, score: f64, created_at: i64) -> Entry {
        Entry {
            hit: Found {
                id: 0,
                uid: uid.to_owned(),
                created_at,
                score,
            },
            in_contexts,
            attached: Vec::new(),
            distance,
        }
    }

    fn uids(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.hit.uid.as_str()).collect()
    }

    #[test]
    fn with_a_query_the_rank_comes_first_then_the_nearer_scope_then_the_uid() {
        let mut entries = vec![
            entry("b", true, 1, -2.0, 1),
            entry("a", false, 0, -1.0, 1),
            entry("c", true, 2, -2.0, 1),
            entry("d", false, 0, -2.0, 1),
        ];
        order(&mut entries, true);
        assert_eq!(uids(&entries), vec!["d", "b", "c", "a"]);
    }

    #[test]
    fn without_a_query_the_newest_comes_first_then_the_nearer_scope_then_the_uid() {
        let mut entries = vec![
            entry("x", true, 1, 0.0, 5),
            entry("y", false, 0, 0.0, 5),
            entry("a", true, 1, 0.0, 5),
            entry("z", false, 0, 0.0, 9),
        ];
        order(&mut entries, false);
        assert_eq!(uids(&entries), vec!["z", "y", "a", "x"]);
    }

    #[test]
    fn a_scope_key_names_the_resolved_scope() {
        assert_eq!(scope_key(Scope::All), "all");
        assert_eq!(scope_key(Scope::Repository), "repository");
        assert_eq!(scope_key(Scope::Context(4)), "context:4");
    }
}
