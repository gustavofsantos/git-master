//! Discarding one unstaged hunk against real repositories in temporary directories.

use std::path::Path;
use std::process::Command;

use gm_core::GitPort;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::{DiffSide, HunkId};

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
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn write(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap()
}

fn repo_with(name: &str, content: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    write(dir.path(), name, content);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

fn numbered(count: u32) -> String {
    (1..=count).fold(String::new(), |mut text, n| {
        text.push_str("line ");
        text.push_str(&n.to_string());
        text.push('\n');
        text
    })
}

fn unstaged_hunks(dir: &Path, path: &str) -> Vec<HunkId> {
    SubprocessGit::default()
        .diff(dir, path, DiffSide::Unstaged)
        .unwrap()
        .hunks
        .into_iter()
        .map(|hunk| hunk.id)
        .collect()
}

#[test]
fn discarding_one_hunk_restores_it_and_keeps_the_other() {
    let original = numbered(20);
    let edited = original
        .replace("line 2\n", "two\n")
        .replace("line 19\n", "nineteen\n");
    let dir = repo_with("f.txt", &original);
    write(dir.path(), "f.txt", &edited);
    let port = SubprocessGit::default();
    let ids = unstaged_hunks(dir.path(), "f.txt");
    assert_eq!(ids.len(), 2);

    port.discard_hunk(dir.path(), "f.txt", &ids[0]).unwrap();

    let expected = edited.replace("two\n", "line 2\n");
    assert_eq!(read(dir.path(), "f.txt"), expected);
    assert_eq!(git(dir.path(), &["diff", "--cached", "--name-only"]), "");
    assert_eq!(unstaged_hunks(dir.path(), "f.txt").len(), 1);
}

#[test]
fn discarding_a_hunk_at_the_end_without_a_newline_restores_the_bytes() {
    let original = "one\ntwo";
    let dir = repo_with("f.txt", original);
    write(dir.path(), "f.txt", "one\nTWO");
    let port = SubprocessGit::default();
    let ids = unstaged_hunks(dir.path(), "f.txt");

    port.discard_hunk(dir.path(), "f.txt", &ids[0]).unwrap();

    assert_eq!(read(dir.path(), "f.txt"), original);
}

#[test]
fn a_stale_hunk_id_is_rejected_and_the_file_is_left_alone() {
    let dir = repo_with("f.txt", &numbered(5));
    write(
        dir.path(),
        "f.txt",
        &numbered(5).replace("line 3\n", "three\n"),
    );
    let port = SubprocessGit::default();
    let id = unstaged_hunks(dir.path(), "f.txt").remove(0);
    port.discard_hunk(dir.path(), "f.txt", &id).unwrap();
    let after = read(dir.path(), "f.txt");

    port.discard_hunk(dir.path(), "f.txt", &id).unwrap_err();

    assert_eq!(read(dir.path(), "f.txt"), after);
}

#[test]
fn an_untracked_file_is_refused() {
    let dir = repo_with("a.txt", "one\n");
    write(dir.path(), "new.txt", "fresh\n");
    let port = SubprocessGit::default();

    let error = port
        .discard_hunk(dir.path(), "new.txt", &HunkId("0".into()))
        .unwrap_err();

    assert!(error.0.contains("untracked"), "{error}");
    assert!(dir.path().join("new.txt").exists());
}

#[test]
fn a_discard_is_logged_as_a_write() {
    let dir = repo_with("f.txt", "one\n");
    write(dir.path(), "f.txt", "two\n");
    let log = CommandLog::default();
    let port = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };
    let id = unstaged_hunks(dir.path(), "f.txt").remove(0);

    port.discard_hunk(dir.path(), "f.txt", &id).unwrap();

    let entries = log.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].args.join(" "), "apply --reverse");
    assert!(entries[0].succeeded());
    assert_eq!(read(dir.path(), "f.txt"), "one\n");
}
