//! The suggested fix of a finding and the fix loop (D215 (3)) against real repositories in
//! temporary directories: a reviewer's verdict keeps the fix of each finding redacted, another
//! agent marks a finding handled with a summary, and the `findings` gate still waits for a
//! person to resolve or dismiss it. Each test removes its directories when it ends, and the
//! user's reviewer and gate directories are always temporary ones.

use std::path::Path;
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::{Engine, reviewers};
use gm_schema::{
    Actor, Command, CommandKind, Event, FindingInput, GatesStatus, OperationId, OperationState,
    ReviewAuthorFilter, ReviewComment, ReviewSeverity, ReviewStatus, Reviewer, ReviewerSource,
    Verdict,
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
    String::from_utf8(output.stdout).unwrap()
}

/// A repository on `main` with `f.txt` (five lines) committed, checked out on `feat`, with the
/// `security` reviewer defined and a `findings` gate that blocks a push on any high finding.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("f.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    git(dir.path(), &["checkout", "-q", "-b", "feat"]);
    for preset in reviewers::presets() {
        if preset.name == "security" {
            let reviewer = Reviewer {
                source: ReviewerSource::Repository,
                ..preset
            };
            reviewers::save(&reviewers::repo_dir(&dir.path().join(".git")), &reviewer).unwrap();
        }
    }
    let gitmaster = dir.path().join(".git").join("gitmaster");
    std::fs::create_dir_all(&gitmaster).unwrap();
    std::fs::write(
        gitmaster.join("gates.json"),
        r#"{"gates":[{"id":"no-high","kind":"findings","severity":"high","on":["push"]}]}"#,
    )
    .unwrap();
    dir
}

fn reviewer_actor() -> Actor {
    Actor::Agent {
        name: "reviewer".to_owned(),
    }
}

fn fixer_actor() -> Actor {
    Actor::Agent {
        name: "fixer".to_owned(),
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

/// An engine with the repository open, and the user's directories temporary.
struct Session {
    engine: Engine,
    _user: tempfile::TempDir,
    next: u64,
}

impl Session {
    fn open(dir: &Path) -> Self {
        let user = tempfile::tempdir().unwrap();
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_reviewer_user_dir(Some(reviewers::user_dir(user.path())));
        engine.set_gates_user_dir(Some(user.path().to_path_buf()));
        let mut session = Self {
            engine,
            _user: user,
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

    fn comments(&mut self) -> Vec<ReviewComment> {
        let events = self.run(
            Actor::Human,
            CommandKind::ListReviewComments {
                author: ReviewAuthorFilter::Any,
            },
        );
        events
            .into_iter()
            .find_map(|event| match event {
                Event::ReviewCommentsListed { comments, .. } => Some(comments),
                _ => None,
            })
            .expect("the list answers")
    }

    fn verdicts(&mut self) -> Vec<Verdict> {
        let events = self.run(
            fixer_actor(),
            CommandKind::ListVerdicts {
                branch: None,
                reviewer: None,
                limit: 20,
            },
        );
        events
            .into_iter()
            .find_map(|event| match event {
                Event::VerdictsListed { verdicts, .. } => Some(verdicts),
                _ => None,
            })
            .expect("the listing answers")
    }

    fn gates(&mut self) -> GatesStatus {
        let events = self.run(Actor::Human, CommandKind::GatesStatus { branch: None });
        assert!(done(&events), "{}", failure(&events));
        events
            .into_iter()
            .find_map(|event| match event {
                Event::GatesStatusRead { status, .. } => Some(status),
                _ => None,
            })
            .expect("the gate check answers")
    }
}

fn finding(start: u32, body: &str, fix: Option<&str>) -> FindingInput {
    FindingInput {
        severity: ReviewSeverity::High,
        path: "f.txt".to_owned(),
        start_line: start,
        end_line: start,
        body: body.to_owned(),
        suggested_fix: fix.map(str::to_owned),
    }
}

/// Records a `security` verdict on `feat` at its tip, as the reviewer agent.
fn record(session: &mut Session, findings: Vec<FindingInput>) -> Vec<Event> {
    session.run(
        reviewer_actor(),
        CommandKind::RecordVerdict {
            reviewer: "security".to_owned(),
            branch: "feat".to_owned(),
            head: "HEAD".to_owned(),
            value: "High".to_owned(),
            rationale: "Two lines are unchecked.".to_owned(),
            findings,
        },
    )
}

/// The id of the only finding comment in `events`.
fn finding_id(events: &[Event]) -> i64 {
    events
        .iter()
        .find_map(|event| match event {
            Event::VerdictRecorded { comment_ids, .. } => comment_ids.first().copied(),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no finding comment in {events:?}"))
}

fn comment(session: &mut Session, id: i64) -> ReviewComment {
    session
        .comments()
        .into_iter()
        .find(|comment| comment.id == id)
        .unwrap_or_else(|| panic!("no comment {id}"))
}

fn hand_off(session: &mut Session, actor: Actor, id: i64, summary: &str) -> Vec<Event> {
    session.run(
        actor,
        CommandKind::HandleReviewComment {
            id,
            summary: summary.to_owned(),
        },
    )
}

#[test]
fn a_finding_keeps_its_fix_redacted_and_the_listing_returns_body_fix_and_line() {
    let dir = repo();
    let mut session = Session::open(dir.path());
    // Built from pieces so that no secret sits in the source (an AWS access key id shape).
    let secret = ["AKIA", "ABCDEFGHIJKLMNOP"].concat();
    let fix = format!("Reject an empty value here. The key is {secret}.");
    let recorded = record(
        &mut session,
        vec![
            finding(2, "unchecked input", Some(&fix)),
            finding(4, "no bound", Some("   ")),
        ],
    );
    assert!(done(&recorded), "{}", failure(&recorded));

    let verdict = session.verdicts().remove(0);
    assert_eq!(verdict.findings.len(), 2);
    let first = &verdict.findings[0];
    assert_eq!(first.body, "unchecked input");
    assert_eq!(first.start_line, Some(2));
    let stored = first.suggested_fix.as_deref().expect("the fix is stored");
    assert!(stored.contains("Reject an empty value here"), "{stored}");
    assert!(
        !stored.contains(&secret),
        "the fix kept the secret: {stored}"
    );
    assert_eq!(
        verdict.findings[1].suggested_fix, None,
        "a blank fix is no fix"
    );

    let listed = finding_id(&recorded);
    let root = comment(&mut session, listed);
    assert!(root.finding);
    assert_eq!(root.author.as_deref(), Some("agent:reviewer"));
    assert_eq!(root.suggested_fix.as_deref(), Some(stored));
    let json = serde_json::to_string(&session.comments()).unwrap();
    assert!(!json.contains(&secret), "the comment kept the secret");
}

#[test]
fn another_agent_hands_off_a_finding_and_the_findings_gate_still_waits_for_the_person() {
    let dir = repo();
    let mut session = Session::open(dir.path());
    let recorded = record(
        &mut session,
        vec![finding(2, "unchecked input", Some("Reject it."))],
    );
    assert!(done(&recorded), "{}", failure(&recorded));
    let id = finding_id(&recorded);
    assert!(!session.gates().ready, "an open finding blocks");

    let handed = hand_off(
        &mut session,
        fixer_actor(),
        id,
        "Rejects empty input at line 2.",
    );
    assert!(done(&handed), "{}", failure(&handed));
    let root = comment(&mut session, id);
    assert_eq!(root.status, ReviewStatus::Handled);
    let reply = root.replies.last().expect("the summary is a reply");
    assert_eq!(reply.author, "agent:fixer");
    assert_eq!(reply.body, "Rejects empty input at line 2.");

    let gates = session.gates();
    assert!(!gates.ready, "a handled finding still blocks");
    let gate = gates
        .gates
        .iter()
        .find(|gate| gate.id == "no-high")
        .unwrap();
    assert!(
        gate.reason
            .as_deref()
            .is_some_and(|reason| reason.contains("unresolved finding")),
        "{gate:?}"
    );

    for kind in [
        CommandKind::ResolveReviewComment { id },
        CommandKind::ReopenReviewComment { id },
        CommandKind::DeleteReviewComment { id },
    ] {
        let refused = session.run(fixer_actor(), kind);
        assert_eq!(state_of(&refused), Some(OperationState::Failed));
        assert!(
            failure(&refused).starts_with("only a human can "),
            "{}",
            failure(&refused)
        );
    }
    assert_eq!(comment(&mut session, id).status, ReviewStatus::Handled);

    let resolved = session.run(Actor::Human, CommandKind::ResolveReviewComment { id });
    assert!(done(&resolved), "{}", failure(&resolved));
    assert!(session.gates().ready, "a person's resolve passes the gate");
}

#[test]
fn an_agent_still_cannot_hand_off_a_plain_note_it_started() {
    let dir = repo();
    let mut session = Session::open(dir.path());
    let note = session.run(
        reviewer_actor(),
        CommandKind::CreateAgentReviewComment {
            path: "f.txt".to_owned(),
            anchor_text: "three".to_owned(),
            kind: gm_schema::ReviewKind::Note,
            body: "the loop here is bounded".to_owned(),
            severity: None,
        },
    );
    assert!(done(&note), "{}", failure(&note));
    let id = session
        .comments()
        .into_iter()
        .find(|comment| comment.author.is_some())
        .expect("the agent's note is listed")
        .id;
    let refused = hand_off(&mut session, fixer_actor(), id, "done");
    assert_eq!(
        failure(&refused),
        "an agent cannot hand off a thread it started: only a person closes it"
    );
    assert_eq!(comment(&mut session, id).status, ReviewStatus::Open);
}

#[test]
fn a_person_reply_reopens_a_handled_finding_and_a_resolve_then_passes_the_gate() {
    let dir = repo();
    let mut session = Session::open(dir.path());
    let recorded = record(&mut session, vec![finding(2, "unchecked input", None)]);
    let id = finding_id(&recorded);
    assert!(done(&hand_off(
        &mut session,
        fixer_actor(),
        id,
        "First try."
    )));

    let reply = session.run(
        Actor::Human,
        CommandKind::ReplyToReviewComment {
            id,
            body: "The empty case is still open.".to_owned(),
        },
    );
    assert!(done(&reply), "{}", failure(&reply));
    assert_eq!(comment(&mut session, id).status, ReviewStatus::Open);
    assert!(!session.gates().ready);

    assert!(done(&hand_off(
        &mut session,
        fixer_actor(),
        id,
        "Covered the empty case."
    )));
    assert_eq!(comment(&mut session, id).status, ReviewStatus::Handled);
    let resolved = session.run(Actor::Human, CommandKind::ResolveReviewComment { id });
    assert!(done(&resolved), "{}", failure(&resolved));
    assert!(session.gates().ready);
}

#[test]
fn an_empty_summary_is_refused_and_the_finding_stays_open() {
    let dir = repo();
    let mut session = Session::open(dir.path());
    let recorded = record(&mut session, vec![finding(2, "unchecked input", None)]);
    let id = finding_id(&recorded);
    let refused = hand_off(&mut session, fixer_actor(), id, "   ");
    assert_eq!(failure(&refused), "the summary is empty");
    assert_eq!(comment(&mut session, id).status, ReviewStatus::Open);
    assert!(comment(&mut session, id).replies.is_empty());
}

#[test]
fn a_suggested_fix_over_a_thousand_characters_is_refused_and_writes_nothing() {
    let dir = repo();
    let mut session = Session::open(dir.path());
    let refused = record(
        &mut session,
        vec![finding(2, "unchecked input", Some(&"x".repeat(1_001)))],
    );
    assert_eq!(state_of(&refused), Some(OperationState::Failed));
    assert!(
        failure(&refused).contains("1001 characters"),
        "{}",
        failure(&refused)
    );
    assert!(session.verdicts().is_empty(), "no verdict was written");
    assert!(session.comments().is_empty(), "no finding was written");

    let accepted = record(
        &mut session,
        vec![finding(2, "unchecked input", Some(&"x".repeat(1_000)))],
    );
    assert!(done(&accepted), "{}", failure(&accepted));
    assert_eq!(session.verdicts().len(), 1);
}

#[test]
fn retention_does_not_let_a_pruned_finding_count_against_the_agent_thread_limits() {
    let dir = repo();
    for i in 0..16 {
        std::fs::write(dir.path().join(format!("n{i}.txt")), "alpha\n").unwrap();
    }
    let mut session = Session::open(dir.path());
    // The first verdict keeps an open finding; five more push it out of retention, which
    // unlinks the finding from its verdict but keeps it open.
    let first = record(&mut session, vec![finding(2, "unchecked input", None)]);
    assert!(done(&first), "{}", failure(&first));
    let finding_id = finding_id(&first);
    for _ in 0..5 {
        let more = record(&mut session, Vec::new());
        assert!(done(&more), "{}", failure(&more));
    }
    assert_eq!(session.verdicts().len(), 5, "retention kept five verdicts");
    let kept = comment(&mut session, finding_id);
    assert_eq!(
        kept.status,
        ReviewStatus::Open,
        "the pruned finding is still open"
    );

    let note = |session: &mut Session, path: &str, anchor: &str| {
        session.run(
            reviewer_actor(),
            CommandKind::CreateAgentReviewComment {
                path: path.to_owned(),
                anchor_text: anchor.to_owned(),
                kind: gm_schema::ReviewKind::Note,
                body: format!("a note on {anchor}"),
                severity: None,
            },
        )
    };
    // Five notes on f.txt fill the per-file limit; the pruned finding on f.txt must not take one.
    for anchor in ["one", "two", "three", "four", "five"] {
        let added = note(&mut session, "f.txt", anchor);
        assert!(done(&added), "{anchor}: {}", failure(&added));
    }
    // Fifteen more on other files make twenty, the repository limit.
    for i in 0..15 {
        let added = note(&mut session, &format!("n{i}.txt"), "alpha");
        assert!(done(&added), "n{i}: {}", failure(&added));
    }
    let refused = note(&mut session, "n15.txt", "alpha");
    assert!(
        failure(&refused).contains("20 open agent threads"),
        "{}",
        failure(&refused)
    );
}
