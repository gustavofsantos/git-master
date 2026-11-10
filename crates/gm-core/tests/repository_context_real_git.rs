//! A repository's own memories live in its context (D222 (4), (6)) against real repositories in
//! temporary directories: the context a proposal without a context goes to, the refusals of a
//! repository bound to several contexts, a clash and a missing contexts file, and the move of a
//! repository's `threads.db` memories into `contexts.db` on the next memory command, interrupted or
//! not. Memories that a repository kept in `threads.db` before K1b are written there with the store's
//! own API, since the engine no longer writes them. Each engine uses a `contexts.db` of its own
//! temporary directory and keeps the global and system Git configuration out.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::contexts::repo_id;
use gm_core::contexts::store::ContextStore;
use gm_core::git::SubprocessGit;
use gm_core::threads::memories::NewMemory;
use gm_core::threads::store::ThreadStore;
use gm_schema::threads::{
    Memory, MemoryBasis, MemoryKind, MemoryMark, MemorySource, MemorySourceInput, MemorySourceKind,
    MemoryStatus,
};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A temporary directory with one repository `api` on `main` that has a file, and a `contexts.db`
/// beside it that does not exist yet.
struct World {
    _base: tempfile::TempDir,
    root: PathBuf,
    db: PathBuf,
}

impl World {
    fn new() -> Self {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("api");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("f.txt"), "one\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "init"]);
        let db = base.path().join("contexts.db");
        Self {
            _base: base,
            root,
            db,
        }
    }
}

fn engine_on(root: &Path, db: Option<&Path>) -> Engine {
    let git = SubprocessGit {
        log: Some(CommandLog::default()),
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    engine.set_contexts_db(db.map(Path::to_path_buf));
    let events = send(
        &mut engine,
        &Actor::Human,
        CommandKind::OpenRepo {
            path: root.to_string_lossy().into_owned(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Done, "open: {events:?}");
    engine
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

fn agent() -> Actor {
    Actor::Agent {
        name: "claude-code".into(),
    }
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

fn failure(events: &[Event]) -> String {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::OperationChanged { detail, .. } => detail.clone(),
            _ => None,
        })
        .unwrap_or_default()
}

fn memory_of(events: &[Event]) -> Memory {
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoryChanged {
                memory: Some(memory),
                ..
            } => Some(memory.clone()),
            _ => None,
        })
        .expect("a memory")
}

fn file(path: &str) -> Vec<MemorySourceInput> {
    vec![MemorySourceInput {
        kind: MemorySourceKind::File,
        reference: path.into(),
        entry_from: None,
        entry_to: None,
    }]
}

/// An agent's proposal of a rule with a file source, in `context` (`None` for the repository's own).
fn propose(engine: &mut Engine, text: &str, context: Option<&str>) -> Vec<Event> {
    send(
        engine,
        &agent(),
        CommandKind::ProposeMemory {
            kind: MemoryKind::Rule,
            text: text.into(),
            sources: file("f.txt"),
            context: context.map(str::to_owned),
            basis: MemoryBasis::Observed,
            member: None,
        },
    )
}

fn create_context(engine: &mut Engine, name: &str) -> i64 {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::CreateContext {
            name: name.into(),
            parent: None,
        },
    );
    assert_eq!(
        state_of(&events),
        OperationState::Done,
        "create: {events:?}"
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::ContextsChanged {
                context: Some(context),
                ..
            } => Some(context.id),
            _ => None,
        })
        .expect("a context")
}

fn join(engine: &mut Engine, context: i64, root: &Path) {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::AddRepoToContext {
            context,
            path: root.to_string_lossy().into_owned(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Done, "join: {events:?}");
}

/// The contexts the repository is bound to, as `ListContexts` answers them.
fn bound_names(engine: &mut Engine) -> Vec<String> {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ListContexts {
            repo: true,
            all: false,
        },
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::ContextsListed { contexts, .. } => Some(
                contexts
                    .iter()
                    .map(|context| context.name.clone())
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .expect("the contexts")
}

/// `ListMemories` with every filter open, for a human: every status, every scope.
fn list(engine: &mut Engine) -> (Vec<Memory>, Option<String>) {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ListMemories {
            query: String::new(),
            kind: None,
            status: None,
            path: None,
            author: None,
            scope: None,
            limit: 100,
            cursor: None,
        },
    );
    assert_eq!(state_of(&events), OperationState::Done, "list: {events:?}");
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoriesListed { memories, note, .. } => Some((memories.clone(), note.clone())),
            _ => None,
        })
        .expect("the list")
}

fn read(engine: &mut Engine, uid: &str) -> (Memory, usize) {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ReadMemory { uid: uid.into() },
    );
    assert_eq!(state_of(&events), OperationState::Done, "read: {events:?}");
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoryRead {
                memory,
                mark_history,
                ..
            } => Some((memory.clone(), mark_history.len())),
            _ => None,
        })
        .expect("a memory read")
}

/// The repository id the clone at `root` holds, once it has one.
fn id_of(root: &Path) -> String {
    repo_id::read(&root.join(".git")).expect("the clone has an id")
}

/// The memories a store of `threads.db` holds for the clone at `root`, by uid, which must be found.
fn stored_in_threads(root: &Path, uid: &str) -> bool {
    let store = ThreadStore::open(&root.join(".git")).expect("the store opens");
    store.memory_id_of(uid).is_ok()
}

/// Copies a folder with everything in it, `.git` included.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

const NOW: i64 = 1_700_000_000_000;

/// Writes one memory of each kind into the `threads.db` of the clone at `root`, as a repository kept
/// them before K1b: an accepted rule, a rejected one, and an accepted one that supersedes an older
/// accepted one. Returns the uids in that order: accepted, rejected, old, new.
fn keep_in_threads(root: &Path) -> [String; 4] {
    let mut store = ThreadStore::open(&root.join(".git")).expect("the store opens");
    let source = MemorySource {
        kind: MemorySourceKind::File,
        reference: "f.txt".into(),
        repo_id: None,
        entry_from: None,
        entry_to: None,
        blob_oid: None,
        blob_read: false,
    };
    let sources = std::slice::from_ref(&source);
    let new = |text: &'static str, status: MemoryStatus, origin: &'static str| NewMemory {
        kind: MemoryKind::Rule,
        text,
        basis: MemoryBasis::Observed,
        status,
        origin,
        sources,
        at: NOW,
    };
    let (kept, _) = store
        .propose_memory(&new(
            "Keep the ledger append-only",
            MemoryStatus::Accepted,
            "human",
        ))
        .expect("accepted");
    let (rejected, _) = store
        .propose_memory(&new(
            "Use a global lock",
            MemoryStatus::Proposed,
            "agent:claude-code",
        ))
        .expect("proposed");
    store.reject_memory(rejected.id, NOW + 1).expect("rejected");
    let (old, _) = store
        .propose_memory(&new(
            "Return errors, never panic",
            MemoryStatus::Proposed,
            "agent:claude-code",
        ))
        .expect("old");
    store
        .accept_memory(old.id, None, &[], &[], NOW + 2)
        .expect("accept old");
    let (newer, _) = store
        .propose_memory(&new(
            "Return errors as Result",
            MemoryStatus::Proposed,
            "agent:claude-code",
        ))
        .expect("newer");
    store
        .accept_memory(newer.id, None, &[old.id], &[], NOW + 3)
        .expect("supersede");
    [kept.uid, rejected.uid, old.uid, newer.uid]
}

#[test]
fn an_agents_proposal_in_an_unbound_repository_creates_its_context_and_a_second_reuses_it() {
    let world = World::new();
    let mut engine = engine_on(&world.root, Some(&world.db));

    let first = memory_of(&send(
        &mut engine,
        &agent(),
        CommandKind::ProposeMemory {
            kind: MemoryKind::Rule,
            text: "Run the release checks".into(),
            sources: file("f.txt"),
            context: None,
            basis: MemoryBasis::Observed,
            member: None,
        },
    ));
    assert_eq!(
        first.scope, "api",
        "the memory is labelled by its repository's context"
    );
    assert_eq!(first.status, MemoryStatus::Proposed);
    assert_eq!(
        first.sources[0].repo_id.as_deref(),
        Some(id_of(&world.root).as_str())
    );
    assert_eq!(bound_names(&mut engine), ["api"]);
    assert!(!stored_in_threads(&world.root, &first.uid));

    let second = memory_of(&propose(&mut engine, "Name the branch in the PR", None));
    assert_eq!(second.scope, "api");
    assert_eq!(
        bound_names(&mut engine),
        ["api"],
        "the same context, not a second one"
    );

    let mut other = engine_on(&world.root, Some(&world.db));
    let (listed, note) = list(&mut other);
    assert_eq!(note, None);
    assert_eq!(listed.len(), 2);
}

#[test]
fn a_proposal_in_a_repository_bound_to_one_context_goes_there() {
    let world = World::new();
    let mut engine = engine_on(&world.root, Some(&world.db));
    let platform = create_context(&mut engine, "Platform");
    join(&mut engine, platform, &world.root);

    let proposed = memory_of(&propose(&mut engine, "Retry the upload twice", None));
    assert_eq!(proposed.scope, "Platform");
    assert_eq!(
        bound_names(&mut engine),
        ["Platform"],
        "no context of its own is made"
    );
}

#[test]
fn a_proposal_in_a_repository_bound_to_several_is_refused_and_names_them() {
    let world = World::new();
    let mut engine = engine_on(&world.root, Some(&world.db));
    let beta = create_context(&mut engine, "Beta");
    let alpha = create_context(&mut engine, "Alpha");
    join(&mut engine, beta, &world.root);
    join(&mut engine, alpha, &world.root);

    let refused = propose(&mut engine, "Name the branch in the PR", None);
    assert_eq!(state_of(&refused), OperationState::Failed);
    assert_eq!(
        failure(&refused),
        "this repository is bound to several contexts: \"Alpha\", \"Beta\"; name one with context"
    );
    let (listed, _) = list(&mut engine);
    assert!(listed.is_empty(), "nothing is written");

    let named = propose(&mut engine, "Name the branch in the PR", Some("Beta"));
    assert_eq!(state_of(&named), OperationState::Done, "{named:?}");
    assert_eq!(memory_of(&named).scope, "Beta");
}

#[test]
fn a_proposal_with_no_contexts_file_is_refused_and_nothing_is_made() {
    let world = World::new();
    let mut engine = engine_on(&world.root, None);
    let refused = propose(&mut engine, "Name the branch in the PR", None);
    assert_eq!(state_of(&refused), OperationState::Failed);
    assert_eq!(
        failure(&refused),
        "no contexts database is configured, so the memory has nowhere to go"
    );
    assert!(!world.db.exists(), "no contexts file is made by a refusal");
}

#[test]
fn a_clone_whose_id_clashes_is_refused_with_the_clash_note() {
    let world = World::new();
    let mut engine = engine_on(&world.root, Some(&world.db));
    let platform = create_context(&mut engine, "Platform");
    join(&mut engine, platform, &world.root);

    let copy = world.root.parent().unwrap().join("api-copy");
    copy_dir(&world.root, &copy);
    let mut clone = engine_on(&copy, Some(&world.db));
    let refused = propose(&mut clone, "Name the branch in the PR", None);
    assert_eq!(state_of(&refused), OperationState::Failed);
    assert!(
        failure(&refused).contains("shares its repository id with the clone at"),
        "{}",
        failure(&refused)
    );
}

#[test]
fn a_repositorys_threads_memories_move_into_its_context_on_the_next_command_with_their_history() {
    let world = World::new();
    let [kept, rejected, old, newer] = keep_in_threads(&world.root);
    // Marks and retrievals are counted by the engine while no contexts file is set: they are kept
    // in `threads.db` with the memories, which is where they were before K1b.
    let mut before = engine_on(&world.root, None);
    let marked = send(
        &mut before,
        &agent(),
        CommandKind::MarkMemory {
            uid: kept.clone(),
            mark: MemoryMark::Useful,
            reason: None,
        },
    );
    assert_eq!(state_of(&marked), OperationState::Done, "{marked:?}");
    send(
        &mut before,
        &Actor::Human,
        CommandKind::MarkMemory {
            uid: kept.clone(),
            mark: MemoryMark::Wrong,
            reason: Some("the lock is not needed".into()),
        },
    );
    for _ in 0..2 {
        let counted = send(
            &mut before,
            &agent(),
            CommandKind::RecordMemoryRetrieval {
                uids: vec![kept.clone()],
            },
        );
        assert_eq!(state_of(&counted), OperationState::Done);
    }
    drop(before);
    assert!(stored_in_threads(&world.root, &kept));
    assert!(!world.db.exists(), "no contexts file before the move");

    // The first command with a contexts file moves everything: a list shows the repository's context.
    let mut engine = engine_on(&world.root, Some(&world.db));
    let (listed, note) = list(&mut engine);
    assert_eq!(note, None, "the move did not fail");
    assert_eq!(listed.len(), 4);
    for memory in &listed {
        assert_eq!(memory.scope, "api", "{}", memory.text);
        assert!(
            !stored_in_threads(&world.root, &memory.uid),
            "threads.db is empty of it"
        );
    }
    let uids: Vec<&str> = listed.iter().map(|memory| memory.uid.as_str()).collect();
    for uid in [&kept, &rejected, &old, &newer] {
        assert!(uids.contains(&uid.as_str()), "{uid} is listed");
    }

    let (kept_memory, history) = read(&mut engine, &kept);
    assert_eq!(kept_memory.status, MemoryStatus::Accepted);
    assert_eq!(kept_memory.marks.len(), 2, "the latest mark of each actor");
    assert_eq!(
        history, 2,
        "the whole history moved: the agent's mark and the person's"
    );
    assert_eq!(kept_memory.retrieval_count, 2);
    assert!(kept_memory.last_retrieved_at.is_some());
    assert_eq!(
        kept_memory.sources[0].repo_id.as_deref(),
        Some(id_of(&world.root).as_str()),
        "a source names its repository"
    );
    assert_eq!(
        read(&mut engine, &rejected).0.status,
        MemoryStatus::Rejected
    );
    let (old_memory, _) = read(&mut engine, &old);
    assert_eq!(old_memory.status, MemoryStatus::Superseded);
    assert_eq!(old_memory.superseded_by, std::slice::from_ref(&newer));
    let (new_memory, _) = read(&mut engine, &newer);
    assert_eq!(new_memory.supersedes, std::slice::from_ref(&old));
    assert_eq!(bound_names(&mut engine), ["api"]);

    // The next command finds nothing left to move, and the list is the same.
    let (again, _) = list(&mut engine);
    assert_eq!(again.len(), 4);
}

/// A second repository `name` beside the world's `api`, on `main`, with one commit.
fn second_repo(world: &World, name: &str) -> PathBuf {
    let root = world.root.parent().unwrap().join(name);
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("f.txt"), "one\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "init"]);
    root
}

#[test]
fn a_repositorys_threads_memories_move_to_its_own_context_when_it_is_bound_to_a_shared_one() {
    let world = World::new();
    let mut setup = engine_on(&world.root, Some(&world.db));
    let platform = create_context(&mut setup, "Platform");
    join(&mut setup, platform, &world.root);
    drop(setup);
    let billing_root = second_repo(&world, "billing");
    let mut billing = engine_on(&billing_root, Some(&world.db));
    join(&mut billing, platform, &billing_root);
    drop(billing);
    let uids = keep_in_threads(&world.root);

    // The move goes to the repository's own context, `api`, bound beside the shared `Platform`; the
    // memories are not put in `Platform`, which the other repository sees.
    let mut engine = engine_on(&world.root, Some(&world.db));
    let (listed, note) = list(&mut engine);
    assert_eq!(note, None, "the move did not fail");
    assert_eq!(listed.len(), 4);
    for memory in &listed {
        assert_eq!(memory.scope, "api", "{}", memory.text);
        assert!(!stored_in_threads(&world.root, &memory.uid));
    }
    for uid in &uids {
        assert!(listed.iter().any(|memory| &memory.uid == uid), "{uid}");
    }
    let mut bound = bound_names(&mut engine);
    bound.sort();
    assert_eq!(bound, ["Platform", "api"], "the own context is bound too");

    // From the other repository, bound only to `Platform`, none of the memories is listed.
    let mut other = engine_on(&billing_root, Some(&world.db));
    let (seen, note) = list(&mut other);
    assert_eq!(note, None);
    assert!(
        seen.is_empty(),
        "a private memory of api is not visible from billing: {seen:?}"
    );
    assert_eq!(bound_names(&mut other), ["Platform"]);
}

#[test]
fn a_move_finished_after_an_interrupted_insert_duplicates_nothing() {
    let world = World::new();
    let uids = keep_in_threads(&world.root);
    let id = id_of_or_new(&world.root);

    // The crash: every memory is inserted into contexts.db, and threads.db keeps them all.
    {
        let source = ThreadStore::open(&world.root.join(".git")).expect("the store opens");
        let mut contexts = ContextStore::open(&world.db).expect("the contexts file");
        let context = contexts
            .repository_context(&id, &world.root.join(".git").to_string_lossy(), "api", NOW)
            .expect("the repository's context");
        for uid in &uids {
            let memory = source.read_memory_by_uid(uid).expect("read");
            let mut stored = memory.clone();
            for source in &mut stored.sources {
                source.repo_id.get_or_insert_with(|| id.clone());
            }
            assert!(
                contexts
                    .import_memory(&stored, &memory.marks, context)
                    .expect("import"),
                "the first insert is new"
            );
        }
    }
    assert!(
        stored_in_threads(&world.root, &uids[0]),
        "threads.db is intact"
    );

    // The next command finds the uids in contexts.db and only deletes them.
    let mut engine = engine_on(&world.root, Some(&world.db));
    let (listed, note) = list(&mut engine);
    assert_eq!(note, None);
    assert_eq!(listed.len(), 4, "one copy of each");
    for uid in &uids {
        assert!(
            !stored_in_threads(&world.root, uid),
            "{uid} left threads.db"
        );
    }
    let (newer, _) = read(&mut engine, &uids[3]);
    assert_eq!(
        newer.supersedes,
        std::slice::from_ref(&uids[2]),
        "the link came once"
    );
}

#[test]
fn a_list_in_a_repository_with_no_memories_creates_no_contexts_file() {
    let world = World::new();
    let mut engine = engine_on(&world.root, Some(&world.db));
    let (listed, note) = list(&mut engine);
    assert!(listed.is_empty());
    assert_eq!(note, None);
    assert!(
        !world.db.exists(),
        "a read creates no contexts file (D193 (2))"
    );
}

/// The repository id of the clone at `root`, written if it has none yet.
fn id_of_or_new(root: &Path) -> String {
    repo_id::ensure(&root.join(".git")).expect("the id")
}
