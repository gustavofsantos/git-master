//! The scope of the history list (H6, D139) and the own commits of a stack
//! branch, through the engine, against real repositories in temporary
//! directories. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_schema::{
    Actor, Command, CommandKind, Event, HistoryScope, OperationId, OperationState, StackSnapshot,
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
    next: u64,
}

impl Repo {
    /// main (init, m2 after the stack forked) <- base (b1) <- top (t1), on
    /// `top`; plus `side` (s1) off main, in no stack, and a tag `v1` on init.
    fn stack() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        git_in(&root, &["init", "-q", "-b", "main"]);
        commit_file(&root, "init");
        git_in(&root, &["tag", "v1"]);
        let mut repo = Self {
            _dir: dir,
            root,
            engine: Engine::new(Box::new(SubprocessGit::default())),
            next: 1,
        };
        repo.run(CommandKind::OpenRepo {
            path: repo.root.to_string_lossy().into_owned(),
        });
        repo.run(CommandKind::CreateChildBranch {
            name: "base".into(),
            parent: "main".into(),
            checkout: true,
        });
        commit_file(&repo.root, "b1");
        repo.run(CommandKind::CreateChildBranch {
            name: "top".into(),
            parent: "base".into(),
            checkout: false,
        });
        repo.git(&["checkout", "-q", "main"]);
        commit_file(&repo.root, "m2");
        repo.git(&["checkout", "-q", "-b", "side"]);
        commit_file(&repo.root, "s1");
        repo.git(&["checkout", "-q", "top"]);
        commit_file(&repo.root, "t1");
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(&self.root, args)
    }

    fn run(&mut self, kind: CommandKind) -> Vec<Event> {
        self.next += 1;
        let events = self.engine.handle(
            OperationId(self.next),
            &Command {
                actor: Actor::Human,
                kind,
            },
        );
        assert!(
            !events.iter().any(|e| matches!(
                e,
                Event::OperationChanged {
                    state: OperationState::Failed,
                    ..
                }
            )),
            "{events:?}"
        );
        events
    }

    /// The subjects of the first page of `scope`, newest first.
    fn subjects(&mut self, scope: HistoryScope) -> Vec<String> {
        self.run(CommandKind::LoadHistory { scope });
        let engine = &mut self.engine;
        let events: Vec<Event> = engine
            .take_history_jobs()
            .into_iter()
            .flat_map(|job| engine.history_report(job.run()))
            .collect();
        let history = events
            .iter()
            .find_map(|e| match e {
                Event::HistoryLoaded { history, .. } => Some(history.clone()),
                _ => None,
            })
            .expect("the load answers");
        let mut subjects: Vec<String> = history
            .commits
            .into_iter()
            .map(|commit| commit.subject)
            .collect();
        subjects.sort();
        subjects
    }

    fn stacks(&mut self) -> StackSnapshot {
        self.run(CommandKind::LoadStacks)
            .into_iter()
            .find_map(|e| match e {
                Event::StacksLoaded { stacks, .. } => Some(stacks),
                _ => None,
            })
            .expect("LoadStacks answers")
    }
}

fn sorted(list: &[&str]) -> Vec<String> {
    let mut list: Vec<String> = list.iter().map(|s| (*s).to_owned()).collect();
    list.sort();
    list
}

#[test]
fn the_current_branch_is_the_history_of_head() {
    let mut repo = Repo::stack();
    assert_eq!(
        repo.subjects(HistoryScope::CurrentBranch),
        sorted(&["init", "b1", "t1"])
    );
}

#[test]
fn the_current_stack_adds_its_branches_and_the_default_branch() {
    let mut repo = Repo::stack();
    // m2 is main's, which base is behind; s1 is on a branch in no stack.
    assert_eq!(
        repo.subjects(HistoryScope::CurrentStack),
        sorted(&["init", "b1", "t1", "m2"])
    );
}

#[test]
fn the_current_stack_of_a_branch_in_no_stack_is_head() {
    let mut repo = Repo::stack();
    repo.git(&["checkout", "-q", "side"]);
    assert_eq!(
        repo.subjects(HistoryScope::CurrentStack),
        sorted(&["init", "m2", "s1"])
    );
}

#[test]
fn all_refs_reads_every_branch_and_tag() {
    let mut repo = Repo::stack();
    repo.git(&["checkout", "-q", "--orphan", "lonely"]);
    commit_file(&repo.root, "orphan");
    repo.git(&["checkout", "-q", "top"]);
    assert_eq!(
        repo.subjects(HistoryScope::AllRefs),
        sorted(&["init", "b1", "t1", "m2", "s1", "orphan"])
    );
}

#[test]
fn one_ref_reads_a_branch_even_when_a_tag_has_its_name() {
    let mut repo = Repo::stack();
    // A tag named like the branch, on another commit: the branch wins.
    repo.git(&["tag", "side", "v1"]);
    assert_eq!(
        repo.subjects(HistoryScope::Ref {
            name: "side".into()
        }),
        sorted(&["init", "m2", "s1"])
    );
    assert_eq!(
        repo.subjects(HistoryScope::Ref { name: "v1".into() }),
        sorted(&["init"])
    );
}

#[test]
fn one_ref_refuses_a_name_read_as_an_option() {
    let mut repo = Repo::stack();
    repo.next += 1;
    let events = repo.engine.handle(
        OperationId(repo.next),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::LoadHistory {
                scope: HistoryScope::Ref {
                    name: "--all".into(),
                },
            },
        },
    );
    assert!(events.iter().any(|e| matches!(
        e,
        Event::OperationChanged {
            state: OperationState::Failed,
            ..
        }
    )));
}

#[test]
fn each_stack_branch_lists_its_own_commits_newest_first() {
    let mut repo = Repo::stack();
    let t2 = commit_file(&repo.root, "t2");
    let t1 = repo.git(&["rev-parse", "top~1"]);
    let b1 = repo.git(&["rev-parse", "base"]);
    let snapshot = repo.stacks();
    assert_eq!(snapshot.branch("top").unwrap().commits, [t2, t1]);
    assert_eq!(snapshot.branch("base").unwrap().commits, [b1]);
    assert!(snapshot.branch("main").unwrap().commits.is_empty());
    // base is behind main by m2.
    assert_eq!(snapshot.branch("base").unwrap().behind, 1);
}
