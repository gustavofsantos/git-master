//! Stashes (B2, D94) against real repositories in temporary directories: the
//! list, the four writes through the adapter and through the engine, the commit
//! detail of a stash, and the command log. Each test removes its directory when
//! it ends.

use std::path::Path;
use std::process::Command as Process;
use std::sync::atomic::AtomicBool;

use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::{Engine, GitPort};
use gm_schema::{
    Actor, Command, CommandKind, CommitDetail, Event, OperationId, OperationState,
    STASH_LIST_CHANGED, StashEntry,
};

fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    Process::new("git")
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

fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_output(dir, args);
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

/// A repository on `main` with `f.txt` ("one") and `g.txt` ("one") committed.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "t"]);
    git(dir.path(), &["config", "user.email", "t@t"]);
    write(dir.path(), "f.txt", "one\n");
    write(dir.path(), "g.txt", "one\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

fn port() -> SubprocessGit {
    SubprocessGit::default()
}

fn stashes(dir: &Path) -> Vec<StashEntry> {
    port().stashes(dir).unwrap()
}

/// Changes `f.txt` to `content` and stashes it with `message`.
fn stash_change(dir: &Path, content: &str, message: Option<&str>) {
    write(dir, "f.txt", content);
    port().create_stash(dir, message, false).unwrap();
}

fn engine_on(dir: &Path, log: &CommandLog) -> Engine {
    let git = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    let events = engine.handle(
        OperationId(1),
        &human(CommandKind::OpenRepo {
            path: dir.to_string_lossy().into_owned(),
        }),
    );
    assert!(done(&events), "open failed: {events:?}");
    engine
}

fn human(kind: CommandKind) -> Command {
    Command {
        actor: Actor::Human,
        kind,
    }
}

fn state_of(events: &[Event]) -> Option<OperationState> {
    events.iter().rev().find_map(|e| match e {
        Event::OperationChanged { state, .. } => Some(*state),
        _ => None,
    })
}

fn done(events: &[Event]) -> bool {
    state_of(events) == Some(OperationState::Done)
}

/// The reason of the failed operation, if it failed.
fn refusal(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::OperationChanged {
            state: OperationState::Failed,
            detail,
            ..
        } => detail.clone(),
        _ => None,
    })
}

/// The stash list the operation answered with, if it answered one.
fn listed(events: &[Event]) -> Option<Vec<StashEntry>> {
    events.iter().find_map(|e| match e {
        Event::StashesListed { stashes, .. } => Some(stashes.clone()),
        _ => None,
    })
}

#[test]
fn the_list_is_newest_first_with_its_fields() {
    let dir = repo();
    let base = git(dir.path(), &["rev-parse", "--short", "HEAD"]);
    stash_change(dir.path(), "first\n", Some("first note"));
    stash_change(dir.path(), "second\n", None);
    stash_change(dir.path(), "third\n", Some("the newest: ünï ✓"));

    let list = stashes(dir.path());
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].message, "On main: the newest: ünï ✓");
    assert_eq!(
        list[1].message,
        format!("WIP on main: {} init", base.trim())
    );
    assert_eq!(list[2].message, "On main: first note");
    for (n, entry) in list.iter().enumerate() {
        assert_eq!(entry.index as usize, n);
        let name = format!("stash@{{{n}}}");
        assert_eq!(entry.oid, git(dir.path(), &["rev-parse", &name]).trim());
        assert_eq!(
            entry.time.to_string(),
            git(dir.path(), &["log", "-1", "--format=%ct", &name]).trim()
        );
        assert_eq!(entry.branch.as_deref(), Some("main"));
    }
}

#[test]
fn no_stash_is_an_empty_list_and_no_entry() {
    let dir = repo();
    assert!(stashes(dir.path()).is_empty());
    assert_eq!(port().resolve_stash(dir.path(), 0).unwrap(), None);
}

#[test]
fn a_create_without_a_message_takes_gits_default_subject() {
    let dir = repo();
    stash_change(dir.path(), "two\n", None);
    let list = stashes(dir.path());
    assert!(
        list[0].message.starts_with("WIP on main: ") && list[0].message.ends_with(" init"),
        "{}",
        list[0].message
    );
    assert_eq!(read(dir.path(), "f.txt"), "one\n");
}

#[test]
fn a_create_with_a_message_keeps_it_on_the_branch() {
    let dir = repo();
    stash_change(dir.path(), "two\n", Some("my note"));
    let list = stashes(dir.path());
    assert_eq!(list[0].message, "On main: my note");
    assert_eq!(list[0].branch.as_deref(), Some("main"));
}

#[test]
fn include_untracked_stores_the_new_files_and_leaves_none_on_disk() {
    let dir = repo();
    write(dir.path(), "f.txt", "two\n");
    write(dir.path(), "u.txt", "new file\n");
    port().create_stash(dir.path(), Some("all"), true).unwrap();

    assert!(!dir.path().join("u.txt").exists());
    assert_eq!(read(dir.path(), "f.txt"), "one\n");
    assert_eq!(
        git(dir.path(), &["show", "stash@{0}^3:u.txt"]),
        "new file\n"
    );
    assert!(git(dir.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn without_include_untracked_a_new_file_stays_where_it_is() {
    let dir = repo();
    write(dir.path(), "f.txt", "two\n");
    write(dir.path(), "u.txt", "new file\n");
    port().create_stash(dir.path(), None, false).unwrap();

    assert_eq!(read(dir.path(), "u.txt"), "new file\n");
    // Without `-u` the stash has no third parent for untracked files.
    let third = git_output(dir.path(), &["rev-parse", "--verify", "-q", "stash@{0}^3"]);
    assert!(!third.status.success());
}

#[test]
fn nothing_to_stash_is_refused_with_gits_text_and_nothing_is_stored() {
    let dir = repo();
    let error = port().create_stash(dir.path(), None, false).unwrap_err();
    assert_eq!(error.0, "No local changes to save");
    assert!(stashes(dir.path()).is_empty());

    // An untracked file alone is nothing to stash unless it is included.
    write(dir.path(), "u.txt", "new file\n");
    let error = port().create_stash(dir.path(), None, false).unwrap_err();
    assert_eq!(error.0, "No local changes to save");
    assert!(stashes(dir.path()).is_empty());
    assert_eq!(read(dir.path(), "u.txt"), "new file\n");
}

#[test]
fn apply_restores_the_change_and_keeps_the_stash() {
    let dir = repo();
    stash_change(dir.path(), "two\n", Some("keep me"));
    assert_eq!(read(dir.path(), "f.txt"), "one\n");

    port().apply_stash(dir.path(), 0).unwrap();

    assert_eq!(read(dir.path(), "f.txt"), "two\n");
    let list = stashes(dir.path());
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].message, "On main: keep me");
}

#[test]
fn pop_restores_the_change_and_removes_the_stash() {
    let dir = repo();
    stash_change(dir.path(), "two\n", Some("take me"));

    port().pop_stash(dir.path(), 0).unwrap();

    assert_eq!(read(dir.path(), "f.txt"), "two\n");
    assert!(stashes(dir.path()).is_empty());
}

#[test]
fn drop_removes_the_stash_and_renumbers_the_rest() {
    let dir = repo();
    stash_change(dir.path(), "a\n", Some("a"));
    stash_change(dir.path(), "b\n", Some("b"));
    stash_change(dir.path(), "c\n", Some("c"));
    let before = stashes(dir.path());
    let oid_of_a = before[2].oid.clone();

    port().drop_stash(dir.path(), 1).unwrap();

    let after = stashes(dir.path());
    let messages: Vec<&str> = after.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(messages, ["On main: c", "On main: a"]);
    assert_eq!(after[1].index, 1);
    assert_eq!(after[1].oid, oid_of_a);
    assert_eq!(read(dir.path(), "f.txt"), "one\n");
}

#[test]
fn apply_onto_a_working_tree_that_would_be_overwritten_gives_gits_error() {
    let dir = repo();
    stash_change(dir.path(), "two\n", Some("s"));
    write(dir.path(), "f.txt", "local\n");

    let error = port().apply_stash(dir.path(), 0).unwrap_err();

    assert!(
        error
            .0
            .contains("Your local changes to the following files would be overwritten by merge"),
        "{}",
        error.0
    );
    assert!(error.0.contains("f.txt"), "{}", error.0);
    assert_eq!(read(dir.path(), "f.txt"), "local\n");
    assert_eq!(stashes(dir.path()).len(), 1);
}

/// Stashes a change to `f.txt`, then commits a different change to the same
/// line, so applying the stash conflicts.
fn conflicting_stash(dir: &Path) {
    stash_change(dir, "stashed\n", Some("s"));
    write(dir, "f.txt", "committed\n");
    git(dir, &["commit", "-q", "-am", "other"]);
}

#[test]
fn apply_with_a_conflict_gives_the_conflict_lines_and_keeps_the_stash() {
    let dir = repo();
    conflicting_stash(dir.path());

    let error = port().apply_stash(dir.path(), 0).unwrap_err();

    assert!(
        error
            .0
            .contains("CONFLICT (content): Merge conflict in f.txt"),
        "{}",
        error.0
    );
    assert_eq!(stashes(dir.path()).len(), 1);
}

#[test]
fn a_pop_with_a_conflict_keeps_the_stash() {
    let dir = repo();
    conflicting_stash(dir.path());

    let error = port().pop_stash(dir.path(), 0).unwrap_err();

    assert!(
        error
            .0
            .contains("CONFLICT (content): Merge conflict in f.txt"),
        "{}",
        error.0
    );
    assert!(error.0.contains("The stash entry is kept"), "{}", error.0);
    let list = stashes(dir.path());
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].message, "On main: s");
}

#[test]
fn the_detail_of_a_stash_lists_the_stashed_files() {
    let dir = repo();
    write(dir.path(), "f.txt", "two\n");
    write(dir.path(), "g.txt", "two\n");
    port()
        .create_stash(dir.path(), Some("both"), false)
        .unwrap();
    let oid = stashes(dir.path())[0].oid.clone();

    let root = port().resolve_root(dir.path()).unwrap();
    let detail_read = port().commit_detail(&root, &oid).unwrap();
    let detail: CommitDetail = detail_read(&AtomicBool::new(false)).unwrap();

    assert_eq!(detail.oid, oid);
    let mut paths: Vec<&str> = detail.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["f.txt", "g.txt"]);
}

#[test]
fn the_writes_are_logged_and_the_list_read_is_not() {
    let dir = repo();
    let log = CommandLog::default();
    let port = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };

    write(dir.path(), "f.txt", "two\n");
    port.create_stash(dir.path(), Some("my note"), false)
        .unwrap();
    let after_create = log.entries().len();
    assert_eq!(after_create, 1);
    assert_eq!(log.entries()[0].args.join(" "), "stash push -m my note");

    port.stashes(dir.path()).unwrap();
    port.resolve_stash(dir.path(), 0).unwrap();
    assert_eq!(log.entries().len(), after_create);

    port.pop_stash(dir.path(), 0).unwrap();
    assert_eq!(log.entries().len(), after_create + 1);
    assert_eq!(
        log.entries()[after_create].args.join(" "),
        "stash pop stash@{0}"
    );
}

#[test]
fn a_stale_index_is_refused_by_the_engine_and_nothing_changes() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    stash_change(dir.path(), "a\n", Some("a"));
    stash_change(dir.path(), "b\n", Some("b"));
    write(dir.path(), "f.txt", "two\n");
    let events = engine.handle(OperationId(2), &human(CommandKind::ListStashes));
    let list = listed(&events).expect("the list answers");
    let newest = list[0].clone();
    assert_eq!(newest.message, "On main: b");

    // Another process drops the newest stash, so `stash@{0}` is now another one.
    git(dir.path(), &["stash", "drop", "-q", "stash@{0}"]);
    let before = git(dir.path(), &["stash", "list"]);
    let worktree = read(dir.path(), "f.txt");

    for (n, kind) in [
        CommandKind::ApplyStash {
            index: 0,
            oid: newest.oid.clone(),
        },
        CommandKind::PopStash {
            index: 0,
            oid: newest.oid.clone(),
        },
        CommandKind::DropStash {
            index: 0,
            oid: newest.oid.clone(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        let events = engine.handle(OperationId(10 + n as u64), &human(kind));
        assert_eq!(refusal(&events).as_deref(), Some(STASH_LIST_CHANGED));
    }
    assert_eq!(git(dir.path(), &["stash", "list"]), before);
    assert_eq!(read(dir.path(), "f.txt"), worktree);
}

#[test]
fn an_engine_write_answers_with_the_status_and_the_new_list() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    write(dir.path(), "f.txt", "two\n");

    let events = engine.handle(
        OperationId(2),
        &human(CommandKind::CreateStash {
            message: Some("   ".into()),
            include_untracked: false,
        }),
    );
    assert!(done(&events), "{events:?}");
    let list = listed(&events).expect("a write answers with the list");
    assert_eq!(list.len(), 1);
    assert!(
        list[0].message.starts_with("WIP on main: "),
        "{}",
        list[0].message
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::StatusLoaded { .. }))
    );

    let events = engine.handle(
        OperationId(3),
        &human(CommandKind::PopStash {
            index: 0,
            oid: list[0].oid.clone(),
        }),
    );
    assert!(done(&events), "{events:?}");
    assert!(listed(&events).expect("the list answers").is_empty());
    assert_eq!(read(dir.path(), "f.txt"), "two\n");
    assert_eq!(
        log.entries().len(),
        2,
        "create and pop are logged; the list reads are not"
    );
}

#[test]
fn a_refused_create_is_not_logged_as_a_write() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);

    let events = engine.handle(
        OperationId(2),
        &human(CommandKind::CreateStash {
            message: None,
            include_untracked: false,
        }),
    );

    assert_eq!(
        refusal(&events).as_deref(),
        Some("No local changes to save")
    );
    assert!(log.entries().is_empty());
}
