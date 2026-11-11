//! The changes between two revisions (D231), read with real git in temporary
//! repositories: the range's file list and totals, the merge base, renames,
//! binary files, the stack of the range, the refusals and the file limit.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use gm_core::git::SubprocessGit;
use gm_core::{GitError, GitPort};
use gm_schema::{Change, CommitFile, DETAIL_FILE_LIMIT, FileDiff, RangeMode, RevisionRange};

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

fn rev(dir: &Path, name: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", name])
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

fn write(dir: &Path, path: &str, bytes: &[u8]) {
    std::fs::write(dir.join(path), bytes).unwrap();
}

fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// The range between `from` and `to`, read the way the engine reads it.
fn range(dir: &Path, from: &str, to: &str, mode: RangeMode) -> Result<RevisionRange, GitError> {
    let stop = AtomicBool::new(false);
    SubprocessGit::default()
        .revision_range(dir, from, to, mode)
        .unwrap()(&stop)
}

fn file<'a>(range: &'a RevisionRange, path: &str) -> &'a CommitFile {
    range
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} is not in the range"))
}

/// Two tags: `v1` has a.txt and b.txt; `v2` changes a.txt, adds c.txt, deletes b.txt.
fn two_tags() -> tempfile::TempDir {
    let dir = repo();
    let path = dir.path();
    write(path, "a.txt", b"1\n2\n3\n");
    write(path, "b.txt", b"x\n");
    commit(path, "one");
    git(path, &["tag", "v1"]);
    write(path, "a.txt", b"1\ntwo\n3\n");
    write(path, "c.txt", b"p\nq\n");
    std::fs::remove_file(path.join("b.txt")).unwrap();
    commit(path, "two");
    git(path, &["tag", "v2"]);
    dir
}

#[test]
fn direct_between_two_tags_lists_the_files_with_their_letters_and_counts() {
    let dir = two_tags();
    let path = dir.path();

    let read = range(path, "v1", "v2", RangeMode::Direct).unwrap();

    assert_eq!(read.from_oid, rev(path, "v1"));
    assert_eq!(read.to_oid, rev(path, "v2"));
    assert_eq!(read.base, read.from_oid);
    assert_eq!(read.tip, read.to_oid);
    assert_eq!(
        read.from_short,
        rev(path, "v1").chars().take(7).collect::<String>()
    );
    assert_eq!(read.files.len(), 3);

    let a = file(&read, "a.txt");
    assert_eq!((a.change, a.added, a.deleted), (Change::Modified, 1, 1));
    let b = file(&read, "b.txt");
    assert_eq!((b.change, b.added, b.deleted), (Change::Deleted, 0, 1));
    let c = file(&read, "c.txt");
    assert_eq!((c.change, c.added, c.deleted), (Change::Added, 2, 0));

    assert_eq!((read.added, read.deleted), (3, 2));
    assert_eq!(read.more_files, 0);
}

#[test]
fn since_common_ancestor_lists_only_the_branch_own_files_after_the_base_advanced() {
    let dir = repo();
    let path = dir.path();
    write(path, "base.txt", b"base\n");
    commit(path, "base");
    git(path, &["checkout", "-q", "-b", "feature"]);
    write(path, "feature.txt", b"feature\n");
    commit(path, "feature work");
    git(path, &["checkout", "-q", "main"]);
    write(path, "main_only.txt", b"landed later\n");
    commit(path, "main moves on");
    let merge_base = rev(path, "main~1");

    let read = range(path, "main", "feature", RangeMode::SinceCommonAncestor).unwrap();

    assert_eq!(read.base, merge_base);
    assert_eq!(read.from_oid, rev(path, "main"));
    let paths: Vec<&str> = read.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["feature.txt"]);
    assert_eq!(file(&read, "feature.txt").change, Change::Added);

    // Direct compares the two trees, so the base's own later file shows up there.
    let direct = range(path, "main", "feature", RangeMode::Direct).unwrap();
    assert!(direct.files.iter().any(|f| f.path == "main_only.txt"));
}

#[test]
fn a_rename_is_reported_with_its_old_path() {
    let dir = repo();
    let path = dir.path();
    write(path, "a.txt", b"one\ntwo\nthree\nfour\n");
    commit(path, "add");
    git(path, &["tag", "before"]);
    git(path, &["mv", "a.txt", "renamed.txt"]);
    commit(path, "rename");

    let read = range(path, "before", "HEAD", RangeMode::Direct).unwrap();

    assert_eq!(read.files.len(), 1);
    let renamed = &read.files[0];
    assert_eq!(renamed.change, Change::Renamed);
    assert_eq!(renamed.path, "renamed.txt");
    assert_eq!(renamed.old_path.as_deref(), Some("a.txt"));
    assert_eq!((renamed.added, renamed.deleted), (0, 0));
}

#[test]
fn a_binary_file_is_marked_binary_and_counts_zero_lines() {
    let dir = repo();
    let path = dir.path();
    write(path, "a.txt", b"text\n");
    commit(path, "one");
    git(path, &["tag", "before"]);
    write(path, "image.bin", b"\0\x01\x02binary\0\0");
    write(path, "a.txt", b"text\nmore\n");
    commit(path, "two");

    let read = range(path, "before", "HEAD", RangeMode::Direct).unwrap();

    let binary = file(&read, "image.bin");
    assert!(binary.binary);
    assert_eq!((binary.added, binary.deleted), (0, 0));
    assert_eq!(file(&read, "a.txt").added, 1);
    assert_eq!(read.added, 1, "the binary file adds nothing to the totals");
}

#[test]
fn the_stack_of_a_range_has_one_diff_per_listed_file_in_order() {
    let dir = two_tags();
    let path = dir.path();
    let read = range(path, "v1", "v2", RangeMode::Direct).unwrap();

    let stop = AtomicBool::new(false);
    let stack = SubprocessGit::default()
        .revision_stack(path, &read.base, &read.tip, &read.files)
        .unwrap();
    let diffs: Vec<FileDiff> = stack(&stop).unwrap();

    let listed: Vec<&str> = read.files.iter().map(|f| f.path.as_str()).collect();
    let got: Vec<&str> = diffs.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(got, listed);
    let a = diffs.iter().find(|d| d.path == "a.txt").unwrap();
    assert!(!a.hunks.is_empty());
    assert!(!a.binary);
}

#[test]
fn an_unknown_revision_fails_with_gits_message_naming_its_side() {
    let dir = two_tags();
    let path = dir.path();

    let to_unknown = range(path, "v1", "no-such-tag", RangeMode::Direct).unwrap_err();
    assert!(to_unknown.0.starts_with("to: "), "{}", to_unknown.0);
    assert!(to_unknown.0.len() > "to: ".len());

    let from_unknown = range(path, "no-such-tag", "v2", RangeMode::Direct).unwrap_err();
    assert!(from_unknown.0.starts_with("from: "), "{}", from_unknown.0);
}

#[test]
fn a_revision_that_starts_with_a_dash_is_refused_and_git_never_runs() {
    let dir = two_tags();
    let path = dir.path();
    let marker = path.join("marker.txt");

    // Had git run with this as an option, it would have written the file.
    let refused = SubprocessGit::default().revision_range(
        path,
        "--output=marker.txt",
        "v2",
        RangeMode::Direct,
    );
    assert_eq!(
        refused.err().unwrap().0,
        "from: \"--output=marker.txt\" is not a revision"
    );
    let refused = SubprocessGit::default().revision_range(path, "v1", "-x", RangeMode::Direct);
    assert!(refused.is_err());
    assert!(!marker.exists());
}

#[test]
fn since_common_ancestor_of_unrelated_histories_fails_with_the_no_common_ancestor_message() {
    let dir = two_tags();
    let path = dir.path();
    git(path, &["checkout", "-q", "--orphan", "other"]);
    git(path, &["rm", "-q", "-rf", "."]);
    write(path, "lone.txt", b"alone\n");
    commit(path, "unrelated");

    let error = range(path, "v2", "other", RangeMode::SinceCommonAncestor).unwrap_err();
    assert_eq!(error.0, "v2 and other have no common ancestor");

    // Direct does not need a common ancestor: it compares the two trees.
    let direct = range(path, "v2", "other", RangeMode::Direct).unwrap();
    assert!(direct.files.iter().any(|f| f.path == "lone.txt"));
}

#[test]
fn a_revision_compared_with_itself_has_no_files() {
    let dir = two_tags();
    let path = dir.path();

    let read = range(path, "v2", "v2", RangeMode::Direct).unwrap();
    assert!(read.files.is_empty());
    assert_eq!((read.added, read.deleted, read.more_files), (0, 0, 0));

    let since = range(path, "v2", "v2", RangeMode::SinceCommonAncestor).unwrap();
    assert!(since.files.is_empty());
}

#[test]
fn more_than_the_limit_of_changed_files_sets_more_files_and_totals_cover_them_all() {
    let dir = repo();
    let path = dir.path();
    write(path, "seed.txt", b"seed\n");
    commit(path, "seed");
    git(path, &["tag", "before"]);
    let total = DETAIL_FILE_LIMIT + 5;
    for index in 0..total {
        write(path, &format!("f{index:04}.txt"), b"line\nline\n");
    }
    commit(path, "many");

    let read = range(path, "before", "HEAD", RangeMode::Direct).unwrap();

    assert_eq!(read.files.len(), DETAIL_FILE_LIMIT);
    assert_eq!(read.more_files, 5);
    assert_eq!(read.added, 2 * u32::try_from(total).unwrap());
    assert_eq!(read.deleted, 0);
}
