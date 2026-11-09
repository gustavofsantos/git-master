//! The engine's side of contexts (design/threads.md, "Contexts"; D191, D193): the list of the
//! contexts, the commands that change them and the repository identity that decides whether the
//! open clone may use the contexts its id belongs to (D193 (4)). The commands are declared in
//! `gm-schema`; `engine` routes them here. The file is `contexts.db` in the app data directory.

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
        CommandKind::ListContexts { repo } => list(id, db, git, root, global_config, *repo),
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

/// The contexts of a workspace's members, each with the members that see it (D216 (4)): the union of
/// the members' contexts with their parents, nearest first. A member that cannot be read adds none.
fn workspace_contexts(
    store: &mut ContextStore,
    git: &dyn GitPort,
    folders: &[Folder],
) -> Result<Vec<Context>, String> {
    let nodes = store.nodes().map_err(|e| e.to_string())?;
    let mut member_of = Vec::new();
    let mut by_member: Vec<(&str, Vec<i64>)> = Vec::new();
    for folder in folders {
        let Ok(Sight::Contexts {
            member_of: direct, ..
        }) = sight(store, git, &folder.root)
        else {
            continue;
        };
        let sees: Vec<i64> = visibility::visible_contexts(&nodes, &direct)
            .into_iter()
            .map(|(context, _)| context)
            .collect();
        member_of.extend(direct);
        by_member.push((folder.name.as_str(), sees));
    }
    visibility::visible_contexts(&nodes, &member_of)
        .into_iter()
        .map(|(context, _)| {
            let mut read = store.read_context(context).map_err(context_error)?;
            read.members = by_member
                .iter()
                .filter(|(_, sees)| sees.contains(&context))
                .map(|(name, _)| (*name).to_owned())
                .collect();
            Ok(read)
        })
        .collect()
}

/// `ListContexts`: every context, or with `repo` those the open repository sees. A clash lists
/// none and names the other clone (D193 (4)). From a workspace, `repo` lists its members' contexts
/// (D216 (4)).
fn list(
    id: OperationId,
    db: Option<&Path>,
    git: &dyn GitPort,
    root: Option<&Path>,
    global_config: Option<&Path>,
    repo: bool,
) -> Result<Vec<Event>, String> {
    let listed = |contexts, repo_id, clash| Event::ContextsListed {
        id,
        contexts,
        repo_id,
        clash,
    };
    let store = open_read(db)?;
    if !repo {
        let contexts = match store {
            Some(store) => store.list_contexts().map_err(|e| e.to_string())?,
            None => Vec::new(),
        };
        return Ok(vec![listed(contexts, None, None)]);
    }
    if let Some(folders) =
        root.and_then(|root| memory_scope::workspace_folders(git, global_config, root))
    {
        let contexts = match store {
            Some(mut store) => workspace_contexts(&mut store, git, &folders)?,
            None => Vec::new(),
        };
        return Ok(vec![listed(contexts, None, None)]);
    }
    let common = common_of(git, root)?;
    let here = repo_id::read(&common);
    let Some(mut store) = store else {
        return Ok(vec![listed(Vec::new(), here, None)]);
    };
    let event = match repo_identity(&mut store, &common)? {
        RepoState::None => listed(Vec::new(), here, None),
        RepoState::Ok(repo_id) => {
            let contexts = visible(&store, &repo_id)?;
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
