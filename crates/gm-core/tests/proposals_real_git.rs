//! Proposed reviewers and gates (D214 (3)) against real repositories in temporary directories. An
//! agent proposes; the reviewer list and the gate check never see a proposal, and only a person's
//! accept writes a definition. The user's reviewer and gate directories are temporary ones, and
//! each test removes its directories when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::{Engine, reviewers};
use gm_schema::{
    Actor, Command, CommandKind, Event, GateDef, GateKind, GateLayer, GateOp, GateOutcome,
    GatesStatus, OperationId, OperationState, Proposal, ProposalStatus, Reviewer,
    ReviewerDirection, ReviewerProposal, ReviewerSignal, ReviewerSource, ReviewerSubject, When,
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

/// A repository on `feat`, one commit ahead of `origin`'s `main`, which is a local bare remote.
struct Fixture {
    _dir: tempfile::TempDir,
    origin: PathBuf,
    repo: PathBuf,
}

fn fixture() -> Fixture {
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
    Fixture {
        _dir: dir,
        origin,
        repo,
    }
}

fn commit(dir: &Path, text: &str, message: &str) {
    std::fs::write(dir.join("f.txt"), text).unwrap();
    git(dir, &["commit", "-q", "-am", message]);
}

/// A repository on `feat`, the proposals' home; the user's directories are under `user`.
fn repo_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    commit(&repo, "one\nthree\n", "feat change");
    (dir, repo)
}

/// The repository's `.git/gitmaster` directory, where its reviewers and gates live.
fn repo_gitmaster(dir: &Path) -> PathBuf {
    dir.join(".git").join("gitmaster")
}

fn write_repo_gates(dir: &Path, text: &str) {
    std::fs::create_dir_all(repo_gitmaster(dir)).unwrap();
    std::fs::write(repo_gitmaster(dir).join("gates.json"), text).unwrap();
}

/// An engine with `dir` open and the user's reviewers and gates in `user`.
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
        let events = session.human(CommandKind::OpenRepo { path: s(dir) });
        assert!(done(&events), "open failed: {events:?}");
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

    /// A push or restack: runs its remote job to the end, as the runner does.
    fn remote(&mut self, kind: CommandKind) -> Vec<Event> {
        let mut events = self.human(kind);
        for job in self.engine.take_remote_jobs() {
            let report = job.run();
            events.extend(self.engine.remote_report(report));
        }
        events
    }

    fn proposals(&mut self, status: Option<ProposalStatus>) -> Vec<Proposal> {
        let events = self.human(CommandKind::ListProposals { status, limit: 0 });
        proposals_listed(&events)
    }

    fn gates_status(&mut self) -> GatesStatus {
        let events = self.human(CommandKind::GatesStatus {
            branch: None,
            member: None,
        });
        events
            .iter()
            .find_map(|event| match event {
                Event::GatesStatusRead { status, .. } => Some(status.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no GatesStatusRead in {events:?}"))
    }

    fn reviewer_names(&mut self) -> Vec<(String, ReviewerSource)> {
        let events = self.human(CommandKind::ListReviewers { name: None });
        reviewers_listed(&events)
            .into_iter()
            .map(|reviewer| (reviewer.name, reviewer.source))
            .collect()
    }
}

fn terminal(events: &[Event]) -> (OperationState, Option<String>) {
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

fn done(events: &[Event]) -> bool {
    terminal(events).0 == OperationState::Done
}

/// The reason an operation failed or was vetoed, or an empty text when it did not.
fn refusal(events: &[Event]) -> String {
    let (state, detail) = terminal(events);
    assert_ne!(
        state,
        OperationState::Done,
        "expected a refusal: {events:?}"
    );
    detail.unwrap_or_default()
}

fn proposal_of(events: &[Event]) -> Proposal {
    events
        .iter()
        .find_map(|event| match event {
            Event::ProposalDecided { proposal, .. } => Some(*proposal.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no ProposalDecided in {events:?}"))
}

fn proposals_listed(events: &[Event]) -> Vec<Proposal> {
    events
        .iter()
        .find_map(|event| match event {
            Event::ProposalsListed { proposals, .. } => Some(proposals.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no ProposalsListed in {events:?}"))
}

fn reviewers_listed(events: &[Event]) -> Vec<Reviewer> {
    events
        .iter()
        .find_map(|event| match event {
            Event::ReviewersListed { reviewers, .. } => Some(reviewers.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no ReviewersListed in {events:?}"))
}

fn blocked(events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::GatesBlocked { .. }))
}

fn gate_named<'a>(status: &'a GatesStatus, id: &str) -> &'a GateOutcome {
    status
        .gates
        .iter()
        .find(|gate| gate.id == id)
        .unwrap_or_else(|| panic!("no gate {id} in {:?}", status.gates))
}

fn reviewer_proposal(name: &str, description: &str) -> ReviewerProposal {
    ReviewerProposal {
        name: name.to_owned(),
        description: description.to_owned(),
        instructions: "Look for the billing rules that the change breaks.".to_owned(),
        signal: ReviewerSignal::Level,
        direction: ReviewerDirection::LowerIsBetter,
        subject: ReviewerSubject::Branch,
        model: None,
        paths: Vec::new(),
        branches: Vec::new(),
        members: Vec::new(),
    }
}

fn fresh_verdict_gate(id: &str, reviewer: &str) -> GateDef {
    GateDef {
        id: id.to_owned(),
        kind: GateKind::FreshVerdict {
            reviewers: vec![reviewer.to_owned()],
        },
        on: vec![GateOp::Push],
        when: When::default(),
        members: Vec::new(),
    }
}

fn propose_reviewer(session: &mut Session, name: &str, description: &str) -> Vec<Event> {
    session.agent(CommandKind::ProposeReviewer {
        reviewer: Box::new(reviewer_proposal(name, description)),
        reason: "the billing rules are not in the code".to_owned(),
    })
}

fn propose_gate(session: &mut Session, gate: GateDef) -> Vec<Event> {
    session.agent(CommandKind::ProposeGate {
        gate: Box::new(gate),
        reason: "billing changes need the billing reviewer".to_owned(),
    })
}

fn accept(session: &mut Session, id: i64, layer: GateLayer) -> Vec<Event> {
    session.human(CommandKind::AcceptProposal {
        id,
        layer,
        edited_reviewer: None,
        edited_gate: None,
    })
}

/// The reviewer file of `name` in the repository's `.git/gitmaster/reviewers`.
fn repo_reviewer_file(dir: &Path, name: &str) -> PathBuf {
    repo_gitmaster(dir)
        .join("reviewers")
        .join(format!("{name}.md"))
}

#[test]
fn an_agent_proposes_a_reviewer_and_the_reviewer_list_does_not_show_it() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    let mut session = Session::open(&repo, user.path());
    let events = propose_reviewer(&mut session, "billing", "Knows the billing rules.");
    assert!(done(&events), "{events:?}");
    let proposal = proposal_of(&events);
    assert_eq!(proposal.status, ProposalStatus::Proposed);
    assert_eq!(proposal.origin, "agent:test-agent");
    assert!(!proposal.updates);
    assert_eq!(proposal.decided_at, None);
    let names: Vec<String> = session
        .reviewer_names()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(!names.contains(&"billing".to_owned()), "{names:?}");
    assert_eq!(session.proposals(None).len(), 1);
    assert!(!repo_reviewer_file(&repo, "billing").exists());
}

#[test]
fn a_gate_that_names_a_proposed_reviewer_still_blocks_as_not_defined() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    write_repo_gates(
        &repo,
        r#"{"gates":[{"id":"billing-gate","kind":"fresh_verdict","reviewers":["billing"],"on":["push"]}]}"#,
    );
    let mut session = Session::open(&repo, user.path());
    assert!(done(&propose_reviewer(
        &mut session,
        "billing",
        "Knows the billing rules."
    )));
    let status = session.gates_status();
    assert!(!status.ready);
    let gate = gate_named(&status, "billing-gate");
    assert!(!gate.passed);
    assert!(
        gate.reason
            .as_deref()
            .unwrap_or_default()
            .contains("reviewer billing is not defined"),
        "{gate:?}"
    );
}

#[test]
fn a_person_accepts_a_proposed_reviewer_into_the_repository_and_it_is_listed() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    write_repo_gates(
        &repo,
        r#"{"gates":[{"id":"billing-gate","kind":"fresh_verdict","reviewers":["billing"],"on":["push"]}]}"#,
    );
    let mut session = Session::open(&repo, user.path());
    let proposed = propose_reviewer(&mut session, "billing", "Knows the billing rules.");
    let id = proposal_of(&proposed).id;
    let accepted = accept(&mut session, id, GateLayer::Repository);
    assert!(done(&accepted), "{accepted:?}");
    let proposal = proposal_of(&accepted);
    assert_eq!(proposal.status, ProposalStatus::Accepted);
    assert!(proposal.decided_at.is_some());
    assert!(repo_reviewer_file(&repo, "billing").exists());
    let listed = session.reviewer_names();
    assert!(
        listed.contains(&("billing".to_owned(), ReviewerSource::Repository)),
        "{listed:?}"
    );
    let status = session.gates_status();
    assert!(
        status
            .missing
            .iter()
            .any(|missing| missing.reviewer == "billing"),
        "the accepted reviewer needs a verdict: {status:?}"
    );
}

#[test]
fn accepting_with_an_edited_definition_writes_the_edited_one() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    let mut session = Session::open(&repo, user.path());
    let id = proposal_of(&propose_reviewer(
        &mut session,
        "billing",
        "Old description.",
    ))
    .id;
    let accepted = session.human(CommandKind::AcceptProposal {
        id,
        layer: GateLayer::Repository,
        edited_reviewer: Some(Box::new(reviewer_proposal("billing", "New description."))),
        edited_gate: None,
    });
    assert!(done(&accepted), "{accepted:?}");
    let text = std::fs::read_to_string(repo_reviewer_file(&repo, "billing")).unwrap();
    assert!(text.contains("New description."), "{text}");
    assert!(!text.contains("Old description."), "{text}");
    let listed = session.human(CommandKind::ListReviewers {
        name: Some("billing".to_owned()),
    });
    assert_eq!(reviewers_listed(&listed)[0].description, "New description.");
    assert_eq!(
        proposal_of(&accepted).reviewer.unwrap().description,
        "New description."
    );
}

#[test]
fn accepting_into_the_user_layer_writes_to_the_temporary_user_directory() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    let mut session = Session::open(&repo, user.path());
    let id = proposal_of(&propose_reviewer(
        &mut session,
        "billing",
        "Knows the billing rules.",
    ))
    .id;
    assert!(done(&accept(&mut session, id, GateLayer::User)));
    assert!(user.path().join("reviewers").join("billing.md").exists());
    assert!(!repo_reviewer_file(&repo, "billing").exists());
    let listed = session.reviewer_names();
    assert!(
        listed.contains(&("billing".to_owned(), ReviewerSource::User)),
        "{listed:?}"
    );
}

#[test]
fn a_proposed_gate_does_not_block_a_push_and_an_accepted_one_does() {
    let fx = fixture();
    let user = tempfile::tempdir().unwrap();
    let mut session = Session::open(&fx.repo, user.path());
    let proposed = propose_gate(
        &mut session,
        fresh_verdict_gate("security-gate", "security"),
    );
    let id = proposal_of(&proposed).id;
    assert!(
        session.gates_status().gates.is_empty(),
        "a proposed gate is not a gate"
    );
    let pushed = session.remote(CommandKind::PushWithLease);
    assert!(done(&pushed), "{pushed:?}");
    assert!(!blocked(&pushed));
    assert!(!git(&fx.origin, &["branch", "--list", "feat"]).is_empty());

    commit(&fx.repo, "one\nthree\nfour\n", "more on feat");
    assert!(done(&accept(&mut session, id, GateLayer::Repository)));
    let vetoed = session.remote(CommandKind::PushWithLease);
    assert_eq!(terminal(&vetoed).0, OperationState::Vetoed, "{vetoed:?}");
    assert!(blocked(&vetoed), "{vetoed:?}");
    assert!(
        refusal(&vetoed).contains("security"),
        "the veto names the gate: {vetoed:?}"
    );
}

#[test]
fn an_update_proposal_names_the_accepted_definition_it_would_replace() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    reviewers::save(
        &reviewers::repo_dir(&repo.join(".git")),
        &Reviewer {
            name: "billing".to_owned(),
            description: "Accepted description.".to_owned(),
            instructions: "Check the rounding.".to_owned(),
            signal: ReviewerSignal::Level,
            direction: ReviewerDirection::LowerIsBetter,
            subject: ReviewerSubject::Branch,
            model: None,
            source: ReviewerSource::Repository,
            paths: Vec::new(),
            branches: Vec::new(),
            members: Vec::new(),
        },
    )
    .unwrap();
    write_repo_gates(
        &repo,
        r#"{"gates":[{"id":"billing-gate","kind":"up_to_date","on":["push"]}]}"#,
    );
    let mut session = Session::open(&repo, user.path());
    let reviewer = proposal_of(&propose_reviewer(
        &mut session,
        "billing",
        "Proposed description.",
    ));
    assert!(reviewer.updates);
    assert_eq!(
        reviewer.current_reviewer.unwrap().description,
        "Accepted description."
    );
    assert_eq!(
        reviewer.reviewer.unwrap().description,
        "Proposed description."
    );
    let gate = proposal_of(&propose_gate(
        &mut session,
        GateDef {
            id: "billing-gate".to_owned(),
            kind: GateKind::UpToDate,
            on: vec![GateOp::Restack],
            when: When::default(),
            members: Vec::new(),
        },
    ));
    assert!(gate.updates);
    assert_eq!(gate.current_gate.unwrap().on, vec![GateOp::Push]);
}

#[test]
fn denying_keeps_the_proposal_listed_and_an_identical_proposal_is_refused_after() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    let mut session = Session::open(&repo, user.path());
    let id = proposal_of(&propose_reviewer(
        &mut session,
        "billing",
        "Knows the billing rules.",
    ))
    .id;
    let denied = session.human(CommandKind::DenyProposal { id });
    assert!(done(&denied), "{denied:?}");
    let proposal = proposal_of(&denied);
    assert_eq!(proposal.status, ProposalStatus::Denied);
    assert!(proposal.decided_at.is_some());
    assert_eq!(session.proposals(Some(ProposalStatus::Denied)).len(), 1);
    let again = propose_reviewer(&mut session, "billing", "Knows the billing rules.");
    assert!(
        refusal(&again).contains("this exact proposal was denied"),
        "{again:?}"
    );
    assert!(!done(&again));
}

#[test]
fn the_eleventh_open_proposal_is_refused_and_says_what_to_do() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    let mut session = Session::open(&repo, user.path());
    for index in 0..10 {
        let events = propose_reviewer(&mut session, &format!("rev-{index}"), "A reviewer.");
        assert!(done(&events), "{events:?}");
    }
    let eleventh = propose_reviewer(&mut session, "rev-10", "A reviewer.");
    assert_eq!(
        refusal(&eleventh),
        "at most 10 open proposals; ask the person to accept or deny some"
    );
    assert_eq!(session.proposals(Some(ProposalStatus::Proposed)).len(), 10);
}

#[test]
fn an_identical_open_proposal_is_refused_and_names_the_first() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    let mut session = Session::open(&repo, user.path());
    let first = proposal_of(&propose_reviewer(
        &mut session,
        "billing",
        "Knows the billing rules.",
    ))
    .id;
    let again = propose_reviewer(&mut session, "billing", "Knows the billing rules.");
    assert_eq!(
        refusal(&again),
        format!("already proposed as proposal {first}")
    );
}

#[test]
fn invalid_proposals_are_refused_and_nothing_is_stored() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    let mut session = Session::open(&repo, user.path());

    let mut bad_name = reviewer_proposal("Bad Name", "A reviewer.");
    let events = session.agent(CommandKind::ProposeReviewer {
        reviewer: Box::new(bad_name.clone()),
        reason: "why".to_owned(),
    });
    assert!(
        refusal(&events).contains("not a reviewer name"),
        "{events:?}"
    );

    bad_name = reviewer_proposal("billing", "A reviewer.");
    bad_name.paths = vec!["../outside".to_owned()];
    let events = session.agent(CommandKind::ProposeReviewer {
        reviewer: Box::new(bad_name.clone()),
        reason: "why".to_owned(),
    });
    assert!(!done(&events), "{events:?}");

    let multi_line = reviewer_proposal("billing", "two\nlines");
    let events = session.agent(CommandKind::ProposeReviewer {
        reviewer: Box::new(multi_line),
        reason: "why".to_owned(),
    });
    assert!(!done(&events), "{events:?}");

    let empty_reason = session.agent(CommandKind::ProposeReviewer {
        reviewer: Box::new(reviewer_proposal("billing", "A reviewer.")),
        reason: "   ".to_owned(),
    });
    assert!(
        refusal(&empty_reason).contains("the reason is empty"),
        "{empty_reason:?}"
    );

    let long_reason = session.agent(CommandKind::ProposeReviewer {
        reviewer: Box::new(reviewer_proposal("billing", "A reviewer.")),
        reason: "x".repeat(301),
    });
    assert!(
        refusal(&long_reason).contains("at most 300"),
        "{long_reason:?}"
    );

    let no_ops = session.agent(CommandKind::ProposeGate {
        gate: Box::new(GateDef {
            on: Vec::new(),
            ..fresh_verdict_gate("gate-one", "billing")
        }),
        reason: "why".to_owned(),
    });
    assert!(!done(&no_ops), "{no_ops:?}");

    assert!(session.proposals(None).is_empty());
}

#[test]
fn secrets_in_the_instructions_and_the_reason_are_stored_redacted() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    let mut session = Session::open(&repo, user.path());
    // Built from pieces so that no secret sits in the source (an AWS access key id shape).
    let secret = format!("AKIA{}", "ABCDEFGHIJKLMNOP");
    let mut reviewer = reviewer_proposal("billing", "Knows the billing rules.");
    reviewer.instructions = format!("Use the key {secret} to check the invoices.");
    let events = session.agent(CommandKind::ProposeReviewer {
        reviewer: Box::new(reviewer),
        reason: format!("the key {secret} was in the code"),
    });
    assert!(done(&events), "{events:?}");
    let proposal = proposal_of(&events);
    let instructions = proposal.reviewer.unwrap().instructions;
    assert!(!instructions.contains(&secret), "{instructions}");
    assert!(
        instructions.contains("to check the invoices"),
        "{instructions}"
    );
    assert!(!proposal.reason.contains(&secret), "{}", proposal.reason);
    let listed = serde_json::to_string(&session.proposals(None)).unwrap();
    assert!(!listed.contains(&secret), "the store kept the secret");
}

#[test]
fn a_misconfigured_redaction_refuses_the_proposal_and_stores_nothing() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    git(&repo, &["config", "gitmaster-redact.broken.regex", "("]);
    let mut session = Session::open(&repo, user.path());
    let events = propose_reviewer(&mut session, "billing", "Knows the billing rules.");
    let said = refusal(&events);
    assert!(said.contains("redaction is misconfigured"), "{said}");
    assert!(session.proposals(None).is_empty());
}

#[test]
fn a_gates_file_with_errors_refuses_accepting_a_gate_and_leaves_it_proposed() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    write_repo_gates(&repo, "{not json");
    let mut session = Session::open(&repo, user.path());
    let id = proposal_of(&propose_gate(
        &mut session,
        fresh_verdict_gate("billing-gate", "billing"),
    ))
    .id;
    let accepted = accept(&mut session, id, GateLayer::Repository);
    assert!(
        refusal(&accepted).contains("fix gates.json first"),
        "{accepted:?}"
    );
    let open = session.proposals(Some(ProposalStatus::Proposed));
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, id);
}

#[test]
fn an_agent_cannot_accept_or_deny_a_proposal() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    let mut session = Session::open(&repo, user.path());
    let id = proposal_of(&propose_reviewer(
        &mut session,
        "billing",
        "Knows the billing rules.",
    ))
    .id;
    let accept_events = session.agent(CommandKind::AcceptProposal {
        id,
        layer: GateLayer::Repository,
        edited_reviewer: None,
        edited_gate: None,
    });
    assert_eq!(
        refusal(&accept_events),
        "only a human can accept a proposal"
    );
    let deny_events = session.agent(CommandKind::DenyProposal { id });
    assert_eq!(refusal(&deny_events), "only a human can deny a proposal");
    assert!(!repo_reviewer_file(&repo, "billing").exists());
    assert_eq!(session.proposals(Some(ProposalStatus::Proposed)).len(), 1);
}

#[test]
fn a_failed_write_leaves_the_proposal_proposed() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    // A file where the reviewers folder should be: the write cannot create it.
    std::fs::create_dir_all(repo_gitmaster(&repo)).unwrap();
    std::fs::write(repo_gitmaster(&repo).join("reviewers"), "not a folder").unwrap();
    let mut session = Session::open(&repo, user.path());
    let id = proposal_of(&propose_reviewer(
        &mut session,
        "billing",
        "Knows the billing rules.",
    ))
    .id;
    let accepted = accept(&mut session, id, GateLayer::Repository);
    assert!(refusal(&accepted).contains("cannot"), "{accepted:?}");
    let open = session.proposals(Some(ProposalStatus::Proposed));
    assert_eq!(open.len(), 1, "the proposal is still open");
    assert_eq!(open[0].status, ProposalStatus::Proposed);
}

#[test]
fn accepting_a_gate_writes_the_gate_file_and_a_gate_with_the_same_id_replaces_it() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    let mut session = Session::open(&repo, user.path());
    let first = proposal_of(&propose_gate(
        &mut session,
        fresh_verdict_gate("g-one", "billing"),
    ))
    .id;
    assert!(done(&accept(&mut session, first, GateLayer::Repository)));
    let file = repo_gitmaster(&repo).join("gates.json");
    let text = std::fs::read_to_string(&file).unwrap();
    let document: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(document["gates"].as_array().unwrap().len(), 1, "{text}");

    let second = proposal_of(&propose_gate(
        &mut session,
        fresh_verdict_gate("g-one", "security"),
    ))
    .id;
    assert!(done(&accept(&mut session, second, GateLayer::Repository)));
    let text = std::fs::read_to_string(&file).unwrap();
    let document: serde_json::Value = serde_json::from_str(&text).unwrap();
    let gates = document["gates"].as_array().unwrap();
    assert_eq!(gates.len(), 1, "{text}");
    assert_eq!(gates[0]["reviewers"][0], "security", "{text}");
}

#[test]
fn proposing_is_logged_and_listing_is_not() {
    let (_dir, repo) = repo_dir();
    let user = tempfile::tempdir().unwrap();
    let mut session = Session::open(&repo, user.path());
    propose_reviewer(&mut session, "billing", "Knows the billing rules.");
    session.proposals(None);
    let kinds: Vec<&CommandKind> = session
        .engine
        .log()
        .iter()
        .map(|entry| &entry.kind)
        .collect();
    assert!(
        kinds
            .iter()
            .any(|kind| matches!(kind, CommandKind::ProposeReviewer { .. })),
        "a proposal is a write and is logged"
    );
    assert!(
        !kinds
            .iter()
            .any(|kind| matches!(kind, CommandKind::ListProposals { .. })),
        "a listing is a read and is not logged"
    );
}
