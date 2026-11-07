//! The engine's side of threads (design/threads.md, "Engine surface"): the store of
//! the open repository, the reads and writes of threads, the Markdown export and the
//! scrub. The commands are declared in `gm-schema`; `engine` routes them here.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gm_schema::threads::{
    AttachKind, CutBound, EditChange, EditState, EntryKind, Evidence, LabelDetail, LinkKind,
    LinkOrigin, MemoryFilter, MemoryKind, MemorySource, MemorySourceInput, MemorySourceKind,
    MemoryStatus, ProvenanceCommit, ProvenanceCut, ProvenanceEdit, ProvenanceLabel, ProvenanceLink,
    ProvenanceResult, ProvenanceThread, RelationDirection, ThreadDetail, ThreadEdit, ThreadFilter,
    ThreadSummary,
};
use gm_schema::{Actor, CommandKind, Event, OperationId};

use crate::blame::BlameLine;
use crate::memory_scope::{self, Home, Stores};
use crate::threads::members;
use crate::threads::memories::{self, BlobRead, MemoryAction, NewMemory};
use crate::threads::provenance::{
    self, COMMIT_LIMIT, CONTEXT, EDIT_LIMIT, EditFact, LABEL_LIMIT, LINK_LIMIT, Owner,
    THREAD_LIMIT, Verdict,
};
use crate::threads::redact::Redactor;
use crate::threads::resolve::{self, FOLLOW_LIMIT, RESOLVE_LIMIT};
use crate::threads::store::{MetaUpdate, SearchRequest, StoreError, ThreadStore};
use crate::workspace_pointer;
use crate::{BlameRead, GitPort};

/// Commit names one Git process reads when a store opens (D145).
const SNAPSHOT_BATCH: usize = 500;
const LABEL_MISSING: &str = "Label not found.";
use crate::memory_scope::MEMORY_MISSING;

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
        let mut store = ThreadStore::open(&common).map_err(|e| e.to_string())?;
        fill_snapshots(&mut store, git, root);
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

/// Takes the snapshot of each commit link that has none, while the commits still exist (D145).
/// The store cannot ask Git, so this runs when the repository's store opens, in one Git process
/// per `SNAPSHOT_BATCH` names. A commit Git no longer knows keeps none, and the link shows its id;
/// failures are ignored.
fn fill_snapshots(store: &mut ThreadStore, git: &dyn GitPort, root: &Path) {
    let Ok(refs) = store.commit_refs_without_snapshot() else {
        return;
    };
    for batch in refs.chunks(SNAPSHOT_BATCH) {
        let Ok(found) = git.commit_snapshots(root, batch) else {
            continue;
        };
        for (reference, snapshot) in found {
            let _ = store.set_commit_snapshot(&reference, &snapshot);
        }
    }
}

/// The origin a row written by `actor` records (D143): `human`, or `agent:<client>`.
pub(crate) fn origin_of(actor: &Actor) -> String {
    match actor {
        Actor::Human => "human".to_owned(),
        Actor::Agent { name } => format!("agent:{name}"),
    }
}

/// Milliseconds since the Unix epoch, the time of a label or relation row.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// What a command reads besides the store: the repository, whose `gitmaster.*` configuration
/// holds the redaction patterns.
pub(crate) struct Context<'a> {
    pub(crate) git: &'a dyn GitPort,
    pub(crate) root: &'a Path,
    /// A file to read as the global Git configuration; `None` is the user's own. For the tests.
    pub(crate) global_config: Option<&'a Path>,
    /// The contexts file (`contexts.db`), read by the memory commands; `None` when there is none.
    pub(crate) contexts_db: Option<&'a Path>,
}

/// What a thread command needs a human for, or `None` when an agent may run it.
/// The reads are open to an agent, and so are the record commands (D146), which write only the
/// store and are logged with their actor (design/threads.md, "Engine surface").
pub(crate) fn human_only(kind: &CommandKind) -> Option<&'static str> {
    Some(match kind {
        CommandKind::ExportThread { .. } => "export a thread",
        CommandKind::UpdateThread { .. } => "change a thread",
        CommandKind::LinkThread { .. } => "link a thread",
        CommandKind::UnlinkThread { .. } => "unlink a thread",
        CommandKind::DeleteThread { .. } => "delete a thread",
        CommandKind::DeleteThreadEntry { .. } => "delete a thread entry",
        CommandKind::RenameLabel { .. } => "rename a label",
        CommandKind::DeleteLabel { .. } => "delete a label",
        CommandKind::DetachLabel { .. } => "detach a label",
        CommandKind::RemoveThreadRelation { .. } => "remove a thread link",
        CommandKind::AcceptMemory { .. } => "accept a memory",
        CommandKind::RejectMemory { .. } => "reject a memory",
        CommandKind::DeleteMemory { .. } => "delete a memory",
        CommandKind::MoveMemoryToContext { .. } => "move a memory to a context",
        CommandKind::AttachMemory { .. } => "attach a memory to a context",
        CommandKind::DetachMemory { .. } => "detach a memory from a context",
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
            | CommandKind::ReadProvenance { .. }
            | CommandKind::ReadThreadEdit { .. }
            | CommandKind::ReadThreadMarkdown { .. }
            | CommandKind::ExportThread { .. }
            | CommandKind::UpdateThread { .. }
            | CommandKind::LinkThread { .. }
            | CommandKind::UnlinkThread { .. }
            | CommandKind::DeleteThread { .. }
            | CommandKind::DeleteThreadEntry { .. }
            | CommandKind::ListLabels { .. }
            | CommandKind::ReadLabel { .. }
            | CommandKind::CreateLabel { .. }
            | CommandKind::AttachLabel { .. }
            | CommandKind::LinkThreads { .. }
            | CommandKind::RenameLabel { .. }
            | CommandKind::DeleteLabel { .. }
            | CommandKind::DetachLabel { .. }
            | CommandKind::RemoveThreadRelation { .. }
            | CommandKind::ScrubThreads
    ) || is_memory_command(kind)
}

/// Whether `kind` is one of the memory commands (D190, D193).
pub(crate) fn is_memory_command(kind: &CommandKind) -> bool {
    matches!(
        kind,
        CommandKind::ListMemories { .. }
            | CommandKind::ReadMemory { .. }
            | CommandKind::ProposeMemory { .. }
            | CommandKind::AcceptMemory { .. }
            | CommandKind::RejectMemory { .. }
            | CommandKind::DeleteMemory { .. }
            | CommandKind::MoveMemoryToContext { .. }
            | CommandKind::AttachMemory { .. }
            | CommandKind::DetachMemory { .. }
    )
}

/// The threads linked to `commit` through a member in each workspace listed in this repository's
/// pointer file (D180 (2)), each marked with its workspace folder. A workspace that is missing,
/// unreadable or written by another schema is skipped without an error.
fn workspace_threads(ctx: &Context<'_>, commit: &str, limit: u32) -> Vec<ThreadSummary> {
    let Ok(common) = ctx.git.common_dir(ctx.root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for workspace in workspace_pointer::read(&common) {
        let Ok(Some(store)) = ThreadStore::open_workspace_read_only(&workspace.join(".git")) else {
            continue;
        };
        let Ok(threads) = store.list_member_commit_threads(commit, limit) else {
            continue;
        };
        let folder = workspace.to_string_lossy().into_owned();
        found.extend(threads.into_iter().map(|mut thread| {
            thread.workspace = Some(folder.clone());
            thread
        }));
    }
    found
}

/// Runs one thread command against `store`. Writes return `ThreadsChanged` after
/// their own answer; the caller has checked the actor, and `actor` is the origin of the rows
/// a record command writes.
pub(crate) fn run(
    id: OperationId,
    store: &mut ThreadStore,
    ctx: &Context<'_>,
    actor: &Actor,
    kind: &CommandKind,
) -> Result<Vec<Event>, String> {
    let origin = origin_of(actor);
    match kind {
        CommandKind::ListLabels { .. }
        | CommandKind::ReadLabel { .. }
        | CommandKind::CreateLabel { .. }
        | CommandKind::AttachLabel { .. }
        | CommandKind::LinkThreads { .. }
        | CommandKind::RenameLabel { .. }
        | CommandKind::DeleteLabel { .. }
        | CommandKind::DetachLabel { .. }
        | CommandKind::RemoveThreadRelation { .. } => labels_command(id, store, ctx, &origin, kind),
        _ if is_memory_command(kind) => memories_command(id, store, ctx, actor, kind),
        CommandKind::ListThreads {
            filter,
            before_at,
            before_uid,
            limit,
        } => list_threads(
            id,
            store,
            ctx,
            filter,
            *before_at,
            before_uid.as_deref(),
            *limit,
        ),
        CommandKind::SearchThreads {
            query,
            filter,
            limit,
            cursor,
            group,
            order,
            context,
            facets,
        } => {
            let request = SearchRequest {
                query,
                filter,
                limit: page(*limit)?,
                cursor: cursor.as_deref(),
                group: *group,
                order: *order,
                context: *context,
                facets: *facets,
            };
            let found = store.search_page(&request).map_err(|e| e.to_string())?;
            Ok(vec![Event::ThreadSearchResults {
                id,
                hits: found.hits,
                has_more: found.has_more,
                next_cursor: found.next_cursor,
                facets: found.facets,
                diagnosis: found.diagnosis,
            }])
        }
        CommandKind::ReadThread {
            uid,
            from_seq,
            limit,
        } => {
            let mut detail = store
                .read_thread(uid, *from_seq, page(*limit)?)
                .map_err(|e| named(e, THREAD_MISSING))?;
            resolve_thread(store, ctx, &mut detail)?;
            Ok(vec![Event::ThreadRead {
                id,
                detail: Box::new(detail),
            }])
        }
        CommandKind::ReadThreadEdit { id: edit_id } => {
            let edit = store
                .read_edit(*edit_id)
                .map_err(|e| named(e, "Edit not found."))?;
            let state = edit_state(ctx, &edit);
            Ok(vec![Event::ThreadEditRead { id, edit, state }])
        }
        CommandKind::ReadThreadMarkdown { uid } => {
            let markdown = thread_markdown(store, uid)?;
            Ok(vec![Event::ThreadMarkdown {
                id,
                uid: uid.clone(),
                markdown,
            }])
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

/// Lists one page of threads; a commit's first page also holds its workspace threads (D180 (2)).
fn list_threads(
    id: OperationId,
    store: &ThreadStore,
    ctx: &Context<'_>,
    filter: &ThreadFilter,
    before_at: Option<i64>,
    before_uid: Option<&str>,
    limit: u32,
) -> Result<Vec<Event>, String> {
    let before = cursor(before_at, before_uid)?;
    let size = page(limit)?;
    let mut threads = store
        .list_threads(filter, before, size)
        .map_err(|e| e.to_string())?;
    // A commit's threads also come from the workspaces this repository is a member of, on
    // the first page (D180 (2)). The page keeps its size: a workspace thread that does not
    // fit on the first page is not listed, and later pages hold only this store's threads.
    if let (Some(commit), None) = (&filter.commit, before_at) {
        threads.extend(workspace_threads(ctx, commit, size));
        threads.sort_by(|a, b| (b.last_event_at, &b.uid).cmp(&(a.last_event_at, &a.uid)));
        threads.truncate(usize::try_from(size).unwrap_or(usize::MAX));
    }
    let total = store.total_threads().map_err(|e| e.to_string())?;
    Ok(vec![Event::ThreadsListed { id, threads, total }])
}

/// The label and relation commands (D143, D145). The reads answer their lists; a write names the
/// rows it makes with `origin`, the actor's, and answers the row as it now is.
fn labels_command(
    id: OperationId,
    store: &mut ThreadStore,
    ctx: &Context<'_>,
    origin: &str,
    kind: &CommandKind,
) -> Result<Vec<Event>, String> {
    match kind {
        CommandKind::ListLabels { query, limit } => {
            let labels = store
                .list_labels(query, page(*limit)?)
                .map_err(|e| e.to_string())?;
            Ok(vec![Event::LabelsListed { id, labels }])
        }
        CommandKind::ReadLabel { id: label } => {
            let mut detail = store
                .read_label(*label)
                .map_err(|e| named(e, LABEL_MISSING))?;
            resolve_label(store, ctx, &mut detail)?;
            Ok(vec![Event::LabelRead { id, detail }])
        }
        CommandKind::CreateLabel {
            name,
            description,
            color,
        } => {
            let label = store
                .create_label(name, description, color.as_deref(), origin, now_ms())
                .map_err(|e| e.to_string())?;
            Ok(label_written(id, Some(label)))
        }
        CommandKind::AttachLabel {
            label,
            kind: attach,
            reference,
        } => {
            let snapshot = match attach {
                AttachKind::Commit => Some(
                    ctx.git
                        .commit_snapshot(ctx.root, reference)
                        .map_err(|e| e.0)?,
                ),
                _ => None,
            };
            let (label, _) = store
                .attach_label(
                    label,
                    *attach,
                    reference,
                    snapshot.as_ref(),
                    origin,
                    now_ms(),
                )
                .map_err(|e| e.to_string())?;
            Ok(label_written(id, Some(label)))
        }
        CommandKind::LinkThreads { from, to, kind } => {
            let relation = store
                .link_threads(from, to, *kind, origin, now_ms())
                .map_err(|e| e.to_string())?;
            Ok(vec![
                Event::ThreadRelationChanged {
                    id,
                    relation,
                    removed: false,
                },
                Event::ThreadsChanged,
            ])
        }
        CommandKind::RenameLabel { id: label, name } => {
            let label = store
                .rename_label(*label, name)
                .map_err(|e| named(e, LABEL_MISSING))?;
            Ok(label_written(id, Some(label)))
        }
        CommandKind::DeleteLabel { id: label } => {
            store
                .delete_label(*label)
                .map_err(|e| named(e, LABEL_MISSING))?;
            Ok(label_written(id, None))
        }
        CommandKind::DetachLabel {
            label,
            kind: attach,
            reference,
        } => {
            store
                .detach_label(label, *attach, reference)
                .map_err(|e| named(e, "No such label on that item."))?;
            Ok(label_written(id, None))
        }
        CommandKind::RemoveThreadRelation { from, to, kind } => {
            let relation = store
                .remove_thread_relation(from, to, *kind)
                .map_err(|e| named(e, "No such link between those threads."))?;
            Ok(vec![
                Event::ThreadRelationChanged {
                    id,
                    relation,
                    removed: true,
                },
                Event::ThreadsChanged,
            ])
        }
        _ => Err("not a label command".to_owned()),
    }
}

/// The events of a label write: the label as it now is, or none when it is gone, then the
/// thread rows, which show their labels.
fn label_written(id: OperationId, label: Option<gm_schema::threads::Label>) -> Vec<Event> {
    vec![Event::LabelsChanged { id, label }, Event::ThreadsChanged]
}

/// What a memory proposal says, before the store sees it (D190 (1), D193 (7)).
pub(crate) struct Proposal<'a> {
    pub(crate) kind: MemoryKind,
    pub(crate) text: &'a str,
    pub(crate) sources: &'a [MemorySourceInput],
}

/// The memory commands (D190, D192, D193). `memories::authorize` checks the actor here as well as in
/// `human_only`, and the text is redacted before the store sees it (D121). An agent's proposal is
/// `proposed`; a human's is `accepted` and records the blob of each file source at `HEAD`. A memory
/// of a context is found and acted on through `memory_scope`, which reads `contexts.db`.
fn memories_command(
    id: OperationId,
    store: &mut ThreadStore,
    ctx: &Context<'_>,
    actor: &Actor,
    kind: &CommandKind,
) -> Result<Vec<Event>, String> {
    let is_human = matches!(actor, Actor::Human);
    let mut stores = Stores::new(store, ctx);
    match kind {
        CommandKind::ListMemories {
            query,
            kind: memory_kind,
            status,
            path,
            author,
            scope,
            limit,
            cursor,
        } => {
            // An agent reads the accepted memories unless it asks for another status (D192 (7)).
            let filter = MemoryFilter {
                kind: *memory_kind,
                status: (*status).or((!is_human).then_some(MemoryStatus::Accepted)),
                path: path.clone(),
                author: author.clone(),
            };
            let listed = memory_scope::list_command(
                &stores,
                ctx,
                &memory_scope::Ask {
                    query,
                    filter: &filter,
                    scope: scope.as_deref(),
                    limit: page(*limit)?,
                    cursor: cursor.as_deref(),
                },
            )?;
            Ok(vec![Event::MemoriesListed {
                id,
                memories: listed.memories,
                next_cursor: listed.next_cursor,
                contexts_read: listed.contexts_read,
                note: listed.note,
                proposed_total: listed.proposed_total,
            }])
        }
        CommandKind::ReadMemory { uid } => {
            let memory = memory_scope::read(&stores, ctx, uid)?;
            Ok(vec![Event::MemoryRead { id, memory }])
        }
        CommandKind::ProposeMemory {
            kind: memory_kind,
            text,
            sources,
            context,
        } => {
            let proposal = Proposal {
                kind: *memory_kind,
                text,
                sources,
            };
            match context {
                None => propose_memory(id, &mut stores, ctx, actor, &proposal),
                Some(name) => {
                    memory_scope::propose_in_context(id, &mut stores, ctx, actor, name, &proposal)
                }
            }
        }
        CommandKind::AcceptMemory { .. }
        | CommandKind::RejectMemory { .. }
        | CommandKind::DeleteMemory { .. } => act_on_memory(id, &mut stores, ctx, is_human, kind),
        CommandKind::MoveMemoryToContext { uid, context } => {
            memory_scope::move_to_context(id, &mut stores, ctx, uid, *context)
        }
        CommandKind::AttachMemory { uid, context } => {
            memory_scope::attach(id, &mut stores, ctx, uid, *context)
        }
        CommandKind::DetachMemory { uid, context } => {
            memory_scope::detach(id, &mut stores, ctx, uid, *context)
        }
        _ => Err("not a memory command".to_owned()),
    }
}

/// Accepts, rejects or deletes a memory, of the repository's store or of a context (D192, D193
/// (7)). The engine has checked the actor; each action checks it again.
fn act_on_memory(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    is_human: bool,
    kind: &CommandKind,
) -> Result<Vec<Event>, String> {
    match kind {
        CommandKind::AcceptMemory {
            uid,
            text,
            supersedes,
        } => match memory_scope::home_of(stores, uid)? {
            Home::Context => memory_scope::accept_in_context(
                id,
                stores,
                ctx,
                is_human,
                uid,
                text.as_deref(),
                supersedes,
            ),
            Home::Repository(_) => {
                accept_memory(id, stores, ctx, is_human, uid, text.as_deref(), supersedes)
            }
        },
        CommandKind::RejectMemory { uid } => match memory_scope::home_of(stores, uid)? {
            Home::Context => memory_scope::reject_in_context(id, stores, ctx, is_human, uid),
            Home::Repository(_) => reject_memory(id, stores, ctx, is_human, uid),
        },
        CommandKind::DeleteMemory { uid } => match memory_scope::home_of(stores, uid)? {
            Home::Context => memory_scope::delete_in_context(id, stores, is_human, uid),
            Home::Repository(_) => delete_memory(id, stores, is_human, uid),
        },
        _ => Err("not a memory command".to_owned()),
    }
}

/// The row id of the repository's memory `uid`, or `MEMORY_MISSING`.
fn memory_row(store: &ThreadStore, uid: &str) -> Result<i64, String> {
    store
        .memory_id_of(uid)
        .map_err(|e| named(e, MEMORY_MISSING))
}

/// The row ids of the memories a human supersedes. A uid of the repository's store is the only one
/// it may name: a uid of a context is another scope, and a memory supersedes only memories of its
/// own store (D193 (1)).
fn supersede_rows(stores: &Stores<'_>, uids: &[String]) -> Result<Vec<i64>, String> {
    uids.iter()
        .map(|uid| {
            stores.threads.memory_id_of(uid).map_err(|e| match e {
                StoreError::NotFound if memory_scope::is_in_contexts(stores, uid) => {
                    memory_scope::other_scope(uid)
                }
                StoreError::NotFound => format!("memory {uid} is not in this repository"),
                other => other.to_string(),
            })
        })
        .collect()
}

/// Proposes a memory in the repository's own store: an agent's is `proposed`, a human's is
/// `accepted` (D190, D192).
fn propose_memory(
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
    let text = redacted(ctx, proposal.text)?;
    let kept = proposal
        .sources
        .iter()
        .map(|source| stored_source(ctx, source, is_human))
        .collect::<Result<Vec<_>, _>>()?;
    let status = if is_human {
        MemoryStatus::Accepted
    } else {
        MemoryStatus::Proposed
    };
    let (memory, created) = stores
        .threads
        .propose_memory(&NewMemory {
            kind: proposal.kind,
            text: &text,
            status,
            origin: &origin_of(actor),
            sources: &kept,
            at: now_ms(),
        })
        .map_err(|e| e.to_string())?;
    Ok(memory_scope::repository_changed(
        id,
        ctx,
        &stores.reach,
        memory,
        created,
    ))
}

/// Accepts memory `uid` of the repository's store, recording the blob of each file source at `HEAD`
/// (D192).
fn accept_memory(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    is_human: bool,
    uid: &str,
    text: Option<&str>,
    supersedes: &[String],
) -> Result<Vec<Event>, String> {
    memories::authorize(is_human, &MemoryAction::Accept)?;
    let memory_id = memory_row(stores.threads, uid)?;
    let supersedes = supersede_rows(stores, supersedes)?;
    let text = text.map(|text| redacted(ctx, text)).transpose()?;
    // The blobs are read before the write, so the acceptance records what HEAD holds now. An
    // accepted memory keeps the blobs of its first acceptance (D192 (4)), so none are read.
    let already_accepted = stores
        .threads
        .read_memory(memory_id)
        .map_err(|e| named(e, MEMORY_MISSING))?
        .status
        == MemoryStatus::Accepted;
    let blobs: Vec<(String, BlobRead)> = if already_accepted {
        Vec::new()
    } else {
        stores
            .threads
            .memory_file_sources(memory_id)
            .map_err(|e| named(e, MEMORY_MISSING))?
            .into_iter()
            .map(|path| {
                let blob = blob_at_head(ctx, &path);
                (path, blob)
            })
            .collect()
    };
    let memory = stores
        .threads
        .accept_memory(memory_id, text.as_deref(), &supersedes, &blobs, now_ms())
        .map_err(|e| named(e, MEMORY_MISSING))?;
    Ok(memory_scope::repository_changed(
        id,
        ctx,
        &stores.reach,
        memory,
        false,
    ))
}

/// Rejects the proposed memory `uid` of the repository's store (D192).
fn reject_memory(
    id: OperationId,
    stores: &mut Stores<'_>,
    ctx: &Context<'_>,
    is_human: bool,
    uid: &str,
) -> Result<Vec<Event>, String> {
    memories::authorize(is_human, &MemoryAction::Reject)?;
    let memory_id = memory_row(stores.threads, uid)?;
    let memory = stores
        .threads
        .reject_memory(memory_id, now_ms())
        .map_err(|e| named(e, MEMORY_MISSING))?;
    Ok(memory_scope::repository_changed(
        id,
        ctx,
        &stores.reach,
        memory,
        false,
    ))
}

/// Deletes memory `uid` of the repository's store with its sources and links (D192).
fn delete_memory(
    id: OperationId,
    stores: &mut Stores<'_>,
    is_human: bool,
    uid: &str,
) -> Result<Vec<Event>, String> {
    memories::authorize(is_human, &MemoryAction::Delete)?;
    let memory_id = memory_row(stores.threads, uid)?;
    stores
        .threads
        .delete_memory(memory_id)
        .map_err(|e| named(e, MEMORY_MISSING))?;
    Ok(vec![Event::MemoryChanged {
        id,
        memory: None,
        created: false,
    }])
}

/// The text with the repository's redaction patterns applied (D121).
pub(crate) fn redacted(ctx: &Context<'_>, text: &str) -> Result<String, String> {
    Ok(redactor(ctx)?.redact(text).text)
}

/// A source as the store keeps it. A commit is resolved by Git first, so a short id is stored as
/// its full id and an unknown one is refused (D192 (5)). A human's file source records its blob at
/// `HEAD` now (D192 (3)); an agent's does not, since its proposal is not accepted yet.
pub(crate) fn stored_source(
    ctx: &Context<'_>,
    source: &MemorySourceInput,
    is_human: bool,
) -> Result<MemorySource, String> {
    let source = if source.kind == MemorySourceKind::Commit {
        let full = ctx
            .git
            .resolve_commit(ctx.root, &source.reference)
            .map_err(|_| format!("commit {} is not in this repository", source.reference))?;
        MemorySourceInput {
            reference: full,
            ..source.clone()
        }
    } else {
        source.clone()
    };
    let checked = memories::check_source(&source).map_err(|e| e.to_string())?;
    let read = match checked.kind {
        MemorySourceKind::File if is_human => blob_at_head(ctx, &checked.reference),
        _ => BlobRead::NotRead,
    };
    Ok(MemorySource {
        kind: checked.kind,
        reference: checked.reference,
        repo_id: None,
        entry_from: checked.entry_from,
        entry_to: checked.entry_to,
        blob_oid: read.oid().map(str::to_owned),
        blob_read: read.was_read(),
    })
}

/// The blob of `path` at `HEAD`: absent when there is no such file, not read when Git cannot
/// answer (D193 (9)).
fn blob_at_head(ctx: &Context<'_>, path: &str) -> BlobRead {
    BlobRead::of(ctx.git.blob_at_commit(ctx.root, "HEAD", path))
}

/// Records the replays of a restack in the threads store (D142, D145), when the repository has
/// one. A failure keeps the store as it was: a read then matches the commit by its patch id.
pub(crate) fn remap(
    slot: &mut Option<ThreadsSession>,
    git: &dyn GitPort,
    root: &Path,
    pairs: &[(String, String)],
) {
    if pairs.is_empty() {
        return;
    }
    let Ok(common) = git.common_dir(root) else {
        return;
    };
    // Only a repository that already has a store is written: a restack does not create one.
    if !common.join("gitmaster").join("threads.db").exists() {
        return;
    }
    if let Ok(store) = store(slot, git, root) {
        let _ = store.remap_commits(pairs);
    }
}

/// Shows what the commit links of a thread resolve to now (D145). The stored ids are not changed;
/// each answer that is cached is written, so the next read does not search again.
fn resolve_thread(
    store: &mut ThreadStore,
    ctx: &Context<'_>,
    detail: &mut ThreadDetail,
) -> Result<(), String> {
    let uid = detail.summary.uid.clone();
    let cache = store.commit_link_cache(&uid).map_err(|e| e.to_string())?;
    let mut budget = RESOLVE_LIMIT;
    for link in detail
        .links
        .iter_mut()
        .filter(|link| link.kind == LinkKind::Commit)
    {
        let cached = cache
            .iter()
            .find(|(reference, _)| *reference == link.reference)
            .and_then(|(_, value)| value.clone());
        let answer = resolve::commit(
            ctx.git,
            ctx.root,
            &link.reference,
            detail.summary.branch.as_deref(),
            cached.as_deref(),
            &mut budget,
        );
        link.resolved = answer.resolved;
        link.state = Some(answer.state);
        if let Some(value) = answer.cache {
            store
                .set_link_cache(&uid, &link.reference, &value)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Shows what the commit attachments of a label resolve to, and where its file attachments are now
/// (D145). The label's file paths are followed through renames, at most `FOLLOW_LIMIT` of them.
fn resolve_label(
    store: &mut ThreadStore,
    ctx: &Context<'_>,
    detail: &mut LabelDetail,
) -> Result<(), String> {
    let label = detail.label.id;
    let mut budget = RESOLVE_LIMIT;
    let mut follows = FOLLOW_LIMIT;
    let mut renames: Option<Vec<(String, String)>> = None;
    for attachment in &mut detail.attachments {
        match attachment.kind {
            AttachKind::Commit => {
                let cached = attachment.resolved.take();
                let answer = resolve::commit(
                    ctx.git,
                    ctx.root,
                    &attachment.reference,
                    None,
                    cached.as_deref(),
                    &mut budget,
                );
                attachment.resolved = answer.resolved;
                attachment.state = Some(answer.state);
                if let Some(value) = answer.cache {
                    store
                        .set_attach_cache(label, &attachment.reference, &value)
                        .map_err(|e| e.to_string())?;
                }
            }
            AttachKind::File => {
                if follows == 0 {
                    continue;
                }
                follows -= 1;
                attachment.current_path =
                    Some(follow_file(ctx, &mut renames, &attachment.reference));
            }
            AttachKind::Thread | AttachKind::Label => {}
        }
    }
    Ok(())
}

/// The path a file has now (D145). A path that exists at HEAD is its own; any other is followed
/// through the renames of the history, which are read once per read.
fn follow_file(
    ctx: &Context<'_>,
    renames: &mut Option<Vec<(String, String)>>,
    path: &str,
) -> String {
    if ctx
        .git
        .blob_at_commit(ctx.root, "HEAD", path)
        .ok()
        .flatten()
        .is_some()
    {
        return path.to_owned();
    }
    let list = renames.get_or_insert_with(|| ctx.git.renames_to_head(ctx.root).unwrap_or_default());
    resolve::follow(list, path)
}

/// Fills the second edge of a provenance thread (D144): its labels and the threads linked to it,
/// each at most its limit, and a cut says so. The linked threads are named, not walked further.
fn second_edge(
    store: &ThreadStore,
    commit: &str,
    thread: &mut ProvenanceThread,
    cuts: &mut Vec<ProvenanceCut>,
) -> Result<(), String> {
    let mut labels = store
        .thread_labels(&thread.uid, bound(LABEL_LIMIT + 1))
        .map_err(|e| e.to_string())?;
    if provenance::cut(&mut labels, LABEL_LIMIT) {
        cuts.push(ProvenanceCut {
            bound: CutBound::Labels,
            limit: bound(LABEL_LIMIT),
            commit: Some(commit.to_owned()),
        });
    }
    thread.labels = labels
        .into_iter()
        .map(|(name, origin)| ProvenanceLabel { name, origin })
        .collect();

    let mut relations = store
        .thread_relations(&thread.uid, bound(LINK_LIMIT + 1))
        .map_err(|e| e.to_string())?;
    if provenance::cut(&mut relations, LINK_LIMIT) {
        cuts.push(ProvenanceCut {
            bound: CutBound::Links,
            limit: bound(LINK_LIMIT),
            commit: Some(commit.to_owned()),
        });
    }
    let mut links = Vec::with_capacity(relations.len());
    for (uid, kind, outgoing, origin) in relations {
        let title = store.thread_summary(&uid).map_err(|e| e.to_string())?.title;
        links.push(ProvenanceLink {
            uid,
            title,
            kind,
            direction: if outgoing {
                RelationDirection::Out
            } else {
                RelationDirection::In
            },
            origin,
        });
    }
    thread.links = links;
    Ok(())
}

/// Checks a provenance request before anything is read (D144).
pub(crate) fn check_provenance(path: &str, from: u32, to: u32) -> Result<(), String> {
    provenance::check_path(path)?;
    provenance::check_range(from, to)
}

/// The blame of a provenance request, started by `ReadProvenance` (D144). It runs on a thread of
/// its own and stops like a search once `stop` is set; its answer is dropped by `id`.
pub struct ProvenanceJob {
    pub id: OperationId,
    pub read: BlameRead,
    pub stop: Arc<AtomicBool>,
}

/// The lines blame gave for a provenance request, or Git's message.
pub struct ProvenanceReport {
    pub id: OperationId,
    pub lines: Result<Vec<BlameLine>, String>,
}

impl ProvenanceJob {
    /// Runs the blame to its end or to the stop request.
    #[must_use]
    pub fn run(self) -> ProvenanceReport {
        let lines = (self.read)(&self.stop).map_err(|error| error.0);
        ProvenanceReport { id: self.id, lines }
    }
}

/// Explains lines `from` to `to` of `path` (D144) from the lines blame gave: the commits, each
/// commit the threads linked to it and the edits that match its lines, each edit the entries
/// around it. The bounds are the rules' (`provenance`), and a cut result says what it cut.
/// The lines carry workspace paths. `member` is the member their commits live in (D181): the
/// blob of each commit is read from its checkout, not from the workspace.
pub(crate) fn provenance_from_lines(
    store: &mut ThreadStore,
    ctx: &Context<'_>,
    path: &str,
    from: u32,
    to: u32,
    lines: &[BlameLine],
    member: Option<&str>,
) -> Result<ProvenanceResult, String> {
    let owners = provenance::owners(lines);
    let mut result = ProvenanceResult {
        path: path.to_owned(),
        from_line: from,
        to_line: to,
        commits: Vec::new(),
        cuts: Vec::new(),
    };
    if owners.len() > COMMIT_LIMIT {
        result.cuts.push(ProvenanceCut {
            bound: CutBound::Commits,
            limit: bound(COMMIT_LIMIT),
            commit: None,
        });
    }
    for owner in owners.into_iter().take(COMMIT_LIMIT) {
        let commit = commit_provenance(store, ctx, &owner, member, &mut result.cuts)?;
        result.commits.push(commit);
    }
    Ok(result)
}

/// The threads that explain one commit's lines, at most `THREAD_LIMIT` of them.
fn commit_provenance(
    store: &ThreadStore,
    ctx: &Context<'_>,
    owner: &Owner,
    member: Option<&str>,
    cuts: &mut Vec<ProvenanceCut>,
) -> Result<ProvenanceCommit, String> {
    // A member's commits are in its own repository, so the blob is read there, under the path
    // inside the member (D181). The store keeps the workspace path.
    let (checkout, inside) = match member.and_then(|name| {
        members::in_member(&owner.path, name).map(|inside| (ctx.root.join(name), inside))
    }) {
        Some(checkout) => checkout,
        None => (ctx.root.to_path_buf(), owner.path.as_str()),
    };
    let blob = ctx
        .git
        .blob_at_commit(&checkout, &owner.commit, inside)
        .map_err(|e| e.0)?;
    let limit = u32::try_from(EDIT_LIMIT + 1).unwrap_or(u32::MAX);
    let rows = store
        .path_edits(&owner.path, owner.author_at, blob.as_deref(), limit)
        .map_err(|e| e.to_string())?;
    if rows.len() > EDIT_LIMIT {
        cuts.push(ProvenanceCut {
            bound: CutBound::Candidates,
            limit: bound(EDIT_LIMIT),
            commit: Some(owner.commit.clone()),
        });
    }
    let judged: Vec<(EditFact, Verdict)> = rows
        .into_iter()
        .take(EDIT_LIMIT)
        .map(|row| {
            let fact = EditFact::new(row);
            let verdict = provenance::judge(owner, blob.as_deref(), &fact);
            (fact, verdict)
        })
        .filter(|(_, verdict)| !verdict.evidence.is_empty() || verdict.unverifiable.is_some())
        .collect();
    let ambiguous = judged
        .iter()
        .filter(|(_, verdict)| verdict.decisive())
        .count()
        > 1;

    let mut drafts: BTreeMap<String, Draft> = BTreeMap::new();
    for (uid, origin) in store
        .commit_links(&owner.commit)
        .map_err(|e| e.to_string())?
    {
        let evidence = match origin {
            LinkOrigin::Observed => Evidence::ObservedCommit,
            LinkOrigin::Human => Evidence::Manual,
        };
        drafts.entry(uid).or_default().evidence.push(evidence);
    }
    add_edits(store, &mut drafts, judged, ambiguous)?;

    let mut threads = Vec::new();
    for (uid, mut draft) in drafts {
        let summary = store.thread_summary(&uid).map_err(|e| e.to_string())?;
        provenance::strongest_first(&mut draft.evidence);
        let thread = ProvenanceThread {
            uid,
            title: summary.title,
            harness: summary.harness,
            branch: summary.branch,
            evidence: draft.evidence,
            ambiguous: draft.ambiguous,
            edits: draft.edits,
            labels: Vec::new(),
            links: Vec::new(),
        };
        threads.push((thread, summary.last_event_at));
    }
    threads.sort_by(|(a, a_at), (b, b_at)| {
        provenance::thread_rank(&a.evidence, *a_at)
            .cmp(&provenance::thread_rank(&b.evidence, *b_at))
            .then_with(|| a.uid.cmp(&b.uid))
    });
    if threads.len() > THREAD_LIMIT {
        cuts.push(ProvenanceCut {
            bound: CutBound::Threads,
            limit: bound(THREAD_LIMIT),
            commit: Some(owner.commit.clone()),
        });
        threads.truncate(THREAD_LIMIT);
    }
    for (thread, _) in &mut threads {
        second_edge(store, &owner.commit, thread, cuts)?;
    }

    Ok(ProvenanceCommit {
        id: owner.commit.clone(),
        subject: owner.subject.clone(),
        author: owner.author.clone(),
        author_at: owner.author_at,
        path: owner.path.clone(),
        lines: owner.runs.clone(),
        threads: threads.into_iter().map(|(thread, _)| thread).collect(),
    })
}

/// Adds each judged edit to the draft of its thread, with the entries around it (D144).
/// `ambiguous` says that other edits also match the lines (D144).
fn add_edits(
    store: &ThreadStore,
    drafts: &mut BTreeMap<String, Draft>,
    judged: Vec<(EditFact, Verdict)>,
    ambiguous: bool,
) -> Result<(), String> {
    for (fact, verdict) in judged {
        let entries = store
            .entries_around(&fact.thread_uid, fact.seq, CONTEXT)
            .map_err(|e| e.to_string())?;
        let draft = drafts.entry(fact.thread_uid.clone()).or_default();
        draft.evidence.extend(verdict.evidence.iter().copied());
        draft.ambiguous |= ambiguous && verdict.decisive();
        draft.edits.push(ProvenanceEdit {
            edit_id: fact.edit_id,
            entry_id: fact.entry_id,
            seq: fact.seq,
            path: fact.path,
            at: fact.at,
            evidence: verdict.evidence,
            unverifiable: verdict.unverifiable,
            entries,
        });
    }
    Ok(())
}

/// What one thread contributes to a commit's answer, before it is cut to `THREAD_LIMIT`.
#[derive(Default)]
struct Draft {
    evidence: Vec<Evidence>,
    ambiguous: bool,
    edits: Vec<ProvenanceEdit>,
}

/// A count of the provenance bounds as the wire type holds it.
fn bound(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Writes thread `uid` as Markdown to `path`, as stored.
fn export(
    id: OperationId,
    store: &mut ThreadStore,
    ctx: &Context<'_>,
    uid: &str,
    path: &str,
) -> Result<Vec<Event>, String> {
    let target = export_target(ctx.root, path)?;
    let text = thread_markdown(store, uid)?;
    std::fs::write(&target, text).map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    Ok(vec![Event::ThreadExported {
        id,
        path: target.to_string_lossy().into_owned(),
    }])
}

/// The whole of thread `uid` as Markdown, the text as stored. Redaction happens at
/// ingest, so nothing is redacted here. The export and `ReadThreadMarkdown` both
/// produce their text here, so they agree.
fn thread_markdown(store: &ThreadStore, uid: &str) -> Result<String, String> {
    let detail = store
        .read_thread(uid, 0, u32::MAX)
        .map_err(|e| named(e, THREAD_MISSING))?;
    Ok(markdown(&detail))
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
        } => {
            let meta = MetaUpdate {
                title: title.clone(),
                note: note.clone(),
                starred: *starred,
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
            if link.kind == gm_schema::threads::LinkKind::Commit
                && let Ok(snapshot) = ctx.git.commit_snapshot(ctx.root, &link.reference)
            {
                let _ = store.set_commit_snapshot(&link.reference, &snapshot);
            }
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
pub(crate) fn named(error: StoreError, missing: &str) -> String {
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

/// The redaction patterns of the `gitmaster.*` configuration, compiled. Invalid patterns refuse
/// the command, as capture does (design/threads.md, "Redaction").
fn redactor(ctx: &Context<'_>) -> Result<Redactor, String> {
    let source = crate::settings::Source {
        global: ctx.global_config.map(Path::to_path_buf),
    };
    let config = crate::settings::Settings::load(ctx.root, &source).threads;
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
/// when the edit was not measured or a file cannot be read. An edit in a workspace
/// member reads its `HEAD` from the member's own repository (D180).
fn edit_state(ctx: &Context<'_>, edit: &ThreadEdit) -> EditState {
    if edit.post_oid.is_none() {
        return EditState::Unknown;
    }
    let working = working_blob(ctx.root, &edit.path);
    let head = match edit.member.as_deref() {
        Some(member) => member_head_blob(ctx.git, ctx.root, member, &edit.path),
        None => head_blob(ctx.git, ctx.root, &edit.path),
    };
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

/// The blob id of the workspace path `path` at the `HEAD` of its `member` (D180). The member's
/// commits are in its own repository, so the path is read there, without the member's folder.
fn member_head_blob(
    git: &dyn GitPort,
    root: &Path,
    member: &str,
    path: &str,
) -> Result<Option<String>, ()> {
    let inside = path
        .strip_prefix(member)
        .and_then(|rest| rest.strip_prefix('/'))
        .ok_or(())?;
    head_blob(git, &root.join(member), inside)
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
    use gm_schema::threads::{
        LinkKind, LinkOrigin, SearchGroup, SearchOrder, ThreadLink, ThreadSummary,
    };
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
            member: None,
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
            Event::ThreadRead { detail, .. } => (**detail).clone(),
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
                cursor: None,
                group: SearchGroup::Entry,
                order: SearchOrder::Relevance,
                context: 0,
                facets: false,
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
                },
                "only a human can change a thread",
            ),
            (
                CommandKind::LinkThread {
                    uid: a.clone(),
                    link: ThreadLink {
                        member: None,
                        kind: LinkKind::Branch,
                        reference: "main".into(),
                        origin: LinkOrigin::Human,
                        resolved: None,
                        state: None,
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
            },
        );
        assert_eq!(state_of(&update), OperationState::Done);
        assert!(has_changed(&update));
        let attached = send(
            &mut engine,
            &human,
            CommandKind::AttachLabel {
                label: "flaky".into(),
                kind: AttachKind::Thread,
                reference: a.clone(),
            },
        );
        assert_eq!(state_of(&attached), OperationState::Done);
        assert!(has_changed(&attached));
        let row = list(&mut engine, ThreadFilter::default())
            .into_iter()
            .find(|t| t.uid == a)
            .expect("a is listed");
        assert_eq!(row.title, "Backoff");
        assert_eq!(row.note, "keep");
        assert!(row.starred);
        assert_eq!(row.tags, vec!["flaky".to_owned()]);

        let link = ThreadLink {
            member: None,
            kind: LinkKind::Commit,
            reference: "abc123".into(),
            origin: LinkOrigin::Human,
            resolved: None,
            state: None,
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
        // A read derives the state of a commit link; this id is not in the fake's history.
        let read_back = read(&mut engine, &a).links;
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].reference, link.reference);
        assert_eq!(read_back[0].origin, link.origin);
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
            &agent,
            CommandKind::ReadProvenance {
                path: "a.txt".into(),
                from_line: 1,
                to_line: 1,
            },
        );
        send(
            &mut engine,
            &human(),
            CommandKind::SearchThreads {
                query: "backoff".into(),
                filter: ThreadFilter::default(),
                limit: 5,
                cursor: None,
                group: SearchGroup::Entry,
                order: SearchOrder::Relevance,
                context: 0,
                facets: false,
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
                | CommandKind::ReadProvenance { .. }
                | CommandKind::ReadThreadEdit { .. }
                | CommandKind::ReadThreadMarkdown { .. }
                | CommandKind::ExportThread { .. }
        )
    }

    #[test]
    fn read_thread_markdown_is_the_text_an_export_writes() {
        let (_dir, root, [a, _, _]) = repository();
        let out = tempfile::tempdir().expect("out dir");
        let settings = ticket_settings(out.path());
        let fake = FakeGit::new(&root, RepoStatus::default());
        let mut store = ThreadStore::open(&root.join(".git")).expect("open");
        let ctx = context(&fake, &root, &settings);

        let events = run(
            OperationId(4),
            &mut store,
            &ctx,
            &human(),
            &CommandKind::ReadThreadMarkdown { uid: a.clone() },
        )
        .expect("read markdown");
        let Event::ThreadMarkdown { id, uid, markdown } = &events[0] else {
            panic!("expected the markdown, got {events:?}");
        };
        assert_eq!(*id, OperationId(4));
        assert_eq!(uid, &a);
        assert!(markdown.starts_with("# retry with backoff for the client\n"));
        assert!(
            markdown.contains("TICKET-42"),
            "the stored text, not redacted again: {markdown}"
        );

        let target = out.path().join("same.md");
        run(
            OperationId(5),
            &mut store,
            &ctx,
            &human(),
            &CommandKind::ExportThread {
                uid: a,
                path: target.to_string_lossy().into_owned(),
            },
        )
        .expect("export");
        let written = std::fs::read_to_string(&target).expect("read export");
        assert_eq!(&written, markdown, "the read and the export are identical");
    }

    #[test]
    fn read_thread_markdown_refuses_unknown_threads() {
        let (_dir, root, [a, _, _]) = repository();
        let out = tempfile::tempdir().expect("out dir");
        let settings = ticket_settings(out.path());
        let fake = FakeGit::new(&root, RepoStatus::default());
        let mut store = ThreadStore::open(&root.join(".git")).expect("open");
        let ctx = context(&fake, &root, &settings);
        let missing = run(
            OperationId(6),
            &mut store,
            &ctx,
            &human(),
            &CommandKind::ReadThreadMarkdown {
                uid: "ffffffffffffffff".into(),
            },
        );
        assert_eq!(missing.unwrap_err(), "Thread not found.");

        // Invalid patterns do not block the read or the export: redaction is an
        // ingest and scrub concern, so the stored text is what both return.
        let broken = out.path().join("broken-gitconfig");
        std::fs::write(&broken, "[gitmaster-redact \"bad\"]\n\tregex = (\n")
            .expect("write settings");
        let broken_ctx = context(&fake, &root, &broken);
        let read = run(
            OperationId(7),
            &mut store,
            &broken_ctx,
            &human(),
            &CommandKind::ReadThreadMarkdown { uid: a.clone() },
        )
        .expect("read markdown with invalid patterns");
        let Event::ThreadMarkdown { markdown, .. } = &read[0] else {
            panic!("expected the markdown, got {read:?}");
        };
        assert!(markdown.contains("TICKET-42"), "{markdown}");
        let target = out.path().join("t.md");
        run(
            OperationId(8),
            &mut store,
            &broken_ctx,
            &human(),
            &CommandKind::ExportThread {
                uid: a,
                path: target.to_string_lossy().into_owned(),
            },
        )
        .expect("export with invalid patterns");
        assert_eq!(
            std::fs::read_to_string(&target).expect("read export"),
            *markdown
        );
    }

    #[test]
    fn an_agent_reads_thread_markdown_without_a_change_or_a_log_entry() {
        let (_dir, root, [a, _, _]) = repository();
        let mut engine = engine_over(&root);
        let events = send(
            &mut engine,
            &agent(),
            CommandKind::ReadThreadMarkdown { uid: a },
        );
        assert_eq!(state_of(&events), OperationState::Done);
        assert!(matches!(answer(&events), Event::ThreadMarkdown { .. }));
        assert!(!has_changed(&events));
        assert!(
            engine
                .log()
                .iter()
                .all(|entry| !is_thread_read(&entry.kind))
        );
    }

    /// A global Git configuration whose redaction patterns match `TICKET-<digits>`.
    fn ticket_settings(dir: &Path) -> PathBuf {
        let path = dir.join("gitconfig");
        std::fs::write(
            &path,
            "[gitmaster-redact \"ticket\"]\n\tregex = TICKET-[0-9]+\n",
        )
        .expect("write settings");
        path
    }

    fn context<'a>(git: &'a dyn GitPort, root: &'a Path, settings: &'a Path) -> Context<'a> {
        Context {
            git,
            root,
            global_config: Some(settings),
            contexts_db: None,
        }
    }

    #[test]
    fn export_writes_the_stored_text_as_it_is() {
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
        let events = run(OperationId(2), &mut store, &ctx, &human(), &kind).expect("export");
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
            text.contains("TICKET-42"),
            "the export does not redact again: {text}"
        );
    }

    #[test]
    fn export_writes_text_scrubbed_earlier_with_the_pattern_applied() {
        let (_dir, root, [a, _, _]) = repository();
        let out = tempfile::tempdir().expect("out dir");
        let settings = ticket_settings(out.path());
        let fake = FakeGit::new(&root, RepoStatus::default());
        let mut store = ThreadStore::open(&root.join(".git")).expect("open");
        let ctx = context(&fake, &root, &settings);
        run(
            OperationId(1),
            &mut store,
            &ctx,
            &human(),
            &CommandKind::ScrubThreads,
        )
        .expect("scrub");

        let target = out.path().join("scrubbed.md");
        run(
            OperationId(2),
            &mut store,
            &ctx,
            &human(),
            &CommandKind::ExportThread {
                uid: a,
                path: target.to_string_lossy().into_owned(),
            },
        )
        .expect("export");
        let text = std::fs::read_to_string(&target).expect("read export");
        assert!(!text.contains("TICKET-42"), "scrubbed earlier: {text}");
        assert!(text.contains("[REDACTED:ticket]"), "{text}");
    }

    #[test]
    fn export_refuses_bad_paths() {
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
                &human(),
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
        let broken = out.path().join("broken-gitconfig");
        std::fs::write(&broken, "[gitmaster-redact \"bad\"]\n\tregex = (\n")
            .expect("write settings");
        let ctx = context(&fake, &root, &broken);
        assert!(
            run(
                OperationId(4),
                &mut store,
                &ctx,
                &human(),
                &CommandKind::ScrubThreads
            )
            .is_err()
        );
        assert!(read_back(&mut store, &a).contains("TICKET-42"));

        let ctx = context(&fake, &root, &settings);
        let events = run(
            OperationId(4),
            &mut store,
            &ctx,
            &human(),
            &CommandKind::ScrubThreads,
        )
        .expect("scrub");
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
