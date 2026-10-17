//! Removing a worktree (slice 1c, W5; D31) against real repositories in
//! temporary directories. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::{Engine, GitPort};
use gm_schema::{Actor, Blocker, Command, CommandKind, Event, OperationId, OperationState};

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

/// A repository with one commit on `main`, and a linked worktree on `feature`.
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

fn command(actor: Actor, kind: CommandKind) -> Command {
    Command { actor, kind }
}

/// An engine with `root` open, as a window on that folder is.
fn open(root: &Path) -> Engine {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    let events = engine.handle(
        OperationId(1),
        &command(Actor::Human, CommandKind::OpenRepo { path: text(root) }),
    );
    assert!(done(&events), "open failed: {events:?}");
    engine
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

fn inspect(engine: &mut Engine, id: u64, path: &Path) -> gm_schema::WorktreeInspection {
    let events = engine.handle(
        OperationId(id),
        &command(
            Actor::Human,
            CommandKind::InspectWorktree { path: text(path) },
        ),
    );
    let found = events.iter().find_map(|e| match e {
        Event::WorktreeInspected { inspection, .. } => Some(inspection.clone()),
        _ => None,
    });
    found.unwrap_or_else(|| panic!("no inspection: {events:?}"))
}

fn remove(engine: &mut Engine, id: u64, path: &Path, force: bool) -> Vec<Event> {
    engine.handle(
        OperationId(id),
        &command(
            Actor::Human,
            CommandKind::RemoveWorktree {
                path: text(path),
                force,
            },
        ),
    )
}

fn listed_paths(events: &[Event]) -> Option<Vec<String>> {
    events.iter().find_map(|e| match e {
        Event::WorktreesLoaded { worktrees, .. } => {
            Some(worktrees.iter().map(|w| w.path.clone()).collect())
        }
        _ => None,
    })
}

#[test]
fn a_clean_worktree_is_removed_without_force() {
    let fx = fixture();
    let mut engine = open(&fx.main);
    let inspection = inspect(&mut engine, 2, &fx.feature);
    assert!(inspection.blockers.is_empty(), "{inspection:?}");
    assert_eq!(inspection.name, "feature");

    let events = remove(&mut engine, 3, &fx.feature, false);
    assert!(done(&events), "{events:?}");
    assert!(!fx.feature.exists());
    let paths = listed_paths(&events).unwrap();
    assert!(!paths.contains(&text(&fx.feature)), "{paths:?}");
}

#[test]
fn a_fresh_worktree_from_main_is_not_blocked_by_unpushed() {
    // Its HEAD is main's commit, which another branch has: nothing is lost.
    let fx = fixture();
    let mut engine = open(&fx.main);
    assert_eq!(inspect(&mut engine, 2, &fx.feature).unpushed, 0);
}

#[test]
fn a_commit_only_in_the_worktree_is_unpushed_and_refused_without_force() {
    let fx = fixture();
    std::fs::write(fx.feature.join("b.txt"), "x\n").unwrap();
    git_in(&fx.feature, &["add", "."]);
    git_in(&fx.feature, &["commit", "-q", "-m", "only here"]);

    let mut engine = open(&fx.main);
    let inspection = inspect(&mut engine, 2, &fx.feature);
    assert_eq!(inspection.unpushed, 1);
    assert_eq!(inspection.blockers, vec![Blocker::Unpushed]);

    let events = remove(&mut engine, 3, &fx.feature, false);
    let reason = refusal(&events).expect("refused");
    assert!(reason.contains("feature cannot be removed"), "{reason}");
    assert!(fx.feature.exists());
}

#[test]
fn a_dirty_worktree_is_refused_without_force() {
    let fx = fixture();
    std::fs::write(fx.feature.join("a.txt"), "changed\n").unwrap();
    std::fs::write(fx.feature.join("new.txt"), "x\n").unwrap();

    let mut engine = open(&fx.main);
    let inspection = inspect(&mut engine, 2, &fx.feature);
    assert_eq!(inspection.blockers, vec![Blocker::Dirty]);
    assert_eq!((inspection.unstaged, inspection.untracked), (1, 1));

    let events = remove(&mut engine, 3, &fx.feature, false);
    assert!(refusal(&events).is_some(), "{events:?}");
    assert!(fx.feature.join("a.txt").exists());
    assert!(fx.feature.join("new.txt").exists());
}

#[test]
fn a_dirty_worktree_needs_the_confirmation_to_be_forced() {
    let fx = fixture();
    std::fs::write(fx.feature.join("new.txt"), "x\n").unwrap();
    let mut engine = open(&fx.main);

    // Without the confirmation the engine refuses, whatever the UI showed.
    let events = remove(&mut engine, 2, &fx.feature, false);
    assert!(refusal(&events).is_some(), "{events:?}");
    assert!(fx.feature.join("new.txt").exists());

    let events = remove(&mut engine, 3, &fx.feature, true);
    assert!(done(&events), "{events:?}");
    assert!(!fx.feature.exists());
}

#[test]
fn a_locked_worktree_is_refused_without_force_and_removed_with_it() {
    let fx = fixture();
    git_in(
        &fx.main,
        &[
            "worktree",
            "lock",
            "--reason=on the USB stick",
            "--",
            &text(&fx.feature),
        ],
    );
    let mut engine = open(&fx.main);
    let inspection = inspect(&mut engine, 2, &fx.feature);
    assert_eq!(inspection.blockers, vec![Blocker::Locked]);
    assert_eq!(inspection.locked.as_deref(), Some("on the USB stick"));

    let refused = remove(&mut engine, 3, &fx.feature, false);
    assert!(refusal(&refused).is_some());
    assert!(fx.feature.exists());

    let events = remove(&mut engine, 4, &fx.feature, true);
    assert!(done(&events), "{events:?}");
    assert!(!fx.feature.exists());
}

#[test]
fn the_main_checkout_is_refused_even_with_force_and_its_name() {
    let fx = fixture();
    let mut engine = open(&fx.main);
    let inspection = inspect(&mut engine, 2, &fx.main);
    assert!(inspection.blockers.contains(&Blocker::Main));
    assert_eq!(inspection.name, "main");

    let events = remove(&mut engine, 3, &fx.main, true);
    assert_eq!(
        refusal(&events).as_deref(),
        Some("The main checkout cannot be removed.")
    );
    assert!(fx.main.join(".git").exists());
}

#[test]
fn the_window_own_worktree_is_refused_even_with_force() {
    let fx = fixture();
    let mut engine = open(&fx.feature);
    let inspection = inspect(&mut engine, 2, &fx.feature);
    assert!(inspection.blockers.contains(&Blocker::Current));

    let events = remove(&mut engine, 3, &fx.feature, true);
    assert!(refusal(&events).unwrap().contains("Open the main checkout"));
    assert!(fx.feature.exists());
}

#[test]
fn a_missing_folder_is_inspected_as_missing_and_refused() {
    let fx = fixture();
    std::fs::remove_dir_all(&fx.feature).unwrap();
    let mut engine = open(&fx.main);
    let inspection = inspect(&mut engine, 2, &fx.feature);
    assert!(inspection.missing);
    assert_eq!(inspection.blockers, vec![Blocker::Missing]);

    let events = remove(&mut engine, 3, &fx.feature, true);
    assert!(refusal(&events).unwrap().contains("Prune"));
}

#[test]
fn inspecting_the_same_state_twice_gives_the_same_inspection() {
    let fx = fixture();
    std::fs::write(fx.feature.join("new.txt"), "x\n").unwrap();
    let mut engine = open(&fx.main);
    let first = inspect(&mut engine, 2, &fx.feature);
    let second = inspect(&mut engine, 3, &fx.feature);
    assert_eq!(first, second);
    let index = fx.main.join(".git/index");
    assert!(!index.with_extension("lock").exists());
}

#[test]
fn git_own_refusal_reaches_the_caller_unchanged() {
    // The engine lets a locked worktree through when forced, so Git's own
    // refusal is reached with force 0 at the adapter, as it is in the CLI.
    let fx = fixture();
    git_in(&fx.main, &["worktree", "lock", "--", &text(&fx.feature)]);
    let adapter = SubprocessGit::default();
    let error = adapter
        .remove_worktree(&fx.main, &text(&fx.feature), 0)
        .unwrap_err();
    let direct = Process::new("git")
        .args(["worktree", "remove", "--", &text(&fx.feature)])
        .current_dir(&fx.main)
        .output()
        .unwrap();
    let direct = String::from_utf8_lossy(&direct.stderr).trim().to_owned();
    assert_eq!(error.0, direct);
    assert!(
        error.0.contains("cannot remove a locked working tree"),
        "{error}"
    );
}

#[test]
fn an_agent_cannot_remove_a_worktree() {
    let fx = fixture();
    let mut engine = open(&fx.main);
    let events = engine.handle(
        OperationId(2),
        &command(
            Actor::Agent {
                name: "claude".into(),
            },
            CommandKind::RemoveWorktree {
                path: text(&fx.feature),
                force: true,
            },
        ),
    );
    assert_eq!(
        refusal(&events).as_deref(),
        Some("only a human can remove a worktree")
    );
    assert!(fx.feature.exists());
}

#[test]
fn a_path_that_is_not_a_worktree_is_refused_before_git_runs() {
    let fx = fixture();
    let mut engine = open(&fx.main);
    let events = remove(&mut engine, 2, &fx.main.join("nope"), false);
    assert!(
        refusal(&events)
            .unwrap()
            .contains("is not a worktree of this repository")
    );
}

#[test]
fn commits_pushed_to_the_upstream_are_not_unpushed() {
    let fx = fixture();
    let remote = fx.base.path().join("origin.git");
    git_in(
        fx.base.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git_in(
        &fx.main,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git_in(
        &fx.feature,
        &["commit", "-q", "--allow-empty", "-m", "pushed"],
    );
    git_in(&fx.feature, &["push", "-q", "-u", "origin", "feature"]);

    let mut engine = open(&fx.main);
    assert_eq!(inspect(&mut engine, 2, &fx.feature).unpushed, 0);

    // One more commit, made locally after the push: ahead of the upstream.
    git_in(
        &fx.feature,
        &["commit", "-q", "--allow-empty", "-m", "local"],
    );
    assert_eq!(inspect(&mut engine, 3, &fx.feature).unpushed, 1);
}
