//! Submodules (D175) against real repositories in temporary directories. The
//! submodule is a linked worktree of another clone, as a workspace member is
//! (D174): its row's flags after a commit, an edit and a new file; the commits it
//! gained, newest first; a detached HEAD; a submodule behind the commit the
//! repository records; a folder that is not checked out; a path the read refuses;
//! and the command log. Each test removes its directories when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::{Engine, GitPort};
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, StatusEntry, SubmoduleChange,
    SubmoduleDetail,
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

fn oid_of(dir: &Path, rev: &str) -> String {
    git(dir, &["rev-parse", rev]).trim().to_owned()
}

/// Commits a new file `name` with `message`, and returns the new id.
fn commit(dir: &Path, name: &str, message: &str) -> String {
    write(dir, name, &format!("{message}\n"));
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", message]);
    oid_of(dir, "HEAD")
}

fn port() -> SubprocessGit {
    SubprocessGit::default()
}

/// A clone `repo` with `f.txt` committed as `init` and a branch `feat`, and a
/// superproject `super` whose submodule `sub` is a linked worktree of `repo` on
/// `feat`, added the way a workspace adds its members.
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let superproject = tmp.path().join("super");
    std::fs::create_dir(&repo).unwrap();
    std::fs::create_dir(&superproject).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    commit(&repo, "f.txt", "init");
    git(&repo, &["branch", "feat"]);
    git(&superproject, &["init", "-q", "-b", "main"]);
    commit(&superproject, "README", "super");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            superproject.join("sub").to_str().unwrap(),
            "feat",
        ],
    );
    git(
        &superproject,
        &[
            "submodule",
            "add",
            "-q",
            "-b",
            "feat",
            repo.to_str().unwrap(),
            "sub",
        ],
    );
    git(&superproject, &["commit", "-q", "-m", "add sub"]);
    (tmp, repo, superproject)
}

fn sub(superproject: &Path) -> PathBuf {
    superproject.join("sub")
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

fn done(events: &[Event]) -> bool {
    events.iter().rev().any(|e| {
        matches!(
            e,
            Event::OperationChanged {
                state: OperationState::Done,
                ..
            }
        )
    })
}

/// The reason of the failed operation, if it failed.
fn refusal(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::OperationChanged {
            state: OperationState::Failed,
            detail,
            ..
        } => Some(detail.clone().unwrap_or_default()),
        _ => None,
    })
}

fn detail_in(events: &[Event]) -> Option<SubmoduleDetail> {
    events.iter().find_map(|e| match e {
        Event::SubmoduleLoaded { submodule, .. } => Some(submodule.clone()),
        _ => None,
    })
}

fn load(engine: &mut Engine, id: u64, path: &str) -> SubmoduleDetail {
    let events = engine.handle(
        OperationId(id),
        &human(CommandKind::LoadSubmodule {
            path: path.to_owned(),
        }),
    );
    assert!(done(&events), "load failed: {events:?}");
    detail_in(&events).expect("a submodule detail")
}

/// The status row of `path` in the superproject.
fn row(superproject: &Path, path: &str) -> StatusEntry {
    port()
        .status(superproject)
        .unwrap()
        .entries
        .into_iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("no status row for {path}"))
}

#[test]
fn a_commit_inside_the_submodule_marks_new_commits_and_lists_them() {
    let (_tmp, _repo, superproject) = fixture();
    let second = commit(&sub(&superproject), "a.txt", "second");

    assert_eq!(
        row(&superproject, "sub").submodule,
        Some(SubmoduleChange {
            commit_changed: true,
            modified: false,
            untracked: false,
        })
    );

    let log = CommandLog::default();
    let mut engine = engine_on(&superproject, &log);
    let detail = load(&mut engine, 2, "sub");
    assert_eq!(detail.path, "sub");
    assert_eq!(detail.branch.as_deref(), Some("feat"));
    assert!(!detail.detached);
    assert!(detail.checked_out);
    assert_eq!(detail.head.as_deref(), Some(second.as_str()));
    assert_eq!(
        detail.recorded.as_deref(),
        Some(git(&superproject, &["rev-parse", "HEAD:sub"]).trim())
    );
    assert_eq!(detail.commits.len(), 1);
    assert_eq!(detail.commits[0].oid, second);
    assert_eq!(detail.commits[0].subject, "second");
    assert_eq!(detail.commits[0].author_name, "t");
    assert!(detail.commits[0].author_time > 0);
    assert!(!detail.more);
    assert_eq!(detail.missing_commits, 0);
    assert!(log.entries().is_empty(), "a read is not logged");
}

#[test]
fn the_commits_the_submodule_gained_are_listed_newest_first() {
    let (_tmp, _repo, superproject) = fixture();
    commit(&sub(&superproject), "a.txt", "second");
    let third = commit(&sub(&superproject), "b.txt", "third");

    let mut engine = engine_on(&superproject, &CommandLog::default());
    let detail = load(&mut engine, 2, "sub");
    let subjects: Vec<&str> = detail.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["third", "second"]);
    assert_eq!(detail.commits[0].oid, third);
}

#[test]
fn an_edited_tracked_file_marks_modified() {
    let (_tmp, _repo, superproject) = fixture();
    write(&sub(&superproject), "f.txt", "changed\n");

    assert_eq!(
        row(&superproject, "sub").submodule,
        Some(SubmoduleChange {
            commit_changed: false,
            modified: true,
            untracked: false,
        })
    );
}

#[test]
fn a_new_file_marks_untracked() {
    let (_tmp, _repo, superproject) = fixture();
    write(&sub(&superproject), "new.txt", "new\n");

    assert_eq!(
        row(&superproject, "sub").submodule,
        Some(SubmoduleChange {
            commit_changed: false,
            modified: false,
            untracked: true,
        })
    );
}

#[test]
fn a_detached_head_has_no_branch() {
    let (_tmp, _repo, superproject) = fixture();
    let head = oid_of(&sub(&superproject), "HEAD");
    git(&sub(&superproject), &["checkout", "-q", "--detach"]);

    let mut engine = engine_on(&superproject, &CommandLog::default());
    let detail = load(&mut engine, 2, "sub");
    assert!(detail.detached);
    assert_eq!(detail.branch, None);
    assert_eq!(detail.head.as_deref(), Some(head.as_str()));
    assert!(detail.commits.is_empty());
}

#[test]
fn a_submodule_behind_the_recorded_commit_counts_what_it_lacks() {
    let (_tmp, _repo, superproject) = fixture();
    let init = oid_of(&sub(&superproject), "HEAD");
    let second = commit(&sub(&superproject), "a.txt", "second");
    git(&superproject, &["add", "sub"]);
    git(&superproject, &["commit", "-q", "-m", "record second"]);
    git(&sub(&superproject), &["reset", "-q", "--hard", &init]);

    let mut engine = engine_on(&superproject, &CommandLog::default());
    let detail = load(&mut engine, 2, "sub");
    assert_eq!(detail.head.as_deref(), Some(init.as_str()));
    assert_eq!(
        detail.recorded.as_deref(),
        Some(second.as_str()),
        "the repository records the second commit"
    );
    assert_eq!(detail.missing_commits, 1);
    assert!(detail.commits.is_empty());
    assert!(!detail.more);
}

#[test]
fn a_folder_that_is_not_checked_out_says_so() {
    let (_tmp, _repo, superproject) = fixture();
    // What a clone without `--recurse-submodules` leaves: the empty folder.
    std::fs::remove_dir_all(sub(&superproject)).unwrap();
    std::fs::create_dir(sub(&superproject)).unwrap();

    let mut engine = engine_on(&superproject, &CommandLog::default());
    let detail = load(&mut engine, 2, "sub");
    assert!(!detail.checked_out);
    assert_eq!(detail.head, None);
    assert_eq!(detail.branch, None);
    assert!(!detail.detached);
    assert!(detail.recorded.is_some());
    assert!(detail.commits.is_empty());
    assert_eq!(detail.missing_commits, 0);
}

#[test]
fn a_path_the_index_does_not_hold_as_a_submodule_is_refused() {
    let (_tmp, _repo, superproject) = fixture();
    let mut engine = engine_on(&superproject, &CommandLog::default());
    for (id, path) in [
        (2, "/etc"),
        (3, "../repo"),
        (4, "sub/../sub"),
        (5, ""),
        (6, "README"),
        (7, "missing"),
    ] {
        let events = engine.handle(
            OperationId(id),
            &human(CommandKind::LoadSubmodule {
                path: path.to_owned(),
            }),
        );
        assert!(refusal(&events).is_some(), "{path:?} should be refused");
        assert!(detail_in(&events).is_none(), "{path:?} answered");
    }
}
