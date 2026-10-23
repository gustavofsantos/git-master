//! Moving the change of one file to a branch or a worktree (D109) through the
//! engine, against real repositories in temporary directories. Each test
//! removes its directory when it ends. The refusals must leave both sides as
//! they were; a race is made to happen with a `GitPort` that fires a hook once
//! at a chosen point and otherwise is Git as it is.

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::{DiscardOutcome, Engine, FileMerge, GitError, GitPort, TreeMerge};
use gm_schema::{
    Actor, BranchEntry, Command, CommandKind, Event, MoveOutcome, MoveTarget, OperationId,
    OperationState, RepoStatus, WorktreeEntry,
};

fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    Process::new("git")
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
        .unwrap()
}

/// The trimmed output of a successful git command.
fn git_in(dir: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git_bytes(dir, args))
        .trim()
        .to_owned()
}

/// The output of a successful git command, as bytes.
fn git_bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

/// The short status lines, sorted, with their leading spaces.
fn status_lines(dir: &Path) -> Vec<String> {
    let mut lines: Vec<String> =
        String::from_utf8_lossy(&git_bytes(dir, &["status", "--porcelain"]))
            .lines()
            .map(str::to_owned)
            .collect();
    lines.sort();
    lines
}

/// The move refs (D109) that exist in the repository.
fn move_refs(dir: &Path) -> Vec<String> {
    git_in(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/gitmaster/move/",
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn write(dir: &Path, name: &str, content: &[u8]) {
    let file = dir.join(name);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, content).unwrap();
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

/// A repository on `main` with one commit: `a.txt`, `multi.txt` (five lines),
/// `other.txt`, a binary `bin.dat` and `sub/file.txt`.
struct Fixture {
    _base: tempfile::TempDir,
    main: PathBuf,
}

fn fixture() -> Fixture {
    let base = tempfile::tempdir().unwrap();
    let main = base.path().join("repo");
    std::fs::create_dir(&main).unwrap();
    git_in(&main, &["init", "-q", "-b", "main"]);
    write(&main, "a.txt", b"one\n");
    write(&main, "multi.txt", b"1\n2\n3\n4\n5\n");
    write(&main, "other.txt", b"x\n");
    write(&main, "bin.dat", b"\0a");
    write(&main, "sub/file.txt", b"s\n");
    git_in(&main, &["add", "."]);
    git_in(&main, &["commit", "-q", "-m", "init"]);
    Fixture { _base: base, main }
}

/// A linked worktree at `<base>/<name>` on the new branch `branch`.
fn worktree(fixture: &Fixture, name: &str, branch: &str) -> PathBuf {
    let path = fixture.main.parent().unwrap().join(name);
    git_in(
        &fixture.main,
        &["worktree", "add", "-q", "-b", branch, &text(&path)],
    );
    path
}

/// A commit on `dev`, from HEAD of `main`, that changes `file` to `content`.
/// Leaves `main` checked out, with its working tree as it was.
fn commit_on_dev(main: &Path, file: &str, content: &[u8]) {
    git_in(main, &["checkout", "-q", "-b", "dev"]);
    write(main, file, content);
    git_in(main, &["commit", "-q", "-am", "dev change"]);
    git_in(main, &["checkout", "-q", "main"]);
}

fn engine_with(git: Box<dyn GitPort>, root: &Path) -> Engine {
    let mut engine = Engine::new(git);
    let events = engine.handle(
        OperationId(1),
        &human(CommandKind::OpenRepo { path: text(root) }),
    );
    assert!(done(&events), "open failed: {events:?}");
    engine
}

fn engine_on(root: &Path, log: &CommandLog) -> Engine {
    engine_with(
        Box::new(SubprocessGit {
            log: Some(log.clone()),
            ..SubprocessGit::default()
        }),
        root,
    )
}

fn human(kind: CommandKind) -> Command {
    Command {
        actor: Actor::Human,
        kind,
    }
}

fn done(events: &[Event]) -> bool {
    events.iter().any(|e| {
        matches!(
            e,
            Event::OperationChanged {
                state: OperationState::Done,
                ..
            }
        )
    })
}

/// The reason an operation failed with; panics when it did not fail.
fn failure(events: &[Event]) -> String {
    events
        .iter()
        .find_map(|e| match e {
            Event::OperationChanged {
                state: OperationState::Failed,
                detail,
                ..
            } => Some(detail.clone().unwrap_or_default()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the move did not fail: {events:?}"))
}

/// What a finished move did; panics when it did not finish.
fn outcome(events: &[Event]) -> MoveOutcome {
    assert!(done(events), "the move did not finish: {events:?}");
    events
        .iter()
        .find_map(|e| match e {
            Event::ChangesMoved { moved, .. } => Some(moved.clone()),
            _ => None,
        })
        .expect("the move is answered by ChangesMoved")
}

static NEXT_ID: AtomicU64 = AtomicU64::new(100);

/// Runs `MoveChanges` as the human, with a fresh operation id.
fn move_to(
    engine: &mut Engine,
    path: &str,
    target: MoveTarget,
    message: Option<&str>,
) -> (OperationId, Vec<Event>) {
    let id = OperationId(NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let events = engine.handle(
        id,
        &human(CommandKind::MoveChanges {
            path: path.to_owned(),
            target,
            message: message.map(str::to_owned),
        }),
    );
    (id, events)
}

fn branch(name: &str, create: bool) -> MoveTarget {
    MoveTarget::Branch {
        name: name.to_owned(),
        create,
    }
}

fn worktree_target(path: &Path) -> MoveTarget {
    MoveTarget::Worktree { path: text(path) }
}

/// The safety ref of operation `id` exists and holds `content` for `path`.
fn assert_safety_ref(repo: &Path, id: OperationId, path: &str, content: &[u8]) {
    let name = format!("refs/gitmaster/move/{}", id.0);
    assert_eq!(move_refs(repo), [name.as_str()], "the safety ref of {id:?}");
    assert_eq!(
        git_bytes(repo, &["show", &format!("{name}:{path}")]),
        content,
        "the safety ref holds the file as it was moved"
    );
}

/// A `GitPort` that is Git as it is, except that a hook runs once at a chosen
/// point: after the first `resolve_commit` (the tip a move merges onto) or
/// after a move ref is written (the snapshot is taken, nothing is written yet).
struct Racing {
    git: SubprocessGit,
    after_resolve: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    after_move_ref: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Racing {
    fn new(log: &CommandLog) -> Self {
        Self {
            git: SubprocessGit {
                log: Some(log.clone()),
                ..SubprocessGit::default()
            },
            after_resolve: Mutex::new(None),
            after_move_ref: Mutex::new(None),
        }
    }

    fn on_resolve(self, hook: impl FnOnce() + Send + 'static) -> Self {
        *self.after_resolve.lock().unwrap() = Some(Box::new(hook));
        self
    }

    fn on_move_ref(self, hook: impl FnOnce() + Send + 'static) -> Self {
        *self.after_move_ref.lock().unwrap() = Some(Box::new(hook));
        self
    }
}

fn fire(slot: &Mutex<Option<Box<dyn FnOnce() + Send>>>) {
    let hook = slot.lock().unwrap().take();
    if let Some(hook) = hook {
        hook();
    }
}

/// Moves the branch `branch` to a new commit on its own tip, as another
/// process would while the popover is open.
fn advance_branch(repo: &Path, branch: &str) {
    let tip = git_in(repo, &["rev-parse", branch]);
    let tree = git_in(repo, &["rev-parse", &format!("{tip}^{{tree}}")]);
    let commit = git_in(repo, &["commit-tree", &tree, "-p", &tip, "-m", "raced"]);
    git_in(
        repo,
        &["update-ref", &format!("refs/heads/{branch}"), &commit, &tip],
    );
}

impl GitPort for Racing {
    fn resolve_root(&self, path: &Path) -> Result<PathBuf, GitError> {
        self.git.resolve_root(path)
    }

    fn status(&self, root: &Path) -> Result<RepoStatus, GitError> {
        self.git.status(root)
    }

    fn worktrees(&self, root: &Path) -> Result<Vec<WorktreeEntry>, GitError> {
        self.git.worktrees(root)
    }

    fn branches(&self, root: &Path) -> Result<Vec<BranchEntry>, GitError> {
        self.git.branches(root)
    }

    fn git_version(&self) -> Result<String, GitError> {
        self.git.git_version()
    }

    fn snapshot_paths(&self, root: &Path, paths: &[String]) -> Result<String, GitError> {
        self.git.snapshot_paths(root, paths)
    }

    fn resolve_commit(&self, root: &Path, rev: &str) -> Result<String, GitError> {
        let commit = self.git.resolve_commit(root, rev);
        fire(&self.after_resolve);
        commit
    }

    fn merge_trees(
        &self,
        root: &Path,
        base: &str,
        ours: &str,
        theirs: &str,
    ) -> Result<TreeMerge, GitError> {
        self.git.merge_trees(root, base, ours, theirs)
    }

    fn commit_tree(
        &self,
        root: &Path,
        tree: &str,
        parent: Option<&str>,
        message: &str,
    ) -> Result<String, GitError> {
        self.git.commit_tree(root, tree, parent, message)
    }

    fn update_ref(
        &self,
        root: &Path,
        name: &str,
        new: &str,
        old: Option<&str>,
    ) -> Result<(), GitError> {
        let result = self.git.update_ref(root, name, new, old);
        if name.starts_with("refs/gitmaster/move/") {
            fire(&self.after_move_ref);
        }
        result
    }

    fn merge_file(
        &self,
        root: &Path,
        base: &[u8],
        ours: &[u8],
        theirs: &[u8],
    ) -> Result<FileMerge, GitError> {
        self.git.merge_file(root, base, ours, theirs)
    }

    fn discard_moved(
        &self,
        root: &Path,
        path: &str,
        snapshot: &str,
    ) -> Result<DiscardOutcome, GitError> {
        self.git.discard_moved(root, path, snapshot)
    }

    fn file_at_commit(
        &self,
        root: &Path,
        oid: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitError> {
        self.git.file_at_commit(root, oid, path)
    }

    fn command_log(&self) -> Option<CommandLog> {
        self.git.command_log()
    }
}

// Worktree target (D109, the per-file table).

#[test]
fn a_modified_file_moves_to_a_clean_worktree_and_the_source_returns_to_head() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let (id, events) = move_to(&mut engine, "a.txt", worktree_target(&folder), None);

    assert_eq!(outcome(&events), MoveOutcome::default());
    assert_eq!(read(&folder.join("a.txt")), b"changed\n");
    assert_eq!(read(&fixture.main.join("a.txt")), b"one\n");
    assert!(status_lines(&fixture.main).is_empty());
    assert_eq!(
        status_lines(&folder),
        [" M a.txt"],
        "unstaged in the target"
    );
    assert_safety_ref(&fixture.main, id, "a.txt", b"changed\n");
}

#[test]
fn a_new_file_arrives_untracked_and_leaves_the_source() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    write(&fixture.main, "sub/new.txt", b"new\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let (id, events) = move_to(&mut engine, "sub/new.txt", worktree_target(&folder), None);

    outcome(&events);
    assert_eq!(read(&folder.join("sub/new.txt")), b"new\n");
    assert_eq!(status_lines(&folder), ["?? sub/new.txt"]);
    assert!(!fixture.main.join("sub/new.txt").exists());
    assert!(status_lines(&fixture.main).is_empty());
    assert_safety_ref(&fixture.main, id, "sub/new.txt", b"new\n");
}

#[test]
fn a_deleted_file_is_deleted_in_the_worktree_and_restored_in_the_source() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    std::fs::remove_file(fixture.main.join("other.txt")).unwrap();
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let (_, events) = move_to(&mut engine, "other.txt", worktree_target(&folder), None);

    outcome(&events);
    assert!(!folder.join("other.txt").exists());
    assert_eq!(status_lines(&folder), [" D other.txt"]);
    assert_eq!(read(&fixture.main.join("other.txt")), b"x\n");
    assert!(status_lines(&fixture.main).is_empty());
}

#[test]
fn a_dirty_worktree_with_edits_on_other_lines_gets_both_changes() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    write(&fixture.main, "multi.txt", b"ONE\n2\n3\n4\n5\n");
    write(&folder, "multi.txt", b"1\n2\n3\n4\nFIVE\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let (_, events) = move_to(&mut engine, "multi.txt", worktree_target(&folder), None);

    outcome(&events);
    assert_eq!(read(&folder.join("multi.txt")), b"ONE\n2\n3\n4\nFIVE\n");
    assert_eq!(read(&fixture.main.join("multi.txt")), b"1\n2\n3\n4\n5\n");
}

#[test]
fn a_dirty_worktree_with_conflicting_edits_is_refused_and_both_sides_stay() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    write(&fixture.main, "multi.txt", b"MAIN\n2\n3\n4\n5\n");
    write(&folder, "multi.txt", b"WT\n2\n3\n4\n5\n");
    let source_before = status_lines(&fixture.main);
    let target_before = status_lines(&folder);
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let (_, events) = move_to(&mut engine, "multi.txt", worktree_target(&folder), None);

    assert_eq!(failure(&events), "the change conflicts with multi.txt");
    assert_eq!(read(&fixture.main.join("multi.txt")), b"MAIN\n2\n3\n4\n5\n");
    assert_eq!(read(&folder.join("multi.txt")), b"WT\n2\n3\n4\n5\n");
    assert_eq!(status_lines(&fixture.main), source_before);
    assert_eq!(status_lines(&folder), target_before);
    assert!(
        move_refs(&fixture.main).is_empty(),
        "no safety ref on a refusal"
    );
}

#[test]
fn the_target_index_is_untouched_and_the_change_arrives_unstaged() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    write(&folder, "other.txt", b"STAGED\n");
    git_in(&folder, &["add", "other.txt"]);
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let (_, events) = move_to(&mut engine, "a.txt", worktree_target(&folder), None);

    outcome(&events);
    assert_eq!(
        git_in(&folder, &["diff", "--cached", "--name-only"]),
        "other.txt"
    );
    assert_eq!(git_bytes(&folder, &["show", ":other.txt"]), b"STAGED\n");
    assert_eq!(status_lines(&folder), [" M a.txt", "M  other.txt"]);
}

#[test]
fn a_binary_file_is_replaced_when_the_worktree_still_has_the_base() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    write(&fixture.main, "bin.dat", b"\0c");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let (_, events) = move_to(&mut engine, "bin.dat", worktree_target(&folder), None);

    outcome(&events);
    assert_eq!(read(&folder.join("bin.dat")), b"\0c");
    assert_eq!(read(&fixture.main.join("bin.dat")), b"\0a");
}

#[test]
fn a_locked_worktree_accepts_the_file() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    git_in(&fixture.main, &["worktree", "lock", &text(&folder)]);
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let (_, events) = move_to(&mut engine, "a.txt", worktree_target(&folder), None);

    outcome(&events);
    assert_eq!(read(&folder.join("a.txt")), b"changed\n");
}

#[test]
fn a_source_file_edited_after_the_snapshot_is_kept_and_reported() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    write(&fixture.main, "a.txt", b"changed\n");
    let main = fixture.main.clone();
    let log = CommandLog::default();
    let git = Racing::new(&log).on_move_ref(move || write(&main, "a.txt", b"edited later\n"));
    let mut engine = engine_with(Box::new(git), &fixture.main);

    let (id, events) = move_to(&mut engine, "a.txt", worktree_target(&folder), None);

    let moved = outcome(&events);
    assert!(
        moved
            .kept
            .as_deref()
            .is_some_and(|reason| !reason.is_empty()),
        "the source is kept, with a reason: {moved:?}"
    );
    assert_eq!(read(&fixture.main.join("a.txt")), b"edited later\n");
    assert_eq!(read(&folder.join("a.txt")), b"changed\n");
    assert_safety_ref(&fixture.main, id, "a.txt", b"changed\n");
}

// Branch target (D109, the commit built with plumbing).

#[test]
fn a_change_becomes_a_commit_on_a_new_branch_and_the_window_stays_put() {
    let fixture = fixture();
    let main = &fixture.main;
    let old_head = git_in(main, &["rev-parse", "HEAD"]);
    write(main, "other.txt", b"STAGED\n");
    git_in(main, &["add", "other.txt"]);
    write(main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(main, &log);

    let (id, events) = move_to(
        &mut engine,
        "a.txt",
        branch("feature/x", true),
        Some("  move a\n"),
    );

    let commit = outcome(&events)
        .commit
        .expect("a branch move names its commit");
    assert_eq!(git_in(main, &["rev-parse", "feature/x"]), commit);
    assert_eq!(
        git_in(main, &["rev-parse", &format!("{commit}^")]),
        old_head
    );
    assert_eq!(
        git_in(main, &["log", "-1", "--format=%s", &commit]),
        "move a"
    );
    assert_eq!(
        git_in(main, &["diff", "--name-only", &old_head, &commit]),
        "a.txt"
    );
    assert_eq!(git_in(main, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(git_in(main, &["rev-parse", "HEAD"]), old_head);
    assert_eq!(
        git_in(main, &["diff", "--cached", "--name-only"]),
        "other.txt"
    );
    assert_eq!(git_bytes(main, &["show", ":other.txt"]), b"STAGED\n");
    assert_eq!(read(&main.join("other.txt")), b"STAGED\n");
    assert_eq!(
        read(&main.join("a.txt")),
        b"one\n",
        "the source returns to HEAD"
    );
    assert_safety_ref(main, id, "a.txt", b"changed\n");
}

#[test]
fn a_change_merges_onto_an_existing_branch_that_diverged() {
    let fixture = fixture();
    let main = &fixture.main;
    commit_on_dev(main, "other.txt", b"dev\n");
    let dev_tip = git_in(main, &["rev-parse", "dev"]);
    write(main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(main, &log);

    let (_, events) = move_to(&mut engine, "a.txt", branch("dev", false), Some("move a"));

    let commit = outcome(&events)
        .commit
        .expect("a branch move names its commit");
    assert_eq!(git_in(main, &["rev-parse", "dev"]), commit);
    assert_eq!(git_in(main, &["rev-parse", &format!("{commit}^")]), dev_tip);
    assert_eq!(git_bytes(main, &["show", "dev:other.txt"]), b"dev\n");
    assert_eq!(git_bytes(main, &["show", "dev:a.txt"]), b"changed\n");
    assert_eq!(
        git_in(main, &["diff", "--name-only", &dev_tip, &commit]),
        "a.txt"
    );
    assert_eq!(git_in(main, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert!(status_lines(main).is_empty());
}

#[test]
fn a_change_that_conflicts_with_the_branch_tip_is_refused_and_nothing_changes() {
    let fixture = fixture();
    let main = &fixture.main;
    commit_on_dev(main, "a.txt", b"dev\n");
    let dev_tip = git_in(main, &["rev-parse", "dev"]);
    write(main, "a.txt", b"changed\n");
    let source_before = status_lines(main);
    let log = CommandLog::default();
    let mut engine = engine_on(main, &log);

    let (_, events) = move_to(&mut engine, "a.txt", branch("dev", false), Some("move a"));

    assert_eq!(failure(&events), "the change conflicts with a.txt");
    assert_eq!(git_in(main, &["rev-parse", "dev"]), dev_tip);
    assert_eq!(read(&main.join("a.txt")), b"changed\n");
    assert_eq!(status_lines(main), source_before);
    assert!(move_refs(main).is_empty());
}

#[test]
fn a_branch_checked_out_in_another_worktree_is_refused() {
    let fixture = fixture();
    let main = &fixture.main;
    let folder = worktree(&fixture, "wt", "feat");
    let feat_tip = git_in(main, &["rev-parse", "feat"]);
    write(main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(main, &log);

    let (_, events) = move_to(&mut engine, "a.txt", branch("feat", false), Some("move a"));

    assert!(
        failure(&events).starts_with("the branch is checked out in "),
        "{events:?}"
    );
    assert_eq!(git_in(main, &["rev-parse", "feat"]), feat_tip);
    assert_eq!(read(&main.join("a.txt")), b"changed\n");
    assert!(move_refs(main).is_empty());
    assert!(status_lines(&folder).is_empty());
}

#[test]
fn an_empty_message_is_refused_before_anything_is_written() {
    let fixture = fixture();
    let main = &fixture.main;
    write(main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(main, &log);

    for message in [None, Some("   \n")] {
        let (_, events) = move_to(&mut engine, "a.txt", branch("feature/x", true), message);
        assert_eq!(failure(&events), "a commit message is required");
    }

    assert_eq!(git_in(main, &["branch", "--list", "feature/x"]), "");
    assert_eq!(read(&main.join("a.txt")), b"changed\n");
    assert!(move_refs(main).is_empty());
}

#[test]
fn a_branch_that_moves_while_the_move_runs_is_not_overwritten() {
    let fixture = fixture();
    let main = &fixture.main;
    commit_on_dev(main, "other.txt", b"dev\n");
    let dev_before = git_in(main, &["rev-parse", "dev"]);
    write(main, "a.txt", b"changed\n");
    let source_before = status_lines(main);
    let race_repo = main.clone();
    let log = CommandLog::default();
    let git = Racing::new(&log).on_resolve(move || advance_branch(&race_repo, "dev"));
    let mut engine = engine_with(Box::new(git), main);

    let (id, events) = move_to(&mut engine, "a.txt", branch("dev", false), Some("move a"));

    assert!(
        failure(&events).contains("dev"),
        "the stale tip is refused by Git: {events:?}"
    );
    let raced = git_in(main, &["rev-parse", "dev"]);
    assert_ne!(raced, dev_before, "the other process's commit stays");
    assert_eq!(
        git_in(main, &["rev-parse", &format!("{raced}^")]),
        dev_before
    );
    assert_eq!(read(&main.join("a.txt")), b"changed\n");
    assert_eq!(status_lines(main), source_before);
    assert_safety_ref(main, id, "a.txt", b"changed\n");
}

// Both targets.

#[test]
fn a_move_is_recorded_in_the_command_log_with_its_safety_ref() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let (id, events) = move_to(&mut engine, "a.txt", worktree_target(&folder), None);

    outcome(&events);
    let safety = format!("refs/gitmaster/move/{}", id.0);
    assert!(
        log.entries()
            .iter()
            .any(|record| record.args.contains(&safety)),
        "the ref update is in the Git log"
    );
    assert!(engine.log().iter().any(|entry| {
        entry.id == id
            && entry.state == OperationState::Done
            && matches!(entry.kind, CommandKind::MoveChanges { .. })
    }));
}

#[test]
fn an_agent_cannot_move_changes() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let events = engine.handle(
        OperationId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
        &Command {
            actor: Actor::Agent {
                name: "bot".to_owned(),
            },
            kind: CommandKind::MoveChanges {
                path: "a.txt".to_owned(),
                target: worktree_target(&folder),
                message: None,
            },
        },
    );

    assert_eq!(failure(&events), "only a human can move changes");
    assert_eq!(read(&folder.join("a.txt")), b"one\n");
    assert_eq!(read(&fixture.main.join("a.txt")), b"changed\n");
    assert!(move_refs(&fixture.main).is_empty());
}

#[test]
fn a_worktree_whose_folder_is_gone_is_refused_and_the_source_stays() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    std::fs::remove_dir_all(&folder).unwrap();
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let (_, events) = move_to(&mut engine, "a.txt", worktree_target(&folder), None);

    assert_eq!(failure(&events), "the worktree folder is missing");
    assert_eq!(read(&fixture.main.join("a.txt")), b"changed\n");
    assert!(move_refs(&fixture.main).is_empty());
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_on_the_way_to_the_file_is_refused_and_nothing_leaves_the_repository() {
    let fixture = fixture();
    let folder = worktree(&fixture, "wt", "feat");
    let outside = tempfile::tempdir().unwrap();
    std::fs::remove_dir_all(folder.join("sub")).unwrap();
    std::os::unix::fs::symlink(outside.path(), folder.join("sub")).unwrap();
    write(&fixture.main, "sub/file.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let (_, events) = move_to(&mut engine, "sub/file.txt", worktree_target(&folder), None);

    assert_eq!(
        failure(&events),
        "the path is a symbolic link or leaves the worktree"
    );
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    assert_eq!(read(&fixture.main.join("sub/file.txt")), b"changed\n");
}

#[test]
fn a_move_on_an_unborn_branch_is_an_error_that_changes_nothing() {
    let base = tempfile::tempdir().unwrap();
    let main = base.path().join("repo");
    std::fs::create_dir(&main).unwrap();
    git_in(&main, &["init", "-q", "-b", "main"]);
    write(&main, "new.txt", b"new\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&main, &log);

    let (_, events) = move_to(
        &mut engine,
        "new.txt",
        branch("feature/x", true),
        Some("first"),
    );

    assert!(!failure(&events).is_empty(), "{events:?}");
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::ChangesMoved { .. })),
        "{events:?}"
    );
    assert_eq!(read(&main.join("new.txt")), b"new\n");
    assert!(move_refs(&main).is_empty());
    assert_eq!(git_in(&main, &["for-each-ref", "refs/heads/"]), "");
}
