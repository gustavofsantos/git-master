//! Discarding every change of one file against real repositories in temporary
//! directories (B1, D93).

use std::path::Path;
use std::process::Command;

use gm_core::GitPort;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::StatusEntry;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
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

fn write(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap()
}

/// The content of `path` in the index.
fn indexed(dir: &Path, path: &str) -> String {
    git(dir, &["show", &format!(":{path}")])
}

fn repo_with(name: &str, content: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    write(dir.path(), name, content);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

fn status_of(dir: &Path, path: &str) -> Option<StatusEntry> {
    SubprocessGit::default()
        .status(dir)
        .unwrap()
        .entries
        .into_iter()
        .find(|entry| entry.path == path)
}

#[test]
fn discarding_unstaged_changes_restores_the_index_and_keeps_the_staged_part() {
    let dir = repo_with("f.txt", "one\n");
    write(dir.path(), "f.txt", "staged\n");
    git(dir.path(), &["add", "f.txt"]);
    write(dir.path(), "f.txt", "staged\nunstaged\n");

    SubprocessGit::default()
        .discard_file(dir.path(), "f.txt", false)
        .unwrap();

    assert_eq!(read(dir.path(), "f.txt"), "staged\n");
    assert_eq!(indexed(dir.path(), "f.txt"), "staged\n");
    let entry = status_of(dir.path(), "f.txt").unwrap();
    assert!(entry.staged.is_some());
    assert!(entry.unstaged.is_none());
}

#[test]
fn discarding_all_changes_restores_head_in_the_index_and_the_worktree() {
    let dir = repo_with("f.txt", "one\n");
    write(dir.path(), "f.txt", "staged\n");
    git(dir.path(), &["add", "f.txt"]);
    write(dir.path(), "f.txt", "staged\nunstaged\n");

    SubprocessGit::default()
        .discard_file(dir.path(), "f.txt", true)
        .unwrap();

    assert_eq!(read(dir.path(), "f.txt"), "one\n");
    assert_eq!(indexed(dir.path(), "f.txt"), "one\n");
    assert!(status_of(dir.path(), "f.txt").is_none());
}

#[test]
fn a_deleted_tracked_file_comes_back_from_the_index() {
    let dir = repo_with("f.txt", "one\n");
    std::fs::remove_file(dir.path().join("f.txt")).unwrap();

    SubprocessGit::default()
        .discard_file(dir.path(), "f.txt", false)
        .unwrap();

    assert_eq!(read(dir.path(), "f.txt"), "one\n");
    assert!(status_of(dir.path(), "f.txt").is_none());
}

#[test]
fn an_untracked_file_is_deleted() {
    let dir = repo_with("a.txt", "one\n");
    write(dir.path(), "new.txt", "fresh\n");

    SubprocessGit::default()
        .discard_file(dir.path(), "new.txt", false)
        .unwrap();

    assert!(!dir.path().join("new.txt").exists());
    assert_eq!(read(dir.path(), "a.txt"), "one\n");
}

#[test]
fn an_untracked_folder_is_deleted_with_its_content() {
    let dir = repo_with("a.txt", "one\n");
    std::fs::create_dir(dir.path().join("docs")).unwrap();
    write(dir.path(), "docs/x.txt", "x\n");
    write(dir.path(), "docs/y.txt", "y\n");

    SubprocessGit::default()
        .discard_file(dir.path(), "docs/", false)
        .unwrap();

    assert!(!dir.path().join("docs").exists());
    assert_eq!(read(dir.path(), "a.txt"), "one\n");
}

#[test]
fn an_intent_to_add_file_is_removed_without_losing_anything_else() {
    let dir = repo_with("a.txt", "one\n");
    write(dir.path(), "new.txt", "fresh\n");
    git(dir.path(), &["add", "-N", "new.txt"]);

    SubprocessGit::default()
        .discard_file(dir.path(), "new.txt", false)
        .unwrap();

    assert!(!dir.path().join("new.txt").exists());
    assert_eq!(git(dir.path(), &["ls-files"]), "a.txt\n");
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
}

#[test]
fn a_new_staged_file_is_refused_without_the_staged_side() {
    let dir = repo_with("a.txt", "one\n");
    write(dir.path(), "new.txt", "fresh\n");
    git(dir.path(), &["add", "new.txt"]);

    let error = SubprocessGit::default()
        .discard_file(dir.path(), "new.txt", false)
        .unwrap_err();

    assert_eq!(error.0, "new.txt has nothing unstaged to discard");
    assert_eq!(read(dir.path(), "new.txt"), "fresh\n");
    assert_eq!(indexed(dir.path(), "new.txt"), "fresh\n");
}

#[test]
fn a_new_staged_file_is_removed_from_the_index_and_the_disk_with_the_staged_side() {
    let dir = repo_with("a.txt", "one\n");
    write(dir.path(), "new.txt", "fresh\n");
    git(dir.path(), &["add", "new.txt"]);

    SubprocessGit::default()
        .discard_file(dir.path(), "new.txt", true)
        .unwrap();

    assert!(!dir.path().join("new.txt").exists());
    assert_eq!(git(dir.path(), &["ls-files", "--", "new.txt"]), "");
    assert!(status_of(dir.path(), "new.txt").is_none());
}

#[test]
fn a_staged_rename_puts_the_old_path_back_and_removes_the_new_one() {
    let dir = repo_with("a.txt", "one\none more line\n");
    git(dir.path(), &["mv", "a.txt", "b.txt"]);

    SubprocessGit::default()
        .discard_file(dir.path(), "b.txt", true)
        .unwrap();

    assert!(!dir.path().join("b.txt").exists());
    assert_eq!(read(dir.path(), "a.txt"), "one\none more line\n");
    assert_eq!(indexed(dir.path(), "a.txt"), "one\none more line\n");
    assert_eq!(git(dir.path(), &["ls-files"]), "a.txt\n");
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
}

#[test]
fn a_binary_file_comes_back_byte_for_byte() {
    let original: Vec<u8> = vec![0, 1, 2, 255, 0, 10, 13, 0, 200];
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("blob.bin"), &original).unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    std::fs::write(dir.path().join("blob.bin"), [9u8, 9, 0, 3]).unwrap();

    SubprocessGit::default()
        .discard_file(dir.path(), "blob.bin", false)
        .unwrap();

    assert_eq!(
        std::fs::read(dir.path().join("blob.bin")).unwrap(),
        original
    );
}

#[test]
fn paths_with_spaces_and_unicode_are_discarded() {
    let names = ["my notes.txt", "relatório ção 文件.txt"];
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    for name in names {
        write(dir.path(), name, "one\n");
    }
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    for name in names {
        write(dir.path(), name, "changed\n");
    }

    for name in names {
        SubprocessGit::default()
            .discard_file(dir.path(), name, false)
            .unwrap();
        assert_eq!(read(dir.path(), name), "one\n");
    }
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
}

#[test]
fn a_path_with_pathspec_magic_names_only_that_file() {
    let literal = ":(glob)*";
    let dir = repo_with(literal, "one\n");
    write(dir.path(), "a.txt", "one\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "second"]);
    write(dir.path(), literal, "changed\n");
    write(dir.path(), "a.txt", "changed\n");

    SubprocessGit::default()
        .discard_file(dir.path(), literal, false)
        .unwrap();

    assert_eq!(read(dir.path(), literal), "one\n");
    assert_eq!(read(dir.path(), "a.txt"), "changed\n");
}

#[test]
fn a_path_without_changes_is_refused() {
    let dir = repo_with("f.txt", "one\n");

    let error = SubprocessGit::default()
        .discard_file(dir.path(), "f.txt", true)
        .unwrap_err();
    assert_eq!(error.0, "f.txt has no changes to discard");

    let error = SubprocessGit::default()
        .discard_file(dir.path(), "missing.txt", false)
        .unwrap_err();
    assert_eq!(error.0, "missing.txt has no changes to discard");
}

#[test]
fn a_conflicted_file_is_refused_until_the_conflict_is_resolved() {
    let dir = repo_with("f.txt", "base\n");
    git(dir.path(), &["checkout", "-q", "-b", "feature"]);
    write(dir.path(), "f.txt", "feature\n");
    git(dir.path(), &["commit", "-q", "-am", "feature"]);
    git(dir.path(), &["checkout", "-q", "main"]);
    write(dir.path(), "f.txt", "main\n");
    git(dir.path(), &["commit", "-q", "-am", "main"]);
    let merge = git_output(dir.path(), &["merge", "feature"]);
    assert!(!merge.status.success(), "the merge should conflict");
    let before = read(dir.path(), "f.txt");

    let error = SubprocessGit::default()
        .discard_file(dir.path(), "f.txt", false)
        .unwrap_err();
    assert_eq!(error.0, "f.txt is conflicted: resolve the conflict first");
    let error = SubprocessGit::default()
        .discard_file(dir.path(), "f.txt", true)
        .unwrap_err();
    assert_eq!(error.0, "f.txt is conflicted: resolve the conflict first");

    assert_eq!(read(dir.path(), "f.txt"), before);
    assert!(before.contains("<<<<<<<"));
}

#[test]
fn a_locked_index_is_refused_and_nothing_is_touched() {
    let dir = repo_with("f.txt", "one\n");
    write(dir.path(), "f.txt", "changed\n");
    std::fs::write(dir.path().join(".git/index.lock"), "").unwrap();

    let error = SubprocessGit::default()
        .discard_file(dir.path(), "f.txt", false)
        .unwrap_err();

    assert!(error.0.contains("index is locked"), "{error}");
    assert_eq!(read(dir.path(), "f.txt"), "changed\n");
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

    port.discard_file(dir.path(), "f.txt", false).unwrap();

    let entries = log.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].args.join(" "),
        "--literal-pathspecs restore --worktree -- f.txt"
    );
    assert!(entries[0].succeeded());
    assert_eq!(read(dir.path(), "f.txt"), "one\n");
}

#[test]
fn a_refused_discard_is_not_logged() {
    let dir = repo_with("f.txt", "one\n");
    let log = CommandLog::default();
    let port = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };

    port.discard_file(dir.path(), "f.txt", true).unwrap_err();

    assert!(log.entries().is_empty());
}
