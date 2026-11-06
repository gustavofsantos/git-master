//! The commit lookup of a member's repository lists the threads of the workspaces it belongs to
//! (D180 (2)), against real repositories, workspaces and databases in temporary directories. The
//! workspaces are made by `gitmaster workspace`, so their pointer files are the ones the CLI writes.
//! The fake harness stands in for the agent; the commits are real.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::threads::config::ThreadsConfig;
use gm_core::threads::harness::fake::{self, FakeAdapter};
use gm_core::threads::pipeline::{Context, ingest, locate_repo};
use gm_core::threads::store::ThreadStore;
use gm_core::workspace_cli::run_with;
use gm_core::worktree_cli::Env;
use gm_schema::threads::{LinkKind, LinkOrigin, ThreadFilter, ThreadLink, ThreadSummary};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId};
use serde_json::{Value, json};

const NOW: i64 = 1_700_000_000_000;

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
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8 output")
        .trim_end()
        .to_owned()
}

/// A scratch folder with the repository `repo` (branches `feat1` and `feat2`) and an identity file
/// for the commits the workspace CLI makes.
struct Fixture {
    _dir: tempfile::TempDir,
    base: PathBuf,
    repo: PathBuf,
    identity: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().canonicalize().expect("canonical");
        let repo = base.join("repo");
        fs::create_dir(&repo).expect("repo");
        git(&repo, &["init", "-q", "-b", "main"]);
        fs::write(repo.join("a.txt"), "clone\n").expect("write");
        git(&repo, &["add", "a.txt"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        git(&repo, &["branch", "feat1"]);
        git(&repo, &["branch", "feat2"]);
        let identity = base.join("identity.gitconfig");
        fs::write(
            &identity,
            "[user]\n\tname = t\n\temail = t@t\n[commit]\n\tgpgsign = false\n",
        )
        .expect("identity");
        Self {
            _dir: dir,
            base,
            repo,
            identity,
        }
    }

    fn root(&self) -> PathBuf {
        self.base.join("workspaces")
    }

    /// `gitmaster workspace <words>` run from the scratch folder: status, stdout and stderr.
    fn cli(&self, words: &[&str]) -> (i32, String, String) {
        let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        let env = Env {
            cwd: self.base.clone(),
            home: Some(self.base.join("home")),
            worktrees_home: None,
        };
        let git = SubprocessGit {
            global_config: Some(self.identity.clone()),
            ..SubprocessGit::default()
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with(
            &words,
            &env,
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

    /// A workspace `name` under the root, with the repository on `branch` as member `billing`.
    fn workspace(&self, name: &str, branch: &str) -> PathBuf {
        let root = self.root();
        let root_text = root.to_str().unwrap().to_owned();
        let (code, _, err) = self.cli(&["create", name, "--root", &root_text]);
        assert_eq!(code, 0, "create failed: {err}");
        let ws = root.join(name);
        let ws_text = ws.to_str().unwrap().to_owned();
        let repo_text = self.repo.to_str().unwrap().to_owned();
        let (code, _, err) = self.cli(&[
            "add",
            &repo_text,
            branch,
            "--workspace",
            &ws_text,
            "--as",
            "billing",
        ]);
        assert_eq!(code, 0, "add failed: {err}");
        ws
    }

    /// The pointer file of the member repository, where the CLI listed the workspaces.
    fn pointers(&self) -> PathBuf {
        self.repo.join(".git").join("gitmaster").join("workspaces")
    }
}

/// One session fed payload by payload like the hook, from `dir`.
struct Session {
    dir: PathBuf,
    store: ThreadStore,
    id: String,
}

impl Session {
    fn open(dir: &Path, id: &str) -> Self {
        Self {
            dir: dir.to_path_buf(),
            store: ThreadStore::open(&dir.join(".git")).expect("open store"),
            id: id.to_owned(),
        }
    }

    fn send(&mut self, value: &Value) {
        let bytes = serde_json::to_vec(value).expect("json");
        let repo = locate_repo(&self.dir).expect("inside the repository");
        let config = ThreadsConfig::default();
        let ctx = Context {
            config: &config,
            now_ms: NOW,
        };
        let _ = ingest(&FakeAdapter, &bytes, &repo, &ctx, &mut self.store);
    }

    /// Starts the session at `at` and sends its first prompt.
    fn begin(&mut self, at: i64) {
        let dir = self.dir.display().to_string();
        self.send(
            &json!({"event": "session_start", "session": self.id, "id": "start",
            "at": at, "cwd": dir}),
        );
        self.send(
            &json!({"event": "prompt", "session": self.id, "id": "u1", "at": at,
            "text": "work"}),
        );
    }

    fn end(&mut self, at: i64) {
        self.send(&json!({"event": "turn_end", "session": self.id, "id": "e1", "at": at}));
    }
}

/// The uid of the thread of session `id` under the fake harness.
fn uid(id: &str) -> String {
    ThreadStore::thread_uid(fake::ID, id)
}

/// Links the thread of session `id`, stored under `dir`, to `commit`, by hand.
fn link_by_hand(dir: &Path, id: &str, commit: &str, member: Option<&str>) {
    let mut store = ThreadStore::open(&dir.join(".git")).expect("open store");
    store
        .link(
            &uid(id),
            &ThreadLink {
                kind: LinkKind::Commit,
                reference: commit.to_owned(),
                origin: LinkOrigin::Human,
                resolved: None,
                state: None,
                member: member.map(str::to_owned),
            },
        )
        .expect("link");
}

/// The member `billing` of `ws` commits a file on its branch, in a thread captured at the
/// workspace root. Returns the commit.
fn captured_member_commit(ws: &Path, id: &str, begin_at: i64, end_at: i64) -> String {
    let billing = ws.join("billing");
    let mut session = Session::open(ws, id);
    session.begin(begin_at);
    fs::write(billing.join(format!("{id}.rs")), "fn y() {}\n").expect("write");
    git(&billing, &["add", "."]);
    git(&billing, &["commit", "-q", "-m", "member work"]);
    let commit = git(&billing, &["rev-parse", "HEAD"]);
    session.end(end_at);
    commit
}

/// The threads of the commit `commit` that the History panel lists from the member repository.
fn list(
    repo: &Path,
    commit: &str,
    before: Option<(i64, String)>,
    limit: u32,
) -> Vec<ThreadSummary> {
    let git = SubprocessGit {
        log: Some(CommandLog::default()),
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    engine.handle(
        OperationId(1),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::OpenRepo {
                path: repo.to_string_lossy().into_owned(),
            },
        },
    );
    let (before_at, before_uid) = match before {
        Some((at, uid)) => (Some(at), Some(uid)),
        None => (None, None),
    };
    let events = engine.handle(
        OperationId(2),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::ListThreads {
                filter: ThreadFilter {
                    commit: Some(commit.to_owned()),
                    ..ThreadFilter::default()
                },
                before_at,
                before_uid,
                limit,
            },
        },
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::ThreadsListed { threads, .. } => Some(threads.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no listing: {events:?}"))
}

fn uids(threads: &[ThreadSummary]) -> Vec<&str> {
    threads.iter().map(|thread| thread.uid.as_str()).collect()
}

/// A workspace `billing` of which a thread captured the commit `commit`, and a second workspace
/// whose thread is linked by hand to the same commit with its member. Returns the commit and the
/// two workspaces.
struct Scene {
    fixture: Fixture,
    one: PathBuf,
    two: PathBuf,
    commit: String,
}

fn scene() -> Scene {
    let fixture = Fixture::new();
    let one = fixture.workspace("one", "feat1");
    let two = fixture.workspace("two", "feat2");
    let commit = captured_member_commit(&one, "one", 1, 40);
    // The second workspace's thread names the commit by hand, with its member; the commit is not
    // in its branch, so the link is the only way it reaches this lookup.
    let mut session = Session::open(&two, "two");
    session.begin(30);
    drop(session);
    link_by_hand(&two, "two", &commit, Some("billing"));
    Scene {
        fixture,
        one,
        two,
        commit,
    }
}

#[test]
fn the_commit_lookup_lists_the_workspace_threads_marked_with_their_workspace() {
    let scene = scene();
    let threads = list(&scene.fixture.repo, &scene.commit, None, 10);

    assert_eq!(uids(&threads), [uid("one"), uid("two")]);
    assert_eq!(
        threads[0].workspace.as_deref(),
        Some(scene.one.to_str().unwrap()),
        "the workspace folder, as the pointer file names it"
    );
    assert_eq!(
        threads[1].workspace.as_deref(),
        Some(scene.two.to_str().unwrap())
    );
}

#[test]
fn workspace_threads_merge_with_the_local_ones_by_time_and_the_page_size_holds() {
    let scene = scene();
    let repo = scene.fixture.repo.clone();
    let mut local = Session::open(&repo, "local-new");
    local.begin(35);
    drop(local);
    link_by_hand(&repo, "local-new", &scene.commit, None);

    let all = list(&repo, &scene.commit, None, 10);
    assert_eq!(
        uids(&all),
        [uid("one"), uid("local-new"), uid("two")],
        "newest first, local and workspace together"
    );
    assert_eq!(all[1].workspace, None, "a local thread names no workspace");

    let page = list(&repo, &scene.commit, None, 2);
    assert_eq!(
        uids(&page),
        [uid("one"), uid("local-new")],
        "the page keeps its size; the older workspace thread is not on it"
    );
}

#[test]
fn a_later_page_holds_only_the_local_threads() {
    let scene = scene();
    let repo = scene.fixture.repo.clone();
    let mut older = Session::open(&repo, "local-old");
    older.begin(5);
    drop(older);
    let mut newer = Session::open(&repo, "local-new");
    newer.begin(35);
    drop(newer);
    link_by_hand(&repo, "local-old", &scene.commit, None);
    link_by_hand(&repo, "local-new", &scene.commit, None);

    let page = list(&repo, &scene.commit, Some((35, uid("local-new"))), 10);
    assert_eq!(
        uids(&page),
        [uid("local-old")],
        "the workspace threads, which are older than the cursor, are on the first page only"
    );
}

#[test]
fn a_missing_workspace_and_one_of_another_schema_are_skipped() {
    let scene = scene();
    let fixture = &scene.fixture;
    // A third workspace with a thread on the commit, whose store is then given another schema.
    let root_text = fixture.root().to_str().unwrap().to_owned();
    let (code, _, err) = fixture.cli(&["create", "three", "--root", &root_text]);
    assert_eq!(code, 0, "create failed: {err}");
    let three = fixture.root().join("three");
    let mut session = Session::open(&three, "three");
    session.begin(20);
    drop(session);
    link_by_hand(&three, "three", &scene.commit, Some("billing"));
    let store_path = three.join(".git").join("gitmaster").join("threads.db");
    {
        let conn = rusqlite::Connection::open(&store_path).expect("open");
        conn.pragma_update(None, "user_version", 99).expect("bump");
    }
    // A workspace folder that no longer exists is still listed in the pointer file.
    let ghost = fixture.base.join("gone");
    let mut pointers = fs::OpenOptions::new()
        .append(true)
        .open(fixture.pointers())
        .expect("pointer file");
    writeln!(pointers, "{}", ghost.display()).expect("write");
    writeln!(pointers, "{}", three.display()).expect("write");

    let threads = list(&fixture.repo, &scene.commit, None, 10);
    assert_eq!(uids(&threads), [uid("one"), uid("two")]);
    assert!(!ghost.exists(), "a missing workspace is not created");
    let conn = rusqlite::Connection::open(&store_path).expect("open");
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, 99, "another schema is not migrated");
}

#[test]
fn a_commit_no_member_of_a_workspace_links_finds_no_workspace_thread() {
    let scene = scene();
    let head = git(&scene.fixture.repo, &["rev-parse", "main"]);
    assert_ne!(head, scene.commit);
    assert!(list(&scene.fixture.repo, &head, None, 10).is_empty());
}
