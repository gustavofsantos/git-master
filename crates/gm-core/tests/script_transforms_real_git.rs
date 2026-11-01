//! Transforms of field values (design/workflow-scripts.md, `WF3d`, D155) end to end: real
//! repositories in temporary directories and real Lua workflows. This harness plays the engine
//! thread that carries a run's effects and events, as the runner does. The parity checks run the
//! fake programs of the `prepare-commit` hooks as transforms and compare the message with the one
//! the hook gave (the hook path is gone since `WF3e`; the messages are the literals it gave).

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gm_core::git::SubprocessGit;
use gm_core::script::run::ScriptEvent;
use gm_core::script::trust::TrustStore;
use gm_core::{EffectRequest, Engine, ScriptLink};
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, WorkflowRunStatus,
};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
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

fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "user.email", "t@t"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

/// A script that runs `body` for the workflow `name` on `on`. A transform's `body` returns its
/// value.
fn phase(name: &str, on: &str, body: &str) -> String {
    format!(
        "local M = {{}}\nM.meta = {{ name = \"{name}\", trigger = {{ on = \"{on}\" }} }}\n\
         function M.execute(ctx, input)\n{body}\nend\nreturn M\n"
    )
}

fn transform(name: &str, field: &str, body: &str) -> String {
    phase(name, &format!("transform.{field}"), body)
}

fn write_script(flows: &Path, file: &str, source: &str) {
    std::fs::create_dir_all(flows).unwrap();
    std::fs::write(flows.join(file), source).unwrap();
}

enum Message {
    Effect(EffectRequest),
    Event(ScriptEvent),
}

struct Harness {
    engine: Engine,
    messages: Receiver<Message>,
    repo: PathBuf,
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
            repo: repo.to_path_buf(),
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

    /// Runs a command as an agent, and returns its id with the events.
    fn handle_as_agent(&mut self, kind: CommandKind) -> (OperationId, Vec<Event>) {
        self.last += 1;
        let id = OperationId(self.last);
        let events = self.engine.handle(
            id,
            &Command {
                actor: Actor::Agent {
                    name: "agent".into(),
                },
                kind,
            },
        );
        (id, events)
    }

    /// Plays the engine thread until the operation `id` ends, or panics.
    fn finish(&mut self, id: OperationId, events: Vec<Event>) -> Vec<Event> {
        self.pump_until(events, move |events| {
            final_state(events, id).is_some_and(|(state, _)| state.is_terminal())
        })
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

    /// Writes `file` and stages it, as the person does in the window.
    fn stage(&mut self, file: &str, text: &str) {
        std::fs::write(self.repo.join(file), text).unwrap();
        git(&self.repo, &["add", file]);
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

/// The answer of the transform operation `id`: its value and whether the repository changed.
fn finished(events: &[Event], id: OperationId) -> Option<(Option<String>, bool)> {
    events.iter().find_map(|e| match e {
        Event::TransformFinished {
            id: got,
            value,
            changed,
            ..
        } if *got == id => Some((value.clone(), *changed)),
        _ => None,
    })
}

fn notices(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowNotice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
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

/// A repository with `a.txt` committed and a workflow folder beside it.
fn staged_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    let flows = dir.path().join("flows");
    std::fs::create_dir_all(&flows).unwrap();
    (dir, repo, flows)
}

fn commit_message(current: &str) -> CommandKind {
    CommandKind::RunTransform {
        field: "commit_message".into(),
        current: current.into(),
    }
}

#[test]
fn one_transform_answers_with_its_message() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "llm.lua",
        &transform(
            "llm",
            "commit_message",
            "return \"feat: suggested by a workflow\"",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("llm");
    h.stage("a.txt", "two\n");
    let (id, events) = h.handle(commit_message(""));
    let events = h.finish(id, events);
    assert_eq!(
        finished(&events, id),
        Some((Some("feat: suggested by a workflow".to_owned()), false))
    );
    assert_eq!(
        final_state(&events, id).map(|(state, _)| state),
        Some(OperationState::Done)
    );
}

#[test]
fn two_transforms_run_in_file_name_order_and_the_second_sees_the_first_answer() {
    let (_dir, repo, flows) = staged_repo();
    // Written in the reverse order: the file names decide.
    write_script(
        &flows,
        "b.lua",
        &transform(
            "second",
            "commit_message",
            "return input.trigger.current .. \"+two\"",
        ),
    );
    write_script(
        &flows,
        "a.lua",
        &transform("first", "commit_message", "return \"one\""),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("first");
    h.review("second");
    let (id, events) = h.handle(commit_message(""));
    let events = h.finish(id, events);
    assert_eq!(
        finished(&events, id),
        Some((Some("one+two".to_owned()), false))
    );
}

#[test]
fn a_nil_answer_leaves_the_value_and_the_next_transform_reads_the_text() {
    let (_dir, repo, flows) = staged_repo();
    write_script(&flows, "a.lua", &transform("quiet", "commit_message", ""));
    write_script(
        &flows,
        "b.lua",
        &transform(
            "keep",
            "commit_message",
            "return input.trigger.current .. \" kept\"",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("quiet");
    h.review("keep");
    let (id, events) = h.handle(commit_message("typed"));
    let events = h.finish(id, events);
    assert_eq!(
        finished(&events, id),
        Some((Some("typed kept".to_owned()), false))
    );
}

#[test]
fn nothing_answering_means_no_value() {
    let (_dir, repo, flows) = staged_repo();
    write_script(&flows, "a.lua", &transform("quiet", "commit_message", ""));
    let mut h = Harness::new(&repo, &flows, true);
    h.review("quiet");
    let (id, events) = h.handle(commit_message("typed"));
    let events = h.finish(id, events);
    assert_eq!(finished(&events, id), Some((None, false)));
    assert_eq!(
        final_state(&events, id).map(|(state, _)| state),
        Some(OperationState::Done)
    );
}

#[test]
fn a_failing_transform_is_skipped_with_a_notice_and_the_next_still_runs() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "a.lua",
        &transform("broken", "commit_message", "ctx.fail(\"tool missing\")"),
    );
    write_script(
        &flows,
        "b.lua",
        &transform("next", "commit_message", "return \"feat: next\""),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("broken");
    h.review("next");
    let (id, events) = h.handle(commit_message(""));
    let events = h.finish(id, events);
    assert_eq!(
        finished(&events, id),
        Some((Some("feat: next".to_owned()), false))
    );
    assert!(
        notices(&events)
            .iter()
            .any(|text| text == "broken failed: tool missing"),
        "{:?}",
        notices(&events)
    );
}

#[test]
fn an_unreviewed_transform_does_not_run_and_says_so() {
    let (_dir, repo, flows) = staged_repo();
    // The unreviewed one would answer "feat: a", and the reviewed one keeps what came before it.
    write_script(
        &flows,
        "a.lua",
        &transform("unreviewed", "commit_message", "return \"feat: a\""),
    );
    write_script(
        &flows,
        "b.lua",
        &transform("reviewed", "commit_message", "return input.trigger.current"),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("reviewed");
    let (id, events) = h.handle(commit_message("typed"));
    let events = h.finish(id, events);
    assert_eq!(
        finished(&events, id),
        Some((Some("typed".to_owned()), false)),
        "the unreviewed transform did not run"
    );
    assert!(
        notices(&events).contains(
            &"unreviewed did not run on transform.commit_message: review it first.".to_owned()
        ),
        "{:?}",
        notices(&events)
    );
}

#[test]
fn staging_another_file_while_a_transform_waits_reports_a_change() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "slow.lua",
        &transform(
            "slow",
            "commit_message",
            "ctx.wait(\"restack.resumed\", { timeout = \"1s\" })\nreturn \"feat: after the wait\"",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("slow");
    let (id, events) = h.handle(commit_message(""));
    h.stage("b.txt", "new\n");
    let events = h.finish(id, events);
    assert_eq!(
        finished(&events, id),
        Some((Some("feat: after the wait".to_owned()), true))
    );
}

#[test]
fn a_branch_name_transform_gets_its_field_and_the_typed_name() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "branch.lua",
        &transform(
            "branch",
            "branch_name",
            "return input.trigger.field .. \":\" .. input.trigger.current",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("branch");
    let (id, events) = h.handle(CommandKind::RunTransform {
        field: "branch_name".into(),
        current: "login".into(),
    });
    let events = h.finish(id, events);
    assert_eq!(
        finished(&events, id),
        Some((Some("branch_name:login".to_owned()), false))
    );
}

#[test]
fn composer_opened_fires_its_workflow_and_then_runs_the_commit_transform() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "opened.lua",
        &phase("opened", "composer.opened", "ctx.notify(\"opened\")"),
    );
    write_script(
        &flows,
        "llm.lua",
        &transform(
            "llm",
            "commit_message",
            "return \"feat: from the composer\"",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("opened");
    h.review("llm");
    h.stage("a.txt", "two\n");
    let (id, events) = h.handle(CommandKind::ComposerOpened);
    let events = h.finish(id, events);
    assert!(
        notices(&events).contains(&"opened".to_owned()),
        "{:?}",
        notices(&events)
    );
    assert_eq!(
        finished(&events, id),
        Some((Some("feat: from the composer".to_owned()), false))
    );
}

#[test]
fn composer_opened_with_nothing_loaded_answers_at_once() {
    let (_dir, repo, flows) = staged_repo();
    let mut h = Harness::new(&repo, &flows, true);
    let (id, events) = h.handle(CommandKind::ComposerOpened);
    assert_eq!(finished(&events, id), Some((None, false)));
    assert_eq!(
        final_state(&events, id).map(|(state, _)| state),
        Some(OperationState::Done)
    );
    assert_eq!(runs_started(&events), 0);
}

#[test]
fn cancelling_a_transform_ends_it_with_no_value() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "hold.lua",
        &transform(
            "hold",
            "commit_message",
            "ctx.wait(\"restack.resumed\", { timeout = \"30s\" })\nreturn \"late\"",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("hold");
    let (id, events) = h.handle(commit_message(""));
    let events = h.pump_until(events, |events| {
        events.iter().any(|e| {
            matches!(
                e,
                Event::WorkflowRunChanged {
                    status: WorkflowRunStatus::Waiting,
                    ..
                }
            )
        })
    });
    let (_, cancel) = h.handle(CommandKind::CancelOperation { op: id });
    assert_eq!(
        final_state(&cancel, id).map(|(state, _)| state),
        Some(OperationState::Cancelled),
        "{cancel:#?}"
    );
    assert_eq!(finished(&cancel, id), Some((None, false)));
    let late = h.settle(Duration::from_millis(300));
    assert_eq!(finished(&events, id), None);
    assert_eq!(finished(&late, id), None, "the late answer is not used");
}

#[test]
fn a_new_transform_supersedes_the_one_in_progress() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "slow.lua",
        &transform(
            "slow",
            "commit_message",
            "if input.trigger.current == \"slow\" then\n\
             ctx.wait(\"restack.resumed\", { timeout = \"30s\" })\nend\n\
             return \"feat: \" .. input.trigger.current",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("slow");
    let (first, events) = h.handle(commit_message("slow"));
    let events = h.pump_until(events, |events| {
        events.iter().any(|e| {
            matches!(
                e,
                Event::WorkflowRunChanged {
                    status: WorkflowRunStatus::Waiting,
                    ..
                }
            )
        })
    });
    let (second, next) = h.handle(commit_message("fast"));
    assert_eq!(finished(&next, first), Some((None, false)));
    assert_eq!(
        final_state(&next, first).map(|(state, _)| state),
        Some(OperationState::Cancelled)
    );
    let next = h.finish(second, next);
    assert_eq!(
        finished(&next, second),
        Some((Some("feat: fast".to_owned()), false))
    );
    let late = h.settle(Duration::from_millis(300));
    assert_eq!(finished(&events, first), None);
    assert_eq!(finished(&late, first), None);
}

#[test]
fn with_the_switch_off_nothing_runs_and_the_answer_has_no_value() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "llm.lua",
        &transform(
            "llm",
            "commit_message",
            "ctx.notify(\"ran\")\nreturn \"feat: x\"",
        ),
    );
    let mut h = Harness::new(&repo, &flows, false);
    h.review("llm");
    let (id, events) = h.handle(commit_message("typed"));
    assert_eq!(finished(&events, id), Some((None, false)));
    assert_eq!(
        final_state(&events, id).map(|(state, _)| state),
        Some(OperationState::Done)
    );
    let late = h.settle(Duration::from_millis(200));
    assert_eq!(runs_started(&events) + runs_started(&late), 0);
    assert!(notices(&late).is_empty());
}

#[test]
fn an_agent_cannot_run_a_transform() {
    let (_dir, repo, flows) = staged_repo();
    let mut h = Harness::new(&repo, &flows, true);
    let (id, events) = h.handle_as_agent(commit_message(""));
    assert_eq!(
        final_state(&events, id).map(|(state, _)| state),
        Some(OperationState::Failed),
        "{events:#?}"
    );
    assert!(failure(&events).is_some_and(|detail| detail.contains("human")));
}

#[test]
fn a_field_without_transforms_is_refused() {
    let (_dir, repo, flows) = staged_repo();
    let mut h = Harness::new(&repo, &flows, true);
    let (id, events) = h.handle(CommandKind::RunTransform {
        field: "conflict_hunk".into(),
        current: String::new(),
    });
    assert_eq!(
        final_state(&events, id).map(|(state, _)| state),
        Some(OperationState::Failed)
    );
}

/// The `prepare-commit` fake of the parity checks: reads its stdin for `invocation_id`, runs `body`,
/// and answers with the message `$MSG`.
fn answer(body: &str) -> String {
    format!(
        "IN=$(cat)\nID=$(printf '%s' \"$IN\" | sed 's/.*\"invocation_id\":\"\\([^\"]*\\)\".*/\\1/')\n{body}\nprintf '{{\"invocation_id\":\"%s\",\"output\":{{\"message\":\"%s\"}}}}\\n' \"$ID\" \"$MSG\""
    )
}

/// Writes the fake program `name` (answering with `body`) and returns the command that runs it.
fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(format!("{name}.sh"));
    std::fs::write(&path, format!("#!/bin/sh\n{}\n", answer(body))).unwrap();
    path
}

#[test]
fn parity_one_fake_program_gives_the_message_the_prepare_commit_hook_gave() {
    let (dir, repo, flows) = staged_repo();
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    git(&repo, &["add", "a.txt"]);
    let program = fake(dir.path(), "llm", "MSG='feat: suggested'");

    // The same program, run by a transform through `ctx.exec` with the JSON contract of the hook.
    write_script(
        &flows,
        "llm.lua",
        &transform(
            "llm",
            "commit_message",
            &format!(
                "local r = ctx.exec({{ \"sh\", \"{}\" }}, {{ stdin = '{{\"invocation_id\":\"transform\",\"input\":{{\"message\":\"' .. input.trigger.current .. '\"}}}}', format = \"json\", timeout = \"30s\" }})\nreturn r.json.output.message",
                program.display()
            ),
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("llm");
    let (id, events) = h.handle(commit_message(""));
    let events = h.finish(id, events);
    let from_transform = finished(&events, id).and_then(|(value, _)| value);
    // The message the `prepare-commit` hook gave for this program.
    assert_eq!(from_transform.as_deref(), Some("feat: suggested"));
}

#[test]
fn parity_a_chain_passes_each_answer_to_the_next_step_as_prepare_commit_does() {
    let (dir, repo, flows) = staged_repo();
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    git(&repo, &["add", "a.txt"]);
    let first = fake(dir.path(), "first", "MSG=one");
    let second = fake(
        dir.path(),
        "second",
        "printf '%s' \"$IN\" | grep -q '\"message\":\"one\"' || exit 3\nMSG=one+two",
    );
    let step = |program: &Path, name: &str| {
        format!(
            "local r = ctx.exec({{ \"sh\", \"{}\" }}, {{ stdin = '{{\"invocation_id\":\"{name}\",\"input\":{{\"message\":\"' .. input.trigger.current .. '\"}}}}', format = \"json\", timeout = \"30s\" }})\nif r.json == nil then return nil end\nreturn r.json.output.message",
            program.display()
        )
    };
    write_script(
        &flows,
        "a.lua",
        &transform("first", "commit_message", &step(&first, "first")),
    );
    write_script(
        &flows,
        "b.lua",
        &transform("second", "commit_message", &step(&second, "second")),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("first");
    h.review("second");
    let (id, events) = h.handle(commit_message(""));
    let events = h.finish(id, events);
    let from_transform = finished(&events, id).and_then(|(value, _)| value);
    // The message the `prepare-commit` chain gave for these two programs.
    assert_eq!(from_transform.as_deref(), Some("one+two"));
}
