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
use gm_schema::threads::{Context, Memory, MemoryKind, MemorySourceInput, MemorySourceKind};
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
