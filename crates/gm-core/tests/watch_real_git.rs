//! The watcher against real repositories in temporary directories. Each wait
//! has a bound and fails fast; quiet periods are observed, not slept through.

use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gm_core::EngineHandle;
use gm_core::git::SubprocessGit;
use gm_core::hook::ApprovalStore;
use gm_core::watch::{self, Notice, WatchConfig, WatchHandle};
use gm_schema::{Actor, CommandKind, Event, StageTarget, WorktreeEntry};

/// The longest any single wait may take.
const WAIT: Duration = Duration::from_secs(10);
/// Longer than the gaps between the git commands of one burst, so a burst is
/// one burst even on a loaded machine.
const CONFIG: WatchConfig = WatchConfig {
    max_dirs: 5_000,
    debounce: Duration::from_millis(400),
    max_wait: Duration::from_secs(3),
};

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

/// A repository with `a.txt` and `sub/b.txt` on `main`, and a branch `other`.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    std::fs::write(dir.path().join("sub/b.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    git(dir.path(), &["branch", "other"]);
    dir
}

struct Session {
    engine: EngineHandle,
    watch: WatchHandle,
    notices: Receiver<Notice>,
}

fn session(config: WatchConfig) -> Session {
    let engine = gm_core::spawn_with(
        Box::new(SubprocessGit::default()),
        ApprovalStore::unavailable(),
    );
    let (watch, async_notices) =
        watch::spawn_with(engine.clone(), Arc::new(OnceLock::new()), config);
    // The front-ends await the async channel; the tests bridge it to a bounded wait.
    let (tx, notices) = mpsc::channel();
    std::thread::spawn(move || {
        while let Some(notice) = async_notices.next_blocking() {
            if tx.send(notice).is_err() {
                break;
            }
        }
    });
    Session {
        engine,
        watch,
        notices,
    }
}

fn open(session: &Session, dir: &Path) {
    session.engine.dispatch(
        Actor::Human,
        CommandKind::OpenRepo {
            path: dir.to_string_lossy().into_owned(),
        },
    );
}

/// Waits for the next notice matching `want`, and returns it.
fn wait_for(rx: &Receiver<Notice>, want: impl Fn(&Notice) -> bool) -> Notice {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(notice) if want(&notice) => return notice,
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => panic!("no matching notice within {WAIT:?}"),
            Err(RecvTimeoutError::Disconnected) => panic!("the watcher stopped"),
        }
    }
}

fn watching(rx: &Receiver<Notice>) {
    wait_for(rx, |n| matches!(n, Notice::Watching { .. }));
}

/// Collects notices until none arrives for `quiet`.
fn settle(rx: &Receiver<Notice>, quiet: Duration) -> Vec<Notice> {
    let deadline = Instant::now() + WAIT;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "the notices never went quiet");
        match rx.recv_timeout(quiet.min(left)) {
            Ok(notice) => seen.push(notice),
            Err(_) => return seen,
        }
    }
}

fn refreshes(notices: &[Notice]) -> usize {
    notices
        .iter()
        .filter(|n| matches!(n, Notice::Refreshing(_)))
        .count()
}

/// Long enough that no watcher step of a test is still pending.
const QUIET: Duration = Duration::from_millis(1500);

#[test]
fn an_external_burst_refreshes_exactly_once() {
    let dir = repo();
    let s = session(CONFIG);
    open(&s, dir.path());
    watching(&s.notices);

    // Edit, create, switch, commit: all by the git CLI, outside the app.
    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    git(dir.path(), &["checkout", "-q", "other"]);
    git(dir.path(), &["commit", "-q", "-am", "external"]);
    std::fs::write(dir.path().join("new.txt"), "x\n").unwrap();

    let seen = settle(&s.notices, QUIET);
    assert_eq!(refreshes(&seen), 1, "notices: {seen:?}");
}

#[test]
fn a_focus_request_refreshes_once() {
    let dir = repo();
    let s = session(CONFIG);
    open(&s, dir.path());
    watching(&s.notices);

    s.watch.request();

    let seen = settle(&s.notices, QUIET);
    assert_eq!(refreshes(&seen), 1, "notices: {seen:?}");
}

/// Polls the engine's events until a worktree listing satisfies `want`, and returns it.
fn worktrees_until(
    events: &async_channel::Receiver<Event>,
    want: impl Fn(&[WorktreeEntry]) -> bool,
) -> Vec<WorktreeEntry> {
    let deadline = Instant::now() + WAIT;
    let mut last = Vec::new();
    loop {
        while let Ok(event) = events.try_recv() {
            if let Event::WorktreesLoaded { worktrees, .. } = event {
                if want(&worktrees) {
                    return worktrees;
                }
                last = worktrees;
            }
        }
        assert!(
            Instant::now() < deadline,
            "no matching listing; last: {last:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_worktree_added_and_removed_from_the_cli_refreshes_the_list() {
    let dir = repo();
    let base = tempfile::tempdir().unwrap();
    let linked = base.path().join("linked wt");
    let s = session(CONFIG);
    let events = s.engine.subscribe();
    open(&s, dir.path());
    watching(&s.notices);
    worktrees_until(&events, |list| list.len() == 1);

    // The first linked worktree creates `worktrees/`, which the watcher only learns of now.
    git(
        dir.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );
    let added = settle(&s.notices, QUIET);
    assert_eq!(refreshes(&added), 1, "notices: {added:?}");
    let listed = worktrees_until(&events, |list| list.len() == 2);
    assert!(listed[1].path.ends_with("linked wt"), "{listed:?}");

    git(
        dir.path(),
        &["worktree", "remove", linked.to_str().unwrap()],
    );
    let removed = settle(&s.notices, QUIET);
    assert_eq!(refreshes(&removed), 1, "notices: {removed:?}");
    worktrees_until(&events, |list| list.len() == 1);
}

#[test]
fn ignored_churn_causes_no_refresh() {
    let dir = repo();
    std::fs::write(dir.path().join(".gitignore"), "target/\n*.log\n").unwrap();
    git(dir.path(), &["add", ".gitignore"]);
    git(dir.path(), &["commit", "-q", "-m", "ignore"]);
    let s = session(CONFIG);
    open(&s, dir.path());
    watching(&s.notices);

    let target = dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    for i in 0..300 {
        std::fs::write(target.join(format!("f{i}.o")), "x").unwrap();
    }
    let log = dir.path().join("app.log");
    for _ in 0..100 {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .unwrap()
            .write_all(b"tick\n")
            .unwrap();
    }
    let churn = settle(&s.notices, QUIET);
    assert_eq!(refreshes(&churn), 0, "ignored files refreshed: {churn:?}");

    // The same watcher still sees a visible change, so the silence above means something.
    std::fs::write(dir.path().join("visible.txt"), "x\n").unwrap();
    let after = settle(&s.notices, QUIET);
    assert_eq!(refreshes(&after), 1, "notices: {after:?}");
}

#[test]
fn an_app_write_refreshes_nothing_extra() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    let s = session(CONFIG);
    open(&s, dir.path());
    watching(&s.notices);

    s.engine.dispatch(
        Actor::Human,
        CommandKind::Stage {
            target: StageTarget::File {
                path: "a.txt".to_owned(),
            },
        },
    );

    let seen = settle(&s.notices, QUIET);
    assert_eq!(refreshes(&seen), 0, "the stage refreshed again: {seen:?}");
    let staged = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&staged.stdout).trim(), "a.txt");
}

#[test]
fn past_the_cap_it_degrades_once_and_focus_still_refreshes() {
    let dir = repo();
    let tiny = WatchConfig {
        max_dirs: 1,
        ..CONFIG
    };
    let s = session(tiny);
    open(&s, dir.path());
    wait_for(&s.notices, |n| matches!(n, Notice::Degraded { .. }));

    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    let quiet = settle(&s.notices, QUIET);
    assert_eq!(
        refreshes(&quiet),
        0,
        "degraded watcher refreshed: {quiet:?}"
    );
    assert!(
        !quiet.iter().any(|n| matches!(n, Notice::Degraded { .. })),
        "the degraded notice came twice: {quiet:?}"
    );

    s.watch.request();
    let focus = settle(&s.notices, QUIET);
    assert_eq!(refreshes(&focus), 1, "notices: {focus:?}");
}

#[test]
fn opening_another_repository_moves_the_watch() {
    let first = repo();
    let second = repo();
    let s = session(CONFIG);
    open(&s, first.path());
    watching(&s.notices);
    open(&s, second.path());
    watching(&s.notices);

    std::fs::write(first.path().join("a.txt"), "two\n").unwrap();
    let old = settle(&s.notices, QUIET);
    assert_eq!(
        refreshes(&old),
        0,
        "the old repository is still watched: {old:?}"
    );

    std::fs::write(second.path().join("a.txt"), "two\n").unwrap();
    let new = settle(&s.notices, QUIET);
    assert_eq!(refreshes(&new), 1, "notices: {new:?}");
}
