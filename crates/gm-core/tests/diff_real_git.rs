//! `parse_unified` against the output of the real `git diff` in temporary repositories.

use std::path::Path;
use std::process::{Command, Output};

use gm_core::diff::parse_unified;
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
        ])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// Runs a diff command. `git diff --no-index` exits with 1 when the files differ.
fn diff_output(dir: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// The `added` and `removed` columns of `git diff --numstat`.
fn numstat(dir: &Path, args: &[&str]) -> (u32, u32) {
    let output = diff_output(dir, args);
    let text = String::from_utf8(output.stdout).unwrap();
    let mut columns = text.lines().next().unwrap().split('\t');
    let added = columns.next().unwrap().parse().unwrap();
    let removed = columns.next().unwrap().parse().unwrap();
    (added, removed)
}

/// Counts of added and removed lines in a parsed diff.
fn counts(diff: &FileDiff) -> (u32, u32) {
    let lines = diff.hunks.iter().flat_map(|h| &h.lines);
    let added = lines.clone().filter(|l| l.kind == LineKind::Added).count();
    let removed = lines.filter(|l| l.kind == LineKind::Removed).count();
    (
        u32::try_from(added).unwrap(),
        u32::try_from(removed).unwrap(),
    )
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

/// Runs `git diff -U3 --no-color --no-ext-diff` over `pathspec` and checks the
/// result for `path` against `--numstat`.
fn check_against_numstat(dir: &Path, path: &str, pathspec: &[&str], extra: &[&str]) -> FileDiff {
    let mut args = vec!["diff", "-U3", "--no-color", "--no-ext-diff"];
    args.extend_from_slice(extra);
    args.push("--");
    args.extend_from_slice(pathspec);
    let raw = diff_output(dir, &args).stdout;

    let mut stat_args = vec!["diff", "--numstat"];
    stat_args.extend_from_slice(extra);
    stat_args.push("--");
    stat_args.extend_from_slice(pathspec);
    let expected = numstat(dir, &stat_args);

    let parsed = parse_unified(path, &raw).unwrap();
    assert_eq!(counts(&parsed), expected, "line counts for {path}");
    parsed
}

#[test]
fn modified_file_matches_numstat() {
    let dir = repo();
    std::fs::write(
        dir.path().join("a.txt"),
        "one\nTWO\nthree\nfour\nfive\nsix\n",
    )
    .unwrap();

    let parsed = check_against_numstat(dir.path(), "a.txt", &["a.txt"], &[]);

    assert_eq!(parsed.path, "a.txt");
    assert_eq!(parsed.old_path, None);
    assert_eq!(parsed.hunks.len(), 1);
    let hunk = &parsed.hunks[0];
    assert_eq!((hunk.old_start, hunk.new_start), (1, 1));
    assert_eq!(hunk.lines[0].text, "one");
    assert_eq!(
        (hunk.lines[0].old_no, hunk.lines[0].new_no),
        (Some(1), Some(1))
    );
}

#[test]
fn staged_file_matches_numstat() {
    let dir = repo();
    std::fs::write(
        dir.path().join("a.txt"),
        "one\ntwo\nthree\nfour\nfive\nsix\n",
    )
    .unwrap();
    git(dir.path(), &["add", "a.txt"]);
    std::fs::write(
        dir.path().join("a.txt"),
        "zero\none\ntwo\nthree\nfour\nfive\nsix\n",
    )
    .unwrap();

    let staged = check_against_numstat(dir.path(), "a.txt", &["a.txt"], &["--cached"]);
    assert_eq!(staged.hunks.len(), 1);
    assert_eq!(staged.hunks[0].lines[0].text, "three");
    assert_eq!(staged.hunks[0].lines.last().unwrap().text, "six");
}

#[test]
fn renamed_file_with_edits_matches_numstat() {
    let dir = repo();
    git(dir.path(), &["mv", "a.txt", "renamed.txt"]);
    std::fs::write(
        dir.path().join("renamed.txt"),
        "one\nTWO\nthree\nfour\nfive\n",
    )
    .unwrap();
    git(dir.path(), &["add", "renamed.txt"]);

    let parsed = check_against_numstat(
        dir.path(),
        "renamed.txt",
        &["a.txt", "renamed.txt"],
        &["--cached", "-M"],
    );

    assert_eq!(parsed.path, "renamed.txt");
    assert_eq!(parsed.old_path.as_deref(), Some("a.txt"));
    assert_eq!(parsed.hunks.len(), 1);
}

#[test]
fn new_file_without_trailing_newline_matches_numstat() {
    let dir = repo();
    std::fs::write(dir.path().join("new.txt"), "x\ny").unwrap();

    let raw = diff_output(
        dir.path(),
        &[
            "diff",
            "-U3",
            "--no-color",
            "--no-ext-diff",
            "--no-index",
            "--",
            "/dev/null",
            "new.txt",
        ],
    )
    .stdout;
    let expected = numstat(
        dir.path(),
        &[
            "diff",
            "--numstat",
            "--no-index",
            "--",
            "/dev/null",
            "new.txt",
        ],
    );

    let parsed = parse_unified("new.txt", &raw).unwrap();
    assert_eq!(counts(&parsed), expected);
    assert_eq!(parsed.hunks.len(), 1);
    let lines = &parsed.hunks[0].lines;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].old_no, None);
    assert!(!lines[0].no_newline);
    assert_eq!(lines[1].text, "y");
    assert!(lines[1].no_newline);
}
