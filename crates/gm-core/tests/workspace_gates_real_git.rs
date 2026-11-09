//! Gates and reviewers of a workspace's members against real repositories in temporary
//! directories (D214 (5), D174, D180 (2)): a workspace with two members, each a worktree of its
//! own clone, and the definitions its repository keeps in `.git/gitmaster/`. A verdict, an
//! approval and a description land in the member's clone, and a member's own push is governed by
//! the workspace's gates. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::review_store::ReviewStore;
use gm_core::workspace_cli::run_with;
use gm_core::worktree_cli::Env;
use gm_core::{Engine, reviewers};
use gm_schema::{
    Actor, Command, CommandKind, Event, FindingInput, GateDef, GateKind, GateLayer, GateOp,
    GatesStatus, MemberGates, OperationId, OperationState, Proposal, ReviewSeverity,
    ReviewerDirection, ReviewerProposal, ReviewerSignal, ReviewerSubject, When,
};

fn git(dir: &Path, args: &[&str]) -> String {
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

fn tip(dir: &Path, branch: &str) -> String {
    git(dir, &["rev-parse", branch])
}

/// The common Git directory of `dir`, absolute.
fn common_of(dir: &Path) -> PathBuf {
    PathBuf::from(git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ))
}

/// A scratch folder with the workspace commands' Git configuration and the paths of one fixture.
struct Fixture {
    _dir: tempfile::TempDir,
    base: PathBuf,
    identity: PathBuf,
    user: PathBuf,
    /// The main checkouts of the two clones, on `main`.
    api: PathBuf,
    web: PathBuf,
    /// The workspace `pay`, and its members, the worktrees on `feat`.
    ws: PathBuf,
    member_api: PathBuf,
    member_web: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let identity = base.join("identity.gitconfig");
        std::fs::write(
            &identity,
            "[user]\n\tname = t\n\temail = t@t\n[commit]\n\tgpgsign = false\n",
        )
        .unwrap();
        let user = base.join("user");
        std::fs::create_dir(&user).unwrap();
        let api = clone(&base, "api");
        let web = clone(&base, "web");
        let mut fixture = Self {
            _dir: dir,
            base,
            identity,
            user,
            member_api: PathBuf::new(),
            member_web: PathBuf::new(),
            ws: PathBuf::new(),
            api,
            web,
        };
        fixture.ws = fixture.create("pay");
        fixture.member_api = fixture.add(&fixture.ws.clone(), &fixture.api.clone(), "feat");
        fixture.member_web = fixture.add(&fixture.ws.clone(), &fixture.web.clone(), "feat");
        fixture
    }

    fn env(&self, cwd: &Path) -> Env {
        Env {
            cwd: cwd.to_path_buf(),
            home: Some(self.base.join("home")),
            worktrees_home: None,
        }
    }

    /// Runs `gitmaster workspace <words>` in `cwd`; returns status, stdout and stderr.
    fn run_in(&self, cwd: &Path, words: &[&str]) -> (i32, String, String) {
        let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        let git = SubprocessGit {
            global_config: Some(self.identity.clone()),
            ..SubprocessGit::default()
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with(
            &words,
            &self.env(cwd),
            &git,
            Some(&self.identity),
            &mut "".as_bytes(),
            &mut out,
            &mut err,
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    /// `create <name>` under the fixture's root, asserting success.
    fn create(&self, name: &str) -> PathBuf {
        let root = self.base.join("workspaces");
        let (code, out, err) = self.run_in(
            &self.base,
            &["create", name, "--root", root.to_str().unwrap()],
        );
        assert_eq!(code, 0, "create failed: {err}");
        let dir = root.join(name);
        assert_eq!(out, format!("{}\n", dir.display()));
        dir
    }

    /// `add <repo> <branch>` into `ws`, asserting success; returns the member folder.
    fn add(&self, ws: &Path, repo: &Path, branch: &str) -> PathBuf {
        let (code, out, err) = self.run_in(
            &self.base,
            &[
                "add",
                repo.to_str().unwrap(),
                branch,
                "--workspace",
                ws.to_str().unwrap(),
            ],
        );
        assert_eq!(code, 0, "add failed: {err}");
        PathBuf::from(out.trim_end())
    }

    /// The gate directory of the workspace `ws`: `.git/gitmaster`.
    fn ws_gitmaster(ws: &Path) -> PathBuf {
        ws.join(".git").join("gitmaster")
    }

    fn write_ws_gates(ws: &Path, text: &str) {
        let dir = Self::ws_gitmaster(ws);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("gates.json"), text).unwrap();
    }

    /// The security reviewer of the workspace, for member `api` only.
    fn write_security(ws: &Path) {
        let dir = Self::ws_gitmaster(ws).join("reviewers");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("security.md"),
            "---\ndescription: Checks unchecked input.\nsignal: pass\nsubject: branch\nmembers: api\n---\nRead the change and flag unchecked input.\n",
        )
        .unwrap();
    }

    /// The workspace gate `security-review`: a fresh verdict of `security`, for member `api`.
    fn write_security_gate(ws: &Path) {
        Self::write_ws_gates(
            ws,
            r#"{"gates": [{"id": "security-review", "kind": "fresh_verdict", "on": ["push"], "reviewers": ["security"], "members": ["api"]}]}"#,
        );
    }
}

/// A clone `base/<name>` on `main` with `f.txt`, pushed to a bare origin, and a `feat` branch
/// with one change to `f.txt`, left unchecked-out for a member to take.
fn clone(base: &Path, name: &str) -> PathBuf {
    let origin = base.join(format!("{name}.git"));
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare", "-b", "main"]);
    let repo = base.join(name);
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("f.txt"), "one\nthree\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "feat change"]);
    git(&repo, &["checkout", "-q", "main"]);
    repo
}

/// An engine with `dir` open and the user's definitions in `user`.
struct Session {
    engine: Engine,
    next: u64,
}

impl Session {
    fn open(dir: &Path, user: &Path) -> Self {
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_reviewer_user_dir(Some(reviewers::user_dir(user)));
        engine.set_gates_user_dir(Some(user.to_path_buf()));
        let mut session = Self { engine, next: 1 };
        let events = session.human(CommandKind::OpenRepo {
            path: dir.to_string_lossy().into_owned(),
        });
        assert!(done(&events), "open failed: {}", failure(&events));
        session
    }

    fn run(&mut self, actor: Actor, kind: CommandKind) -> Vec<Event> {
        self.next += 1;
        self.engine
            .handle(OperationId(self.next), &Command { actor, kind })
    }

    fn human(&mut self, kind: CommandKind) -> Vec<Event> {
        self.run(Actor::Human, kind)
    }

    fn agent(&mut self, kind: CommandKind) -> Vec<Event> {
        self.run(
            Actor::Agent {
                name: "test-agent".to_owned(),
            },
            kind,
        )
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

    /// The gate check of `branch` for `member` (the workspace's aggregate when `None`).
    fn gates(&mut self, branch: Option<&str>, member: Option<&str>) -> Vec<Event> {
        self.human(CommandKind::GatesStatus {
            branch: branch.map(str::to_owned),
            member: member.map(str::to_owned),
        })
    }
}

fn done(events: &[Event]) -> bool {
    state_of(events) == Some(OperationState::Done)
}

fn state_of(events: &[Event]) -> Option<OperationState> {
    events.iter().rev().find_map(|event| match event {
        Event::OperationChanged { state, .. } if state.is_terminal() => Some(*state),
        _ => None,
    })
}

/// The detail of the terminal state an operation ended in (a veto's text, or a failure's), or an
/// empty string when it has none.
fn ended_detail(events: &[Event]) -> String {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::OperationChanged { state, detail, .. } if state.is_terminal() => {
                Some(detail.clone().unwrap_or_default())
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// The reason an operation failed, or an empty string when it did not.
fn failure(events: &[Event]) -> String {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::OperationChanged {
                state: OperationState::Failed,
                detail,
                ..
            } => detail.clone(),
            _ => None,
        })
        .unwrap_or_default()
}

/// The status of a single member or a repository, from its `GatesStatusRead`.
fn status_of(events: &[Event]) -> GatesStatus {
    events
        .iter()
        .find_map(|event| match event {
            Event::GatesStatusRead { status, .. } => Some(status.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no gate status in {events:?}"))
}

/// The aggregate of a workspace: its readiness and every member's row.
fn aggregate_of(events: &[Event]) -> (bool, Vec<MemberGates>) {
    events
        .iter()
        .find_map(|event| match event {
            Event::WorkspaceGatesRead { ready, members, .. } => Some((*ready, members.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no workspace gates in {events:?}"))
}

fn member_row<'a>(members: &'a [MemberGates], name: &str) -> &'a MemberGates {
    members
        .iter()
        .find(|row| row.member == name)
        .unwrap_or_else(|| panic!("no member {name} in {members:?}"))
}

fn gate<'a>(status: &'a GatesStatus, id: &str) -> &'a gm_schema::GateOutcome {
    status
        .gates
        .iter()
        .find(|gate| gate.id == id)
        .unwrap_or_else(|| panic!("no gate {id} in {:?}", status.gates))
}

fn security_finding(line: u32) -> FindingInput {
    FindingInput {
        severity: ReviewSeverity::Medium,
        path: "f.txt".to_owned(),
        start_line: line,
        end_line: line,
        body: "Unchecked input reaches the query.".to_owned(),
        suggested_fix: None,
    }
}

/// Records a passing `security` verdict for member `api` on its tip, as an agent does.
fn record_security(session: &mut Session, member: Option<&str>, head: &str) -> Vec<Event> {
    session.agent(CommandKind::RecordVerdict {
        reviewer: "security".into(),
        branch: "feat".into(),
        head: head.into(),
        value: "pass".into(),
        rationale: "Read the change; the input is checked.".into(),
        findings: vec![security_finding(1)],
        member: member.map(str::to_owned),
    })
}

#[test]
fn a_workspace_gate_applies_to_its_member_and_not_to_the_other() {
    let fx = Fixture::new();
    Fixture::write_security(&fx.ws);
    Fixture::write_security_gate(&fx.ws);
    let mut ws = Session::open(&fx.ws, &fx.user);

    let api = status_of(&ws.gates(Some("feat"), Some("api")));
    let gate_api = gate(&api, "security-review");
    assert!(gate_api.applicable, "{gate_api:?}");
    assert!(!gate_api.passed, "{gate_api:?}");
    assert_eq!(gate_api.layer, GateLayer::Workspace);
    assert_eq!(api.missing.len(), 1, "{api:?}");
    assert_eq!(api.missing[0].reviewer, "security");
    assert!(!api.ready);

    let web = status_of(&ws.gates(Some("feat"), Some("web")));
    let gate_web = gate(&web, "security-review");
    assert!(!gate_web.applicable, "{gate_web:?}");
    assert_eq!(
        gate_web.not_applicable.as_deref(),
        Some("it is for the members api of a workspace")
    );
    assert!(web.ready, "{web:?}");
}

#[test]
fn a_verdict_for_a_member_lands_in_its_clone_and_not_in_the_workspace() {
    let fx = Fixture::new();
    Fixture::write_security(&fx.ws);
    Fixture::write_security_gate(&fx.ws);
    let mut ws = Session::open(&fx.ws, &fx.user);
    let head = tip(&fx.member_api, "feat");

    let recorded = ws.agent(CommandKind::RecordVerdict {
        reviewer: "security".into(),
        branch: "feat".into(),
        head: head.clone(),
        value: "pass".into(),
        rationale: "Read the change; the input is checked.".into(),
        findings: vec![security_finding(1)],
        member: Some("api".into()),
    });
    assert!(done(&recorded), "{}", failure(&recorded));
    assert!(status_of(&ws.gates(Some("feat"), Some("api"))).ready);

    // The clone alone, opened at its own main checkout, keeps the verdict.
    let mut clone = Session::open(&fx.api, &fx.user);
    let listed = clone.human(CommandKind::ListVerdicts {
        branch: Some("feat".into()),
        reviewer: None,
        limit: 0,
        member: None,
    });
    let verdicts = listed
        .iter()
        .find_map(|event| match event {
            Event::VerdictsListed { verdicts, .. } => Some(verdicts.clone()),
            _ => None,
        })
        .expect("verdicts");
    assert_eq!(verdicts.len(), 1, "{verdicts:?}");
    assert_eq!(verdicts[0].reviewer, "security");
    assert_eq!(verdicts[0].head, head);

    // The workspace's own store holds no verdict.
    let store = ReviewStore::open(&Fixture::ws_gitmaster(&fx.ws).join("review.db")).unwrap();
    assert!(store.list_verdicts(None, None, 10).unwrap().is_empty());
}

#[test]
fn a_members_own_push_is_blocked_by_the_workspace_gate_until_its_verdict_is_recorded() {
    let fx = Fixture::new();
    Fixture::write_security(&fx.ws);
    Fixture::write_security_gate(&fx.ws);
    // The member's own window: its worktree in the workspace folder, on its branch.
    let mut own = Session::open(&fx.member_api, &fx.user);

    let vetoed = own.remote(CommandKind::PushWithLease);
    assert_eq!(
        state_of(&vetoed),
        Some(OperationState::Vetoed),
        "{vetoed:?}"
    );
    let text = ended_detail(&vetoed);
    assert!(text.contains("Blocked by gate security-review"), "{text}");

    let head = tip(&fx.member_api, "feat");
    let recorded = record_security(&mut own, None, &head);
    assert!(done(&recorded), "{}", failure(&recorded));

    let pushed = own.remote(CommandKind::PushWithLease);
    assert!(done(&pushed), "{}", failure(&pushed));
    assert_eq!(
        git(&fx.api, &["ls-remote", "origin", "refs/heads/feat"])
            .split_whitespace()
            .next(),
        Some(head.as_str())
    );
}

#[test]
fn the_aggregate_is_ready_only_when_every_member_is_and_lists_both() {
    let fx = Fixture::new();
    Fixture::write_security(&fx.ws);
    Fixture::write_security_gate(&fx.ws);
    let mut ws = Session::open(&fx.ws, &fx.user);

    let (ready, members) = aggregate_of(&ws.gates(None, None));
    assert!(!ready);
    let names: Vec<&str> = members.iter().map(|row| row.member.as_str()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names.contains(&"api") && names.contains(&"web"),
        "{names:?}"
    );
    assert!(!member_row(&members, "api").status.as_ref().unwrap().ready);
    assert!(member_row(&members, "web").status.as_ref().unwrap().ready);
    assert!(member_row(&members, "web").error.is_none());

    let head = tip(&fx.member_api, "feat");
    assert!(done(&record_security(&mut ws, Some("api"), &head)));
    let (ready, members) = aggregate_of(&ws.gates(None, None));
    assert!(ready, "{members:?}");
    assert_eq!(members.len(), 2);
}

#[test]
fn a_detached_member_makes_the_aggregate_not_ready_with_an_error() {
    let fx = Fixture::new();
    git(&fx.member_web, &["checkout", "-q", "--detach"]);
    let mut ws = Session::open(&fx.ws, &fx.user);

    let (ready, members) = aggregate_of(&ws.gates(None, None));
    assert!(!ready);
    let web = member_row(&members, "web");
    assert_eq!(web.branch, None);
    assert!(web.status.is_none());
    assert!(
        web.error
            .as_deref()
            .is_some_and(|error| error.contains("detached")),
        "{web:?}"
    );
    assert!(member_row(&members, "api").error.is_none());
}

#[test]
fn a_write_or_a_listed_read_without_a_member_in_a_workspace_is_refused() {
    let fx = Fixture::new();
    let mut ws = Session::open(&fx.ws, &fx.user);
    let head = tip(&fx.member_api, "feat");

    let verdict = record_security(&mut ws, None, &head);
    assert_eq!(state_of(&verdict), Some(OperationState::Failed));
    assert_eq!(failure(&verdict), "this is a workspace: name the member");

    let approve = ws.human(CommandKind::ApproveBranch {
        branch: "feat".into(),
        head: head.clone(),
        note: None,
        member: None,
    });
    assert_eq!(failure(&approve), "this is a workspace: name the member");

    let listed = ws.human(CommandKind::ListVerdicts {
        branch: None,
        reviewer: None,
        limit: 0,
        member: None,
    });
    assert_eq!(failure(&listed), "this is a workspace: name the member");

    // A branch named without a member is no aggregate: it is refused rather than dropped.
    let named = ws.gates(Some("feat"), None);
    assert!(failure(&named).starts_with("this is a workspace: name the member whose branch feat"));
}

#[test]
fn a_member_outside_a_workspace_and_an_unknown_member_are_refused() {
    let fx = Fixture::new();
    let mut clone = Session::open(&fx.api, &fx.user);
    let outside = clone.gates(Some("feat"), Some("api"));
    assert_eq!(state_of(&outside), Some(OperationState::Failed));
    assert_eq!(failure(&outside), "this repository is not a workspace");

    let mut ws = Session::open(&fx.ws, &fx.user);
    let unknown = ws.gates(Some("feat"), Some("nope"));
    let text = failure(&unknown);
    assert!(
        text.starts_with("there is no member nope in this workspace; members are: "),
        "{text}"
    );
    assert!(text.contains("api") && text.contains("web"), "{text}");
}

#[test]
fn approval_and_intent_of_a_member_are_kept_in_its_clone() {
    let fx = Fixture::new();
    let head = tip(&fx.member_api, "feat");
    let mut ws = Session::open(&fx.ws, &fx.user);

    let approved = ws.human(CommandKind::ApproveBranch {
        branch: "feat".into(),
        head: head.clone(),
        note: Some("looks right".into()),
        member: Some("api".into()),
    });
    assert!(done(&approved), "{}", failure(&approved));
    let mut clone = Session::open(&fx.api, &fx.user);
    let read = clone.human(CommandKind::ReadApproval {
        branch: "feat".into(),
        member: None,
    });
    let approval = read
        .iter()
        .find_map(|event| match event {
            Event::ApprovalRead { approval, .. } => approval.clone(),
            _ => None,
        })
        .expect("an approval in the clone");
    assert_eq!(approval.head, head);
    assert_eq!(approval.note.as_deref(), Some("looks right"));

    let described = ws.human(CommandKind::SetBranchDescription {
        branch: "feat".into(),
        text: "Adds the security gate.".into(),
        member: Some("api".into()),
    });
    assert!(done(&described), "{}", failure(&described));
    let text = clone.human(CommandKind::ReadBranchDescription {
        branch: "feat".into(),
        member: None,
    });
    assert!(
        text.iter().any(|event| matches!(
            event,
            Event::BranchDescriptionRead { text: Some(text), .. } if text == "Adds the security gate."
        )),
        "{text:?}"
    );

    let revoked = ws.human(CommandKind::RevokeApproval {
        branch: "feat".into(),
        member: Some("api".into()),
    });
    assert!(done(&revoked), "{}", failure(&revoked));
    let after = clone.human(CommandKind::ReadApproval {
        branch: "feat".into(),
        member: None,
    });
    assert!(
        after
            .iter()
            .any(|event| matches!(event, Event::ApprovalRead { approval: None, .. }))
    );

    // The workspace's own store never saw the approval.
    let store = ReviewStore::open(&Fixture::ws_gitmaster(&fx.ws).join("review.db")).unwrap();
    assert!(store.approval("feat").unwrap().is_none());
}

#[test]
fn a_user_gate_with_the_id_of_a_workspace_gate_loses_to_the_workspace_one() {
    let fx = Fixture::new();
    std::fs::write(
        fx.user.join("gates.json"),
        r#"{"gates": [{"id": "security-review", "kind": "up_to_date", "on": ["push"]}]}"#,
    )
    .unwrap();
    Fixture::write_security(&fx.ws);
    Fixture::write_security_gate(&fx.ws);
    let mut ws = Session::open(&fx.ws, &fx.user);

    let status = status_of(&ws.gates(Some("feat"), Some("api")));
    let winner = gate(&status, "security-review");
    assert_eq!(winner.layer, GateLayer::Workspace, "{winner:?}");
    assert_eq!(winner.kind, "fresh_verdict");
    assert_eq!(
        status
            .gates
            .iter()
            .filter(|g| g.id == "security-review")
            .count(),
        1
    );
}

#[test]
fn a_gate_id_defined_by_two_workspaces_of_one_clone_fails_closed() {
    let fx = Fixture::new();
    // The clone api is a member of a second workspace, on another branch of its own.
    git(&fx.api, &["branch", "feat2"]);
    let second = fx.create("ship");
    Fixture::write_ws_gates(
        &fx.ws,
        r#"{"gates": [{"id": "shared", "kind": "up_to_date", "on": ["push"]}]}"#,
    );
    Fixture::write_ws_gates(
        &second,
        r#"{"gates": [{"id": "shared", "kind": "up_to_date", "on": ["push"]}]}"#,
    );
    fx.add(&second, &fx.api, "feat2");
    let mut clone = Session::open(&fx.api, &fx.user);

    let status = status_of(&clone.gates(Some("feat"), None));
    assert!(!status.ready, "{status:?}");
    assert!(
        status
            .errors
            .iter()
            .any(|error| error.contains("the gate shared is defined in two workspaces")),
        "{:?}",
        status.errors
    );
    assert_eq!(common_of(&fx.api), common_of(&fx.member_api));
}

#[test]
fn accepting_a_proposal_into_the_workspace_writes_its_file_and_is_refused_elsewhere() {
    let fx = Fixture::new();
    let mut ws = Session::open(&fx.ws, &fx.user);

    let gate = GateDef {
        id: "api-up".into(),
        kind: GateKind::UpToDate,
        on: vec![GateOp::Push],
        when: When::default(),
        members: vec!["api".into()],
    };
    let proposed = ws.agent(CommandKind::ProposeGate {
        gate: Box::new(gate),
        reason: "api must stay current with its parent".into(),
    });
    let proposal = proposal_of(&proposed);
    let accepted = ws.human(CommandKind::AcceptProposal {
        id: proposal.id,
        layer: GateLayer::Workspace,
        edited_reviewer: None,
        edited_gate: None,
    });
    assert!(done(&accepted), "{}", failure(&accepted));
    assert_eq!(
        proposal_of(&accepted).status,
        gm_schema::ProposalStatus::Accepted
    );
    let written =
        std::fs::read_to_string(Fixture::ws_gitmaster(&fx.ws).join("gates.json")).unwrap();
    assert!(written.contains("api-up"), "{written}");

    let mut clone = Session::open(&fx.api, &fx.user);
    let refused = clone.human(CommandKind::AcceptProposal {
        id: proposal.id,
        layer: GateLayer::Workspace,
        edited_reviewer: None,
        edited_gate: None,
    });
    assert_eq!(failure(&refused), "this is not a workspace");
}

#[test]
fn members_are_refused_outside_the_workspace_layer_for_a_reviewer_and_a_gate() {
    let fx = Fixture::new();
    let mut ws = Session::open(&fx.ws, &fx.user);

    let proposed = ws.agent(CommandKind::ProposeReviewer {
        reviewer: Box::new(ReviewerProposal {
            name: "billing".into(),
            description: "Knows the billing rules.".into(),
            instructions: "Check the rounding of invoices.".into(),
            signal: ReviewerSignal::Pass,
            direction: ReviewerDirection::LowerIsBetter,
            subject: ReviewerSubject::Branch,
            model: None,
            paths: Vec::new(),
            branches: Vec::new(),
            members: vec!["api".into()],
        }),
        reason: "the billing rules are not in the code".into(),
    });
    let reviewer = proposal_of(&proposed);
    for layer in [GateLayer::Repository, GateLayer::User] {
        let refused = ws.human(CommandKind::AcceptProposal {
            id: reviewer.id,
            layer,
            edited_reviewer: None,
            edited_gate: None,
        });
        assert_eq!(
            failure(&refused),
            "members only apply in a workspace definition",
            "{layer:?}"
        );
    }
    let accepted = ws.human(CommandKind::AcceptProposal {
        id: reviewer.id,
        layer: GateLayer::Workspace,
        edited_reviewer: None,
        edited_gate: None,
    });
    assert!(done(&accepted), "{}", failure(&accepted));
    let written =
        std::fs::read_to_string(reviewers::repo_dir(&fx.ws.join(".git")).join("billing.md"))
            .unwrap();
    assert!(written.contains("members: api"), "{written}");
}

/// The proposal a proposal event answers.
fn proposal_of(events: &[Event]) -> Proposal {
    events
        .iter()
        .find_map(|event| match event {
            Event::ProposalDecided { proposal, .. } => Some((**proposal).clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no proposal decided in {events:?}"))
}
