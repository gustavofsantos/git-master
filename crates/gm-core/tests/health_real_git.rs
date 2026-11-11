//! The code health index against real git in temporary repositories (D225 (5)).

use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_core::health::ingest::{ingest, read_commits, resolve, window};
use gm_core::health::store::{HealthStore, db_path};
use gm_core::health::{CommitRecord, FileChange};
use gm_schema::{
    Actor, Command as OpCommand, CommandKind, Event, HealthProgress, HealthReport, OperationId,
    OperationState,
};
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

/// A repository where `a.rs` changes in five commits, `b.rs` once, and `gone.rs` is deleted at
/// the end; two authors. Commit `k` is dated `T0 + k` days.
fn health_repo(dir: &Path) {
    Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(dir)
        .status()
        .unwrap();
    write(dir, "a.rs", b"v0\n");
    write(dir, "b.rs", b"v0\n");
    write(dir, "gone.rs", b"v0\n");
    git_at(dir, ALICE, T0, &["add", "."]);
    git_at(dir, ALICE, T0, &["commit", "-q", "-m", "root"]);
    for k in 1_i64..=5 {
        let author = if k % 2 == 0 { BOB } else { ALICE };
        write(dir, "a.rs", format!("v{k}\n").as_bytes());
        let message = format!("a {k}");
        git_at(dir, author, T0 + k * DAY, &["commit", "-qam", &message]);
    }
    write(dir, "b.rs", b"v1\n");
    git_at(dir, BOB, T0 + 6 * DAY, &["commit", "-qam", "b once"]);
    git_at(dir, ALICE, T0 + 7 * DAY, &["rm", "-q", "gone.rs"]);
    git_at(
        dir,
        ALICE,
        T0 + 7 * DAY,
        &["commit", "-q", "-m", "drop gone"],
    );
}

/// An engine with `root` open, as the MCP session opens it.
fn engine_on(root: &Path) -> Engine {
    let git = SubprocessGit {
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    let events = engine.handle(
        OperationId(1),
        &OpCommand {
            actor: Actor::Human,
            kind: CommandKind::OpenRepo {
                path: root.to_string_lossy().into_owned(),
            },
        },
    );
    assert_eq!(final_state(&events).0, OperationState::Done, "{events:?}");
    engine
}

/// Runs a command as the MCP session does: the events of `handle`, then each health job run to
/// its end and its outcome passed back.
fn run(engine: &mut Engine, id: u64, kind: CommandKind) -> Vec<Event> {
    let mut events = engine.handle(
        OperationId(id),
        &OpCommand {
            actor: Actor::Human,
            kind,
        },
    );
    for job in engine.take_health_jobs() {
        let outcome = job.run(&mut |_| {});
        events.extend(engine.health_report(outcome));
    }
    events
}

fn load_health(rev: Option<&str>) -> CommandKind {
    CommandKind::LoadHealth {
        rev: rev.map(str::to_owned),
    }
}

/// The state and detail of the last `OperationChanged` of `events`.
fn final_state(events: &[Event]) -> (OperationState, Option<String>) {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::OperationChanged { state, detail, .. } => Some((*state, detail.clone())),
            _ => None,
        })
        .expect("an operation changed state")
}

/// The last report `events` carry.
fn loaded(events: &[Event]) -> &HealthReport {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::HealthLoaded { report, .. } => Some(report.as_ref()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no HealthLoaded in {events:?}"))
}

#[test]
fn load_health_answers_with_the_report_of_head() {
    let dir = tempfile::tempdir().unwrap();
    health_repo(dir.path());
    let mut engine = engine_on(dir.path());

    let events = run(&mut engine, 2, load_health(None));

    assert_eq!(final_state(&events).0, OperationState::Done);
    let report = loaded(&events);
    assert!(report.complete, "the index holds the whole window");
    assert_eq!(report.rev, head(dir.path()));
    assert_eq!(report.files[0].path, "a.rs");
    assert_eq!(report.files[0].rank, 1);
    assert!(
        report.files.iter().all(|f| f.path != "gone.rs"),
        "a deleted file is not reported"
    );
}

#[test]
fn a_second_load_of_the_same_commit_answers_from_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    health_repo(dir.path());
    let mut engine = engine_on(dir.path());
    let first = run(&mut engine, 2, load_health(None));

    let events = engine.handle(
        OperationId(3),
        &OpCommand {
            actor: Actor::Human,
            kind: load_health(None),
        },
    );

    assert!(
        engine.take_health_jobs().is_empty(),
        "no job for a cached report"
    );
    assert_eq!(final_state(&events).0, OperationState::Done);
    assert_eq!(loaded(&events), loaded(&first));
}

#[test]
fn a_new_commit_starts_a_job_at_a_new_rev() {
    let dir = tempfile::tempdir().unwrap();
    health_repo(dir.path());
    let mut engine = engine_on(dir.path());
    let before = loaded(&run(&mut engine, 2, load_health(None))).rev.clone();
    write(dir.path(), "a.rs", b"again\n");
    git_at(
        dir.path(),
        BOB,
        T0 + 8 * DAY,
        &["commit", "-qam", "a again"],
    );

    let events = run(&mut engine, 3, load_health(None));

    let report = loaded(&events);
    assert_eq!(report.rev, head(dir.path()));
    assert_ne!(report.rev, before);
    assert!(report.complete);
}

#[test]
fn a_changed_setting_starts_a_job_with_another_config_hash() {
    let dir = tempfile::tempdir().unwrap();
    health_repo(dir.path());
    let mut engine = engine_on(dir.path());
    let before = loaded(&run(&mut engine, 2, load_health(None)))
        .config_hash
        .clone();
    git_at(
        dir.path(),
        ALICE,
        T0,
        &["config", "gitmaster.health.halfLife", "10"],
    );

    let events = engine.handle(
        OperationId(3),
        &OpCommand {
            actor: Actor::Human,
            kind: load_health(None),
        },
    );
    assert_eq!(
        engine.take_health_jobs().len(),
        1,
        "a changed setting needs a job"
    );
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::HealthLoaded { .. }))
    );

    let report = loaded(&run(&mut engine, 4, load_health(None))).clone();
    assert_ne!(report.config_hash, before);
}

#[test]
fn a_bad_rev_fails_the_operation_with_a_message() {
    let dir = tempfile::tempdir().unwrap();
    health_repo(dir.path());
    let mut engine = engine_on(dir.path());

    let events = run(&mut engine, 2, load_health(Some("no-such-rev")));

    let (state, detail) = final_state(&events);
    assert_eq!(state, OperationState::Failed);
    assert!(detail.is_some_and(|m| m.contains("no-such-rev")));
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::HealthLoaded { .. }))
    );
}
