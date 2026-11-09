//! A workspace reads its members' memories and the contexts its members belong to (D216) against
//! real repositories in temporary directories: a workspace with three members, each a worktree of
//! its own clone, a fourth clone outside it, and one `contexts.db`. Each test removes its directory
//! when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_core::workspace_cli::run_with;
use gm_core::worktree_cli::Env;
use gm_schema::threads::{
    Context, FileChange, FileChangeState, Memory, MemoryBasis, MemoryKind, MemoryMark,
    MemoryMarkSummary, MemorySourceInput, MemorySourceKind, MemoryStatus,
};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// The common Git directory of `dir`, absolute.
fn common_of(dir: &Path) -> PathBuf {
    PathBuf::from(git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ))
}

/// A clone `base/<name>` on `main` with `f.txt`, pushed to a bare origin, and a `feat` branch with
/// one change to `f.txt`, left unchecked-out for a member to take.
fn clone(base: &Path, name: &str) -> PathBuf {
    let origin = base.join(format!("{name}.git"));
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare", "-b", "main"]);
    let repo = base.join(name);
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("f.txt"), "one\nthree\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "feat change"]);
    git(&repo, &["checkout", "-q", "main"]);
    repo
}

/// The workspace of the test, its members and the paths of the clones they come from.
struct Fixture {
    _dir: tempfile::TempDir,
    base: PathBuf,
    identity: PathBuf,
    db: PathBuf,
    /// The main checkouts of the clones; `docs` is not a member.
    api: PathBuf,
    web: PathBuf,
    jobs: PathBuf,
    docs: PathBuf,
    /// The workspace `pay`, and its members, the worktrees on `feat`.
    ws: PathBuf,
    member_api: PathBuf,
    member_web: PathBuf,
    member_jobs: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let identity = base.join("identity.gitconfig");
        std::fs::write(
            &identity,
            "[user]\n\tname = t\n\temail = t@t\n[commit]\n\tgpgsign = false\n",
        )
        .unwrap();
        let api = clone(&base, "api");
        let web = clone(&base, "web");
        let jobs = clone(&base, "jobs");
        let docs = clone(&base, "docs");
        let mut fixture = Self {
            _dir: dir,
            db: base.join("data").join("contexts.db"),
            base,
            identity,
            api,
            web,
            jobs,
            docs,
            ws: PathBuf::new(),
            member_api: PathBuf::new(),
            member_web: PathBuf::new(),
            member_jobs: PathBuf::new(),
        };
        fixture.ws = fixture.create("pay");
        fixture.member_api = fixture.add(&fixture.ws.clone(), &fixture.api.clone(), "feat");
        fixture.member_web = fixture.add(&fixture.ws.clone(), &fixture.web.clone(), "feat");
        fixture.member_jobs = fixture.add(&fixture.ws.clone(), &fixture.jobs.clone(), "feat");
        fixture
    }

    fn env(&self, cwd: &Path) -> Env {
        Env {
            cwd: cwd.to_path_buf(),
            home: Some(self.base.join("home")),
            worktrees_home: None,
        }
    }

    /// Runs `gitmaster workspace <words>` in `cwd`; returns status, stdout and stderr.
    fn run_in(&self, cwd: &Path, words: &[&str]) -> (i32, String, String) {
        let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        let git = SubprocessGit {
            global_config: Some(self.identity.clone()),
            ..SubprocessGit::default()
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with(
            &words,
            &self.env(cwd),
            &git,
            Some(&self.identity),
            &mut "".as_bytes(),
            &mut out,
            &mut err,
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    /// `create <name>` under the fixture's root, asserting success.
    fn create(&self, name: &str) -> PathBuf {
        let root = self.base.join("workspaces");
        let (code, out, err) = self.run_in(
            &self.base,
            &["create", name, "--root", root.to_str().unwrap()],
        );
        assert_eq!(code, 0, "create failed: {err}");
        let dir = root.join(name);
        assert_eq!(out, format!("{}\n", dir.display()));
        dir
    }

    /// `add <repo> <branch>` into `ws`, asserting success; returns the member folder.
    fn add(&self, ws: &Path, repo: &Path, branch: &str) -> PathBuf {
        let (code, out, err) = self.run_in(
            &self.base,
            &[
                "add",
                repo.to_str().unwrap(),
                branch,
                "--workspace",
                ws.to_str().unwrap(),
            ],
        );
        assert_eq!(code, 0, "add failed: {err}");
        PathBuf::from(out.trim_end())
    }

    /// An engine with `root` open, on the fixture's contexts file and workspace configuration.
    fn engine_on(&self, root: &Path) -> Engine {
        let git = SubprocessGit {
            global_config: Some(self.identity.clone()),
            ..SubprocessGit::default()
        };
        let mut engine = Engine::new(Box::new(git));
        engine.set_contexts_db(Some(self.db.clone()));
        engine.set_workspace_global_config(Some(self.identity.clone()));
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

    /// Two contexts of the clones, and memories in each store (D216):
    /// Platform (api, jobs), Billing under it (web), and Partners (docs, outside the workspace).
    /// The workspace and each member write their own memories; a member memory that names a file
    /// is accepted at its own `HEAD`.
    fn seed(&self) {
        let mut api = self.engine_on(&self.api);
        let platform = create(&mut api, "Platform", None);
        let billing = create(&mut api, "Billing", Some(platform));
        let partners = create(&mut api, "Partners", None);
        join(&mut api, platform, &self.api);
        join(&mut api, platform, &self.jobs);
        join(&mut api, billing, &self.web);
        join(&mut api, partners, &self.docs);

        propose(
            &mut self.engine_on(&self.ws),
            Kind::Fact,
            "workspace fact",
            Vec::new(),
            None,
        );
        let mut member_api = self.engine_on(&self.member_api);
        propose(
            &mut member_api,
            Kind::Rule,
            "api rule",
            vec![file("f.txt")],
            None,
        );
        propose(
            &mut member_api,
            Kind::Decision,
            "api note",
            Vec::new(),
            None,
        );
        propose(
            &mut member_api,
            Kind::Rule,
            "platform policy",
            Vec::new(),
            Some("Platform"),
        );
        let mut member_web = self.engine_on(&self.member_web);
        propose(&mut member_web, Kind::Fact, "web fact", Vec::new(), None);
        propose(
            &mut member_web,
            Kind::Lesson,
            "billing rule",
            Vec::new(),
            Some("Billing"),
        );
        propose(
            &mut self.engine_on(&self.docs),
            Kind::Fact,
            "partners policy",
            Vec::new(),
            Some("Partners"),
        );
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Fact,
    Rule,
    Decision,
    Lesson,
}

impl Kind {
    fn memory(self) -> MemoryKind {
        match self {
            Self::Fact => MemoryKind::Fact,
            Self::Rule => MemoryKind::Rule,
            Self::Decision => MemoryKind::Decision,
            Self::Lesson => MemoryKind::Lesson,
        }
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

/// The detail an operation failed with, or an empty text.
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

fn file(path: &str) -> MemorySourceInput {
    MemorySourceInput {
        kind: MemorySourceKind::File,
        reference: path.to_owned(),
        entry_from: None,
        entry_to: None,
    }
}

/// A memory the human writes, which is accepted at once; in a context when `context` names one.
fn propose(
    engine: &mut Engine,
    kind: Kind,
    text: &str,
    sources: Vec<MemorySourceInput>,
    context: Option<&str>,
) -> Memory {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ProposeMemory {
            kind: kind.memory(),
            text: text.to_owned(),
            sources,
            context: context.map(str::to_owned),
            basis: MemoryBasis::Observed,
            member: None,
        },
    );
    assert_eq!(
        state_of(&events),
        OperationState::Done,
        "propose: {events:?}"
    );
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

/// Creates a context as the human; returns its id.
fn create(engine: &mut Engine, name: &str, parent: Option<i64>) -> i64 {
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
    events
        .iter()
        .find_map(|event| match event {
            Event::ContextsChanged {
                context: Some(Context { id, .. }),
                ..
            } => Some(*id),
            _ => None,
        })
        .expect("the context")
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

/// The memories a list answered, with its note.
#[derive(Debug)]
struct Listing {
    memories: Vec<Memory>,
    note: Option<String>,
}

/// `ListMemories` as the human, unfiltered, with `scope`. A refusal is its detail.
fn list(engine: &mut Engine, scope: Option<&str>) -> Result<Listing, String> {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ListMemories {
            query: String::new(),
            kind: None,
            status: None,
            path: None,
            author: None,
            scope: scope.map(str::to_owned),
            limit: 100,
            cursor: None,
        },
    );
    if state_of(&events) != OperationState::Done {
        return Err(failure(&events));
    }
    Ok(events
        .iter()
        .find_map(|event| match event {
            Event::MemoriesListed { memories, note, .. } => Some(Listing {
                memories: memories.clone(),
                note: note.clone(),
            }),
            _ => None,
        })
        .expect("memories listed"))
}

/// The texts, scopes and members of memories, sorted, so a test compares them whole.
fn labelled(memories: &[Memory]) -> Vec<(String, String, Option<String>)> {
    let mut rows: Vec<_> = memories
        .iter()
        .map(|memory| {
            (
                memory.text.clone(),
                memory.scope.clone(),
                memory.member.clone(),
            )
        })
        .collect();
    rows.sort();
    rows
}

fn row(text: &str, scope: &str, member: Option<&str>) -> (String, String, Option<String>) {
    (text.to_owned(), scope.to_owned(), member.map(str::to_owned))
}

/// The uid of the memory with `text` in a list of `memories`.
fn uid_of(memories: &[Memory], text: &str) -> String {
    memories
        .iter()
        .find(|memory| memory.text == text)
        .unwrap_or_else(|| panic!("{text} is listed"))
        .uid
        .clone()
}

/// `ReadMemory` of `uid`; its memory.
fn read(engine: &mut Engine, uid: &str) -> Memory {
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

/// `ListContexts`: the contexts, the repository id and the clash.
fn contexts(engine: &mut Engine, repo: bool) -> (Vec<Context>, Option<String>, Option<String>) {
    let events = send(engine, &Actor::Human, CommandKind::ListContexts { repo });
    assert_eq!(state_of(&events), OperationState::Done, "list: {events:?}");
    events
        .iter()
        .find_map(|event| match event {
            Event::ContextsListed {
                contexts,
                repo_id,
                clash,
                ..
            } => Some((contexts.clone(), repo_id.clone(), clash.clone())),
            _ => None,
        })
        .expect("contexts listed")
}

/// The members of each context named `name` in a context list.
fn members_of(contexts: &[Context], name: &str) -> Vec<String> {
    contexts
        .iter()
        .find(|context| context.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
        .members
        .clone()
}

#[test]
fn a_workspace_reads_its_own_its_members_and_the_contexts_they_see() {
    let fx = Fixture::new();
    fx.seed();
    let listed = list(&mut fx.engine_on(&fx.ws), None).expect("the list answers");
    assert_eq!(listed.note, None);
    let mut expected = vec![
        row("workspace fact", "workspace", None),
        row("api rule", "member api", Some("api")),
        row("api note", "member api", Some("api")),
        row("web fact", "member web", Some("web")),
        row("platform policy", "Platform", None),
        row("billing rule", "Platform / Billing", None),
    ];
    expected.sort();
    assert_eq!(labelled(&listed.memories), expected);
}

#[test]
fn a_member_memory_added_later_appears_on_the_next_read() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let before = list(&mut ws, Some("member api")).expect("the list answers");
    assert_eq!(before.memories.len(), 2);
    propose(
        &mut fx.engine_on(&fx.member_api),
        Kind::Fact,
        "api later",
        Vec::new(),
        None,
    );
    let after = list(&mut ws, Some("member api")).expect("the list answers");
    assert!(labelled(&after.memories).contains(&row("api later", "member api", Some("api"))));
}

#[test]
fn the_scope_names_the_workspace_a_member_or_a_context() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let scoped = |ws: &mut Engine, scope: &str| {
        labelled(&list(ws, Some(scope)).expect("the list answers").memories)
    };
    assert_eq!(
        scoped(&mut ws, "workspace"),
        vec![row("workspace fact", "workspace", None)]
    );
    assert_eq!(
        scoped(&mut ws, "member api"),
        vec![
            row("api note", "member api", Some("api")),
            row("api rule", "member api", Some("api")),
        ]
    );
    assert_eq!(
        scoped(&mut ws, "member web"),
        vec![row("web fact", "member web", Some("web"))]
    );
    assert_eq!(
        scoped(&mut ws, "Platform"),
        vec![row("platform policy", "Platform", None)]
    );
    assert_eq!(
        scoped(&mut ws, "Platform / Billing"),
        vec![row("billing rule", "Platform / Billing", None)]
    );
    assert_eq!(
        scoped(&mut ws, "Billing"),
        vec![row("billing rule", "Platform / Billing", None)]
    );
    assert_eq!(scoped(&mut ws, "member jobs"), Vec::new());
}

#[test]
fn an_ambiguous_or_unknown_scope_is_refused_and_names_the_candidates() {
    let fx = Fixture::new();
    fx.seed();
    let mut api = fx.engine_on(&fx.api);
    let api_context = create(&mut api, "api", None);
    join(&mut api, api_context, &fx.api);
    let mut ws = fx.engine_on(&fx.ws);
    let ambiguous = list(&mut ws, Some("api")).expect_err("a bare name in two senses is refused");
    assert!(ambiguous.contains("member api"), "{ambiguous}");
    assert!(ambiguous.contains("context"), "{ambiguous}");
    let unknown = list(&mut ws, Some("member nope")).expect_err("an unknown member is refused");
    assert!(unknown.contains("members are: api, jobs, web"), "{unknown}");
    assert!(list(&mut ws, Some("member api")).is_ok());
}

#[test]
fn an_unreadable_member_is_named_and_the_others_still_answer() {
    let fx = Fixture::new();
    fx.seed();
    let store = common_of(&fx.web).join("gitmaster").join("threads.db");
    for sidecar in ["threads.db-wal", "threads.db-shm"] {
        let _ = std::fs::remove_file(store.with_file_name(sidecar));
    }
    std::fs::write(&store, "this is not a SQLite database\n".repeat(64)).unwrap();
    let listed = list(&mut fx.engine_on(&fx.ws), None).expect("the list answers");
    let note = listed.note.expect("the answer names the member");
    assert!(note.contains("member web could not be read ("), "{note}");
    let rows = labelled(&listed.memories);
    assert!(rows.contains(&row("api rule", "member api", Some("api"))));
    assert!(rows.contains(&row("workspace fact", "workspace", None)));
    assert!(!rows.iter().any(|(text, ..)| text == "web fact"));
}

#[test]
fn a_member_with_no_store_is_empty_and_its_store_is_not_created() {
    let fx = Fixture::new();
    fx.seed();
    let listed = list(&mut fx.engine_on(&fx.ws), None).expect("the list answers");
    assert_eq!(listed.note, None);
    assert!(
        !common_of(&fx.jobs)
            .join("gitmaster")
            .join("threads.db")
            .exists()
    );
}

#[test]
fn reading_a_member_memory_gives_its_scope_and_its_changes_in_that_member() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let uid = uid_of(
        &list(&mut ws, None).expect("the list answers").memories,
        "api rule",
    );
    let before = read(&mut ws, &uid);
    assert_eq!(before.scope, "member api");
    assert_eq!(before.member.as_deref(), Some("api"));
    assert!(before.files_changed_since.is_empty());
    std::fs::write(fx.member_api.join("f.txt"), "one\nfour\n").unwrap();
    git(&fx.member_api, &["commit", "-q", "-am", "api change"]);
    let after = read(&mut ws, &uid);
    assert_eq!(
        after.files_changed_since,
        vec![FileChange {
            path: "f.txt".to_owned(),
            state: FileChangeState::Changed,
        }]
    );
}

#[test]
fn a_retrieval_is_counted_in_the_store_the_memory_lives_in() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let uid = uid_of(
        &list(&mut ws, None).expect("the list answers").memories,
        "api rule",
    );
    let events = send(
        &mut ws,
        &Actor::Human,
        CommandKind::RecordMemoryRetrieval {
            uids: vec![uid.clone()],
        },
    );
    assert_eq!(state_of(&events), OperationState::Done);
    assert_eq!(
        read(&mut fx.engine_on(&fx.member_api), &uid).retrieval_count,
        1
    );
    assert_eq!(read(&mut ws, &uid).retrieval_count, 1);
}

#[test]
fn a_member_memory_keeps_the_rules_of_its_member_from_the_workspace() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let uid = uid_of(
        &list(&mut ws, None).expect("the list answers").memories,
        "api rule",
    );
    // A rejection takes only a proposed memory, whichever store holds it (D192 (4)).
    let events = send(
        &mut ws,
        &Actor::Human,
        CommandKind::RejectMemory { uid: uid.clone() },
    );
    assert_eq!(state_of(&events), OperationState::Failed);
    assert!(!failure(&events).is_empty(), "{events:?}");
    assert_eq!(read(&mut ws, &uid).status, MemoryStatus::Accepted);
}

/// The member clones' agent: a name that is only shown (D218 (1)).
fn agent() -> Actor {
    Actor::Agent {
        name: "claude".to_owned(),
    }
}

/// The memory a command answers, or the detail it failed with.
fn answer(engine: &mut Engine, actor: &Actor, kind: CommandKind) -> Result<Memory, String> {
    let events = send(engine, actor, kind);
    if state_of(&events) != OperationState::Done {
        return Err(failure(&events));
    }
    Ok(events
        .iter()
        .find_map(|event| match event {
            Event::MemoryChanged {
                memory: Some(memory),
                ..
            } => Some(memory.clone()),
            _ => None,
        })
        .expect("a memory"))
}

/// A memory proposed as `actor` (a proposal needs a source), in `member` and `context` when given;
/// the memory it answers, or the detail it failed with.
fn propose_in(
    engine: &mut Engine,
    actor: &Actor,
    text: &str,
    member: Option<&str>,
    context: Option<&str>,
) -> Result<Memory, String> {
    answer(
        engine,
        actor,
        CommandKind::ProposeMemory {
            kind: MemoryKind::Rule,
            text: text.to_owned(),
            sources: vec![file("f.txt")],
            context: context.map(str::to_owned),
            basis: MemoryBasis::Observed,
            member: member.map(str::to_owned),
        },
    )
}

/// The status of the memory with `text` in a list.
fn status_in(memories: &[Memory], text: &str) -> MemoryStatus {
    memories
        .iter()
        .find(|memory| memory.text == text)
        .unwrap_or_else(|| panic!("{text} is listed"))
        .status
}

/// `ReadMemory` of `uid`, with its full mark history, oldest first.
fn read_with_history(engine: &mut Engine, uid: &str) -> (Memory, Vec<MemoryMarkSummary>) {
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
            Event::MemoryRead {
                memory,
                mark_history,
                ..
            } => Some((memory.clone(), mark_history.clone())),
            _ => None,
        })
        .expect("a memory read")
}

/// The id of the context named `name` in a context list.
fn id_named(contexts: &[Context], name: &str) -> i64 {
    contexts
        .iter()
        .find(|context| context.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
        .id
}

/// Replaces the clone's store with a file that is not a database, so the member cannot be read.
fn corrupt_store(clone: &Path) {
    let store = common_of(clone).join("gitmaster").join("threads.db");
    for sidecar in ["threads.db-wal", "threads.db-shm"] {
        let _ = std::fs::remove_file(store.with_file_name(sidecar));
    }
    std::fs::write(&store, "this is not a SQLite database\n".repeat(64)).unwrap();
}

#[test]
fn a_human_accepts_a_proposed_member_memory_from_the_workspace() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let proposed = propose_in(&mut ws, &agent(), "api proposed", Some("api"), None)
        .expect("the agent proposes in api");
    assert_eq!(proposed.status, MemoryStatus::Proposed);
    assert_eq!(proposed.scope, "member api");
    assert_eq!(proposed.member.as_deref(), Some("api"));
    let uid = uid_of(
        &list(&mut ws, Some("member api"))
            .expect("the list answers")
            .memories,
        "api proposed",
    );
    let accepted = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid: uid.clone(),
            text: None,
            supersedes: Vec::new(),
        },
    )
    .expect("the person accepts it");
    assert_eq!(accepted.status, MemoryStatus::Accepted);
    assert_eq!(accepted.scope, "member api");
    assert_eq!(accepted.member.as_deref(), Some("api"));
    // The workspace's list shows the change at once.
    let at_once = list(&mut ws, Some("member api")).expect("the list answers");
    assert_eq!(
        status_in(&at_once.memories, "api proposed"),
        MemoryStatus::Accepted
    );
    // A second engine on the clone alone sees it accepted in the clone's own store.
    let clone = list(&mut fx.engine_on(&fx.api), None).expect("the list answers");
    assert_eq!(
        status_in(&clone.memories, "api proposed"),
        MemoryStatus::Accepted
    );
    assert!(labelled(&clone.memories).contains(&row("api proposed", "repository", None)));
}

#[test]
fn a_human_rejects_and_deletes_member_memories_from_the_workspace() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    propose_in(&mut ws, &agent(), "api to reject", Some("api"), None).expect("proposed");
    propose_in(&mut ws, &agent(), "api to delete", Some("api"), None).expect("proposed");
    let listed = list(&mut ws, Some("member api"))
        .expect("the list answers")
        .memories;
    let rejected = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::RejectMemory {
            uid: uid_of(&listed, "api to reject"),
        },
    )
    .expect("the person rejects it");
    assert_eq!(rejected.status, MemoryStatus::Rejected);
    assert_eq!(rejected.scope, "member api");
    let events = send(
        &mut ws,
        &Actor::Human,
        CommandKind::DeleteMemory {
            uid: uid_of(&listed, "api to delete"),
        },
    );
    assert_eq!(state_of(&events), OperationState::Done, "{events:?}");
    let clone = list(&mut fx.engine_on(&fx.api), None).expect("the list answers");
    assert_eq!(
        status_in(&clone.memories, "api to reject"),
        MemoryStatus::Rejected
    );
    assert!(
        !clone
            .memories
            .iter()
            .any(|memory| memory.text == "api to delete")
    );
}

#[test]
fn an_agent_marks_a_member_memory_in_the_member_and_cannot_accept_it() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    propose_in(&mut ws, &agent(), "api to decide", Some("api"), None).expect("proposed");
    let listed = list(&mut ws, Some("member api"))
        .expect("the list answers")
        .memories;
    let uid = uid_of(&listed, "api rule");
    let marked = answer(
        &mut ws,
        &agent(),
        CommandKind::MarkMemory {
            uid: uid.clone(),
            mark: MemoryMark::Useful,
            reason: Some("it helped".to_owned()),
        },
    )
    .expect("an agent may mark");
    assert!(
        marked
            .marks
            .iter()
            .any(|mark| mark.actor == "agent:claude" && mark.mark == MemoryMark::Useful),
        "{marked:?}"
    );
    // The mark is in the member's store: the clone reads it, with its history.
    let (in_clone, history) = read_with_history(&mut fx.engine_on(&fx.api), &uid);
    assert!(
        in_clone
            .marks
            .iter()
            .any(|mark| mark.actor == "agent:claude"),
        "{in_clone:?}"
    );
    assert!(history.iter().any(|mark| mark.actor == "agent:claude"));
    // An agent accepts nothing, even from the workspace.
    let events = send(
        &mut ws,
        &agent(),
        CommandKind::AcceptMemory {
            uid: uid_of(&listed, "api to decide"),
            text: None,
            supersedes: Vec::new(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Failed);
    assert!(failure(&events).contains("only a human"), "{events:?}");
    let clone = list(&mut fx.engine_on(&fx.api), None).expect("the list answers");
    assert_eq!(
        status_in(&clone.memories, "api to decide"),
        MemoryStatus::Proposed
    );
}

#[test]
fn an_edit_from_the_workspace_goes_through_the_members_redaction() {
    let fx = Fixture::new();
    fx.seed();
    // The pattern is in the member's clone only: the workspace has none.
    git(
        &fx.api,
        &["config", "gitmaster-redact.ticket.regex", "TKT-[0-9]+"],
    );
    let mut ws = fx.engine_on(&fx.ws);
    let proposed = propose_in(
        &mut ws,
        &agent(),
        "Ticket TKT-77 is open",
        Some("api"),
        None,
    )
    .expect("proposed");
    assert_eq!(proposed.text, "Ticket [REDACTED:ticket] is open");
    let uid = proposed.uid.clone();
    let edited = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid,
            text: Some("Closed TKT-4242 for good".to_owned()),
            supersedes: Vec::new(),
        },
    )
    .expect("the person edits and accepts");
    assert_eq!(edited.text, "Closed [REDACTED:ticket] for good");
    assert_eq!(edited.status, MemoryStatus::Accepted);
}

#[test]
fn a_member_memory_moves_to_a_context_its_member_belongs_to_and_no_other() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let (ws_contexts, _, _) = contexts(&mut ws, true);
    let platform = id_named(&ws_contexts, "Platform");
    let billing = id_named(&ws_contexts, "Billing");
    let (docs_contexts, _, _) = contexts(&mut fx.engine_on(&fx.docs), true);
    let partners = id_named(&docs_contexts, "Partners");
    let uid = uid_of(
        &list(&mut ws, Some("member api"))
            .expect("the list answers")
            .memories,
        "api note",
    );
    // api belongs to Platform, not to Billing or Partners: its own rules refuse those.
    let outside = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::MoveMemoryToContext {
            uid: uid.clone(),
            context: partners,
        },
    );
    assert!(
        outside
            .expect_err("a context the member does not belong to")
            .contains("The context is not one this repository belongs to."),
    );
    let moved = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::MoveMemoryToContext {
            uid: uid.clone(),
            context: platform,
        },
    )
    .expect("api belongs to Platform");
    assert_eq!(moved.uid, uid);
    assert_eq!(moved.scope, "Platform");
    assert_eq!(moved.member, None);
    assert!(
        labelled(
            &list(&mut fx.engine_on(&fx.api), None)
                .expect("lists")
                .memories
        )
        .contains(&row("api note", "Platform", None))
    );
    // Attached from the workspace, to a context one of its members belongs to (Billing, of web).
    let attached = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::AttachMemory {
            uid: uid.clone(),
            context: billing,
        },
    )
    .expect("the workspace sees Billing through web");
    assert_eq!(attached.contexts.len(), 2, "{attached:?}");
    let refused = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::AttachMemory {
            uid: uid.clone(),
            context: partners,
        },
    );
    assert!(
        refused
            .expect_err("a context no member belongs to")
            .contains("The context is not one this repository belongs to."),
    );
    let detached = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::DetachMemory {
            uid,
            context: billing,
        },
    )
    .expect("detached from Billing");
    assert_eq!(detached.contexts.len(), 1, "{detached:?}");
}

#[test]
fn a_proposal_with_a_member_is_stored_in_that_members_store() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let in_api = propose_in(&mut ws, &agent(), "api from workspace", Some("api"), None)
        .expect("proposed in api");
    assert_eq!(in_api.status, MemoryStatus::Proposed);
    assert_eq!(in_api.scope, "member api");
    let clone = list(&mut fx.engine_on(&fx.api), None).expect("the list answers");
    assert_eq!(
        status_in(&clone.memories, "api from workspace"),
        MemoryStatus::Proposed
    );
    let own = list(&mut ws, Some("workspace")).expect("the list answers");
    assert!(
        !own.memories
            .iter()
            .any(|memory| memory.text == "api from workspace")
    );
    // Without a member it stays in the workspace's own store.
    let own_one = propose_in(&mut ws, &agent(), "workspace only", None, None).expect("proposed");
    assert_eq!(own_one.scope, "workspace");
    assert_eq!(own_one.member, None);
    let clone = list(&mut fx.engine_on(&fx.api), None).expect("the list answers");
    assert!(
        !clone
            .memories
            .iter()
            .any(|memory| memory.text == "workspace only")
    );
    // A human's proposal in a member is accepted, as in the member.
    let human = propose_in(&mut ws, &Actor::Human, "api by person", Some("api"), None)
        .expect("written in api");
    assert_eq!(human.status, MemoryStatus::Accepted);
    // A member with no store yet gets one when a proposal is written to it, as its own engine would.
    assert!(
        !common_of(&fx.jobs)
            .join("gitmaster")
            .join("threads.db")
            .exists()
    );
    let in_jobs =
        propose_in(&mut ws, &agent(), "jobs first", Some("jobs"), None).expect("proposed in jobs");
    assert_eq!(in_jobs.scope, "member jobs");
    let jobs = list(&mut fx.engine_on(&fx.jobs), None).expect("the list answers");
    assert_eq!(
        status_in(&jobs.memories, "jobs first"),
        MemoryStatus::Proposed
    );
    // The context of a member's proposal is resolved against the member's contexts.
    let in_platform = propose_in(
        &mut ws,
        &agent(),
        "api in platform",
        Some("api"),
        Some("Platform"),
    )
    .expect("api belongs to Platform");
    assert_eq!(in_platform.scope, "Platform");
    assert_eq!(in_platform.member, None);
    // web sees Platform as the parent of Billing, but not Partners, which no member belongs to.
    let refused = propose_in(
        &mut ws,
        &agent(),
        "web in partners",
        Some("web"),
        Some("Partners"),
    );
    assert!(
        refused
            .expect_err("web does not belong to Partners")
            .contains("is not a context this repository belongs to"),
    );
}

#[test]
fn a_member_named_in_a_plain_repository_is_refused_and_nothing_is_written() {
    let fx = Fixture::new();
    fx.seed();
    let mut api = fx.engine_on(&fx.api);
    let before = labelled(&list(&mut api, None).expect("lists").memories);
    let refused = propose_in(&mut api, &agent(), "not here", Some("web"), None);
    assert_eq!(
        refused.expect_err("refused"),
        "this repository is not a workspace"
    );
    assert_eq!(
        labelled(&list(&mut api, None).expect("lists").memories),
        before
    );
}

#[test]
fn an_unknown_member_is_refused_and_the_answer_names_the_members() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let refused = propose_in(&mut ws, &agent(), "nowhere", Some("nope"), None)
        .expect_err("an unknown member is refused");
    assert!(refused.contains("there is no member nope"), "{refused}");
    assert!(refused.contains("members are: api, jobs, web"), "{refused}");
}

#[test]
fn an_unavailable_member_refuses_with_its_reason_and_changes_nothing() {
    let fx = Fixture::new();
    fx.seed();
    corrupt_store(&fx.web);
    let mut ws = fx.engine_on(&fx.ws);
    let before = labelled(&list(&mut ws, None).expect("the list answers").memories);
    let refused = propose_in(&mut ws, &agent(), "into web", Some("web"), None)
        .expect_err("an unreadable member refuses");
    assert!(
        refused.contains("member web could not be read ("),
        "{refused}"
    );
    // A command on a memory no readable store holds names the member that may have it.
    let events = send(
        &mut ws,
        &Actor::Human,
        CommandKind::RejectMemory {
            uid: "0123456789abcdef".to_owned(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Failed);
    assert!(
        failure(&events).contains("member web could not be read ("),
        "{events:?}"
    );
    assert_eq!(
        labelled(&list(&mut ws, None).expect("the list answers").memories),
        before
    );
    assert!(
        list(&mut fx.engine_on(&fx.api), None)
            .expect("the clone answers")
            .memories
            .iter()
            .any(|memory| memory.text == "api rule")
    );
}

#[test]
fn a_workspace_lists_the_contexts_of_its_members_with_the_members_that_see_them() {
    let fx = Fixture::new();
    fx.seed();
    let (listed, repo_id, clash) = contexts(&mut fx.engine_on(&fx.ws), true);
    let names: Vec<&str> = listed.iter().map(|context| context.name.as_str()).collect();
    assert_eq!(names, vec!["Platform", "Billing"]);
    assert_eq!(members_of(&listed, "Platform"), vec!["api", "jobs", "web"]);
    assert_eq!(members_of(&listed, "Billing"), vec!["web"]);
    assert_eq!(repo_id, None, "the workspace is in no context");
    assert_eq!(clash, None);
    assert!(!common_of(&fx.ws).join("gitmaster").join("repo-id").exists());
}

#[test]
fn a_plain_repository_lists_no_member_and_sees_only_its_own_scopes() {
    let fx = Fixture::new();
    fx.seed();
    let mut api = fx.engine_on(&fx.member_api);
    let listed = list(&mut api, None).expect("the list answers");
    assert_eq!(listed.note, None);
    assert_eq!(
        labelled(&listed.memories),
        vec![
            row("api note", "repository", None),
            row("api rule", "repository", None),
            row("platform policy", "Platform", None),
        ]
    );
    for memory in &listed.memories {
        let json = serde_json::to_value(memory).unwrap();
        assert!(json.get("member").is_none(), "{json}");
    }
    let (plain, _, _) = contexts(&mut api, true);
    let names: Vec<&str> = plain.iter().map(|context| context.name.as_str()).collect();
    assert_eq!(names, vec!["Platform"]);
    assert!(members_of(&plain, "Platform").is_empty());
}
