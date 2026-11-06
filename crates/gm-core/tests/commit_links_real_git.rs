//! Commit links after a rewrite (D145) against real repositories in temporary directories: an
//! amend, a rebase, a squash, a reorder, a rewrite outside `GitMaster` with a unique match, one with
//! a duplicate patch (ambiguous), a commit no rewrite explains (gone), and a restack by `GitMaster`.
//! The stored id of a link never changes. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::threads::store::{Batch, NewEntry, ThreadStore};
use gm_schema::threads::{CommitState, LinkKind, LinkOrigin, ThreadDetail, ThreadLink};
use gm_schema::{
    Actor, Command, CommandKind, EntryKind, Event, OperationId, OperationState, RestackPlan,
};

fn git_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let mut command = Process::new("git");
    command
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
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = git_env(dir, args, &[]);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
    next: u64,
}

fn state_of(events: &[Event]) -> Option<OperationState> {
    events.iter().rev().find_map(|event| match event {
        Event::OperationChanged { state, .. } => Some(*state),
        _ => None,
    })
}

impl Repo {
    /// A repository on `main` with one commit, `base.txt`, and the engine open on it.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        git_in(&root, &["init", "-q", "-b", "main"]);
        // The identity a replay commits with: the global config is kept out of these tests.
        git_in(&root, &["config", "user.name", "t"]);
        git_in(&root, &["config", "user.email", "t@t"]);
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git_in(&root, &["add", "."]);
        git_in(&root, &["commit", "-q", "-m", "base"]);
        let git = SubprocessGit {
            log: Some(CommandLog::default()),
            global_config: Some("/dev/null".into()),
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
        assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
        Self {
            _dir: dir,
            root,
            engine,
            next: 2,
        }
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(&self.root, args)
    }

    /// Commits `content` to `file` on the current branch, and returns the new HEAD.
    fn commit(&self, file: &str, content: &str, message: &str) -> String {
        std::fs::write(self.root.join(file), content).unwrap();
        self.git(&["add", "--", file]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn store(&self) -> ThreadStore {
        ThreadStore::open(&self.root.join(".git")).expect("open the store")
    }

    /// A thread with one prompt, on `branch`, whose uid is returned.
    fn thread(&self, branch: &str) -> String {
        let entry = NewEntry {
            source_id: "p1".into(),
            kind: EntryKind::User,
            parent_id: None,
            tool: None,
            text: "change the files".into(),
            args_json: None,
            is_error: false,
            truncated: false,
            orig_bytes: None,
            redactions: 0,
            hits: Vec::new(),
            at: 1_700_000_000_000,
            edit: None,
        };
        let batch = Batch {
            harness: "claude-code".into(),
            session_id: "s-1".into(),
            worktree: self.root.to_string_lossy().into_owned(),
            branch: Some(branch.into()),
            head: None,
            model: Some("claude".into()),
            harness_version: None,
            transcript: None,
            cursor: None,
            started_at: 1_700_000_000_000,
            at: 1_700_000_000_010,
            ended: None,
            entries: vec![entry],
            gaps: Vec::new(),
        };
        let mut store = self.store();
        store.apply(&batch).expect("apply").uid
    }

    /// Links thread `uid` to commit `commit`, as `GitMaster` observes a commit (D126).
    fn link(&self, uid: &str, commit: &str) {
        self.store()
            .link(
                uid,
                &ThreadLink {
                    member: None,
                    kind: LinkKind::Commit,
                    reference: commit.to_owned(),
                    origin: LinkOrigin::Observed,
                    resolved: None,
                    state: None,
                },
            )
            .expect("link");
    }

    fn run(&mut self, kind: CommandKind) -> Vec<Event> {
        self.next += 1;
        self.engine.handle(
            OperationId(self.next),
            &Command {
                actor: Actor::Human,
                kind,
            },
        )
    }

    fn read(&mut self, uid: &str) -> ThreadDetail {
        let events = self.run(CommandKind::ReadThread {
            uid: uid.into(),
            from_seq: 0,
            limit: 50,
        });
        events
            .iter()
            .find_map(|event| match event {
                Event::ThreadRead { detail, .. } => Some(detail.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no thread read: {events:?}"))
    }

    /// The link of `commit` in what `read` answered. Its stored id is `commit`, always.
    fn link_state(&mut self, uid: &str, commit: &str) -> (CommitState, Option<String>) {
        let detail = self.read(uid);
        let link = detail
            .links
            .iter()
            .find(|link| link.reference == commit)
            .unwrap_or_else(|| panic!("the stored id {commit} changed or is missing"));
        (
            link.state.expect("a commit link has a state"),
            link.resolved.clone(),
        )
    }
}

#[test]
fn an_amend_that_keeps_the_change_resolves_to_the_new_commit() {
    let mut repo = Repo::new();
    let first = repo.commit("c.txt", "c\n", "one");
    let uid = repo.thread("main");
    repo.link(&uid, &first);
    let amended = {
        repo.git(&["commit", "-q", "--amend", "-m", "two"]);
        repo.git(&["rev-parse", "HEAD"])
    };
    assert_ne!(first, amended);
    assert_eq!(
        repo.link_state(&uid, &first),
        (CommitState::Remapped, Some(amended.clone()))
    );
    // The row keeps the id it was stored with.
    assert_eq!(
        repo.store()
            .commit_link_cache(&uid)
            .unwrap()
            .into_iter()
            .find(|(reference, _)| reference == &first)
            .and_then(|(_, cache)| cache),
        Some(amended)
    );
}

#[test]
fn an_amend_that_changes_the_patch_leaves_the_commit_gone() {
    let mut repo = Repo::new();
    let first = repo.commit("d.txt", "d\n", "one");
    let uid = repo.thread("main");
    repo.link(&uid, &first);
    std::fs::write(repo.root.join("d.txt"), "changed\n").unwrap();
    repo.git(&["commit", "-q", "-a", "--amend", "-m", "one"]);
    assert_eq!(repo.link_state(&uid, &first), (CommitState::Gone, None));
}

#[test]
fn a_rebase_resolves_each_replayed_commit() {
    let mut repo = Repo::new();
    repo.git(&["checkout", "-q", "-b", "feature"]);
    let feature = repo.commit("f.txt", "f\n", "feature work");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit("m.txt", "m\n", "main moves on");
    repo.git(&["checkout", "-q", "feature"]);
    repo.git(&["rebase", "-q", "main"]);
    let replayed = repo.git(&["rev-parse", "HEAD"]);
    assert_ne!(feature, replayed);
    let uid = repo.thread("feature");
    repo.link(&uid, &feature);
    assert_eq!(
        repo.link_state(&uid, &feature),
        (CommitState::Remapped, Some(replayed))
    );
}

#[test]
fn a_squash_leaves_the_squashed_commits_gone_and_the_base_current() {
    let mut repo = Repo::new();
    let base = repo.git(&["rev-parse", "HEAD"]);
    let first = repo.commit("a.txt", "a\n", "a");
    let second = repo.commit("b.txt", "b\n", "b");
    let uid = repo.thread("main");
    repo.link(&uid, &first);
    repo.link(&uid, &second);
    repo.link(&uid, &base);
    repo.git(&["reset", "-q", "--soft", &base]);
    repo.git(&["commit", "-q", "-m", "a and b"]);
    assert_eq!(repo.link_state(&uid, &first), (CommitState::Gone, None));
    assert_eq!(repo.link_state(&uid, &second), (CommitState::Gone, None));
    assert_eq!(repo.link_state(&uid, &base), (CommitState::Current, None));
}

#[test]
fn a_reorder_resolves_each_commit_to_its_replay() {
    let mut repo = Repo::new();
    let first = repo.commit("a.txt", "a\n", "a");
    let second = repo.commit("b.txt", "b\n", "b");
    let uid = repo.thread("main");
    repo.link(&uid, &first);
    repo.link(&uid, &second);
    // The todo list that puts `second` first, written by a sequence editor that copies it in.
    let todo = repo.root.parent().unwrap().join("todo.txt");
    std::fs::write(&todo, format!("pick {second}\npick {first}\n")).unwrap();
    let editor = format!("cp {} ", todo.display());
    let output = git_env(
        &repo.root,
        &["rebase", "-q", "-i", "HEAD~2"],
        &[("GIT_SEQUENCE_EDITOR", editor.as_str())],
    );
    assert!(output.status.success(), "{output:?}");
    let new_tip = repo.git(&["rev-parse", "HEAD"]);
    let below = repo.git(&["rev-parse", "HEAD~1"]);
    assert_eq!(
        repo.link_state(&uid, &first),
        (CommitState::Remapped, Some(new_tip))
    );
    assert_eq!(
        repo.link_state(&uid, &second),
        (CommitState::Remapped, Some(below))
    );
}

#[test]
fn a_rewrite_with_two_commits_of_the_same_patch_is_ambiguous() {
    let mut repo = Repo::new();
    let base = repo.git(&["rev-parse", "HEAD"]);
    let original = repo.commit("f.txt", "x\n", "add f");
    let uid = repo.thread("main");
    repo.link(&uid, &original);
    // The branch is rebuilt from the base: the same file is added, removed and added again, so
    // two commits of the new range carry the patch of the original one.
    repo.git(&["reset", "-q", "--hard", &base]);
    repo.commit("f.txt", "x\n", "add f again");
    repo.git(&["rm", "-q", "f.txt"]);
    repo.git(&["commit", "-q", "-m", "remove f"]);
    repo.commit("f.txt", "x\n", "add f a third time");
    assert_eq!(
        repo.link_state(&uid, &original),
        (CommitState::Ambiguous, None)
    );
}

#[test]
fn a_commit_that_no_rewrite_explains_is_gone_and_a_missing_one_too() {
    let mut repo = Repo::new();
    let base = repo.git(&["rev-parse", "HEAD"]);
    let original = repo.commit("g.txt", "g\n", "add g");
    let missing = "0123456789abcdef0123456789abcdef01234567";
    let uid = repo.thread("main");
    repo.link(&uid, &original);
    repo.link(&uid, missing);
    repo.git(&["reset", "-q", "--hard", &base]);
    repo.commit("unrelated.txt", "u\n", "something else");
    assert_eq!(repo.link_state(&uid, &original), (CommitState::Gone, None));
    assert_eq!(repo.link_state(&uid, missing), (CommitState::Gone, None));
}

#[test]
fn a_restack_by_gitmaster_is_recorded_and_the_link_resolves_to_the_replay() {
    let mut repo = Repo::new();
    repo.run(CommandKind::CreateChildBranch {
        name: "a".into(),
        parent: "main".into(),
        checkout: false,
    });
    repo.git(&["checkout", "-q", "a"]);
    let on_a = repo.commit("a.txt", "a\n", "a work");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit("m.txt", "m\n", "main moves on");
    let uid = repo.thread("a");
    repo.link(&uid, &on_a);

    let events = repo.run(CommandKind::PlanRestack {
        branch: "main".into(),
        skip: Vec::new(),
    });
    let plan: RestackPlan = events
        .iter()
        .find_map(|event| match event {
            Event::RestackPlanned { plan, .. } => Some(plan.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no plan: {events:?}"));
    let events = repo.run(CommandKind::ApplyRestack {
        branch: "main".into(),
        plan_id: plan.plan_id.clone(),
        skip: Vec::new(),
    });
    assert_eq!(state_of(&events), Some(OperationState::Done), "{events:?}");
    let replay = repo.git(&["rev-parse", "a"]);
    assert_ne!(on_a, replay);
    // The restack wrote the replay into the cache, before any read.
    assert_eq!(
        repo.store()
            .commit_link_cache(&uid)
            .unwrap()
            .into_iter()
            .find(|(reference, _)| reference == &on_a)
            .and_then(|(_, cache)| cache),
        Some(replay.clone())
    );
    assert_eq!(
        repo.link_state(&uid, &on_a),
        (CommitState::Remapped, Some(replay))
    );
}

#[test]
fn a_current_commit_link_shows_current_and_no_replacement() {
    let mut repo = Repo::new();
    let tip = repo.commit("h.txt", "h\n", "h");
    let uid = repo.thread("main");
    repo.link(&uid, &tip);
    assert_eq!(repo.link_state(&uid, &tip), (CommitState::Current, None));
}
