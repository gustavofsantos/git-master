//! The code health index against real git in temporary repositories (D225 (5)).

use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use gm_core::health::ingest::{ingest, read_commits, resolve, window};
use gm_core::health::store::{HealthStore, db_path};
use gm_core::health::{CommitRecord, FileChange};
use gm_schema::HealthProgress;
use sha2::{Digest, Sha256};

const T0: i64 = 1_700_000_000;
const DAY: i64 = 86_400;
const ALICE: (&str, &str) = ("Alice", "alice@example.com");
const BOB: (&str, &str) = ("Bob", "BOB@example.com");

/// Runs git in `dir` as `author`, with the committer and author dates set to `time`.
fn git_at(dir: &Path, author: (&str, &str), time: i64, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", author.0)
        .env("GIT_AUTHOR_EMAIL", author.1)
        .env("GIT_COMMITTER_NAME", author.0)
        .env("GIT_COMMITTER_EMAIL", author.1)
        .env("GIT_AUTHOR_DATE", format!("@{time} +0000"))
        .env("GIT_COMMITTER_DATE", format!("@{time} +0000"))
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

fn subject(dir: &Path, sha: &str) -> String {
    let out = Command::new("git")
        .args(["log", "-1", "--format=%s", sha])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn write(dir: &Path, path: &str, content: &[u8]) {
    let full = dir.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(full, content).unwrap();
}

/// A repository with: a root commit by Alice (a file with a space, a unicode file, a binary
/// file); a commit by Bob editing a file; a side branch by Alice with a rename; a merge by Bob.
/// Commit `k` is dated `T0 + k` days, so the commits are in order of their creation.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(d)
        .status()
        .unwrap();

    write(d, "a.txt", b"one\ntwo\n");
    write(d, "my file.txt", b"spaced\n");
    write(d, "ünï/ß.txt", b"unicode\n");
    write(d, "logo.bin", &[0, 1, 2, 255, 0, 7]);
    git_at(d, ALICE, T0, &["add", "."]);
    git_at(d, ALICE, T0, &["commit", "-q", "-m", "root"]);

    write(d, "a.txt", b"one\ntwo\nthree\n");
    git_at(d, BOB, T0 + DAY, &["commit", "-qam", "bob edits a"]);

    git_at(d, ALICE, T0 + 2 * DAY, &["checkout", "-q", "-b", "side"]);
    git_at(d, ALICE, T0 + 2 * DAY, &["mv", "a.txt", "renamed.txt"]);
    write(d, "renamed.txt", b"one\ntwo\nthree\nfour\n");
    git_at(d, ALICE, T0 + 2 * DAY, &["add", "renamed.txt"]);
    git_at(d, ALICE, T0 + 2 * DAY, &["commit", "-q", "-m", "rename a"]);

    git_at(d, BOB, T0 + 3 * DAY, &["checkout", "-q", "main"]);
    write(d, "my file.txt", b"spaced again\n");
    git_at(
        d,
        BOB,
        T0 + 3 * DAY,
        &["commit", "-qam", "bob edits spaced"],
    );

    git_at(
        d,
        BOB,
        T0 + 4 * DAY,
        &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
    );
    dir
}

/// Every commit of the fixture, as (sha, subject), from `git log`.
fn all_commits(dir: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args(["rev-list", "--all"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn find<'a>(records: &'a [CommitRecord], dir: &Path, subject_text: &str) -> &'a CommitRecord {
    records
        .iter()
        .find(|r| subject(dir, &r.sha) == subject_text)
        .unwrap_or_else(|| panic!("no commit {subject_text}"))
}

fn author_key(email: &str) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(email.to_lowercase().as_bytes());
    let mut out = String::new();
    for byte in &digest[..8] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[test]
fn resolve_gives_the_full_id_and_committer_time_of_head() {
    let dir = fixture();
    let (sha, time) = resolve(dir.path(), "HEAD").unwrap();
    assert_eq!(sha, head(dir.path()));
    assert_eq!(time, T0 + 4 * DAY);
}

#[test]
fn resolve_reports_a_bad_rev_and_an_unborn_head() {
    let dir = fixture();
    let error = resolve(dir.path(), "no-such-rev").unwrap_err();
    assert!(error.contains("no-such-rev"), "{error}");

    let empty = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(empty.path())
        .status()
        .unwrap();
    let error = resolve(empty.path(), "HEAD").unwrap_err();
    assert!(error.contains("HEAD"), "{error}");
}

#[test]
fn window_is_newest_first_and_honours_since() {
    let dir = fixture();
    let d = dir.path();
    let all = window(d, "HEAD", 0).unwrap();
    assert_eq!(all.len(), 5);
    assert_eq!(all[0], head(d));
    assert_eq!(subject(d, &all[4]), "root");
    let times: Vec<i64> = all.iter().map(|sha| resolve(d, sha).unwrap().1).collect();
    assert!(times.windows(2).all(|pair| pair[0] >= pair[1]), "{times:?}");

    let recent = window(d, "HEAD", T0 + 2 * DAY).unwrap();
    assert_eq!(recent.len(), 3);
    assert!(!recent.contains(&all[4]));
}

#[test]
fn ingest_indexes_every_commit_with_its_changes() {
    let dir = fixture();
    let d = dir.path();
    let mut store = HealthStore::open(&db_path(&d.join(".git"))).unwrap();
    let cancel = AtomicBool::new(false);
    let mut calls: Vec<HealthProgress> = Vec::new();
    let progress = ingest(d, &mut store, "HEAD", 0, &cancel, &mut |p| calls.push(p)).unwrap();

    assert_eq!(
        progress,
        HealthProgress {
            indexed: 5,
            total: 5
        }
    );
    assert_eq!(calls.last(), Some(&progress));

    let shas = window(d, "HEAD", 0).unwrap();
    assert!(store.missing(&shas).unwrap().is_empty());
    let records = store.load(&shas).unwrap();
    assert_eq!(records.len(), 5);

    let rename = find(&records, d, "rename a");
    assert_eq!(rename.parents, 1);
    assert_eq!(
        rename.changes,
        vec![FileChange {
            path: "renamed.txt".to_owned(),
            old_path: Some("a.txt".to_owned()),
            added: Some(1),
            deleted: Some(0),
        }]
    );

    let root = find(&records, d, "root");
    assert_eq!(root.parents, 0);
    assert_eq!(root.author, author_key("alice@example.com"));
    let paths: Vec<&str> = root.changes.iter().map(|c| c.path.as_str()).collect();
    assert!(paths.contains(&"my file.txt"), "{paths:?}");
    assert!(paths.contains(&"ünï/ß.txt"), "{paths:?}");
    let binary = root.changes.iter().find(|c| c.path == "logo.bin").unwrap();
    assert_eq!((binary.added, binary.deleted), (None, None));

    let merge = find(&records, d, "merge side");
    assert_eq!(merge.parents, 2);
    assert!(merge.changes.is_empty());

    let bob = find(&records, d, "bob edits a");
    assert_eq!(bob.author, author_key("bob@example.com"));
    assert_eq!(bob.time, T0 + DAY);
}

#[test]
fn a_second_ingest_reads_nothing() {
    let dir = fixture();
    let d = dir.path();
    let mut store = HealthStore::open(&db_path(&d.join(".git"))).unwrap();
    let cancel = AtomicBool::new(false);
    ingest(d, &mut store, "HEAD", 0, &cancel, &mut |_| {}).unwrap();

    let mut calls = Vec::new();
    let progress = ingest(d, &mut store, "HEAD", 0, &cancel, &mut |p| calls.push(p)).unwrap();
    assert_eq!(
        progress,
        HealthProgress {
            indexed: 5,
            total: 5
        }
    );
    assert_eq!(calls, vec![progress]);
}

#[test]
fn a_cancelled_ingest_leaves_a_state_that_a_later_call_resumes() {
    let dir = fixture();
    let d = dir.path();
    let mut store = HealthStore::open(&db_path(&d.join(".git"))).unwrap();
    let cancelled = AtomicBool::new(true);
    let error = ingest(d, &mut store, "HEAD", 0, &cancelled, &mut |_| {}).unwrap_err();
    assert_eq!(error, "cancelled");
    let shas = window(d, "HEAD", 0).unwrap();
    assert_eq!(store.missing(&shas).unwrap().len(), 5);

    let running = AtomicBool::new(false);
    let progress = ingest(d, &mut store, "HEAD", 0, &running, &mut |_| {}).unwrap();
    assert_eq!(
        progress,
        HealthProgress {
            indexed: 5,
            total: 5
        }
    );
    assert!(store.missing(&shas).unwrap().is_empty());
}

#[test]
fn two_stores_on_one_file_never_duplicate_changes() {
    let dir = fixture();
    let d = dir.path();
    let path = db_path(&d.join(".git"));
    let mut first = HealthStore::open(&path).unwrap();
    let mut second = HealthStore::open(&path).unwrap();
    let shas = all_commits(d);
    let cancel = AtomicBool::new(false);
    let records = read_commits(d, &shas, &cancel).unwrap();

    assert_eq!(first.insert(&records).unwrap(), 5);
    assert_eq!(second.insert(&records).unwrap(), 0);
    let loaded = first.load(&shas).unwrap();
    assert_eq!(loaded.len(), 5);
    let changes: usize = loaded.iter().map(|r| r.changes.len()).sum();
    let expected: usize = records.iter().map(|r| r.changes.len()).sum();
    assert_eq!(changes, expected);
    assert_eq!(second.load(&shas).unwrap(), loaded);
}

#[test]
fn read_commits_of_nothing_is_empty() {
    let dir = fixture();
    let cancel = AtomicBool::new(false);
    assert!(read_commits(dir.path(), &[], &cancel).unwrap().is_empty());
}
