//! The subprocess adapter against real repositories in temporary directories.

use std::path::Path;
use std::process::Command;

use gm_core::GitPort;
use gm_core::git::SubprocessGit;
use gm_schema::Change;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
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
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

#[test]
fn reports_staged_unstaged_and_untracked() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    std::fs::write(dir.path().join("b.txt"), "two\n").unwrap();
    std::fs::write(dir.path().join("new file.txt"), "x\n").unwrap();

    let root = SubprocessGit::default().resolve_root(dir.path()).unwrap();
    let status = SubprocessGit::default().status(&root).unwrap();

    assert_eq!(status.branch.head.as_deref(), Some("main"));
    let find = |p: &str| status.entries.iter().find(|e| e.path == p).unwrap();
    assert_eq!(find("a.txt").staged, Some(Change::Modified));
    assert_eq!(find("b.txt").unstaged, Some(Change::Modified));
    assert!(find("new file.txt").untracked);
}

#[test]
fn status_does_not_touch_the_index() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    let index = dir.path().join(".git/index");
    let before = std::fs::read(&index).unwrap();
    SubprocessGit::default().status(dir.path()).unwrap();
    assert_eq!(std::fs::read(&index).unwrap(), before);
    assert!(!dir.path().join(".git/index.lock").exists());
}

#[test]
fn a_plain_directory_is_not_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    assert!(SubprocessGit::default().resolve_root(dir.path()).is_err());
}
