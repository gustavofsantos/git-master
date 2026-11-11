//! The memories of the contexts a repository reads (design/threads.md, "Contexts"; D191, D193,
//! D222). Every memory lives in `contexts.db`; `threads.db` keeps the threads and, until a command
//! moves them, the memories written before (D222 (6)). This module joins them: the contexts a
//! repository or a workspace is bound to and any context it names (`Reach`), the merged and paged
//! list (`list_command`), the scope a memory is labelled with, the files a memory names in other
//! repositories, and the commands that act on a context memory or move one into a context.
//! A read never creates `contexts.db`, and a failure reading it never fails a read: the repository's
//! memories answer, and the answer says why (D193 (2), (4), (6)).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gm_schema::threads::{
    Memory, MemoryContext, MemoryFilter, MemoryMark, MemoryMarkSummary, MemorySource,
    MemorySourceKind, MemoryStatus,
};
use gm_schema::{Actor, Event, OperationId};

use crate::GitPort;
use crate::contexts::store::ContextStore;
use crate::contexts::visibility::{self, ContextNode, Scope};
use crate::contexts_session::{self, Sight};
use crate::threads::memories::{self, BlobRead, Found, Lookup, MemoryAction, NewMemory};
use crate::threads::store::{StoreError, ThreadStore};
use crate::threads_session::{
    Context, Proposal, named, now_ms, origin_of, redacted, stored_source,
};
use crate::workspace::Workspaces;

/// What a memory command answers when no memory of that uid is found.
pub(crate) const MEMORY_MISSING: &str = "Memory not found.";
/// The scope label of the repository's own memories (D193 (5)).
const REPOSITORY: &str = "repository";
/// The name of a workspace's own repository among its contexts' members (D222 (5)).
pub(crate) const WORKSPACE: &str = "workspace";
const NOT_VISIBLE: &str = "The context is not one this repository belongs to.";
/// What attaching to a context that does not exist answers (D222 (3)).
const NO_CONTEXT: &str = "Context not found.";
const NOT_ATTACHED: &str = "The memory is not attached to that context.";
const MOVE_FIRST: &str = "The memory is in this repository only: move it to a context first.";
const NO_CONTEXT_OF_ITS_OWN: &str =
    "The memory is in this repository only, so it has no context to detach from.";
const NOT_ACCEPTED: &str = "only an accepted memory can be attached to another context";
const NOT_MOVABLE: &str = "only a proposed or accepted memory can move to a context";
const LINKED: &str = "a memory with supersedes links cannot move to a context";
const NOT_A_WORKSPACE: &str = "this repository is not a workspace";
/// What a proposal without a context answers when no contexts database is configured (D222 (4)).
const NO_CONTEXTS_DB: &str = "no contexts database is configured, so the memory has nowhere to go";

/// A folder of a workspace's member: its name in the workspace and its checkout (D216 (1)).
#[derive(Clone, Debug)]
pub(crate) struct Folder {
    pub(crate) name: String,
    pub(crate) root: PathBuf,
}

/// The members of the workspace at `root`, in folder order, or `None` when `root` is not a workspace
/// (D174). A repository with no `.gitmodules` is not one, so a plain repository's reads run no Git
/// process for this.
pub(crate) fn workspace_folders(
    git: &dyn GitPort,
    global_config: Option<&Path>,
    root: &Path,
) -> Option<Vec<Folder>> {
    if !root.join(".gitmodules").is_file() {
        return None;
    }
    let workspaces = Workspaces::new(git, global_config.map(Path::to_path_buf));
    if !workspaces.is_workspace(root) {
        return None;
    }
    let mut folders: Vec<Folder> = workspaces
        .members(root)
        .ok()?
        .into_iter()
        .map(|member| Folder {
            root: root.join(&member.path),
            name: member.path,
        })
        .collect();
    folders.sort_by(|a, b| a.name.cmp(&b.name));
    Some(folders)
}

/// The threads store of the clone at `root` when it has one: `None` when it has none, an error when it
/// cannot be read. It is opened without creating or migrating it (D222 (6)).
fn open_member(git: &dyn GitPort, root: &Path) -> Result<Option<ThreadStore>, String> {
    if !root.is_dir() {
        return Err("its folder is missing".to_owned());
    }
    let common = git.common_dir(root).map_err(|e| e.0)?;
    ThreadStore::open_member(&common).map_err(|e| e.to_string())
}

/// What the open repository's identity says about its contexts (D193 (4)): its id when its contexts
/// are used, the contexts it sees, the note of a clash, and every context's node.
struct Joined {
    repo_id: Option<String>,
    visible: Vec<(i64, u32)>,
    note: Option<String>,
    nodes: Vec<ContextNode>,
    /// True when this clone's id is held by another clone: it reads no contexts at all (D193 (4)).
    clashed: bool,
    /// The folder of each readable clone of a workspace, by its id (D222 (5)).
    locals: Vec<(String, PathBuf)>,
}

/// The contexts a repository or a workspace reads, as a memory command reads them (D191 (3), D193
/// (4), (5), D222 (3), (5)): the ones it is bound to with their ancestors are its default reach, and
/// any context can be named.
pub(crate) struct Reach {
    /// The repository's id, when its contexts are used: `None` for a clone with no id in a
    /// context, a clash, or a workspace.
    repo_id: Option<String>,
    /// The contexts the repository is bound to and their ancestors, with their distance, nearest
    /// first (`visibility::visible_contexts`). This is the default reach, and the one a proposal and a
    /// move are checked against. A workspace's is the union of its own bindings and its members'.
    visible: Vec<(i64, u32)>,
    /// Every context, for names and paths.
    nodes: Vec<ContextNode>,
    /// `contexts.db`, when it exists and opened.
    store: Option<ContextStore>,
    /// False when `contexts.db` exists but could not be opened or read.
    read: bool,
    /// True on a clash: the clone's contexts are not read (`store` is kept for paths, D193 (4)).
    clashed: bool,
    /// Why the contexts are not used, when they are not (D193 (4)), joined for every member.
    note: Option<String>,
    /// Whether the open root is a workspace (D222 (5)). A workspace reads no memory of its own
    /// threads store, which holds only what the migration has not moved yet.
    workspace: bool,
    /// The member folders of a workspace in folder order; empty for a repository.
    folders: Vec<Folder>,
    /// The folder of each readable clone of a workspace, its own and its members', by repo id: a file
    /// source of one of them is read there, at its `HEAD` (D193 (9), D222 (5)).
    locals: Vec<(String, PathBuf)>,
}

impl Reach {
    /// What the open root sees in `db`. A missing file gives no contexts; a file that cannot be read
    /// gives none and says so in `note`. A workspace (`folders` given) sees what it and its members
    /// are bound to together (D222 (5)).
    pub(crate) fn load(
        git: &dyn GitPort,
        root: &Path,
        db: Option<&Path>,
        folders: Option<&[Folder]>,
    ) -> Self {
        let mut reach = Self::bare(folders);
        let Some(path) = db else {
            return reach;
        };
        let mut store = match ContextStore::open_existing(path) {
            Ok(Some(store)) => store,
            Ok(None) => return reach,
            Err(error) => return reach.unreadable(&error.to_string()),
        };
        let joined = match folders {
            Some(folders) => Self::join_workspace(git, root, folders, &mut store),
            None => Self::join(git, root, &mut store),
        };
        match joined {
            Ok(joined) => {
                reach.repo_id = joined.repo_id;
                reach.visible = joined.visible;
                reach.nodes = joined.nodes;
                reach.note = joined.note;
                reach.clashed = joined.clashed;
                reach.locals = joined.locals;
                reach.store = Some(store);
                reach
            }
            Err(error) => reach.unreadable(&error),
        }
    }

    /// A reach that sees no context, for the open root and its member folders.
    fn bare(folders: Option<&[Folder]>) -> Self {
        Self {
            repo_id: None,
            visible: Vec::new(),
            nodes: Vec::new(),
            store: None,
            read: true,
            clashed: false,
            note: None,
            workspace: folders.is_some(),
            folders: folders.map(<[Folder]>::to_vec).unwrap_or_default(),
            locals: Vec::new(),
        }
    }

    fn unreadable(self, error: &str) -> Self {
        Self {
            read: false,
            note: Some(format!(
                "contexts.db could not be read ({error}), so the answer holds only this repository's memories"
            )),
            ..self
        }
    }

    /// What the clone at `root` sees, from its identity in `store` (D193 (4)).
    fn join(git: &dyn GitPort, root: &Path, store: &mut ContextStore) -> Result<Joined, String> {
        let nodes = store.nodes().map_err(|e| e.to_string())?;
        let (repo_id, visible, note, clashed) = match contexts_session::sight(store, git, root)? {
            Sight::Nothing => (None, Vec::new(), None, false),
            Sight::Contexts { repo_id, member_of } => (
                Some(repo_id),
                visibility::visible_contexts(&nodes, &member_of),
                None,
                false,
            ),
            Sight::Clash { other_path } => (None, Vec::new(), Some(clash_note(&other_path)), true),
        };
        Ok(Joined {
            repo_id,
            visible,
            note,
            nodes,
            clashed,
            locals: Vec::new(),
        })
    }

    /// What a workspace sees (D222 (5)): the contexts bound directly to the workspace repository and
    /// to each member, with their parents. A member with a clash, or whose identity cannot be read,
    /// adds none and a note names it.
    fn join_workspace(
        git: &dyn GitPort,
        root: &Path,
        folders: &[Folder],
        store: &mut ContextStore,
    ) -> Result<Joined, String> {
        let nodes = store.nodes().map_err(|e| e.to_string())?;
        let mut member_of = Vec::new();
        let mut problems = Vec::new();
        let mut workspace_id = None;
        let mut locals = Vec::new();
        let parts = std::iter::once(("workspace".to_owned(), root.to_path_buf())).chain(
            folders
                .iter()
                .map(|folder| (format!("member {}", folder.name), folder.root.clone())),
        );
        for (index, (label, path)) in parts.enumerate() {
            match contexts_session::sight(store, git, &path) {
                Ok(Sight::Contexts {
                    repo_id,
                    member_of: direct,
                }) => {
                    if index == 0 {
                        workspace_id = Some(repo_id.clone());
                    }
                    locals.push((repo_id, path));
                    member_of.extend(direct);
                }
                Ok(Sight::Nothing) => {}
                Ok(Sight::Clash { other_path }) => {
                    problems.push(format!("{label}: {}", clash_note(&other_path)));
                }
                Err(error) => {
                    problems.push(format!("{label}: its contexts could not be read ({error})"));
                }
            }
        }
        Ok(Joined {
            repo_id: workspace_id,
            visible: visibility::visible_contexts(&nodes, &member_of),
            note: join_notes(&problems),
            nodes,
            clashed: false,
            locals,
        })
    }

    /// The contexts file, when it opened and this clone may read its contexts: never on a clash
    /// (D193 (4)).
    pub(crate) fn store(&self) -> Option<&ContextStore> {
        if self.clashed {
            return None;
        }
        self.store.as_ref()
    }

    fn store_mut(&mut self) -> Option<&mut ContextStore> {
        if self.clashed {
            return None;
        }
        self.store.as_mut()
    }

    /// The repository's id, when its contexts are used.
    pub(crate) fn repo_id(&self) -> Option<&str> {
        self.repo_id.as_deref()
    }

    /// The folder of a clone of the workspace whose id is `repo_id`, when it is readable.
    fn local_root(&self, repo_id: &str) -> Option<&Path> {
        self.locals
            .iter()
            .find(|(id, _)| id == repo_id)
            .map(|(_, path)| path.as_path())
    }

    /// The last known folder of repository `repo_id`, when `contexts.db` has one.
    pub(crate) fn path_of(&self, repo_id: &str) -> Option<String> {
        self.store.as_ref()?.repo_path(repo_id).ok().flatten()
    }

    /// Whether the repository is bound to `context` or inherits from it: the one check a proposal and
    /// a move are made against (D222 (4)).
    fn sees_context(&self, context: i64) -> bool {
        self.visible.iter().any(|&(id, _)| id == context)
    }

    /// The contexts a request's scope covers, with their distance, nearest first (D222 (2), (6)): the
    /// default reach for `All`, and for `Context` the named context, its ancestors and every context
    /// under it (`visibility::scope_contexts`).
    fn reach_of(&self, scope: Scope) -> Vec<(i64, u32)> {
        match scope {
            Scope::All => self.visible.clone(),
            Scope::Context(id) => visibility::scope_contexts(&self.nodes, id),
        }
    }

    /// The scope label of a context memory attached to `contexts`: the path of its nearest context in
    /// `near`, the scope's reach (D193 (5), D222 (6)). A memory none of whose contexts is in `near`
    /// takes the path of its first attached context by path.
    fn label_of(&self, near: &[(i64, u32)], contexts: &[i64]) -> String {
        match near.iter().find(|(id, _)| contexts.contains(id)) {
            Some(&(id, _)) => visibility::context_path(&self.nodes, id),
            None => self
                .attached_contexts(contexts)
                .first()
                .map_or_else(String::new, |context| context.path.clone()),
        }
    }

    /// Every context among `attached`, by path: the contexts a memory answer carries (D193 (1), D222
    /// (3)). Contexts the repository is not bound to are listed too, so a memory's places are complete.
    fn attached_contexts(&self, attached: &[i64]) -> Vec<MemoryContext> {
        let mut contexts: Vec<MemoryContext> = attached
            .iter()
            .map(|&id| MemoryContext {
                id,
                path: visibility::context_path(&self.nodes, id),
            })
            .collect();
        contexts.sort_by(|a, b| a.path.cmp(&b.path).then(a.id.cmp(&b.id)));
        contexts
    }

    /// Whether the open root is a workspace.
    pub(crate) fn is_workspace(&self) -> bool {
        self.workspace
    }

    /// The scope a request names, `None` meaning every scope (D193 (5)).
    fn scope(&self, text: Option<&str>) -> Result<Scope, String> {
        match text {
            None => Ok(Scope::All),
            Some(text) => self.named(text.trim()),
        }
    }

    /// What a scope name means: a context, by path or unique name, bound or not (D223 (1)). The names
    /// of the retired workspace scopes, `workspace` and `member <folder>`, are refused with a message
    /// that says so, unless a context has that name.
    fn named(&self, text: &str) -> Result<Scope, String> {
        visibility::resolve_scope(&self.nodes, &self.visible, text).map_err(|message| {
            if message.starts_with("No context") && retired_scope(text) {
                format!("scope \"{text}\" no longer exists: name a context, or use bound")
            } else {
                message
            }
        })
    }

    /// The member of a workspace that `name` names, a folder of it.
    fn member_folder(&self, name: &str) -> Result<&Folder, String> {
        self.folders
            .iter()
            .find(|folder| folder.name == name)
            .ok_or_else(|| {
                format!(
                    "there is no member {name} in this workspace; members are: {}",
                    self.folders
                        .iter()
                        .map(|folder| folder.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }

    /// The contexts a request's scope covers: the default reach for `All`, the named context with its
    /// ancestors and its descendants for `Context` (D222 (2), (6)).
    fn scope_ids(&self, scope: Scope) -> Vec<i64> {
        self.reach_of(scope).into_iter().map(|(id, _)| id).collect()
    }

    /// The context a person's proposal names, by path or unique name: any context of the tree, bound or
    /// not (D223 (6)). A name resolves first among the bound ones, as a read does.
    fn context_named_by_person(&self, name: &str) -> Result<i64, String> {
        if !self.read {
            return Err(self.note.clone().unwrap_or_default());
        }
        match self.scope(Some(name)) {
            Ok(Scope::Context(id)) => Ok(id),
            Ok(Scope::All) => Err(format!(
                "\"{name}\" is not a context: leave the context out to propose for this repository."
            )),
            Err(message) => Err(message),
        }
    }

    /// The context a proposal names, by path or unique name. It must be one the repository is bound to
    /// or inherits from: an agent proposes only into those (D191 (4)(d), D222 (3)).
    fn context_named(&self, name: &str) -> Result<i64, String> {
        if !self.read {
            return Err(self.note.clone().unwrap_or_default());
        }
        let not_reached =
            || format!("\"{name}\" is not a context this repository is bound to or inherits from.");
        match self.scope(Some(name)) {
            Ok(Scope::Context(id)) if self.sees_context(id) => Ok(id),
            Ok(Scope::Context(_)) => Err(not_reached()),
            Ok(Scope::All) => Err(format!(
                "\"{name}\" is not a context: leave the context out to propose for this repository."
            )),
            Err(message) if message.starts_with("No context") => Err(not_reached()),
            Err(message) => Err(message),
        }
    }
}

/// The id a memory written from the clone at `ctx.root` carries on its sources (D193 (7)), recorded
/// with that clone so its file sources resolve even when it is bound to no context (D222 (5)). A
/// repository with no contexts file answers with the reach's note, as a read does.
fn writer_of(reach: &mut Reach, ctx: &Context<'_>) -> Result<String, String> {
    let note = reach.note.clone();
    let store = reach
        .store_mut()
        .ok_or_else(|| note.unwrap_or_else(|| NOT_VISIBLE.to_owned()))?;
    contexts_session::writer(store, ctx.git, ctx.root)
}

/// Whether `text` is the name of a scope that no longer exists: a workspace's own scope or one of
/// its members (D222 (5)).
fn retired_scope(text: &str) -> bool {
    text == WORKSPACE || text.starts_with("member ")
}

/// The note of a clone whose id is held by another clone (D193 (4)).
pub(crate) fn clash_note(other_path: &str) -> String {
    format!(
        "this clone shares its repository id with the clone at {other_path}, so its contexts are not used here"
    )
}

/// The distance of the nearest context among `contexts` in `near`; `u32::MAX` when none is.
fn nearest(near: &[(i64, u32)], contexts: &[i64]) -> u32 {
    near.iter()
        .filter(|(id, _)| contexts.contains(id))
        .map(|&(_, distance)| distance)
        .min()
        .unwrap_or(u32::MAX)
}

/// The notes of one answer, joined; `None` when there are none.
fn join_notes(notes: &[String]) -> Option<String> {
    (!notes.is_empty()).then(|| notes.join("; "))
}

/// The note of a member whose store could not answer (D216 (3)).
fn member_failed(folder: &str, reason: &str) -> String {
    format!("member {folder} could not be read ({reason})")
}

/// The contexts the root of a request sees, read from `ctx`: a workspace's with its members' (D222 (5)).
fn reach_for(ctx: &Context<'_>) -> Reach {
    let folders = workspace_folders(ctx.git, ctx.global_config, ctx.root);
    Reach::load(ctx.git, ctx.root, ctx.contexts_db, folders.as_deref())
}

/// What the memory commands read and write: the repository's threads store, which holds only what
/// a plain repository has not moved yet (D222 (6)), and the contexts it sees.
pub(crate) struct Stores<'a> {
    pub(crate) threads: &'a mut ThreadStore,
    pub(crate) reach: Reach,
}

impl<'a> Stores<'a> {
    /// The stores of one memory command: the repository's threads store and the contexts it sees now.
    pub(crate) fn new(threads: &'a mut ThreadStore, ctx: &Context<'_>) -> Self {
        Self {
            threads,
            reach: reach_for(ctx),
        }
    }

    /// Reads the contexts again, after a command bound the root to a context (D222 (4), (5)).
    pub(crate) fn reload(&mut self, ctx: &Context<'_>) {
        self.reach = reach_for(ctx);
    }
}

/// Where a memory the repository can act on is stored (D193 (1), (7), D222 (3)).
pub(crate) enum Home {
    /// The repository's threads store, for a plain repository whose memories have not moved yet.
    Repository(i64),
    /// `contexts.db`, attached to one or more contexts (D222 (3)).
    Context,
}

/// Finds memory `uid` in the repository's threads store (a plain repository only: a workspace reads
/// none), then in `contexts.db`, whether or not the repository is bound to one of its contexts
/// (D222 (3)). Any other memory is not found (D193 (7)).
pub(crate) fn home_of(stores: &Stores<'_>, uid: &str) -> Result<Home, String> {
    if !stores.reach.is_workspace() {
        match stores.threads.memory_id_of(uid) {
            Ok(id) => return Ok(Home::Repository(id)),
            Err(StoreError::NotFound) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let contexts = stores
        .reach
        .store()
        .ok_or_else(|| MEMORY_MISSING.to_owned())?;
    match contexts.memory_contexts(uid) {
        Ok(attached) if !attached.is_empty() => Ok(Home::Context),
        Ok(_) | Err(StoreError::NotFound) => Err(MEMORY_MISSING.to_owned()),
        Err(error) => Err(error.to_string()),
    }
}

/// Whether `uid` is a memory of `contexts.db`: the scope a supersedes link must
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

/// The scope label and the files changed of a memory of the open root's own store or of a context,
/// set once it is answered (D193 (5), (9)). `attached` holds the contexts a context memory is in and
/// is empty for the own store's. `near` is the reach of the request's scope, which the label follows
/// (D222 (6)): the default reach for a write or a read by uid.
fn finish(
    ctx: &Context<'_>,
    reach: &Reach,
    memory: &mut Memory,
    attached: &[i64],
    near: &[(i64, u32)],
) {
    memory.scope = if attached.is_empty() {
        REPOSITORY.to_owned()
    } else {
        reach.label_of(near, attached)
    };
    memory.contexts = reach.attached_contexts(attached);
    fill_changes(ctx, reach, ctx.root, memory);
}

/// Derives `files_changed_since` of `memory` from `HEAD` now (D192 (6), D193 (9)). Never stored.
/// `home` is the folder of the store the memory is in, which a source naming no repository is read in.
pub(crate) fn fill_changes(ctx: &Context<'_>, reach: &Reach, home: &Path, memory: &mut Memory) {
    let current = |source: &MemorySource| -> Result<Option<String>, Lookup> {
        let root = source_root(ctx, reach, home, source.repo_id.as_deref())?;
        blob_under(ctx, &root, &source.reference).map_err(|_| Lookup::Failed)
    };
    memory.files_changed_since = memories::files_changed_since(memory, &current);
}

/// The folder a file source of repository `repo_id` is read from (D193 (9)): `home` for a source that
/// names no repository, this repository's for its own id, the recorded folder of another, which must
/// still exist. Otherwise the source is unavailable, and nothing is guessed.
fn source_root(
    ctx: &Context<'_>,
    reach: &Reach,
    home: &Path,
    repo_id: Option<&str>,
) -> Result<PathBuf, Lookup> {
    match repo_id {
        None => Ok(home.to_path_buf()),
        Some(id) if reach.repo_id() == Some(id) => Ok(ctx.root.to_path_buf()),
        Some(id) => match reach.local_root(id) {
            Some(path) => Ok(path.to_path_buf()),
            None => reach
                .path_of(id)
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
                .ok_or(Lookup::Unavailable),
        },
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
    finish(ctx, reach, &mut memory, attached, &reach.visible);
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
    pub(crate) proposed_total: u32,
    /// The accepted memories in the scope, and the characters of their text (D201 (5)).
    pub(crate) accepted_total: u32,
    pub(crate) accepted_characters: u64,
}

/// `ListMemories` (D193 (5), (6), D222 (3)): the memories of the scope the request names, each once,
/// merged and paged. A scope naming a context that cannot be resolved while `contexts.db` is unreadable
/// answers nothing and says why. The notes of the contexts and of the threads store say what did not
/// answer; a read never fails for them.
pub(crate) fn list_command(
    stores: &Stores<'_>,
    ctx: &Context<'_>,
    ask: &Ask<'_>,
) -> Result<Listed, String> {
    let reach = &stores.reach;
    let contexts_read = reach.read;
    let notes: Vec<String> = reach.note.iter().cloned().collect();
    let scope = match reach.scope(ask.scope) {
        Ok(scope) => scope,
        Err(_) if !reach.read => {
            return Ok(Listed {
                memories: Vec::new(),
                next_cursor: None,
                contexts_read,
                note: join_notes(&notes),
                proposed_total: count_proposed_visible(stores)?,
                accepted_total: 0,
                accepted_characters: 0,
            });
        }
        Err(message) => return Err(message),
    };
    let (memories, next_cursor) = list(stores, ctx, ask, scope)?;
    let proposed_total = count_proposed_visible(stores)?;
    let (accepted_total, accepted_characters) = accepted_size(stores, scope)?;
    Ok(Listed {
        memories,
        next_cursor,
        contexts_read,
        note: join_notes(&notes),
        proposed_total,
        accepted_total,
        accepted_characters,
    })
}

/// Whether a scope covers the repository's own threads store: a plain repository's, until its memories
/// have moved (D222 (6)). A workspace reads none of it.
fn includes_own(reach: &Reach, scope: Scope) -> bool {
    !reach.is_workspace() && matches!(scope, Scope::All)
}

/// Whether a scope covers contexts: the default reach, or one named context with its ancestors and
/// descendants.
fn reads_contexts(scope: Scope) -> bool {
    matches!(scope, Scope::All | Scope::Context(_))
}

/// The accepted memories of the scope `scope` covers, and the characters of their text together,
/// each memory once (D201 (5)). The filters, the query and the page play no part.
fn accepted_size(stores: &Stores<'_>, scope: Scope) -> Result<(u32, u64), String> {
    let mut rows = Vec::new();
    if includes_own(&stores.reach, scope) {
        rows.extend(stores.threads.accepted_text().map_err(|e| e.to_string())?);
    }
    if reads_contexts(scope)
        && let Some(contexts) = stores.reach.store()
    {
        let ids = stores.reach.scope_ids(scope);
        rows.extend(contexts.accepted_text_in(&ids).map_err(|e| e.to_string())?);
    }
    Ok(memories::accepted_size(rows))
}

/// Counts one retrieval, at `at`, of each memory of `uids` the repository sees (D201 (4)): in the store
/// it lives in, the repository's threads store or `contexts.db`. A uid it does not see is skipped. Every
/// uid is tried even when a write fails; the first failure is returned, for the caller to log, and the
/// read that named the memories has already answered.
pub(crate) fn count_retrievals(
    stores: &mut Stores<'_>,
    uids: &[String],
    at: i64,
) -> Result<(), String> {
    let mut first_error = None;
    for uid in uids {
        let counted = match home_of(stores, uid) {
            Ok(Home::Repository(_)) => stores
                .threads
                .count_retrieval(uid, at)
                .map_err(|e| e.to_string()),
            Ok(Home::Context) => match stores.reach.store() {
                Some(contexts) => contexts.count_retrieval(uid, at).map_err(|e| e.to_string()),
                None => Ok(false),
            },
            Err(error) if error == MEMORY_MISSING => Ok(false),
            Err(error) => Err(error),
        };
        if let Err(error) = counted {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// How many proposed memories the repository sees (D193 (6), D222 (5)): its threads store, for a plain
/// repository, and every visible context, a memory held by both counted once. The filters, the status,
/// the query and the scope of the request play no part.
fn count_proposed_visible(stores: &Stores<'_>) -> Result<u32, String> {
    let repository = if stores.reach.is_workspace() {
        0
    } else {
        stores.threads.count_proposed().map_err(|e| e.to_string())?
    };
    let Some(contexts) = stores.reach.store() else {
        return Ok(repository);
    };
    let ids = stores.reach.scope_ids(Scope::All);
    let in_contexts = contexts
        .count_proposed_in(&ids)
        .map_err(|e| e.to_string())?;
    if repository == 0 || in_contexts == 0 {
        return Ok(repository.saturating_add(in_contexts));
    }
    let uids = contexts.proposed_uids_in(&ids).map_err(|e| e.to_string())?;
    let shared = stores
        .threads
        .count_proposed_among(&uids)
        .map_err(|e| e.to_string())?;
    Ok(repository
        .saturating_add(in_contexts)
        .saturating_sub(shared))
}

/// Where a listed memory is stored.
enum Place {
    /// The repository's threads store, for a plain repository whose memories have not moved yet.
    Own,
    /// `contexts.db`.
    Context,
}

/// One memory a list finds, before its page is read.
struct Entry {
    hit: Found,
    place: Place,
    /// The contexts it is attached to (empty for the threads store).
    attached: Vec<i64>,
    /// Its place among the scopes: the distance of the nearest context it is attached to, and 0 for the
    /// repository's own threads store (D193 (6)).
    near: u32,
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
        rank.then(a.near.cmp(&b.near))
            .then_with(|| a.hit.uid.cmp(&b.hit.uid))
    });
}

/// The scope a cursor names: the resolved scope, so `Billing` and `Platform / Billing` agree.
fn scope_key(scope: Scope) -> String {
    match scope {
        Scope::All => "all".to_owned(),
        Scope::Context(id) => format!("context:{id}"),
    }
}

/// The merged list of the scopes `scope` names, one page of it (D193 (6)). A memory held by two
/// stores (a move interrupted after its insert) is listed once, nearest first.
fn list(
    stores: &Stores<'_>,
    ctx: &Context<'_>,
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
    if includes_own(&stores.reach, scope) {
        let found = stores
            .threads
            .find_memories(ask.query, ask.filter)
            .map_err(|e| e.to_string())?;
        entries.extend(found.into_iter().map(|hit| Entry {
            hit,
            place: Place::Own,
            attached: Vec::new(),
            near: 0,
        }));
    }
    let near = stores.reach.reach_of(scope);
    if reads_contexts(scope)
        && let Some(contexts) = stores.reach.store()
    {
        let ids = stores.reach.scope_ids(scope);
        let found = contexts
            .find_memories_in(&ids, ask.query, ask.filter)
            .map_err(|e| e.to_string())?;
        for hit in found {
            let attached = contexts
                .memory_contexts(&hit.uid)
                .map_err(|e| e.to_string())?;
            let distance = nearest(&near, &attached);
            entries.push(Entry {
                hit,
                place: Place::Context,
                attached,
                near: distance,
            });
        }
    }
    order(&mut entries, !ask.query.trim().is_empty());
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
        memories.push(page_memory(stores, ctx, entry, &near)?);
    }
    Ok((memories, next))
}

/// The memory of one listed entry, labelled and with its changes derived in the store that holds it.
/// `near` is the reach of the request's scope, which its label follows.
fn page_memory(
    stores: &Stores<'_>,
    ctx: &Context<'_>,
    entry: &Entry,
    near: &[(i64, u32)],
) -> Result<Memory, String> {
    match entry.place {
        Place::Own => {
            let mut memory = stores
                .threads
                .read_memory(entry.hit.id)
                .map_err(|e| e.to_string())?;
            finish(ctx, &stores.reach, &mut memory, &[], near);
            Ok(memory)
        }
        Place::Context => {
            let contexts = stores
                .reach
                .store()
                .ok_or_else(|| MEMORY_MISSING.to_owned())?;
            let mut memory = contexts
                .read_memory_by_uid(&entry.hit.uid)
                .map_err(|e| e.to_string())?;
            finish(ctx, &stores.reach, &mut memory, &entry.attached, near);
            Ok(memory)
        }
    }
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

/// `ReadMemory` of any memory the engine finds: of the repository's threads store or of
/// `contexts.db`, bound to a context or not (D193 (7), D216 (2), (3), D222 (3)).
pub(crate) fn read(stores: &Stores<'_>, ctx: &Context<'_>, uid: &str) -> Result<Memory, String> {
    match home_of(stores, uid)? {
        Home::Repository(id) => {
            let mut memory = stores
                .threads
                .read_memory(id)
                .map_err(|e| named(e, MEMORY_MISSING))?;
            finish(ctx, &stores.reach, &mut memory, &[], &stores.reach.visible);
            Ok(memory)
        }
        Home::Context => {
            let (mut memory, attached) = context_memory(&stores.reach, uid)?;
            finish(
                ctx,
                &stores.reach,
                &mut memory,
                &attached,
                &stores.reach.visible,
            );
            Ok(memory)
        }
    }
}

/// `ProposeMemory` with a context (D193 (7)): the memory is stored once, in a context the repository
/// is bound to or inherits from, and every source names this repository. An agent's is `proposed`, a
/// human's `accepted`.
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
    let context = if is_human {
        stores.reach.context_named_by_person(name)?
    } else {
        stores.reach.context_named(name)?
    };
    let repo_id = writer_of(&mut stores.reach, ctx)?;
    store_proposal(id, stores, ctx, actor, &repo_id, context, proposal)
}

/// `ProposeMemory` without a context in a plain repository (D222 (4)): the memory goes to the one
/// context the repository is bound to, or else to its own context, which is created and bound when
/// needed. The contexts are read once more after that, so the reach sees the new binding. Refused
/// when no contexts database is configured, when several contexts are bound and when the id clashes.
pub(crate) fn propose_in_repository(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    actor: &Actor,
    proposal: &Proposal<'_>,
) -> Result<Vec<Event>, String> {
    let is_human = matches!(actor, Actor::Human);
    memories::authorize(
        is_human,
        &MemoryAction::Propose {
            sources: proposal.sources.len(),
        },
    )?;
    let db = ctx.contexts_db.ok_or_else(|| NO_CONTEXTS_DB.to_owned())?;
    let mut contexts = ContextStore::open(db).map_err(|e| e.to_string())?;
    let home = contexts_session::proposal_home(&mut contexts, ctx.git, ctx.root)?;
    drop(contexts);
    stores.reload(ctx);
    store_proposal(
        id,
        stores,
        ctx,
        actor,
        &home.repo_id,
        home.context,
        proposal,
    )
}

/// `ProposeMemory` without a context in a workspace (D222 (5)): the contexts bound to the workspace and
/// to its members decide it (`contexts_session::workspace_home`). The contexts are read once more
/// after a binding is made, as for a plain repository.
pub(crate) fn propose_in_workspace(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    actor: &Actor,
    proposal: &Proposal<'_>,
) -> Result<Vec<Event>, String> {
    let is_human = matches!(actor, Actor::Human);
    memories::authorize(
        is_human,
        &MemoryAction::Propose {
            sources: proposal.sources.len(),
        },
    )?;
    let db = ctx.contexts_db.ok_or_else(|| NO_CONTEXTS_DB.to_owned())?;
    let members = workspace_folders(ctx.git, ctx.global_config, ctx.root).unwrap_or_default();
    let mut contexts = ContextStore::open(db).map_err(|e| e.to_string())?;
    let home = contexts_session::workspace_home(&mut contexts, ctx.git, ctx.root, &members)?;
    drop(contexts);
    stores.reload(ctx);
    store_proposal(
        id,
        stores,
        ctx,
        actor,
        &home.repo_id,
        home.context,
        proposal,
    )
}

/// `ProposeMemory` with `member` (D222 (5)): the memory goes to that member's own context, created
/// and bound when needed. It is checked and written from the member's clone, so its file sources are
/// read at the member's `HEAD` and its text is redacted with the member's configuration.
pub(crate) fn propose_in_member(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    actor: &Actor,
    folder: &str,
    proposal: &Proposal<'_>,
) -> Result<Vec<Event>, String> {
    let is_human = matches!(actor, Actor::Human);
    if !stores.reach.is_workspace() {
        return Err(NOT_A_WORKSPACE.to_owned());
    }
    memories::authorize(
        is_human,
        &MemoryAction::Propose {
            sources: proposal.sources.len(),
        },
    )?;
    let member = stores.reach.member_folder(folder)?.root.clone();
    let db = ctx.contexts_db.ok_or_else(|| NO_CONTEXTS_DB.to_owned())?;
    let mut contexts = ContextStore::open(db).map_err(|e| e.to_string())?;
    let home = contexts_session::own_home(&mut contexts, ctx.git, &member)?;
    drop(contexts);
    stores.reload(ctx);
    let member_ctx = Context {
        git: ctx.git,
        root: &member,
        global_config: ctx.global_config,
        contexts_db: ctx.contexts_db,
    };
    store_proposal(
        id,
        stores,
        &member_ctx,
        actor,
        &home.repo_id,
        home.context,
        proposal,
    )
}

/// Stores a proposal at `context`, with `repo_id` on each of its sources (D193 (7)). The checks of
/// the proposal were made by the caller.
fn store_proposal(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    actor: &Actor,
    repo_id: &str,
    context: i64,
    proposal: &Proposal<'_>,
) -> Result<Vec<Event>, String> {
    let is_human = matches!(actor, Actor::Human);
    let text = redacted(ctx, proposal.text)?;
    let mut sources = proposal
        .sources
        .iter()
        .map(|source| stored_source(ctx, source, is_human))
        .collect::<Result<Vec<_>, _>>()?;
    for source in &mut sources {
        source.repo_id = Some(repo_id.to_owned());
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
                basis: proposal.basis,
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

/// Moves the memories of the threads store of the clone at `root` into `contexts.db`, into the context
/// of that clone (D222 (6)). Each memory keeps its uid, sources, marks, retrieval count and links; it is
/// inserted first, the links next, and only then deleted, so a move interrupted at any point loses
/// nothing and the next command finishes it. Nothing happens when no contexts database is set, when the
/// store holds no memory (a read creates no file), or when the clone's id clashes.
fn move_into(
    store: &mut ThreadStore,
    git: &dyn GitPort,
    db: Option<&Path>,
    root: &Path,
) -> Result<(), String> {
    let Some(db) = db else {
        return Ok(());
    };
    if !store.has_memories().map_err(|e| e.to_string())? {
        return Ok(());
    }
    let mut contexts = ContextStore::open(db).map_err(|e| e.to_string())?;
    let Some(home) = contexts_session::move_home(&mut contexts, git, root)? else {
        return Ok(());
    };
    let moved = store.memories_to_move().map_err(|e| e.to_string())?;
    let uids: Vec<String> = moved.iter().map(|(memory, _)| memory.uid.clone()).collect();
    for (mut memory, marks) in moved {
        for source in &mut memory.sources {
            source.repo_id.get_or_insert_with(|| home.repo_id.clone());
        }
        contexts
            .import_memory(&memory, &marks, home.context)
            .map_err(|e| e.to_string())?;
    }
    contexts
        .import_links(&store.supersedes_links().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    for uid in &uids {
        match store.memory_id_of(uid) {
            Ok(row) => store.delete_memory(row).map_err(|e| e.to_string())?,
            Err(StoreError::NotFound) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

/// Moves the memories of the repository's own threads store into its context (D222 (6)). A failure is
/// the caller's to report: a read says it in its note, a write is refused.
pub(crate) fn move_own(store: &mut ThreadStore, ctx: &Context<'_>) -> Result<(), String> {
    move_into(store, ctx.git, ctx.contexts_db, ctx.root)
}

/// Moves the memories of each workspace member's threads store into the context of its clone (D222 (6)).
/// A member that cannot be read, or whose move fails, keeps its memories; the notes returned name it.
/// Nothing is returned for a plain repository.
pub(crate) fn move_members(ctx: &Context<'_>) -> Vec<String> {
    let Some(folders) = workspace_folders(ctx.git, ctx.global_config, ctx.root) else {
        return Vec::new();
    };
    let mut notes = Vec::new();
    for folder in &folders {
        let moved = open_member(ctx.git, &folder.root).and_then(|store| match store {
            Some(mut threads) => move_into(&mut threads, ctx.git, ctx.contexts_db, &folder.root),
            None => Ok(()),
        });
        if let Err(reason) = moved {
            notes.push(member_failed(&folder.name, &reason));
        }
    }
    notes
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

/// The blob at `HEAD` of each file source, as the acceptance records it: not read when the
/// source's folder is not known (D193 (9)).
fn file_blobs(ctx: &Context<'_>, reach: &Reach, memory: &Memory) -> Vec<(String, BlobRead)> {
    memory
        .sources
        .iter()
        .filter(|source| source.kind == MemorySourceKind::File)
        .map(|source| {
            let read = match source_root(ctx, reach, ctx.root, source.repo_id.as_deref()) {
                Ok(root) => BlobRead::of(blob_under(ctx, &root, &source.reference)),
                Err(_) => BlobRead::NotRead,
            };
            (source.reference.clone(), read)
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

/// `MoveMemoryToContext` (D193 (8), D222 (1)): the person moves memory `uid` to `context`. A memory
/// of a repository is stored in `context` under the same uid, with its history and its sources, and
/// then deleted here; a retry after a crash between the two writes finds the copy and only deletes. A
/// different memory of the same text in the context refuses the move, and nothing is deleted. A memory
/// already in `contexts.db` is attached to `context` and detached from every other context, in one
/// transaction, so it keeps its uid, text, history and marks.
pub(crate) fn move_to_context(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    is_human: bool,
    uid: &str,
    context: i64,
) -> Result<Vec<Event>, String> {
    memories::authorize(is_human, &MemoryAction::Move)?;
    match home_of(stores, uid) {
        Ok(Home::Repository(row)) => move_from_repository(id, stores, ctx, row, uid, context),
        Ok(Home::Context) => move_between_contexts(id, stores, ctx, uid, context),
        Err(message) => Err(message),
    }
}

/// Moves a memory of `contexts.db` to `context` (D222 (1)). Its status and links are checked as for a
/// move out of a repository.
fn move_between_contexts(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    uid: &str,
    context: i64,
) -> Result<Vec<Event>, String> {
    let contexts = stores
        .reach
        .store_mut()
        .ok_or_else(|| MEMORY_MISSING.to_owned())?;
    let memory = contexts
        .read_memory_by_uid(uid)
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
    contexts
        .move_memory_to(uid, context)
        .map_err(|e| named(e, MEMORY_MISSING))?;
    let attached = contexts.memory_contexts(uid).map_err(|e| e.to_string())?;
    let memory = contexts
        .read_memory_by_uid(uid)
        .map_err(|e| named(e, MEMORY_MISSING))?;
    Ok(changed(id, ctx, &stores.reach, memory, false, &attached))
}

/// Moves a memory of the repository's own store to `context`, from its row `row` (D193 (8)).
fn move_from_repository(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    row: i64,
    uid: &str,
    context: i64,
) -> Result<Vec<Event>, String> {
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
    let repo_id = writer_of(&mut stores.reach, ctx)?;
    for source in &mut memory.sources {
        source.repo_id = Some(repo_id.clone());
    }
    let history = stores
        .threads
        .memory_mark_history(uid)
        .map_err(|e| e.to_string())?;
    let contexts = stores
        .reach
        .store_mut()
        .ok_or_else(|| NOT_VISIBLE.to_owned())?;
    let (moved, created) = contexts
        .move_memory_in(&memory, &[context], &history)
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

/// `MarkMemory` (D202): `actor` (its origin) marks memory `uid`, of the repository or of any context,
/// `useful` or `wrong`. The reason is redacted like the text and at most 200
/// characters. Only an accepted memory takes a mark; a repeat of the actor's latest mark writes
/// nothing and answers the memory as it is. The memory's text, status and order are not touched.
pub(crate) fn mark(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    actor: &str,
    uid: &str,
    mark: MemoryMark,
    reason: Option<&str>,
) -> Result<Vec<Event>, String> {
    let reason = reason.map(|reason| redacted(ctx, reason)).transpose()?;
    let reason = memories::check_reason(reason.as_deref())?;
    let at = now_ms();
    let (memory, written, attached) = match home_of(stores, uid)? {
        Home::Repository(_) => {
            let (memory, written) = stores
                .threads
                .mark_memory(uid, actor, mark, reason.as_deref(), at)
                .map_err(|e| named(e, MEMORY_MISSING))?;
            (memory, written, Vec::new())
        }
        Home::Context => {
            let contexts = stores
                .reach
                .store_mut()
                .ok_or_else(|| MEMORY_MISSING.to_owned())?;
            let (memory, written) = contexts
                .mark_memory(uid, actor, mark, reason.as_deref(), at)
                .map_err(|e| named(e, MEMORY_MISSING))?;
            let attached = contexts
                .memory_contexts(uid)
                .map_err(|e| named(e, MEMORY_MISSING))?;
            (memory, written, attached)
        }
    };
    Ok(changed(id, ctx, &stores.reach, memory, written, &attached))
}

/// The marks of memory `uid`, oldest first, for `ReadMemory` (D202 (2)). The memory's own `marks` are
/// the latest of each actor; this is the history the person sees.
pub(crate) fn mark_history(
    stores: &Stores<'_>,
    uid: &str,
) -> Result<Vec<MemoryMarkSummary>, String> {
    match home_of(stores, uid)? {
        Home::Repository(_) => stores
            .threads
            .memory_mark_history(uid)
            .map_err(|e| e.to_string()),
        Home::Context => stores
            .reach
            .store()
            .ok_or_else(|| MEMORY_MISSING.to_owned())?
            .memory_mark_history(uid)
            .map_err(|e| e.to_string()),
    }
}

/// `AttachMemory` (D193 (8), D222 (3)): accepted context memory `uid` also joins `context`, any
/// existing context.
pub(crate) fn attach(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    uid: &str,
    context: i64,
) -> Result<Vec<Event>, String> {
    match home_of(stores, uid)? {
        Home::Repository(_) => return Err(MOVE_FIRST.to_owned()),
        Home::Context => {}
    }
    let (memory, _) = context_memory(&stores.reach, uid)?;
    if memory.status != MemoryStatus::Accepted {
        return Err(NOT_ACCEPTED.to_owned());
    }
    let contexts = stores
        .reach
        .store_mut()
        .ok_or_else(|| MEMORY_MISSING.to_owned())?;
    contexts
        .attach_memory(uid, context)
        .map_err(|e| named(e, NO_CONTEXT))?;
    let (memory, attached) = context_memory(&stores.reach, uid)?;
    Ok(changed(id, ctx, &stores.reach, memory, false, &attached))
}

/// `DetachMemory` (D193 (8), D222 (3)): context memory `uid` leaves `context`, any context it is in.
/// The last context is refused by the store.
pub(crate) fn detach(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    uid: &str,
    context: i64,
) -> Result<Vec<Event>, String> {
    match home_of(stores, uid)? {
        Home::Repository(_) => return Err(NO_CONTEXT_OF_ITS_OWN.to_owned()),
        Home::Context => {}
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
            place: if in_contexts {
                Place::Context
            } else {
                Place::Own
            },
            attached: Vec::new(),
            near: distance,
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
        assert_eq!(scope_key(Scope::Context(4)), "context:4");
    }
}
