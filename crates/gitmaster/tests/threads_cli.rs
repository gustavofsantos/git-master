//! Runs the built `gitmaster` binary for the one-shot thread and harness commands
//! (docs/reference/cli.md, design/threads.md "The hook command (D124)").

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gm_core::threads::store::ThreadStore;
use gm_schema::threads::ThreadFilter;

const BINARY: &str = env!("CARGO_BIN_EXE_gitmaster");

/// Claude Code hook payloads from the harness fixtures; `cwd` is replaced by the test repository.
const SESSION_START: &str = "session_start_startup.hook";
const PRE_EDIT: &str = "pre_tool_use_edit.hook";
const POST_EDIT: &str = "post_tool_use_edit.hook";

/// A scratch directory that is removed when the test ends.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let root =
            std::env::temp_dir().join(format!("gm-threads-cli-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).expect("scratch dir");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    /// A git repository with one commit-less worktree under the scratch directory.
    fn repo(&self) -> PathBuf {
        let repo = self.root.join("repo");
        fs::create_dir_all(&repo).expect("repo dir");
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .expect("git init");
        assert!(status.success(), "git init failed");
        repo
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A fixture payload with its `cwd` pointed at `repo`.
fn payload(name: &str, repo: &Path) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../gm-core/tests/fixtures/threads/claude-code")
        .join(name);
    let text = fs::read_to_string(&path).expect("fixture");
    text.replace(
        "\"cwd\":\"/home/dev/repo\"",
        &format!("\"cwd\":\"{}\"", repo.display()),
    )
    .into_bytes()
}

/// Runs `gitmaster <args>` in `dir` with the home and XDG directories under `home`, feeding
/// `stdin`.
fn gitmaster(args: &[&str], stdin: &[u8], dir: &Path, home: &Path) -> Output {
    let mut child = Command::new(BINARY)
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn gitmaster");
    let mut input = child.stdin.take().expect("stdin");
    input.write_all(stdin).expect("write stdin");
    drop(input);
    child.wait_with_output().expect("wait for gitmaster")
}

fn hook(stdin: &[u8], dir: &Path, home: &Path) -> Output {
    gitmaster(
        &["thread", "hook", "--harness", "claude-code"],
        stdin,
        dir,
        home,
    )
}

/// Asserts the hook contract: exit `0`, nothing on standard output.
fn assert_silent_success(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
}

#[test]
fn the_hook_stores_a_session_and_prints_nothing() {
    let scratch = Scratch::new();
    let repo = scratch.repo();
    let home = scratch.path().join("home");

    let started = Instant::now();
    let first = hook(&payload(SESSION_START, &repo), &repo, &home);
    let elapsed = started.elapsed();
    assert_silent_success(&first);
    hook(&payload(PRE_EDIT, &repo), &repo, &home);
    let last = hook(&payload(POST_EDIT, &repo), &repo, &home);
    assert_silent_success(&last);
    eprintln!("one hook run took {elapsed:?} (wall time, debug build)");

    let db = repo.join(".git/gitmaster/threads.db");
    assert!(db.exists(), "no thread database at {}", db.display());
    let store = ThreadStore::open(&repo.join(".git")).expect("open store");
    let threads = store
        .list_threads(&ThreadFilter::default(), None, 10)
        .expect("list threads");
    assert_eq!(threads.len(), 1);
}

#[test]
fn the_hook_exits_zero_and_silent_on_bad_input() {
    let scratch = Scratch::new();
    let repo = scratch.repo();
    let outside = scratch.path().join("outside");
    fs::create_dir_all(&outside).expect("outside dir");
    let home = scratch.path().join("home");

    assert_silent_success(&hook(b"this is not json", &repo, &home));
    assert_silent_success(&hook(b"", &repo, &home));
    assert_silent_success(&hook(&payload(SESSION_START, &outside), &outside, &home));
    let unknown = gitmaster(
        &["thread", "hook", "--harness", "no-such-agent"],
        &payload(SESSION_START, &repo),
        &repo,
        &home,
    );
    assert_silent_success(&unknown);
    assert!(!repo.join(".git/gitmaster/threads.db").exists());
    let log = fs::read_to_string(repo.join(".git/gitmaster/threads.log")).expect("capture log");
    assert!(log.contains("unknown harness 'no-such-agent'"), "{log}");
}

#[test]
fn harness_list_names_the_claude_code_adapter() {
    let scratch = Scratch::new();
    let home = scratch.path().join("home");
    let output = gitmaster(&["harness", "list"], b"", scratch.path(), &home);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("claude-code"), "{stdout}");
}
