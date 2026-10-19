//! Fetch, pull and push with lease (remote slice; decided in the roadmap)
//! against real repositories in temporary directories. The remotes are local
//! bare repositories, so nothing needs the network. Each test removes its
//! directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::remote::RemoteReport;
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

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

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `origin` (a bare repository), `repo` (the one the engine works in, on `main`,
/// tracking `origin/main`) and `other` (a second clone that pushes elsewhere).
struct Fixture {
    base: tempfile::TempDir,
    origin: PathBuf,
    repo: PathBuf,
    other: PathBuf,
}

fn fixture() -> Fixture {
    let base = tempfile::tempdir().unwrap();
    let origin = base.path().join("origin.git");
    git_in(
        base.path(),
        &["init", "-q", "--bare", "-b", "main", &text(&origin)],
    );
    let repo = base.path().join("repo");
    git_in(base.path(), &["init", "-q", "-b", "main", &text(&repo)]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-q", "-m", "init"]);
    git_in(&repo, &["remote", "add", "origin", &text(&origin)]);
    git_in(&repo, &["push", "-q", "-u", "origin", "main"]);
    let other = base.path().join("other");
    git_in(
        base.path(),
        &["clone", "-q", "-b", "main", &text(&origin), &text(&other)],
    );
    Fixture {
        base,
        origin,
        repo,
        other,
    }
}

/// Writes a file and commits it in `dir`.
fn commit(dir: &Path, file: &str, message: &str) {
    std::fs::write(dir.join(file), format!("{message}\n")).unwrap();
    git_in(dir, &["add", "."]);
    git_in(dir, &["commit", "-q", "-m", message]);
}

fn head(dir: &Path) -> String {
    git_in(dir, &["rev-parse", "HEAD"])
}

/// The commit `rev` names in the bare origin.
fn origin_at(origin: &Path, rev: &str) -> String {
    git_in(origin, &["rev-parse", rev])
}

fn open(root: &Path) -> Engine {
    open_with(root, SubprocessGit::default())
}

fn open_with(root: &Path, git: SubprocessGit) -> Engine {
    let mut engine = Engine::new(Box::new(git));
    let events = send(
        &mut engine,
        1,
        Actor::Human,
        CommandKind::OpenRepo { path: text(root) },
    );
    assert!(ended(&events).0 == OperationState::Done, "open: {events:?}");
    engine
}

fn send(engine: &mut Engine, id: u64, actor: Actor, kind: CommandKind) -> Vec<Event> {
    engine.handle(OperationId(id), &Command { actor, kind })
}

/// Handles a remote command and runs its job to the end, as the runner does.
fn remote(engine: &mut Engine, id: u64, actor: Actor, kind: CommandKind) -> Vec<Event> {
    let mut events = send(engine, id, actor, kind);
    for job in engine.take_remote_jobs() {
        let report: RemoteReport = job.run();
        events.extend(engine.remote_report(report));
    }
    events
}

/// The state and detail of the operation's last event that ended it.
fn ended(events: &[Event]) -> (OperationState, Option<String>) {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::OperationChanged { state, detail, .. } if state.is_terminal() => {
                Some((*state, detail.clone()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no terminal event: {events:?}"))
}

fn succeeded(events: &[Event]) -> bool {
    ended(events).0 == OperationState::Done
}

fn failure(events: &[Event]) -> String {
    let (state, detail) = ended(events);
    assert_eq!(state, OperationState::Failed, "{events:?}");
    detail.unwrap_or_default()
}

fn agent() -> Actor {
    Actor::Agent {
        name: "agent".into(),
    }
}

#[test]
fn fetch_brings_new_remote_commits_and_prunes_a_deleted_branch() {
    let fx = fixture();
    git_in(
        &fx.other,
        &["push", "-q", "origin", "main:refs/heads/topic"],
    );
    git_in(&fx.repo, &["fetch", "-q", "origin"]);
    assert!(git_in(&fx.repo, &["branch", "-r"]).contains("origin/topic"));

    git_in(&fx.other, &["push", "-q", "origin", "--delete", "topic"]);
    commit(&fx.other, "b.txt", "two");
    git_in(&fx.other, &["push", "-q", "origin", "main"]);
    let remote_head = head(&fx.other);

    let mut engine = open(&fx.repo);
    let events = remote(&mut engine, 2, Actor::Human, CommandKind::Fetch);
    assert!(succeeded(&events), "{events:?}");
    assert_eq!(ended(&events).1.as_deref(), Some("Fetched"));
    assert_eq!(git_in(&fx.repo, &["rev-parse", "origin/main"]), remote_head);
    assert!(
        !git_in(&fx.repo, &["branch", "-r"]).contains("origin/topic"),
        "the deleted remote branch was not pruned"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::StatusLoaded { .. })),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::BranchesLoaded { .. })),
        "{events:?}"
    );
}

#[test]
fn fetch_failure_passes_gits_text_through() {
    let fx = fixture();
    git_in(
        &fx.repo,
        &["remote", "add", "broken", "/nonexistent/broken.git"],
    );
    let mut engine = open(&fx.repo);
    let events = remote(&mut engine, 2, Actor::Human, CommandKind::Fetch);
    assert!(
        failure(&events).contains("does not appear to be a git repository"),
        "{events:?}"
    );
}

#[test]
fn pull_fast_forwards_the_branch_to_its_upstream() {
    let fx = fixture();
    commit(&fx.other, "b.txt", "two");
    git_in(&fx.other, &["push", "-q", "origin", "main"]);
    let remote_head = head(&fx.other);

    let mut engine = open(&fx.repo);
    let events = remote(&mut engine, 2, Actor::Human, CommandKind::Pull);
    assert!(succeeded(&events), "{events:?}");
    assert_eq!(ended(&events).1.as_deref(), Some("Pulled"));
    assert_eq!(head(&fx.repo), remote_head);
    assert_eq!(
        git_in(&fx.repo, &["rev-list", "--count", "HEAD..origin/main"]),
        "0"
    );
}

#[test]
fn pull_refuses_a_diverged_branch_with_gits_message_and_keeps_it() {
    let fx = fixture();
    // A repository that rebases on pull by default must still only fast-forward.
    git_in(&fx.repo, &["config", "pull.rebase", "true"]);
    commit(&fx.repo, "local.txt", "local");
    commit(&fx.other, "remote.txt", "remote");
    git_in(&fx.other, &["push", "-q", "origin", "main"]);
    let before = head(&fx.repo);

    let mut engine = open(&fx.repo);
    let events = remote(&mut engine, 2, Actor::Human, CommandKind::Pull);
    assert!(
        failure(&events).contains("Not possible to fast-forward"),
        "{events:?}"
    );
    // The status is read before the failure, so the failure is the last event the
    // status bar sees and its text is not wiped by the status.
    assert!(
        matches!(
            events.last(),
            Some(Event::OperationChanged {
                state: OperationState::Failed,
                ..
            })
        ),
        "{events:?}"
    );
    assert_eq!(head(&fx.repo), before, "the local branch moved");
    assert_eq!(
        git_in(&fx.repo, &["rev-list", "--count", "origin/main..HEAD"]),
        "1"
    );
}

#[test]
fn pull_without_an_upstream_is_refused_before_git_runs() {
    let fx = fixture();
    git_in(&fx.repo, &["checkout", "-q", "-b", "local"]);
    let mut engine = open(&fx.repo);
    let events = send(&mut engine, 2, Actor::Human, CommandKind::Pull);
    assert_eq!(
        failure(&events),
        "local has no upstream, so there is nothing to pull from"
    );
    assert!(engine.take_remote_jobs().is_empty());
}

#[test]
fn push_with_lease_pushes_a_new_commit_to_the_upstream() {
    let fx = fixture();
    commit(&fx.repo, "b.txt", "two");
    let mut engine = open(&fx.repo);
    let events = remote(&mut engine, 2, Actor::Human, CommandKind::PushWithLease);
    assert!(succeeded(&events), "{events:?}");
    assert_eq!(ended(&events).1.as_deref(), Some("Pushed"));
    assert_eq!(origin_at(&fx.origin, "main"), head(&fx.repo));
}

#[test]
fn push_with_lease_after_an_amend_succeeds_when_the_remote_did_not_move() {
    let fx = fixture();
    commit(&fx.repo, "b.txt", "two");
    git_in(&fx.repo, &["push", "-q", "origin", "main"]);
    let old = head(&fx.repo);
    // Rewrites the commit that was pushed; the remote has not moved since.
    std::fs::write(fx.repo.join("b.txt"), "amended\n").unwrap();
    git_in(&fx.repo, &["add", "."]);
    git_in(&fx.repo, &["commit", "-q", "--amend", "-m", "two, amended"]);
    let amended = head(&fx.repo);
    assert_ne!(amended, old);

    let mut engine = open(&fx.repo);
    let events = remote(&mut engine, 2, Actor::Human, CommandKind::PushWithLease);
    assert!(succeeded(&events), "{events:?}");
    assert_eq!(origin_at(&fx.origin, "main"), amended);
}

#[test]
fn push_with_lease_is_refused_when_someone_pushed_meanwhile() {
    let fx = fixture();
    commit(&fx.repo, "b.txt", "two");
    git_in(&fx.repo, &["push", "-q", "origin", "main"]);
    // The engine's clone rewrites what it pushed; the other clone pushes on top
    // of the old commit without the engine fetching it, so the lease is stale.
    git_in(&fx.repo, &["reset", "-q", "--hard", "HEAD~1"]);
    commit(&fx.repo, "c.txt", "rewritten");
    let rewritten = head(&fx.repo);
    git_in(&fx.other, &["pull", "-q", "--ff-only"]);
    commit(&fx.other, "d.txt", "someone else");
    git_in(&fx.other, &["push", "-q", "origin", "main"]);
    let theirs = origin_at(&fx.origin, "main");

    let mut engine = open(&fx.repo);
    let events = remote(&mut engine, 2, Actor::Human, CommandKind::PushWithLease);
    assert!(
        failure(&events).contains("stale info"),
        "the lease was not refused: {events:?}"
    );
    assert_eq!(origin_at(&fx.origin, "main"), theirs, "the remote moved");
    assert_ne!(origin_at(&fx.origin, "main"), rewritten);
}

#[test]
fn push_without_an_upstream_and_one_remote_sets_the_upstream() {
    let fx = fixture();
    git_in(&fx.repo, &["checkout", "-q", "-b", "feature"]);
    commit(&fx.repo, "f.txt", "feature");
    let mut engine = open(&fx.repo);
    let events = remote(&mut engine, 2, Actor::Human, CommandKind::PushWithLease);
    assert!(succeeded(&events), "{events:?}");
    assert_eq!(origin_at(&fx.origin, "feature"), head(&fx.repo));
    assert_eq!(
        git_in(&fx.repo, &["config", "branch.feature.merge"]),
        "refs/heads/feature"
    );
    assert_eq!(
        git_in(
            &fx.repo,
            &["rev-parse", "--abbrev-ref", "feature@{upstream}"]
        ),
        "origin/feature"
    );
    // Only the current branch went: main on the remote is where it was.
    assert_ne!(origin_at(&fx.origin, "main"), head(&fx.repo));
}

#[test]
fn push_without_an_upstream_and_two_remotes_is_refused_and_pushes_nothing() {
    let fx = fixture();
    let fork = fx.base.path().join("fork.git");
    git_in(fx.base.path(), &["init", "-q", "--bare", &text(&fork)]);
    git_in(&fx.repo, &["remote", "add", "fork", &text(&fork)]);
    git_in(&fx.repo, &["checkout", "-q", "-b", "feature"]);
    commit(&fx.repo, "f.txt", "feature");

    let mut engine = open(&fx.repo);
    let events = send(&mut engine, 2, Actor::Human, CommandKind::PushWithLease);
    let message = failure(&events);
    assert!(message.contains("2 remotes"), "{message}");
    assert!(engine.take_remote_jobs().is_empty());
    assert!(
        !git_in(&fx.origin, &["branch", "--list", "feature"]).contains("feature"),
        "a branch was pushed"
    );
    assert!(
        git_in(&fx.base.path().join("fork.git"), &["branch", "--list"]).is_empty(),
        "the fork received a branch"
    );
}

#[test]
fn pull_and_push_from_a_detached_head_are_refused_and_fetch_still_runs() {
    let fx = fixture();
    git_in(&fx.repo, &["checkout", "-q", "--detach"]);
    let mut engine = open(&fx.repo);

    let pull = send(&mut engine, 2, Actor::Human, CommandKind::Pull);
    assert!(failure(&pull).contains("detached"), "{pull:?}");
    let push = send(&mut engine, 3, Actor::Human, CommandKind::PushWithLease);
    assert!(failure(&push).contains("detached"), "{push:?}");

    let fetch = remote(&mut engine, 4, Actor::Human, CommandKind::Fetch);
    assert!(succeeded(&fetch), "{fetch:?}");
}

#[test]
fn an_agent_is_refused_for_every_remote_command() {
    let fx = fixture();
    commit(&fx.repo, "b.txt", "two");
    let mut engine = open(&fx.repo);
    for (id, kind, what) in [
        (2, CommandKind::Fetch, "fetch"),
        (3, CommandKind::Pull, "pull"),
        (4, CommandKind::PushWithLease, "push with lease"),
    ] {
        let events = send(&mut engine, id, agent(), kind);
        assert_eq!(failure(&events), format!("only a human can {what}"));
    }
    assert!(engine.take_remote_jobs().is_empty());
    assert_ne!(origin_at(&fx.origin, "main"), head(&fx.repo));
}

#[test]
fn a_remote_command_is_refused_while_another_runs() {
    let fx = fixture();
    let mut engine = open(&fx.repo);
    let started = send(&mut engine, 2, Actor::Human, CommandKind::Fetch);
    assert!(
        started.iter().any(|e| matches!(
            e,
            Event::OperationChanged {
                state: OperationState::Executing,
                ..
            }
        )),
        "{started:?}"
    );

    let refused = send(&mut engine, 3, Actor::Human, CommandKind::Pull);
    assert_eq!(
        failure(&refused),
        "a remote operation is already running",
        "{refused:?}"
    );

    let jobs = engine.take_remote_jobs();
    assert_eq!(jobs.len(), 1);
    let mut events = Vec::new();
    for job in jobs {
        events.extend(engine.remote_report(job.run()));
    }
    assert_eq!(ended(&events).1.as_deref(), Some("Fetched"));

    // The slot is free again once the fetch has answered.
    let pull = remote(&mut engine, 4, Actor::Human, CommandKind::Pull);
    assert!(succeeded(&pull), "{pull:?}");
}

#[test]
fn the_command_log_records_the_git_runs_and_not_the_reads() {
    let fx = fixture();
    commit(&fx.repo, "b.txt", "two");
    let log = CommandLog::default();
    let git = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };
    let mut engine = open_with(&fx.repo, git);
    assert!(succeeded(&remote(
        &mut engine,
        2,
        Actor::Human,
        CommandKind::Fetch
    )));
    assert!(succeeded(&remote(
        &mut engine,
        3,
        Actor::Human,
        CommandKind::PushWithLease
    )));

    let records = log.entries();
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].args, ["fetch", "--all", "--prune"]);
    assert!(records[0].succeeded());
    assert_eq!(
        records[1].args,
        [
            "push",
            "--no-follow-tags",
            "--force-with-lease",
            "origin",
            "refs/heads/main:refs/heads/main",
        ]
    );
    assert!(records[1].succeeded(), "{:?}", records[1]);
}

#[test]
fn remote_commands_need_an_open_repository() {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    let events = send(&mut engine, 1, Actor::Human, CommandKind::Fetch);
    assert_eq!(failure(&events), "no repository is open");
}
