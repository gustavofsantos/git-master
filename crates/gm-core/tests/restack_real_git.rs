//! The restack plan through the engine (D14, D142), against real repositories in
//! temporary directories: the replay of each commit, the new tips, the refusals,
//! and that planning moves no ref. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, ReplayStep, RestackBlock,
    RestackPlan,
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

/// A repository with one commit on `main`.
fn repo(base: &Path) -> PathBuf {
    let main = base.join("repo");
    std::fs::create_dir(&main).unwrap();
    git_in(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("a.txt"), "one\n").unwrap();
    git_in(&main, &["add", "."]);
    git_in(&main, &["commit", "-q", "-m", "init"]);
    main
}

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
    log: CommandLog,
    next: u64,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = repo(dir.path());
        let log = CommandLog::default();
        let git = SubprocessGit {
            log: Some(log.clone()),
            ..SubprocessGit::default()
        };
        let mut engine = Engine::new(Box::new(git));
        let events = engine.handle(
            OperationId(1),
            &Command {
                actor: Actor::Human,
                kind: CommandKind::OpenRepo {
                    path: root.to_string_lossy().into_owned(),
                },
            },
        );
        assert_eq!(state_of(&events), Some(OperationState::Done));
        Self {
            _dir: dir,
            root,
            engine,
            log,
            next: 2,
        }
    }

    fn run_as(&mut self, actor: Actor, kind: CommandKind) -> Vec<Event> {
        self.next += 1;
        self.engine
            .handle(OperationId(self.next), &Command { actor, kind })
    }

    fn ok(&mut self, kind: CommandKind) {
        let shown = format!("{kind:?}");
        let events = self.run_as(Actor::Human, kind);
        assert_eq!(
            state_of(&events),
            Some(OperationState::Done),
            "{shown} failed: {events:?}"
        );
    }

    fn plan(&mut self, branch: &str) -> RestackPlan {
        let events = self.run_as(
            Actor::Human,
            CommandKind::PlanRestack {
                branch: branch.into(),
            },
        );
        events
            .iter()
            .find_map(|event| match event {
                Event::RestackPlanned { plan, .. } => Some(plan.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no plan: {events:?}"))
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(&self.root, args)
    }

    fn child(&mut self, name: &str, parent: &str) {
        self.ok(CommandKind::CreateChildBranch {
            name: name.into(),
            parent: parent.into(),
            checkout: false,
        });
    }

    /// Commits `file` with `content` on `branch`, leaving `main` checked out.
    fn commit_on(&self, branch: &str, file: &str, content: &str) -> String {
        self.git(&["checkout", "-q", branch]);
        std::fs::write(self.root.join(file), content).unwrap();
        self.git(&["add", file]);
        self.git(&["commit", "-q", "-m", &format!("{file}: {content}")]);
        let tip = self.git(&["rev-parse", "HEAD"]);
        self.git(&["checkout", "-q", "main"]);
        tip
    }

    fn refs(&self) -> String {
        self.git(&[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
        ])
    }
}

fn state_of(events: &[Event]) -> Option<OperationState> {
    events.iter().rev().find_map(|e| match e {
        Event::OperationChanged { state, .. } => Some(*state),
        _ => None,
    })
}

fn refusal(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::OperationChanged {
            state: OperationState::Failed,
            detail: Some(detail),
            ..
        } => Some(detail.clone()),
        _ => None,
    })
}

fn branch<'a>(plan: &'a RestackPlan, name: &str) -> &'a gm_schema::PlannedBranch {
    plan.branches
        .iter()
        .find(|branch| branch.name == name)
        .unwrap_or_else(|| panic!("{name} is not in {plan:?}"))
}

/// `main` -> `a` -> `b`, each with one commit, then `main` moves on.
fn stale_chain(repo: &mut Repo) {
    repo.child("a", "main");
    repo.commit_on("a", "a.1", "a1");
    repo.child("b", "a");
    repo.commit_on("b", "b.1", "b1");
    repo.commit_on("main", "m.1", "m1");
}

#[test]
fn a_stale_chain_is_planned_commit_by_commit_without_moving_a_ref() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    let before = repo.refs();
    let main_tip = repo.git(&["rev-parse", "main"]);
    let plan = repo.plan("main");
    assert_eq!(repo.refs(), before, "planning moved a ref");
    assert!(plan.can_apply(), "{plan:?}");
    let a = branch(&plan, "a");
    let b = branch(&plan, "b");
    assert_eq!(a.onto, main_tip);
    let a_tip = a.new_tip.clone().unwrap();
    assert_eq!(b.onto, a_tip);
    // The new commits exist, sit on the new base and keep message and author.
    assert_eq!(repo.git(&["rev-parse", &format!("{a_tip}^")]), main_tip);
    assert_eq!(repo.git(&["log", "-1", "--format=%s", &a_tip]), "a.1: a1");
    let b_tip = b.new_tip.clone().unwrap();
    assert_eq!(repo.git(&["rev-parse", &format!("{b_tip}^")]), a_tip);
    let files = repo.git(&["ls-tree", "-r", "--name-only", &b_tip]);
    for file in ["a.txt", "m.1", "a.1", "b.1"] {
        assert!(files.lines().any(|line| line == file), "{file} in {files}");
    }
}

#[test]
fn the_author_and_date_of_a_replayed_commit_are_kept() {
    let mut repo = Repo::new();
    repo.child("a", "main");
    repo.git(&["checkout", "-q", "a"]);
    std::fs::write(repo.root.join("a.1"), "x\n").unwrap();
    repo.git(&["add", "a.1"]);
    repo.git(&[
        "-c",
        "user.name=Ann",
        "-c",
        "user.email=ann@example.com",
        "commit",
        "-q",
        "-m",
        "by ann",
        "--date=2020-01-02T03:04:05+0200",
    ]);
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_on("main", "m.1", "m1");
    let plan = repo.plan("main");
    let tip = branch(&plan, "a").new_tip.clone().unwrap();
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an <%ae> %ad", "--date=raw", &tip]),
        "Ann <ann@example.com> 1577927045 +0200"
    );
}

#[test]
fn a_conflict_blocks_its_branch_and_children_but_not_siblings() {
    let mut repo = Repo::new();
    repo.child("a", "main");
    repo.child("b", "a");
    repo.child("c", "main");
    repo.commit_on("a", "a.txt", "from a\n");
    repo.commit_on("b", "b.1", "b1");
    repo.commit_on("c", "c.1", "c1");
    let conflicting = repo.commit_on("main", "a.txt", "from main\n");
    let plan = repo.plan("main");
    let a = branch(&plan, "a");
    assert!(
        matches!(a.block, Some(RestackBlock::Conflict { .. })),
        "{a:?}"
    );
    let last = a.commits.last().unwrap();
    assert_eq!(
        last.step,
        ReplayStep::Conflict {
            paths: vec!["a.txt".into()]
        }
    );
    assert!(a.new_tip.is_none());
    assert!(matches!(
        branch(&plan, "b").block,
        Some(RestackBlock::ParentBlocked { .. })
    ));
    let c = branch(&plan, "c");
    assert_eq!(c.onto, conflicting);
    assert!(c.block.is_none() && c.new_tip.is_some());
    assert!(plan.can_apply());
}

#[test]
fn a_commit_already_in_the_new_base_is_dropped() {
    let mut repo = Repo::new();
    repo.child("a", "main");
    repo.commit_on("a", "same.txt", "same\n");
    repo.commit_on("a", "a.2", "a2");
    // The parent lands the same change by itself.
    // (A different message, or the same second would make the same commit.)
    std::fs::write(repo.root.join("same.txt"), "same\n").unwrap();
    repo.git(&["add", "same.txt"]);
    repo.git(&["commit", "-q", "-m", "landed on its own"]);
    let plan = repo.plan("main");
    let a = branch(&plan, "a");
    assert!(!a.commits.is_empty(), "{plan:?}");
    assert_eq!(a.commits[0].step, ReplayStep::Dropped);
    assert_eq!(a.replayed(), 1);
}

#[test]
fn a_merge_commit_among_the_own_commits_refuses_the_branch() {
    let mut repo = Repo::new();
    repo.child("a", "main");
    repo.commit_on("a", "a.1", "a1");
    repo.git(&["checkout", "-q", "-b", "side", "main"]);
    std::fs::write(repo.root.join("s.1"), "s\n").unwrap();
    repo.git(&["add", "s.1"]);
    repo.git(&["commit", "-q", "-m", "side"]);
    repo.git(&["checkout", "-q", "a"]);
    repo.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_on("main", "m.1", "m1");
    let plan = repo.plan("main");
    assert!(
        matches!(branch(&plan, "a").block, Some(RestackBlock::Refused { .. })),
        "{plan:?}"
    );
    assert!(!plan.can_apply());
}

#[test]
fn a_dirty_working_copy_stops_the_plan_only_when_its_branch_is_in_it() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    std::fs::write(repo.root.join("a.txt"), "dirty\n").unwrap();
    // `main` is checked out and is the root, not part of the replay.
    assert!(repo.plan("main").can_apply());
    repo.git(&["stash", "-q"]);
    repo.git(&["checkout", "-q", "a"]);
    std::fs::write(repo.root.join("a.txt"), "dirty\n").unwrap();
    let plan = repo.plan("main");
    assert_eq!(plan.stopped.len(), 1, "{plan:?}");
    assert!(!plan.can_apply());
}

#[test]
fn a_branch_checked_out_in_a_linked_worktree_is_refused() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    let linked = repo.root.parent().unwrap().join("linked");
    repo.git(&["worktree", "add", "-q", linked.to_str().unwrap(), "a"]);
    let plan = repo.plan("main");
    assert!(
        matches!(branch(&plan, "a").block, Some(RestackBlock::Refused { .. })),
        "{plan:?}"
    );
    assert!(matches!(
        branch(&plan, "b").block,
        Some(RestackBlock::ParentBlocked { .. })
    ));
}

#[test]
fn current_stacks_and_unstacked_branches_have_nothing_to_plan() {
    let mut repo = Repo::new();
    repo.child("a", "main");
    repo.commit_on("a", "a.1", "a1");
    assert!(repo.plan("main").branches.is_empty());
    repo.git(&["branch", "loose"]);
    let events = repo.run_as(
        Actor::Human,
        CommandKind::PlanRestack {
            branch: "loose".into(),
        },
    );
    assert!(refusal(&events).is_some_and(|text| text.contains("no stack")));
}

#[test]
fn planning_is_a_read_and_not_in_the_command_log() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    let logged = repo.log.entries().len();
    repo.plan("main");
    assert_eq!(repo.log.entries().len(), logged);
}
