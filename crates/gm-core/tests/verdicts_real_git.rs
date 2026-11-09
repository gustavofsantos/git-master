//! Verdicts and reviewers (D212) against real repositories in temporary directories: an agent
//! records a verdict through the engine, the refusals write nothing, the store keeps the last
//! five per reviewer and branch, staleness follows the branch, and the reviewer listing reads the
//! repository, the user's directory and the presets. Each test removes its directories when it
//! ends, and the user's reviewer directory is always a temporary one.

use std::path::Path;
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::{Engine, reviewers};
use gm_schema::{
    Actor, Command, CommandKind, Event, FindingInput, OperationId, OperationState, ReviewSeverity,
    ReviewStatus, Reviewer, ReviewerDirection, ReviewerSignal, ReviewerSource, ReviewerSubject,
    Verdict,
};

fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    Process::new("git")
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
        .unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A repository on `main` with `f.txt` committed (five lines), checked out on `feat`.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("f.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    git(dir.path(), &["checkout", "-q", "-b", "feat"]);
    dir
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
            name: "test-agent".to_owned(),
        },
        kind,
    }
}

/// An engine on `dir` whose user configuration directory is `user`.
fn engine_on(dir: &Path, user: &Path) -> Engine {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    engine.set_reviewer_user_dir(Some(reviewers::user_dir(user)));
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

fn finding(severity: ReviewSeverity, path: &str, start: u32, end: u32, body: &str) -> FindingInput {
    FindingInput {
        severity,
        path: path.to_owned(),
        start_line: start,
        end_line: end,
        body: body.to_owned(),
    }
}

fn record_kind(
    reviewer: &str,
    branch: &str,
    head: &str,
    value: &str,
    rationale: &str,
    findings: Vec<FindingInput>,
) -> CommandKind {
    CommandKind::RecordVerdict {
        reviewer: reviewer.to_owned(),
        branch: branch.to_owned(),
        head: head.to_owned(),
        value: value.to_owned(),
        rationale: rationale.to_owned(),
        findings,
    }
}

/// Records as the agent and returns the events.
fn record(engine: &mut Engine, id: u64, kind: CommandKind) -> Vec<Event> {
    engine.handle(OperationId(id), &agent(kind))
}

fn verdicts_of(engine: &mut Engine, id: u64) -> Vec<Verdict> {
    let events = engine.handle(
        OperationId(id),
        &agent(CommandKind::ListVerdicts {
            branch: None,
            reviewer: None,
            limit: 20,
        }),
    );
    events
        .into_iter()
        .find_map(|event| match event {
            Event::VerdictsListed { verdicts, .. } => Some(verdicts),
            _ => None,
        })
        .expect("the listing answers")
}

/// The review comments as the engine lists them, for "nothing was written" checks.
fn comments_json(engine: &mut Engine, id: u64) -> serde_json::Value {
    let events = engine.handle(
        OperationId(id),
        &human(CommandKind::ListReviewComments {
            author: gm_schema::ReviewAuthorFilter::Any,
        }),
    );
    let comments = events
        .into_iter()
        .find_map(|event| match event {
            Event::ReviewCommentsListed { comments, .. } => Some(comments),
            _ => None,
        })
        .expect("the list answers");
    serde_json::to_value(comments).unwrap()
}

fn verdict_json(engine: &mut Engine, id: u64) -> serde_json::Value {
    serde_json::to_value(verdicts_of(engine, id)).unwrap()
}

fn recorded(events: &[Event]) -> (Verdict, Vec<i64>) {
    events
        .iter()
        .find_map(|event| match event {
            Event::VerdictRecorded {
                verdict,
                comment_ids,
                ..
            } => Some((verdict.clone(), comment_ids.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no verdict recorded in {events:?}"))
}

fn head_of(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"]).trim().to_owned()
}

fn reviewer(name: &str, description: &str, signal: ReviewerSignal) -> Reviewer {
    Reviewer {
        name: name.to_owned(),
        description: description.to_owned(),
        instructions: "Read the change and report what you find.".to_owned(),
        signal,
        direction: ReviewerDirection::HigherIsBetter,
        subject: ReviewerSubject::Branch,
        model: None,
        source: ReviewerSource::Repository,
        paths: Vec::new(),
        branches: Vec::new(),
    }
}

#[test]
fn an_agent_records_a_verdict_with_two_findings_on_the_checked_out_branch() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    let head = head_of(dir.path());

    let events = record(
        &mut engine,
        2,
        record_kind(
            "security",
            "feat",
            "HEAD",
            "High",
            "Two lines are unchecked.",
            vec![
                finding(
                    ReviewSeverity::High,
                    "f.txt",
                    2,
                    2,
                    "the input is not checked",
                ),
                finding(
                    ReviewSeverity::Low,
                    "f.txt",
                    4,
                    5,
                    "the tail is not reviewed",
                ),
            ],
        ),
    );
    assert!(done(&events), "{events:?}");
    let (verdict, comment_ids) = recorded(&events);
    assert_eq!(verdict.reviewer, "security");
    assert_eq!(verdict.branch, "feat");
    assert_eq!(verdict.head, head, "HEAD resolves to the commit");
    assert_eq!(verdict.value, "high", "a level is stored in lower case");
    assert_eq!(verdict.origin, "agent:test-agent");
    assert!(!verdict.stale);
    assert_eq!(comment_ids.len(), 2);
    assert_eq!(verdict.findings.len(), 2);
    assert_eq!(verdict.findings[0].comment_id, comment_ids[0]);

    let comments = comments_json(&mut engine, 3);
    let first = comments
        .as_array()
        .unwrap()
        .iter()
        .find(|comment| comment["id"] == comment_ids[0])
        .expect("the first finding is listed");
    assert_eq!(first["kind"], "flag");
    assert_eq!(first["severity"], "high");
    assert_eq!(first["author"], "agent:test-agent");
    assert_eq!(first["status"], "open");
    assert_eq!(first["anchor"]["text"], "two");
    assert_eq!(first["location"]["start_line"], 2);
    assert_eq!(first["location"]["end_line"], 2);
    let second = comments
        .as_array()
        .unwrap()
        .iter()
        .find(|comment| comment["id"] == comment_ids[1])
        .expect("the second finding is listed");
    assert_eq!(second["anchor"]["text"], "four\nfive");
    assert_eq!(second["location"]["start_line"], 4);
    assert_eq!(second["location"]["end_line"], 5);
}

#[test]
fn a_person_cannot_record_a_verdict() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    let events = engine.handle(
        OperationId(2),
        &human(record_kind(
            "security",
            "feat",
            "HEAD",
            "low",
            "fine",
            Vec::new(),
        )),
    );
    assert!(
        failure(&events).contains("only an agent records a verdict"),
        "{events:?}"
    );
    assert!(verdicts_of(&mut engine, 3).is_empty());
}

/// The repository with a reviewer whose signal is a pass, which no preset has.
fn repo_with_a_pass_reviewer() -> tempfile::TempDir {
    let dir = repo();
    reviewers::save(
        &reviewers::repo_dir(&dir.path().join(".git")),
        &reviewer("gate", "A pass gate", ReviewerSignal::Pass),
    )
    .unwrap();
    dir
}

/// Records each case as the agent and checks that it is refused for its reason.
fn assert_refused(engine: &mut Engine, first_id: u64, cases: Vec<(&str, CommandKind, &str)>) {
    for (index, (case, kind, reason)) in cases.into_iter().enumerate() {
        let events = record(engine, first_id + index as u64, kind);
        let said = failure(&events);
        assert!(
            said.contains(reason),
            "{case}: {said:?} does not say {reason:?}"
        );
    }
}

#[test]
fn a_record_naming_a_bad_reviewer_branch_head_or_value_writes_nothing() {
    let dir = repo_with_a_pass_reviewer();
    let user = tempfile::tempdir().unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    let verdicts_before = verdict_json(&mut engine, 2);
    let comments_before = comments_json(&mut engine, 3);
    let cases: Vec<(&str, CommandKind, &str)> = vec![
        (
            "unknown reviewer",
            record_kind("nope", "feat", "HEAD", "low", "x", Vec::new()),
            "there is no reviewer named \"nope\"",
        ),
        (
            "another branch",
            record_kind("security", "main", "HEAD", "low", "x", Vec::new()),
            "is not checked out here",
        ),
        (
            "a commit that is not there",
            record_kind("security", "feat", "no-such-commit", "low", "x", Vec::new()),
            "is not a commit",
        ),
        (
            "a level that is not one",
            record_kind("security", "feat", "HEAD", "critical", "x", Vec::new()),
            "a level is low, medium or high",
        ),
        (
            "a score past 100",
            record_kind("code-review", "feat", "HEAD", "101", "x", Vec::new()),
            "a score is a whole number from 0 to 100",
        ),
        (
            "a pass that is neither pass nor fail",
            record_kind("gate", "feat", "HEAD", "maybe", "x", Vec::new()),
            "a pass is pass or fail",
        ),
    ];
    assert_refused(&mut engine, 100, cases);
    assert_eq!(
        verdict_json(&mut engine, 50),
        verdicts_before,
        "no verdict was written"
    );
    assert_eq!(
        comments_json(&mut engine, 51),
        comments_before,
        "no comment was written"
    );
}

#[test]
fn a_record_with_a_bad_rationale_or_findings_writes_nothing() {
    let dir = repo_with_a_pass_reviewer();
    let user = tempfile::tempdir().unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    let verdicts_before = verdict_json(&mut engine, 2);
    let comments_before = comments_json(&mut engine, 3);
    let long_rationale = "r".repeat(4_001);
    let long_body = "b".repeat(1_001);
    let fine = || finding(ReviewSeverity::Low, "f.txt", 1, 1, "fine");
    let cases: Vec<(&str, CommandKind, &str)> = vec![
        (
            "an empty rationale",
            record_kind("security", "feat", "HEAD", "low", "   ", Vec::new()),
            "the rationale is empty",
        ),
        (
            "a rationale of 4,001 characters",
            record_kind(
                "security",
                "feat",
                "HEAD",
                "low",
                &long_rationale,
                Vec::new(),
            ),
            "at most 4000 are allowed",
        ),
        (
            "lines past the end of the file",
            record_kind(
                "security",
                "feat",
                "HEAD",
                "low",
                "x",
                vec![finding(ReviewSeverity::Low, "f.txt", 9, 10, "gone")],
            ),
            "lines 9-10 are not in the file",
        ),
        (
            "a file that is not there",
            record_kind(
                "security",
                "feat",
                "HEAD",
                "low",
                "x",
                vec![finding(ReviewSeverity::Low, "missing.txt", 1, 1, "gone")],
            ),
            "there is no file missing.txt",
        ),
        (
            "an empty finding body",
            record_kind(
                "security",
                "feat",
                "HEAD",
                "low",
                "x",
                vec![finding(ReviewSeverity::Low, "f.txt", 1, 1, "  ")],
            ),
            "a finding's body is empty",
        ),
        (
            "a finding body of 1,001 characters",
            record_kind(
                "security",
                "feat",
                "HEAD",
                "low",
                "x",
                vec![finding(ReviewSeverity::Low, "f.txt", 1, 1, &long_body)],
            ),
            "at most 1000 are allowed",
        ),
        (
            "31 findings",
            record_kind("security", "feat", "HEAD", "low", "x", vec![fine(); 31]),
            "at most 30 findings, not 31",
        ),
    ];
    assert_refused(&mut engine, 200, cases);
    assert_eq!(
        verdict_json(&mut engine, 250),
        verdicts_before,
        "no verdict was written"
    );
    assert_eq!(
        comments_json(&mut engine, 251),
        comments_before,
        "no comment was written"
    );
}

#[test]
fn a_branch_that_is_not_checked_out_is_refused_and_so_is_a_detached_head() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    git(dir.path(), &["checkout", "-q", "--detach"]);
    let events = record(
        &mut engine,
        2,
        record_kind("security", "feat", "HEAD", "low", "x", Vec::new()),
    );
    assert!(failure(&events).contains("HEAD is detached"), "{events:?}");
}

#[test]
fn a_second_verdict_keeps_both_and_a_sixth_drops_the_oldest() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    for run in 1..=2 {
        let events = record(
            &mut engine,
            2 + run,
            record_kind(
                "security",
                "feat",
                "HEAD",
                "low",
                &format!("run {run}"),
                Vec::new(),
            ),
        );
        assert!(done(&events), "{events:?}");
    }
    let kept = verdicts_of(&mut engine, 10);
    assert_eq!(kept.len(), 2, "two verdicts of one reviewer are both kept");
    assert_eq!(kept[0].rationale, "run 2", "newest first");

    for run in 3..=6 {
        let events = record(
            &mut engine,
            10 + run,
            record_kind(
                "security",
                "feat",
                "HEAD",
                "low",
                &format!("run {run}"),
                Vec::new(),
            ),
        );
        assert!(done(&events), "{events:?}");
    }
    let kept = verdicts_of(&mut engine, 30);
    let rationales: Vec<&str> = kept
        .iter()
        .map(|verdict| verdict.rationale.as_str())
        .collect();
    assert_eq!(rationales, ["run 6", "run 5", "run 4", "run 3", "run 2"]);
}

#[test]
fn a_verdict_is_stale_once_the_branch_moves_and_a_verdict_at_the_new_tip_is_not() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    let first = record(
        &mut engine,
        2,
        record_kind("security", "feat", "HEAD", "low", "before", Vec::new()),
    );
    assert!(done(&first), "{first:?}");

    std::fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "change two"]);

    let stale = verdicts_of(&mut engine, 3);
    assert!(stale[0].stale, "the verdict at the old tip is stale");

    let second = record(
        &mut engine,
        4,
        record_kind("security", "feat", "HEAD", "low", "after", Vec::new()),
    );
    assert!(done(&second), "{second:?}");
    let now = verdicts_of(&mut engine, 5);
    let after = now
        .iter()
        .find(|verdict| verdict.rationale == "after")
        .unwrap();
    let before = now
        .iter()
        .find(|verdict| verdict.rationale == "before")
        .unwrap();
    assert!(!after.stale, "a verdict at the new tip is current");
    assert!(before.stale, "the verdict at the old tip stays stale");
}

#[test]
fn a_verdict_is_stale_when_its_branch_is_gone() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    let events = record(
        &mut engine,
        2,
        record_kind("security", "feat", "HEAD", "low", "x", Vec::new()),
    );
    assert!(done(&events), "{events:?}");
    git(dir.path(), &["checkout", "-q", "main"]);
    git(dir.path(), &["branch", "-q", "-D", "feat"]);
    let listed = verdicts_of(&mut engine, 3);
    assert!(listed[0].stale, "a verdict of a deleted branch is stale");
}

#[test]
fn the_listing_filters_by_reviewer_and_takes_a_limit() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    for (reviewer, run) in [("security", 1), ("code-review", 2), ("security", 3)] {
        let events = record(
            &mut engine,
            1 + run,
            record_kind(
                reviewer,
                "feat",
                "HEAD",
                if reviewer == "security" { "low" } else { "70" },
                &format!("run {run}"),
                Vec::new(),
            ),
        );
        assert!(done(&events), "{events:?}");
    }
    let only_security = engine.handle(
        OperationId(20),
        &agent(CommandKind::ListVerdicts {
            branch: Some("feat".into()),
            reviewer: Some("security".into()),
            limit: 0,
        }),
    );
    let listed = only_security
        .into_iter()
        .find_map(|event| match event {
            Event::VerdictsListed { verdicts, .. } => Some(verdicts),
            _ => None,
        })
        .unwrap();
    let rationales: Vec<&str> = listed
        .iter()
        .map(|verdict| verdict.rationale.as_str())
        .collect();
    assert_eq!(rationales, ["run 3", "run 1"]);
}

#[test]
fn the_reviewer_list_shows_the_repository_the_user_and_the_presets() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join(".git");
    reviewers::save(
        &reviewers::repo_dir(&git_dir),
        &reviewer("style", "Repository style", ReviewerSignal::Score),
    )
    .unwrap();
    reviewers::save(
        &reviewers::repo_dir(&git_dir),
        &reviewer("security", "Repository security", ReviewerSignal::Level),
    )
    .unwrap();
    let mut user_copy = reviewer("lint", "User lint", ReviewerSignal::Pass);
    user_copy.source = ReviewerSource::User;
    reviewers::save(&reviewers::user_dir(user.path()), &user_copy).unwrap();
    std::fs::write(
        reviewers::repo_dir(&git_dir).join("broken.md"),
        "no front matter here",
    )
    .unwrap();
    let mut engine = engine_on(dir.path(), user.path());

    let events = engine.handle(
        OperationId(2),
        &human(CommandKind::ListReviewers { name: None }),
    );
    let (listed, errors) = events
        .into_iter()
        .find_map(|event| match event {
            Event::ReviewersListed {
                reviewers, errors, ..
            } => Some((reviewers, errors)),
            _ => None,
        })
        .expect("the listing answers");
    let find = |name: &str| {
        listed
            .iter()
            .find(|reviewer| reviewer.name == name)
            .unwrap()
    };
    assert_eq!(find("style").source, ReviewerSource::Repository);
    assert_eq!(find("lint").source, ReviewerSource::User);
    assert_eq!(find("code-review").source, ReviewerSource::Preset);
    let security = find("security");
    assert_eq!(
        security.source,
        ReviewerSource::Repository,
        "the repository file wins"
    );
    assert_eq!(security.description, "Repository security");
    assert_eq!(
        listed
            .iter()
            .filter(|reviewer| reviewer.name == "security")
            .count(),
        1
    );
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].file, "broken.md");

    let one = engine.handle(
        OperationId(3),
        &human(CommandKind::ListReviewers {
            name: Some("security".into()),
        }),
    );
    let names: Vec<String> = one
        .into_iter()
        .find_map(|event| match event {
            Event::ReviewersListed { reviewers, .. } => Some(reviewers),
            _ => None,
        })
        .unwrap()
        .into_iter()
        .map(|reviewer| reviewer.name)
        .collect();
    assert_eq!(names, ["security"]);
}

#[test]
fn the_verdict_list_names_its_findings_with_their_status_now() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    let events = record(
        &mut engine,
        2,
        record_kind(
            "security",
            "feat",
            "HEAD",
            "medium",
            "one finding",
            vec![finding(ReviewSeverity::Medium, "f.txt", 3, 3, "three")],
        ),
    );
    let (_, comment_ids) = recorded(&events);
    let listed = verdicts_of(&mut engine, 3);
    assert_eq!(listed[0].findings.len(), 1);
    assert_eq!(listed[0].findings[0].comment_id, comment_ids[0]);
    assert_eq!(listed[0].findings[0].status, ReviewStatus::Open);
    assert_eq!(listed[0].findings[0].severity, ReviewSeverity::Medium);
    assert_eq!(listed[0].findings[0].path, "f.txt");
}

#[test]
fn a_head_that_is_not_the_tip_is_refused_and_nothing_is_written() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    let old = head_of(dir.path());
    std::fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "change two"]);
    let verdicts_before = verdict_json(&mut engine, 2);
    let comments_before = comments_json(&mut engine, 3);

    let events = record(
        &mut engine,
        4,
        record_kind(
            "security",
            "feat",
            &old,
            "low",
            "judged the old tip",
            Vec::new(),
        ),
    );
    let said = failure(&events);
    assert!(
        said.contains(&format!("head {} is not the tip of feat", &old[..7])),
        "{said}"
    );
    assert!(said.contains("run the reviewer again"), "{said}");
    assert_eq!(
        verdict_json(&mut engine, 5),
        verdicts_before,
        "no verdict was written"
    );
    assert_eq!(
        comments_json(&mut engine, 6),
        comments_before,
        "no comment was written"
    );

    let at_tip = record(
        &mut engine,
        7,
        record_kind(
            "security",
            "feat",
            "HEAD",
            "low",
            "judged the tip",
            Vec::new(),
        ),
    );
    assert!(done(&at_tip), "{at_tip:?}");
}

#[test]
fn a_secret_in_the_rationale_or_a_finding_is_stored_redacted() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    // Built from pieces so that no secret sits in the source (an AWS access key id shape).
    let secret = format!("AKIA{}", "ABCDEFGHIJKLMNOP");
    let events = record(
        &mut engine,
        2,
        record_kind(
            "security",
            "feat",
            "HEAD",
            "low",
            &format!("the key {secret} is in the rationale"),
            vec![finding(
                ReviewSeverity::High,
                "f.txt",
                2,
                2,
                &format!("the key {secret} is in the body"),
            )],
        ),
    );
    assert!(done(&events), "{events:?}");
    let (verdict, comment_ids) = recorded(&events);
    assert!(
        !verdict.rationale.contains(&secret),
        "{}",
        verdict.rationale
    );
    assert!(
        verdict.rationale.contains("rationale"),
        "the rest of the text stays"
    );
    let comments = comments_json(&mut engine, 3).to_string();
    assert!(!comments.contains(&secret), "the comment kept the secret");
    assert!(
        comments.contains("in the body"),
        "the rest of the body stays"
    );
    assert_eq!(comment_ids.len(), 1);
    assert!(
        !serde_json::to_string(&verdicts_of(&mut engine, 4))
            .unwrap()
            .contains(&secret)
    );
}

#[test]
fn a_broken_redaction_pattern_refuses_the_record_and_writes_nothing() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    git(
        dir.path(),
        &["config", "gitmaster-redact.broken.regex", "("],
    );
    let mut engine = engine_on(dir.path(), user.path());
    let events = record(
        &mut engine,
        2,
        record_kind("security", "feat", "HEAD", "low", "fine", Vec::new()),
    );
    let said = failure(&events);
    assert!(said.contains("redaction is misconfigured"), "{said}");
    assert!(
        said.contains("broken"),
        "the reason names the pattern: {said}"
    );
    assert!(
        verdicts_of(&mut engine, 3).is_empty(),
        "no verdict was written"
    );
}

#[test]
fn a_reviewer_of_another_subject_is_refused_and_nothing_is_written() {
    let dir = repo();
    let user = tempfile::tempdir().unwrap();
    let mut staged = reviewer(
        "staged-check",
        "Judges the staged diff",
        ReviewerSignal::Level,
    );
    staged.subject = ReviewerSubject::Staged;
    reviewers::save(&reviewers::repo_dir(&dir.path().join(".git")), &staged).unwrap();
    let mut engine = engine_on(dir.path(), user.path());
    let events = record(
        &mut engine,
        2,
        record_kind("staged-check", "feat", "HEAD", "low", "x", Vec::new()),
    );
    assert_eq!(
        failure(&events),
        "reviewer staged-check has subject staged; only branch reviewers can record verdicts for now"
    );
    assert!(verdicts_of(&mut engine, 3).is_empty());
}
