//! Threads in a workspace (D180) against real repositories in temporary directories: a workspace
//! whose member is a linked worktree of the user's clone, as `GitMaster` creates it. The fake
//! harness stands in for the agent, run at the workspace root; the commits are real.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::threads::config::ThreadsConfig;
use gm_core::threads::harness::fake::{self, FakeAdapter};
use gm_core::threads::pipeline::{Context, Outcome, ingest, locate_repo};
use gm_core::threads::store::ThreadStore;
use gm_schema::threads::{EditState, LinkKind, LinkOrigin, ThreadDetail};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId};
use serde_json::{Value, json};

const NOW: i64 = 1_700_000_000_000;
const SESSION: &str = "session-1";

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

/// The blob id `git hash-object` gives the bytes.
fn blob(text: &str) -> String {
    let file = tempfile::NamedTempFile::new().expect("temp file");
    fs::write(file.path(), text).expect("write");
    let path = file.path().to_str().expect("utf-8 path").to_owned();
    git(&std::env::temp_dir(), &["hash-object", path.as_str()])
}

/// A workspace `ws` whose member `billing` is a linked worktree of the clone `repo`, on branch
/// `feat`, registered as a submodule. `billing/src/x.rs` holds "one\n" and is committed in the
/// member, so the member has a HEAD before any session starts.
struct Workspace {
    _dir: tempfile::TempDir,
    ws: PathBuf,
    billing: PathBuf,
}

fn workspace() -> Workspace {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path().join("repo");
    fs::create_dir(&repo).expect("repo");
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("a.txt"), "clone\n").expect("write");
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(&repo, &["branch", "feat"]);

    let ws = dir.path().join("ws");
    fs::create_dir(&ws).expect("ws");
    git(&ws, &["init", "-q", "-b", "main"]);
    fs::write(ws.join("README.md"), "workspace\n").expect("write");
    git(&ws, &["add", "README.md"]);
    git(&ws, &["commit", "-q", "-m", "init"]);

    let billing = ws.join("billing");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            billing.to_str().expect("utf-8"),
            "feat",
        ],
    );
    fs::create_dir_all(billing.join("src")).expect("src");
    fs::write(billing.join("src/x.rs"), "one\n").expect("write");
    git(&billing, &["add", "src/x.rs"]);
    git(&billing, &["commit", "-q", "-m", "base"]);
    git(
        &ws,
        &[
            "submodule",
            "add",
            "-q",
            "-b",
            "feat",
            repo.to_str().expect("utf-8"),
            "billing",
        ],
    );
    git(&ws, &["commit", "-q", "-m", "add billing"]);
    Workspace {
        _dir: dir,
        ws,
        billing,
    }
}

/// One session, fed payload by payload like the hook, from the workspace root.
struct Session {
    dir: PathBuf,
    store: ThreadStore,
    config: ThreadsConfig,
}

impl Session {
    fn open(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            store: ThreadStore::open(&dir.join(".git")).expect("open store"),
            config: ThreadsConfig::default(),
        }
    }

    fn send(&mut self, value: &Value) -> Outcome {
        let bytes = serde_json::to_vec(value).expect("json");
        let repo = locate_repo(&self.dir).expect("inside the repository");
        let ctx = Context {
            config: &self.config,
            now_ms: NOW,
        };
        ingest(&FakeAdapter, &bytes, &repo, &ctx, &mut self.store)
    }

    fn detail(&self) -> ThreadDetail {
        self.store
            .read_thread(&ThreadStore::thread_uid(fake::ID, SESSION), 0, 10_000)
            .expect("read thread")
    }
}

fn start(dir: &Path) -> Value {
    json!({"event": "session_start", "session": SESSION, "id": "start", "at": 1,
        "cwd": dir.display().to_string()})
}

fn prompt(id: &str, at: i64, text: &str) -> Value {
    json!({"event": "prompt", "session": SESSION, "id": id, "at": at, "text": text})
}

fn call(id: &str, at: i64, tool: &str, args: &Value) -> Value {
    json!({"event": "tool_call", "session": SESSION, "id": id, "at": at, "tool": tool, "args": args})
}

fn result(id: &str, at: i64, call: &str, text: &str) -> Value {
    json!({"event": "tool_result", "session": SESSION, "id": id, "at": at, "call": call, "text": text})
}

fn turn_end(id: &str, at: i64) -> Value {
    json!({"event": "turn_end", "session": SESSION, "id": id, "at": at})
}

fn edit_phase(event: &str, call: &str, path: &str) -> Value {
    json!({"event": event, "session": SESSION, "call": call, "path": path})
}

/// The agent edits `path` from `old` to `new` through its edit tool, as one call.
fn edit(session: &mut Session, dir: &Path, call_id: &str, path: &str, old: &str, new: &str) {
    session.send(&call(
        call_id,
        3,
        "edit",
        &json!({"path": path, "old": old, "new": new}),
    ));
    session.send(&edit_phase("edit_before", call_id, path));
    fs::write(dir.join(path), new).expect("edit the file");
    session.send(&edit_phase("edit_after", call_id, path));
    session.send(&result(&format!("r-{call_id}"), 4, call_id, "edited"));
}

/// The state of the edit `id`, as the engine reads it from the workspace root.
fn edit_state(root: &Path, id: i64) -> EditState {
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
                path: root.to_string_lossy().into_owned(),
            },
        },
    );
    let events = engine.handle(
        OperationId(2),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::ReadThreadEdit { id },
        },
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::ThreadEditRead { state, .. } => Some(*state),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no edit read: {events:?}"))
}

#[test]
fn an_edit_inside_a_member_names_the_member_and_keeps_its_workspace_path() {
    let ws = workspace();
    let mut session = Session::open(&ws.ws);
    session.send(&start(&ws.ws));
    session.send(&prompt("u1", 2, "change the billing code"));
    edit(
        &mut session,
        &ws.ws,
        "t1",
        "billing/src/x.rs",
        "one\n",
        "two\n",
    );
    session.send(&turn_end("e1", 5));

    let detail = session.detail();
    assert_eq!(detail.edits.len(), 1);
    let edit = &detail.edits[0];
    assert_eq!(edit.path, "billing/src/x.rs");
    assert_eq!(edit.member.as_deref(), Some("billing"));
    assert_eq!(edit.post_oid.as_deref(), Some(blob("two\n").as_str()));
}

#[test]
fn a_member_edit_reads_its_state_from_the_member_head() {
    let ws = workspace();
    let mut session = Session::open(&ws.ws);
    session.send(&start(&ws.ws));
    session.send(&prompt("u1", 2, "change the billing code"));
    edit(
        &mut session,
        &ws.ws,
        "t1",
        "billing/src/x.rs",
        "one\n",
        "two\n",
    );
    session.send(&turn_end("e1", 5));
    let id = session.detail().edits[0].id;

    assert_eq!(
        edit_state(&ws.ws, id),
        EditState::InWorkingTree,
        "the file still holds the edit"
    );
    git(&ws.billing, &["commit", "-q", "-am", "two"]);
    fs::write(ws.billing.join("src/x.rs"), "three\n").expect("write");
    assert_eq!(
        edit_state(&ws.ws, id),
        EditState::InHead,
        "the edit is committed in the member and the file has moved on"
    );
}

#[test]
fn commits_in_a_member_are_linked_to_the_member_and_workspace_commits_are_not() {
    let ws = workspace();
    let mut session = Session::open(&ws.ws);
    session.send(&start(&ws.ws));
    session.send(&prompt("u1", 2, "start"));

    fs::write(ws.billing.join("y.rs"), "fn y() {}\n").expect("write");
    git(&ws.billing, &["add", "y.rs"]);
    git(&ws.billing, &["commit", "-q", "-m", "member work"]);
    let member_commit = git(&ws.billing, &["rev-parse", "HEAD"]);
    session.send(&turn_end("e1", 3));

    fs::write(ws.ws.join("notes.md"), "notes\n").expect("write");
    git(&ws.ws, &["add", "notes.md"]);
    git(&ws.ws, &["commit", "-q", "-m", "workspace work"]);
    let workspace_commit = git(&ws.ws, &["rev-parse", "HEAD"]);
    session.send(&turn_end("e2", 4));

    let detail = session.detail();
    let link = |reference: &str| {
        detail
            .links
            .iter()
            .find(|link| link.kind == LinkKind::Commit && link.reference == reference)
            .unwrap_or_else(|| panic!("no commit link for {reference}: {:?}", detail.links))
    };
    let linked = link(&member_commit);
    assert_eq!(linked.member.as_deref(), Some("billing"));
    assert_eq!(linked.origin, LinkOrigin::Observed);
    assert_eq!(link(&workspace_commit).member, None);

    let heads = session
        .store
        .member_heads(&ThreadStore::thread_uid(fake::ID, SESSION))
        .expect("heads");
    assert_eq!(heads.len(), 1);
    assert_eq!(heads[0].member, "billing");
    assert_ne!(heads[0].head_start, heads[0].head_end);
    assert_eq!(heads[0].head_end.as_deref(), Some(member_commit.as_str()));
}

#[test]
fn a_repository_without_gitmodules_records_no_member() {
    let dir = tempfile::tempdir().expect("tempdir");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    fs::write(dir.path().join("a.txt"), "one\n").expect("write");
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    let mut session = Session::open(dir.path());
    session.send(&start(dir.path()));
    session.send(&prompt("u1", 2, "change a.txt"));
    session.send(&call(
        "t1",
        3,
        "edit",
        &json!({"path": "a.txt", "old": "one\n", "new": "two\n"}),
    ));
    session.send(&edit_phase("edit_before", "t1", "a.txt"));
    fs::write(dir.path().join("a.txt"), "two\n").expect("edit the file");
    session.send(&edit_phase("edit_after", "t1", "a.txt"));
    session.send(&result("r1", 4, "t1", "edited"));
    git(dir.path(), &["commit", "-q", "-am", "change a"]);
    session.send(&turn_end("e1", 5));

    let detail = session.detail();
    assert_eq!(detail.edits[0].member, None);
    assert!(detail.links.iter().all(|link| link.member.is_none()));
    let heads = session
        .store
        .member_heads(&ThreadStore::thread_uid(fake::ID, SESSION))
        .expect("heads");
    assert!(heads.is_empty());
}
