//! Contexts (D191, D193) against real repositories in one temporary directory: the list and its
//! visibility, the human-only writes, the repository identity (a moved clone, a copied clone that
//! clashes, `RenewRepoId`), two engines on one `contexts.db`, and memories addressed by uid. Each
//! engine keeps the global and system Git configuration out and uses a `contexts.db` of its own
//! temporary directory. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::threads::MemoryBasis;
use gm_schema::threads::{
    Context, FileChange, FileChangeState, Memory, MemoryKind, MemorySourceInput, MemorySourceKind,
    MemoryStatus,
};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

/// The repositories of the fixture, each one folder under the repositories directory.
const REPOS: [&str; 9] = [
    "api", "web", "billing", "auth", "jobs", "infra", "docs", "portal", "sync",
];

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

/// The temporary directory of a test: the repositories, and a `contexts.db` in a data folder that
/// does not exist yet.
struct World {
    _base: tempfile::TempDir,
    repos: PathBuf,
    db: PathBuf,
}

impl World {
    fn new() -> Self {
        let base = tempfile::tempdir().unwrap();
        let repos = base.path().join("repos");
        std::fs::create_dir(&repos).unwrap();
        for name in REPOS {
            let root = repos.join(name);
            std::fs::create_dir(&root).unwrap();
            git(&root, &["init", "-q", "-b", "main"]);
            git(&root, &["commit", "-q", "--allow-empty", "-m", "init"]);
        }
        let db = base.path().join("data").join("contexts.db");
        Self {
            _base: base,
            repos,
            db,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.repos.join(name)
    }
}

fn engine_on(root: &Path, db: &Path) -> Engine {
    let git = SubprocessGit {
        log: Some(CommandLog::default()),
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    engine.set_contexts_db(Some(db.to_path_buf()));
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

fn failure(events: &[Event]) -> Option<String> {
    events.iter().rev().find_map(|event| match event {
        Event::OperationChanged { detail, .. } => detail.clone(),
        _ => None,
    })
}

/// What a context list answered: the contexts, the repository id and the clash, if any.
struct Listed {
    contexts: Vec<Context>,
    repo_id: Option<String>,
    clash: Option<String>,
}

fn list(engine: &mut Engine, actor: &Actor, repo: bool) -> Listed {
    let events = send(engine, actor, CommandKind::ListContexts { repo });
    assert_eq!(state_of(&events), OperationState::Done, "list: {events:?}");
    events
        .iter()
        .find_map(|event| match event {
            Event::ContextsListed {
                contexts,
                repo_id,
                clash,
                ..
            } => Some(Listed {
                contexts: contexts.clone(),
                repo_id: repo_id.clone(),
                clash: clash.clone(),
            }),
            _ => None,
        })
        .expect("contexts listed")
}

/// The context a write answered, and the memories a delete removed.
fn changed(events: &[Event]) -> (Option<Context>, u32) {
    events
        .iter()
        .find_map(|event| match event {
            Event::ContextsChanged {
                context,
                deleted_memories,
                ..
            } => Some((context.clone(), *deleted_memories)),
            _ => None,
        })
        .expect("a context change")
}

/// Creates a context as the human and returns it; the write must succeed.
fn create(engine: &mut Engine, name: &str, parent: Option<i64>) -> Context {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::CreateContext {
            name: name.to_owned(),
            parent,
        },
    );
    assert_eq!(
        state_of(&events),
        OperationState::Done,
        "create: {events:?}"
    );
    changed(&events).0.expect("the context")
}

/// Adds the repository at `path` to context `context` as the human.
fn join(engine: &mut Engine, context: i64, path: &Path) {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::AddRepoToContext {
            context,
            path: path.to_string_lossy().into_owned(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Done, "join: {events:?}");
}

/// The contexts of the fixture, as the author describes them (design/threads.md): Platform with
/// seven repositories, Billing under it with three, and Partners with five. Returns their ids.
struct Fixture {
    platform: i64,
    billing: i64,
    partners: i64,
}

fn populate(world: &World) -> Fixture {
    let mut engine = engine_on(&world.path("api"), &world.db);
    let platform = create(&mut engine, "Platform", None).id;
    let billing = create(&mut engine, "Billing", Some(platform)).id;
    let partners = create(&mut engine, "Partners", None).id;
    for name in ["api", "web", "billing", "auth", "jobs", "infra", "docs"] {
        join(&mut engine, platform, &world.path(name));
    }
    for name in ["billing", "api", "jobs"] {
        join(&mut engine, billing, &world.path(name));
    }
    for name in ["portal", "api", "sync", "auth", "docs"] {
        join(&mut engine, partners, &world.path(name));
    }
    Fixture {
        platform,
        billing,
        partners,
    }
}

/// The paths of the contexts a repository sees, sorted, for a set comparison.
fn paths_seen(listed: &Listed) -> Vec<String> {
    let mut paths: Vec<String> = listed.contexts.iter().map(|c| c.path.clone()).collect();
    paths.sort();
    paths
}

fn is_hex32(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The repository id a clone's `.git/gitmaster/repo-id` holds.
fn repo_id_file(root: &Path) -> String {
    std::fs::read_to_string(root.join(".git").join("gitmaster").join("repo-id"))
        .expect("a repo-id file")
        .trim()
        .to_owned()
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

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

#[test]
fn with_no_contexts_file_the_list_is_empty_and_no_file_is_made() {
    let world = World::new();
    let mut engine = engine_on(&world.path("api"), &world.db);

    let all = list(&mut engine, &Actor::Human, false);
    assert!(all.contexts.is_empty());
    let mine = list(&mut engine, &agent(), true);
    assert!(mine.contexts.is_empty());
    assert_eq!(mine.repo_id, None);
    assert_eq!(mine.clash, None);
    assert!(!world.db.exists(), "a read creates no file");
    assert!(!world.db.parent().unwrap().exists(), "not even its folder");
}

#[test]
fn a_repository_sees_the_contexts_it_belongs_to_and_their_ancestors() {
    let world = World::new();
    let fixture = populate(&world);

    let mut api = engine_on(&world.path("api"), &world.db);
    let seen = list(&mut api, &Actor::Human, true);
    assert_eq!(
        paths_seen(&seen),
        vec!["Partners", "Platform", "Platform / Billing"]
    );
    assert!(seen.repo_id.is_some());
    assert_eq!(seen.clash, None);
    // Nearest first: the direct memberships, then the parent of Billing.
    let ids: Vec<i64> = seen.contexts.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&fixture.billing));

    let mut web = engine_on(&world.path("web"), &world.db);
    assert_eq!(
        paths_seen(&list(&mut web, &Actor::Human, true)),
        vec!["Platform"]
    );

    let mut portal = engine_on(&world.path("portal"), &world.db);
    assert_eq!(
        paths_seen(&list(&mut portal, &Actor::Human, true)),
        vec!["Partners"]
    );

    let all = list(&mut portal, &Actor::Human, false);
    assert_eq!(all.contexts.len(), 3, "every context, with repo false");
    let platform = all
        .contexts
        .iter()
        .find(|c| c.id == fixture.platform)
        .expect("Platform");
    assert_eq!(platform.repos.len(), 7);
    assert_eq!(platform.sub_contexts, 1);
}

#[test]
fn joining_writes_a_private_repo_id_and_leaves_the_working_tree_clean() {
    let world = World::new();
    let mut engine = engine_on(&world.path("api"), &world.db);
    let first = create(&mut engine, "Platform", None).id;
    let second = create(&mut engine, "Billing", None).id;

    join(&mut engine, first, &world.path("api"));
    let id = repo_id_file(&world.path("api"));
    assert!(is_hex32(&id), "{id}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(world.path("api").join(".git/gitmaster/repo-id"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert_eq!(git(&world.path("api"), &["status", "--porcelain"]), "");

    join(&mut engine, second, &world.path("api"));
    assert_eq!(repo_id_file(&world.path("api")), id, "the same id");
    assert_eq!(git(&world.path("api"), &["status", "--porcelain"]), "");
    let seen = list(&mut engine, &Actor::Human, true);
    assert_eq!(paths_seen(&seen), vec!["Billing", "Platform"]);
}

#[test]
fn an_agent_is_refused_every_context_write_and_can_read() {
    let world = World::new();
    let fixture = populate(&world);
    let mut engine = engine_on(&world.path("api"), &world.db);
    let before = list(&mut engine, &Actor::Human, false).contexts.len();

    let refused = [
        CommandKind::CreateContext {
            name: "Agents".into(),
            parent: None,
        },
        CommandKind::UpdateContext {
            id: fixture.partners,
            name: Some("Renamed".into()),
            parent: None,
        },
        CommandKind::DeleteContext {
            id: fixture.partners,
            delete_memories: true,
        },
        CommandKind::AddRepoToContext {
            context: fixture.partners,
            path: world.path("web").to_string_lossy().into_owned(),
        },
        CommandKind::RemoveRepoFromContext {
            context: fixture.partners,
            repo_id: "0123456789abcdef0123456789abcdef".into(),
        },
        CommandKind::RenewRepoId,
    ];
    for request in refused {
        let events = send(&mut engine, &agent(), request.clone());
        assert_eq!(state_of(&events), OperationState::Failed, "{request:?}");
        let message = failure(&events).expect("a reason");
        assert!(message.starts_with("only a human can"), "{message}");
    }
    assert_eq!(
        list(&mut engine, &Actor::Human, false).contexts.len(),
        before
    );
    assert_eq!(list(&mut engine, &agent(), true).contexts.len(), 3);
}

#[test]
fn update_refuses_a_cycle_and_a_duplicate_sibling_and_delete_refuses_sub_contexts() {
    let world = World::new();
    let mut engine = engine_on(&world.path("api"), &world.db);
    let platform = create(&mut engine, "Platform", None);
    let billing = create(&mut engine, "Billing", Some(platform.id));
    let partners = create(&mut engine, "Partners", None);

    let cycle = send(
        &mut engine,
        &Actor::Human,
        CommandKind::UpdateContext {
            id: platform.id,
            name: None,
            parent: Some(Some(billing.id)),
        },
    );
    assert_eq!(state_of(&cycle), OperationState::Failed);
    assert!(
        failure(&cycle).is_some_and(|message| message.contains("under itself")),
        "{cycle:?}"
    );

    let duplicate = send(
        &mut engine,
        &Actor::Human,
        CommandKind::UpdateContext {
            id: partners.id,
            name: Some("Platform".into()),
            parent: None,
        },
    );
    assert_eq!(state_of(&duplicate), OperationState::Failed);
    assert!(
        failure(&duplicate).is_some_and(|message| message.contains("already exists")),
        "{duplicate:?}"
    );
    let again = send(
        &mut engine,
        &Actor::Human,
        CommandKind::CreateContext {
            name: "Billing".into(),
            parent: Some(platform.id),
        },
    );
    assert_eq!(state_of(&again), OperationState::Failed, "a sibling name");

    let parked = send(
        &mut engine,
        &Actor::Human,
        CommandKind::DeleteContext {
            id: platform.id,
            delete_memories: false,
        },
    );
    assert_eq!(state_of(&parked), OperationState::Failed);
    assert!(
        failure(&parked).is_some_and(|message| message.contains("sub-contexts")),
        "{parked:?}"
    );

    // A move to the root is spelled with a null parent, and it is applied.
    let moved = send(
        &mut engine,
        &Actor::Human,
        CommandKind::UpdateContext {
            id: billing.id,
            name: None,
            parent: Some(None),
        },
    );
    assert_eq!(state_of(&moved), OperationState::Done);
    assert_eq!(changed(&moved).0.expect("moved").path, "Billing");
}

#[test]
fn a_moved_clone_keeps_its_contexts_and_its_stored_path_follows_it() {
    let world = World::new();
    populate(&world);
    let old = world.path("api");
    let new = world.repos.join("api-moved");
    std::fs::rename(&old, &new).unwrap();

    let mut engine = engine_on(&new, &world.db);
    let seen = list(&mut engine, &Actor::Human, true);
    assert_eq!(
        paths_seen(&seen),
        vec!["Partners", "Platform", "Platform / Billing"]
    );
    assert_eq!(seen.clash, None);
    let stored = seen
        .contexts
        .iter()
        .flat_map(|context| context.repos.iter())
        .filter(|repo| repo.name == "api")
        .map(|repo| canonical(Path::new(&repo.path)))
        .collect::<Vec<_>>();
    assert_eq!(stored.len(), 3, "the path is set in each context");
    assert!(
        stored
            .iter()
            .all(|path| *path == canonical(&new.join(".git")))
    );
}

#[test]
fn a_copied_clone_clashes_and_renewing_it_leaves_the_original_in_its_contexts() {
    let world = World::new();
    populate(&world);
    let original = world.path("api");
    let copy = world.repos.join("api-copy");
    copy_dir(&original, &copy);
    let original_id = repo_id_file(&original);

    let mut clone = engine_on(&copy, &world.db);
    let clashed = list(&mut clone, &Actor::Human, true);
    assert!(clashed.contexts.is_empty(), "no contexts from a clash");
    assert_eq!(clashed.repo_id.as_deref(), Some(original_id.as_str()));
    let other = clashed.clash.expect("the clash names the other clone");
    assert_eq!(
        canonical(Path::new(&other)),
        canonical(&original.join(".git"))
    );

    let renewed = send(&mut clone, &Actor::Human, CommandKind::RenewRepoId);
    assert_eq!(state_of(&renewed), OperationState::Done);
    assert_eq!(changed(&renewed).0, None);
    let new_id = repo_id_file(&copy);
    assert!(is_hex32(&new_id));
    assert_ne!(new_id, original_id);
    let after = list(&mut clone, &Actor::Human, true);
    assert!(after.contexts.is_empty(), "the copy is in no context");
    assert_eq!(after.clash, None);
    assert_eq!(after.repo_id.as_deref(), Some(new_id.as_str()));

    assert_eq!(
        repo_id_file(&original),
        original_id,
        "the original keeps its id"
    );
    let mut original_engine = engine_on(&original, &world.db);
    assert_eq!(
        paths_seen(&list(&mut original_engine, &Actor::Human, true)),
        vec!["Partners", "Platform", "Platform / Billing"]
    );
}

#[test]
fn renewing_a_recorded_clone_gives_up_its_memberships() {
    let world = World::new();
    populate(&world);
    let original = world.path("api");
    let old_id = repo_id_file(&original);
    let mut engine = engine_on(&original, &world.db);
    let renewed = send(&mut engine, &Actor::Human, CommandKind::RenewRepoId);
    assert_eq!(state_of(&renewed), OperationState::Done);
    let after = list(&mut engine, &Actor::Human, true);
    assert!(after.contexts.is_empty(), "the new id is in no context");
    assert_eq!(after.repo_id, Some(repo_id_file(&original)));
    assert_ne!(repo_id_file(&original), old_id, "a new id");
    assert_eq!(
        paths_seen(&list(&mut engine, &Actor::Human, true)),
        Vec::<String>::new()
    );
}

#[test]
fn two_engines_on_one_contexts_file_both_write_without_error() {
    let world = World::new();
    let mut first = engine_on(&world.path("api"), &world.db);
    let mut second = engine_on(&world.path("web"), &world.db);

    create(&mut first, "Alpha", None);
    create(&mut second, "Beta", None);
    let mut names: Vec<String> = list(&mut first, &Actor::Human, false)
        .contexts
        .into_iter()
        .map(|c| c.name)
        .collect();
    names.sort();
    assert_eq!(names, vec!["Alpha", "Beta"]);
}

#[test]
fn a_memory_is_proposed_accepted_and_read_by_its_uid() {
    let world = World::new();
    let mut engine = engine_on(&world.path("api"), &world.db);

    let events = send(
        &mut engine,
        &agent(),
        CommandKind::ProposeMemory {
            kind: MemoryKind::Rule,
            text: "Run the release checks".into(),
            sources: vec![MemorySourceInput {
                kind: MemorySourceKind::File,
                reference: "src/lib.rs".into(),
                entry_from: None,
                entry_to: None,
            }],
            context: None,
            basis: MemoryBasis::Observed,
            member: None,
        },
    );
    let proposed = memory_of(&events);
    assert_eq!(proposed.uid.len(), 16);

    let accepted = send(
        &mut engine,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid: proposed.uid.clone(),
            text: None,
            supersedes: Vec::new(),
        },
    );
    let accepted = memory_of(&accepted);
    assert_eq!(accepted.uid, proposed.uid);
    assert_eq!(accepted.status, gm_schema::threads::MemoryStatus::Accepted);

    let read = send(
        &mut engine,
        &Actor::Human,
        CommandKind::ReadMemory {
            uid: proposed.uid.clone(),
        },
    );
    assert!(read.iter().any(|event| matches!(
        event,
        Event::MemoryRead { memory, .. } if memory.uid == proposed.uid
    )));

    let unknown = send(
        &mut engine,
        &Actor::Human,
        CommandKind::ReadMemory {
            uid: "0123456789abcdef".into(),
        },
    );
    assert_eq!(state_of(&unknown), OperationState::Failed);
    assert_eq!(failure(&unknown).as_deref(), Some("Memory not found."));

    let stray = send(
        &mut engine,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid: proposed.uid.clone(),
            text: None,
            supersedes: vec!["0123456789abcdef".into()],
        },
    );
    assert_eq!(state_of(&stray), OperationState::Failed);
    assert!(
        failure(&stray).is_some_and(|message| message.contains("0123456789abcdef")),
        "{stray:?}"
    );
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

#[test]
fn context_reads_are_not_logged_and_writes_are_with_their_actor() {
    let world = World::new();
    let mut engine = engine_on(&world.path("api"), &world.db);
    list(&mut engine, &agent(), true);
    list(&mut engine, &Actor::Human, false);
    create(&mut engine, "Platform", None);

    let log = engine.log();
    assert!(
        log.iter()
            .all(|entry| !matches!(entry.kind, CommandKind::ListContexts { .. })),
        "reads are not in the command log"
    );
    assert!(log.iter().any(|entry| {
        entry.actor == Actor::Human && matches!(entry.kind, CommandKind::CreateContext { .. })
    }));
}

/// What a `ListMemories` answered.
#[derive(Debug)]
struct Seen {
    memories: Vec<Memory>,
    next: Option<String>,
    contexts_read: bool,
    note: Option<String>,
    proposed_total: u32,
}

/// Lists the memories `actor` sees from the open repository in `scope`, one page at a time. A
/// refused request answers its reason.
fn search(
    engine: &mut Engine,
    actor: &Actor,
    query: &str,
    scope: Option<&str>,
    limit: u32,
    cursor: Option<&str>,
) -> Result<Seen, String> {
    let events = send(
        engine,
        actor,
        CommandKind::ListMemories {
            query: query.to_owned(),
            kind: None,
            status: None,
            path: None,
            author: None,
            scope: scope.map(str::to_owned),
            limit,
            cursor: cursor.map(str::to_owned),
        },
    );
    if state_of(&events) == OperationState::Failed {
        return Err(failure(&events).unwrap_or_default());
    }
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoriesListed {
                memories,
                next_cursor,
                contexts_read,
                note,
                proposed_total,
                ..
            } => Some(Seen {
                memories: memories.clone(),
                next: next_cursor.clone(),
                contexts_read: *contexts_read,
                note: note.clone(),
                proposed_total: *proposed_total,
            }),
            _ => None,
        })
        .ok_or_else(|| "no listing".to_owned())
}

/// The texts of memories, sorted, for a comparison.
fn texts(memories: &[Memory]) -> Vec<String> {
    let mut texts: Vec<String> = memories.iter().map(|m| m.text.clone()).collect();
    texts.sort();
    texts
}

/// Proposes a rule with a source as `actor`, at `context` when one is named (D193 (7)).
fn propose_at(engine: &mut Engine, actor: &Actor, context: Option<&str>, text: &str) -> Vec<Event> {
    send(
        engine,
        actor,
        CommandKind::ProposeMemory {
            kind: MemoryKind::Rule,
            text: text.to_owned(),
            sources: vec![MemorySourceInput {
                kind: MemorySourceKind::File,
                reference: "src/lib.rs".into(),
                entry_from: None,
                entry_to: None,
            }],
            context: context.map(str::to_owned),
            basis: MemoryBasis::Observed,
            member: None,
        },
    )
}

/// Accepts memory `uid` as the human from the open repository.
fn accept_as_human(engine: &mut Engine, uid: &str) -> Vec<Event> {
    send(
        engine,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid: uid.to_owned(),
            text: None,
            supersedes: Vec::new(),
        },
    )
}

/// Reads memory `uid` from the open repository.
fn read_memory(engine: &mut Engine, uid: &str) -> Memory {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ReadMemory {
            uid: uid.to_owned(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Done, "read: {events:?}");
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoryRead { memory, .. } => Some(memory.clone()),
            _ => None,
        })
        .expect("a memory read")
}

fn write_file(root: &Path, path: &str, text: &str) {
    if let Some(parent) = root.join(path).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(root.join(path), text).unwrap();
}

/// Commits every change of the working tree under `message`.
fn commit_all(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
}

#[test]
fn a_memory_in_a_context_is_found_from_every_repository_that_sees_the_context() {
    let world = World::new();
    populate(&world);
    let mut web = engine_on(&world.path("web"), &world.db);
    let stored = memory_of(&propose_at(
        &mut web,
        &Actor::Human,
        Some("Platform"),
        "Platform rule",
    ));
    assert_eq!(stored.scope, "Platform");

    for name in ["api", "web", "billing", "auth", "jobs", "infra", "docs"] {
        let mut engine = engine_on(&world.path(name), &world.db);
        let found = search(&mut engine, &agent(), "", None, 50, None).expect("list");
        assert_eq!(texts(&found.memories), ["Platform rule"], "{name}");
    }
    for name in ["portal", "sync"] {
        let mut engine = engine_on(&world.path(name), &world.db);
        let found = search(&mut engine, &agent(), "", None, 50, None).expect("list");
        assert!(found.memories.is_empty(), "{name} does not see Platform");
    }
}

#[test]
fn a_memory_in_billing_is_found_from_billing_and_its_repositories_and_not_from_the_rest_of_platform()
 {
    let world = World::new();
    populate(&world);
    let mut billing = engine_on(&world.path("billing"), &world.db);
    assert_eq!(
        state_of(&propose_at(
            &mut billing,
            &Actor::Human,
            Some("Billing"),
            "Billing rule"
        )),
        OperationState::Done
    );
    for name in ["billing", "api", "jobs"] {
        let mut engine = engine_on(&world.path(name), &world.db);
        let found = search(&mut engine, &agent(), "", None, 50, None).expect("list");
        assert_eq!(texts(&found.memories), ["Billing rule"], "{name}");
    }
    for name in ["web", "auth", "infra", "docs", "portal", "sync"] {
        let mut engine = engine_on(&world.path(name), &world.db);
        let found = search(&mut engine, &agent(), "", None, 50, None).expect("list");
        assert!(found.memories.is_empty(), "{name} does not see Billing");
    }
}

#[test]
fn a_repository_in_three_contexts_sees_all_three_and_labels_each_result_with_its_scope() {
    let world = World::new();
    populate_with_scopes(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    let found = search(&mut api, &Actor::Human, "", None, 50, None)
        .expect("list")
        .memories;
    let mut labelled: Vec<(&str, &str)> = found
        .iter()
        .map(|memory| (memory.text.as_str(), memory.scope.as_str()))
        .collect();
    labelled.sort_unstable();
    assert_eq!(
        labelled,
        [
            ("Billing rule", "Platform / Billing"),
            ("Partners rule", "Partners"),
            ("Platform rule", "Platform"),
        ]
    );
}

#[test]
fn the_scope_filter_names_the_repository_or_one_visible_context_and_refuses_the_rest() {
    let world = World::new();
    let fixture = populate_with_scopes(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    let scoped = |api: &mut Engine, scope: &str| -> Vec<String> {
        let found = search(api, &Actor::Human, "", Some(scope), 50, None).expect(scope);
        texts(&found.memories)
    };
    assert_eq!(
        sorted(scoped(&mut api, "repository")),
        ["Billing rule", "Partners rule", "Platform rule"],
        "a repository's own scope is what it is bound to"
    );
    assert_eq!(
        sorted(scoped(&mut api, "Billing")),
        ["Billing rule", "Platform rule"],
        "a sub-context inherits the memories of its parent"
    );
    assert_eq!(
        sorted(scoped(&mut api, "Platform / Billing")),
        ["Billing rule", "Platform rule"]
    );
    assert_eq!(
        scoped(&mut api, "Platform"),
        ["Platform rule"],
        "a context does not take the memories of its sub-contexts"
    );
    assert_eq!(scoped(&mut api, "all").len(), 3);

    // A second Billing under Partners, joined by api, makes the bare name ambiguous.
    let second = create(&mut api, "Billing", Some(fixture.partners));
    join(&mut api, second.id, &world.path("api"));
    let ambiguous =
        search(&mut api, &Actor::Human, "", Some("Billing"), 50, None).expect_err("ambiguous");
    assert!(ambiguous.contains("several"), "{ambiguous}");
    assert_eq!(
        sorted(scoped(&mut api, "Platform / Billing")),
        ["Billing rule", "Platform rule"],
        "the path still names one"
    );

    // Ops is bound to portal alone, but api reads it by name (D222 (3)): a memory of Ops is listed
    // from api when it names Ops, and not in the default list of api.
    let ops = create(&mut api, "Ops", None);
    join(&mut api, ops.id, &world.path("portal"));
    let mut portal = engine_on(&world.path("portal"), &world.db);
    propose_at(&mut portal, &Actor::Human, Some("Ops"), "Ops rule");
    assert_eq!(scoped(&mut api, "Ops"), ["Ops rule"]);
    let unknown =
        search(&mut api, &Actor::Human, "", Some("Nowhere"), 50, None).expect_err("unknown");
    assert!(unknown.contains("No context named"), "{unknown}");
}

/// The texts of a list, sorted, for a comparison that does not depend on the order.
fn sorted(mut texts: Vec<String>) -> Vec<String> {
    texts.sort();
    texts
}

/// The fixture of `four_scopes`, with the memories of each scope in place, as its own function so
/// the scope tests can take the fixture ids.
fn populate_with_scopes(world: &World) -> Fixture {
    let fixture = populate(world);
    let mut api = engine_on(&world.path("api"), &world.db);
    for (context, text) in [
        ("Platform", "Platform rule"),
        ("Billing", "Billing rule"),
        ("Partners", "Partners rule"),
    ] {
        propose_at(&mut api, &Actor::Human, Some(context), text);
    }
    fixture
}

#[test]
fn an_equally_relevant_memory_in_two_bound_contexts_is_listed_with_each_scope() {
    let world = World::new();
    populate(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    // The same text in each context, and nothing else in either, so `bm25` is equal.
    propose_at(
        &mut api,
        &Actor::Human,
        Some("Platform"),
        "The widget cache is warm",
    );
    propose_at(
        &mut api,
        &Actor::Human,
        Some("Billing"),
        "The widget cache is warm",
    );
    let found = search(&mut api, &Actor::Human, "widget", None, 50, None)
        .expect("list")
        .memories;
    // Both contexts are bound to api, so neither is nearer: the order is by uid, and each is labelled.
    let mut scopes: Vec<&str> = found.iter().map(|memory| memory.scope.as_str()).collect();
    scopes.sort_unstable();
    assert_eq!(scopes, ["Platform", "Platform / Billing"]);
}

#[test]
fn pages_walk_the_merged_list_without_repeats_or_gaps_and_a_cursor_stays_with_its_scope() {
    let world = World::new();
    populate(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    propose_at(&mut api, &Actor::Human, Some("Partners"), "alpha note");
    propose_at(&mut api, &Actor::Human, Some("Platform"), "beta note");
    propose_at(&mut api, &Actor::Human, Some("Billing"), "gamma note");

    let whole: Vec<String> = search(&mut api, &Actor::Human, "note", None, 50, None)
        .expect("all")
        .memories
        .into_iter()
        .map(|memory| memory.text)
        .collect();
    assert_eq!(whole.len(), 3);

    let mut walked = Vec::new();
    let mut cursor: Option<String> = None;
    let mut first_cursor = None;
    loop {
        let page =
            search(&mut api, &Actor::Human, "note", None, 1, cursor.as_deref()).expect("page");
        assert_eq!(page.memories.len(), 1);
        walked.extend(page.memories.into_iter().map(|memory| memory.text));
        if first_cursor.is_none() {
            first_cursor.clone_from(&page.next);
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(
        walked, whole,
        "every memory once, in the order of the whole list"
    );

    let first_cursor = first_cursor.expect("a second page");
    let other_scope = search(
        &mut api,
        &Actor::Human,
        "note",
        Some("repository"),
        1,
        Some(&first_cursor),
    )
    .expect_err("a cursor of all is not a cursor of repository");
    assert!(other_scope.contains("another search"), "{other_scope}");
    let other_query = search(
        &mut api,
        &Actor::Human,
        "alpha",
        None,
        1,
        Some(&first_cursor),
    )
    .expect_err("a cursor of another query is refused");
    assert!(other_query.contains("another search"), "{other_query}");
}

#[test]
fn an_agent_proposes_in_a_context_it_belongs_to_and_a_human_makes_it_visible_to_the_others() {
    let world = World::new();
    populate(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    let mut portal = engine_on(&world.path("portal"), &world.db);

    let refused = propose_at(&mut portal, &agent(), Some("Platform"), "Portal rule");
    assert_eq!(state_of(&refused), OperationState::Failed);
    assert!(
        failure(&refused).is_some_and(|message| message.contains("Platform")),
        "{refused:?}"
    );

    let proposed = memory_of(&propose_at(
        &mut api,
        &agent(),
        Some("Platform"),
        "Shared proposal",
    ));
    assert_eq!(proposed.status, MemoryStatus::Proposed);
    assert_eq!(proposed.scope, "Platform");
    let mut web = engine_on(&world.path("web"), &world.db);
    assert!(
        search(&mut web, &agent(), "", None, 50, None)
            .expect("list")
            .memories
            .is_empty(),
        "a proposal is not seen by default"
    );
    assert_eq!(
        texts(
            &search(&mut web, &Actor::Human, "", None, 50, None)
                .expect("list")
                .memories
        ),
        ["Shared proposal"]
    );

    let mut billing = engine_on(&world.path("billing"), &world.db);
    let accepted = memory_of(&accept_as_human(&mut billing, &proposed.uid));
    assert_eq!(accepted.status, MemoryStatus::Accepted);
    assert_eq!(
        texts(
            &search(&mut api, &agent(), "", None, 50, None)
                .expect("list")
                .memories
        ),
        ["Shared proposal"],
        "the accepted memory is seen by agents of every repository in the context"
    );
}

#[test]
fn a_memory_attached_to_a_second_context_is_found_there_and_its_last_context_cannot_be_detached() {
    let world = World::new();
    let fixture = populate(&world);
    let mut billing = engine_on(&world.path("billing"), &world.db);
    let stored = memory_of(&propose_at(
        &mut billing,
        &Actor::Human,
        Some("Billing"),
        "Billing only rule",
    ));
    let mut api = engine_on(&world.path("api"), &world.db);
    let refused = send(
        &mut api,
        &agent(),
        CommandKind::AttachMemory {
            uid: stored.uid.clone(),
            context: fixture.partners,
        },
    );
    assert_eq!(state_of(&refused), OperationState::Failed);
    assert!(
        failure(&refused).is_some_and(|message| message.starts_with("only a human can")),
        "{refused:?}"
    );
    let events = send(
        &mut api,
        &Actor::Human,
        CommandKind::AttachMemory {
            uid: stored.uid.clone(),
            context: fixture.partners,
        },
    );
    assert_eq!(state_of(&events), OperationState::Done, "{events:?}");

    let mut portal = engine_on(&world.path("portal"), &world.db);
    assert_eq!(
        texts(
            &search(&mut portal, &agent(), "", None, 50, None)
                .expect("list")
                .memories
        ),
        ["Billing only rule"]
    );

    let detached = send(
        &mut api,
        &Actor::Human,
        CommandKind::DetachMemory {
            uid: stored.uid.clone(),
            context: fixture.partners,
        },
    );
    assert_eq!(state_of(&detached), OperationState::Done, "{detached:?}");
    assert!(
        search(&mut portal, &agent(), "", None, 50, None)
            .expect("list")
            .memories
            .is_empty(),
        "portal no longer sees it"
    );

    let last = send(
        &mut api,
        &Actor::Human,
        CommandKind::DetachMemory {
            uid: stored.uid.clone(),
            context: fixture.billing,
        },
    );
    assert_eq!(state_of(&last), OperationState::Failed);
    assert!(
        failure(&last).is_some_and(|message| message.contains("at least one context")),
        "{last:?}"
    );
}

#[test]
fn a_file_of_another_repository_is_read_at_its_own_head_and_is_unavailable_once_its_folder_is_gone()
{
    let world = World::new();
    populate(&world);
    let api_root = world.path("api");
    write_file(&api_root, "src/lib.rs", "fn one() {}\n");
    commit_all(&api_root, "add the lib");
    let mut api = engine_on(&api_root, &world.db);
    let proposed = memory_of(&propose_at(
        &mut api,
        &agent(),
        Some("Platform"),
        "Lib rule",
    ));

    let mut web = engine_on(&world.path("web"), &world.db);
    let accepted = accept_as_human(&mut web, &proposed.uid);
    assert_eq!(state_of(&accepted), OperationState::Done, "{accepted:?}");
    let unchanged = read_memory(&mut web, &proposed.uid);
    assert!(
        unchanged.files_changed_since.is_empty(),
        "api's HEAD is the one accepted: {:?}",
        unchanged.files_changed_since
    );

    write_file(&api_root, "src/lib.rs", "fn two() {}\n");
    commit_all(&api_root, "change the lib");
    let changed = read_memory(&mut web, &proposed.uid);
    assert_eq!(
        changed.files_changed_since,
        vec![FileChange {
            path: "src/lib.rs".into(),
            state: FileChangeState::Changed,
        }]
    );

    std::fs::remove_dir_all(&api_root).unwrap();
    let gone = read_memory(&mut web, &proposed.uid);
    assert_eq!(
        gone.files_changed_since,
        vec![FileChange {
            path: "src/lib.rs".into(),
            state: FileChangeState::Unavailable,
        }]
    );
}

#[test]
fn a_file_whose_repository_was_unknown_at_acceptance_stays_unavailable_when_it_becomes_known() {
    let world = World::new();
    let mut web = engine_on(&world.path("web"), &world.db);
    let platform = create(&mut web, "Platform", None).id;
    join(&mut web, platform, &world.path("web"));
    join(&mut web, platform, &world.path("portal"));
    let portal_root = world.path("portal");
    write_file(&portal_root, "src/lib.rs", "fn one() {}\n");
    commit_all(&portal_root, "add the lib");
    let mut portal = engine_on(&portal_root, &world.db);
    let proposed = memory_of(&propose_at(
        &mut portal,
        &agent(),
        Some("Platform"),
        "Portal rule",
    ));

    // Portal leaves every context, so web no longer knows its folder when the human accepts.
    let portal_id = repo_id_file(&portal_root);
    let left = send(
        &mut web,
        &Actor::Human,
        CommandKind::RemoveRepoFromContext {
            context: platform,
            repo_id: portal_id,
        },
    );
    assert_eq!(state_of(&left), OperationState::Done, "leave: {left:?}");
    let accepted = accept_as_human(&mut web, &proposed.uid);
    assert_eq!(state_of(&accepted), OperationState::Done, "{accepted:?}");
    let gone = read_memory(&mut web, &proposed.uid);
    assert!(!gone.sources[0].blob_read, "not read at acceptance");
    let want = vec![FileChange {
        path: "src/lib.rs".into(),
        state: FileChangeState::Unavailable,
    }];
    assert_eq!(gone.files_changed_since, want);

    // Portal joins a context web sees: its folder is known now, and the file is still unavailable,
    // never changed, because nothing was read when the memory was accepted.
    join(&mut web, platform, &portal_root);
    let known = read_memory(&mut web, &proposed.uid);
    assert_eq!(known.files_changed_since, want);
}

#[test]
fn an_unreadable_contexts_file_answers_nothing_and_says_why() {
    let world = World::new();
    populate_with_scopes(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    std::fs::write(&world.db, b"this is not a database, only junk bytes").unwrap();

    let listed = search(&mut api, &Actor::Human, "", None, 50, None).expect("still answers");
    assert!(listed.memories.is_empty(), "every memory is in contexts.db");
    assert!(!listed.contexts_read);
    assert!(
        listed
            .note
            .as_deref()
            .is_some_and(|note| note.contains("could not be read")),
        "{listed:?}"
    );
    let scoped = search(&mut api, &agent(), "", Some("Platform"), 50, None).expect("answers");
    assert!(scoped.memories.is_empty(), "a context answers nothing");
    assert!(!scoped.contexts_read);
}

#[test]
fn a_copied_clone_with_a_clashing_id_lists_no_context_and_says_why() {
    let world = World::new();
    populate(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    propose_at(&mut api, &Actor::Human, Some("Partners"), "Partners rule");
    let platform_rule = memory_of(&propose_at(
        &mut api,
        &Actor::Human,
        Some("Platform"),
        "Platform rule",
    ));
    let copy = world.repos.join("api-copy");
    copy_dir(&world.path("api"), &copy);

    let mut clone = engine_on(&copy, &world.db);
    let listed = search(&mut clone, &Actor::Human, "", None, 50, None).expect("list");
    assert!(listed.memories.is_empty(), "a clash reads no context");
    assert!(
        listed.contexts_read,
        "the file is readable, the clone clashes"
    );
    assert!(
        listed.note.is_some_and(|note| note.contains("clone at")),
        "the note names the other clone"
    );
    let scoped = search(&mut clone, &Actor::Human, "", Some("Platform"), 50, None)
        .expect("the scope is answered");
    assert!(
        scoped.memories.is_empty(),
        "no context is read from a clash"
    );
    let read = send(
        &mut clone,
        &Actor::Human,
        CommandKind::ReadMemory {
            uid: platform_rule.uid.clone(),
        },
    );
    assert_eq!(failure(&read).as_deref(), Some("Memory not found."));
}

/// The number of proposed memories in a merged list, for a comparison with its `proposed_total`.
fn proposed_in(memories: &[Memory]) -> u32 {
    u32::try_from(
        memories
            .iter()
            .filter(|memory| memory.status == gm_schema::threads::MemoryStatus::Proposed)
            .count(),
    )
    .unwrap()
}

/// Lists everything `actor` sees from the open repository in `scope`, unpaged.
fn everything_in(engine: &mut Engine, scope: Option<&str>) -> Seen {
    search(engine, &Actor::Human, "", scope, 50, None).expect("list")
}

#[test]
fn the_proposed_total_of_a_context_shared_by_two_repositories_matches_their_merged_lists() {
    let world = World::new();
    populate(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    let mut web = engine_on(&world.path("web"), &world.db);
    // Two proposals in Platform, one from each repository, and one of api's own.
    propose_at(
        &mut api,
        &agent(),
        Some("Platform"),
        "api proposal in Platform",
    );
    propose_at(
        &mut web,
        &agent(),
        Some("Platform"),
        "web proposal in Platform",
    );
    propose_at(
        &mut api,
        &agent(),
        Some("Partners"),
        "api partners proposal",
    );
    // An accepted memory in Platform is not counted.
    propose_at(
        &mut web,
        &Actor::Human,
        Some("Platform"),
        "accepted in Platform",
    );

    for (name, engine) in [("api", &mut api), ("web", &mut web)] {
        let seen = everything_in(engine, None);
        assert_eq!(
            seen.proposed_total,
            proposed_in(&seen.memories),
            "{name} counts what its merged list shows"
        );
    }
    let everything = everything_in(&mut api, None);
    assert_eq!(
        everything.proposed_total, 3,
        "api: two in Platform and its own"
    );
    assert_eq!(everything.memories.len(), 4);
    assert_eq!(everything_in(&mut web, None).proposed_total, 2);

    // The total is the same for every scope: the scope filters the list, not the count.
    let platform = everything_in(&mut api, Some("Platform"));
    assert_eq!(platform.proposed_total, 3);
    assert_eq!(platform.memories.len(), 3);
    let partners = everything_in(&mut api, Some("Partners"));
    assert_eq!(partners.proposed_total, 3);
    assert_eq!(partners.memories.len(), 1);
}

#[test]
fn the_proposed_total_is_full_on_a_page_of_one_and_ignores_the_query() {
    let world = World::new();
    populate(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    let mut billing = engine_on(&world.path("billing"), &world.db);
    propose_at(&mut api, &agent(), Some("Platform"), "alpha in Platform");
    propose_at(&mut billing, &agent(), Some("Billing"), "beta in Billing");
    propose_at(&mut api, &agent(), Some("Partners"), "gamma in Partners");

    let page = search(&mut api, &Actor::Human, "", None, 1, None).expect("page");
    assert_eq!(page.memories.len(), 1);
    assert!(page.next.is_some());
    assert_eq!(page.proposed_total, 3, "the page does not narrow the total");

    let matching = search(&mut api, &Actor::Human, "gamma", None, 1, None).expect("query");
    assert_eq!(texts(&matching.memories), ["gamma in Partners"]);
    assert_eq!(
        matching.proposed_total, 3,
        "the query does not narrow the total"
    );

    let billing = everything_in(&mut api, Some("Billing"));
    assert_eq!(
        billing.memories.len(),
        2,
        "beta is in Billing and alpha is inherited from Platform"
    );
    assert_eq!(billing.proposed_total, 3, "the total ignores the scope");
}

/// What a `PreviewDeleteContext` answered.
struct Preview {
    sub_contexts: u32,
    memories: Vec<Memory>,
    total: u32,
}

fn preview_of(engine: &mut Engine, actor: &Actor, context: i64) -> Preview {
    let events = send(
        engine,
        actor,
        CommandKind::PreviewDeleteContext { id: context },
    );
    assert_eq!(
        state_of(&events),
        OperationState::Done,
        "preview: {events:?}"
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::ContextDeletePreview {
                context: answered,
                sub_contexts,
                memories,
                total,
                ..
            } => {
                assert_eq!(*answered, context);
                Some(Preview {
                    sub_contexts: *sub_contexts,
                    memories: memories.clone(),
                    total: *total,
                })
            }
            _ => None,
        })
        .expect("a delete preview")
}

#[test]
fn the_preview_lists_exactly_the_memories_a_delete_would_remove() {
    let world = World::new();
    let fixture = populate(&world);
    let mut engine = engine_on(&world.path("api"), &world.db);
    let first = memory_of(&propose_at(
        &mut engine,
        &Actor::Human,
        Some("Billing"),
        "Billing rule one",
    ))
    .uid;
    let second = memory_of(&propose_at(
        &mut engine,
        &Actor::Human,
        Some("Billing"),
        "Billing rule two",
    ))
    .uid;
    let shared = memory_of(&propose_at(
        &mut engine,
        &Actor::Human,
        Some("Billing"),
        "Shared rule",
    ))
    .uid;
    let attached = send(
        &mut engine,
        &Actor::Human,
        CommandKind::AttachMemory {
            uid: shared.clone(),
            context: fixture.partners,
        },
    );
    assert_eq!(state_of(&attached), OperationState::Done, "{attached:?}");

    // An agent may read the preview, and it lists the two held alone and not the shared one.
    let preview = preview_of(&mut engine, &agent(), fixture.billing);
    assert_eq!(preview.total, 2);
    assert_eq!(preview.sub_contexts, 0);
    let mut uids: Vec<String> = preview.memories.iter().map(|m| m.uid.clone()).collect();
    uids.sort();
    let mut expected = vec![first.clone(), second.clone()];
    expected.sort();
    assert_eq!(uids, expected);
    assert!(
        preview
            .memories
            .iter()
            .all(|m| m.scope == "Platform / Billing"),
        "{:?}",
        preview
            .memories
            .iter()
            .map(|m| &m.scope)
            .collect::<Vec<_>>()
    );

    // A delete that keeps the memories is refused and names the count.
    let refused = send(
        &mut engine,
        &Actor::Human,
        CommandKind::DeleteContext {
            id: fixture.billing,
            delete_memories: false,
        },
    );
    assert_eq!(state_of(&refused), OperationState::Failed);
    assert!(
        failure(&refused).is_some_and(|message| message.contains("2 memories")),
        "{refused:?}"
    );

    // A delete with the memories removes exactly the previewed two, and counts them.
    let deleted = send(
        &mut engine,
        &Actor::Human,
        CommandKind::DeleteContext {
            id: fixture.billing,
            delete_memories: true,
        },
    );
    assert_eq!(state_of(&deleted), OperationState::Done, "{deleted:?}");
    assert_eq!(changed(&deleted).1, preview.total);
    for uid in [&first, &second] {
        let gone = send(
            &mut engine,
            &Actor::Human,
            CommandKind::ReadMemory { uid: uid.clone() },
        );
        assert_eq!(state_of(&gone), OperationState::Failed, "{uid} is deleted");
    }
    assert_eq!(read_memory(&mut engine, &shared).uid, shared);

    let missing = send(
        &mut engine,
        &agent(),
        CommandKind::PreviewDeleteContext {
            id: fixture.billing,
        },
    );
    assert_eq!(state_of(&missing), OperationState::Failed);
    assert_eq!(failure(&missing).as_deref(), Some("Context not found."));
}

#[test]
fn the_preview_counts_sub_contexts_and_caps_the_list_at_fifty_with_the_full_total() {
    let world = World::new();
    let fixture = populate(&world);
    let mut engine = engine_on(&world.path("api"), &world.db);

    let platform = preview_of(&mut engine, &agent(), fixture.platform);
    assert_eq!(platform.sub_contexts, 1, "Billing sits under Platform");
    assert_eq!(platform.total, 0);
    assert!(platform.memories.is_empty());

    for n in 0..51 {
        propose_at(
            &mut engine,
            &Actor::Human,
            Some("Partners"),
            &format!("Partners rule {n:02}"),
        );
    }
    let partners = preview_of(&mut engine, &agent(), fixture.partners);
    assert_eq!(partners.sub_contexts, 0);
    assert_eq!(partners.total, 51);
    assert_eq!(partners.memories.len(), 50);
}

/// The contexts a memory carries, as (id, path) pairs in the order the engine gives them.
fn contexts_of(memory: &Memory) -> Vec<(i64, String)> {
    memory
        .contexts
        .iter()
        .map(|context| (context.id, context.path.clone()))
        .collect()
}

#[test]
fn a_memory_lists_every_visible_context_it_is_attached_to_by_path() {
    let world = World::new();
    let fixture = populate(&world);
    let mut billing = engine_on(&world.path("billing"), &world.db);
    let stored = memory_of(&propose_at(
        &mut billing,
        &Actor::Human,
        Some("Billing"),
        "Billing and partners rule",
    ));
    let mut api = engine_on(&world.path("api"), &world.db);
    let attached = send(
        &mut api,
        &Actor::Human,
        CommandKind::AttachMemory {
            uid: stored.uid.clone(),
            context: fixture.partners,
        },
    );
    assert_eq!(state_of(&attached), OperationState::Done, "{attached:?}");

    let expected = vec![
        (fixture.partners, "Partners".to_owned()),
        (fixture.billing, "Platform / Billing".to_owned()),
    ];
    assert_eq!(contexts_of(&read_memory(&mut api, &stored.uid)), expected);
    let seen = search(&mut api, &agent(), "", None, 50, None).expect("list");
    let listed = seen
        .memories
        .iter()
        .find(|memory| memory.uid == stored.uid)
        .expect("listed");
    assert_eq!(contexts_of(listed), expected);
    assert_eq!(
        listed.scope, "Platform / Billing",
        "scope keeps its meaning"
    );
}

#[test]
fn a_memory_lists_every_context_it_is_attached_to_even_one_the_repository_is_not_bound_to() {
    let world = World::new();
    let fixture = populate(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    let stored = memory_of(&propose_at(
        &mut api,
        &Actor::Human,
        Some("Platform"),
        "Platform rule, also in partners",
    ));
    let attached = send(
        &mut api,
        &Actor::Human,
        CommandKind::AttachMemory {
            uid: stored.uid.clone(),
            context: fixture.partners,
        },
    );
    assert_eq!(state_of(&attached), OperationState::Done, "{attached:?}");

    // web is bound to Platform only. It still lists every context of the memory, Partners included
    // (D222 (3)), and its scope is the nearest context it reads, Platform.
    let mut web = engine_on(&world.path("web"), &world.db);
    let expected = vec![
        (fixture.partners, "Partners".to_owned()),
        (fixture.platform, "Platform".to_owned()),
    ];
    assert_eq!(contexts_of(&read_memory(&mut web, &stored.uid)), expected);
    let seen = search(&mut web, &agent(), "", None, 50, None).expect("list");
    let listed = seen
        .memories
        .iter()
        .find(|memory| memory.uid == stored.uid)
        .expect("listed from platform");
    assert_eq!(contexts_of(listed), expected);
    assert_eq!(listed.scope, "Platform");
}

#[test]
fn detaching_from_one_context_leaves_the_other_listed() {
    let world = World::new();
    let fixture = populate(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    let stored = memory_of(&propose_at(
        &mut api,
        &Actor::Human,
        Some("Platform"),
        "Shared platform rule",
    ));
    let attached = send(
        &mut api,
        &Actor::Human,
        CommandKind::AttachMemory {
            uid: stored.uid.clone(),
            context: fixture.partners,
        },
    );
    assert_eq!(state_of(&attached), OperationState::Done, "{attached:?}");

    let detached = send(
        &mut api,
        &Actor::Human,
        CommandKind::DetachMemory {
            uid: stored.uid.clone(),
            context: fixture.partners,
        },
    );
    assert_eq!(state_of(&detached), OperationState::Done, "{detached:?}");
    assert_eq!(
        contexts_of(&read_memory(&mut api, &stored.uid)),
        vec![(fixture.platform, "Platform".to_owned())]
    );
    let mut portal = engine_on(&world.path("portal"), &world.db);
    assert!(
        search(&mut portal, &agent(), "", None, 50, None)
            .expect("list")
            .memories
            .is_empty(),
        "portal no longer sees it"
    );
}

/// The text of each memory a list or a read names, as the test compares them.
fn named_texts(listed: &Seen) -> Vec<String> {
    texts(&listed.memories)
}

#[test]
fn a_context_no_repository_is_bound_to_is_read_by_name_and_left_out_of_the_default_list() {
    let world = World::new();
    populate(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    let mut portal = engine_on(&world.path("portal"), &world.db);
    // The incident is bound to portal only, holds an accepted memory, then portal lets go of it.
    let incident = create(&mut portal, "Incident: stuck exports", None);
    join(&mut portal, incident.id, &world.path("portal"));
    let stored = memory_of(&propose_at(
        &mut portal,
        &Actor::Human,
        Some("Incident: stuck exports"),
        "Restart the export worker first",
    ));
    let unbound = send(
        &mut portal,
        &Actor::Human,
        CommandKind::RemoveRepoFromContext {
            context: incident.id,
            repo_id: repo_id_file(&world.path("portal")),
        },
    );
    assert_eq!(state_of(&unbound), OperationState::Done, "{unbound:?}");

    // api is bound to Platform, Billing and Partners: the incident is not in its default list.
    let default = search(&mut api, &agent(), "", None, 50, None).expect("list");
    assert!(
        !named_texts(&default).contains(&"Restart the export worker first".to_owned()),
        "{default:?}"
    );
    let everything = search(&mut api, &agent(), "", Some("all"), 50, None).expect("list");
    assert!(
        !named_texts(&everything).contains(&"Restart the export worker first".to_owned()),
        "all is the default reach"
    );

    // Named by its scope, it is listed, labelled with the context.
    let named = search(
        &mut api,
        &agent(),
        "",
        Some("Incident: stuck exports"),
        50,
        None,
    )
    .expect("the named context is read");
    assert_eq!(named_texts(&named), ["Restart the export worker first"]);
    assert_eq!(named.memories[0].scope, "Incident: stuck exports");
    assert_eq!(named.memories[0].uid, stored.uid);
}

#[test]
fn a_memory_of_an_unbound_context_is_read_by_its_uid() {
    let world = World::new();
    populate(&world);
    let mut portal = engine_on(&world.path("portal"), &world.db);
    let incident = create(&mut portal, "Incident: stuck exports", None);
    join(&mut portal, incident.id, &world.path("portal"));
    let stored = memory_of(&propose_at(
        &mut portal,
        &Actor::Human,
        Some("Incident: stuck exports"),
        "Restart the export worker first",
    ));
    let unbound = send(
        &mut portal,
        &Actor::Human,
        CommandKind::RemoveRepoFromContext {
            context: incident.id,
            repo_id: repo_id_file(&world.path("portal")),
        },
    );
    assert_eq!(state_of(&unbound), OperationState::Done, "{unbound:?}");

    // api does not see the incident, but reads the memory by uid, with its place in the context.
    let mut api = engine_on(&world.path("api"), &world.db);
    let read = read_memory(&mut api, &stored.uid);
    assert_eq!(read.text, "Restart the export worker first");
    assert_eq!(read.scope, "Incident: stuck exports");
    assert_eq!(
        contexts_of(&read),
        vec![(incident.id, "Incident: stuck exports".to_owned())]
    );
}

#[test]
fn a_sub_context_reads_its_parents_memories_from_a_repository_bound_to_neither() {
    let world = World::new();
    populate(&world);
    let mut api = engine_on(&world.path("api"), &world.db);
    propose_at(&mut api, &Actor::Human, Some("Platform"), "Platform rule");
    propose_at(&mut api, &Actor::Human, Some("Billing"), "Billing rule");
    propose_at(&mut api, &Actor::Human, Some("Partners"), "Partners rule");
    // portal is bound to Partners only, so it does not bind Billing, yet names it.
    let mut portal = engine_on(&world.path("portal"), &world.db);
    let billing = search(&mut portal, &agent(), "", Some("Billing"), 50, None).expect("read");
    assert_eq!(
        sorted(named_texts(&billing)),
        ["Billing rule", "Platform rule"],
        "Billing and its ancestor, not Partners"
    );
    assert_eq!(
        named_texts(&search(&mut portal, &agent(), "", Some("Platform"), 50, None).expect("read")),
        ["Platform rule"],
        "a parent does not take its child's memories"
    );
}

#[test]
fn an_agent_cannot_propose_into_a_context_it_is_not_bound_to_or_inherits_from() {
    let world = World::new();
    populate(&world);
    let mut portal = engine_on(&world.path("portal"), &world.db);
    let incident = create(&mut portal, "Incident: stuck exports", None);
    join(&mut portal, incident.id, &world.path("portal"));
    let unbound = send(
        &mut portal,
        &Actor::Human,
        CommandKind::RemoveRepoFromContext {
            context: incident.id,
            repo_id: repo_id_file(&world.path("portal")),
        },
    );
    assert_eq!(state_of(&unbound), OperationState::Done, "{unbound:?}");

    let mut api = engine_on(&world.path("api"), &world.db);
    let refused = propose_at(
        &mut api,
        &agent(),
        Some("Incident: stuck exports"),
        "Not from api",
    );
    assert_eq!(state_of(&refused), OperationState::Failed, "{refused:?}");
    assert_eq!(
        failure(&refused).as_deref(),
        Some(
            "\"Incident: stuck exports\" is not a context this repository is bound to or inherits from."
        )
    );
}
