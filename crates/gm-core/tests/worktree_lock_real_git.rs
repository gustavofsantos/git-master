//! Locking, unlocking and pruning worktrees (slice 1c, W6; D31) against real
//! repositories in temporary directories. Each test removes its directory when
//! it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, PruneCandidate, WorktreeEntry,
};

fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
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
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repository on `main`, with a linked worktree `repo-feature` on `feature`.
struct Fixture {
    base: tempfile::TempDir,
    main: PathBuf,
    feature: PathBuf,
}

fn fixture() -> Fixture {
    let base = tempfile::tempdir().unwrap();
    let main = base.path().join("repo");
    std::fs::create_dir(&main).unwrap();
    git_in(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("a.txt"), "one\n").unwrap();
    git_in(&main, &["add", "."]);
    git_in(&main, &["commit", "-q", "-m", "init"]);
    let feature = base.path().join("repo-feature");
    git_in(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            feature.to_str().unwrap(),
        ],
    );
    Fixture {
        base,
        main,
        feature,
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn open(root: &Path) -> Engine {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    let events = send(
        &mut engine,
        1,
        Actor::Human,
        CommandKind::OpenRepo { path: text(root) },
    );
    assert!(done(&events), "open failed: {events:?}");
    engine
}

fn send(engine: &mut Engine, id: u64, actor: Actor, kind: CommandKind) -> Vec<Event> {
    engine.handle(OperationId(id), &Command { actor, kind })
}

fn done(events: &[Event]) -> bool {
    events.iter().any(|e| {
        matches!(
            e,
            Event::OperationChanged {
                state: OperationState::Done,
                ..
            }
        )
    })
}

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

/// The listed worktree at `path`, from the list the engine sent back.
fn listed_at(events: &[Event], path: &Path) -> Option<WorktreeEntry> {
    events.iter().find_map(|e| match e {
        Event::WorktreesLoaded { worktrees, .. } => {
            worktrees.iter().find(|w| w.path == text(path)).cloned()
        }
        _ => None,
    })
}

/// The lock reason of the listed worktree at `path`: `Some("")` when locked without one.
fn lock_reason(events: &[Event], path: &Path) -> Option<String> {
    listed_at(events, path).and_then(|entry| entry.locked)
}

fn candidates(events: &[Event]) -> Vec<PruneCandidate> {
    events
        .iter()
        .find_map(|e| match e {
            Event::PrunableListed { candidates, .. } => Some(candidates.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no prunable list: {events:?}"))
}

#[test]
fn lock_with_a_reason_shows_it_and_unlock_clears_it() {
    let fx = fixture();
    let mut engine = open(&fx.main);
    let events = send(
        &mut engine,
        2,
        Actor::Human,
        CommandKind::LockWorktree {
            path: text(&fx.feature),
            reason: "on the USB stick".into(),
        },
    );
    assert!(done(&events), "{events:?}");
    assert_eq!(
        lock_reason(&events, &fx.feature).as_deref(),
        Some("on the USB stick")
    );
    assert!(
        git_in(&fx.main, &["worktree", "list", "--porcelain"]).contains("locked on the USB stick")
    );

    let events = send(
        &mut engine,
        3,
        Actor::Human,
        CommandKind::UnlockWorktree {
            path: text(&fx.feature),
        },
    );
    assert!(done(&events), "{events:?}");
    assert_eq!(lock_reason(&events, &fx.feature), None);
}

#[test]
fn lock_without_a_reason_locks_with_an_empty_reason() {
    let fx = fixture();
    let mut engine = open(&fx.main);
    let events = send(
        &mut engine,
        2,
        Actor::Human,
        CommandKind::LockWorktree {
            path: text(&fx.feature),
            reason: String::new(),
        },
    );
    assert_eq!(lock_reason(&events, &fx.feature).as_deref(), Some(""));
}

#[test]
fn locking_twice_and_unlocking_an_unlocked_worktree_pass_gits_refusal_through() {
    let fx = fixture();
    let mut engine = open(&fx.main);
    let lock = CommandKind::LockWorktree {
        path: text(&fx.feature),
        reason: String::new(),
    };
    assert!(done(&send(&mut engine, 2, Actor::Human, lock.clone())));
    let again = send(&mut engine, 3, Actor::Human, lock);
    assert!(
        refusal(&again).unwrap().contains("already locked"),
        "{again:?}"
    );

    let unlock = CommandKind::UnlockWorktree {
        path: text(&fx.feature),
    };
    assert!(done(&send(&mut engine, 4, Actor::Human, unlock.clone())));
    let again = send(&mut engine, 5, Actor::Human, unlock);
    assert!(
        refusal(&again).unwrap().contains("is not locked"),
        "{again:?}"
    );
}

#[test]
fn lock_and_unlock_refuse_an_unlisted_path_and_an_agent() {
    let fx = fixture();
    let mut engine = open(&fx.main);
    let unlisted = send(
        &mut engine,
        2,
        Actor::Human,
        CommandKind::LockWorktree {
            path: text(&fx.base.path().join("nowhere")),
            reason: String::new(),
        },
    );
    assert!(
        refusal(&unlisted)
            .unwrap()
            .contains("is not a worktree of this repository")
    );

    let agent = Actor::Agent {
        name: "agent".into(),
    };
    let refused = send(
        &mut engine,
        3,
        agent.clone(),
        CommandKind::LockWorktree {
            path: text(&fx.feature),
            reason: String::new(),
        },
    );
    assert_eq!(
        refusal(&refused).as_deref(),
        Some("only a human can lock a worktree")
    );
    let refused = send(
        &mut engine,
        4,
        agent,
        CommandKind::UnlockWorktree {
            path: text(&fx.feature),
        },
    );
    assert_eq!(
        refusal(&refused).as_deref(),
        Some("only a human can unlock a worktree")
    );
}

#[test]
fn prunable_lists_a_worktree_whose_folder_is_gone_and_is_stable() {
    let fx = fixture();
    std::fs::remove_dir_all(&fx.feature).unwrap();
    let mut engine = open(&fx.main);
    let first = candidates(&send(
        &mut engine,
        2,
        Actor::Human,
        CommandKind::ListPrunable,
    ));
    let second = candidates(&send(
        &mut engine,
        3,
        Actor::Human,
        CommandKind::ListPrunable,
    ));
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0].name, "repo-feature");
    assert!(
        first[0].reason.contains("non-existent location"),
        "{first:?}"
    );
    assert_eq!(first, second);
    // The listing changes nothing: the record is still there.
    assert!(git_in(&fx.main, &["worktree", "list", "--porcelain"]).contains("prunable"));
}

#[test]
fn prunable_is_empty_while_every_folder_is_there() {
    let fx = fixture();
    let mut engine = open(&fx.main);
    assert!(
        candidates(&send(
            &mut engine,
            2,
            Actor::Human,
            CommandKind::ListPrunable
        ))
        .is_empty()
    );
}

#[test]
fn prune_removes_exactly_the_records_that_were_listed() {
    let fx = fixture();
    std::fs::remove_dir_all(&fx.feature).unwrap();
    let mut engine = open(&fx.main);
    let listed = candidates(&send(
        &mut engine,
        2,
        Actor::Human,
        CommandKind::ListPrunable,
    ));
    let names: Vec<String> = listed.iter().map(|c| c.name.clone()).collect();
    let events = send(
        &mut engine,
        3,
        Actor::Human,
        CommandKind::PruneWorktrees { names },
    );
    assert!(done(&events), "{events:?}");
    let paths: Vec<String> = events
        .iter()
        .find_map(|e| match e {
            Event::WorktreesLoaded { worktrees, .. } => {
                Some(worktrees.iter().map(|w| w.path.clone()).collect())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(paths, vec![text(&fx.main)]);
    assert!(
        candidates(&send(
            &mut engine,
            4,
            Actor::Human,
            CommandKind::ListPrunable
        ))
        .is_empty()
    );
}

#[test]
fn prune_refuses_when_the_list_changed_since_it_was_shown() {
    let fx = fixture();
    std::fs::remove_dir_all(&fx.feature).unwrap();
    let mut engine = open(&fx.main);
    let shown: Vec<String> = candidates(&send(
        &mut engine,
        2,
        Actor::Human,
        CommandKind::ListPrunable,
    ))
    .into_iter()
    .map(|c| c.name)
    .collect();

    // A second worktree goes missing after the list was shown.
    let other = fx.base.path().join("repo-other");
    git_in(
        &fx.main,
        &["worktree", "add", "-q", "--detach", other.to_str().unwrap()],
    );
    std::fs::remove_dir_all(&other).unwrap();

    let events = send(
        &mut engine,
        3,
        Actor::Human,
        CommandKind::PruneWorktrees { names: shown },
    );
    assert!(
        refusal(&events)
            .unwrap()
            .contains("changed since they were listed")
    );
    let listing = git_in(&fx.main, &["worktree", "list", "--porcelain"]);
    assert!(
        listing.contains("repo-other") && listing.contains("repo-feature"),
        "{listing}"
    );
}

#[test]
fn a_locked_missing_worktree_is_neither_listed_nor_pruned() {
    let fx = fixture();
    git_in(&fx.main, &["worktree", "lock", "--", &text(&fx.feature)]);
    std::fs::remove_dir_all(&fx.feature).unwrap();
    let mut engine = open(&fx.main);
    assert!(
        candidates(&send(
            &mut engine,
            2,
            Actor::Human,
            CommandKind::ListPrunable
        ))
        .is_empty()
    );
    let events = send(
        &mut engine,
        3,
        Actor::Human,
        CommandKind::PruneWorktrees { names: Vec::new() },
    );
    assert!(done(&events), "{events:?}");
    assert!(fx.main.join(".git/worktrees/repo-feature").exists());
}

#[test]
fn an_agent_cannot_prune() {
    let fx = fixture();
    std::fs::remove_dir_all(&fx.feature).unwrap();
    let mut engine = open(&fx.main);
    let events = send(
        &mut engine,
        2,
        Actor::Agent {
            name: "agent".into(),
        },
        CommandKind::PruneWorktrees {
            names: vec!["repo-feature".into()],
        },
    );
    assert_eq!(
        refusal(&events).as_deref(),
        Some("only a human can prune worktrees")
    );
    assert!(fx.main.join(".git/worktrees/repo-feature").exists());
}
