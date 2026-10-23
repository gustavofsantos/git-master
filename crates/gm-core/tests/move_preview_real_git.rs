//! Previewing the move of one file's change (D109) against real repositories in
//! temporary directories. Each test removes its directory when it ends. A
//! preview must change nothing, which the last test checks.

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::atomic::{AtomicU64, Ordering};

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::{Actor, Command, CommandKind, Event, MoveBlocker, MoveTarget, OperationId};

fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Runs git without asserting its exit code: a merge is meant to conflict.
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

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Writes `content` to `dir/name`, creating folders on the way.
fn write(dir: &Path, name: &str, content: &[u8]) {
    let file = dir.join(name);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, content).unwrap();
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

fn engine_on(root: &Path, log: &CommandLog) -> Engine {
    let git = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    let events = engine.handle(
        OperationId(1),
        &human(CommandKind::OpenRepo { path: text(root) }),
    );
    assert!(done(&events), "open failed: {events:?}");
    engine
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
                state: gm_schema::OperationState::Done,
                ..
            }
        )
    })
}

static NEXT_ID: AtomicU64 = AtomicU64::new(100);

/// The blockers `PreviewMoveChanges` answers for `path` and `target`.
fn blockers(engine: &mut Engine, path: &str, target: MoveTarget) -> Vec<MoveBlocker> {
    let id = OperationId(NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let events = engine.handle(
        id,
        &human(CommandKind::PreviewMoveChanges {
            path: path.to_owned(),
            target,
        }),
    );
    assert!(done(&events), "preview failed: {events:?}");
    events
        .iter()
        .find_map(|e| match e {
            Event::MovePreviewed { preview, .. } => Some(preview.blockers.clone()),
            _ => None,
        })
        .expect("the preview is answered")
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

fn conflict(paths: &[&str]) -> MoveBlocker {
    MoveBlocker::Conflict {
        paths: paths.iter().map(|p| (*p).to_owned()).collect(),
    }
}

#[test]
fn a_change_moves_to_a_new_branch_from_head() {
    let fixture = fixture();
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert!(blockers(&mut engine, "a.txt", branch("feature/x", true)).is_empty());
}

#[test]
fn a_change_moves_to_an_existing_branch_whose_tip_does_not_touch_it() {
    let fixture = fixture();
    commit_on_dev(&fixture.main, "other.txt", b"dev\n");
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert!(blockers(&mut engine, "a.txt", branch("dev", false)).is_empty());
}

#[test]
fn a_new_branch_over_an_existing_one_is_refused() {
    let fixture = fixture();
    git_in(&fixture.main, &["branch", "dev"]);
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "a.txt", branch("dev", true)),
        vec![MoveBlocker::BranchExists]
    );
}

#[test]
fn a_missing_branch_is_not_found() {
    let fixture = fixture();
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "a.txt", branch("nope", false)),
        vec![MoveBlocker::BranchNotFound]
    );
}

#[test]
fn an_invalid_branch_name_is_refused_with_the_reason() {
    let fixture = fixture();
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    let found = blockers(&mut engine, "a.txt", branch("a..b", true));
    assert!(
        matches!(
            found.as_slice(),
            [MoveBlocker::InvalidName { reason }] if reason.contains("..")
        ),
        "{found:?}"
    );
}

#[test]
fn the_branch_of_the_window_is_the_same_branch() {
    let fixture = fixture();
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "a.txt", branch("main", false)),
        vec![MoveBlocker::SameBranch]
    );
}

#[test]
fn a_branch_checked_out_in_another_worktree_is_refused_with_its_folder() {
    let fixture = fixture();
    git_in(&fixture.main, &["branch", "dev"]);
    let folder = fixture.main.parent().unwrap().join("repo-dev-checkout");
    git_in(
        &fixture.main,
        &["worktree", "add", "-q", &text(&folder), "dev"],
    );
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "a.txt", branch("dev", false)),
        vec![MoveBlocker::BranchCheckedOutElsewhere {
            path: text(&folder)
        }]
    );
}

#[test]
fn a_change_that_conflicts_on_the_branch_tip_is_refused_with_the_path() {
    let fixture = fixture();
    commit_on_dev(&fixture.main, "a.txt", b"theirs\n");
    write(&fixture.main, "a.txt", b"mine\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "a.txt", branch("dev", false)),
        vec![conflict(&["a.txt"])]
    );
}

#[test]
fn a_clean_worktree_accepts_the_change() {
    let fixture = fixture();
    let folder = worktree(&fixture, "repo-clean", "feat");
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert!(blockers(&mut engine, "a.txt", worktree_target(&folder)).is_empty());
}

#[test]
fn a_dirty_worktree_with_another_file_changed_accepts_the_change() {
    let fixture = fixture();
    let folder = worktree(&fixture, "repo-dirty-other", "feat");
    write(&folder, "other.txt", b"dirty there\n");
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert!(blockers(&mut engine, "a.txt", worktree_target(&folder)).is_empty());
}

#[test]
fn overlapping_edits_on_other_lines_merge_without_a_blocker() {
    let fixture = fixture();
    let folder = worktree(&fixture, "repo-merge-lines", "feat");
    write(&folder, "multi.txt", b"1\n2\n3\n4\nfive\n");
    write(&fixture.main, "multi.txt", b"one\n2\n3\n4\n5\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert!(blockers(&mut engine, "multi.txt", worktree_target(&folder)).is_empty());
}

#[test]
fn overlapping_edits_on_the_same_line_conflict() {
    let fixture = fixture();
    let folder = worktree(&fixture, "repo-merge-same", "feat");
    write(&folder, "multi.txt", b"feature\n2\n3\n4\n5\n");
    write(&fixture.main, "multi.txt", b"main\n2\n3\n4\n5\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "multi.txt", worktree_target(&folder)),
        vec![conflict(&["multi.txt"])]
    );
}

#[test]
fn a_new_file_over_a_different_file_is_refused() {
    let fixture = fixture();
    let folder = worktree(&fixture, "repo-add-over", "feat");
    write(&folder, "new.txt", b"other\n");
    write(&fixture.main, "new.txt", b"new\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "new.txt", worktree_target(&folder)),
        vec![MoveBlocker::AddOverExisting]
    );
}

#[test]
fn a_deleted_file_that_the_worktree_changed_conflicts() {
    let fixture = fixture();
    let folder = worktree(&fixture, "repo-delete-changed", "feat");
    write(&folder, "a.txt", b"feature\n");
    std::fs::remove_file(fixture.main.join("a.txt")).unwrap();
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "a.txt", worktree_target(&folder)),
        vec![conflict(&["a.txt"])]
    );
}

#[test]
fn a_binary_file_that_differs_in_the_worktree_is_refused() {
    let fixture = fixture();
    let folder = worktree(&fixture, "repo-binary", "feat");
    write(&folder, "bin.dat", b"\0c");
    write(&fixture.main, "bin.dat", b"\0b");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "bin.dat", worktree_target(&folder)),
        vec![MoveBlocker::BinaryDiffers]
    );
}

#[test]
fn a_symbolic_link_at_the_target_path_is_refused() {
    let fixture = fixture();
    let folder = worktree(&fixture, "repo-link-file", "feat");
    std::fs::remove_file(folder.join("a.txt")).unwrap();
    std::os::unix::fs::symlink("other.txt", folder.join("a.txt")).unwrap();
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "a.txt", worktree_target(&folder)),
        vec![MoveBlocker::UnsafePath]
    );
}

#[test]
fn a_symbolic_link_on_the_way_to_the_target_path_is_refused() {
    let fixture = fixture();
    let folder = worktree(&fixture, "repo-link-dir", "feat");
    std::fs::remove_dir_all(folder.join("sub")).unwrap();
    std::os::unix::fs::symlink(".", folder.join("sub")).unwrap();
    write(&fixture.main, "sub/file.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "sub/file.txt", worktree_target(&folder)),
        vec![MoveBlocker::UnsafePath]
    );
}

#[test]
fn a_worktree_whose_folder_is_gone_is_missing() {
    let fixture = fixture();
    let folder = worktree(&fixture, "repo-gone", "feat");
    std::fs::remove_dir_all(&folder).unwrap();
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "a.txt", worktree_target(&folder)),
        vec![MoveBlocker::WorktreeMissing]
    );
}

#[test]
fn a_folder_that_is_no_worktree_is_not_found() {
    let fixture = fixture();
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    let elsewhere = fixture.main.parent().unwrap().join("nowhere");
    assert_eq!(
        blockers(&mut engine, "a.txt", worktree_target(&elsewhere)),
        vec![MoveBlocker::WorktreeNotFound]
    );
}

#[test]
fn the_window_worktree_is_the_same_worktree() {
    let fixture = fixture();
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "a.txt", worktree_target(&fixture.main)),
        vec![MoveBlocker::SameWorktree]
    );
}

#[test]
fn a_path_without_a_change_has_nothing_to_move() {
    let fixture = fixture();
    write(&fixture.main, "a.txt", b"changed\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "other.txt", branch("feature/x", true)),
        vec![MoveBlocker::NothingToMove]
    );
}

#[test]
fn a_conflicted_path_is_refused() {
    let fixture = fixture();
    commit_on_dev(&fixture.main, "a.txt", b"theirs\n");
    write(&fixture.main, "a.txt", b"mine\n");
    git_in(&fixture.main, &["commit", "-q", "-am", "main change"]);
    let merged = git_output(&fixture.main, &["merge", "-q", "dev"]);
    assert!(!merged.status.success(), "the merge must conflict");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);
    assert_eq!(
        blockers(&mut engine, "a.txt", branch("feature/x", true)),
        vec![MoveBlocker::Conflicted]
    );
}

#[test]
fn a_preview_changes_nothing_and_is_not_in_the_command_log() {
    let fixture = fixture();
    let folder = worktree(&fixture, "repo-readonly", "feat");
    write(&folder, "other.txt", b"dirty there\n");
    write(&fixture.main, "a.txt", b"changed\n");
    write(&fixture.main, "new.txt", b"new\n");
    let log = CommandLog::default();
    let mut engine = engine_on(&fixture.main, &log);

    let state = || {
        (
            git_in(&fixture.main, &["status", "--porcelain"]),
            git_in(&fixture.main, &["for-each-ref"]),
            git_in(&fixture.main, &["worktree", "list", "--porcelain"]),
        )
    };
    let before = state();
    let commands_before = log.entries().len();
    let operations_before = engine.log().len();

    assert!(blockers(&mut engine, "a.txt", branch("feature/x", true)).is_empty());
    assert!(blockers(&mut engine, "a.txt", worktree_target(&folder)).is_empty());
    assert!(blockers(&mut engine, "new.txt", worktree_target(&folder)).is_empty());
    assert_eq!(
        blockers(&mut engine, "a.txt", branch("main", false)),
        vec![MoveBlocker::SameBranch]
    );

    assert_eq!(state(), before, "the repository is unchanged by a preview");
    assert_eq!(
        log.entries().len(),
        commands_before,
        "no Git command is logged"
    );
    assert_eq!(
        engine.log().len(),
        operations_before,
        "no operation is logged"
    );
}
