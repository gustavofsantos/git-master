//! The engine's side of contexts (design/threads.md, "Contexts"; D191, D193): the list of the
//! contexts, the commands that change them and the repository identity that decides whether the
//! open clone may use the contexts its id belongs to (D193 (4)). The commands are declared in
//! `gm-schema`; `engine` routes them here. The file is `contexts.db` in the app data directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gm_schema::threads::Context;
use gm_schema::{CommandKind, Event, OperationId};

use crate::GitPort;
use crate::contexts::repo_id::{self, Identity};
use crate::contexts::store::ContextStore;
use crate::contexts::visibility;
use crate::memory_scope::{self, Folder};
use crate::threads::store::StoreError;
use crate::threads_session::now_ms;

const CONTEXT_MISSING: &str = "Context not found.";

/// What the open clone's id says about its contexts (D193 (4)).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RepoState {
    /// No id, or an id in no context: the clone has no contexts.
    None,
    /// The id is in a context, and the clone is where the contexts last saw it.
    Ok(String),
    /// The id is held by another clone at `other_path`: the contexts are not used from this one.
    Clash { repo_id: String, other_path: String },
}

/// Whether `kind` is one of the context commands.
pub(crate) fn is_contexts_command(kind: &CommandKind) -> bool {
    matches!(
        kind,
        CommandKind::ListContexts { .. }
            | CommandKind::PreviewDeleteContext { .. }
            | CommandKind::CreateContext { .. }
            | CommandKind::UpdateContext { .. }
            | CommandKind::DeleteContext { .. }
            | CommandKind::AddRepoToContext { .. }
            | CommandKind::RemoveRepoFromContext { .. }
            | CommandKind::RenewRepoId
    )
}

/// What a context command needs a human for, or `None` when an agent may run it. Only the list is
/// open to an agent (D193 (3) and (4)).
pub(crate) fn human_only(kind: &CommandKind) -> Option<&'static str> {
    Some(match kind {
        CommandKind::CreateContext { .. } => "create a context",
        CommandKind::UpdateContext { .. } => "change a context",
        CommandKind::DeleteContext { .. } => "delete a context",
        CommandKind::AddRepoToContext { .. } => "add a repository to a context",
        CommandKind::RemoveRepoFromContext { .. } => "remove a repository from a context",
        CommandKind::RenewRepoId => "renew a repository id",
        _ => return None,
    })
}

/// Runs one context command. `db` is the contexts file (`None` when there is no data directory),
/// and `root` the open repository, when one is open. `global_config` is the Git configuration a
/// workspace's Git calls read (D177 (5)).
pub(crate) fn run(
    id: OperationId,
    db: Option<&Path>,
    git: &dyn GitPort,
    root: Option<&Path>,
    global_config: Option<&Path>,
    kind: &CommandKind,
) -> Result<Vec<Event>, String> {
    match kind {
        CommandKind::ListContexts { repo, all } => {
            list(id, db, git, root, global_config, *repo, *all)
        }
        CommandKind::PreviewDeleteContext { id: context } => preview_delete(id, db, *context),
        CommandKind::CreateContext { name, parent } => create(id, db, name, *parent),
        CommandKind::UpdateContext { .. } => update(id, db, kind),
        CommandKind::DeleteContext {
            id: context,
            delete_memories,
        } => delete(id, db, *context, *delete_memories),
        CommandKind::AddRepoToContext { context, path } => add_repo(id, db, git, *context, path),
        CommandKind::RemoveRepoFromContext { context, repo_id } => {
            remove_repo(id, db, *context, repo_id)
        }
        CommandKind::RenewRepoId => renew(id, db, git, root),
        _ => Err("not a context command".to_owned()),
    }
}

/// The contexts file for a read: `None` when there is none, which is never created by a read.
fn open_read(db: Option<&Path>) -> Result<Option<ContextStore>, String> {
    match db {
        Some(path) => ContextStore::open_existing(path).map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

/// The contexts file for a write, created when it is missing.
fn open_write(db: Option<&Path>) -> Result<ContextStore, String> {
    let path = db.ok_or_else(|| "the app data directory is not available".to_owned())?;
    ContextStore::open(path).map_err(|e| e.to_string())
}

/// The message of a store error, where `NotFound` names the context.
fn context_error(error: StoreError) -> String {
    match error {
        StoreError::NotFound => CONTEXT_MISSING.to_owned(),
        other => other.to_string(),
    }
}

/// The common directory of the open repository.
fn common_of(git: &dyn GitPort, root: Option<&Path>) -> Result<PathBuf, String> {
    let root = root.ok_or_else(|| "no repository is open".to_owned())?;
    git.common_dir(root).map_err(|e| e.0)
}

/// The answer of a write: the context as it now is and the memories a delete removed.
fn changed(id: OperationId, context: Option<Context>, deleted_memories: u32) -> Vec<Event> {
    vec![Event::ContextsChanged {
        id,
        context,
        deleted_memories,
    }]
}

/// The contexts a repository sees, nearest first, each with its path (D191 (3)).
fn visible(store: &ContextStore, repo_id: &str) -> Result<Vec<Context>, String> {
    let nodes = store.nodes().map_err(|e| e.to_string())?;
    let member_of = store.repo_contexts(repo_id).map_err(|e| e.to_string())?;
    visibility::visible_contexts(&nodes, &member_of)
        .into_iter()
        .map(|(context, _)| store.read_context(context).map_err(context_error))
        .collect()
}

/// Whether the clone whose common directory is `common` may use the contexts its id belongs to
/// (D193 (4)). A recorded path that no longer holds the id means the clone moved, so the path is
/// updated. A recorded path that still holds it means another clone shares the id: a clash.
/// Pub(crate) for the search of the contexts (C1c).
pub(crate) fn repo_identity(store: &mut ContextStore, common: &Path) -> Result<RepoState, String> {
    let Some(id) = repo_id::read(common) else {
        return Ok(RepoState::None);
    };
    let Some(recorded) = store.repo_path(&id).map_err(|e| e.to_string())? else {
        return Ok(RepoState::None);
    };
    let recorded_path = PathBuf::from(&recorded);
    let at_recorded = if recorded_path == common {
        None
    } else {
        repo_id::read(&recorded_path)
    };
    match repo_id::identity(&recorded_path, common, at_recorded.as_deref(), &id) {
        Identity::Same => Ok(RepoState::Ok(id)),
        Identity::Moved => {
            let here = common.to_string_lossy();
            store
                .update_repo_path(&id, &here)
                .map_err(|e| e.to_string())?;
            Ok(RepoState::Ok(id))
        }
        Identity::Clash => Ok(RepoState::Clash {
            repo_id: id,
            other_path: recorded,
        }),
    }
}

/// What a clone's identity says about the contexts it sees (D193 (4)): its id and the contexts it
/// belongs to directly, or nothing, or the other clone it clashes with.
pub(crate) enum Sight {
    Contexts {
        repo_id: String,
        member_of: Vec<i64>,
    },
    Nothing,
    Clash {
        other_path: String,
    },
}

/// The sight of the clone at `root`, read from its common directory (D193 (4), D216 (1)).
pub(crate) fn sight(
    store: &mut ContextStore,
    git: &dyn GitPort,
    root: &Path,
) -> Result<Sight, String> {
    let common = git.common_dir(root).map_err(|e| e.0)?;
    Ok(match repo_identity(store, &common)? {
        RepoState::None => Sight::Nothing,
        RepoState::Ok(repo_id) => {
            let member_of = store.repo_contexts(&repo_id).map_err(|e| e.to_string())?;
            Sight::Contexts { repo_id, member_of }
        }
        RepoState::Clash { other_path, .. } => Sight::Clash { other_path },
    })
}

/// The context a repository's own memories go to (D222 (4), (6)): its id, and the context it is
/// stored in.
pub(crate) struct OwnContext {
    pub(crate) repo_id: String,
    pub(crate) context: i64,
}

/// What the clone's identity says about its binding: its id, if any, and the contexts it is bound to
/// directly, or the other clone it clashes with.
enum Binding {
    Clone {
        repo_id: Option<String>,
        bound: Vec<i64>,
    },
    Clash {
        other_path: String,
    },
}

fn binding(store: &mut ContextStore, git: &dyn GitPort, root: &Path) -> Result<Binding, String> {
    Ok(match sight(store, git, root)? {
        Sight::Nothing => Binding::Clone {
            repo_id: None,
            bound: Vec::new(),
        },
        Sight::Contexts { repo_id, member_of } => Binding::Clone {
            repo_id: Some(repo_id),
            bound: member_of,
        },
        Sight::Clash { other_path } => Binding::Clash { other_path },
    })
}

/// The repository's own context, created and bound when it does not exist yet (D222 (4)). The writer
/// is recorded first, as `record_writer` does, and the context is found by its id.
fn own_context(
    store: &mut ContextStore,
    git: &dyn GitPort,
    root: &Path,
    repo_id: Option<String>,
) -> Result<OwnContext, String> {
    let repo_id = record_writer(store, git, root, repo_id)?;
    let (common, folder) = location(git, root)?;
    let context = store
        .repository_context(&repo_id, &common, &folder, now_ms())
        .map_err(|e| e.to_string())?;
    Ok(OwnContext { repo_id, context })
}

/// The id a memory written from the clone at `root` names on its sources, recorded with its folder
/// (D222 (5)). The contexts a writer is bound to are checked first: a clone whose id clashes is
/// refused, since recording it would take the other clone's folder.
pub(crate) fn writer(
    store: &mut ContextStore,
    git: &dyn GitPort,
    root: &Path,
) -> Result<String, String> {
    match binding(store, git, root)? {
        Binding::Clash { other_path } => Err(memory_scope::clash_note(&other_path)),
        Binding::Clone { repo_id, .. } => record_writer(store, git, root, repo_id),
    }
}

/// Records the clone at `root` as a writer, with its common directory and folder name (D222 (5)),
/// and returns its id. The id is the clone's own, or a new one written to the clone when it has none
/// (`add_repo` writes it the same way). The caller has checked that the id does not clash.
fn record_writer(
    store: &mut ContextStore,
    git: &dyn GitPort,
    root: &Path,
    repo_id: Option<String>,
) -> Result<String, String> {
    let repo_id = if let Some(id) = repo_id {
        id
    } else {
        let common = git.common_dir(root).map_err(|e| e.0)?;
        repo_id::ensure(&common).map_err(|e| e.to_string())?
    };
    let (common, folder) = location(git, root)?;
    store
        .record_repository(&repo_id, &common, &folder, now_ms())
        .map_err(|e| e.to_string())?;
    Ok(repo_id)
}

/// The common directory of the clone at `root` as text, and its folder name.
fn location(git: &dyn GitPort, root: &Path) -> Result<(String, String), String> {
    let common = git.common_dir(root).map_err(|e| e.0)?;
    let top = git.resolve_root(root).map_err(|e| e.0)?;
    let folder = top.file_name().map_or_else(
        || top.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    Ok((common.to_string_lossy().into_owned(), folder))
}

/// The refusal of a proposal in a repository bound to several contexts: it names them by path,
/// sorted (D222 (4)).
fn several_message(store: &ContextStore, bound: &[i64]) -> Result<String, String> {
    let nodes = store.nodes().map_err(|e| e.to_string())?;
    let mut paths: Vec<String> = bound
        .iter()
        .map(|&id| visibility::context_path(&nodes, id))
        .collect();
    paths.sort();
    let names = paths
        .iter()
        .map(|path| format!("\"{path}\""))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "this repository is bound to several contexts: {names}; name one with context"
    ))
}

/// Where a proposal without a context goes (D222 (4)): the one context the repository is bound to,
/// or else its own context, created and bound when needed. Refused when several contexts are bound
/// and when the clone's id clashes.
pub(crate) fn proposal_home(
    store: &mut ContextStore,
    git: &dyn GitPort,
    root: &Path,
) -> Result<OwnContext, String> {
    match binding(store, git, root)? {
        Binding::Clash { other_path } => Err(memory_scope::clash_note(&other_path)),
        Binding::Clone { bound, .. } if bound.len() > 1 => Err(several_message(store, &bound)?),
        Binding::Clone { repo_id, bound } => one_or_own(store, git, root, repo_id, &bound),
    }
}

/// Where the memories of a plain repository move when they leave its `threads.db` (D222 (6)): always
/// the repository's own context, the one with `repository_of` set to its id, whatever contexts it is
/// bound to. A repository's private memories must not become visible to every other repository bound
/// to a shared context. `None` for a clone whose id clashes, which is left as it is.
pub(crate) fn move_home(
    store: &mut ContextStore,
    git: &dyn GitPort,
    root: &Path,
) -> Result<Option<OwnContext>, String> {
    match binding(store, git, root)? {
        Binding::Clash { .. } => Ok(None),
        Binding::Clone { repo_id, .. } => own_context(store, git, root, repo_id).map(Some),
    }
}

/// The one context a clone is bound to, with its id, or else its own context.
fn one_or_own(
    store: &mut ContextStore,
    git: &dyn GitPort,
    root: &Path,
    repo_id: Option<String>,
    bound: &[i64],
) -> Result<OwnContext, String> {
    match (repo_id, bound) {
        (Some(repo_id), [context]) => Ok(OwnContext {
            repo_id: record_writer(store, git, root, Some(repo_id))?,
            context: *context,
        }),
        (repo_id, _) => own_context(store, git, root, repo_id),
    }
}

/// Where the memories a member of a workspace keeps go, and where a proposal `member` names goes
/// (D222 (5)): the member's own context, the one of its clone, created and bound when needed. Refused
/// when the clone's id clashes.
pub(crate) fn own_home(
    store: &mut ContextStore,
    git: &dyn GitPort,
    root: &Path,
) -> Result<OwnContext, String> {
    match binding(store, git, root)? {
        Binding::Clash { other_path } => Err(memory_scope::clash_note(&other_path)),
        Binding::Clone { repo_id, .. } => own_context(store, git, root, repo_id),
    }
}

/// Where a proposal without a context goes in a workspace (D222 (5)): over the contexts bound directly
/// to the workspace repository and to each member. Exactly one is used; none gives the workspace's own
/// context, created and bound to the workspace; several are refused, named by path. A member whose id
/// clashes, or that cannot be read, refuses too, since its bindings are not known.
pub(crate) fn workspace_home(
    store: &mut ContextStore,
    git: &dyn GitPort,
    root: &Path,
    members: &[Folder],
) -> Result<OwnContext, String> {
    let (repo_id, mut bound) = match binding(store, git, root)? {
        Binding::Clash { other_path } => return Err(memory_scope::clash_note(&other_path)),
        Binding::Clone { repo_id, bound } => (repo_id, bound),
    };
    for member in members {
        match binding(store, git, &member.root) {
            Ok(Binding::Clone { bound: direct, .. }) => bound.extend(direct),
            Ok(Binding::Clash { other_path }) => {
                return Err(format!(
                    "member {}: {}",
                    member.name,
                    memory_scope::clash_note(&other_path)
                ));
            }
            Err(error) => {
                return Err(format!(
                    "member {} could not be read ({error})",
                    member.name
                ));
            }
        }
    }
    bound.sort_unstable();
    bound.dedup();
    match bound.as_slice() {
        [] => own_context(store, git, root, repo_id),
        [context] => Ok(OwnContext {
            repo_id: record_writer(store, git, root, repo_id)?,
            context: *context,
        }),
        several => Err(several_message(store, several)?),
    }
}

/// What the repositories of a workspace are bound to (D216 (4), D222 (5)): the contexts bound directly
/// to the workspace and to each member, and for each folder the contexts it sees, with their ancestors.
struct WorkspaceBindings {
    direct: Vec<i64>,
    by_member: Vec<(String, Vec<i64>)>,
}

/// The bindings of a workspace and its members. A member that cannot be read adds none.
fn workspace_bindings(
    store: &mut ContextStore,
    git: &dyn GitPort,
    root: &Path,
    folders: &[Folder],
) -> Result<WorkspaceBindings, String> {
    let nodes = store.nodes().map_err(|e| e.to_string())?;
    let mut direct_all = Vec::new();
    let mut by_member = Vec::new();
    let named = std::iter::once((memory_scope::WORKSPACE, root)).chain(
        folders
            .iter()
            .map(|folder| (folder.name.as_str(), folder.root.as_path())),
    );
    for (name, path) in named {
        let Ok(Sight::Contexts {
            member_of: direct, ..
        }) = sight(store, git, path)
        else {
            continue;
        };
        let sees: Vec<i64> = visibility::visible_contexts(&nodes, &direct)
            .into_iter()
            .map(|(context, _)| context)
            .collect();
        direct_all.extend(direct);
        by_member.push((name.to_owned(), sees));
    }
    Ok(WorkspaceBindings {
        direct: direct_all,
        by_member,
    })
}

/// The contexts a workspace sees, each with the members that see it: the union of the contexts bound
/// to it and to each member with their parents, nearest first. With `every`, every context, each with
/// the members that see it.
fn workspace_contexts(
    store: &ContextStore,
    bindings: &WorkspaceBindings,
    every: bool,
) -> Result<Vec<Context>, String> {
    let nodes = store.nodes().map_err(|e| e.to_string())?;
    let ids: Vec<i64> = if every {
        nodes.iter().map(|node| node.id).collect()
    } else {
        visibility::visible_contexts(&nodes, &bindings.direct)
            .into_iter()
            .map(|(context, _)| context)
            .collect()
    };
    ids.into_iter()
        .map(|context| {
            let mut read = store.read_context(context).map_err(context_error)?;
            read.members = bindings
                .by_member
                .iter()
                .filter(|(_, sees)| sees.contains(&context))
                .map(|(name, _)| name.clone())
                .collect();
            Ok(read)
        })
        .collect()
}

/// Marks each listed context as bound when `direct` names it, and counts the accepted memories it
/// inherits: its own and its ancestors' (D222 (2)). The ancestors of a listed context are listed too,
/// so their own counts are at hand.
fn annotate(store: &ContextStore, contexts: &mut [Context], direct: &[i64]) -> Result<(), String> {
    let nodes = store.nodes().map_err(|e| e.to_string())?;
    let own: BTreeMap<i64, u32> = contexts
        .iter()
        .map(|context| (context.id, context.accepted_memories))
        .collect();
    for context in contexts.iter_mut() {
        context.bound = direct.contains(&context.id);
        context.inherited_memories = visibility::visible_contexts(&nodes, &[context.id])
            .iter()
            .filter_map(|(id, _)| own.get(id))
            .fold(0_u32, |sum, count| sum.saturating_add(*count));
    }
    Ok(())
}

/// `ListContexts`: the contexts the open repository sees, or with `all` every context, or without
/// `repo` every context with no binding read (the app's list). A clash lists none and names the other
/// clone (D193 (4)). From a workspace, `repo` lists the contexts of the workspace and its members
/// (D216 (4), D222 (5)). Each listed context of a repository or a workspace says whether it is bound
/// to it directly and how many memories it inherits (D222 (2), (3)).
fn list(
    id: OperationId,
    db: Option<&Path>,
    git: &dyn GitPort,
    root: Option<&Path>,
    global_config: Option<&Path>,
    repo: bool,
    all: bool,
) -> Result<Vec<Event>, String> {
    let listed = |contexts, repo_id, clash| Event::ContextsListed {
        id,
        contexts,
        repo_id,
        clash,
    };
    let store = open_read(db)?;
    if !repo && !all {
        let contexts = match store {
            Some(store) => store.list_contexts().map_err(|e| e.to_string())?,
            None => Vec::new(),
        };
        return Ok(vec![listed(contexts, None, None)]);
    }
    if let Some(root) = root
        && let Some(folders) = memory_scope::workspace_folders(git, global_config, root)
    {
        let Some(mut store) = store else {
            return Ok(vec![listed(Vec::new(), None, None)]);
        };
        let bindings = workspace_bindings(&mut store, git, root, &folders)?;
        let mut contexts = workspace_contexts(&store, &bindings, all)?;
        annotate(&store, &mut contexts, &bindings.direct)?;
        return Ok(vec![listed(contexts, None, None)]);
    }
    let common = common_of(git, root)?;
    let here = repo_id::read(&common);
    let Some(mut store) = store else {
        return Ok(vec![listed(Vec::new(), here, None)]);
    };
    let event = match repo_identity(&mut store, &common)? {
        RepoState::None => {
            let mut contexts = if all {
                store.list_contexts().map_err(|e| e.to_string())?
            } else {
                Vec::new()
            };
            annotate(&store, &mut contexts, &[])?;
            listed(contexts, here, None)
        }
        RepoState::Ok(repo_id) => {
            let direct = store.repo_contexts(&repo_id).map_err(|e| e.to_string())?;
            let mut contexts = if all {
                store.list_contexts().map_err(|e| e.to_string())?
            } else {
                visible(&store, &repo_id)?
            };
            annotate(&store, &mut contexts, &direct)?;
            listed(contexts, Some(repo_id), None)
        }
        RepoState::Clash {
            repo_id,
            other_path,
        } => listed(Vec::new(), Some(repo_id), Some(other_path)),
    };
    Ok(vec![event])
}

/// The most memories a `ContextDeletePreview` carries; its `total` is the full count.
const DELETE_PREVIEW_LIMIT: usize = 50;

/// `PreviewDeleteContext`: what `DeleteContext` would remove, read without a write. The memories
/// are those `DeleteContext` with `delete_memories` deletes, each scoped to the context's path.
fn preview_delete(id: OperationId, db: Option<&Path>, context: i64) -> Result<Vec<Event>, String> {
    let store = open_read(db)?.ok_or_else(|| CONTEXT_MISSING.to_owned())?;
    let mut preview = store
        .delete_preview(context, DELETE_PREVIEW_LIMIT)
        .map_err(context_error)?;
    let nodes = store.nodes().map_err(|e| e.to_string())?;
    let scope = visibility::context_path(&nodes, context);
    for memory in &mut preview.memories {
        memory.scope.clone_from(&scope);
    }
    Ok(vec![Event::ContextDeletePreview {
        id,
        context,
        sub_contexts: preview.sub_contexts,
        memories: preview.memories,
        total: preview.total,
    }])
}

/// `CreateContext`.
fn create(
    id: OperationId,
    db: Option<&Path>,
    name: &str,
    parent: Option<i64>,
) -> Result<Vec<Event>, String> {
    let mut store = open_write(db)?;
    let context = store
        .create_context(name, parent, now_ms())
        .map_err(|e| e.to_string())?;
    Ok(changed(id, Some(context), 0))
}

/// `UpdateContext`. The command is taken whole, since its parent is a nested option.
fn update(id: OperationId, db: Option<&Path>, kind: &CommandKind) -> Result<Vec<Event>, String> {
    let CommandKind::UpdateContext {
        id: context,
        name,
        parent,
    } = kind
    else {
        return Err("not a context update".to_owned());
    };
    let mut store = open_write(db)?;
    let context = store
        .update_context(*context, name.as_deref(), *parent)
        .map_err(context_error)?;
    Ok(changed(id, Some(context), 0))
}

/// `DeleteContext`: the memories it held alone are counted in the answer.
fn delete(
    id: OperationId,
    db: Option<&Path>,
    context: i64,
    delete_memories: bool,
) -> Result<Vec<Event>, String> {
    let mut store = open_write(db)?;
    let deleted = store
        .delete_context(context, delete_memories)
        .map_err(context_error)?;
    Ok(changed(id, None, deleted))
}

/// `AddRepoToContext`: the repository of `path` joins the context, and gets its `repo-id` on the
/// first join. A clone whose id clashes is refused, since its path would take the other clone's.
fn add_repo(
    id: OperationId,
    db: Option<&Path>,
    git: &dyn GitPort,
    context: i64,
    path: &str,
) -> Result<Vec<Event>, String> {
    let folder = Path::new(path);
    let not_a_repository = || format!("{path} is not a Git repository.");
    let common = git.common_dir(folder).map_err(|_| not_a_repository())?;
    let top = git.resolve_root(folder).map_err(|_| not_a_repository())?;
    let name = top.file_name().map_or_else(
        || path.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let mut store = open_write(db)?;
    store.read_context(context).map_err(context_error)?;
    if let RepoState::Clash { other_path, .. } = repo_identity(&mut store, &common)? {
        return Err(format!(
            "this clone shares its repository id with the clone at {other_path}; renew its id first"
        ));
    }
    let repo_id = repo_id::ensure(&common).map_err(|e| e.to_string())?;
    let here = common.to_string_lossy();
    store
        .record_repository(&repo_id, &here, &name, now_ms())
        .map_err(|e| e.to_string())?;
    store
        .add_repo(context, &repo_id, &here, &name, now_ms())
        .map_err(|e| e.to_string())?;
    let context = store.read_context(context).map_err(context_error)?;
    Ok(changed(id, Some(context), 0))
}

/// `RemoveRepoFromContext`. The memories stay.
fn remove_repo(
    id: OperationId,
    db: Option<&Path>,
    context: i64,
    repo: &str,
) -> Result<Vec<Event>, String> {
    let mut store = open_write(db)?;
    store.read_context(context).map_err(context_error)?;
    store.remove_repo(context, repo).map_err(|e| match e {
        StoreError::NotFound => "The repository is not in this context.".to_owned(),
        other => other.to_string(),
    })?;
    let context = store.read_context(context).map_err(context_error)?;
    Ok(changed(id, Some(context), 0))
}

/// `RenewRepoId`. The clone that keeps the old id is the other one, so only a clone that is itself
/// recorded in the contexts gives its memberships up; a copy's clash leaves them to the original
/// (D193 (4)).
fn renew(
    id: OperationId,
    db: Option<&Path>,
    git: &dyn GitPort,
    root: Option<&Path>,
) -> Result<Vec<Event>, String> {
    let common = common_of(git, root)?;
    if let Some(mut store) = open_read(db)?
        && let RepoState::Ok(old) = repo_identity(&mut store, &common)?
    {
        store.forget_repo(&old).map_err(|e| e.to_string())?;
    }
    repo_id::renew(&common).map_err(|e| e.to_string())?;
    Ok(changed(id, None, 0))
}
