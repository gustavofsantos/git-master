//! The restack plan through the engine (D14, D142), against real repositories in
//! temporary directories: the replay of each commit, the new tips, the refusals,
//! and that planning moves no ref. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::restack_resolutions;
use gm_schema::{
    Actor, Command, CommandKind, ConflictFileRead, ConflictOperation, ConflictSite, Event,
    OperationId, OperationState, ReplayStep, RestackBlock, RestackPlan, RestackResolution,
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

/// An empty directory of the test's own, for the user's workflows and reviewers (R2h): the
/// developer's configuration must not change what this test pushes or restacks.
fn isolate_user_config(engine: &mut Engine, repo: &Path) {
    let user = repo.parent().unwrap().join("user-config");
    std::fs::create_dir_all(&user).unwrap();
    engine.set_reviewer_user_dir(Some(gm_core::reviewers::user_dir(&user)));
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
        isolate_user_config(&mut engine, &root);
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
                skip: Vec::new(),
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

/// A second repository with one commit, for tests that open another one.
fn another_repo(base: &Path) -> PathBuf {
    repo(base)
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
            skip: Vec::new(),
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

impl Repo {
    fn apply(&mut self, branch: &str, plan_id: &str) -> Vec<Event> {
        self.run_as(
            Actor::Human,
            CommandKind::ApplyRestack {
                branch: branch.into(),
                plan_id: plan_id.into(),
                skip: Vec::new(),
            },
        )
    }

    fn undo(&mut self) -> Vec<Event> {
        self.run_as(Actor::Human, CommandKind::UndoRestack)
    }

    fn config(&self, branch: &str) -> Option<String> {
        let output = Process::new("git")
            .args([
                "config",
                "--local",
                "--get",
                &format!("branch.{branch}.stackParentBase"),
            ])
            .current_dir(&self.root)
            .output()
            .unwrap();
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
}

#[test]
fn applying_a_plan_moves_the_branches_and_their_bases_and_leaves_the_stack_current() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    let plan = repo.plan("main");
    let events = repo.apply("main", &plan.plan_id);
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    for name in ["a", "b"] {
        let planned = branch(&plan, name);
        assert_eq!(
            repo.git(&["rev-parse", name]),
            planned.new_tip.clone().unwrap()
        );
        assert_eq!(repo.config(name), Some(planned.onto.clone()));
    }
    assert!(
        repo.plan("main").branches.is_empty(),
        "nothing is stale now"
    );
    let snapshot = events
        .iter()
        .find_map(|event| match event {
            Event::StacksLoaded { stacks, .. } => Some(stacks.clone()),
            _ => None,
        })
        .expect("stacks follow");
    assert_eq!(snapshot.branch("b").unwrap().restack, 0);
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
}

#[test]
fn a_plan_that_went_stale_is_not_applied() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    let plan = repo.plan("main");
    repo.commit_on("main", "m.2", "m2");
    let before = repo.refs();
    let events = repo.apply("main", &plan.plan_id);
    assert!(refusal(&events).is_some_and(|text| text.contains("Plan again")));
    assert_eq!(repo.refs(), before);
}

#[test]
fn the_checked_out_branch_gets_its_files_and_stays_clean() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    repo.git(&["checkout", "-q", "b"]);
    let plan = repo.plan("main");
    let events = repo.apply("main", &plan.plan_id);
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    assert!(repo.root.join("m.1").exists() && repo.root.join("b.1").exists());
}

#[test]
fn undo_puts_back_every_branch_and_base_once() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    repo.git(&["checkout", "-q", "b"]);
    let refs = repo.refs();
    let bases = (repo.config("a"), repo.config("b"));
    let plan = repo.plan("main");
    repo.apply("main", &plan.plan_id);
    assert_ne!(repo.refs(), refs);
    let events = repo.undo();
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    assert_eq!(repo.refs(), refs);
    assert_eq!((repo.config("a"), repo.config("b")), bases);
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    assert!(!repo.root.join("m.1").exists());
    assert!(refusal(&repo.undo()).is_some_and(|text| text.contains("no restack")));
}

#[test]
fn undo_is_refused_when_a_branch_moved_since() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    let plan = repo.plan("main");
    repo.apply("main", &plan.plan_id);
    repo.commit_on("a", "later", "later");
    let refs = repo.refs();
    assert!(refusal(&repo.undo()).is_some_and(|text| text.contains("moved since")));
    assert_eq!(repo.refs(), refs);
}

#[test]
fn a_blocked_branch_stays_while_its_sibling_moves() {
    let mut repo = Repo::new();
    repo.child("a", "main");
    repo.child("c", "main");
    repo.commit_on("a", "a.txt", "from a\n");
    repo.commit_on("c", "c.1", "c1");
    repo.commit_on("main", "a.txt", "from main\n");
    let a_tip = repo.git(&["rev-parse", "a"]);
    let plan = repo.plan("main");
    let events = repo.apply("main", &plan.plan_id);
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    assert_eq!(repo.git(&["rev-parse", "a"]), a_tip);
    assert_eq!(
        repo.git(&["rev-parse", "c"]),
        branch(&plan, "c").new_tip.clone().unwrap()
    );
}

#[test]
fn an_agent_cannot_apply_or_undo() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    let plan = repo.plan("main");
    let events = repo.run_as(
        Actor::Agent { name: "x".into() },
        CommandKind::ApplyRestack {
            branch: "main".into(),
            plan_id: plan.plan_id,
            skip: Vec::new(),
        },
    );
    assert!(refusal(&events).is_some_and(|text| text.contains("only a human")));
}

impl Repo {
    fn record_path(&self) -> PathBuf {
        self.root.join(".git/gitmaster/restack.json")
    }

    /// Rewrites the saved record's phase, as a kill before the last save leaves it.
    fn set_phase(&self, phase: &str) {
        let text = std::fs::read_to_string(self.record_path()).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["phase"] = phase.into();
        std::fs::write(self.record_path(), value.to_string()).unwrap();
    }

    fn resume(&mut self) -> Vec<Event> {
        self.run_as(Actor::Human, CommandKind::ResumeRestack)
    }

    fn put_base(&self, branch: &str, base: &str) {
        self.git(&[
            "config",
            "--local",
            &format!("branch.{branch}.stackParentBase"),
            base,
        ]);
    }
}

fn record_of(events: &[Event]) -> Option<gm_schema::RestackRecord> {
    events.iter().find_map(|event| match event {
        Event::RestackFinished { record, .. } => record.clone(),
        _ => None,
    })
}

#[test]
fn a_kill_before_the_bases_are_written_is_finished_on_resume() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    let plan = repo.plan("main");
    repo.apply("main", &plan.plan_id);
    let new_bases = (repo.config("a"), repo.config("b"));
    repo.put_base("a", &branch(&plan, "a").old_base.clone().unwrap());
    repo.put_base("b", &branch(&plan, "b").old_base.clone().unwrap());
    repo.set_phase("applying");
    let record = record_of(&repo.resume()).expect("the record is kept for undo");
    assert_eq!(record.phase, gm_schema::RestackPhase::Applied);
    assert_eq!((repo.config("a"), repo.config("b")), new_bases);
    assert!(repo.plan("main").branches.is_empty());
}

#[test]
fn a_kill_before_the_files_follow_is_finished_on_resume() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    repo.git(&["checkout", "-q", "b"]);
    let old_b = repo.git(&["rev-parse", "b"]);
    let plan = repo.plan("main");
    repo.apply("main", &plan.plan_id);
    // The ref moved but the index and files stayed at the old commit.
    let new_b = repo.git(&["rev-parse", "b"]);
    repo.git(&["read-tree", "-m", "-u", &new_b, &old_b]);
    assert_ne!(repo.git(&["status", "--porcelain"]), "");
    repo.set_phase("applying");
    assert!(record_of(&repo.resume()).is_some());
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    assert!(repo.root.join("m.1").exists());
}

#[test]
fn a_kill_before_any_ref_moved_drops_the_record() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    let refs = repo.refs();
    let plan = repo.plan("main");
    repo.apply("main", &plan.plan_id);
    // Put everything back by hand, then pretend the kill came first.
    for name in ["a", "b"] {
        let planned = branch(&plan, name);
        repo.git(&[
            "update-ref",
            &format!("refs/heads/{name}"),
            &planned.old_tip,
        ]);
        repo.put_base(name, &planned.old_base.clone().unwrap());
    }
    assert_eq!(repo.refs(), refs);
    repo.set_phase("applying");
    assert_eq!(record_of(&repo.resume()), None);
    assert!(!repo.record_path().exists());
}

#[test]
fn a_mix_of_old_and_new_tips_is_reported_and_left_alone() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    let plan = repo.plan("main");
    repo.apply("main", &plan.plan_id);
    repo.git(&["update-ref", "refs/heads/b", &branch(&plan, "b").old_tip]);
    repo.set_phase("applying");
    let refs = repo.refs();
    let events = repo.resume();
    assert!(
        refusal(&events).is_some_and(|text| text.contains("mix")),
        "{events:?}"
    );
    assert_eq!(repo.refs(), refs);
    assert!(repo.record_path().exists());
}

#[test]
fn an_applied_record_survives_a_new_engine_and_can_be_undone() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    let refs = repo.refs();
    let plan = repo.plan("main");
    repo.apply("main", &plan.plan_id);
    // A new engine on the same repository, as after closing and opening the app.
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    isolate_user_config(&mut engine, &repo.root);
    let human = |kind| Command {
        actor: Actor::Human,
        kind,
    };
    engine.handle(
        OperationId(1),
        &human(CommandKind::OpenRepo {
            path: repo.root.to_string_lossy().into_owned(),
        }),
    );
    let events = engine.handle(OperationId(2), &human(CommandKind::ResumeRestack));
    assert!(record_of(&events).is_some());
    let events = engine.handle(OperationId(3), &human(CommandKind::UndoRestack));
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    assert_eq!(repo.refs(), refs);
}

#[test]
fn a_skipped_branch_and_its_children_stay_while_the_rest_moves() {
    let mut repo = Repo::new();
    stale_chain(&mut repo);
    repo.child("c", "main");
    repo.commit_on("c", "c.1", "c1");
    repo.commit_on("main", "m.2", "m2");
    let (a_tip, b_tip) = (repo.git(&["rev-parse", "a"]), repo.git(&["rev-parse", "b"]));
    let skip = vec!["a".to_owned()];
    let events = repo.run_as(
        Actor::Human,
        CommandKind::PlanRestack {
            branch: "main".into(),
            skip: skip.clone(),
        },
    );
    let plan = events
        .iter()
        .find_map(|event| match event {
            Event::RestackPlanned { plan, .. } => Some(plan.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(branch(&plan, "a").block, Some(RestackBlock::Skipped));
    assert!(matches!(
        branch(&plan, "b").block,
        Some(RestackBlock::ParentBlocked { .. })
    ));
    // The plan made without the skip is another plan.
    let whole = repo.plan("main");
    assert_ne!(whole.plan_id, plan.plan_id);
    let events = repo.run_as(
        Actor::Human,
        CommandKind::ApplyRestack {
            branch: "main".into(),
            plan_id: plan.plan_id.clone(),
            skip,
        },
    );
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    assert_eq!(
        (repo.git(&["rev-parse", "a"]), repo.git(&["rev-parse", "b"])),
        (a_tip, b_tip)
    );
    assert_eq!(
        repo.git(&["rev-parse", "c"]),
        branch(&plan, "c").new_tip.clone().unwrap()
    );
    // The skipped ones are still stale: a later plan offers them again.
    assert!(repo.plan("main").branches.iter().any(|b| b.name == "a"));
}

// Restack conflict resolution (D154), through the engine: start, resolve in the resolution
// worktree, finish, plan again, apply.

/// `main` -> `a` (one commit on `a.txt`), then `main` changes `a.txt` too, so replaying `a`
/// onto the new `main` conflicts in its one commit. `b` is a child of `a` with its own file.
fn conflicting_chain(repo: &mut Repo) -> String {
    repo.child("a", "main");
    repo.commit_on("a", "a.txt", "from a\n");
    repo.child("b", "a");
    repo.commit_on("b", "b.1", "b1");
    repo.commit_on("main", "a.txt", "from main\n")
}

impl Repo {
    fn start(&mut self, plan_id: &str, conflicted: &str) -> Vec<Event> {
        self.run_as(
            Actor::Human,
            CommandKind::StartRestackResolution {
                branch: "main".into(),
                plan_id: plan_id.into(),
                skip: Vec::new(),
                conflicted: conflicted.into(),
            },
        )
    }

    fn finish(&mut self) -> Vec<Event> {
        self.run_as(Actor::Human, CommandKind::FinishRestackResolution)
    }

    fn cancel(&mut self) -> Vec<Event> {
        self.run_as(Actor::Human, CommandKind::CancelRestackResolution)
    }

    /// Reads `a.txt` of the resolution worktree, or of the checkout when `restack` is false.
    fn read(&mut self, restack: bool) -> Vec<Event> {
        let site = if restack {
            ConflictSite::Restack
        } else {
            ConflictSite::WorkingCopy
        };
        self.run_as(
            Actor::Human,
            CommandKind::ReadConflict {
                path: "a.txt".into(),
                site,
            },
        )
    }

    /// Writes `text` as the resolution of `a.txt` in the resolution worktree.
    fn resolve_restack(&mut self, text: &str) -> Vec<Event> {
        self.run_as(
            Actor::Human,
            CommandKind::ResolveConflict {
                path: "a.txt".into(),
                text: text.into(),
                origins: Vec::new(),
                site: ConflictSite::Restack,
            },
        )
    }

    fn worktree_count(&self) -> usize {
        self.git(&["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count()
    }

    fn ref_exists(&self, name: &str) -> bool {
        Process::new("git")
            .args(["rev-parse", "--verify", "--quiet", name])
            .current_dir(&self.root)
            .output()
            .unwrap()
            .status
            .success()
    }

    fn resolutions(&self) -> Vec<restack_resolutions::Resolution> {
        restack_resolutions::load(&SubprocessGit::default(), &self.root.join(".git")).unwrap()
    }
}

fn resolution_started(events: &[Event]) -> Option<RestackResolution> {
    events.iter().find_map(|e| match e {
        Event::RestackResolutionStarted { resolution, .. } => Some(resolution.clone()),
        _ => None,
    })
}

fn resolved_commit(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::RestackResolutionFinished { resolved, .. } => Some(resolved.clone()),
        _ => None,
    })
}

fn read_of(events: &[Event]) -> Option<ConflictFileRead> {
    events.iter().find_map(|e| match e {
        Event::ConflictRead { read, .. } => Some(read.clone()),
        _ => None,
    })
}

fn planned_by_finish(events: &[Event]) -> Option<RestackPlan> {
    events.iter().find_map(|e| match e {
        Event::RestackPlanned { plan, .. } => Some(plan.clone()),
        _ => None,
    })
}

#[test]
fn a_conflict_is_resolved_in_the_worktree_and_the_restack_applies_it() {
    let mut repo = Repo::new();
    let main_tip = conflicting_chain(&mut repo);
    // The checkout has a file of its own, and stays as it is.
    std::fs::write(repo.root.join("dirty.txt"), "x\n").unwrap();
    let plan = repo.plan("main");
    let conflict = branch(&plan, "a").commits.last().unwrap().oid.clone();
    assert!(
        matches!(
            branch(&plan, "a").block,
            Some(RestackBlock::Conflict { .. })
        ),
        "{plan:?}"
    );

    let events = repo.start(&plan.plan_id, "a");
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    let started = resolution_started(&events).expect("the worktree is open");
    assert_eq!(started.from, "main");
    assert_eq!(started.conflicted, "a");
    assert_eq!(started.parent, "main");
    assert_eq!(started.commit, conflict);
    assert_eq!(started.subject, "a.txt: from a");
    assert_eq!(started.onto, main_tip);
    assert_eq!(started.paths, ["a.txt"]);
    assert_eq!(repo.worktree_count(), 2);

    // The sides are named as in a rebase: the new parent, then the commit being replayed.
    let read = read_of(&repo.read(true)).expect("the restack conflict is read");
    assert_eq!(read.operation, ConflictOperation::Rebase);
    assert_eq!(read.left.branch, "main");
    assert_eq!(read.left.role, "new parent");
    assert_eq!(read.right.role, "commit being replayed");
    assert_eq!(read.right.subject, "a.txt: from a");
    assert!(read.text.contains("<<<<<<< main"), "{}", read.text);

    let events = repo.resolve_restack("resolved\n");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ConflictResolved { .. })),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::StatusLoaded { .. })),
        "a restack resolution does not reload the checkout's status: {events:?}"
    );

    let events = repo.finish();
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    let resolved = resolved_commit(&events).expect("the resolution is recorded");
    let plan = planned_by_finish(&events).expect("the restack is planned again");
    let a = branch(&plan, "a");
    assert_eq!(a.block, None, "{a:?}");
    assert_eq!(
        a.commits[0].step,
        ReplayStep::Resolved {
            new: resolved.clone()
        }
    );
    assert_eq!(a.new_tip.as_deref(), Some(resolved.as_str()));
    assert!(branch(&plan, "b").new_tip.is_some());
    assert_eq!(repo.worktree_count(), 1);
    assert_eq!(repo.resolutions().len(), 1);
    assert!(repo.ref_exists(&format!("refs/gitmaster/restack/resolved/{resolved}")));
    assert!(plan.can_apply());

    let events = repo.apply("main", &plan.plan_id);
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    assert_applied(&mut repo, &plan, &main_tip, &resolved);
}

/// What apply of the resolved restack leaves: every ref moved, the resolved text in `a`, the
/// store and its ref gone, and the checkout as it was.
fn assert_applied(repo: &mut Repo, plan: &RestackPlan, main_tip: &str, resolved: &str) {
    let a_tip = branch(plan, "a").new_tip.clone().unwrap();
    let b_tip = branch(plan, "b").new_tip.clone().unwrap();
    assert_eq!(repo.git(&["rev-parse", "a"]), a_tip);
    assert_eq!(repo.git(&["rev-parse", "b"]), b_tip);
    assert_eq!(repo.git(&["show", &format!("{a_tip}:a.txt")]), "resolved");
    assert_eq!(repo.git(&["rev-parse", &format!("{b_tip}^")]), a_tip);
    // The resolved commit keeps the author and message of the commit it resolves.
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s", &a_tip]),
        "a.txt: from a"
    );
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an <%ae>", &a_tip]),
        "t <t@t>"
    );
    // The entry is spent and its ref is gone.
    assert!(repo.resolutions().is_empty());
    assert!(!repo.ref_exists(&format!("refs/gitmaster/restack/resolved/{resolved}")));
    assert_eq!(repo.worktree_count(), 1);
    // The checkout is untouched: same branch, same file, the untracked file still there.
    assert_eq!(repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert_eq!(repo.git(&["rev-parse", "main"]), main_tip);
    assert_eq!(repo.git(&["status", "--porcelain"]), "?? dirty.txt");
    assert_eq!(repo.git(&["show", "HEAD:a.txt"]), "from main");
    // Reading without a site is still the checkout's read, which is not conflicted.
    let refused = repo.read(false);
    assert!(refusal(&refused).is_some_and(|text| text.contains("is not conflicted")));
    assert_eq!(state_of(&refused), Some(OperationState::Failed));
    // The engine logged the commands that change state, and not the reads.
    let logged: Vec<_> = repo.engine.log().iter().map(|e| e.kind.clone()).collect();
    assert!(
        logged
            .iter()
            .any(|k| matches!(k, CommandKind::StartRestackResolution { .. }))
    );
    assert!(
        logged
            .iter()
            .any(|k| matches!(k, CommandKind::FinishRestackResolution))
    );
    assert!(
        !logged
            .iter()
            .any(|k| matches!(k, CommandKind::ReadConflict { .. }))
    );
}

#[test]
fn a_moved_parent_ignores_a_stale_resolution_and_conflicts_again() {
    let mut repo = Repo::new();
    conflicting_chain(&mut repo);
    let plan = repo.plan("main");
    let events = repo.start(&plan.plan_id, "a");
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    repo.resolve_restack("resolved\n");
    let resolved = resolved_commit(&repo.finish()).expect("resolved");
    assert_eq!(repo.resolutions().len(), 1);

    // `main` moves on: the resolution was made onto the old `main`, so it does not apply.
    repo.commit_on("main", "m.2", "m2");
    let plan = repo.plan("main");
    let a = branch(&plan, "a");
    assert!(
        matches!(a.block, Some(RestackBlock::Conflict { .. })),
        "{a:?}"
    );
    assert_ne!(a.commits[0].step, ReplayStep::Resolved { new: resolved });
    assert!(a.new_tip.is_none());
}

#[test]
fn a_second_conflict_in_the_same_branch_is_resolved_the_same_way() {
    let mut repo = Repo::new();
    repo.child("a", "main");
    repo.commit_on("a", "a.txt", "from a1\n");
    repo.commit_on("a", "a.txt", "from a2\n");
    repo.commit_on("main", "a.txt", "from main\n");

    let plan = repo.plan("main");
    let events = repo.start(&plan.plan_id, "a");
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    repo.resolve_restack("first\n");
    let first = resolved_commit(&repo.finish()).expect("the first is resolved");

    // The first commit now replays clean from the resolution; the second conflicts on it.
    let plan = repo.plan("main");
    let a = branch(&plan, "a");
    assert_eq!(a.commits[0].step, ReplayStep::Resolved { new: first });
    let second = a.commits[1].oid.clone();
    assert_eq!(
        a.block,
        Some(RestackBlock::Conflict {
            commit: second.clone(),
            onto: match &a.commits[0].step {
                ReplayStep::Resolved { new } => new.clone(),
                other => panic!("{other:?}"),
            },
        })
    );

    let events = repo.start(&plan.plan_id, "a");
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    let started = resolution_started(&events).expect("the second conflict is open");
    assert_eq!(started.commit, second);
    assert_eq!(started.paths, ["a.txt"]);
    repo.resolve_restack("final\n");
    let second_resolved = resolved_commit(&repo.finish()).expect("the second is resolved");

    let plan = repo.plan("main");
    let a = branch(&plan, "a");
    assert_eq!(a.block, None, "{a:?}");
    assert_eq!(a.replayed(), 2);
    assert_eq!(a.new_tip.as_deref(), Some(second_resolved.as_str()));
    assert_eq!(repo.resolutions().len(), 2);
    assert!(plan.can_apply());
    let events = repo.apply("main", &plan.plan_id);
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    assert_eq!(repo.git(&["show", "a:a.txt"]), "final");
    assert!(repo.resolutions().is_empty());
    assert_eq!(repo.worktree_count(), 1);
}

#[test]
fn cancelling_leaves_no_worktree_and_the_plan_unchanged() {
    let mut repo = Repo::new();
    conflicting_chain(&mut repo);
    let before = repo.plan("main");
    let refs = repo.refs();

    let events = repo.start(&before.plan_id, "a");
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    assert_eq!(repo.worktree_count(), 2);

    let events = repo.cancel();
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::RestackResolutionCancelled { .. }))
    );
    assert_eq!(repo.worktree_count(), 1);
    assert_eq!(repo.refs(), refs);
    assert!(repo.resolutions().is_empty());
    let after = repo.plan("main");
    assert_eq!(after.plan_id, before.plan_id);

    // Nothing is open any more: reading, resolving and finishing are refused.
    let read = repo.read(true);
    assert!(
        refusal(&read).is_some_and(|text| text.contains("No restack conflict is being resolved."))
    );
    let finished = repo.finish();
    assert!(
        refusal(&finished)
            .is_some_and(|text| text.contains("No restack conflict is being resolved."))
    );
    // Cancelling when nothing is open succeeds.
    assert_eq!(state_of(&repo.cancel()), Some(OperationState::Done));
}

#[test]
fn an_agent_cannot_start_finish_or_cancel_a_resolution() {
    let mut repo = Repo::new();
    conflicting_chain(&mut repo);
    let plan = repo.plan("main");
    let agent = Actor::Agent { name: "x".into() };
    let events = repo.run_as(
        agent.clone(),
        CommandKind::StartRestackResolution {
            branch: "main".into(),
            plan_id: plan.plan_id.clone(),
            skip: Vec::new(),
            conflicted: "a".into(),
        },
    );
    assert!(refusal(&events).is_some_and(|text| text.contains("only a human")));
    assert_eq!(repo.worktree_count(), 1);

    let events = repo.start(&plan.plan_id, "a");
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    let events = repo.run_as(agent.clone(), CommandKind::FinishRestackResolution);
    assert!(refusal(&events).is_some_and(|text| text.contains("only a human")));
    let events = repo.run_as(agent, CommandKind::CancelRestackResolution);
    assert!(refusal(&events).is_some_and(|text| text.contains("only a human")));
    assert_eq!(repo.worktree_count(), 2);
    repo.cancel();
}

#[test]
fn a_start_with_a_plan_that_changed_is_refused() {
    let mut repo = Repo::new();
    conflicting_chain(&mut repo);
    let events = repo.start("not-the-plan", "a");
    assert_eq!(
        state_of(&events),
        Some(OperationState::Failed),
        "{events:?}"
    );
    assert!(
        refusal(&events)
            .is_some_and(|text| text.contains("The stack changed since the plan; plan again."))
    );
    assert_eq!(repo.worktree_count(), 1);
}

#[test]
fn opening_another_repository_drops_the_resolution_of_the_first() {
    let mut repo = Repo::new();
    conflicting_chain(&mut repo);
    let plan = repo.plan("main");
    let events = repo.start(&plan.plan_id, "a");
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");

    let other = tempfile::tempdir().unwrap();
    let other_root = another_repo(other.path());
    let events = repo.run_as(
        Actor::Human,
        CommandKind::OpenRepo {
            path: other_root.to_string_lossy().into_owned(),
        },
    );
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    // The second repository has no conflict, and the first one's session is not reached.
    let read = repo.read(true);
    assert!(
        refusal(&read).is_some_and(|text| text.contains("No restack conflict is being resolved."))
    );
    let finished = repo.finish();
    assert!(
        refusal(&finished)
            .is_some_and(|text| text.contains("No restack conflict is being resolved."))
    );
}

#[test]
fn a_start_on_a_branch_without_a_conflict_is_refused() {
    let mut repo = Repo::new();
    conflicting_chain(&mut repo);
    let plan = repo.plan("main");
    let events = repo.start(&plan.plan_id, "b");
    assert!(refusal(&events).is_some_and(|text| text.contains("b has no conflict in the plan.")));
    assert_eq!(repo.worktree_count(), 1);
}
