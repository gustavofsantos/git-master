//! Converting the `prepare-commit` sections of the user's Git config into a workflow (`WF3e`, D160),
//! end to end: real repositories in temporary directories, a temporary global config, real
//! programs as the commands, and the real Lua workflow the conversion writes.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gm_core::git::SubprocessGit;
use gm_core::script::run::ScriptEvent;
use gm_core::script::trust::TrustStore;
use gm_core::{EffectRequest, Engine, ScriptLink};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

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

/// A repository on `main` with `a.txt` committed, a second change staged, and a workflow folder.
fn staged_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["config", "user.email", "t@t"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    git(&repo, &["add", "a.txt"]);
    let flows = dir.path().join("flows");
    std::fs::create_dir_all(&flows).unwrap();
    (dir, repo, flows)
}

/// Writes the fake program `name` (answering with `body`) and returns its path. It reads its
/// stdin for the invocation id, runs `body`, and answers with the message `$MSG`.
fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(format!("{name}.sh"));
    let script = format!(
        "#!/bin/sh\nIN=$(cat)\nID=$(printf '%s' \"$IN\" | sed 's/.*\"invocation_id\":\"\\([^\"]*\\)\".*/\\1/')\n{body}\nprintf '{{\"invocation_id\":\"%s\",\"output\":{{\"message\":\"%s\"}}}}\\n' \"$ID\" \"$MSG\"\n"
    );
    std::fs::write(&path, script).unwrap();
    path
}

/// A global config with one section per `(name, command, extra lines)`.
fn global_config(dir: &Path, sections: &[(&str, &str, &str)]) -> PathBuf {
    let mut text = String::new();
    for (name, command, extra) in sections {
        let _ = write!(
            text,
            "[gitmaster \"prepare-commit.{name}\"]\n\tcommand = {command}\n{extra}"
        );
    }
    let path = dir.join("global.gitconfig");
    std::fs::write(&path, text).unwrap();
    path
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
    /// Opens `repo` with the workflows of `flows` and the global config `config`.
    fn new(repo: &Path, flows: &Path, config: &Path) -> Self {
        let (tx, messages) = mpsc::channel();
        let effects_tx = Mutex::new(tx.clone());
        let events_tx = Mutex::new(tx);
        let git = SubprocessGit {
            global_config: Some(config.to_path_buf()),
            ..SubprocessGit::default()
        };
        let mut engine = Engine::new(Box::new(git));
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
        engine.set_triggers(true);
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

    fn handle(&mut self, kind: CommandKind) -> (OperationId, Vec<Event>) {
        self.run_as(Actor::Human, kind)
    }

    fn handle_as_agent(&mut self, kind: CommandKind) -> (OperationId, Vec<Event>) {
        self.run_as(
            Actor::Agent {
                name: "agent".into(),
            },
            kind,
        )
    }

    fn run_as(&mut self, actor: Actor, kind: CommandKind) -> (OperationId, Vec<Event>) {
        self.last += 1;
        let id = OperationId(self.last);
        let events = self.engine.handle(id, &Command { actor, kind });
        (id, events)
    }

    /// Plays the engine thread until the operation `id` ends, or panics.
    fn finish(&mut self, id: OperationId, events: Vec<Event>) -> Vec<Event> {
        let mut seen = events;
        let end = Instant::now() + Duration::from_secs(30);
        while !final_state(&seen, id).is_some_and(|(state, _)| state.is_terminal()) {
            assert!(Instant::now() < end, "timed out; events: {seen:#?}");
            seen.extend(self.step(Duration::from_millis(20)));
        }
        seen
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

    /// The `HooksConverted` answer of one conversion.
    fn convert(&mut self) -> (Vec<String>, Vec<(String, String)>, Vec<String>) {
        let (id, events) = self.handle(CommandKind::ConvertHooksToWorkflows);
        let events = self.finish(id, events);
        assert_eq!(
            final_state(&events, id).map(|(state, _)| state),
            Some(OperationState::Done),
            "{events:#?}"
        );
        events
            .iter()
            .find_map(|e| match e {
                Event::HooksConverted {
                    written,
                    skipped,
                    sections,
                    ..
                } => Some((written.clone(), skipped.clone(), sections.clone())),
                _ => None,
            })
            .expect("the conversion answered")
    }

    fn sections(&mut self) -> Vec<String> {
        let (id, events) = self.handle(CommandKind::ListHookSections);
        let events = self.finish(id, events);
        events
            .iter()
            .find_map(|e| match e {
                Event::HookSectionsListed { sections, .. } => Some(sections.clone()),
                _ => None,
            })
            .expect("the sections were listed")
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

fn commit_message(current: &str) -> CommandKind {
    CommandKind::RunTransform {
        field: "commit_message".into(),
        current: current.into(),
    }
}

/// The value the transform gives for the staged repository, after the workflows are reviewed.
fn suggest(h: &mut Harness, reviewed: &[&str]) -> Option<String> {
    for name in reviewed {
        h.review(name);
    }
    let (id, events) = h.handle(commit_message(""));
    let events = h.finish(id, events);
    finished(&events, id).and_then(|(value, _)| value)
}

fn sorted(mut entries: Vec<String>) -> Vec<String> {
    entries.sort();
    entries
}

#[test]
fn a_converted_hook_writes_a_reviewed_transform_with_the_same_message() {
    let (dir, repo, flows) = staged_repo();
    let program = fake(dir.path(), "llm", "MSG='feat: suggested'");
    let config = global_config(
        dir.path(),
        &[(
            "llm",
            &format!("sh {}", program.display()),
            "\ttimeout = 5s\n",
        )],
    );
    let before = std::fs::read(&config).unwrap();

    let mut h = Harness::new(&repo, &flows, &config);
    assert_eq!(
        h.sections(),
        ["global gitmaster.prepare-commit.llm".to_owned()]
    );
    let (written, skipped, sections) = h.convert();
    assert_eq!(written.len(), 1, "one file: {written:?}");
    assert!(
        written[0].ends_with("prepare-commit-llm.lua"),
        "{written:?}"
    );
    assert!(skipped.is_empty());
    assert_eq!(sections, ["global gitmaster.prepare-commit.llm".to_owned()]);

    let source = std::fs::read_to_string(&written[0]).unwrap();
    assert!(source.contains("trigger = { on = \"transform.commit_message\" }"));
    assert_eq!(
        std::fs::read(&config).unwrap(),
        before,
        "the config is not edited"
    );

    // The message the `prepare-commit` hook gave for this program, before the hook path went.
    assert_eq!(
        suggest(&mut h, &["prepare-commit-llm"]).as_deref(),
        Some("feat: suggested")
    );
    let log = h.engine.log().iter().any(|entry| {
        matches!(entry.kind, CommandKind::ConvertHooksToWorkflows)
            && entry.state == OperationState::Done
    });
    assert!(log, "the conversion is in the command log");
}

#[test]
fn a_second_conversion_writes_a_new_file_and_keeps_the_first() {
    let (dir, repo, flows) = staged_repo();
    let program = fake(dir.path(), "llm", "MSG='feat: suggested'");
    let config = global_config(
        dir.path(),
        &[("llm", &format!("sh {}", program.display()), "")],
    );
    let mut h = Harness::new(&repo, &flows, &config);
    let (first, _, _) = h.convert();
    let first_text = std::fs::read_to_string(&first[0]).unwrap();
    let (second, _, _) = h.convert();
    assert!(
        second[0].ends_with("prepare-commit-llm-2.lua"),
        "{second:?}"
    );
    assert_eq!(std::fs::read_to_string(&first[0]).unwrap(), first_text);
    assert!(
        std::fs::read_to_string(&second[0])
            .unwrap()
            .contains("name = \"prepare-commit-llm-2\"")
    );
}

#[test]
fn an_invalid_section_is_skipped_with_its_reason_and_writes_nothing() {
    let (dir, repo, flows) = staged_repo();
    let config = global_config(dir.path(), &[("bad", "", "\ttimeout = 1s\n")]);
    let mut h = Harness::new(&repo, &flows, &config);
    let (written, skipped, sections) = h.convert();
    assert!(written.is_empty());
    assert_eq!(
        skipped,
        [(
            "global gitmaster.prepare-commit.bad".to_owned(),
            "command is empty".to_owned()
        )]
    );
    assert_eq!(sections, ["global gitmaster.prepare-commit.bad".to_owned()]);
    assert!(std::fs::read_dir(&flows).unwrap().next().is_none());
}

#[test]
fn a_repository_section_is_listed_and_never_converted() {
    let (dir, repo, flows) = staged_repo();
    git(
        &repo,
        &[
            "config",
            "gitmaster.prepare-commit.team.command",
            "team-tool",
        ],
    );
    let config = global_config(dir.path(), &[]);
    let mut h = Harness::new(&repo, &flows, &config);
    assert_eq!(
        h.sections(),
        ["local gitmaster.prepare-commit.team".to_owned()]
    );
    let (written, skipped, sections) = h.convert();
    assert!(written.is_empty());
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].0, "local gitmaster.prepare-commit.team");
    assert!(skipped[0].1.contains("D17"));
    assert_eq!(sections, ["local gitmaster.prepare-commit.team".to_owned()]);
    let config_text = git(
        &repo,
        &[
            "config",
            "--local",
            "--get",
            "gitmaster.prepare-commit.team.command",
        ],
    );
    assert_eq!(
        config_text, "team-tool",
        "the repository config is unchanged"
    );
}

#[test]
fn a_chain_of_sections_becomes_one_script_that_passes_each_answer_on() {
    let (dir, repo, flows) = staged_repo();
    let first = fake(dir.path(), "first", "MSG=one");
    let second = fake(
        dir.path(),
        "second",
        "printf '%s' \"$IN\" | grep -q '\"message\":\"one\"' || exit 3\nMSG=one+two",
    );
    let config = global_config(
        dir.path(),
        &[
            ("first", &format!("sh {}", first.display()), ""),
            ("second", &format!("sh {}", second.display()), ""),
        ],
    );
    let mut h = Harness::new(&repo, &flows, &config);
    let (written, _, _) = h.convert();
    assert_eq!(written.len(), 1);
    assert!(
        written[0].ends_with("prepare-commit-chain.lua"),
        "{written:?}"
    );
    assert_eq!(
        suggest(&mut h, &["prepare-commit-chain"]).as_deref(),
        Some("one+two")
    );
}

#[test]
fn a_match_that_does_not_fit_the_branch_is_not_run() {
    let (dir, repo, flows) = staged_repo();
    let program = fake(dir.path(), "llm", "MSG='feat: on a feature branch'");
    let config = global_config(
        dir.path(),
        &[(
            "llm",
            &format!("sh {}", program.display()),
            "\tmatch = branch:feat/*\n",
        )],
    );
    let mut h = Harness::new(&repo, &flows, &config);
    h.convert();
    // On main nothing matches, so the answer has no value and the program never runs.
    assert_eq!(suggest(&mut h, &["prepare-commit-llm"]), None);
    git(&repo, &["checkout", "-q", "-b", "feat/x"]);
    let mut h = Harness::new(&repo, &flows, &config);
    assert_eq!(
        suggest(&mut h, &["prepare-commit-llm"]).as_deref(),
        Some("feat: on a feature branch")
    );
}

#[test]
fn a_failing_command_gives_no_value_as_its_hook_did() {
    let (dir, repo, flows) = staged_repo();
    let program = fake(dir.path(), "llm", "exit 3");
    let config = global_config(
        dir.path(),
        &[("llm", &format!("sh {}", program.display()), "")],
    );
    let mut h = Harness::new(&repo, &flows, &config);
    h.convert();
    assert_eq!(suggest(&mut h, &["prepare-commit-llm"]), None);
}

#[test]
fn an_agent_cannot_convert_hooks() {
    let (dir, repo, flows) = staged_repo();
    let config = global_config(dir.path(), &[]);
    let mut h = Harness::new(&repo, &flows, &config);
    let (id, events) = h.handle_as_agent(CommandKind::ConvertHooksToWorkflows);
    assert_eq!(
        final_state(&events, id),
        Some((
            OperationState::Failed,
            Some("only a human can convert hooks to workflows".to_owned())
        ))
    );
    assert!(std::fs::read_dir(&flows).unwrap().next().is_none());
}

#[test]
fn the_sections_listing_is_sorted_and_names_both_scopes() {
    let (dir, repo, flows) = staged_repo();
    git(
        &repo,
        &[
            "config",
            "gitmaster.prepare-commit.team.command",
            "team-tool",
        ],
    );
    let config = global_config(dir.path(), &[("zeta", "z", ""), ("alpha", "a", "")]);
    let mut h = Harness::new(&repo, &flows, &config);
    assert_eq!(
        sorted(h.sections()),
        [
            "global gitmaster.prepare-commit.alpha",
            "global gitmaster.prepare-commit.zeta",
            "local gitmaster.prepare-commit.team",
        ]
    );
}
