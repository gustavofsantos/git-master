//! The workspace commands of the engine (D183 (1)) against real repositories in temporary
//! directories: the reads answer with the listing, the plan and the removal inspection; the writes
//! create, remove and check out members, and are human only.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_schema::{
    Actor, BranchHow, Command, CommandKind, Event, MemberPlan, MemberRequest, OperationId,
    OperationState, RemovalBlocker, RemovalBlockerKind, WorkspaceProblem, WorkspaceRow,
};

fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
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

/// A scratch folder with a global Git configuration that gives commits an identity, so the
/// engine's workspace commands can commit without touching the process environment.
struct Scratch {
    base: tempfile::TempDir,
    identity: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let base = tempfile::tempdir().unwrap();
        let identity = base.path().join("identity.gitconfig");
        std::fs::write(
            &identity,
            "[user]\n\tname = t\n\temail = t@t\n[commit]\n\tgpgsign = false\n",
        )
        .unwrap();
        Self { base, identity }
    }

    fn path(&self) -> PathBuf {
        self.base.path().canonicalize().unwrap()
    }

    fn root(&self) -> PathBuf {
        self.path().join("ws")
    }

    /// A repository `<base>/<name>` on `main` with one commit.
    fn repo(&self, name: &str) -> PathBuf {
        let repo = self.path().join(name);
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        repo
    }

    /// A repository whose remote-tracking branch `origin/<branch>` exists and whose local branch
    /// does not, as a clone of a remote that has `branch` has.
    fn repo_with_remote_branch(&self, name: &str, branch: &str) -> PathBuf {
        let repo = self.repo(name);
        let origin = self.path().join(format!("origin-{name}.git"));
        git_in(&self.path(), &["init", "-q", "--bare", &text(&origin)]);
        git_in(&repo, &["remote", "add", "origin", &text(&origin)]);
        git_in(&repo, &["push", "-q", "origin", "main"]);
        git_in(&repo, &["branch", branch]);
        git_in(&repo, &["push", "-q", "origin", branch]);
        git_in(&repo, &["branch", "-q", "-D", branch]);
        repo
    }

    /// The workspace under the root, by name: the folder `<root>/<name>`.
    fn ws(&self, name: &str) -> PathBuf {
        self.root().join(name)
    }
}

/// An engine over the real Git, with the workspace commands reading `identity`.
struct Harness {
    engine: Engine,
    next: u64,
}

impl Harness {
    fn new(scratch: &Scratch) -> Self {
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_workspace_global_config(Some(scratch.identity.clone()));
        Self { engine, next: 1 }
    }

    fn run(&mut self, actor: Actor, kind: CommandKind) -> Vec<Event> {
        self.next += 1;
        let command = Command { actor, kind };
        self.engine.handle(OperationId(self.next), &command)
    }

    fn human(&mut self, kind: CommandKind) -> Vec<Event> {
        self.run(Actor::Human, kind)
    }

    /// Opens `repo` as the window's repository; the workspace commands work from it.
    fn open(&mut self, repo: &Path) {
        let events = self.human(CommandKind::OpenRepo { path: text(repo) });
        assert!(done(&events), "open failed: {events:?}");
    }
}

fn state_of(events: &[Event]) -> Option<OperationState> {
    events.iter().rev().find_map(|event| match event {
        Event::OperationChanged { state, .. } => Some(*state),
        _ => None,
    })
}

fn done(events: &[Event]) -> bool {
    state_of(events) == Some(OperationState::Done)
}

/// The reason of the failed operation, if it failed.
fn refusal(events: &[Event]) -> Option<String> {
    events.iter().find_map(|event| match event {
        Event::OperationChanged {
            state: OperationState::Failed,
            detail,
            ..
        } => detail.clone(),
        _ => None,
    })
}

fn create(name: &str, members: &[(&Path, &str)], root: &Path) -> CommandKind {
    CommandKind::CreateWorkspace {
        name: name.into(),
        members: members
            .iter()
            .map(|(repo, branch)| MemberRequest {
                repo: text(repo),
                branch: (*branch).into(),
            })
            .collect(),
        base: None,
        root: Some(text(root)),
    }
}

fn plan(name: &str, members: &[(&Path, &str)], root: &Path) -> CommandKind {
    match create(name, members, root) {
        CommandKind::CreateWorkspace {
            name,
            members,
            base,
            root,
        } => CommandKind::PlanWorkspace {
            name,
            members,
            base,
            root,
        },
        _ => unreachable!(),
    }
}

fn listed(events: &[Event]) -> Option<Vec<WorkspaceRow>> {
    events.iter().find_map(|event| match event {
        Event::WorkspacesListed { workspaces, .. } => Some(workspaces.clone()),
        _ => None,
    })
}

/// The members and problems of the plan answered by `events`.
fn planned(events: &[Event]) -> Option<(Vec<MemberPlan>, Vec<WorkspaceProblem>)> {
    events.iter().find_map(|event| match event {
        Event::WorkspacePlanned {
            members, problems, ..
        } => Some((members.clone(), problems.clone())),
        _ => None,
    })
}

fn inspected(events: &[Event]) -> Option<Vec<RemovalBlocker>> {
    events.iter().find_map(|event| match event {
        Event::WorkspaceRemovalInspected { blockers, .. } => Some(blockers.clone()),
        _ => None,
    })
}

fn worktrees_of(repo: &Path) -> String {
    git_in(repo, &["worktree", "list", "--porcelain"])
}

/// A repository `a` whose branch `feat/a` is checked out in the linked worktree `a-wt`, so a
/// workspace that wants `feat/a` is refused. Returns the main checkout and the worktree.
fn held_branch(scratch: &Scratch) -> (PathBuf, PathBuf) {
    let repo = scratch.repo("a");
    let worktree = scratch.path().join("a-wt");
    git_in(
        &repo,
        &["worktree", "add", "-q", "-b", "feat/a", &text(&worktree)],
    );
    (repo, worktree)
}

#[test]
fn list_shows_the_workspaces_under_the_root_and_skips_other_folders() {
    let scratch = Scratch::new();
    let b = scratch.repo_with_remote_branch("b", "feat/b");
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&b);
    let made = harness.human(create(
        "one",
        &[(&b, "feat/b"), (&c, "feat/c")],
        &scratch.root(),
    ));
    assert!(done(&made), "create failed: {made:?}");
    std::fs::create_dir_all(scratch.root().join("stray")).unwrap();

    let events = harness.human(CommandKind::ListWorkspaces {
        root: Some(text(&scratch.root())),
    });

    let rows = listed(&events).expect("the listing");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].name, "one");
    assert_eq!(rows[0].members, 2);
    assert_eq!(rows[0].path, text(&scratch.ws("one")));
}

#[test]
fn plan_answers_each_member_and_names_the_worktree_that_holds_a_branch() {
    let scratch = Scratch::new();
    let b = scratch.repo_with_remote_branch("b", "feat/b");
    let c = scratch.repo("c");
    let (a, held) = held_branch(&scratch);
    let d = scratch.repo("d");
    git_in(&d, &["branch", "feat/d"]);
    let mut harness = Harness::new(&scratch);
    harness.open(&b);

    let events = harness.human(plan(
        "two",
        &[
            (&a, "feat/a"),
            (&b, "feat/b"),
            (&c, "feat/c"),
            (&d, "feat/d"),
        ],
        &scratch.root(),
    ));

    let (members, problems) = planned(&events).expect("the plan");
    let by_member = |name: &str| {
        members
            .iter()
            .find(|member| member.member == name)
            .unwrap_or_else(|| panic!("no member {name} in {members:?}"))
            .clone()
    };
    assert_eq!(
        by_member("b").how,
        BranchHow::TrackRemote {
            remote: "origin/feat/b".into()
        }
    );
    assert_eq!(by_member("d").how, BranchHow::CheckOut);
    assert_eq!(
        by_member("c").how,
        BranchHow::CreateFrom {
            base: "main".into()
        }
    );
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].path, Some(text(&held)));
    assert!(
        problems[0].message.contains("checked out at"),
        "{problems:?}"
    );
    assert!(!scratch.ws("two").exists(), "planning writes nothing");
}

#[test]
fn create_writes_the_folder_its_members_and_their_pointer_files() {
    let scratch = Scratch::new();
    let b = scratch.repo_with_remote_branch("b", "feat/b");
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&b);

    let events = harness.human(create(
        "pay",
        &[(&b, "feat/b"), (&c, "feat/c")],
        &scratch.root(),
    ));

    assert!(done(&events), "create failed: {events:?}");
    let ws = scratch.ws("pay");
    assert!(events.iter().any(|event| matches!(
        event,
        Event::WorkspaceCreated { path, .. } if *path == text(&ws)
    )));
    assert_eq!(
        git_in(&ws, &["config", "--local", "gitmaster.workspace"]),
        "true"
    );
    let gitmodules = std::fs::read_to_string(ws.join(".gitmodules")).unwrap();
    assert!(gitmodules.contains("path = b"), "{gitmodules}");
    assert!(gitmodules.contains("path = c"), "{gitmodules}");
    assert!(ws.join("b/a.txt").is_file());
    assert!(ws.join("c/a.txt").is_file());
    assert!(worktrees_of(&b).contains(&text(&ws.join("b"))));
    assert_eq!(
        git_in(&b, &["rev-parse", "--abbrev-ref", "feat/b"]),
        "feat/b"
    );
    assert_eq!(
        git_in(&c, &["rev-parse", "--abbrev-ref", "feat/c"]),
        "feat/c"
    );
    // The member's repository lists the workspace (D180 (2)).
    for repo in [&b, &c] {
        let common = git_in(
            repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        );
        let pointers = std::fs::read_to_string(Path::new(&common).join("gitmaster/workspaces"))
            .expect("the pointer file");
        assert!(pointers.lines().any(|line| line == text(&ws)), "{pointers}");
    }
    // The workspace is written to the log; the reads are not.
    let logged = harness.engine.log().iter().any(|entry| {
        matches!(entry.kind, CommandKind::CreateWorkspace { .. })
            && entry.state == OperationState::Done
    });
    assert!(logged, "the create is in the command log");
}

#[test]
fn a_plan_with_a_problem_creates_nothing() {
    let scratch = Scratch::new();
    let b = scratch.repo_with_remote_branch("b", "feat/b");
    let (a, _) = held_branch(&scratch);
    let mut harness = Harness::new(&scratch);
    harness.open(&b);

    let events = harness.human(create(
        "broken",
        &[(&a, "feat/a"), (&b, "feat/b")],
        &scratch.root(),
    ));

    assert!(!done(&events));
    assert!(
        refusal(&events).is_some_and(|reason| reason.contains("no workspace was created")),
        "{events:?}"
    );
    assert!(!scratch.ws("broken").exists());
    assert!(
        !worktrees_of(&b).contains("feat/b"),
        "no member worktree was added"
    );
    assert_eq!(git_in(&b, &["branch", "--list", "feat/b"]), "");
}

#[test]
fn inspection_reports_a_dirty_member_as_an_overridable_blocker() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "dirty",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    std::fs::write(scratch.ws("dirty/c/notes.txt"), "draft\n").unwrap();

    let events = harness.human(CommandKind::InspectWorkspaceRemoval {
        path: text(&scratch.ws("dirty")),
        closing: false,
    });

    let blockers = inspected(&events).expect("the inspection");
    assert_eq!(blockers.len(), 1, "{blockers:?}");
    assert_eq!(blockers[0].kind, RemovalBlockerKind::MemberDirty);
    assert_eq!(blockers[0].member.as_deref(), Some("c"));
    assert!(blockers[0].overridable);
}

#[test]
fn remove_is_refused_without_force_and_done_with_it() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "gone",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    std::fs::write(scratch.ws("gone/c/notes.txt"), "draft\n").unwrap();
    let path = text(&scratch.ws("gone"));

    let refused = harness.human(CommandKind::RemoveWorkspace {
        path: path.clone(),
        force: false,
        closing: false,
    });
    assert!(!done(&refused));
    assert!(
        refusal(&refused).is_some_and(|reason| reason.contains("use --force")),
        "{refused:?}"
    );
    assert!(
        scratch.ws("gone").exists(),
        "a refused removal removes nothing"
    );

    let forced = harness.human(CommandKind::RemoveWorkspace {
        path: path.clone(),
        force: true,
        closing: false,
    });
    assert!(done(&forced), "forced removal failed: {forced:?}");
    assert!(!scratch.ws("gone").exists());
    assert!(
        !worktrees_of(&c).contains("gone/c"),
        "the member's worktree is removed"
    );
    assert!(
        !git_in(&c, &["branch", "--list", "feat/c"]).is_empty(),
        "the branch is kept"
    );
}

#[test]
fn removing_the_window_own_workspace_is_refused_even_when_forced() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "own",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    harness.open(&scratch.ws("own"));
    let path = text(&scratch.ws("own"));

    let blockers = inspected(&harness.human(CommandKind::InspectWorkspaceRemoval {
        path: path.clone(),
        closing: false,
    }))
    .expect("the inspection");
    assert_eq!(blockers.len(), 1, "{blockers:?}");
    assert_eq!(blockers[0].kind, RemovalBlockerKind::Current);
    assert!(!blockers[0].overridable);

    let events = harness.human(CommandKind::RemoveWorkspace {
        path,
        force: true,
        closing: false,
    });
    assert!(!done(&events));
    assert!(
        refusal(&events).is_some_and(|reason| reason.contains("inside that workspace")),
        "{events:?}"
    );
    assert!(scratch.ws("own").exists());
}

#[test]
fn check_out_member_branch_puts_a_detached_member_back_on_its_branch() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "det",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    let member = scratch.ws("det/c");
    git_in(&member, &["checkout", "-q", "--detach"]);
    assert_eq!(
        git_in(&member, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "HEAD"
    );
    harness.open(&scratch.ws("det"));

    let events = harness.human(CommandKind::CheckOutMemberBranch { path: "c".into() });

    assert!(done(&events), "check out failed: {events:?}");
    assert_eq!(
        git_in(&member, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "feat/c"
    );
}

#[test]
fn check_out_refuses_a_detached_member_whose_commits_are_on_no_branch() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "orphan",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    let member = scratch.ws("orphan/c");
    git_in(&member, &["checkout", "-q", "--detach"]);
    std::fs::write(member.join("b.txt"), "two\n").unwrap();
    git_in(&member, &["add", "."]);
    git_in(&member, &["commit", "-q", "-m", "on no branch"]);
    let head = git_in(&member, &["rev-parse", "HEAD"]);
    harness.open(&scratch.ws("orphan"));

    let events = harness.human(CommandKind::CheckOutMemberBranch { path: "c".into() });

    assert!(!done(&events), "the checkout must refuse: {events:?}");
    assert!(
        refusal(&events).is_some_and(|reason| reason
            .contains("1 commit on the detached HEAD is on no branch; put it on a branch first.")),
        "{events:?}"
    );
    assert_eq!(
        git_in(&member, &["rev-parse", "HEAD"]),
        head,
        "HEAD unchanged"
    );
    assert_eq!(
        git_in(&member, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "HEAD",
        "still detached"
    );
}

#[test]
fn check_out_counts_the_commits_a_refused_detached_member_would_leave() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "two",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    let member = scratch.ws("two/c");
    git_in(&member, &["checkout", "-q", "--detach"]);
    for name in ["b.txt", "d.txt"] {
        std::fs::write(member.join(name), "x\n").unwrap();
        git_in(&member, &["add", "."]);
        git_in(&member, &["commit", "-q", "-m", name]);
    }
    harness.open(&scratch.ws("two"));

    let events = harness.human(CommandKind::CheckOutMemberBranch { path: "c".into() });

    assert!(!done(&events));
    assert!(
        refusal(&events).is_some_and(|reason| reason.contains("2 commits on the detached HEAD")),
        "{events:?}"
    );
}

#[test]
fn check_out_puts_a_detached_member_on_a_commit_a_branch_holds() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "held",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    let member = scratch.ws("held/c");
    // A commit that feat/c holds: detaching at its tip leaves nothing on no branch.
    git_in(&member, &["checkout", "-q", "--detach"]);
    harness.open(&scratch.ws("held"));

    let events = harness.human(CommandKind::CheckOutMemberBranch { path: "c".into() });

    assert!(done(&events), "a held commit is checked out: {events:?}");
    assert_eq!(
        git_in(&member, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "feat/c"
    );
}

#[test]
fn an_agent_cannot_write_a_workspace_but_can_read_one() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);

    let made = harness.run(
        Actor::Agent {
            name: "test".into(),
        },
        create("agent", &[(&c, "feat/c")], &scratch.root()),
    );
    assert!(!done(&made));
    assert!(
        refusal(&made).is_some_and(|reason| reason.contains("only a human")),
        "{made:?}"
    );
    assert!(!scratch.ws("agent").exists());

    let listing = harness.run(
        Actor::Agent {
            name: "test".into(),
        },
        CommandKind::ListWorkspaces {
            root: Some(text(&scratch.root())),
        },
    );
    assert!(done(&listing));
    assert!(
        harness.engine.log().iter().all(|entry| !matches!(
            entry.kind,
            CommandKind::ListWorkspaces { .. } | CommandKind::PlanWorkspace { .. }
        )),
        "reads are not logged"
    );
}

#[test]
fn closing_removes_the_window_own_workspace_when_nothing_would_be_lost() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "shut",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    harness.open(&scratch.ws("shut"));
    let path = text(&scratch.ws("shut"));

    // Without `closing` the window's own workspace is refused, even forced (D183 (4)).
    let refused = harness.human(CommandKind::RemoveWorkspace {
        path: path.clone(),
        force: true,
        closing: false,
    });
    assert!(!done(&refused));
    assert!(scratch.ws("shut").exists());

    let inspection = harness.human(CommandKind::InspectWorkspaceRemoval {
        path: path.clone(),
        closing: true,
    });
    assert!(
        inspected(&inspection).is_some_and(|blockers| blockers.is_empty()),
        "{inspection:?}"
    );

    let removed = harness.human(CommandKind::RemoveWorkspace {
        path: path.clone(),
        force: false,
        closing: true,
    });
    assert!(done(&removed), "closing removal failed: {removed:?}");
    assert!(
        removed
            .iter()
            .any(|event| matches!(event, Event::WorkspaceRemoved { .. })),
        "{removed:?}"
    );
    assert!(!scratch.ws("shut").exists());
    assert!(!worktrees_of(&c).contains("shut/c"));
    assert!(
        !git_in(&c, &["branch", "--list", "feat/c"]).is_empty(),
        "the branch is kept"
    );
}

#[test]
fn closing_still_refuses_a_dirty_member_without_force() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "dirty-shut",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    harness.open(&scratch.ws("dirty-shut"));
    std::fs::write(scratch.ws("dirty-shut/c/notes.txt"), "draft\n").unwrap();
    let path = text(&scratch.ws("dirty-shut"));

    let blockers = inspected(&harness.human(CommandKind::InspectWorkspaceRemoval {
        path: path.clone(),
        closing: true,
    }))
    .expect("the inspection");
    assert_eq!(blockers.len(), 1, "{blockers:?}");
    assert_eq!(blockers[0].kind, RemovalBlockerKind::MemberDirty);
    assert!(blockers[0].overridable);

    let refused = harness.human(CommandKind::RemoveWorkspace {
        path: path.clone(),
        force: false,
        closing: true,
    });
    assert!(!done(&refused));
    assert!(
        refusal(&refused).is_some_and(|reason| reason.contains("use --force")),
        "{refused:?}"
    );
    assert!(scratch.ws("dirty-shut").exists());

    let forced = harness.human(CommandKind::RemoveWorkspace {
        path,
        force: true,
        closing: true,
    });
    assert!(done(&forced), "forced closing removal failed: {forced:?}");
    assert!(!scratch.ws("dirty-shut").exists());
}

#[test]
fn a_workspace_marked_with_the_old_kind_key_is_listed_and_removed_from_its_window() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "old",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    let ws = scratch.ws("old");
    git_in(
        &ws,
        &["config", "--local", "--unset", "gitmaster.workspace"],
    );
    git_in(
        &ws,
        &["config", "--local", "gitmaster.workspace.kind", "ephemeral"],
    );

    let events = harness.human(CommandKind::ListWorkspaces {
        root: Some(text(&scratch.root())),
    });
    let rows = listed(&events).expect("the listing");
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["old"], "{rows:?}");

    harness.open(&ws);
    let path = text(&ws);
    let blockers = inspected(&harness.human(CommandKind::InspectWorkspaceRemoval {
        path: path.clone(),
        closing: true,
    }))
    .expect("the inspection");
    assert!(blockers.is_empty(), "{blockers:?}");

    let removed = harness.human(CommandKind::RemoveWorkspace {
        path,
        force: false,
        closing: true,
    });
    assert!(done(&removed), "removal failed: {removed:?}");
    assert!(!ws.exists());
    assert!(
        harness.engine.log().iter().all(|entry| !matches!(
            entry.kind,
            CommandKind::ListWorkspaces { .. } | CommandKind::InspectWorkspaceRemoval { .. }
        )),
        "reads are not logged"
    );
}

// Changing the members of a workspace that exists, and the clone that brings a repository for
// one (D256).

fn members_request(members: &[(&Path, &str)]) -> Vec<MemberRequest> {
    members
        .iter()
        .map(|(repo, branch)| MemberRequest {
            repo: text(repo),
            branch: (*branch).into(),
        })
        .collect()
}

fn gitmodules(ws: &Path) -> String {
    std::fs::read_to_string(ws.join(".gitmodules")).unwrap_or_default()
}

/// Handles a remote command and runs its job to the end, as the runner does.
fn remote(harness: &mut Harness, kind: CommandKind) -> Vec<Event> {
    let mut events = harness.human(kind);
    for job in harness.engine.take_remote_jobs() {
        events.extend(harness.engine.remote_report(job.run()));
    }
    events
}

#[test]
fn members_are_planned_and_added_to_a_workspace_that_exists() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let d = scratch.repo("d");
    let e = scratch.repo_with_remote_branch("e", "feat/x");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "grow",
        &[(&c, "feat/x")],
        &scratch.root()
    ))));
    let ws = scratch.ws("grow");

    let (members, problems) = planned(&harness.human(CommandKind::PlanWorkspaceMembers {
        workspace: text(&ws),
        members: members_request(&[(&d, "feat/x"), (&e, "feat/x")]),
    }))
    .expect("the plan");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(members.len(), 2);
    assert_eq!(
        members[0].how,
        BranchHow::CreateFrom {
            base: "main".into()
        }
    );
    assert_eq!(
        members[1].how,
        BranchHow::TrackRemote {
            remote: "origin/feat/x".into()
        }
    );
    assert!(!ws.join("d").exists(), "planning writes nothing");

    let events = harness.human(CommandKind::AddWorkspaceMembers {
        workspace: text(&ws),
        members: members_request(&[(&d, "feat/x"), (&e, "feat/x")]),
    });
    assert!(done(&events), "{events:?}");
    assert!(events.iter().any(|event| matches!(
        event,
        Event::WorkspaceMembersAdded { members, .. } if *members == ["d", "e"]
    )));
    assert_eq!(
        git_in(&ws.join("d"), &["rev-parse", "--abbrev-ref", "HEAD"]),
        "feat/x"
    );
    assert!(gitmodules(&ws).contains("path = e"));
    assert_eq!(
        git_in(&ws, &["status", "--porcelain"]),
        "",
        "each member is committed"
    );
    assert!(git_in(&ws, &["log", "--format=%s"]).contains("Add e on feat/x"));
}

#[test]
fn adding_a_repository_that_is_already_a_member_adds_nothing() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let d = scratch.repo("d");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "twice",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    let ws = scratch.ws("twice");

    let (_, problems) = planned(&harness.human(CommandKind::PlanWorkspaceMembers {
        workspace: text(&ws),
        members: members_request(&[(&d, "feat/d"), (&c, "feat/other")]),
    }))
    .expect("the plan");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].message.contains("already a member"),
        "{problems:?}"
    );

    let events = harness.human(CommandKind::AddWorkspaceMembers {
        workspace: text(&ws),
        members: members_request(&[(&d, "feat/d"), (&c, "feat/other")]),
    });
    assert!(!done(&events));
    assert!(!ws.join("d").exists(), "one refused row adds none");
}

#[test]
fn a_member_is_removed_with_a_commit_and_its_branch_is_kept() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let d = scratch.repo("d");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "shrink",
        &[(&c, "feat/x"), (&d, "feat/x")],
        &scratch.root()
    ))));
    let ws = scratch.ws("shrink");
    harness.open(&ws);

    let blockers = harness.human(CommandKind::InspectMemberRemoval {
        workspace: text(&ws),
        member: "d".into(),
    });
    assert!(
        blockers.iter().any(|event| matches!(
            event,
            Event::MemberRemovalInspected { blockers, .. } if blockers.is_empty()
        )),
        "{blockers:?}"
    );

    let events = harness.human(CommandKind::RemoveWorkspaceMember {
        workspace: text(&ws),
        member: "d".into(),
        force: false,
    });
    assert!(done(&events), "{events:?}");
    assert!(!ws.join("d").exists());
    assert!(!gitmodules(&ws).contains("path = d"));
    assert!(gitmodules(&ws).contains("path = c"));
    assert_eq!(git_in(&ws, &["status", "--porcelain"]), "");
    assert_eq!(git_in(&ws, &["log", "-1", "--format=%s"]), "Remove d");
    assert!(git_in(&ws, &["ls-files", "-s", "d"]).is_empty());
    assert!(!worktrees_of(&d).contains("shrink/d"));
    assert!(!git_in(&d, &["branch", "--list", "feat/x"]).is_empty());
    let pointer = d.join(".git/gitmaster/workspaces");
    assert!(
        !std::fs::read_to_string(pointer)
            .unwrap_or_default()
            .contains("shrink"),
        "the member's repository no longer lists the workspace"
    );

    // The same repository can join again.
    let again = harness.human(CommandKind::AddWorkspaceMembers {
        workspace: text(&ws),
        members: members_request(&[(&d, "feat/x")]),
    });
    assert!(done(&again), "{again:?}");
    assert!(ws.join("d").exists());
}

#[test]
fn removing_the_last_member_leaves_no_gitmodules() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "empty",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    let ws = scratch.ws("empty");
    let events = harness.human(CommandKind::RemoveWorkspaceMember {
        workspace: text(&ws),
        member: "c".into(),
        force: false,
    });
    assert!(done(&events), "{events:?}");
    assert!(!ws.join(".gitmodules").exists());
    assert_eq!(git_in(&ws, &["status", "--porcelain"]), "");
    assert!(git_in(&ws, &["ls-files"]).is_empty());
}

#[test]
fn a_dirty_member_is_removed_only_when_forced() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "messy",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    let ws = scratch.ws("messy");
    std::fs::write(ws.join("c/notes.txt"), "draft\n").unwrap();

    let inspected = harness.human(CommandKind::InspectMemberRemoval {
        workspace: text(&ws),
        member: "c".into(),
    });
    let blockers = inspected
        .iter()
        .find_map(|event| match event {
            Event::MemberRemovalInspected { blockers, .. } => Some(blockers.clone()),
            _ => None,
        })
        .expect("the inspection");
    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0].kind, RemovalBlockerKind::MemberDirty);
    assert!(blockers[0].overridable);

    let refused = harness.human(CommandKind::RemoveWorkspaceMember {
        workspace: text(&ws),
        member: "c".into(),
        force: false,
    });
    assert!(refusal(&refused).is_some_and(|reason| reason.contains("--force")));
    assert!(ws.join("c/notes.txt").exists());

    let forced = harness.human(CommandKind::RemoveWorkspaceMember {
        workspace: text(&ws),
        member: "c".into(),
        force: true,
    });
    assert!(done(&forced), "{forced:?}");
    assert!(!ws.join("c").exists());
}

#[test]
fn the_window_own_member_cannot_be_removed() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "inside",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    let ws = scratch.ws("inside");
    harness.open(&ws.join("c"));
    let events = harness.human(CommandKind::RemoveWorkspaceMember {
        workspace: text(&ws),
        member: "c".into(),
        force: true,
    });
    assert!(
        refusal(&events).is_some_and(|reason| reason.contains("current directory")),
        "{events:?}"
    );
    assert!(ws.join("c").exists());
}

#[test]
fn an_agent_cannot_change_the_members() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let d = scratch.repo("d");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    assert!(done(&harness.human(create(
        "guarded",
        &[(&c, "feat/c")],
        &scratch.root()
    ))));
    let ws = scratch.ws("guarded");
    let agent = Actor::Agent { name: "a".into() };
    let add = harness.run(
        agent.clone(),
        CommandKind::AddWorkspaceMembers {
            workspace: text(&ws),
            members: members_request(&[(&d, "feat/d")]),
        },
    );
    assert!(!done(&add));
    let remove = harness.run(
        agent,
        CommandKind::RemoveWorkspaceMember {
            workspace: text(&ws),
            member: "c".into(),
            force: true,
        },
    );
    assert!(!done(&remove));
    assert!(ws.join("c").exists() && !ws.join("d").exists());
}

#[test]
fn a_repository_is_cloned_into_the_clone_root_and_can_join() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let origin = scratch.repo_with_remote_branch("up", "feat/x");
    let url = text(&scratch.path().join("origin-up.git"));
    let clone_root = scratch.path().join("clones");
    let config = std::fs::read_to_string(&scratch.identity).unwrap();
    let config = format!(
        "{config}[gitmaster \"clone\"]\n\troot = {}\n",
        text(&clone_root)
    );
    std::fs::write(&scratch.identity, config).unwrap();
    let _ = origin;
    let mut harness = Harness::new(&scratch);
    harness.open(&c);

    let planned = harness.human(CommandKind::PlanClone { url: url.clone() });
    let target = planned
        .iter()
        .find_map(|event| match event {
            Event::ClonePlanned { path, problem, .. } => {
                assert_eq!(problem, &None);
                path.clone()
            }
            _ => None,
        })
        .expect("the planned folder");
    assert_eq!(target, text(&clone_root.join("origin-up")));

    let events = remote(
        &mut harness,
        CommandKind::CloneRepository {
            url: url.clone(),
            path: target.clone(),
        },
    );
    assert!(done(&events), "{events:?}");
    assert!(events.iter().any(|event| matches!(
        event,
        Event::RepositoryCloned { path, .. } if *path == target
    )));
    let cloned = PathBuf::from(&target);
    assert!(cloned.join(".git").is_dir());

    let again = harness.human(CommandKind::PlanClone { url: url.clone() });
    assert!(again.iter().any(|event| matches!(
        event,
        Event::ClonePlanned { path: None, problem: Some(problem), .. } if problem.contains("already exists")
    )));

    assert!(done(&harness.human(create(
        "cloned",
        &[(&c, "feat/x")],
        &scratch.root()
    ))));
    let ws = scratch.ws("cloned");
    let added = harness.human(CommandKind::AddWorkspaceMembers {
        workspace: text(&ws),
        members: members_request(&[(&cloned, "feat/x")]),
    });
    assert!(done(&added), "{added:?}");
    assert_eq!(
        git_in(
            &ws.join("origin-up"),
            &["rev-parse", "--abbrev-ref", "HEAD"]
        ),
        "feat/x"
    );
}

#[test]
fn a_clone_that_fails_creates_nothing_and_says_why() {
    let scratch = Scratch::new();
    let c = scratch.repo("c");
    let mut harness = Harness::new(&scratch);
    harness.open(&c);
    let target = scratch.path().join("clones/nothing");
    let events = remote(
        &mut harness,
        CommandKind::CloneRepository {
            url: text(&scratch.path().join("no-such.git")),
            path: text(&target),
        },
    );
    assert!(!done(&events));
    assert!(refusal(&events).is_some(), "{events:?}");
    assert!(!target.exists());
    let agent = harness.run(
        Actor::Agent { name: "a".into() },
        CommandKind::CloneRepository {
            url: "x".into(),
            path: text(&target),
        },
    );
    assert!(!done(&agent));
}
