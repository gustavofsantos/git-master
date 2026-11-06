//! The agent's reads in a workspace (D181) against real repositories in temporary directories: a
//! workspace whose members are linked worktrees of one clone, as `GitMaster` creates them. A thread
//! is captured at the workspace root and its commit is made in a member; `ReadProvenance` on a
//! member's path blames in the member and finds the thread in the workspace's store.

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
use gm_schema::threads::ProvenanceResult;
use gm_schema::{Actor, Command, CommandKind, Event, Evidence, OperationId, OperationState};
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
    repo: PathBuf,
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
        repo,
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

/// An engine on the repository at `root`, opened as the agent.
fn engine_over(root: &Path) -> Engine {
    let git = SubprocessGit {
        log: Some(CommandLog::default()),
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    let opened = engine.handle(
        OperationId(1),
        &agent(CommandKind::OpenRepo {
            path: root.to_string_lossy().into_owned(),
        }),
    );
    assert!(
        !opened.iter().any(|event| matches!(
            event,
            Event::OperationChanged {
                state: OperationState::Failed,
                ..
            }
        )),
        "open failed: {opened:?}"
    );
    engine
}

fn agent(kind: CommandKind) -> Command {
    Command {
        actor: Actor::Agent {
            name: "test-agent".into(),
        },
        kind,
    }
}

/// The answer of one `ReadProvenance`, with its blame run to its end as the server runs it.
fn provenance(engine: &mut Engine, id: u64, path: &str, from: u32, to: u32) -> ProvenanceResult {
    let mut events = engine.handle(
        OperationId(id),
        &agent(CommandKind::ReadProvenance {
            path: path.to_owned(),
            from_line: from,
            to_line: to,
        }),
    );
    for job in engine.take_provenance_jobs() {
        events.extend(engine.provenance_report(job.run()));
    }
    events
        .into_iter()
        .find_map(|event| match event {
            Event::ThreadProvenance { provenance, .. } => Some(provenance),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no provenance answer"))
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).expect("canonical path")
}

#[test]
fn a_line_in_a_member_is_explained_by_the_thread_that_edited_it() {
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
    git(&ws.billing, &["commit", "-q", "-am", "two"]);
    let member_commit = git(&ws.billing, &["rev-parse", "HEAD"]);
    session.send(&turn_end("e1", 5));
    let uid = ThreadStore::thread_uid(fake::ID, SESSION);

    let mut engine = engine_over(&ws.ws);
    let answer = provenance(&mut engine, 2, "billing/src/x.rs", 1, 1);

    assert!(answer.cuts.is_empty(), "{:?}", answer.cuts);
    assert_eq!(answer.commits.len(), 1, "{:?}", answer.commits);
    let commit = &answer.commits[0];
    assert_eq!(commit.id, member_commit);
    assert_eq!(commit.subject, "two");
    assert_eq!(
        commit.path, "billing/src/x.rs",
        "the path names the workspace, not the member"
    );
    let thread = commit
        .threads
        .iter()
        .find(|thread| thread.uid == uid)
        .unwrap_or_else(|| panic!("no thread {uid} for the member commit: {commit:?}"));
    assert!(
        thread.evidence.contains(&Evidence::BlobMatch),
        "the file at the member commit is what the edit wrote: {:?}",
        thread.evidence
    );
    assert_eq!(thread.edits.len(), 1);
    assert_eq!(thread.edits[0].path, "billing/src/x.rs");
    assert!(thread.edits[0].evidence.contains(&Evidence::BlobMatch));
    assert_eq!(
        blob("two\n"),
        git(&ws.billing, &["rev-parse", "HEAD:src/x.rs"])
    );
}

#[test]
fn a_file_outside_the_members_is_read_from_the_workspace_as_before() {
    let ws = workspace();
    let mut engine = engine_over(&ws.ws);

    let answer = provenance(&mut engine, 2, "README.md", 1, 1);

    assert_eq!(answer.commits.len(), 1, "{:?}", answer.commits);
    assert_eq!(answer.commits[0].path, "README.md");
    assert_eq!(answer.commits[0].subject, "init");
    assert!(answer.commits[0].threads.is_empty());
}

#[test]
fn the_members_are_listed_and_each_names_its_main_checkout() {
    let ws = workspace();
    let mut engine = engine_over(&ws.ws);

    let listed = engine.handle(OperationId(2), &agent(CommandKind::ListMembers));
    let members = listed
        .iter()
        .find_map(|event| match event {
            Event::MembersListed { members, .. } => Some(members.clone()),
            _ => None,
        })
        .expect("the members are listed");
    assert_eq!(members, ["billing"]);

    let loaded = engine.handle(
        OperationId(3),
        &agent(CommandKind::LoadSubmodule {
            path: "billing".into(),
        }),
    );
    let submodule = loaded
        .iter()
        .find_map(|event| match event {
            Event::SubmoduleLoaded { submodule, .. } => Some(submodule.clone()),
            _ => None,
        })
        .expect("the member is loaded");
    let repo = submodule
        .repo
        .expect("a checked-out member names its repository");
    assert_eq!(canonical(Path::new(&repo)), canonical(&ws.repo));
    assert!(submodule.checked_out);
}

#[test]
fn a_repository_without_members_lists_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    fs::write(dir.path().join("a.txt"), "a\n").expect("write");
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    let mut engine = engine_over(dir.path());

    let listed = engine.handle(OperationId(2), &agent(CommandKind::ListMembers));

    assert!(
        listed.iter().any(
            |event| matches!(event, Event::MembersListed { members, .. } if members.is_empty())
        ),
        "{listed:?}"
    );
}
