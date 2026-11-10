//! The hand-off of a scorecard's comment (D219 (6), D200 (5a)) against a real repository: an agent
//! that fixed what a review found marks the comment handled with a summary, and the person closes
//! it. A thread the agent started on its own stays the person's to close. Each test removes its
//! directory when it ends.

use std::path::Path;
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_schema::{
    Actor, Command, CommandKind, CommentLabel, Event, OperationId, OperationState,
    ReviewAuthorFilter, ReviewComment, ReviewKind, ReviewSeverity, ReviewStatus,
    ScorecardCommentInput, ScorecardStatus, ScorecardSummary,
};

fn git(dir: &Path, args: &[&str]) {
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
    assert!(output.status.success(), "git {args:?} failed");
}

/// A repository on `feat` with `lib.rs` committed (three lines).
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("lib.rs"), "fn a() {\n    x.unwrap();\n}\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    git(dir.path(), &["checkout", "-q", "-b", "feat"]);
    dir
}

fn command(actor: Actor, kind: CommandKind) -> Command {
    Command { actor, kind }
}

fn agent() -> Actor {
    Actor::Agent {
        name: "claude-code".to_owned(),
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

fn comments(engine: &mut Engine) -> Vec<ReviewComment> {
    let events = engine.handle(
        OperationId(90),
        &command(
            Actor::Human,
            CommandKind::ListReviewComments {
                author: ReviewAuthorFilter::Any,
            },
        ),
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::ReviewCommentsListed { comments, .. } => Some(comments.clone()),
            _ => None,
        })
        .expect("the comments")
}

fn engine_on(dir: &Path) -> Engine {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    let events = engine.handle(
        OperationId(1),
        &command(
            Actor::Human,
            CommandKind::OpenRepo {
                path: dir.to_string_lossy().into_owned(),
            },
        ),
    );
    assert!(done(&events), "open failed: {events:?}");
    engine
}

/// Publishes a scorecard with one blocking issue on the `unwrap` line, and returns its comment's id.
fn publish_with_a_finding(engine: &mut Engine, dir: &Path) -> i64 {
    let output = Process::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    let head = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    let events = engine.handle(
        OperationId(2),
        &command(
            agent(),
            CommandKind::PublishScorecard {
                name: "security".into(),
                branch: "feat".into(),
                head,
                status: ScorecardStatus::Fail,
                summary: ScorecardSummary {
                    label: CommentLabel::Issue,
                    decorations: vec!["blocking".into()],
                    subject: "A panic on bad input".into(),
                    body: "The unwrap panics.".into(),
                },
                comments: vec![ScorecardCommentInput {
                    path: "lib.rs".into(),
                    start_line: 2,
                    end_line: 2,
                    label: CommentLabel::Issue,
                    decorations: vec!["blocking".into()],
                    body: "Return an error instead of unwrapping.".into(),
                    suggested_fix: Some("x.ok_or(Error::Missing)?;".into()),
                }],
                member: None,
            },
        ),
    );
    assert!(done(&events), "publish failed: {events:?}");
    events
        .iter()
        .find_map(|event| match event {
            Event::ScorecardPublished { comment_ids, .. } => comment_ids.first().copied(),
            _ => None,
        })
        .expect("the comment")
}

#[test]
fn an_agent_hands_off_a_scorecard_comment_and_the_person_closes_it() {
    let dir = repo();
    let mut engine = engine_on(dir.path());
    let id = publish_with_a_finding(&mut engine, dir.path());
    let suggested = comments(&mut engine)
        .into_iter()
        .find(|c| c.id == id)
        .expect("the comment");
    assert!(suggested.scorecard.is_some());
    assert_eq!(suggested.severity, Some(ReviewSeverity::High));
    assert_eq!(suggested.kind, Some(ReviewKind::Flag));
    assert_eq!(
        suggested.suggested_fix.as_deref(),
        Some("x.ok_or(Error::Missing)?;")
    );

    std::fs::write(
        dir.path().join("lib.rs"),
        "fn a() {\n    x.ok_or(Error::Missing)?;\n}\n",
    )
    .unwrap();
    let hand_off = engine.handle(
        OperationId(3),
        &command(
            agent(),
            CommandKind::HandleReviewComment {
                id,
                summary: "returns an error now".into(),
            },
        ),
    );
    assert!(done(&hand_off), "{hand_off:?}");
    let handled = comments(&mut engine)
        .into_iter()
        .find(|c| c.id == id)
        .expect("the comment");
    assert_eq!(handled.status, ReviewStatus::Handled);
    assert_eq!(handled.replies.len(), 1);
    assert_eq!(handled.replies[0].body, "returns an error now");

    let closed = engine.handle(
        OperationId(4),
        &command(Actor::Human, CommandKind::ResolveReviewComment { id }),
    );
    assert!(done(&closed), "{closed:?}");
}

#[test]
fn an_agent_cannot_hand_off_a_thread_it_started_on_its_own() {
    let dir = repo();
    let mut engine = engine_on(dir.path());
    let created = engine.handle(
        OperationId(5),
        &command(
            agent(),
            CommandKind::CreateAgentReviewComment {
                path: "lib.rs".into(),
                anchor_text: "    x.unwrap();".into(),
                kind: Some(ReviewKind::Note),
                body: "a remark".into(),
                severity: None,
                label: None,
                decorations: Vec::new(),
            },
        ),
    );
    assert!(done(&created), "{created:?}");
    let id = comments(&mut engine)
        .into_iter()
        .find(|c| c.author.is_some())
        .expect("the agent's thread")
        .id;
    let refused = engine.handle(
        OperationId(6),
        &command(
            agent(),
            CommandKind::HandleReviewComment {
                id,
                summary: "done".into(),
            },
        ),
    );
    assert!(
        !done(&refused),
        "only a person closes a thread the agent started"
    );
}
