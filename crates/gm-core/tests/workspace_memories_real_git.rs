//! A workspace reads and writes its memories through the contexts (D222 (5)) against real
//! repositories in temporary directories: a workspace with three members, each a worktree of its own
//! clone, a fourth clone outside it, and one `contexts.db`. The memories written before the contexts
//! sit in the threads store of the workspace and of each clone, and the first memory command moves
//! each store's memories into the context of its folder (D222 (6)). Each test removes its directory
//! when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_core::threads::memories::NewMemory;
use gm_core::threads::store::ThreadStore;
use gm_core::workspace_cli::run_with;
use gm_core::worktree_cli::Env;
use gm_schema::threads::{
    Context, FileChange, FileChangeState, Memory, MemoryBasis, MemoryKind, MemoryMark,
    MemoryMarkSummary, MemorySource, MemorySourceInput, MemorySourceKind, MemoryStatus,
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

/// The uids of the memories `Fixture::seed` keeps in the threads stores before the contexts.
struct Seeded {
    ws_fact: String,
    api_rule: String,
    api_note: String,
    web_fact: String,
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

    /// Two contexts of the clones, and memories in each store:
    /// Platform (api, jobs), Billing under it (web), and Partners (docs, outside the workspace).
    /// The memories written before the contexts sit in the threads stores of the workspace (`ws_fact`),
    /// of `api` (a file-naming rule, and a note) and of `web`; `jobs` has no store at all.
    fn seed(&self) -> Seeded {
        let mut api = self.engine_on(&self.api);
        let platform = create(&mut api, "Platform", None);
        let billing = create(&mut api, "Billing", Some(platform));
        let partners = create(&mut api, "Partners", None);
        join(&mut api, platform, &self.api);
        join(&mut api, platform, &self.jobs);
        join(&mut api, billing, &self.web);
        join(&mut api, partners, &self.docs);
        propose(
            &mut self.engine_on(&self.api),
            Kind::Rule,
            "platform policy",
            Vec::new(),
            Some("Platform"),
        );
        propose(
            &mut self.engine_on(&self.web),
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
        Seeded {
            ws_fact: keep_in_store(&self.ws, MemoryKind::Fact, "workspace fact", None),
            api_rule: keep_in_store(
                &self.member_api,
                MemoryKind::Rule,
                "api rule",
                Some("f.txt"),
            ),
            api_note: keep_in_store(&self.member_api, MemoryKind::Decision, "api note", None),
            web_fact: keep_in_store(&self.member_web, MemoryKind::Fact, "web fact", None),
        }
    }
}

/// Keeps an accepted memory in the threads store of the clone or workspace at `root`, as memories
/// written before the contexts were kept there. A file source records its blob at `HEAD`. Returns the
/// memory's uid.
fn keep_in_store(root: &Path, kind: MemoryKind, text: &str, file: Option<&str>) -> String {
    let mut store = ThreadStore::open_keeping_memories(&common_of(root)).expect("the store opens");
    let sources: Vec<MemorySource> = file
        .map(|path| MemorySource {
            kind: MemorySourceKind::File,
            reference: path.to_owned(),
            repo_id: None,
            entry_from: None,
            entry_to: None,
            blob_oid: blob_at_head(root, path),
            blob_read: true,
        })
        .into_iter()
        .collect();
    store
        .propose_memory(&NewMemory {
            kind,
            text,
            basis: MemoryBasis::Observed,
            status: MemoryStatus::Accepted,
            origin: "human",
            sources: &sources,
            at: 1_700_000_000_000,
        })
        .expect("the memory is kept")
        .0
        .uid
}

/// The blob of `path` at `HEAD` of the repository at `root`, when the file is there.
fn blob_at_head(root: &Path, path: &str) -> Option<String> {
    let output = Process::new("git")
        .args(["rev-parse", "-q", "--verify", &format!("HEAD:{path}")])
        .current_dir(root)
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[derive(Clone, Copy)]
enum Kind {
    Fact,
    Rule,
    Lesson,
}

impl Kind {
    fn memory(self) -> MemoryKind {
        match self {
            Self::Fact => MemoryKind::Fact,
            Self::Rule => MemoryKind::Rule,
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

/// Binds the repository at `path` to context `context` as the human.
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

/// The texts and scopes of memories, sorted, so a test compares them whole.
fn labelled(memories: &[Memory]) -> Vec<(String, String)> {
    let mut rows: Vec<_> = memories
        .iter()
        .map(|memory| (memory.text.clone(), memory.scope.clone()))
        .collect();
    rows.sort();
    rows
}

fn row(text: &str, scope: &str) -> (String, String) {
    (text.to_owned(), scope.to_owned())
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
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ListContexts { repo, all: false },
    );
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

/// The names of the contexts in a context list, sorted.
fn names(contexts: &[Context]) -> Vec<String> {
    let mut names: Vec<String> = contexts.iter().map(|c| c.name.clone()).collect();
    names.sort();
    names
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

/// The agent of the tests: a name that is only shown (D218 (1)).
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

/// A memory proposed as `actor` (a proposal needs a source), with `member` or `context` when given;
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
fn a_workspace_moves_its_memories_into_the_contexts_of_its_folders() {
    let fx = Fixture::new();
    let seeded = fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let listed = list(&mut ws, None).expect("the list answers");
    assert_eq!(listed.note, None);
    let mut expected = vec![
        row("workspace fact", "pay"),
        row("api rule", "api"),
        row("api note", "api"),
        row("web fact", "web"),
        row("platform policy", "Platform"),
        row("billing rule", "Platform / Billing"),
    ];
    expected.sort();
    assert_eq!(labelled(&listed.memories), expected);
    // The uids and statuses are the ones the stores held (D222 (6)).
    assert_eq!(uid_of(&listed.memories, "workspace fact"), seeded.ws_fact);
    assert_eq!(uid_of(&listed.memories, "api rule"), seeded.api_rule);
    assert_eq!(uid_of(&listed.memories, "api note"), seeded.api_note);
    assert_eq!(uid_of(&listed.memories, "web fact"), seeded.web_fact);
    assert_eq!(
        status_in(&listed.memories, "api rule"),
        MemoryStatus::Accepted
    );
    // Nothing is left in the threads stores.
    let api_store = ThreadStore::open(&common_of(&fx.member_api)).unwrap();
    assert!(api_store.memory_id_of(&seeded.api_rule).is_err());
    let ws_store = ThreadStore::open(&common_of(&fx.ws)).unwrap();
    assert!(ws_store.memory_id_of(&seeded.ws_fact).is_err());
    // Each folder's context is bound to its clone, the workspace to its own repository.
    let (listed_contexts, _, _) = contexts(&mut ws, false);
    assert_eq!(
        names(&listed_contexts),
        vec!["Billing", "Partners", "Platform", "api", "pay", "web"]
    );
}

#[test]
fn a_workspace_reaches_the_contexts_bound_to_itself_and_its_members_with_their_ancestors() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let ops = create(&mut ws, "Ops", None);
    join(&mut ws, ops, &fx.ws);
    propose(&mut ws, Kind::Fact, "ops note", Vec::new(), Some("Ops"));
    let reached = list(&mut ws, None).expect("the list answers");
    let rows = labelled(&reached.memories);
    assert!(rows.contains(&row("ops note", "Ops")), "{rows:?}");
    assert!(rows.contains(&row("platform policy", "Platform")));
    assert!(rows.contains(&row("billing rule", "Platform / Billing")));
    assert!(
        !rows.iter().any(|(text, _)| text == "partners policy"),
        "Partners is bound to no member: {rows:?}"
    );
}

#[test]
fn the_scope_names_a_context_by_path_or_name_and_refuses_the_retired_workspace_scopes() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let scoped = |ws: &mut Engine, scope: &str| {
        labelled(&list(ws, Some(scope)).expect("the list answers").memories)
    };
    assert_eq!(scoped(&mut ws, "pay"), vec![row("workspace fact", "pay")]);
    assert_eq!(
        scoped(&mut ws, "api"),
        vec![row("api note", "api"), row("api rule", "api")]
    );
    // A sub-context inherits the memories of its parent (D222 (2)), whichever way it is named, and a
    // named context reads the sub-contexts under it too.
    let billing_and_platform = vec![
        row("billing rule", "Platform / Billing"),
        row("platform policy", "Platform"),
    ];
    assert_eq!(scoped(&mut ws, "Platform"), billing_and_platform);
    assert_eq!(scoped(&mut ws, "Platform / Billing"), billing_and_platform);
    assert_eq!(scoped(&mut ws, "Billing"), billing_and_platform);
    // A context the workspace is not bound to is named all the same (D222 (3)).
    assert_eq!(
        scoped(&mut ws, "Partners"),
        vec![row("partners policy", "Partners")]
    );
    assert_eq!(
        list(&mut ws, Some("workspace")).expect_err("the workspace scope is retired"),
        "scope \"workspace\" no longer exists: name a context, or use bound"
    );
    assert_eq!(
        list(&mut ws, Some("member api")).expect_err("the member scope is retired"),
        "scope \"member api\" no longer exists: name a context, or use bound"
    );
}

#[test]
fn a_member_memory_added_later_appears_on_the_next_read() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let before = list(&mut ws, Some("api")).expect("the list answers");
    assert_eq!(before.memories.len(), 2);
    keep_in_store(&fx.member_api, MemoryKind::Fact, "api later", None);
    let after = list(&mut ws, Some("api")).expect("the list answers");
    assert!(labelled(&after.memories).contains(&row("api later", "api")));
}

#[test]
fn an_unreadable_member_is_named_and_the_others_still_answer() {
    let fx = Fixture::new();
    fx.seed();
    corrupt_store(&fx.web);
    let listed = list(&mut fx.engine_on(&fx.ws), None).expect("the list answers");
    let note = listed.note.expect("the answer names the member");
    assert!(note.contains("member web could not be read ("), "{note}");
    let rows = labelled(&listed.memories);
    assert!(rows.contains(&row("api rule", "api")));
    assert!(rows.contains(&row("workspace fact", "pay")));
    assert!(!rows.iter().any(|(text, _)| text == "web fact"));
    // The context of the member's memories answers all the same.
    assert!(rows.contains(&row("billing rule", "Platform / Billing")));
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
    let seeded = fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let before = read(&mut ws, &seeded.api_rule);
    assert_eq!(before.scope, "api");
    assert_eq!(before.member, None);
    assert!(before.files_changed_since.is_empty());
    std::fs::write(fx.member_api.join("f.txt"), "one\nfour\n").unwrap();
    git(&fx.member_api, &["commit", "-q", "-am", "api change"]);
    let after = read(&mut ws, &seeded.api_rule);
    assert_eq!(
        after.files_changed_since,
        vec![FileChange {
            path: "f.txt".to_owned(),
            state: FileChangeState::Changed,
        }]
    );
}

#[test]
fn a_retrieval_is_counted_in_the_context_the_memory_lives_in() {
    let fx = Fixture::new();
    let seeded = fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let events = send(
        &mut ws,
        &Actor::Human,
        CommandKind::RecordMemoryRetrieval {
            uids: vec![seeded.api_rule.clone()],
        },
    );
    assert_eq!(state_of(&events), OperationState::Done);
    assert_eq!(
        read(&mut fx.engine_on(&fx.member_api), &seeded.api_rule).retrieval_count,
        1
    );
    assert_eq!(read(&mut ws, &seeded.api_rule).retrieval_count, 1);
}

#[test]
fn a_rejection_takes_only_a_proposed_memory_whichever_context_holds_it() {
    let fx = Fixture::new();
    let seeded = fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let events = send(
        &mut ws,
        &Actor::Human,
        CommandKind::RejectMemory {
            uid: seeded.api_rule.clone(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Failed);
    assert!(!failure(&events).is_empty(), "{events:?}");
    assert_eq!(
        read(&mut ws, &seeded.api_rule).status,
        MemoryStatus::Accepted
    );
}

#[test]
fn a_proposal_without_a_context_goes_to_the_one_context_the_workspace_and_its_members_share() {
    let fx = Fixture::new();
    let mut api = fx.engine_on(&fx.api);
    let platform = create(&mut api, "Platform", None);
    join(&mut api, platform, &fx.api);
    let mut ws = fx.engine_on(&fx.ws);
    let proposed = propose_in(&mut ws, &agent(), "one shared context", None, None)
        .expect("the one bound context takes it");
    assert_eq!(proposed.scope, "Platform");
    assert_eq!(proposed.status, MemoryStatus::Proposed);
    assert_eq!(proposed.member, None);
    // The workspace's own context was not made: a context is made only when nothing is bound.
    let (listed, _, _) = contexts(&mut ws, false);
    assert!(
        !listed.iter().any(|context| context.name == "pay"),
        "{listed:?}"
    );
}

#[test]
fn a_proposal_without_a_context_goes_to_the_workspace_context_when_nothing_is_bound() {
    let fx = Fixture::new();
    let mut ws = fx.engine_on(&fx.ws);
    let proposed = propose_in(&mut ws, &agent(), "nothing bound", None, None)
        .expect("the workspace's own context takes it");
    assert_eq!(proposed.scope, "pay");
    assert_eq!(proposed.status, MemoryStatus::Proposed);
    let (listed, _, _) = contexts(&mut ws, true);
    assert_eq!(members_of(&listed, "pay"), vec!["workspace"]);
    assert!(
        common_of(&fx.ws)
            .join("gitmaster")
            .join("repo-id")
            .is_file()
    );
}

#[test]
fn a_proposal_without_a_context_is_refused_when_several_contexts_are_bound() {
    let fx = Fixture::new();
    let mut api = fx.engine_on(&fx.api);
    let platform = create(&mut api, "Platform", None);
    let partners = create(&mut api, "Partners", None);
    join(&mut api, platform, &fx.api);
    join(&mut api, partners, &fx.web);
    let mut ws = fx.engine_on(&fx.ws);
    let refused = propose_in(&mut ws, &agent(), "several", None, None)
        .expect_err("two contexts are bound: one must be named");
    assert!(refused.contains("several contexts"), "{refused}");
    assert!(refused.contains("\"Partners\", \"Platform\""), "{refused}");
    assert!(refused.contains("name one with context"), "{refused}");
    assert!(list(&mut ws, None).expect("lists").memories.is_empty());
}

#[test]
fn a_proposal_with_a_member_goes_to_that_members_own_context() {
    let fx = Fixture::new();
    let mut ws = fx.engine_on(&fx.ws);
    let in_web = propose_in(&mut ws, &agent(), "web from workspace", Some("web"), None)
        .expect("proposed in web");
    assert_eq!(in_web.status, MemoryStatus::Proposed);
    assert_eq!(in_web.scope, "web");
    assert_eq!(in_web.member, None);
    // The member's own engine sees it as a proposal of its own context.
    let clone = list(&mut fx.engine_on(&fx.web), None).expect("the list answers");
    assert_eq!(
        status_in(&clone.memories, "web from workspace"),
        MemoryStatus::Proposed
    );
    // A member with no store gets none from a proposal: its memories are in its context.
    let in_jobs =
        propose_in(&mut ws, &agent(), "jobs first", Some("jobs"), None).expect("proposed in jobs");
    assert_eq!(in_jobs.scope, "jobs");
    assert!(
        !common_of(&fx.jobs)
            .join("gitmaster")
            .join("threads.db")
            .exists()
    );
    // The workspace's own context was not made by a member's proposal.
    let (listed, _, _) = contexts(&mut ws, false);
    assert!(!listed.iter().any(|context| context.name == "pay"));
}

#[test]
fn a_proposal_names_a_member_or_a_context_but_not_both() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let refused = propose_in(&mut ws, &agent(), "both", Some("web"), Some("Platform"))
        .expect_err("a member and a context are refused together");
    assert_eq!(
        refused,
        "a memory goes to a context or to a member, not both"
    );
}

#[test]
fn a_proposal_with_a_member_in_a_plain_repository_is_refused_and_nothing_is_written() {
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
fn a_proposal_with_a_named_context_follows_the_workspace_reach() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let refused = propose_in(&mut ws, &agent(), "in partners", None, Some("Partners"))
        .expect_err("the workspace is not bound to Partners");
    assert!(
        refused.contains("is not a context this repository is bound to or inherits from"),
        "{refused}"
    );
    let in_billing = propose_in(&mut ws, &agent(), "in billing", None, Some("Billing"))
        .expect("Billing is reached through web");
    assert_eq!(in_billing.scope, "Platform / Billing");
    assert_eq!(in_billing.status, MemoryStatus::Proposed);
}

#[test]
fn a_human_accepts_a_proposed_member_memory_from_the_workspace() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let proposed = propose_in(&mut ws, &agent(), "api proposed", Some("api"), None)
        .expect("the agent proposes in api");
    assert_eq!(proposed.status, MemoryStatus::Proposed);
    assert_eq!(proposed.scope, "api");
    let uid = uid_of(
        &list(&mut ws, Some("api"))
            .expect("the list answers")
            .memories,
        "api proposed",
    );
    let accepted = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid,
            text: None,
            supersedes: Vec::new(),
        },
    )
    .expect("the person accepts it");
    assert_eq!(accepted.status, MemoryStatus::Accepted);
    assert_eq!(accepted.scope, "api");
    // The clone's own engine sees it accepted in the same context.
    let clone = list(&mut fx.engine_on(&fx.api), None).expect("the list answers");
    assert_eq!(
        status_in(&clone.memories, "api proposed"),
        MemoryStatus::Accepted
    );
    assert!(labelled(&clone.memories).contains(&row("api proposed", "api")));
}

#[test]
fn a_human_rejects_and_deletes_member_memories_from_the_workspace() {
    let fx = Fixture::new();
    fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    propose_in(&mut ws, &agent(), "api to reject", Some("api"), None).expect("proposed");
    propose_in(&mut ws, &agent(), "api to delete", Some("api"), None).expect("proposed");
    let listed = list(&mut ws, Some("api"))
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
    assert_eq!(rejected.scope, "api");
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
fn an_agent_marks_a_member_memory_and_cannot_accept_it() {
    let fx = Fixture::new();
    let seeded = fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    propose_in(&mut ws, &agent(), "api to decide", Some("api"), None).expect("proposed");
    let marked = answer(
        &mut ws,
        &agent(),
        CommandKind::MarkMemory {
            uid: seeded.api_rule.clone(),
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
    // The mark is in the context: the clone reads it, with its history.
    let (in_clone, history) = read_with_history(&mut fx.engine_on(&fx.api), &seeded.api_rule);
    assert!(
        in_clone
            .marks
            .iter()
            .any(|mark| mark.actor == "agent:claude"),
        "{in_clone:?}"
    );
    assert!(history.iter().any(|mark| mark.actor == "agent:claude"));
    // An agent accepts nothing, even from the workspace.
    let uid = uid_of(
        &list(&mut ws, Some("api"))
            .expect("the list answers")
            .memories,
        "api to decide",
    );
    let events = send(
        &mut ws,
        &agent(),
        CommandKind::AcceptMemory {
            uid,
            text: None,
            supersedes: Vec::new(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Failed);
    assert!(failure(&events).contains("only a human"), "{events:?}");
}

#[test]
fn a_proposal_is_redacted_with_its_members_rules_and_an_edit_with_the_workspaces() {
    let fx = Fixture::new();
    fx.seed();
    // The pattern is in the member's clone only, for the proposal; the workspace has its own for the edit.
    git(
        &fx.api,
        &["config", "gitmaster-redact.ticket.regex", "TKT-[0-9]+"],
    );
    git(
        &fx.ws,
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
    let edited = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid: proposed.uid.clone(),
            text: Some("Closed TKT-4242 for good".to_owned()),
            supersedes: Vec::new(),
        },
    )
    .expect("the person edits and accepts");
    assert_eq!(edited.text, "Closed [REDACTED:ticket] for good");
    assert_eq!(edited.status, MemoryStatus::Accepted);
}

#[test]
fn a_memory_attaches_to_any_existing_context_and_detaches_from_one_it_is_in() {
    let fx = Fixture::new();
    let seeded = fx.seed();
    let mut ws = fx.engine_on(&fx.ws);
    let (ws_contexts, _, _) = contexts(&mut ws, false);
    let billing = id_named(&ws_contexts, "Billing");
    let partners = id_named(&ws_contexts, "Partners");
    let uid = seeded.api_rule;
    // It is in api; the workspace attaches it to Billing (reached through web) and to Partners, which
    // no member is bound to (D222 (3)).
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
    let to_partners = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::AttachMemory {
            uid: uid.clone(),
            context: partners,
        },
    )
    .expect("any existing context can be attached to");
    assert_eq!(to_partners.contexts.len(), 3, "{to_partners:?}");
    let back = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::DetachMemory {
            uid: uid.clone(),
            context: partners,
        },
    )
    .expect("detached from Partners");
    assert_eq!(back.contexts.len(), 2, "{back:?}");
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
fn a_workspace_lists_the_contexts_it_and_its_members_are_bound_to_with_the_members_that_see_them() {
    let fx = Fixture::new();
    fx.seed();
    // The first memory command moves the memories into their contexts (D222 (6)); a context list
    // alone does not, as for a plain repository.
    let mut ws = fx.engine_on(&fx.ws);
    list(&mut ws, None).expect("the list answers");
    let (listed, repo_id, clash) = contexts(&mut ws, true);
    assert_eq!(
        names(&listed),
        vec!["Billing", "Platform", "api", "pay", "web"]
    );
    assert_eq!(members_of(&listed, "pay"), vec!["workspace"]);
    assert_eq!(members_of(&listed, "Platform"), vec!["api", "jobs", "web"]);
    assert_eq!(members_of(&listed, "Billing"), vec!["web"]);
    assert_eq!(members_of(&listed, "api"), vec!["api"]);
    assert_eq!(repo_id, None, "a workspace's list names no repository id");
    assert_eq!(clash, None);
}

#[test]
fn a_plain_repository_lists_no_member_and_sees_only_its_own_scopes() {
    let fx = Fixture::new();
    fx.seed();
    let mut api = fx.engine_on(&fx.member_api);
    let listed = list(&mut api, None).expect("the list answers");
    assert_eq!(listed.note, None);
    // The clone's memories moved to its own context, `api`, on this list (D222 (6)).
    assert_eq!(
        labelled(&listed.memories),
        vec![
            row("api note", "api"),
            row("api rule", "api"),
            row("platform policy", "Platform"),
        ]
    );
    for memory in &listed.memories {
        let json = serde_json::to_value(memory).unwrap();
        assert!(json.get("member").is_none(), "{json}");
    }
    let (plain, _, _) = contexts(&mut api, true);
    assert_eq!(
        names(&plain),
        vec!["Platform", "api"],
        "the own context is bound too"
    );
    assert!(members_of(&plain, "Platform").is_empty());
}

#[test]
fn a_memory_proposed_from_an_unbound_workspace_reads_its_file_sources_not_unavailable() {
    let fx = Fixture::new();
    let mut api = fx.engine_on(&fx.api);
    let platform = create(&mut api, "Platform", None);
    join(&mut api, platform, &fx.api);
    // The workspace is bound to no context: its file is committed at its own HEAD.
    std::fs::write(fx.ws.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    git(&fx.ws, &["add", "f.txt"]);
    git(&fx.ws, &["commit", "-q", "-m", "workspace file"]);
    let mut ws = fx.engine_on(&fx.ws);
    let proposed = propose_in(&mut ws, &agent(), "reached through api", None, None)
        .expect("the one context the member is bound to takes it");
    assert_eq!(proposed.scope, "Platform");
    let accepted = answer(
        &mut ws,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid: proposed.uid.clone(),
            text: None,
            supersedes: Vec::new(),
        },
    )
    .expect("the person accepts it");
    assert!(
        accepted.files_changed_since.is_empty(),
        "{:?}",
        accepted.files_changed_since
    );
    assert!(read(&mut ws, &proposed.uid).files_changed_since.is_empty());
    std::fs::write(fx.ws.join("f.txt"), "one\nfour\n").unwrap();
    git(&fx.ws, &["commit", "-q", "-am", "workspace change"]);
    assert_eq!(
        read(&mut ws, &proposed.uid).files_changed_since,
        vec![FileChange {
            path: "f.txt".to_owned(),
            state: FileChangeState::Changed,
        }]
    );
}

#[test]
fn a_named_context_proposal_from_an_unbound_workspace_reaches_a_members_context() {
    let fx = Fixture::new();
    let mut api = fx.engine_on(&fx.api);
    let platform = create(&mut api, "Platform", None);
    join(&mut api, platform, &fx.api);
    let partners = create(&mut api, "Partners", None);
    join(&mut api, partners, &fx.docs);
    let mut ws = fx.engine_on(&fx.ws);
    let in_platform = propose_in(
        &mut ws,
        &agent(),
        "named from the workspace",
        None,
        Some("Platform"),
    )
    .expect("Platform is reached through api");
    assert_eq!(in_platform.scope, "Platform");
    assert_eq!(in_platform.status, MemoryStatus::Proposed);
    let refused = propose_in(&mut ws, &agent(), "not reached", None, Some("Partners"))
        .expect_err("the workspace reaches no Partners");
    assert!(
        refused.contains("is not a context this repository is bound to or inherits from"),
        "{refused}"
    );
}
