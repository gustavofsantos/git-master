//! The native gate check of a push and a restack, and the Bypass of it (D212 (3), (4), D214,
//! D215), against real repositories in temporary directories. The remotes are local bare
//! repositories, so nothing needs the network. The user's gate and reviewer directories are
//! temporary ones too. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::{Engine, GitPort, reviewers};
use gm_schema::{
    Actor, Bypass, Command, CommandKind, Event, FindingInput, GateOp, GateOutcome, GatesStatus,
    OperationId, OperationState, RestackPlan, ReviewSeverity, Reviewer, ReviewerDirection,
    ReviewerSignal, ReviewerSource, ReviewerSubject, Verdict,
};

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `origin` (bare, with `main`), and `repo` on `feat`: one commit on `feat` that is not pushed.
struct Push {
    _dir: tempfile::TempDir,
    origin: PathBuf,
    repo: PathBuf,
    user: PathBuf,
}

fn push_fixture() -> Push {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    git(
        dir.path(),
        &["init", "-q", "--bare", "-b", "main", &s(&origin)],
    );
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(&repo, &["remote", "add", "origin", &s(&origin)]);
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    commit(&repo, "one\nthree\n", "feat change");
    let user = dir.path().join("user");
    std::fs::create_dir(&user).unwrap();
    Push {
        _dir: dir,
        origin,
        repo,
        user,
    }
}

/// Writes `text` to `f.txt` and commits it in `dir`.
fn commit(dir: &Path, text: &str, message: &str) {
    std::fs::write(dir.join("f.txt"), text).unwrap();
    git(dir, &["commit", "-q", "-am", message]);
}

/// `main` -> `a` -> `b`, each with one commit, then `main` moves on. The repository is on `main`,
/// as the restack tests leave it.
struct Stack {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    user: PathBuf,
}

fn stack_fixture() -> Stack {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let user = dir.path().join("user");
    std::fs::create_dir(&user).unwrap();
    let stack = Stack {
        _dir: dir,
        repo,
        user,
    };
    let mut session = Session::open(&stack.repo, &stack.user);
    session.human(CommandKind::CreateChildBranch {
        name: "a".into(),
        parent: "main".into(),
        checkout: false,
    });
    commit_on(&stack.repo, "a", "a.1", "a1");
    session.human(CommandKind::CreateChildBranch {
        name: "b".into(),
        parent: "a".into(),
        checkout: false,
    });
    commit_on(&stack.repo, "b", "b.1", "b1");
    commit_on(&stack.repo, "main", "m.1", "m1");
    stack
}

/// Commits `file` on `branch` and leaves `main` checked out.
fn commit_on(repo: &Path, branch: &str, file: &str, content: &str) {
    git(repo, &["checkout", "-q", branch]);
    std::fs::write(repo.join(file), content).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-q", "-m", &format!("{file}: {content}")]);
    git(repo, &["checkout", "-q", "main"]);
}

/// The tip of `branch` in the bare remote, empty when the remote has no such branch.
fn remote_tip(origin: &Path, branch: &str) -> String {
    git(
        origin,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            &format!("refs/heads/{branch}"),
        ],
    )
}

fn repo_gitmaster(dir: &Path) -> PathBuf {
    dir.join(".git").join("gitmaster")
}

fn write_repo_gates(dir: &Path, text: &str) {
    std::fs::create_dir_all(repo_gitmaster(dir)).unwrap();
    std::fs::write(repo_gitmaster(dir).join("gates.json"), text).unwrap();
}

/// A branch reviewer `security`, saved in the repository.
fn save_security(dir: &Path) {
    let reviewer = Reviewer {
        name: "security".to_owned(),
        description: "Looks for unchecked input.".to_owned(),
        instructions: "Read the change and flag unchecked input.".to_owned(),
        signal: ReviewerSignal::Level,
        direction: ReviewerDirection::LowerIsBetter,
        subject: ReviewerSubject::Branch,
        model: None,
        source: ReviewerSource::Repository,
        paths: Vec::new(),
        branches: Vec::new(),
    };
    reviewers::save(&reviewers::repo_dir(&dir.join(".git")), &reviewer).unwrap();
}

/// The agent's verdict of `security` on `feat` at its tip, with `findings`.
fn security_verdict(findings: Vec<FindingInput>) -> CommandKind {
    CommandKind::RecordVerdict {
        reviewer: "security".to_owned(),
        branch: "feat".to_owned(),
        head: "HEAD".to_owned(),
        value: "low".to_owned(),
        rationale: "Checked the change.".to_owned(),
        findings,
    }
}

fn agent() -> Actor {
    Actor::Agent {
        name: "agent".into(),
    }
}

/// An engine with a repository open, with the user's gate and reviewer directories set.
struct Session {
    engine: Engine,
    next: u64,
}

impl Session {
    fn open(repo: &Path, user: &Path) -> Self {
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_reviewer_user_dir(Some(reviewers::user_dir(user)));
        engine.set_gates_user_dir(Some(user.to_path_buf()));
        let mut session = Self { engine, next: 1 };
        let events = session.human(CommandKind::OpenRepo { path: s(repo) });
        assert_eq!(state(&events), OperationState::Done, "open: {events:?}");
        session
    }

    fn send(&mut self, actor: Actor, kind: CommandKind) -> Vec<Event> {
        self.next += 1;
        self.engine
            .handle(OperationId(self.next), &Command { actor, kind })
    }

    fn human(&mut self, kind: CommandKind) -> Vec<Event> {
        self.send(Actor::Human, kind)
    }

    fn agent(&mut self, kind: CommandKind) -> Vec<Event> {
        self.send(agent(), kind)
    }

    /// Handles a remote command and runs its job to the end, as the runner does.
    fn remote(&mut self, kind: CommandKind) -> Vec<Event> {
        let mut events = self.human(kind);
        for job in self.engine.take_remote_jobs() {
            let report = job.run();
            events.extend(self.engine.remote_report(report));
        }
        events
    }

    /// Plans the restack of `main` and applies it, as the window does.
    fn restack_main(&mut self) -> Vec<Event> {
        let plan = self.plan_main();
        self.human(CommandKind::ApplyRestack {
            branch: "main".into(),
            plan_id: plan.plan_id,
            skip: Vec::new(),
        })
    }

    fn plan_main(&mut self) -> RestackPlan {
        let events = self.human(CommandKind::PlanRestack {
            branch: "main".into(),
            skip: Vec::new(),
        });
        events
            .iter()
            .find_map(|event| match event {
                Event::RestackPlanned { plan, .. } => Some(plan.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no plan: {events:?}"))
    }
}

/// The state the operation ended in, and the detail of that event.
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

fn state(events: &[Event]) -> OperationState {
    ended(events).0
}

/// The details of every event that moved an operation to `wanted`, in order.
fn details_in(events: &[Event], wanted: OperationState) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::OperationChanged {
                state,
                detail: Some(detail),
                ..
            } if *state == wanted => Some(detail.clone()),
            _ => None,
        })
        .collect()
}

/// The gates the native check blocked, as the event lists them.
fn blocked(events: &[Event]) -> (GateOp, String, Vec<GateOutcome>, Vec<String>) {
    events
        .iter()
        .find_map(|event| match event {
            Event::GatesBlocked {
                operation,
                branch,
                gates,
                errors,
                ..
            } => Some((*operation, branch.clone(), gates.clone(), errors.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no GatesBlocked in {events:?}"))
}

fn no_block(events: &[Event]) -> bool {
    !events
        .iter()
        .any(|event| matches!(event, Event::GatesBlocked { .. }))
}

fn ids(events: &[Event]) -> Vec<OperationId> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::OperationChanged { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

const FRESH_SECURITY: &str =
    r#"{"id":"security","kind":"fresh_verdict","reviewers":["security"],"on":["push"]}"#;
const SCOPE: &str = r#"{"id":"scope","kind":"human_approval","on":["push"],"when":{"paths":["services/billing/**"]}}"#;
const APPROVED_PUSH: &str = r#"{"id":"approved","kind":"human_approval","on":["push"]}"#;
const APPROVED_RESTACK: &str = r#"{"id":"approved","kind":"human_approval","on":["restack"]}"#;

#[test]
fn a_push_without_a_verdict_is_vetoed_and_reaches_the_remote_once_the_tip_has_one() {
    let fx = push_fixture();
    save_security(&fx.repo);
    write_repo_gates(
        &fx.repo,
        &format!(r#"{{"gates":[{FRESH_SECURITY},{SCOPE}]}}"#),
    );
    let mut session = Session::open(&fx.repo, &fx.user);

    let events = session.remote(CommandKind::PushWithLease);
    assert_eq!(state(&events), OperationState::Vetoed, "{events:?}");
    let text = ended(&events).1.unwrap_or_default();
    assert!(
        text.starts_with("Blocked by gate security: security has no verdict for "),
        "{text}"
    );
    let (operation, branch, gates, errors) = blocked(&events);
    assert_eq!(operation, GateOp::Push);
    assert_eq!(branch, "feat");
    let names: Vec<&str> = gates.iter().map(|gate| gate.id.as_str()).collect();
    assert_eq!(names, vec!["security"], "a gate that passes is not listed");
    assert!(errors.is_empty());
    assert!(
        remote_tip(&fx.origin, "feat").is_empty(),
        "the push went out"
    );

    // The vetoed push left nothing behind: the next command runs.
    let status = session.human(CommandKind::RefreshStatus);
    assert_eq!(state(&status), OperationState::Done, "{status:?}");

    let verdict = session.agent(security_verdict(Vec::new()));
    assert_eq!(state(&verdict), OperationState::Done, "{verdict:?}");
    let pushed = session.remote(CommandKind::PushWithLease);
    assert_eq!(state(&pushed), OperationState::Done, "{pushed:?}");
    assert!(no_block(&pushed));
    assert_eq!(
        remote_tip(&fx.origin, "feat"),
        git(&fx.repo, &["rev-parse", "feat"])
    );
}

#[test]
fn a_human_approval_gate_blocks_until_a_person_approves_the_tip_and_a_new_commit_blocks_again() {
    let fx = push_fixture();
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{APPROVED_PUSH}]}}"#));
    let mut session = Session::open(&fx.repo, &fx.user);

    let first = session.remote(CommandKind::PushWithLease);
    assert_eq!(state(&first), OperationState::Vetoed);
    assert!(
        ended(&first)
            .1
            .unwrap_or_default()
            .starts_with("Blocked by gate approved: "),
    );
    assert!(remote_tip(&fx.origin, "feat").is_empty());

    let tip = git(&fx.repo, &["rev-parse", "feat"]);
    let approved = session.human(CommandKind::ApproveBranch {
        branch: "feat".into(),
        head: tip.clone(),
        note: None,
    });
    assert_eq!(state(&approved), OperationState::Done, "{approved:?}");
    let pushed = session.remote(CommandKind::PushWithLease);
    assert_eq!(state(&pushed), OperationState::Done, "{pushed:?}");
    assert_eq!(remote_tip(&fx.origin, "feat"), tip);

    commit(&fx.repo, "one\nfour\n", "a later commit");
    let again = session.remote(CommandKind::PushWithLease);
    assert_eq!(state(&again), OperationState::Vetoed, "{again:?}");
    assert!(
        ended(&again)
            .1
            .unwrap_or_default()
            .starts_with("Blocked by gate approved: ")
    );
    assert_eq!(
        remote_tip(&fx.origin, "feat"),
        tip,
        "the new commit went out"
    );
}

#[test]
fn an_open_high_finding_blocks_the_push_until_a_person_resolves_it() {
    let fx = push_fixture();
    save_security(&fx.repo);
    write_repo_gates(
        &fx.repo,
        r#"{"gates":[{"id":"no-high","kind":"findings","severity":"high","on":["push"]}]}"#,
    );
    let mut session = Session::open(&fx.repo, &fx.user);
    let finding = FindingInput {
        severity: ReviewSeverity::High,
        path: "f.txt".to_owned(),
        start_line: 2,
        end_line: 2,
        body: "unchecked input".to_owned(),
    };
    let recorded = session.agent(security_verdict(vec![finding]));
    assert_eq!(state(&recorded), OperationState::Done, "{recorded:?}");
    let comment = recorded
        .iter()
        .find_map(|event| match event {
            Event::VerdictRecorded { comment_ids, .. } => comment_ids.first().copied(),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no finding comment in {recorded:?}"));

    let blocked_push = session.remote(CommandKind::PushWithLease);
    assert_eq!(
        state(&blocked_push),
        OperationState::Vetoed,
        "{blocked_push:?}"
    );
    let text = ended(&blocked_push).1.unwrap_or_default();
    assert!(text.starts_with("Blocked by gate no-high: "), "{text}");
    assert!(text.contains("unresolved finding"), "{text}");
    assert!(remote_tip(&fx.origin, "feat").is_empty());

    let resolved = session.human(CommandKind::ResolveReviewComment { id: comment });
    assert_eq!(state(&resolved), OperationState::Done, "{resolved:?}");
    let pushed = session.remote(CommandKind::PushWithLease);
    assert_eq!(state(&pushed), OperationState::Done, "{pushed:?}");
}

#[test]
fn a_restack_gate_does_not_block_a_push_and_a_push_gate_does_not_block_a_restack() {
    let fx = push_fixture();
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{APPROVED_RESTACK}]}}"#));
    let mut session = Session::open(&fx.repo, &fx.user);
    let pushed = session.remote(CommandKind::PushWithLease);
    assert_eq!(state(&pushed), OperationState::Done, "{pushed:?}");
    assert!(no_block(&pushed));

    let stack = stack_fixture();
    write_repo_gates(&stack.repo, &format!(r#"{{"gates":[{APPROVED_PUSH}]}}"#));
    let mut session = Session::open(&stack.repo, &stack.user);
    let restacked = session.restack_main();
    assert_eq!(state(&restacked), OperationState::Done, "{restacked:?}");
    assert!(no_block(&restacked));
}

#[test]
fn a_restack_is_blocked_by_a_restack_gate_on_its_root_until_the_root_is_approved() {
    let stack = stack_fixture();
    write_repo_gates(&stack.repo, &format!(r#"{{"gates":[{APPROVED_RESTACK}]}}"#));
    let mut session = Session::open(&stack.repo, &stack.user);
    let before = git(&stack.repo, &["rev-parse", "a"]);

    let vetoed = session.restack_main();
    assert_eq!(state(&vetoed), OperationState::Vetoed, "{vetoed:?}");
    let (operation, branch, gates, _) = blocked(&vetoed);
    assert_eq!(operation, GateOp::Restack);
    assert_eq!(branch, "main");
    assert_eq!(gates.len(), 1);
    assert_eq!(gates[0].id, "approved");
    assert_eq!(git(&stack.repo, &["rev-parse", "a"]), before, "a moved");

    let tip = git(&stack.repo, &["rev-parse", "main"]);
    let approved = session.human(CommandKind::ApproveBranch {
        branch: "main".into(),
        head: tip,
        note: None,
    });
    assert_eq!(state(&approved), OperationState::Done, "{approved:?}");
    let restacked = session.restack_main();
    assert_eq!(state(&restacked), OperationState::Done, "{restacked:?}");
    assert_ne!(git(&stack.repo, &["rev-parse", "a"]), before);
}

#[test]
fn a_gate_whose_paths_the_change_does_not_touch_does_not_block_and_one_it_touches_does() {
    let fx = push_fixture();
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{SCOPE}]}}"#));
    let mut session = Session::open(&fx.repo, &fx.user);
    let untouched = session.remote(CommandKind::PushWithLease);
    assert_eq!(state(&untouched), OperationState::Done, "{untouched:?}");
    assert!(no_block(&untouched));

    write_repo_gates(
        &fx.repo,
        r#"{"gates":[{"id":"file","kind":"human_approval","on":["push"],"when":{"paths":["f.*"]}}]}"#,
    );
    let touched = session.remote(CommandKind::PushWithLease);
    assert_eq!(state(&touched), OperationState::Vetoed, "{touched:?}");
    assert_eq!(blocked(&touched).2[0].id, "file");
}

#[test]
fn a_malformed_gate_file_blocks_a_push_and_a_bypass_lets_it_through() {
    let fx = push_fixture();
    write_repo_gates(&fx.repo, r#"{"gates": ["#);
    let mut session = Session::open(&fx.repo, &fx.user);

    let vetoed = session.remote(CommandKind::PushWithLease);
    assert_eq!(state(&vetoed), OperationState::Vetoed, "{vetoed:?}");
    let text = ended(&vetoed).1.unwrap_or_default();
    assert!(text.starts_with("Gate file problem: "), "{text}");
    let (_, _, gates, errors) = blocked(&vetoed);
    assert!(gates.is_empty());
    assert_eq!(errors.len(), 1);
    assert!(remote_tip(&fx.origin, "feat").is_empty());

    let bypassed = session.remote(CommandKind::BypassGates {
        gates: vec!["gates-file".into()],
        reason: "the gate file is being fixed".into(),
        then: Box::new(CommandKind::PushWithLease),
    });
    assert_eq!(state(&bypassed), OperationState::Done, "{bypassed:?}");
    assert_eq!(
        remote_tip(&fx.origin, "feat"),
        git(&fx.repo, &["rev-parse", "feat"])
    );
}

#[test]
fn a_broken_reviewer_file_with_no_gate_blocks_nothing() {
    let fx = push_fixture();
    let reviewers_dir = reviewers::repo_dir(&fx.repo.join(".git"));
    std::fs::create_dir_all(&reviewers_dir).unwrap();
    std::fs::write(reviewers_dir.join("broken.md"), "not a reviewer\n").unwrap();
    let mut session = Session::open(&fx.repo, &fx.user);
    let pushed = session.remote(CommandKind::PushWithLease);
    assert_eq!(state(&pushed), OperationState::Done, "{pushed:?}");
    assert!(no_block(&pushed));
}

#[test]
fn with_no_gate_file_a_push_is_not_checked_at_all() {
    let fx = push_fixture();
    let mut session = Session::open(&fx.repo, &fx.user);
    let pushed = session.remote(CommandKind::PushWithLease);
    assert_eq!(state(&pushed), OperationState::Done, "{pushed:?}");
    assert!(no_block(&pushed));
    assert!(details_in(&pushed, OperationState::Vetoed).is_empty());
}

#[test]
fn a_bypass_runs_a_blocked_push_under_one_operation_and_records_the_gate_and_the_reason() {
    let fx = push_fixture();
    save_security(&fx.repo);
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{FRESH_SECURITY}]}}"#));
    let mut session = Session::open(&fx.repo, &fx.user);

    let events = session.remote(CommandKind::BypassGates {
        gates: vec!["security".into()],
        reason: "  hotfix for billing  ".into(),
        then: Box::new(CommandKind::PushWithLease),
    });
    assert_eq!(state(&events), OperationState::Done, "{events:?}");
    assert_eq!(
        remote_tip(&fx.origin, "feat"),
        git(&fx.repo, &["rev-parse", "feat"])
    );
    let gating = details_in(&events, OperationState::Gating);
    assert!(
        gating.contains(
            &"bypassed gates: security (errors: 0); reason: hotfix for billing".to_owned()
        ),
        "{gating:?}"
    );
    assert!(no_block(&events), "a bypass does not send GatesBlocked");
    let all = ids(&events);
    assert!(!all.is_empty());
    assert!(all.iter().all(|id| *id == all[0]), "{all:?}");
    // The log keeps the command as sent, so the gates and the reason are durable (D28).
    assert!(session.engine.log().iter().any(|entry| {
        entry.id == all[0]
            && matches!(
                &entry.kind,
                CommandKind::BypassGates { gates, reason, .. }
                    if gates == &["security".to_owned()] && reason == "  hotfix for billing  "
            )
    }));
}

#[test]
fn a_bypass_with_nothing_blocking_still_runs_the_command_and_says_so() {
    let fx = push_fixture();
    let mut session = Session::open(&fx.repo, &fx.user);
    let events = session.remote(CommandKind::BypassGates {
        gates: vec!["security".into()],
        reason: "ship it".into(),
        then: Box::new(CommandKind::PushWithLease),
    });
    assert_eq!(state(&events), OperationState::Done, "{events:?}");
    assert!(
        details_in(&events, OperationState::Gating)
            .contains(&"no gate was blocking; reason: ship it".to_owned())
    );
    assert!(!remote_tip(&fx.origin, "feat").is_empty());
}

#[test]
fn a_bypass_is_refused_without_a_usable_reason_or_for_another_command() {
    let fx = push_fixture();
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{APPROVED_PUSH}]}}"#));
    let mut session = Session::open(&fx.repo, &fx.user);

    for reason in ["   ".to_owned(), "x".repeat(501)] {
        let events = session.remote(CommandKind::BypassGates {
            gates: vec!["approved".into()],
            reason,
            then: Box::new(CommandKind::PushWithLease),
        });
        assert_eq!(state(&events), OperationState::Failed, "{events:?}");
        assert_eq!(
            ended(&events).1.as_deref(),
            Some("a bypass needs a reason of 1 to 500 characters")
        );
    }
    for then in [
        CommandKind::Fetch,
        CommandKind::BypassGates {
            gates: vec!["approved".into()],
            reason: "twice".into(),
            then: Box::new(CommandKind::PushWithLease),
        },
    ] {
        let events = session.remote(CommandKind::BypassGates {
            gates: vec!["approved".into()],
            reason: "ok".into(),
            then: Box::new(then),
        });
        assert_eq!(state(&events), OperationState::Failed, "{events:?}");
        assert_eq!(
            ended(&events).1.as_deref(),
            Some("only a push or a restack can be bypassed")
        );
    }
    assert!(remote_tip(&fx.origin, "feat").is_empty());
}

#[test]
fn an_agent_cannot_bypass_a_gate_and_nothing_runs() {
    let fx = push_fixture();
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{APPROVED_PUSH}]}}"#));
    let mut session = Session::open(&fx.repo, &fx.user);
    let events = session.send(
        agent(),
        CommandKind::BypassGates {
            gates: vec!["approved".into()],
            reason: "please".into(),
            then: Box::new(CommandKind::PushWithLease),
        },
    );
    assert_eq!(state(&events), OperationState::Failed, "{events:?}");
    assert_eq!(
        ended(&events).1.as_deref(),
        Some("only a human can bypass gates")
    );
    assert!(session.engine.take_remote_jobs().is_empty());
    assert!(remote_tip(&fx.origin, "feat").is_empty());
}

#[test]
fn a_bypass_runs_a_blocked_restack_and_records_the_gate() {
    let stack = stack_fixture();
    write_repo_gates(&stack.repo, &format!(r#"{{"gates":[{APPROVED_RESTACK}]}}"#));
    let mut session = Session::open(&stack.repo, &stack.user);
    let before = git(&stack.repo, &["rev-parse", "a"]);
    let plan = session.plan_main();
    let events = session.human(CommandKind::BypassGates {
        gates: vec!["approved".into()],
        reason: "restack before the review".into(),
        then: Box::new(CommandKind::ApplyRestack {
            branch: "main".into(),
            plan_id: plan.plan_id,
            skip: Vec::new(),
        }),
    });
    assert_eq!(state(&events), OperationState::Done, "{events:?}");
    assert!(details_in(&events, OperationState::Gating).contains(
        &"bypassed gates: approved (errors: 0); reason: restack before the review".to_owned()
    ),);
    assert_ne!(git(&stack.repo, &["rev-parse", "a"]), before);
}

/// Both gates of the push fixture block it: `approved` (a human approval) and `security` (a
/// verdict that is missing).
fn two_blocking_gates(repo: &Path) {
    save_security(repo);
    write_repo_gates(
        repo,
        &format!(r#"{{"gates":[{APPROVED_PUSH},{FRESH_SECURITY}]}}"#),
    );
}

#[test]
fn a_bypass_naming_only_some_of_the_blocking_gates_is_refused_and_shows_the_rest() {
    let fx = push_fixture();
    two_blocking_gates(&fx.repo);
    let mut session = Session::open(&fx.repo, &fx.user);

    let refused = session.remote(CommandKind::BypassGates {
        gates: vec!["approved".into()],
        reason: "ship it".into(),
        then: Box::new(CommandKind::PushWithLease),
    });
    assert_eq!(state(&refused), OperationState::Failed, "{refused:?}");
    assert_eq!(
        ended(&refused).1.as_deref(),
        Some("the gates changed since you looked: security now block; look again")
    );
    let (_, _, gates, errors) = blocked(&refused);
    let names: Vec<&str> = gates.iter().map(|gate| gate.id.as_str()).collect();
    assert_eq!(names, vec!["approved", "security"]);
    assert!(errors.is_empty());
    assert!(
        remote_tip(&fx.origin, "feat").is_empty(),
        "the push went out"
    );

    let all = session.remote(CommandKind::BypassGates {
        gates: vec!["approved".into(), "security".into()],
        reason: "ship it".into(),
        then: Box::new(CommandKind::PushWithLease),
    });
    assert_eq!(state(&all), OperationState::Done, "{all:?}");
    assert!(!remote_tip(&fx.origin, "feat").is_empty());
}

#[test]
fn a_bypass_with_no_gate_names_is_refused() {
    let fx = push_fixture();
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{APPROVED_PUSH}]}}"#));
    let mut session = Session::open(&fx.repo, &fx.user);
    let events = session.remote(CommandKind::BypassGates {
        gates: Vec::new(),
        reason: "ship it".into(),
        then: Box::new(CommandKind::PushWithLease),
    });
    assert_eq!(state(&events), OperationState::Failed, "{events:?}");
    assert!(
        ended(&events)
            .1
            .unwrap_or_default()
            .starts_with("a bypass names the gates it bypasses")
    );
    let malformed = session.remote(CommandKind::BypassGates {
        gates: vec!["Not A Gate".into()],
        reason: "ship it".into(),
        then: Box::new(CommandKind::PushWithLease),
    });
    assert_eq!(state(&malformed), OperationState::Failed, "{malformed:?}");
    assert!(remote_tip(&fx.origin, "feat").is_empty());
}

#[test]
fn a_bypass_naming_an_extra_gate_that_does_not_block_still_runs_and_records_it() {
    let fx = push_fixture();
    save_security(&fx.repo);
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{FRESH_SECURITY}]}}"#));
    let mut session = Session::open(&fx.repo, &fx.user);
    let events = session.remote(CommandKind::BypassGates {
        gates: vec!["security".into(), "ghost".into()],
        reason: "ship it".into(),
        then: Box::new(CommandKind::PushWithLease),
    });
    assert_eq!(state(&events), OperationState::Done, "{events:?}");
    assert!(
        details_in(&events, OperationState::Gating)
            .contains(&"bypassed gates: security, ghost (errors: 0); reason: ship it".to_owned()),
    );
    assert!(!remote_tip(&fx.origin, "feat").is_empty());
}

/// The bypasses of a `BypassesListed` event.
fn bypasses_of(events: &[Event]) -> Vec<Bypass> {
    events
        .iter()
        .find_map(|event| match event {
            Event::BypassesListed { bypasses, .. } => Some(bypasses.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no BypassesListed in {events:?}"))
}

/// The gate check of a `GatesStatusRead` event.
fn status_of(events: &[Event]) -> GatesStatus {
    events
        .iter()
        .find_map(|event| match event {
            Event::GatesStatusRead { status, .. } => Some(status.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no GatesStatusRead in {events:?}"))
}

/// The verdicts of a `VerdictsListed` event.
fn verdicts_of(events: &[Event]) -> Vec<Verdict> {
    events
        .iter()
        .find_map(|event| match event {
            Event::VerdictsListed { verdicts, .. } => Some(verdicts.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no VerdictsListed in {events:?}"))
}

/// The gate of `status` called `id`.
fn gate_named<'a>(status: &'a GatesStatus, id: &str) -> &'a GateOutcome {
    status
        .gates
        .iter()
        .find(|gate| gate.id == id)
        .unwrap_or_else(|| panic!("no gate {id} in {status:?}"))
}

/// The `findings` gate of high severity that the push fixture uses.
const NO_HIGH: &str = r#"{"id":"no-high","kind":"findings","severity":"high","on":["push"]}"#;

/// A high finding on line 2 of `f.txt`.
fn high_finding() -> FindingInput {
    FindingInput {
        severity: ReviewSeverity::High,
        path: "f.txt".to_owned(),
        start_line: 2,
        end_line: 2,
        body: "unchecked input".to_owned(),
    }
}

/// A bypass of the blocked push of `feat` that names `security`, with `reason`.
fn bypass_push(reason: &str) -> CommandKind {
    CommandKind::BypassGates {
        gates: vec!["security".into()],
        reason: reason.into(),
        then: Box::new(CommandKind::PushWithLease),
    }
}

/// A push fixture whose `security` gate blocks the push, with a reviewer file for `security`.
fn blocked_push_fixture() -> Push {
    let fx = push_fixture();
    save_security(&fx.repo);
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{FRESH_SECURITY}]}}"#));
    fx
}

#[test]
fn a_bypass_writes_an_audit_row_with_the_gates_the_person_named_and_the_reason() {
    let fx = blocked_push_fixture();
    let mut session = Session::open(&fx.repo, &fx.user);
    let head = git(&fx.repo, &["rev-parse", "feat"]);

    let bypassed = session.remote(bypass_push("  hotfix for billing  "));
    assert_eq!(state(&bypassed), OperationState::Done, "{bypassed:?}");

    let rows = bypasses_of(&session.human(CommandKind::ListBypasses {
        branch: None,
        limit: 0,
    }));
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row.branch, "feat");
    assert_eq!(row.head, head, "the audit names the tip the push was about");
    assert_eq!(row.operation, GateOp::Push);
    assert_eq!(row.gates, ["security"]);
    assert_eq!(row.reason, "hotfix for billing");
    assert_eq!(row.actor, "human");
    assert!(row.at > 0);
}

#[test]
fn the_audit_row_survives_closing_the_engine_and_opening_a_new_one() {
    let fx = blocked_push_fixture();
    let mut session = Session::open(&fx.repo, &fx.user);
    let bypassed = session.remote(bypass_push("hotfix for billing"));
    assert_eq!(state(&bypassed), OperationState::Done, "{bypassed:?}");
    drop(session);

    let mut again = Session::open(&fx.repo, &fx.user);
    let rows = bypasses_of(&again.human(CommandKind::ListBypasses {
        branch: None,
        limit: 0,
    }));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].reason, "hotfix for billing");
    assert_eq!(rows[0].gates, ["security"]);
}

#[test]
fn a_bypass_whose_audit_row_cannot_be_written_does_not_run_the_push() {
    let fx = blocked_push_fixture();
    let mut session = Session::open(&fx.repo, &fx.user);
    // Opening the store creates review.db; a trigger then refuses every audit insert, made from
    // another connection, so the write fails for the engine.
    session.human(CommandKind::ListBypasses {
        branch: None,
        limit: 0,
    });
    let database = fx.repo.join(".git").join("gitmaster").join("review.db");
    let other = rusqlite::Connection::open(&database).unwrap();
    other
        .execute_batch(
            "CREATE TRIGGER refuse_audit BEFORE INSERT ON bypasses
             BEGIN SELECT RAISE(ABORT, 'the audit is read only'); END;",
        )
        .unwrap();
    drop(other);

    let events = session.remote(bypass_push("hotfix for billing"));
    assert_eq!(state(&events), OperationState::Failed, "{events:?}");
    let detail = ended(&events).1.unwrap_or_default();
    assert!(
        detail.starts_with("the bypass is refused, its audit row was not written:"),
        "{detail}"
    );
    assert!(
        details_in(&events, OperationState::Gating).is_empty(),
        "{events:?}"
    );
    assert!(
        remote_tip(&fx.origin, "feat").is_empty(),
        "the push went out without its audit row"
    );
}

#[test]
fn list_bypasses_returns_the_newest_first_and_filters_by_branch_and_limit() {
    let fx = blocked_push_fixture();
    let mut session = Session::open(&fx.repo, &fx.user);
    let first = session.remote(bypass_push("first reason"));
    assert_eq!(state(&first), OperationState::Done, "{first:?}");
    let second = session.remote(bypass_push("second reason"));
    assert_eq!(state(&second), OperationState::Done, "{second:?}");

    let all = bypasses_of(&session.human(CommandKind::ListBypasses {
        branch: None,
        limit: 0,
    }));
    let reasons: Vec<&str> = all.iter().map(|row| row.reason.as_str()).collect();
    assert_eq!(reasons, ["second reason", "first reason"]);
    let on_feat = bypasses_of(&session.human(CommandKind::ListBypasses {
        branch: Some("feat".into()),
        limit: 0,
    }));
    assert_eq!(on_feat.len(), 2);
    let elsewhere = bypasses_of(&session.human(CommandKind::ListBypasses {
        branch: Some("other".into()),
        limit: 0,
    }));
    assert!(elsewhere.is_empty());
    let newest = bypasses_of(&session.human(CommandKind::ListBypasses {
        branch: None,
        limit: 1,
    }));
    assert_eq!(newest.len(), 1);
    assert_eq!(newest[0].reason, "second reason");
}

#[test]
fn a_refused_bypass_writes_no_audit_row() {
    let fx = blocked_push_fixture();
    let mut session = Session::open(&fx.repo, &fx.user);

    let empty = session.remote(bypass_push("   "));
    assert_eq!(state(&empty), OperationState::Failed, "{empty:?}");
    let agent_try = session.send(agent(), bypass_push("please"));
    assert_eq!(state(&agent_try), OperationState::Failed, "{agent_try:?}");
    write_repo_gates(
        &fx.repo,
        &format!(r#"{{"gates":[{APPROVED_PUSH},{FRESH_SECURITY}]}}"#),
    );
    let stale = session.remote(bypass_push("ship it"));
    assert_eq!(state(&stale), OperationState::Failed, "{stale:?}");
    assert!(
        ended(&stale).1.unwrap_or_default().contains("look again"),
        "{stale:?}"
    );

    let rows = bypasses_of(&session.human(CommandKind::ListBypasses {
        branch: None,
        limit: 0,
    }));
    assert!(rows.is_empty(), "{rows:?}");
}

#[test]
fn the_git_port_answers_ancestry_for_real_commits_and_refuses_a_missing_one() {
    let fx = push_fixture();
    let adapter = SubprocessGit::default();
    let main = git(&fx.repo, &["rev-parse", "main"]);
    let feat = git(&fx.repo, &["rev-parse", "feat"]);
    assert!(adapter.is_ancestor(&fx.repo, &main, &feat).unwrap());
    assert!(adapter.is_ancestor(&fx.repo, &feat, &feat).unwrap());
    assert!(!adapter.is_ancestor(&fx.repo, &feat, &main).unwrap());
    assert!(
        adapter
            .is_ancestor(&fx.repo, &"1".repeat(40), &feat)
            .is_err(),
        "a commit the repository does not have is an error, which fails closed"
    );
}

#[test]
fn a_finding_on_a_renamed_branch_still_blocks_the_findings_gate() {
    let fx = push_fixture();
    save_security(&fx.repo);
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{NO_HIGH}]}}"#));
    let mut session = Session::open(&fx.repo, &fx.user);
    let recorded = session.agent(security_verdict(vec![high_finding()]));
    assert_eq!(state(&recorded), OperationState::Done, "{recorded:?}");
    let before = status_of(&session.human(CommandKind::GatesStatus {
        branch: Some("feat".into()),
    }));
    assert!(!gate_named(&before, "no-high").passed, "{before:?}");

    git(&fx.repo, &["branch", "-m", "feat", "feat2"]);
    let after = status_of(&session.human(CommandKind::GatesStatus {
        branch: Some("feat2".into()),
    }));
    assert!(
        !gate_named(&after, "no-high").passed,
        "the finding escaped the rename: {after:?}"
    );
    assert!(!after.ready);
}

#[test]
fn a_finding_on_an_unrelated_branch_does_not_block_another_branch() {
    let fx = push_fixture();
    save_security(&fx.repo);
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{NO_HIGH}]}}"#));
    let mut session = Session::open(&fx.repo, &fx.user);
    // `other` starts from `main` as `feat` does, so neither tip contains the other's commit.
    git(&fx.repo, &["checkout", "-q", "-b", "other", "main"]);
    commit(&fx.repo, "one\ntwo\nthree\nfour\n", "other change");
    let recorded = session.agent(CommandKind::RecordVerdict {
        reviewer: "security".to_owned(),
        branch: "other".to_owned(),
        head: "HEAD".to_owned(),
        value: "low".to_owned(),
        rationale: "Checked the change.".to_owned(),
        findings: vec![high_finding()],
    });
    assert_eq!(state(&recorded), OperationState::Done, "{recorded:?}");

    let other = status_of(&session.human(CommandKind::GatesStatus {
        branch: Some("other".into()),
    }));
    assert!(!gate_named(&other, "no-high").passed, "{other:?}");
    let feat = status_of(&session.human(CommandKind::GatesStatus {
        branch: Some("feat".into()),
    }));
    assert!(gate_named(&feat, "no-high").passed, "{feat:?}");
}

#[test]
fn a_preset_name_is_refused_as_a_reviewer_until_a_file_defines_it_and_a_gate_says_why() {
    let fx = push_fixture();
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{FRESH_SECURITY}]}}"#));
    let mut session = Session::open(&fx.repo, &fx.user);

    let refused = session.agent(security_verdict(Vec::new()));
    assert_eq!(state(&refused), OperationState::Failed, "{refused:?}");
    assert_eq!(
        ended(&refused).1.as_deref(),
        Some(
            "security is a preset, not a reviewer yet: add it as a reviewer first (Reviewers Settings, or write .git/gitmaster/reviewers/security.md)"
        )
    );
    let listed = session.human(CommandKind::ListVerdicts {
        branch: Some("feat".into()),
        reviewer: None,
        limit: 0,
    });
    assert!(verdicts_of(&listed).is_empty(), "nothing was written");
    let status = status_of(&session.human(CommandKind::GatesStatus {
        branch: Some("feat".into()),
    }));
    assert_eq!(
        gate_named(&status, "security").reason.as_deref(),
        Some("reviewer security is not defined (it is a preset: add it as a reviewer)")
    );
    let listed = session.human(CommandKind::ListReviewers { name: None });
    let preset = listed.iter().find_map(|event| match event {
        Event::ReviewersListed { reviewers, .. } => reviewers
            .iter()
            .find(|reviewer| reviewer.name == "security")
            .cloned(),
        _ => None,
    });
    assert_eq!(
        preset.map(|reviewer| reviewer.source),
        Some(ReviewerSource::Preset)
    );

    save_security(&fx.repo);
    let accepted = session.agent(security_verdict(Vec::new()));
    assert_eq!(state(&accepted), OperationState::Done, "{accepted:?}");
}
