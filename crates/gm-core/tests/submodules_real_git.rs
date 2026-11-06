//! Submodules in the file list and the text search, against real repositories in
//! temporary directories (D175). A submodule is cloned with `git submodule add`
//! from a local repository, which needs `protocol.file.allow` in the test only;
//! another submodule is a linked worktree of a second repository.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use gm_core::GitPort;
use gm_core::file_index::FileIndex;
use gm_core::git::SubprocessGit;
use gm_core::grep::GrepRequest;
use gm_schema::{GREP_FILE_LIMIT, GREP_TOTAL_LIMIT, GrepOptions, GrepResult, GrepScope};

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

fn commit_all(dir: &Path) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

/// A repository to become a submodule: a `.gitignore` and a file with the needle.
fn library() -> tempfile::TempDir {
    let dir = repo();
    write(dir.path(), ".gitignore", "*.log\n");
    write(
        dir.path(),
        "src/main.rs",
        "fn needle() {}\nlet needle = 1;\n",
    );
    commit_all(dir.path());
    dir
}

/// A superproject with a README and a file with the needle, and the submodule
/// `service-a` cloned from a library. The submodule has one untracked file that
/// its `.gitignore` does not hide, and one that it hides.
fn project() -> tempfile::TempDir {
    let top = repo();
    write(top.path(), "README.md", "needle top\n");
    write(top.path(), "src/a.rs", "needle a\n");
    commit_all(top.path());
    let lib = library();
    let url = lib.path().to_str().unwrap();
    git(
        top.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            url,
            "service-a",
        ],
    );
    let sub = top.path().join("service-a");
    write(&sub, "src/zz.rs", "needle new\n");
    write(&sub, "ignored.log", "needle\n");
    commit_all(top.path());
    top
}

/// A superproject with a submodule `wt` that is a linked worktree of another
/// repository, added as a gitlink with a `.gitmodules` entry.
fn project_with_linked_worktree(other: &Path) -> tempfile::TempDir {
    let top = repo();
    write(top.path(), "README.md", "needle top\n");
    commit_all(top.path());
    write(other, "src/wt.rs", "needle wt\n");
    commit_all(other);
    let wt = top.path().join("wt");
    git(
        other,
        &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
    );
    write(
        top.path(),
        ".gitmodules",
        "[submodule \"wt\"]\n\tpath = wt\n\turl = ../other\n",
    );
    write(&wt, "extra.rs", "needle extra\n");
    git(top.path(), &["add", ".gitmodules", "wt"]);
    git(top.path(), &["commit", "-q", "-m", "init"]);
    top
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

fn find_with(dir: &Path, query: &str, options: GrepOptions, pathspecs: &str) -> GrepResult {
    let git = SubprocessGit::default();
    let root = git.resolve_root(dir).unwrap();
    let request = GrepRequest::new(query, options, pathspecs).unwrap();
    let read = git.grep_text(&root, &request).unwrap();
    read(&AtomicBool::new(false)).unwrap()
}

fn find(dir: &Path, query: &str) -> GrepResult {
    find_with(dir, query, GrepOptions::default(), "")
}

fn tracked_only() -> GrepOptions {
    GrepOptions {
        scope: GrepScope::TrackedOnly,
        ..GrepOptions::default()
    }
}

/// `path:line` of every listed line, in the order the answer gives them.
fn hits(result: &GrepResult) -> Vec<String> {
    result
        .files
        .iter()
        .flat_map(|f| {
            f.lines
                .iter()
                .map(move |l| format!("{}:{}", f.path, l.line))
        })
        .collect()
}

#[test]
fn the_file_list_has_the_submodule_files_under_its_folder_and_no_gitlink() {
    let top = project();

    assert_eq!(
        paths(&list(top.path())),
        [
            ".gitmodules",
            "README.md",
            "service-a/.gitignore",
            "service-a/src/main.rs",
            "service-a/src/zz.rs",
            "src/a.rs",
        ]
    );
}

#[test]
fn the_file_list_honours_the_gitignore_of_the_submodule() {
    let top = project();

    let files = paths(&list(top.path()));

    assert!(
        files
            .iter()
            .all(|p| Path::new(p).extension().is_none_or(|e| e != "log")),
        "{files:?}"
    );
}

#[test]
fn the_folder_tree_shows_the_submodule_as_a_folder() {
    let top = project();
    let files = list(top.path());

    let root = files.list_dir("", false);
    let names: Vec<_> = root.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["service-a", "src", ".gitmodules", "README.md"]);
    let inside = files.list_dir("service-a", false);
    let names: Vec<_> = inside.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["src", ".gitignore"]);
}

#[test]
fn a_file_search_finds_a_file_of_the_submodule_by_its_path() {
    let top = project();

    let found = list(top.path()).search("zz.rs", false, || false).unwrap();

    assert_eq!(found.hits[0].path, "service-a/src/zz.rs");
}

#[test]
fn an_uninitialised_submodule_adds_nothing_and_does_not_fail() {
    let top = repo();
    write(top.path(), "README.md", "x\n");
    write(
        top.path(),
        ".gitmodules",
        "[submodule \"dead\"]\n\tpath = dead\n\turl = /nowhere\n",
    );
    git(top.path(), &["add", "README.md", ".gitmodules"]);
    git(
        top.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "160000,1234567890abcdef1234567890abcdef12345678,dead",
        ],
    );
    git(top.path(), &["commit", "-q", "-m", "init"]);
    std::fs::create_dir(top.path().join("dead")).unwrap();

    assert_eq!(paths(&list(top.path())), [".gitmodules", "README.md"]);
    let found = find(top.path(), "x");
    assert_eq!(hits(&found), ["README.md:1"]);
}

#[test]
fn a_repository_without_gitmodules_has_the_same_list_as_before() {
    let dir = repo();
    write(dir.path(), "a.rs", "x\n");
    write(dir.path(), "dir/b.rs", "x\n");
    commit_all(dir.path());

    assert_eq!(paths(&list(dir.path())), ["a.rs", "dir/b.rs"]);
}

#[test]
fn a_linked_worktree_as_a_submodule_is_listed_with_its_new_files() {
    let other = repo();
    let top = project_with_linked_worktree(other.path());
    write(&top.path().join("wt"), "untracked.txt", "x\n");

    let files = paths(&list(top.path()));

    assert!(files.contains(&"wt/src/wt.rs".to_owned()), "{files:?}");
    assert!(files.contains(&"wt/extra.rs".to_owned()), "{files:?}");
    assert!(files.contains(&"wt/untracked.txt".to_owned()), "{files:?}");
    assert!(files.iter().all(|p| p != "wt"), "{files:?}");
}

#[test]
fn a_search_finds_the_matches_of_the_repository_and_its_submodules_by_full_path() {
    let top = project();

    let result = find(top.path(), "needle");

    assert_eq!(
        hits(&result),
        [
            "README.md:1",
            "service-a/src/main.rs:1",
            "service-a/src/main.rs:2",
            "service-a/src/zz.rs:1",
            "src/a.rs:1",
        ]
    );
    assert!(!result.truncated);
}

#[test]
fn the_submodule_text_shows_the_line_and_its_highlight() {
    let top = project();

    let result = find(top.path(), "needle");

    let file = result
        .files
        .iter()
        .find(|f| f.path == "service-a/src/main.rs")
        .unwrap();
    let (a, b) = file.lines[0].ranges[0];
    assert_eq!(&file.lines[0].text[a..b], "needle");
}

#[test]
fn untracked_files_of_a_submodule_are_searched_only_when_new_files_are_in_scope() {
    let top = project();

    let tracked = find_with(top.path(), "needle", tracked_only(), "");

    assert_eq!(
        hits(&tracked),
        [
            "README.md:1",
            "service-a/src/main.rs:1",
            "service-a/src/main.rs:2",
            "src/a.rs:1",
        ]
    );
}

#[test]
fn a_pathspec_inside_a_submodule_searches_only_that_folder_of_it() {
    let top = project();

    let result = find_with(
        top.path(),
        "needle",
        GrepOptions::default(),
        "service-a/src",
    );

    assert_eq!(
        hits(&result),
        [
            "service-a/src/main.rs:1",
            "service-a/src/main.rs:2",
            "service-a/src/zz.rs:1",
        ]
    );
}

#[test]
fn a_pathspec_naming_a_submodule_searches_all_of_it_and_not_the_repository() {
    let top = project();

    let result = find_with(top.path(), "needle", GrepOptions::default(), "service-a/");

    assert_eq!(
        hits(&result),
        [
            "service-a/src/main.rs:1",
            "service-a/src/main.rs:2",
            "service-a/src/zz.rs:1",
        ]
    );
}

#[test]
fn an_unrelated_pathspec_does_not_search_the_submodules() {
    let top = project();

    let result = find_with(top.path(), "needle", GrepOptions::default(), "README.md");

    assert_eq!(hits(&result), ["README.md:1"]);
}

#[test]
fn an_exclude_naming_a_submodule_skips_it() {
    let top = project();

    let result = find_with(top.path(), "needle", GrepOptions::default(), "!service-a");

    assert_eq!(hits(&result), ["README.md:1", "src/a.rs:1"]);
}

#[test]
fn an_exclude_inside_a_submodule_skips_that_file_only() {
    let top = project();

    let result = find_with(
        top.path(),
        "needle",
        GrepOptions::default(),
        "!service-a/src/zz.rs",
    );

    assert_eq!(
        hits(&result),
        [
            "README.md:1",
            "service-a/src/main.rs:1",
            "service-a/src/main.rs:2",
            "src/a.rs:1",
        ]
    );
}

#[test]
fn a_search_of_a_linked_worktree_submodule_finds_its_new_file() {
    let other = repo();
    let top = project_with_linked_worktree(other.path());
    write(&top.path().join("wt"), "fresh.txt", "needle fresh\n");

    let result = find(top.path(), "needle");

    assert_eq!(
        hits(&result),
        [
            "README.md:1",
            "wt/extra.rs:1",
            "wt/fresh.txt:1",
            "wt/src/wt.rs:1"
        ]
    );
}

#[test]
fn the_limits_of_the_total_and_the_file_apply_across_the_parts() {
    // Repository files a0..a4 (500 lines), submodule files b00..b09 (1000 lines),
    // repository files z0..z4 (500 lines): the total stops inside the submodule.
    let top = repo();
    for i in 0..5 {
        write(top.path(), &format!("a{i}.txt"), &"needle\n".repeat(100));
        write(top.path(), &format!("z{i}.txt"), &"needle\n".repeat(100));
    }
    commit_all(top.path());
    let lib = repo();
    for i in 0..10 {
        write(lib.path(), &format!("b{i:02}.txt"), &"needle\n".repeat(100));
    }
    commit_all(lib.path());
    git(
        top.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            lib.path().to_str().unwrap(),
            "service-a",
        ],
    );
    commit_all(top.path());

    let result = find(top.path(), "needle");

    assert_eq!(result.listed, GREP_TOTAL_LIMIT);
    assert!(result.truncated);
    assert!(result.files.iter().all(|f| !f.path.starts_with('z')));
    let last = result.files.last().unwrap();
    assert_eq!(last.path, "service-a/b04.txt");
    assert!(
        result
            .files
            .iter()
            .all(|f| f.lines.len() == GREP_FILE_LIMIT)
    );
}

#[test]
fn the_per_file_limit_applies_inside_a_submodule() {
    let top = project();
    let lib_file = top.path().join("service-a/long.txt");
    std::fs::write(&lib_file, "needle\n".repeat(150)).unwrap();

    let result = find(top.path(), "needle");

    let file = result
        .files
        .iter()
        .find(|f| f.path == "service-a/long.txt")
        .unwrap();
    assert_eq!(file.lines.len(), GREP_FILE_LIMIT);
    assert_eq!(file.more, 50);
    assert!(!result.truncated);
}

#[test]
fn a_stopped_search_over_submodules_fails_without_an_answer() {
    let top = project();
    let git = SubprocessGit::default();
    let root = git.resolve_root(top.path()).unwrap();
    let request = GrepRequest::new("needle", GrepOptions::default(), "").unwrap();
    let read = git.grep_text(&root, &request).unwrap();

    assert!(read(&AtomicBool::new(true)).is_err());
}

#[test]
fn the_same_search_gives_the_same_answer_twice() {
    let top = project();

    assert_eq!(find(top.path(), "needle"), find(top.path(), "needle"));
}
