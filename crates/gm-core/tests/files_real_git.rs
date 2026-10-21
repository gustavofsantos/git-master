//! The file list read from real repositories in temporary directories (N1, N2).

use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use gm_core::GitPort;
use gm_core::file_index::FileIndex;
use gm_core::git::SubprocessGit;

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

fn write(dir: &Path, path: &str, text: &str) {
    let full = dir.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, text).unwrap();
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    dir
}

fn list(dir: &Path) -> FileIndex {
    let git = SubprocessGit::default();
    let root = git.resolve_root(dir).unwrap();
    let read = git.file_list(&root).unwrap();
    read(&AtomicBool::new(false)).unwrap()
}

fn paths(index: &FileIndex) -> Vec<String> {
    index.paths().collect()
}

/// Every file under `dir` except `.git`, by relative path, sorted.
fn on_disk(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.push(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn the_list_is_the_tracked_files_and_the_untracked_ones_that_are_not_ignored() {
    let dir = repo();
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    write(dir.path(), "README.md", "hi\n");
    write(dir.path(), ".gitignore", "target/\n*.log\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    write(dir.path(), "src/new.rs", "// untracked\n");
    write(dir.path(), "target/debug/out", "ignored\n");
    write(dir.path(), "build.log", "ignored\n");

    let files = paths(&list(dir.path()));

    assert_eq!(
        files,
        [".gitignore", "README.md", "src/main.rs", "src/new.rs"]
    );
}

#[test]
fn a_renamed_file_is_listed_under_its_new_name_only() {
    let dir = repo();
    write(dir.path(), "old/name.rs", "x\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    git(dir.path(), &["mv", "old/name.rs", "new_name.rs"]);

    assert_eq!(paths(&list(dir.path())), ["new_name.rs"]);
}

#[test]
fn names_with_spaces_unicode_and_quotes_come_through_whole() {
    let dir = repo();
    for name in [
        "a b.txt",
        "ação/relatório.md",
        "q\"uote.txt",
        "tab\there.txt",
        "-dash.txt",
    ] {
        write(dir.path(), name, "x\n");
    }
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);

    let files = paths(&list(dir.path()));

    let mut expected: Vec<String> = [
        "a b.txt",
        "ação/relatório.md",
        "q\"uote.txt",
        "tab\there.txt",
        "-dash.txt",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    expected.sort();
    assert_eq!(files, expected);
}

#[test]
fn a_file_deleted_from_disk_but_still_tracked_is_still_listed() {
    // The viewer shows it as missing; the list is Git's view, not the disk's.
    let dir = repo();
    write(dir.path(), "gone.rs", "x\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    std::fs::remove_file(dir.path().join("gone.rs")).unwrap();

    assert_eq!(paths(&list(dir.path())), ["gone.rs"]);
}

#[test]
fn a_sparse_checkout_lists_only_what_is_on_disk() {
    let dir = repo();
    write(dir.path(), "keep/a.rs", "a\n");
    write(dir.path(), "keep/deep/b.rs", "b\n");
    write(dir.path(), "skip/c.rs", "c\n");
    write(dir.path(), "top.txt", "t\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    git(dir.path(), &["sparse-checkout", "init", "--cone"]);
    git(dir.path(), &["sparse-checkout", "set", "keep"]);
    assert!(!dir.path().join("skip").exists(), "the checkout is sparse");

    let files = paths(&list(dir.path()));

    assert_eq!(files, on_disk(dir.path()));
    assert!(files.iter().all(|f| !f.starts_with("skip/")), "{files:?}");
    assert!(files.contains(&"keep/a.rs".to_owned()));
}

#[test]
fn an_empty_repository_has_an_empty_list() {
    let dir = repo();
    assert!(list(dir.path()).is_empty());
}

#[test]
fn a_stopped_read_fails_without_a_list() {
    let dir = repo();
    write(dir.path(), "a.rs", "x\n");
    let git = SubprocessGit::default();
    let root = git.resolve_root(dir.path()).unwrap();
    let read = git.file_list(&root).unwrap();

    assert!(read(&AtomicBool::new(true)).is_err());
}

#[test]
fn a_search_and_a_folder_listing_over_a_real_list() {
    let dir = repo();
    write(dir.path(), "crates/gm-ui/src/diff_pane.rs", "x\n");
    write(dir.path(), "crates/gm-ui/src/diff_rows.rs", "x\n");
    write(dir.path(), "crates/gm-core/src/git.rs", "x\n");
    write(dir.path(), "README.md", "x\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    let files = list(dir.path());

    let found = files.search("dpane", false, || false).unwrap();
    assert_eq!(found.hits[0].path, "crates/gm-ui/src/diff_pane.rs");

    let root = files.list_dir("", false);
    let names: Vec<_> = root.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["crates", "README.md"]);
    let crates = files.list_dir("crates", false);
    let names: Vec<_> = crates.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["gm-core/src", "gm-ui/src"]);
}

#[test]
fn reading_the_list_writes_nothing_into_the_repository() {
    let dir = repo();
    write(dir.path(), "a.rs", "x\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    let before = std::fs::metadata(dir.path().join(".git/index"))
        .unwrap()
        .modified()
        .unwrap();
    list(dir.path());
    let after = std::fs::metadata(dir.path().join(".git/index"))
        .unwrap()
        .modified()
        .unwrap();

    assert_eq!(before, after, "the index must not be rewritten");
    assert!(!dir.path().join(".git/index.lock").exists());
}
