//! Branch stacks through the engine (D13, D68, D138), against real repositories
//! in temporary directories: the links in `.git/config`, the derived states and
//! the metadata writes. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, StackBranch, StackSnapshot,
    StackState,
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

/// The value of a config key, `None` when unset.
fn config(dir: &Path, key: &str) -> Option<String> {
    let output = Process::new("git")
        .args(["config", "--local", "--get", key])
        .current_dir(dir)
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
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

/// Commits a new file `name` on the branch checked out in `dir`.
fn commit_file(dir: &Path, name: &str) -> String {
    std::fs::write(dir.join(name), format!("{name}\n")).unwrap();
    git_in(dir, &["add", name]);
    git_in(dir, &["commit", "-q", "-m", name]);
    git_in(dir, &["rev-parse", "HEAD"])
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

    /// Runs `kind` as a human and returns its stack snapshot; panics when it fails.
    fn ok(&mut self, kind: CommandKind) -> StackSnapshot {
        let shown = format!("{kind:?}");
        let events = self.run_as(Actor::Human, kind);
        assert_eq!(
            state_of(&events),
            Some(OperationState::Done),
            "{shown} failed: {events:?}"
        );
        stacks(&events).expect("a stack command answers with StacksLoaded")
    }

    /// Runs `kind` as a human and returns why it was refused.
    fn refused(&mut self, kind: CommandKind) -> String {
        let shown = format!("{kind:?}");
        let events = self.run_as(Actor::Human, kind);
        refusal(&events).unwrap_or_else(|| panic!("{shown} should fail: {events:?}"))
    }

    fn load(&mut self) -> StackSnapshot {
        self.ok(CommandKind::LoadStacks)
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(&self.root, args)
    }

    fn child(&mut self, name: &str, parent: &str) -> StackSnapshot {
        self.ok(CommandKind::CreateChildBranch {
            name: name.into(),
            parent: parent.into(),
            checkout: false,
        })
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
            detail,
            ..
        } => detail.clone(),
        _ => None,
    })
}

fn stacks(events: &[Event]) -> Option<StackSnapshot> {
    events.iter().find_map(|e| match e {
        Event::StacksLoaded { stacks, .. } => Some(stacks.clone()),
        _ => None,
    })
}

fn entry<'a>(snapshot: &'a StackSnapshot, name: &str) -> &'a StackBranch {
    snapshot
        .branch(name)
        .unwrap_or_else(|| panic!("{name} is not in a stack: {snapshot:?}"))
}

fn state(snapshot: &StackSnapshot, name: &str) -> StackState {
    entry(snapshot, name).state
}

/// The tree as indented names, for comparing shapes.
fn shape(snapshot: &StackSnapshot) -> Vec<String> {
    snapshot
        .stacks
        .iter()
        .flat_map(|stack| &stack.branches)
        .map(|branch| format!("{}{}", "  ".repeat(branch.depth as usize), branch.name))
        .collect()
}

#[test]
fn a_repository_without_links_has_no_stacks() {
    let mut repo = Repo::new();
    repo.git(&["branch", "topic"]);
    assert_eq!(repo.load(), StackSnapshot::default());
}

#[test]
fn a_new_child_starts_at_the_parent_tip_and_records_it() {
    let mut repo = Repo::new();
    let main_tip = repo.git(&["rev-parse", "main"]);
    let snapshot = repo.child("ledger-base", "main");
    assert_eq!(repo.git(&["rev-parse", "ledger-base"]), main_tip);
    assert_eq!(
        config(&repo.root, "branch.ledger-base.stackParent").as_deref(),
        Some("main")
    );
    assert_eq!(
        config(&repo.root, "branch.ledger-base.stackParentBase"),
        Some(main_tip)
    );
    // A child of the tip shows no upstream tracking: it starts at a commit id.
    assert_eq!(config(&repo.root, "branch.ledger-base.merge"), None);
    assert_eq!(shape(&snapshot), ["main", "  ledger-base"]);
    assert_eq!(state(&snapshot, "main"), StackState::Root);
    assert_eq!(state(&snapshot, "ledger-base"), StackState::Current);
    // The writes are in the command log, with git's own arguments.
    let lines: Vec<String> = repo
        .log
        .entries()
        .iter()
        .map(gm_core::command_log::CommandRecord::line)
        .collect();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("config --local branch.ledger-base.stackParent main")),
        "{lines:?}"
    );
}

#[test]
fn loading_the_stacks_writes_nothing_and_logs_nothing() {
    let mut repo = Repo::new();
    repo.child("topic", "main");
    let before = repo.log.entries().len();
    let config_before = std::fs::read(repo.root.join(".git/config")).unwrap();
    repo.load();
    assert_eq!(repo.log.entries().len(), before);
    assert_eq!(
        std::fs::read(repo.root.join(".git/config")).unwrap(),
        config_before
    );
}

#[test]
fn a_parent_with_new_commits_makes_its_children_stale() {
    let mut repo = Repo::new();
    repo.child("ledger-base", "main");
    repo.git(&["checkout", "-q", "ledger-base"]);
    commit_file(&repo.root, "base.txt");
    repo.child("ledger-api", "ledger-base");
    repo.child("ledger-ui", "ledger-base");
    repo.git(&["checkout", "-q", "ledger-ui"]);
    commit_file(&repo.root, "ui.txt");
    commit_file(&repo.root, "ui2.txt");
    repo.child("ledger-e2e", "ledger-ui");

    let snapshot = repo.load();
    assert_eq!(
        shape(&snapshot),
        [
            "main",
            "  ledger-base",
            "    ledger-api",
            "    ledger-ui",
            "      ledger-e2e"
        ]
    );
    assert_eq!(entry(&snapshot, "ledger-ui").ahead, 2);
    assert_eq!(entry(&snapshot, "ledger-base").ahead, 1);
    assert_eq!(entry(&snapshot, "ledger-base").restack, 0);

    repo.git(&["checkout", "-q", "ledger-base"]);
    commit_file(&repo.root, "base2.txt");
    let snapshot = repo.load();
    assert_eq!(state(&snapshot, "ledger-api"), StackState::Stale);
    assert_eq!(state(&snapshot, "ledger-ui"), StackState::Stale);
    assert_eq!(state(&snapshot, "ledger-e2e"), StackState::Current);
    assert_eq!(entry(&snapshot, "ledger-ui").behind, 1);
    // ledger-api, ledger-ui and ledger-e2e under it would be replayed.
    assert_eq!(entry(&snapshot, "ledger-base").restack, 3);
    assert_eq!(state(&snapshot, "ledger-base"), StackState::Current);
}

#[test]
fn a_rebase_made_outside_shows_a_diverged_base() {
    let mut repo = Repo::new();
    repo.child("base", "main");
    repo.git(&["checkout", "-q", "base"]);
    commit_file(&repo.root, "b.txt");
    repo.child("top", "base");
    repo.git(&["checkout", "-q", "top"]);
    commit_file(&repo.root, "t.txt");
    repo.git(&["checkout", "-q", "base"]);
    commit_file(&repo.root, "b2.txt");
    assert_eq!(state(&repo.load(), "top"), StackState::Stale);

    // The child already holds the parent's new commit, so the base is outdated.
    repo.git(&["checkout", "-q", "top"]);
    repo.git(&["rebase", "-q", "base"]);
    assert_eq!(state(&repo.load(), "top"), StackState::DivergedBase);
}

#[test]
fn an_amended_parent_is_stale_not_diverged() {
    let mut repo = Repo::new();
    repo.child("base", "main");
    repo.git(&["checkout", "-q", "base"]);
    commit_file(&repo.root, "b.txt");
    repo.child("top", "base");
    repo.git(&["checkout", "-q", "base"]);
    repo.git(&["commit", "-q", "--amend", "-m", "b, reworded"]);
    let snapshot = repo.load();
    assert_eq!(state(&snapshot, "top"), StackState::Stale);
    assert_eq!(entry(&snapshot, "base").restack, 1);
}

/// main <- base (two commits) <- top (one commit), checked out on main.
fn merged_setup() -> Repo {
    let mut repo = Repo::new();
    repo.child("base", "main");
    repo.git(&["checkout", "-q", "base"]);
    commit_file(&repo.root, "b1.txt");
    commit_file(&repo.root, "b2.txt");
    repo.child("top", "base");
    repo.git(&["checkout", "-q", "top"]);
    commit_file(&repo.root, "t.txt");
    repo.git(&["checkout", "-q", "main"]);
    commit_file(&repo.root, "elsewhere.txt");
    repo
}

#[test]
fn a_squash_merged_parent_is_found_and_the_child_moves_to_the_grandparent() {
    let mut repo = merged_setup();
    assert_eq!(state(&repo.load(), "top"), StackState::Current);
    let base_before = config(&repo.root, "branch.top.stackParentBase");

    repo.git(&["merge", "-q", "--squash", "base"]);
    repo.git(&["commit", "-q", "-m", "squash base"]);
    let snapshot = repo.load();
    assert_eq!(state(&snapshot, "top"), StackState::ParentMerged);

    let snapshot = repo.ok(CommandKind::ReparentToGrandparent {
        branch: "top".into(),
    });
    assert_eq!(
        config(&repo.root, "branch.top.stackParent").as_deref(),
        Some("main")
    );
    assert_eq!(
        config(&repo.root, "branch.top.stackParentBase"),
        base_before
    );
    assert_eq!(entry(&snapshot, "top").parent.as_deref(), Some("main"));
    assert_eq!(state(&snapshot, "top"), StackState::Stale);
}

#[test]
fn a_rebase_merged_and_a_merged_parent_are_found() {
    let mut repo = merged_setup();
    repo.git(&["cherry-pick", "base~1", "base"]);
    assert_eq!(state(&repo.load(), "top"), StackState::ParentMerged);

    let mut repo = merged_setup();
    repo.git(&["merge", "-q", "--no-ff", "-m", "merge base", "base"]);
    assert_eq!(state(&repo.load(), "top"), StackState::ParentMerged);
}

#[test]
fn an_unmerged_parent_and_an_empty_one_are_not_merged() {
    let mut repo = merged_setup();
    // Only one of the two commits of base reached main.
    repo.git(&["cherry-pick", "base~1"]);
    assert_eq!(state(&repo.load(), "top"), StackState::Current);

    let mut repo = Repo::new();
    repo.child("empty", "main");
    repo.child("top", "empty");
    assert_eq!(state(&repo.load(), "top"), StackState::Current);
}

#[test]
fn a_deleted_parent_is_gone_and_reparents_to_the_default_branch() {
    let mut repo = merged_setup();
    repo.git(&["branch", "-D", "base"]);
    // Git removed the values of base itself, so its own parent is unknown.
    assert_eq!(config(&repo.root, "branch.base.stackParent"), None);
    let snapshot = repo.load();
    assert_eq!(shape(&snapshot), ["top"]);
    assert_eq!(state(&snapshot, "top"), StackState::ParentGone);

    let snapshot = repo.ok(CommandKind::ReparentToGrandparent {
        branch: "top".into(),
    });
    assert_eq!(shape(&snapshot), ["main", "  top"]);
}

#[test]
fn reparent_is_refused_while_the_parent_is_there() {
    let mut repo = merged_setup();
    let reason = repo.refused(CommandKind::ReparentToGrandparent {
        branch: "top".into(),
    });
    assert!(reason.contains("neither gone nor merged"), "{reason}");
}

#[test]
fn set_parent_records_the_merge_base_and_clear_removes_both_keys() {
    let mut repo = Repo::new();
    let fork = repo.git(&["rev-parse", "main"]);
    repo.git(&["branch", "lone"]);
    commit_file(&repo.root, "later.txt");
    let snapshot = repo.ok(CommandKind::SetStackParent {
        branch: "lone".into(),
        parent: "main".into(),
    });
    assert_eq!(
        config(&repo.root, "branch.lone.stackParentBase"),
        Some(fork)
    );
    assert_eq!(state(&snapshot, "lone"), StackState::Stale);

    let snapshot = repo.ok(CommandKind::ClearStackParent {
        branch: "lone".into(),
    });
    assert_eq!(snapshot, StackSnapshot::default());
    assert_eq!(config(&repo.root, "branch.lone.stackParent"), None);
    assert_eq!(config(&repo.root, "branch.lone.stackParentBase"), None);
    let reason = repo.refused(CommandKind::ClearStackParent {
        branch: "lone".into(),
    });
    assert!(reason.contains("no stack parent"), "{reason}");
}

#[test]
fn set_parent_refuses_loops_strangers_and_missing_branches() {
    let mut repo = Repo::new();
    repo.child("a", "main");
    repo.child("b", "a");
    let reason = repo.refused(CommandKind::SetStackParent {
        branch: "a".into(),
        parent: "b".into(),
    });
    assert!(reason.contains("under a"), "{reason}");
    let reason = repo.refused(CommandKind::SetStackParent {
        branch: "a".into(),
        parent: "a".into(),
    });
    assert!(reason.contains("own parent"), "{reason}");
    let reason = repo.refused(CommandKind::SetStackParent {
        branch: "a".into(),
        parent: "origin/main".into(),
    });
    assert!(reason.contains("local branch"), "{reason}");

    repo.git(&["checkout", "-q", "--orphan", "stranger"]);
    commit_file(&repo.root, "s.txt");
    let reason = repo.refused(CommandKind::SetStackParent {
        branch: "stranger".into(),
        parent: "main".into(),
    });
    assert!(reason.contains("share no history"), "{reason}");
}

#[test]
fn an_agent_cannot_write_stack_metadata() {
    let mut repo = Repo::new();
    repo.git(&["branch", "lone"]);
    let agent = Actor::Agent {
        name: "test".into(),
    };
    for kind in [
        CommandKind::SetStackParent {
            branch: "lone".into(),
            parent: "main".into(),
        },
        CommandKind::CreateChildBranch {
            name: "kid".into(),
            parent: "main".into(),
            checkout: false,
        },
    ] {
        let events = repo.run_as(agent.clone(), kind);
        assert!(refusal(&events).unwrap().contains("only a human"));
    }
    assert_eq!(config(&repo.root, "branch.lone.stackParent"), None);
    // Reading the stacks is open to an agent.
    let events = repo.run_as(agent, CommandKind::LoadStacks);
    assert_eq!(state_of(&events), Some(OperationState::Done));
}

#[test]
fn names_with_dots_and_slashes_and_a_checked_out_child_work() {
    let mut repo = Repo::new();
    repo.ok(CommandKind::CreateChildBranch {
        name: "feature/v1.2".into(),
        parent: "main".into(),
        checkout: true,
    });
    assert_eq!(repo.git(&["branch", "--show-current"]), "feature/v1.2");
    let snapshot = repo.child("Feature/v1.2.x", "feature/v1.2");
    assert_eq!(
        shape(&snapshot),
        ["main", "  feature/v1.2", "    Feature/v1.2.x"]
    );
}

#[test]
fn a_hand_written_loop_is_listed_apart() {
    let mut repo = Repo::new();
    repo.git(&["branch", "a"]);
    repo.git(&["branch", "b"]);
    let tip = repo.git(&["rev-parse", "main"]);
    repo.git(&["config", "branch.a.stackParent", "b"]);
    repo.git(&["config", "branch.a.stackParentBase", &tip]);
    repo.git(&["config", "branch.b.stackParent", "a"]);
    repo.git(&["config", "branch.b.stackParentBase", &tip]);
    let snapshot = repo.load();
    assert!(snapshot.stacks.is_empty());
    assert_eq!(snapshot.looped, ["a", "b"]);
    // Setting one parent breaks the loop.
    let snapshot = repo.ok(CommandKind::SetStackParent {
        branch: "a".into(),
        parent: "main".into(),
    });
    assert_eq!(shape(&snapshot), ["main", "  a", "    b"]);
}

#[test]
fn a_child_in_a_linked_worktree_reads_the_shared_config() {
    let mut repo = Repo::new();
    repo.child("topic", "main");
    let other = repo.root.parent().unwrap().join("wt");
    repo.git(&["worktree", "add", "-q", other.to_str().unwrap(), "topic"]);
    let git = SubprocessGit::default();
    let mut engine = Engine::new(Box::new(git));
    engine.handle(
        OperationId(1),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::OpenRepo {
                path: other.to_string_lossy().into_owned(),
            },
        },
    );
    let events = engine.handle(
        OperationId(2),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::LoadStacks,
        },
    );
    let snapshot = stacks(&events).unwrap();
    assert_eq!(shape(&snapshot), ["main", "  topic"]);
}
