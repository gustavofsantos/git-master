//! Script suggestions in the conflict resolver (design/workflow-scripts.md, `WF3d`, D168) end to
//! end: real repositories in temporary directories and real Lua workflows. This harness plays the
//! engine thread that carries a run's effects and events, as the runner does (the harness of
//! `script_transforms_real_git.rs`).

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::script::run::ScriptEvent;
use gm_core::script::trust::TrustStore;
use gm_core::{EffectRequest, Engine, ScriptLink};
use gm_schema::{
    Actor, Command, CommandKind, ConflictSite, Event, HunkOrigin, OperationId, OperationState,
    WorkflowRunStatus,
};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Runs git without asserting its exit code: a merge is meant to stop on its conflicts.
fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    Process::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "user.email", "t@t"]);
}

/// A repository on `main` where `git merge feature` stops with two conflicts in `a.txt`, one
/// hunk per changed line: `main` and `feature` both change `x` and `y`, with `middle` between.
fn merge_stopped() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    std::fs::write(repo.join("a.txt"), "top\nx\nmiddle\ny\nend\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    git(&repo, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo.join("a.txt"), "top\nFEAT-x\nmiddle\nFEAT-y\nend\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "feature"]);
    git(&repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("a.txt"), "top\nMAIN-x\nmiddle\nMAIN-y\nend\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "main"]);
    let merged = git_output(&repo, &["merge", "--no-edit", "feature"]);
    assert!(!merged.status.success(), "the merge should conflict");
    let flows = dir.path().join("flows");
    std::fs::create_dir_all(&flows).unwrap();
    (dir, repo, flows)
}

/// A script that runs `body` for the workflow `name` on `on`. A hunk's `body` returns its suggestion.
fn phase(name: &str, on: &str, body: &str) -> String {
    format!(
        "local M = {{}}\nM.meta = {{ name = \"{name}\", trigger = {{ on = \"{on}\" }} }}\n\
         function M.execute(ctx, input)\n{body}\nend\nreturn M\n"
    )
}

fn hunk_script(name: &str, body: &str) -> String {
    phase(name, "transform.conflict_hunk", body)
}

fn write_script(flows: &Path, file: &str, source: &str) {
    std::fs::write(flows.join(file), source).unwrap();
}

enum Message {
    Effect(EffectRequest),
    Event(ScriptEvent),
}

struct Harness {
    engine: Engine,
    messages: Receiver<Message>,
    last: u64,
}

impl Harness {
    /// Opens `repo` with the workflows of `flows`. `triggers` is the switch the app turns on.
    fn new(repo: &Path, flows: &Path, triggers: bool) -> Self {
        let (tx, messages) = mpsc::channel();
        let effects_tx = Mutex::new(tx.clone());
        let events_tx = Mutex::new(tx);
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_workflow_user_dir(Some(flows.to_path_buf()));
        engine.set_trust_store(TrustStore::open_in_memory().unwrap());
        engine.set_script_link(ScriptLink {
            effects: Arc::new(move |request| {
                let _ = effects_tx.lock().unwrap().send(Message::Effect(request));
            }),
            events: Arc::new(move |event| {
                let _ = events_tx.lock().unwrap().send(Message::Event(event));
            }),
        });
        engine.set_triggers(triggers);
        let mut harness = Self {
            engine,
            messages,
            last: 0,
        };
        let (id, events) = harness.handle(CommandKind::OpenRepo {
            path: repo.display().to_string(),
        });
        assert_eq!(
            final_state(&events, id).map(|(state, _)| state),
            Some(OperationState::Done),
            "open: {events:?}"
        );
        harness
    }

    /// Runs a command as the person does, and returns its id with the events so far.
    fn handle(&mut self, kind: CommandKind) -> (OperationId, Vec<Event>) {
        self.last += 1;
        let id = OperationId(self.last);
        let events = self.engine.handle(
            id,
            &Command {
                actor: Actor::Human,
                kind,
            },
        );
        (id, events)
    }

    /// Plays the engine thread until `done` says the events so far are enough, or panics.
    fn pump_until(
        &mut self,
        mut events: Vec<Event>,
        done: impl Fn(&[Event]) -> bool,
    ) -> Vec<Event> {
        let end = Instant::now() + Duration::from_secs(30);
        while !done(&events) {
            assert!(Instant::now() < end, "timed out; events: {events:#?}");
            events.extend(self.step(Duration::from_millis(20)));
        }
        events
    }

    /// Plays the engine thread for `span` and returns what came in.
    fn settle(&mut self, span: Duration) -> Vec<Event> {
        let end = Instant::now() + span;
        let mut events = Vec::new();
        while Instant::now() < end {
            events.extend(self.step(Duration::from_millis(20)));
        }
        events
    }

    fn step(&mut self, wait: Duration) -> Vec<Event> {
        match self.messages.recv_timeout(wait) {
            Ok(Message::Effect(request)) => self.engine.script_effect(request),
            Ok(Message::Event(event)) => self.engine.script_event(event),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => Vec::new(),
        }
    }

    /// Reviews the workflow `name` as the listing shows it.
    fn review(&mut self, name: &str) {
        let (_, events) = self.handle(CommandKind::ListWorkflows);
        let hash = events
            .iter()
            .find_map(|e| match e {
                Event::WorkflowsListed { workflows, .. } => workflows
                    .iter()
                    .find(|w| w.name == name)
                    .map(|w| w.hash.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no workflow named {name}"));
        let (_, events) = self.handle(CommandKind::ReviewWorkflow {
            name: name.to_owned(),
            hash,
            auto_approve: false,
        });
        assert!(failure(&events).is_none(), "review refused: {events:#?}");
    }

    /// Reads `a.txt` of the checkout (or of the restack's resolution worktree).
    fn read(&mut self, site: ConflictSite) -> (OperationId, Vec<Event>) {
        self.handle(CommandKind::ReadConflict {
            path: "a.txt".into(),
            site,
        })
    }
}

/// The last terminal state of the operation `id`, with its detail.
fn final_state(events: &[Event], id: OperationId) -> Option<(OperationState, Option<String>)> {
    events.iter().rev().find_map(|e| match e {
        Event::OperationChanged {
            id: found,
            state,
            detail,
            ..
        } if *found == id && state.is_terminal() => Some((*state, detail.clone())),
        _ => None,
    })
}

fn failure(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::OperationChanged {
            state: OperationState::Failed,
            detail,
            ..
        } => Some(detail.clone().unwrap_or_default()),
        _ => None,
    })
}

/// The suggestions `read` got, in the order they came: index, text and workflow.
fn suggestions(events: &[Event], read: OperationId) -> Vec<(usize, String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::HunkSuggested {
                read: got,
                index,
                text,
                workflow,
                ..
            } if *got == read => Some((*index, text.clone(), workflow.clone())),
            _ => None,
        })
        .collect()
}

/// The workflows asked for review, by name, with the trigger each one names.
fn review_requests(events: &[Event]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowWantsReview {
                workflow, trigger, ..
            } => Some((workflow.clone(), trigger.clone())),
            _ => None,
        })
        .collect()
}

/// How many notices say exactly `text`.
fn notices(events: &[Event], text: &str) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::WorkflowNotice { text: got, .. } if got == text))
        .count()
}

fn runs_started(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::WorkflowRunChanged {
                    status: WorkflowRunStatus::Running,
                    ..
                }
            )
        })
        .count()
}

/// Reads the file with `join`, reviewed, and returns the suggestions once `count` of them came.
fn read_until(h: &mut Harness, site: ConflictSite, count: usize) -> (OperationId, Vec<Event>) {
    let (id, events) = h.read(site);
    let events = h.pump_until(events, |events| suggestions(events, id).len() >= count);
    (id, events)
}

#[test]
fn a_merge_with_two_hunks_gets_one_suggestion_per_hunk() {
    let (_dir, repo, flows) = merge_stopped();
    write_script(
        &flows,
        "join.lua",
        &hunk_script("join", "return input.trigger.left .. input.trigger.right"),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("join");

    let (id, events) = read_until(&mut h, ConflictSite::WorkingCopy, 2);
    assert_eq!(
        suggestions(&events, id),
        vec![
            (0, "MAIN-x\nFEAT-x\n".to_owned(), "join".to_owned()),
            (1, "MAIN-y\nFEAT-y\n".to_owned(), "join".to_owned()),
        ],
        "each hunk gets the sides it saw, in file order"
    );
}

#[test]
fn the_trigger_names_the_file_the_index_and_the_labels_of_the_read() {
    let (_dir, repo, flows) = merge_stopped();
    write_script(
        &flows,
        "names.lua",
        &hunk_script(
            "names",
            "return input.trigger.on .. \" \" .. input.trigger.field .. \" \" .. input.trigger.path .. \" \" .. input.trigger.index .. \" \" .. input.trigger.left_label .. \" \" .. input.trigger.right_label .. \" \" .. input.trigger.current",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("names");

    let (id, events) = read_until(&mut h, ConflictSite::WorkingCopy, 2);
    let texts: Vec<String> = suggestions(&events, id).into_iter().map(|s| s.1).collect();
    assert_eq!(
        texts,
        vec![
            "transform.conflict_hunk conflict_hunk a.txt 0 main feature ".to_owned(),
            "transform.conflict_hunk conflict_hunk a.txt 1 main feature ".to_owned(),
        ]
    );
}

#[test]
fn two_workflows_fold_in_file_order_and_the_second_sees_the_first() {
    let (_dir, repo, flows) = merge_stopped();
    write_script(
        &flows,
        "a-first.lua",
        &hunk_script("a-first", "return \"one:\" .. input.trigger.left"),
    );
    write_script(
        &flows,
        "b-second.lua",
        &hunk_script("b-second", "return input.trigger.current .. \"|two\""),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("a-first");
    h.review("b-second");

    let (id, events) = read_until(&mut h, ConflictSite::WorkingCopy, 2);
    assert_eq!(
        suggestions(&events, id),
        vec![
            (0, "one:MAIN-x\n|two".to_owned(), "b-second".to_owned()),
            (1, "one:MAIN-y\n|two".to_owned(), "b-second".to_owned()),
        ]
    );
}

#[test]
fn a_nil_for_the_first_hunk_gives_a_suggestion_for_the_second_only() {
    let (_dir, repo, flows) = merge_stopped();
    write_script(
        &flows,
        "skip.lua",
        &hunk_script(
            "skip",
            "if input.trigger.index == 0 then return nil end\nreturn \"second \" .. input.trigger.index",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("skip");

    let (id, events) = read_until(&mut h, ConflictSite::WorkingCopy, 1);
    let events = [events, h.settle(Duration::from_millis(400))].concat();
    assert_eq!(
        suggestions(&events, id),
        vec![(1, "second 1".to_owned(), "skip".to_owned())]
    );
}

#[test]
fn an_unreviewed_workflow_is_asked_for_once_and_runs_nothing_until_the_file_is_reopened() {
    let (_dir, repo, flows) = merge_stopped();
    write_script(
        &flows,
        "join.lua",
        &hunk_script("join", "return input.trigger.left .. input.trigger.right"),
    );
    let mut h = Harness::new(&repo, &flows, true);

    let (id, events) = h.read(ConflictSite::WorkingCopy);
    let events = [events, h.settle(Duration::from_millis(400))].concat();
    assert_eq!(
        review_requests(&events),
        vec![("join".to_owned(), "transform.conflict_hunk".to_owned())],
        "one request for two hunks"
    );
    assert!(suggestions(&events, id).is_empty());
    assert_eq!(runs_started(&events), 0);

    h.review("join");
    let (id, events) = read_until(&mut h, ConflictSite::WorkingCopy, 2);
    assert_eq!(
        suggestions(&events, id).len(),
        2,
        "reopening the file runs it"
    );
}

#[test]
fn a_switched_off_workflow_does_not_run_here() {
    let (_dir, repo, flows) = merge_stopped();
    write_script(
        &flows,
        "join.lua",
        &hunk_script("join", "return input.trigger.left"),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("join");
    let (_, events) = h.handle(CommandKind::SetWorkflowEnabled {
        name: "join".into(),
        enabled: false,
    });
    assert!(failure(&events).is_none(), "{events:#?}");

    let (id, events) = h.read(ConflictSite::WorkingCopy);
    let events = [events, h.settle(Duration::from_millis(400))].concat();
    assert!(suggestions(&events, id).is_empty());
    assert!(review_requests(&events).is_empty());
    assert_eq!(runs_started(&events), 0);
}

#[test]
fn a_new_read_cancels_a_waiting_script_and_its_late_answer_never_arrives() {
    let (_dir, repo, flows) = merge_stopped();
    write_script(
        &flows,
        "slow.lua",
        &hunk_script(
            "slow",
            "ctx.wait(\"restack.resumed\", { timeout = \"1s\" })\nctx.notify(\"resumed \" .. input.trigger.index)\nreturn \"late \" .. input.trigger.index",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("slow");

    let (first, events) = h.read(ConflictSite::WorkingCopy);
    let (second, read_again) = h.read(ConflictSite::WorkingCopy);
    assert_ne!(first, second);
    let events = [events, read_again].concat();
    let events = [events, h.settle(Duration::from_millis(2500))].concat();
    assert!(
        suggestions(&events, first).is_empty(),
        "the superseded read sends nothing, even after its wait ended"
    );
    assert_eq!(
        suggestions(&events, second).len(),
        2,
        "the new read answers"
    );
    // The superseded run was cancelled: it never got past its wait. Only the new read's run did.
    assert_eq!(notices(&events, "resumed 0"), 1, "{events:#?}");
}

#[test]
fn resolving_the_file_cancels_its_runs_and_their_answers_are_dropped() {
    let (_dir, repo, flows) = merge_stopped();
    write_script(
        &flows,
        "slow.lua",
        &hunk_script(
            "slow",
            "ctx.wait(\"restack.resumed\", { timeout = \"1s\" })\nreturn \"late\"",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("slow");

    let (read, events) = h.read(ConflictSite::WorkingCopy);
    let (_, resolved) = h.handle(CommandKind::ResolveConflict {
        path: "a.txt".into(),
        text: "resolved\n".into(),
        origins: vec![HunkOrigin::Human],
        site: ConflictSite::WorkingCopy,
    });
    assert!(failure(&resolved).is_none(), "{resolved:#?}");
    let events = [events, resolved, h.settle(Duration::from_millis(2000))].concat();
    assert!(suggestions(&events, read).is_empty());
}

#[test]
fn with_the_triggers_off_no_workflow_runs_on_a_read() {
    let (_dir, repo, flows) = merge_stopped();
    write_script(
        &flows,
        "join.lua",
        &hunk_script("join", "return input.trigger.left"),
    );
    let mut h = Harness::new(&repo, &flows, false);
    h.review("join");

    let (id, events) = h.read(ConflictSite::WorkingCopy);
    let events = [events, h.settle(Duration::from_millis(400))].concat();
    assert!(suggestions(&events, id).is_empty());
    assert_eq!(runs_started(&events), 0);
    assert!(review_requests(&events).is_empty());
}

/// A repository on `main` with a child branch `a` of it, where `a` and `main` both change `a.txt`
/// so that replaying `a` onto `main` conflicts. Returns the directory, the root and the flows.
fn restack_conflict(h: &mut Harness, repo: &Path) {
    let (_, events) = h.handle(CommandKind::CreateChildBranch {
        name: "a".into(),
        parent: "main".into(),
        checkout: false,
    });
    assert!(failure(&events).is_none(), "{events:#?}");
    git(repo, &["checkout", "-q", "a"]);
    std::fs::write(repo.join("a.txt"), "from a\n").unwrap();
    git(repo, &["commit", "-q", "-am", "a.txt: from a"]);
    git(repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("a.txt"), "from main\n").unwrap();
    git(repo, &["commit", "-q", "-am", "a.txt: from main"]);
}

#[test]
fn a_restack_conflict_gets_suggestions_too() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let flows = dir.path().join("flows");
    std::fs::create_dir_all(&flows).unwrap();
    write_script(
        &flows,
        "join.lua",
        &hunk_script("join", "return input.trigger.left .. input.trigger.right"),
    );
    let mut h = Harness::new(&repo, &flows, true);
    restack_conflict(&mut h, &repo);
    h.review("join");

    let (_, events) = h.handle(CommandKind::PlanRestack {
        branch: "main".into(),
        skip: Vec::new(),
    });
    let plan_id = events
        .iter()
        .find_map(|e| match e {
            Event::RestackPlanned { plan, .. } => Some(plan.plan_id.clone()),
            _ => None,
        })
        .expect("a plan");
    let (_, events) = h.handle(CommandKind::StartRestackResolution {
        branch: "main".into(),
        plan_id,
        skip: Vec::new(),
        conflicted: "a".into(),
    });
    assert!(failure(&events).is_none(), "{events:#?}");

    let (id, events) = read_until(&mut h, ConflictSite::Restack, 1);
    assert_eq!(
        suggestions(&events, id),
        vec![(0, "from main\nfrom a\n".to_owned(), "join".to_owned())]
    );
}

#[test]
fn a_resolution_with_a_script_origin_is_in_the_operation_log() {
    let (_dir, repo, _flows) = merge_stopped();
    let log = CommandLog::default();
    let git = SubprocessGit {
        log: Some(log),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    let events = engine.handle(
        OperationId(1),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::OpenRepo {
                path: repo.display().to_string(),
            },
        },
    );
    assert!(final_state(&events, OperationId(1)).is_some());

    let events = engine.handle(
        OperationId(2),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::ResolveConflict {
                path: "a.txt".into(),
                text: "top\nmerged\nmiddle\nmerged y\nend\n".into(),
                origins: vec![HunkOrigin::Script, HunkOrigin::Script],
                site: ConflictSite::WorkingCopy,
            },
        },
    );
    assert_eq!(failure(&events), None, "{events:#?}");
    assert!(engine.log().iter().any(|entry| matches!(
        &entry.kind,
        CommandKind::ResolveConflict { origins, .. } if *origins == [HunkOrigin::Script, HunkOrigin::Script]
    )));
}
