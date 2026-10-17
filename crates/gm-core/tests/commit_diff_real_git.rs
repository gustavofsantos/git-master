//! The diff of one file in one commit, read with real git in temporary
//! repositories. Each case is the change type the brief names.

use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use gm_core::GitPort;
use gm_core::git::SubprocessGit;
use gm_schema::{FileDiff, LineKind};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "merge.ff=false",
        ])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn head(dir: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    dir
}

fn commit(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
    head(dir)
}

/// The file diff of `path` in `oid`, read the way the engine reads it.
fn file_diff(dir: &Path, oid: &str, path: &str, old_path: Option<&str>) -> FileDiff {
    let read = SubprocessGit::default()
        .commit_file_diff(dir, oid, path, old_path)
        .unwrap();
    read(&AtomicBool::new(false)).unwrap()
}

fn kinds(diff: &FileDiff) -> Vec<LineKind> {
    diff.hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter().map(|line| line.kind))
        .collect()
}

#[test]
fn an_added_file_is_all_added_lines() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    commit(dir.path(), "root");
    std::fs::write(dir.path().join("new.txt"), "x\ny\n").unwrap();
    let oid = commit(dir.path(), "add");

    let diff = file_diff(dir.path(), &oid, "new.txt", None);
    assert_eq!(diff.path, "new.txt");
    assert_eq!(diff.old_path, None);
    assert!(!diff.binary);
    assert_eq!(kinds(&diff), [LineKind::Added, LineKind::Added]);
}

#[test]
fn a_modified_file_has_removed_then_added_lines() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
    commit(dir.path(), "root");
    std::fs::write(dir.path().join("a.txt"), "one\n2\nthree\n").unwrap();
    let oid = commit(dir.path(), "modify");

    let diff = file_diff(dir.path(), &oid, "a.txt", None);
    assert_eq!(diff.hunks.len(), 1);
    let texts: Vec<(LineKind, &str)> = diff.hunks[0]
        .lines
        .iter()
        .map(|line| (line.kind, line.text.as_str()))
        .collect();
    assert!(texts.contains(&(LineKind::Removed, "two")));
    assert!(texts.contains(&(LineKind::Added, "2")));
}

#[test]
fn a_deleted_file_is_all_removed_lines() {
    let dir = repo();
    std::fs::write(dir.path().join("gone.txt"), "bye\nbye\n").unwrap();
    commit(dir.path(), "root");
    std::fs::remove_file(dir.path().join("gone.txt")).unwrap();
    let oid = commit(dir.path(), "delete");

    let diff = file_diff(dir.path(), &oid, "gone.txt", None);
    assert_eq!(kinds(&diff), [LineKind::Removed, LineKind::Removed]);
}

#[test]
fn a_rename_with_edits_pairs_both_paths() {
    let dir = repo();
    let mut body = String::new();
    for n in 1..=10 {
        writeln!(body, "line {n}").unwrap();
    }
    std::fs::write(dir.path().join("old.txt"), &body).unwrap();
    commit(dir.path(), "root");
    git(dir.path(), &["mv", "old.txt", "new.txt"]);
    std::fs::write(
        dir.path().join("new.txt"),
        body.replace("line 5\n", "five\n"),
    )
    .unwrap();
    let oid = commit(dir.path(), "rename and edit");

    let paired = file_diff(dir.path(), &oid, "new.txt", Some("old.txt"));
    assert_eq!(paired.path, "new.txt");
    assert_eq!(paired.old_path.as_deref(), Some("old.txt"));
    let changed: Vec<LineKind> = kinds(&paired)
        .into_iter()
        .filter(|kind| *kind != LineKind::Context)
        .collect();
    assert_eq!(changed, [LineKind::Removed, LineKind::Added]);

    // Without the source, git sees only the new side: the edit is lost as an add.
    let alone = file_diff(dir.path(), &oid, "new.txt", None);
    assert_eq!(alone.old_path, None);
    assert!(kinds(&alone).iter().all(|kind| *kind == LineKind::Added));
}

#[test]
fn a_binary_file_has_no_hunks_and_says_so() {
    let dir = repo();
    std::fs::write(dir.path().join("logo.bin"), [0u8, 1, 2, 3]).unwrap();
    commit(dir.path(), "root");
    std::fs::write(dir.path().join("logo.bin"), [0u8, 9, 9, 9, 0]).unwrap();
    let oid = commit(dir.path(), "binary");

    let diff = file_diff(dir.path(), &oid, "logo.bin", None);
    assert!(diff.binary);
    assert!(diff.hunks.is_empty());
}

#[test]
fn the_root_commit_adds_its_files() {
    let dir = repo();
    std::fs::write(dir.path().join("first.txt"), "hello\nworld\n").unwrap();
    let root = commit(dir.path(), "root");

    let diff = file_diff(dir.path(), &root, "first.txt", None);
    assert_eq!(kinds(&diff), [LineKind::Added, LineKind::Added]);
}

#[test]
fn a_merge_is_compared_with_its_first_parent() {
    let dir = repo();
    std::fs::write(dir.path().join("base.txt"), "base\n").unwrap();
    commit(dir.path(), "root");
    git(dir.path(), &["checkout", "-q", "-b", "side"]);
    std::fs::write(dir.path().join("topic.txt"), "topic\n").unwrap();
    commit(dir.path(), "side");
    git(dir.path(), &["checkout", "-q", "main"]);
    std::fs::write(dir.path().join("main.txt"), "main\n").unwrap();
    commit(dir.path(), "main");
    git(
        dir.path(),
        &["merge", "-q", "--no-ff", "side", "-m", "merge"],
    );
    let merge = head(dir.path());

    // topic.txt came in from the second parent, so against the first it is an add.
    let diff = file_diff(dir.path(), &merge, "topic.txt", None);
    assert_eq!(kinds(&diff), [LineKind::Added]);
    // main.txt was already in the first parent, so the merge changes nothing in it.
    let untouched = file_diff(dir.path(), &merge, "main.txt", None);
    assert!(untouched.hunks.is_empty());
}

#[test]
fn a_mode_change_has_no_hunks_and_no_error() {
    let dir = repo();
    std::fs::write(dir.path().join("run.sh"), "echo hi\n").unwrap();
    commit(dir.path(), "root");
    let script = dir.path().join("run.sh");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let oid = commit(dir.path(), "mode");

    let diff = file_diff(dir.path(), &oid, "run.sh", None);
    assert!(!diff.binary);
    assert!(diff.hunks.is_empty());
}

#[test]
fn a_large_file_is_read_whole_without_stalling() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "seed\n").unwrap();
    commit(dir.path(), "root");
    let mut big = String::new();
    for n in 0..20_000 {
        writeln!(big, "row number {n} with some text").unwrap();
    }
    std::fs::write(dir.path().join("big.txt"), big).unwrap();
    let oid = commit(dir.path(), "large");

    let diff = file_diff(dir.path(), &oid, "big.txt", None);
    let added = kinds(&diff)
        .iter()
        .filter(|k| **k == LineKind::Added)
        .count();
    assert_eq!(added, 20_000);
}

#[test]
fn non_utf8_content_is_replaced_not_a_panic() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "seed\n").unwrap();
    commit(dir.path(), "root");
    std::fs::write(dir.path().join("latin.txt"), b"caf\xe9\n").unwrap();
    let oid = commit(dir.path(), "latin1");

    let diff = file_diff(dir.path(), &oid, "latin.txt", None);
    assert_eq!(diff.hunks[0].lines[0].text, "caf\u{fffd}");
}

#[test]
fn a_path_the_commit_does_not_touch_has_no_hunks() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    let oid = commit(dir.path(), "root");

    let diff = file_diff(dir.path(), &oid, "missing.txt", None);
    assert!(diff.hunks.is_empty());
}

#[test]
fn a_value_that_is_not_a_hex_id_is_refused_before_git_runs() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    commit(dir.path(), "root");
    let git = SubprocessGit::default();
    assert!(git.commit_file_diff(dir.path(), "", "a.txt", None).is_err());
    assert!(
        git.commit_file_diff(dir.path(), "--output=x", "a.txt", None)
            .is_err()
    );
    assert!(
        git.commit_file_diff(dir.path(), "HEAD", "a.txt", None)
            .is_err()
    );
}

#[test]
fn an_id_git_does_not_know_is_git_s_error() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    commit(dir.path(), "root");
    let read = SubprocessGit::default()
        .commit_file_diff(dir.path(), &"a".repeat(40), "a.txt", None)
        .unwrap();
    assert!(read(&AtomicBool::new(false)).is_err());
}
