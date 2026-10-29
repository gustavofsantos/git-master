//! The capture pipeline against real Git repositories in temporary directories
//! (design/threads.md, "Tests"). The fake harness stands in for an agent; the repository,
//! its working tree and its commits are real.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gm_core::threads::config::ThreadsConfig;
use gm_core::threads::harness::fake::{self, FakeAdapter};
use gm_core::threads::pipeline::{Context, DropReason, Outcome, ingest, locate_repo};
use gm_core::threads::store::ThreadStore;
use gm_schema::threads::{EditChange, EntryKind, LinkKind, LinkOrigin, ThreadDetail};
use serde_json::{Value, json};

const NOW: i64 = 1_700_000_000_000;
const SESSION: &str = "session-1";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
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

/// A repository on `main` with one commit that holds `a.txt` ("one\n").
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    fs::write(dir.path().join("a.txt"), "one\n").expect("write");
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

/// The blob id `git hash-object` gives the bytes.
fn git_hash(bytes: &[u8]) -> String {
    let mut child = Command::new("git")
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn git");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(bytes)
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait for git");
    String::from_utf8(output.stdout)
        .expect("utf-8")
        .trim()
        .to_owned()
}

fn read_if_exists(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_default()
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// One session in one repository, fed payload by payload like the hook.
struct Session {
    dir: PathBuf,
    store: ThreadStore,
    config: ThreadsConfig,
    /// Every payload sent, in order, for replays.
    sent: Vec<Vec<u8>>,
}

impl Session {
    fn open(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            store: ThreadStore::open(&dir.join(".git")).expect("open store"),
            config: ThreadsConfig::default(),
            sent: Vec::new(),
        }
    }

    fn send(&mut self, value: &Value) -> Outcome {
        let bytes = serde_json::to_vec(value).expect("json");
        self.send_bytes(&bytes)
    }

    fn send_bytes(&mut self, bytes: &[u8]) -> Outcome {
        self.sent.push(bytes.to_vec());
        let repo = locate_repo(&self.dir).expect("inside the repository");
        let ctx = Context {
            config: &self.config,
            now_ms: NOW,
        };
        ingest(&FakeAdapter, bytes, &repo, &ctx, &mut self.store)
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

fn call(id: &str, at: i64, tool: &str, args: Value) -> Value {
    let mut payload =
        json!({"event": "tool_call", "session": SESSION, "id": id, "at": at, "tool": tool});
    payload["args"] = args;
    payload
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

#[test]
fn a_session_with_an_edit_is_stored_with_its_patch_blob_ids_and_links() {
    let dir = repo();
    let mut session = Session::open(dir.path());
    session.send(&start(dir.path()));
    session.send(&prompt("u1", 2, "change a.txt"));
    session.send(&call(
        "t1",
        3,
        "edit",
        json!({"path": "a.txt", "old": "one\n", "new": "two\n"}),
    ));
    session.send(&edit_phase("edit_before", "t1", "a.txt"));
    fs::write(dir.path().join("a.txt"), "two\n").expect("edit the file");
    session.send(&edit_phase("edit_after", "t1", "a.txt"));
    session.send(&result("r1", 4, "t1", "edited"));
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), " M a.txt");
    git(dir.path(), &["commit", "-q", "-am", "change a"]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    assert!(matches!(
        session.send(&turn_end("e1", 5)),
        Outcome::Stored { .. }
    ));

    let detail = session.detail();
    let kinds: Vec<EntryKind> = detail.entries.iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        vec![EntryKind::User, EntryKind::ToolCall, EntryKind::ToolResult]
    );
    assert_eq!(detail.entries[1].tool.as_deref(), Some("edit"));
    assert_eq!(detail.entries[2].tool.as_deref(), Some("edit"));

    assert_eq!(detail.edits.len(), 1);
    let edit = &detail.edits[0];
    assert_eq!(edit.path, "a.txt");
    assert_eq!(edit.change, EditChange::Modify);
    assert!(!edit.from_input, "the measured edit replaces the input one");
    assert_eq!((edit.added, edit.removed), (1, 1));
    assert!(edit.patch.contains("-one\n"), "{}", edit.patch);
    assert!(edit.patch.contains("+two\n"), "{}", edit.patch);
    assert_eq!(edit.pre_oid.as_deref(), Some(git_hash(b"one\n").as_str()));
    assert_eq!(edit.post_oid.as_deref(), Some(git_hash(b"two\n").as_str()));

    let links = &detail.links;
    assert!(links.iter().any(|link| link.kind == LinkKind::File
        && link.reference == "a.txt"
        && link.origin == LinkOrigin::Observed));
    assert!(links.iter().any(|link| link.kind == LinkKind::Commit
        && link.reference == head
        && link.origin == LinkOrigin::Observed));
    assert!(
        links
            .iter()
            .any(|link| link.kind == LinkKind::Branch && link.reference == "main")
    );

    assert_eq!(
        git(dir.path(), &["status", "--porcelain"]),
        "",
        "the capture left the working tree as the test left it"
    );
    let mut names: Vec<String> = fs::read_dir(dir.path())
        .expect("list worktree")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(names, vec![".git".to_owned(), "a.txt".to_owned()]);
    assert!(dir.path().join(".git/gitmaster/threads.db").exists());
}

#[test]
fn secrets_never_reach_the_database_file_the_spool_or_the_log() {
    let dir = repo();
    let common = dir.path().join(".git");
    let token = ["ghp_", &"a".repeat(36)].concat();
    let db = common.join("gitmaster").join("threads.db");
    let mut session = Session::open(dir.path());

    session.send(&start(dir.path()));
    session.send(&prompt("u1", 2, &format!("use {token} for the deploy")));
    session.send(&call(
        "t1",
        3,
        "bash",
        json!({"command": format!("curl -H 'X-Key: {token}' example.test")}),
    ));

    // The database is locked by another connection, so the result waits in the spool.
    let blocker = rusqlite::Connection::open(&db).expect("second connection");
    blocker.execute_batch("BEGIN IMMEDIATE").expect("lock");
    let outcome = session.send(&result("r1", 4, "t1", &format!("printed {token}")));
    assert_eq!(outcome, Outcome::Spooled);
    let spooled: Vec<Vec<u8>> = fs::read_dir(common.join("gitmaster/threads-spool"))
        .expect("spool dir")
        .map(|entry| fs::read(entry.expect("entry").path()).expect("spool file"))
        .collect();
    assert_eq!(spooled.len(), 1);
    assert!(
        spooled.iter().all(|bytes| !contains(bytes, &token)),
        "the spool holds the secret"
    );
    blocker.execute_batch("COMMIT").expect("release");
    drop(blocker);

    let new_text = format!("one\n{token}\n");
    session.send(&call(
        "t2",
        5,
        "edit",
        json!({"path": "a.txt", "old": "one\n", "new": new_text}),
    ));
    session.send(&edit_phase("edit_before", "t2", "a.txt"));
    fs::write(dir.path().join("a.txt"), &new_text).expect("edit");
    session.send(&edit_phase("edit_after", "t2", "a.txt"));
    session.send(&result("r2", 6, "t2", "ok"));

    // Not JSON: the log records that a payload was refused, never what it said.
    let broken = format!("{{\"event\": \"prompt\", \"text\": \"{token}\"");
    assert_eq!(
        session.send_bytes(broken.as_bytes()),
        Outcome::Dropped(DropReason::Unparseable)
    );

    let detail = session.detail();
    assert_eq!(
        detail.entries[0].text,
        "use [REDACTED:github-token] for the deploy"
    );
    assert!(
        detail.summary.redaction_count >= 4,
        "{:?}",
        detail.redactions
    );
    session.store.checkpoint().expect("checkpoint");

    let gitmaster = common.join("gitmaster");
    for file in [
        read_if_exists(&db),
        read_if_exists(&gitmaster.join("threads.db-wal")),
        read_if_exists(&gitmaster.join("threads.log")),
    ] {
        assert!(!contains(&file, &token), "a stored file holds the secret");
    }
    let spool_left = fs::read_dir(gitmaster.join("threads-spool"))
        .expect("spool dir")
        .count();
    assert_eq!(spool_left, 0, "the next run stored the spooled result");
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), " M a.txt");
}

#[test]
fn replaying_the_whole_sequence_twice_changes_nothing() {
    let dir = repo();
    let mut session = Session::open(dir.path());
    session.send(&start(dir.path()));
    session.send(&prompt("u1", 2, "change a.txt"));
    session.send(&call(
        "t1",
        3,
        "edit",
        json!({"path": "a.txt", "old": "one\n", "new": "two\n"}),
    ));
    session.send(&edit_phase("edit_before", "t1", "a.txt"));
    fs::write(dir.path().join("a.txt"), "two\n").expect("edit");
    session.send(&edit_phase("edit_after", "t1", "a.txt"));
    session.send(&result("r1", 4, "t1", "edited"));
    git(dir.path(), &["commit", "-q", "-am", "change a"]);
    session.send(&turn_end("e1", 5));

    let first = session.detail();
    let sent = session.sent.clone();
    for pass in 0..2 {
        for bytes in &sent {
            session.send_bytes(bytes);
        }
        assert_eq!(session.detail(), first, "after replay {pass}");
    }
    assert_eq!(first.summary.entry_count, 3);
    assert_eq!(first.summary.edit_count, 1);
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
}

#[test]
fn an_invalid_pattern_stores_only_a_gap_and_capture_off_stores_nothing() {
    let dir = repo();
    let mut session = Session::open(dir.path());
    session.config = ThreadsConfig::from_settings(&json!({
        "threads": {"redact": {"patterns": [{"name": "acme", "regex": "acme_["}]}}
    }));
    let token = ["ghp_", &"b".repeat(36)].concat();
    let outcome = session.send(&prompt("u1", 1, &format!("secret {token}")));
    assert_eq!(
        outcome,
        Outcome::Dropped(DropReason::InvalidPattern(vec!["acme".to_owned()]))
    );
    let detail = session.detail();
    assert!(detail.entries.is_empty());
    let gaps = session
        .store
        .gaps(&ThreadStore::thread_uid(fake::ID, SESSION))
        .expect("gaps");
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].reason, "invalid redaction pattern: acme");
    session.store.checkpoint().expect("checkpoint");
    assert!(!contains(
        &read_if_exists(&dir.path().join(".git/gitmaster/threads.db")),
        &token
    ));

    let other = repo();
    let mut off = Session::open(other.path());
    off.config = ThreadsConfig::from_settings(&json!({"threads": {"capture": false}}));
    assert_eq!(
        off.send(&prompt("u2", 2, "nothing")),
        Outcome::Dropped(DropReason::CaptureOff)
    );
    assert_eq!(off.store.total_threads().expect("count"), 0);
}

#[test]
fn locate_repo_reads_the_branch_and_head_and_shares_the_store_across_worktrees() {
    let dir = repo();
    let main = locate_repo(dir.path()).expect("repository");
    assert_eq!(
        fs::canonicalize(&main.common_dir).expect("common"),
        fs::canonicalize(dir.path().join(".git")).expect("git dir")
    );
    assert_eq!(main.branch.as_deref(), Some("main"));
    assert_eq!(
        main.head.as_deref(),
        Some(git(dir.path(), &["rev-parse", "HEAD"]).as_str())
    );

    let linked_root = tempfile::tempdir().expect("tempdir");
    let linked = linked_root.path().join("linked");
    git(
        dir.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            linked.to_str().expect("utf-8"),
        ],
    );
    let other = locate_repo(&linked).expect("linked worktree");
    assert_eq!(other.common_dir, main.common_dir);
    assert_eq!(other.branch.as_deref(), Some("feature"));

    git(dir.path(), &["checkout", "-q", "--detach"]);
    let detached = locate_repo(dir.path()).expect("detached");
    assert_eq!(detached.branch, None);
    assert!(detached.head.is_some());

    let unborn_root = tempfile::tempdir().expect("tempdir");
    git(unborn_root.path(), &["init", "-q", "-b", "trunk"]);
    let unborn = locate_repo(unborn_root.path()).expect("unborn");
    assert_eq!(unborn.head, None);
    assert_eq!(unborn.branch.as_deref(), Some("trunk"));

    let outside = tempfile::tempdir().expect("tempdir");
    assert_eq!(locate_repo(outside.path()), None);
}

#[test]
fn creating_deleting_and_binary_edits_are_measured_against_the_working_tree() {
    let dir = repo();
    let mut session = Session::open(dir.path());
    session.send(&start(dir.path()));
    session.send(&prompt("u1", 2, "add, remove and rewrite files"));

    // Create: the file does not exist before the call.
    session.send(&call(
        "c1",
        3,
        "write",
        json!({"path": "new.txt", "content": "hello\n"}),
    ));
    session.send(&edit_phase("edit_before", "c1", "new.txt"));
    fs::write(dir.path().join("new.txt"), "hello\n").expect("create");
    session.send(&edit_phase("edit_after", "c1", "new.txt"));

    // Delete: the same file, removed by the call.
    session.send(&call(
        "d1",
        4,
        "edit",
        json!({"path": "new.txt", "old": "hello\n", "new": ""}),
    ));
    session.send(&edit_phase("edit_before", "d1", "new.txt"));
    fs::remove_file(dir.path().join("new.txt")).expect("delete");
    session.send(&edit_phase("edit_after", "d1", "new.txt"));

    // Binary: a NUL byte makes the patch a note, with no counts.
    fs::write(dir.path().join("bin.dat"), [0_u8, 1, 2]).expect("binary");
    session.send(&call("b1", 5, "edit", json!({"path": "bin.dat"})));
    session.send(&edit_phase("edit_before", "b1", "bin.dat"));
    fs::write(dir.path().join("bin.dat"), [0_u8, 3]).expect("binary change");
    session.send(&edit_phase("edit_after", "b1", "bin.dat"));

    let detail = session.detail();
    assert_eq!(detail.edits.len(), 3, "{:?}", detail.edits);
    let created = &detail.edits[0];
    assert_eq!(created.change, EditChange::Create);
    assert_eq!(created.pre_oid, None);
    assert_eq!(
        created.post_oid.as_deref(),
        Some(git_hash(b"hello\n").as_str())
    );
    assert_eq!((created.added, created.removed), (1, 0));
    assert!(!created.from_input);

    let deleted = &detail.edits[1];
    assert_eq!(deleted.change, EditChange::Delete);
    assert_eq!(
        deleted.pre_oid.as_deref(),
        Some(git_hash(b"hello\n").as_str())
    );
    assert_eq!(deleted.post_oid, None);
    assert_eq!((deleted.added, deleted.removed), (0, 1));
    assert!(!deleted.from_input);

    let binary = &detail.edits[2];
    assert_eq!(binary.path, "bin.dat");
    assert_eq!(binary.patch, "Binary file changed\n");
    assert_eq!((binary.added, binary.removed), (0, 0));
    assert_eq!(
        binary.pre_oid.as_deref(),
        Some(git_hash(&[0_u8, 1, 2]).as_str())
    );
    assert_eq!(
        binary.post_oid.as_deref(),
        Some(git_hash(&[0_u8, 3]).as_str())
    );

    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "?? bin.dat");
}
