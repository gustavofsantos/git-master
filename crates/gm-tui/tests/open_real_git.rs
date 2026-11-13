//! `gitmaster tui` against real Git in temporary repositories (D235).

use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-q", "-m", "first"]);
    dir
}

#[test]
fn opens_the_repository_that_contains_a_subfolder_with_its_status() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/new.txt"), "new\n").unwrap();

    let opened = gm_tui::open(&dir.path().join("sub")).unwrap();

    assert_eq!(opened.status.branch.head.as_deref(), Some("main"));
    let mut paths: Vec<&str> = opened
        .status
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    paths.sort_unstable();
    // Git lists a new folder as one untracked entry, as the app shows it.
    assert_eq!(paths, ["a.txt", "sub/"]);
}

#[test]
fn a_folder_outside_a_repository_is_refused_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let Err(error) = gm_tui::open(dir.path()) else {
        panic!("opened a folder outside a repository")
    };
    assert!(!error.is_empty());
}

#[test]
fn two_paths_are_a_usage_error_and_no_terminal_is_refused() {
    assert_eq!(gm_tui::command(&["a".into(), "b".into()]), 2);
    // The test harness gives no terminal on standard input.
    assert_eq!(gm_tui::command(&[".".into()]), 1);
}
