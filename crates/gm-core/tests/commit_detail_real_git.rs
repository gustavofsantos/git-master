//! The commit detail read against real repositories in temporary directories.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use gm_core::GitPort;
use gm_core::git::SubprocessGit;
use gm_schema::{Change, CommitDetail, CommitFile, DETAIL_FILE_LIMIT};

const WHEN: &str = "2024-01-01T10:00:00+00:00";

/// Runs `git` in `dir` with a fixed identity and both dates set to `when`.
fn git_at(dir: &Path, when: &str, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=Zoë Ünal",
            "-c",
            "user.email=zoe@example.test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "merge.ff=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_DATE", when)
        .env("GIT_COMMITTER_DATE", when)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn git(dir: &Path, args: &[&str]) {
    git_at(dir, WHEN, args);
}

fn head(dir: &Path) -> String {
    rev(dir, "HEAD")
}

fn rev(dir: &Path, spec: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", spec])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "rev-parse {spec} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    dir
}

/// The detail of `oid` in `dir`, read the way the engine reads it: start the
/// read, then run it with no stop request.
fn detail_of(dir: &Path, oid: &str) -> CommitDetail {
    let root = SubprocessGit::default().resolve_root(dir).unwrap();
    let read = SubprocessGit::default().commit_detail(&root, oid).unwrap();
    read(&AtomicBool::new(false)).unwrap()
}

fn file<'a>(detail: &'a CommitDetail, path: &str) -> &'a CommitFile {
    detail
        .files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} is not listed in {:?}", detail.files))
}

fn write(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

#[test]
fn add_modify_delete_and_rename_in_one_commit() {
    let dir = repo();
    let path = dir.path();
    write(path, "keep.txt", "one\ntwo\nthree\n");
    write(path, "gone.txt", "bye\n");
    write(path, "old.txt", "moved\n");
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "base"]);

    write(path, "keep.txt", "one\nTWO\nthree\nfour\n");
    std::fs::remove_file(path.join("gone.txt")).unwrap();
    git(path, &["mv", "old.txt", "new.txt"]);
    write(path, "added.txt", "a\nb\n");
    git(path, &["add", "-A"]);
    git(path, &["commit", "-q", "-m", "feat: all four kinds"]);
    let oid = head(path);

    let detail = detail_of(path, &oid);

    assert_eq!(detail.files.len(), 4, "{:?}", detail.files);
    let added = file(&detail, "added.txt");
    assert_eq!(added.change, Change::Added);
    assert_eq!((added.added, added.deleted, added.binary), (2, 0, false));
    let modified = file(&detail, "keep.txt");
    assert_eq!(modified.change, Change::Modified);
    assert_eq!((modified.added, modified.deleted), (2, 1));
    let deleted = file(&detail, "gone.txt");
    assert_eq!(deleted.change, Change::Deleted);
    assert_eq!((deleted.added, deleted.deleted), (0, 1));
    let renamed = file(&detail, "new.txt");
    assert_eq!(renamed.change, Change::Renamed);
    assert_eq!(renamed.old_path.as_deref(), Some("old.txt"));
    assert_eq!((renamed.added, renamed.deleted), (0, 0));
    assert_eq!(detail.more_files, 0);
}

#[test]
fn a_binary_file_has_no_line_counts() {
    let dir = repo();
    let path = dir.path();
    std::fs::write(path.join("image.bin"), [0u8, 1, 2, 3]).unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "base"]);
    std::fs::write(path.join("image.bin"), [0u8, 9, 9, 9, 0]).unwrap();
    git(path, &["commit", "-q", "-am", "binary change"]);

    let detail = detail_of(path, &head(path));

    let binary = file(&detail, "image.bin");
    assert_eq!(binary.change, Change::Modified);
    assert!(binary.binary);
    assert_eq!((binary.added, binary.deleted), (0, 0));
}

#[test]
fn a_root_commit_lists_every_file_as_added_and_has_no_parents() {
    let dir = repo();
    let path = dir.path();
    write(path, "README.md", "# hi\n");
    write(path, "src.rs", "fn main() {}\n");
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "root"]);

    let detail = detail_of(path, &head(path));

    assert!(detail.parents.is_empty());
    assert_eq!(detail.files.len(), 2);
    assert!(detail.files.iter().all(|f| f.change == Change::Added));
    assert_eq!(file(&detail, "README.md").added, 1);
}

#[test]
fn a_merge_lists_the_changes_against_the_first_parent_only() {
    let dir = repo();
    let path = dir.path();
    write(path, "base.txt", "base\n");
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "base"]);
    let base = head(path);

    git(path, &["checkout", "-q", "-b", "feat"]);
    write(path, "feature.txt", "from feat\n");
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "feat: work"]);

    git(path, &["checkout", "-q", "main"]);
    write(path, "main-only.txt", "from main\n");
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "main: work"]);
    let first_parent = head(path);

    git(
        path,
        &["merge", "-q", "--no-ff", "feat", "-m", "Merge feat"],
    );
    let merge = head(path);

    let detail = detail_of(path, &merge);

    assert_eq!(detail.parents.len(), 2, "{:?}", detail.parents);
    assert!(rev(path, &format!("{merge}^1")).starts_with(&detail.parents[0]));
    assert!(rev(path, &format!("{merge}^2")).starts_with(&detail.parents[1]));
    // The first parent is main, so main's own file is not a change of the merge.
    assert_eq!(detail.files.len(), 1, "{:?}", detail.files);
    assert_eq!(file(&detail, "feature.txt").change, Change::Added);
    assert_ne!(first_parent, base);
}

#[test]
fn the_body_keeps_every_paragraph() {
    let dir = repo();
    let path = dir.path();
    write(path, "a.txt", "a\n");
    git(path, &["add", "."]);
    git(
        path,
        &[
            "commit",
            "-q",
            "-m",
            "feat: subject",
            "-m",
            "First paragraph,\nwrapped by hand.",
            "-m",
            "Second paragraph.",
            "-m",
            "Third paragraph.",
        ],
    );

    let detail = detail_of(path, &head(path));

    assert_eq!(detail.subject, "feat: subject");
    assert_eq!(
        detail.body,
        "First paragraph,\nwrapped by hand.\n\nSecond paragraph.\n\nThird paragraph."
    );
}

#[test]
fn a_commit_without_a_body_has_an_empty_one() {
    let dir = repo();
    let path = dir.path();
    write(path, "a.txt", "a\n");
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "just a subject"]);

    let detail = detail_of(path, &head(path));

    assert_eq!(detail.body, "");
}

#[test]
fn the_header_has_the_full_id_author_and_date() {
    let dir = repo();
    let path = dir.path();
    write(path, "a.txt", "a\n");
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "subject"]);
    let oid = head(path);

    let detail = detail_of(path, &oid);

    assert_eq!(detail.oid, oid);
    assert!(oid.starts_with(&detail.short));
    assert_eq!(detail.author_name, "Zoë Ünal");
    assert_eq!(detail.author_email, "zoe@example.test");
    assert_eq!(detail.author_date, "2024-01-01 10:00:00 +0000");
}

#[test]
fn unicode_in_the_subject_body_and_paths_reads_back_intact() {
    let dir = repo();
    let path = dir.path();
    write(path, "café-日本.txt", "ok\n");
    git(path, &["add", "."]);
    git(
        path,
        &[
            "commit",
            "-q",
            "-m",
            "feat: café ☕ 日本語",
            "-m",
            "Corpo com acentos: ção.",
        ],
    );

    let detail = detail_of(path, &head(path));

    assert_eq!(detail.subject, "feat: café ☕ 日本語");
    assert_eq!(detail.body, "Corpo com acentos: ção.");
    assert_eq!(file(&detail, "café-日本.txt").change, Change::Added);
}

#[test]
fn a_huge_commit_lists_the_cap_and_counts_the_rest() {
    let dir = repo();
    let path = dir.path();
    let total = DETAIL_FILE_LIMIT + 50;
    for index in 0..total {
        write(path, &format!("f{index:04}.txt"), "x\n");
    }
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "many files"]);

    let detail = detail_of(path, &head(path));

    assert_eq!(detail.files.len(), DETAIL_FILE_LIMIT);
    assert_eq!(detail.more_files, 50);
}

#[test]
fn a_superseded_read_ends_without_an_answer() {
    let dir = repo();
    let path = dir.path();
    write(path, "a.txt", "a\n");
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "subject"]);
    let root = SubprocessGit::default().resolve_root(path).unwrap();
    let read = SubprocessGit::default()
        .commit_detail(&root, &head(path))
        .unwrap();

    let stop = AtomicBool::new(true);

    assert!(read(&stop).is_err());
}

#[test]
fn an_id_that_is_not_hex_is_refused_before_git_runs() {
    let dir = repo();
    let path = dir.path();
    write(path, "a.txt", "a\n");
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "subject"]);
    let root = SubprocessGit::default().resolve_root(path).unwrap();

    assert!(
        SubprocessGit::default()
            .commit_detail(&root, "--output=/tmp/x")
            .is_err()
    );
    assert!(SubprocessGit::default().commit_detail(&root, "").is_err());
    assert!(
        SubprocessGit::default()
            .commit_detail(&root, &head(path))
            .is_ok()
    );
}

#[test]
fn an_unknown_commit_gives_gits_own_message() {
    let dir = repo();
    let path = dir.path();
    write(path, "a.txt", "a\n");
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "subject"]);
    let root = SubprocessGit::default().resolve_root(path).unwrap();
    let missing = "0123456789abcdef0123456789abcdef01234567";
    let read = SubprocessGit::default()
        .commit_detail(&root, missing)
        .unwrap();

    let error = read(&AtomicBool::new(false)).unwrap_err();

    assert!(
        error.0.contains("bad object") || error.0.contains("unknown revision"),
        "{error}"
    );
}
