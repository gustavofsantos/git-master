//! Scorecards (D219) against real repositories in temporary directories: an agent publishes a
//! scorecard with its comments through the engine, the refusals write nothing, a scorecard whose
//! head is no longer the tip is stale, and the labels of an agent's comments give their kind and
//! severity. Each test removes its directory when it ends.

use std::path::Path;
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_schema::{
    Actor, Command, CommandKind, CommentLabel, Event, OperationId, OperationState,
    ReviewAuthorFilter, ReviewComment, ReviewKind, ReviewSeverity, ReviewStatus, Scorecard,
    ScorecardCommentInput, ScorecardStatus, ScorecardSummary,
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

/// A repository on `feat` whose `f.txt` has five lines, committed once.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("f.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    git(dir.path(), &["checkout", "-q", "-b", "feat"]);
    dir
}

fn tip(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"])
}

fn human(kind: CommandKind) -> Command {
    Command {
        actor: Actor::Human,
        kind,
    }
}

fn agent(kind: CommandKind) -> Command {
    Command {
        actor: Actor::Agent {
            name: "claude-code".to_owned(),
        },
        kind,
    }
}

fn engine_on(dir: &Path) -> Engine {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    let events = engine.handle(
        OperationId(1),
        &human(CommandKind::OpenRepo {
            path: dir.to_string_lossy().into_owned(),
        }),
    );
    assert!(done(&events), "open failed: {events:?}");
    engine
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

fn summary(label: CommentLabel, decorations: &[&str]) -> ScorecardSummary {
    ScorecardSummary {
        label,
        decorations: decorations.iter().map(|d| (*d).to_owned()).collect(),
        subject: "Unchecked input".to_owned(),
        body: "The parser trusts its input.".to_owned(),
    }
}

fn comment(
    start: u32,
    end: u32,
    label: CommentLabel,
    decorations: &[&str],
) -> ScorecardCommentInput {
    ScorecardCommentInput {
        path: "f.txt".to_owned(),
        start_line: start,
        end_line: end,
        label,
        decorations: decorations.iter().map(|d| (*d).to_owned()).collect(),
        body: format!("about lines {start} to {end}"),
        suggested_fix: None,
    }
}

fn publish(
    engine: &mut Engine,
    head: &str,
    name: &str,
    summary: ScorecardSummary,
    comments: Vec<ScorecardCommentInput>,
) -> Vec<Event> {
    engine.handle(
        OperationId(2),
        &agent(CommandKind::PublishScorecard {
            name: name.to_owned(),
            branch: "feat".to_owned(),
            head: head.to_owned(),
            status: ScorecardStatus::NeedsWork,
            summary,
            comments,
            member: None,
            workflow_run: None,
        }),
    )
}

fn published(events: &[Event]) -> (Scorecard, Vec<i64>) {
    assert!(done(events), "publish failed: {}", failure(events));
    events
        .iter()
        .find_map(|event| match event {
            Event::ScorecardPublished {
                scorecard,
                comment_ids,
                ..
            } => Some((scorecard.clone(), comment_ids.clone())),
            _ => None,
        })
        .expect("a scorecard")
}

fn listed(engine: &mut Engine, name: Option<&str>, include_stale: bool) -> Vec<Scorecard> {
    let events = engine.handle(
        OperationId(3),
        &human(CommandKind::ListScorecards {
            name: name.map(str::to_owned),
            branch: None,
            include_stale,
            limit: 0,
            member: None,
        }),
    );
    assert!(done(&events), "list failed: {}", failure(&events));
    events
        .iter()
        .find_map(|event| match event {
            Event::ScorecardsListed { scorecards, .. } => Some(scorecards.clone()),
            _ => None,
        })
        .expect("a list")
}

fn comments(engine: &mut Engine) -> Vec<ReviewComment> {
    let events = engine.handle(
        OperationId(4),
        &human(CommandKind::ListReviewComments {
            author: ReviewAuthorFilter::Any,
        }),
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::ReviewCommentsListed { comments, .. } => Some(comments.clone()),
            _ => None,
        })
        .expect("the comments")
}

#[test]
fn a_publish_writes_the_scorecard_and_its_comments_with_their_labels() {
    let dir = repo();
    let mut engine = engine_on(dir.path());
    let head = tip(dir.path());
    let (scorecard, ids) = published(&publish(
        &mut engine,
        &head,
        "security",
        summary(CommentLabel::Issue, &["blocking", "security"]),
        vec![
            comment(2, 2, CommentLabel::Issue, &["blocking"]),
            comment(4, 4, CommentLabel::Praise, &[]),
        ],
    ));
    assert_eq!(scorecard.name, "security");
    assert_eq!(scorecard.status, ScorecardStatus::NeedsWork);
    assert_eq!(scorecard.head, head);
    assert!(!scorecard.stale);
    assert_eq!(scorecard.origin, "agent:claude-code");
    assert_eq!(scorecard.decorations, ["blocking", "security"]);
    assert_eq!(scorecard.blocking_open, 1);
    assert_eq!(ids.len(), 2);

    let all = comments(&mut engine);
    let blocking = all.iter().find(|c| c.id == ids[0]).expect("first comment");
    assert_eq!(blocking.scorecard, Some(scorecard.id));
    assert_eq!(blocking.label, Some(CommentLabel::Issue));
    assert_eq!(blocking.decorations, ["blocking"]);
    assert_eq!(blocking.kind, Some(ReviewKind::Flag));
    assert_eq!(blocking.severity, Some(ReviewSeverity::High));
    assert_eq!(blocking.author.as_deref(), Some("agent:claude-code"));
    assert_eq!(blocking.anchor.text, "two");
    let praise = all.iter().find(|c| c.id == ids[1]).expect("second comment");
    assert_eq!(praise.kind, Some(ReviewKind::Note));
    assert_eq!(praise.severity, None);
    assert_eq!(listed(&mut engine, Some("security"), false).len(), 1);
}

#[test]
fn a_head_that_is_not_the_tip_is_refused_and_nothing_is_written() {
    let dir = repo();
    let mut engine = engine_on(dir.path());
    let older = tip(dir.path());
    std::fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "second"]);
    let events = publish(
        &mut engine,
        &older,
        "security",
        summary(CommentLabel::Note, &[]),
        vec![comment(1, 1, CommentLabel::Issue, &[])],
    );
    let reason = failure(&events);
    assert!(reason.contains("is not the tip of feat"), "{reason}");
    assert!(listed(&mut engine, None, true).is_empty());
    assert!(comments(&mut engine).is_empty());
}

#[test]
fn a_scorecard_is_stale_after_a_new_commit_and_listed_only_when_asked() {
    let dir = repo();
    let mut engine = engine_on(dir.path());
    let head = tip(dir.path());
    published(&publish(
        &mut engine,
        &head,
        "security",
        summary(CommentLabel::Note, &[]),
        Vec::new(),
    ));
    assert_eq!(listed(&mut engine, None, false).len(), 1);
    std::fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "second"]);
    assert!(
        listed(&mut engine, None, false).is_empty(),
        "stale is hidden"
    );
    let all = listed(&mut engine, None, true);
    assert_eq!(all.len(), 1);
    assert!(all[0].stale);
    assert_eq!(all[0].head, head, "the scorecard never changes");
}

#[test]
fn an_invalid_name_or_decoration_is_refused_and_nothing_is_written() {
    let dir = repo();
    let mut engine = engine_on(dir.path());
    let head = tip(dir.path());
    let refused = [
        publish(
            &mut engine,
            &head,
            "Security_Review",
            summary(CommentLabel::Note, &[]),
            Vec::new(),
        ),
        publish(
            &mut engine,
            &head,
            "security",
            summary(CommentLabel::Note, &["security", "auth", "api", "db"]),
            Vec::new(),
        ),
        publish(
            &mut engine,
            &head,
            "security",
            summary(CommentLabel::Note, &["blocking", "non-blocking"]),
            Vec::new(),
        ),
        publish(
            &mut engine,
            &head,
            "security",
            summary(CommentLabel::Note, &["Auth"]),
            Vec::new(),
        ),
        publish(
            &mut engine,
            &head,
            "security",
            summary(CommentLabel::Note, &[]),
            vec![comment(
                1,
                1,
                CommentLabel::Issue,
                &["blocking", "blocking"],
            )],
        ),
    ];
    for events in refused {
        assert!(!done(&events), "{events:?}");
        assert!(!failure(&events).is_empty());
    }
    assert!(listed(&mut engine, None, true).is_empty());
    assert!(comments(&mut engine).is_empty());
}

#[test]
fn a_comment_outside_the_file_refuses_the_whole_publish() {
    let dir = repo();
    let mut engine = engine_on(dir.path());
    let head = tip(dir.path());
    let events = publish(
        &mut engine,
        &head,
        "security",
        summary(CommentLabel::Note, &[]),
        vec![
            comment(1, 1, CommentLabel::Issue, &[]),
            comment(4, 9, CommentLabel::Issue, &[]),
        ],
    );
    let reason = failure(&events);
    assert!(reason.contains("f.txt"), "{reason}");
    assert!(reason.contains("not in the file"), "{reason}");
    assert!(listed(&mut engine, None, true).is_empty(), "atomic");
    assert!(
        comments(&mut engine).is_empty(),
        "the first comment is not kept"
    );
}

#[test]
fn a_comment_whose_text_appears_twice_is_refused() {
    let dir = repo();
    std::fs::write(dir.path().join("f.txt"), "same\nother\nsame\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "repeat"]);
    let mut engine = engine_on(dir.path());
    let head = tip(dir.path());
    let events = publish(
        &mut engine,
        &head,
        "security",
        summary(CommentLabel::Note, &[]),
        vec![comment(1, 1, CommentLabel::Issue, &[])],
    );
    let reason = failure(&events);
    assert!(reason.contains("f.txt"), "{reason}");
    assert!(listed(&mut engine, None, true).is_empty());
}

#[test]
fn more_than_thirty_comments_are_refused() {
    let dir = repo();
    let mut engine = engine_on(dir.path());
    let head = tip(dir.path());
    let many: Vec<ScorecardCommentInput> = (0..31)
        .map(|_| comment(1, 1, CommentLabel::Nitpick, &[]))
        .collect();
    let events = publish(
        &mut engine,
        &head,
        "security",
        summary(CommentLabel::Note, &[]),
        many,
    );
    assert!(
        failure(&events).contains("at most 30 comments"),
        "{events:?}"
    );
    assert!(listed(&mut engine, None, true).is_empty());
}

#[test]
fn a_person_publishes_no_scorecard_and_an_agent_cannot_resolve_one_of_its_comments() {
    let dir = repo();
    let mut engine = engine_on(dir.path());
    let head = tip(dir.path());
    let by_person = engine.handle(
        OperationId(5),
        &human(CommandKind::PublishScorecard {
            name: "security".into(),
            branch: "feat".into(),
            head: head.clone(),
            status: ScorecardStatus::Pass,
            summary: summary(CommentLabel::Note, &[]),
            comments: Vec::new(),
            member: None,
            workflow_run: None,
        }),
    );
    assert!(
        failure(&by_person).contains("only an agent"),
        "{by_person:?}"
    );

    let (_, ids) = published(&publish(
        &mut engine,
        &head,
        "security",
        summary(CommentLabel::Issue, &[]),
        vec![comment(2, 2, CommentLabel::Issue, &[])],
    ));
    let resolved = engine.handle(
        OperationId(6),
        &agent(CommandKind::ResolveReviewComment { id: ids[0] }),
    );
    assert!(!done(&resolved), "an agent cannot resolve");
    let handled = engine.handle(
        OperationId(7),
        &agent(CommandKind::HandleReviewComment {
            id: ids[0],
            summary: "fixed".into(),
        }),
    );
    assert!(
        done(&handled),
        "an agent may mark its comment handled: {}",
        failure(&handled)
    );
    let status = comments(&mut engine)
        .into_iter()
        .find(|c| c.id == ids[0])
        .expect("comment")
        .status;
    assert_eq!(status, ReviewStatus::Handled);
}

#[test]
fn an_agent_comment_takes_its_kind_and_severity_from_its_label() {
    let dir = repo();
    let mut engine = engine_on(dir.path());
    let create =
        |kind: Option<ReviewKind>, label: Option<CommentLabel>, decorations: Vec<String>| {
            agent(CommandKind::CreateAgentReviewComment {
                path: "f.txt".into(),
                anchor_text: "four".into(),
                kind,
                body: "a remark".into(),
                severity: None,
                label,
                decorations,
            })
        };
    let todo = engine.handle(
        OperationId(8),
        &create(None, Some(CommentLabel::Todo), vec!["non-blocking".into()]),
    );
    assert!(done(&todo), "{}", failure(&todo));
    let by_label = comments(&mut engine)
        .into_iter()
        .find(|c| c.label == Some(CommentLabel::Todo))
        .expect("the todo");
    assert_eq!(by_label.kind, Some(ReviewKind::Flag));
    assert_eq!(by_label.severity, Some(ReviewSeverity::Medium));
    assert_eq!(by_label.decorations, ["non-blocking"]);

    let praise = engine.handle(
        OperationId(9),
        &create(None, Some(CommentLabel::Praise), Vec::new()),
    );
    assert!(done(&praise), "{}", failure(&praise));

    let given = engine.handle(
        OperationId(10),
        &create(
            Some(ReviewKind::Note),
            Some(CommentLabel::Issue),
            Vec::new(),
        ),
    );
    assert!(done(&given), "an explicit kind wins: {}", failure(&given));

    let neither = engine.handle(OperationId(11), &create(None, None, Vec::new()));
    assert!(
        failure(&neither).contains("a kind"),
        "{}",
        failure(&neither)
    );

    let bad = engine.handle(
        OperationId(12),
        &create(
            None,
            Some(CommentLabel::Nitpick),
            vec!["a".into(), "b".into(), "c".into(), "d".into()],
        ),
    );
    assert!(failure(&bad).contains("at most 3"), "{}", failure(&bad));
}

#[test]
fn the_list_of_scorecards_filters_by_name_and_keeps_the_newest_first() {
    let dir = repo();
    let mut engine = engine_on(dir.path());
    let head = tip(dir.path());
    let first = published(&publish(
        &mut engine,
        &head,
        "security",
        summary(CommentLabel::Note, &[]),
        Vec::new(),
    ))
    .0;
    let second = published(&publish(
        &mut engine,
        &head,
        "code-review",
        summary(CommentLabel::Note, &[]),
        Vec::new(),
    ))
    .0;
    let third = published(&publish(
        &mut engine,
        &head,
        "security",
        summary(CommentLabel::Note, &[]),
        Vec::new(),
    ))
    .0;
    let ids = |rows: Vec<Scorecard>| -> Vec<i64> { rows.iter().map(|row| row.id).collect() };
    assert_eq!(
        ids(listed(&mut engine, None, false)),
        [third.id, second.id, first.id]
    );
    assert_eq!(
        ids(listed(&mut engine, Some("security"), false)),
        [third.id, first.id]
    );
}

#[test]
fn the_recent_list_gives_every_name_its_newest_five_and_keeps_stale_rows() {
    let dir = repo();
    let mut engine = engine_on(dir.path());
    let head = tip(dir.path());
    let lint = published(&publish(
        &mut engine,
        &head,
        "lint",
        summary(CommentLabel::Note, &[]),
        Vec::new(),
    ))
    .0;
    let mut security = Vec::new();
    for _ in 0..7 {
        let events = publish(
            &mut engine,
            &head,
            "security",
            summary(CommentLabel::Note, &[]),
            Vec::new(),
        );
        // A publish is followed by the comment list, which is how the Review page refreshes.
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::ReviewCommentsListed { .. })),
            "a publish refreshes the list"
        );
        security.push(published(&events).0.id);
    }
    let recent = |engine: &mut Engine| -> Vec<Scorecard> {
        let events = engine.handle(
            OperationId(9),
            &human(CommandKind::ListRecentScorecards {
                branch: None,
                member: None,
            }),
        );
        assert!(done(&events), "recent list failed: {}", failure(&events));
        events
            .iter()
            .find_map(|event| match event {
                Event::RecentScorecardsListed { scorecards, .. } => Some(scorecards.clone()),
                _ => None,
            })
            .expect("a recent list")
    };
    let rows = recent(&mut engine);
    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let newest_five: Vec<i64> = security.iter().rev().take(5).copied().collect();
    assert_eq!(
        ids[0], lint.id,
        "the name with one old result is still listed"
    );
    assert_eq!(
        &ids[1..],
        newest_five.as_slice(),
        "five newest of security, newest first"
    );

    std::fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "second"]);
    let stale = recent(&mut engine);
    assert_eq!(stale.len(), 6, "stale rows stay in the recent list");
    assert!(stale.iter().all(|row| row.stale));
}
