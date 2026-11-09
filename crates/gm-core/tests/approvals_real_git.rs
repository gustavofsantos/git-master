//! Approvals and branch intents (D215) against real repositories in temporary directories: a
//! person approves the tip and the approval goes stale when the branch moves, an agent is refused
//! for every write and nothing is written, a head that is not the tip is refused, the description
//! round-trips, and the gate inputs report the approval and the intent. Each test removes its
//! directory when it ends.

use std::path::Path;
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::{Engine, GateInputs};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

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

/// A repository on `main` with `f.txt`, then `feat` with one change to it committed.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("f.txt"), "one\ntwo\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    git(dir.path(), &["checkout", "-q", "-b", "feat"]);
    commit(dir.path(), "one\nthree\n", "feat change");
    dir
}

fn commit(dir: &Path, text: &str, message: &str) {
    std::fs::write(dir.join("f.txt"), text).unwrap();
    git(dir, &["commit", "-q", "-am", message]);
}

/// An engine with `dir` open, and a helper that sends it commands with fresh operation ids.
struct Session {
    engine: Engine,
    next: u64,
}

impl Session {
    fn open(dir: &Path) -> Self {
        let mut session = Self {
            engine: Engine::new(Box::new(SubprocessGit::default())),
            next: 1,
        };
        let events = session.run(
            Actor::Human,
            CommandKind::OpenRepo {
                path: dir.to_string_lossy().into_owned(),
            },
        );
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

    /// The approval of `branch` as a read says it now.
    fn approval(&mut self, branch: &str) -> Option<gm_schema::Approval> {
        let events = self.human(CommandKind::ReadApproval {
            branch: branch.to_owned(),
            member: None,
        });
        approval_in(&events)
    }
}

fn done(events: &[Event]) -> bool {
    state_of(events) == Some(OperationState::Done)
}

fn state_of(events: &[Event]) -> Option<OperationState> {
    events.iter().rev().find_map(|event| match event {
        Event::OperationChanged { state, .. } => Some(*state),
        _ => None,
    })
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

/// The approval the first `ApprovalRead` carries; `None` when it has none or there is no read.
fn approval_in(events: &[Event]) -> Option<gm_schema::Approval> {
    events.iter().find_map(|event| match event {
        Event::ApprovalRead { approval, .. } => approval.clone(),
        _ => None,
    })
}

/// The text the first `BranchDescriptionRead` carries; `None` when it has none or there is no read.
fn text_in(events: &[Event]) -> Option<String> {
    events.iter().find_map(|event| match event {
        Event::BranchDescriptionRead { text, .. } => text.clone(),
        _ => None,
    })
}

fn approve(branch: &str, head: &str, note: Option<&str>) -> CommandKind {
    CommandKind::ApproveBranch {
        branch: branch.to_owned(),
        head: head.to_owned(),
        note: note.map(str::to_owned),
        member: None,
    }
}

#[test]
fn a_person_approves_the_tip_and_the_approval_is_fresh_until_the_branch_moves() {
    let dir = repo();
    let tip = git(dir.path(), &["rev-parse", "feat"]);
    let mut session = Session::open(dir.path());
    let events = session.human(approve("feat", &tip, Some("looks right")));
    assert!(done(&events), "{}", failure(&events));
    let approved = approval_in(&events).expect("the approval");
    assert_eq!(approved.head, tip);
    assert_eq!(approved.origin, "human");
    assert_eq!(approved.note.as_deref(), Some("looks right"));
    assert!(!approved.stale);
    let read = session.approval("feat").expect("still approved");
    assert!(!read.stale);
    commit(dir.path(), "one\nfour\n", "a later commit");
    let moved = session.approval("feat").expect("still stored");
    assert!(moved.stale, "a new commit makes the approval stale");
    assert_eq!(moved.head, tip, "the approved commit is kept as it was");
}

#[test]
fn an_agent_is_refused_every_write_and_nothing_is_written() {
    let dir = repo();
    let tip = git(dir.path(), &["rev-parse", "feat"]);
    let mut session = Session::open(dir.path());
    let approved = session.agent(approve("feat", &tip, None));
    assert_eq!(state_of(&approved), Some(OperationState::Failed));
    assert_eq!(failure(&approved), "only a human can approve a branch");
    let revoked = session.agent(CommandKind::RevokeApproval {
        branch: "feat".to_owned(),
        member: None,
    });
    assert_eq!(failure(&revoked), "only a human can revoke an approval");
    let described = session.agent(CommandKind::SetBranchDescription {
        branch: "feat".to_owned(),
        text: "an agent's intent".to_owned(),
        member: None,
    });
    assert_eq!(
        failure(&described),
        "only a human can edit a branch description"
    );
    assert_eq!(session.approval("feat"), None);
    // `git config --get` exits 1 for a key that is not set.
    let key = Process::new("git")
        .args(["config", "--get", "branch.feat.description"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!key.status.success(), "the description was not written");
}

#[test]
fn an_agent_cannot_revoke_an_approval_that_a_person_gave() {
    let dir = repo();
    let tip = git(dir.path(), &["rev-parse", "feat"]);
    let mut session = Session::open(dir.path());
    assert!(done(&session.human(approve("feat", &tip, None))));
    let revoked = session.agent(CommandKind::RevokeApproval {
        branch: "feat".to_owned(),
        member: None,
    });
    assert_eq!(state_of(&revoked), Some(OperationState::Failed));
    assert!(session.approval("feat").is_some(), "the approval stands");
}

#[test]
fn a_head_that_is_not_the_tip_is_refused_and_says_to_look_again() {
    let dir = repo();
    let old = git(dir.path(), &["rev-parse", "feat~1"]);
    let mut session = Session::open(dir.path());
    let events = session.human(approve("feat", &old, None));
    assert_eq!(state_of(&events), Some(OperationState::Failed));
    let reason = failure(&events);
    assert!(
        reason.starts_with("the branch moved since you looked:")
            && reason.ends_with("; look again"),
        "{reason}"
    );
    assert_eq!(session.approval("feat"), None);
}

#[test]
fn approving_a_branch_that_does_not_exist_is_refused() {
    let dir = repo();
    let tip = git(dir.path(), &["rev-parse", "feat"]);
    let mut session = Session::open(dir.path());
    let events = session.human(approve("nope", &tip, None));
    assert_eq!(failure(&events), "there is no branch named nope");
}

#[test]
fn a_note_over_500_characters_is_refused_and_a_blank_note_is_no_note() {
    let dir = repo();
    let tip = git(dir.path(), &["rev-parse", "feat"]);
    let mut session = Session::open(dir.path());
    let long = "n".repeat(501);
    let events = session.human(approve("feat", &tip, Some(&long)));
    assert_eq!(
        failure(&events),
        "an approval note is at most 500 characters, not 501"
    );
    assert_eq!(session.approval("feat"), None);
    let blank = session.human(approve("feat", &tip, Some("   ")));
    assert!(done(&blank));
    assert_eq!(approval_in(&blank).expect("the approval").note, None);
}

#[test]
fn revoking_removes_the_approval_and_revoking_twice_is_fine() {
    let dir = repo();
    let tip = git(dir.path(), &["rev-parse", "feat"]);
    let mut session = Session::open(dir.path());
    assert!(done(&session.human(approve("feat", &tip, None))));
    let revoked = session.human(CommandKind::RevokeApproval {
        branch: "feat".to_owned(),
        member: None,
    });
    assert!(done(&revoked));
    assert!(
        revoked
            .iter()
            .any(|event| matches!(event, Event::ApprovalRead { approval: None, .. }))
    );
    assert_eq!(approval_in(&revoked), None);
    assert_eq!(session.approval("feat"), None);
    assert!(done(&session.human(CommandKind::RevokeApproval {
        branch: "feat".to_owned(),
        member: None,
    })));
}

#[test]
fn a_description_round_trips_with_newlines_and_quotes_and_unsets_on_blank() {
    let dir = repo();
    git(dir.path(), &["branch", "billing/invoice"]);
    let mut session = Session::open(dir.path());
    let read = |session: &mut Session| {
        text_in(&session.human(CommandKind::ReadBranchDescription {
            branch: "billing/invoice".to_owned(),
            member: None,
        }))
    };
    assert_eq!(read(&mut session), None);
    let text = "Move the invoice total to cents.\n\nKeeps \"legacy\" rounding for -1 values.";
    let set = session.human(CommandKind::SetBranchDescription {
        branch: "billing/invoice".to_owned(),
        text: text.to_owned(),
        member: None,
    });
    assert!(done(&set), "{}", failure(&set));
    assert_eq!(text_in(&set), Some(text.to_owned()));
    assert_eq!(read(&mut session), Some(text.to_owned()));
    let unset = session.human(CommandKind::SetBranchDescription {
        branch: "billing/invoice".to_owned(),
        text: "  ".to_owned(),
        member: None,
    });
    assert_eq!(text_in(&unset), None);
    assert_eq!(read(&mut session), None);
}

#[test]
fn a_description_over_2000_characters_or_with_a_nul_is_refused() {
    let dir = repo();
    let mut session = Session::open(dir.path());
    let long = session.human(CommandKind::SetBranchDescription {
        branch: "feat".to_owned(),
        text: "d".repeat(2001),
        member: None,
    });
    assert_eq!(
        failure(&long),
        "a branch description is at most 2000 characters, not 2001"
    );
    let nul = session.human(CommandKind::SetBranchDescription {
        branch: "feat".to_owned(),
        text: "a\0b".to_owned(),
        member: None,
    });
    assert_eq!(
        failure(&nul),
        "a branch description cannot contain a NUL character"
    );
    let exact = session.human(CommandKind::SetBranchDescription {
        branch: "feat".to_owned(),
        text: "d".repeat(2000),
        member: None,
    });
    assert!(
        done(&exact),
        "2000 characters is allowed: {}",
        failure(&exact)
    );
}

#[test]
fn a_description_of_a_missing_branch_is_refused() {
    let dir = repo();
    let mut session = Session::open(dir.path());
    let events = session.human(CommandKind::SetBranchDescription {
        branch: "nope".to_owned(),
        text: "text".to_owned(),
        member: None,
    });
    assert_eq!(failure(&events), "there is no local branch named nope");
}

#[test]
fn the_gate_inputs_report_the_tip_the_change_the_approval_and_the_intent() {
    let dir = repo();
    let tip = git(dir.path(), &["rev-parse", "feat"]);
    let mut session = Session::open(dir.path());
    assert!(done(&session.human(CommandKind::SetBranchDescription {
        branch: "feat".to_owned(),
        text: "Fix the rounding.".to_owned(),
        member: None,
    })));
    let before = inputs(&mut session, dir.path());
    assert_eq!(before.head, tip);
    assert!(!before.approved);
    assert_eq!(before.intent.as_deref(), Some("Fix the rounding."));
    let change = before.change.expect("feat has a base");
    assert_eq!(change.base_ref, "main");
    assert_eq!(change.paths, Some(vec!["f.txt".to_owned()]));
    assert!(done(&session.human(approve("feat", &tip, None))));
    assert!(inputs(&mut session, dir.path()).approved);
    commit(dir.path(), "one\nfive\n", "moves on");
    assert!(
        !inputs(&mut session, dir.path()).approved,
        "a stale approval does not count"
    );
}

fn inputs(session: &mut Session, dir: &Path) -> GateInputs {
    session
        .engine
        .gate_facts_inputs(dir, "feat")
        .expect("gate inputs")
}
