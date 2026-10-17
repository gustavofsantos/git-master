//! `prepare-commit` hooks end to end: real repositories in temporary
//! directories, real shell scripts as hooks, the real runner threads.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use async_channel::Receiver;
use gm_core::git::SubprocessGit;
use gm_core::hook::ApprovalStore;
use gm_core::{EngineHandle, spawn_with};
use gm_schema::{
    Actor, CommandKind, Event, HookFailure, HookInfo, HookScope, HookState, OperationId,
    OperationState,
};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
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
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A repository on `main` with one commit and one staged change.
fn repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();
    git(dir, &["add", "a.txt"]);
}

/// Writes a script and returns the command that runs it.
fn script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(format!("{name}.sh"));
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    format!("sh {}", path.display())
}

/// Reads stdin, leaves the invocation id in `$ID`, and prints the answer
/// `$MSG` for it. The first line of `body` runs after that.
fn answer(body: &str) -> String {
    format!(
        "IN=$(cat)\nID=$(printf '%s' \"$IN\" | sed 's/.*\"invocation_id\":\"\\([^\"]*\\)\".*/\\1/')\n{body}\nprintf '{{\"invocation_id\":\"%s\",\"output\":{{\"message\":\"%s\"}}}}\\n' \"$ID\" \"$MSG\""
    )
}

/// The user config: one section per `(name, command, extra lines)`.
fn user_config(dir: &Path, sections: &[(&str, &str, &str)]) -> PathBuf {
    use std::fmt::Write as _;
    let mut text = String::new();
    for (name, command, extra) in sections {
        write!(
            text,
            "[gitmaster \"prepare-commit.{name}\"]\n\tcommand = {command}\n{extra}"
        )
        .unwrap();
    }
    let path = dir.join("global.gitconfig");
    std::fs::write(&path, text).unwrap();
    path
}

struct Harness {
    handle: EngineHandle,
    events: Receiver<Event>,
    repo: PathBuf,
    global: PathBuf,
    approvals: PathBuf,
    _dirs: Vec<tempfile::TempDir>,
}

/// An engine with the repository open. `sections` is the user config.
fn start(sections: &[(&str, &str, &str)]) -> Harness {
    let repo_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    repo(repo_dir.path());
    let global = user_config(config_dir.path(), sections);
    let approvals = config_dir.path().join("approvals.json");
    let git = SubprocessGit {
        global_config: Some(global.clone()),
        ..SubprocessGit::default()
    };
    let handle = spawn_with(Box::new(git), ApprovalStore::at(approvals.clone()));
    let events = handle.subscribe();
    let repo = repo_dir.path().to_path_buf();
    let harness = Harness {
        handle,
        events,
        repo,
        global,
        approvals,
        _dirs: vec![repo_dir, config_dir],
    };
    let id = harness.handle.dispatch(
        Actor::Human,
        CommandKind::OpenRepo {
            path: harness.repo.display().to_string(),
        },
    );
    let events = harness.until_done(id);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::StatusLoaded { .. }))
    );
    harness
}

impl Harness {
    /// Every event up to the operation's terminal state.
    fn until_done(&self, id: OperationId) -> Vec<Event> {
        let mut seen = Vec::new();
        loop {
            let event = self.events.recv_blocking().unwrap();
            let terminal = matches!(
                &event,
                Event::OperationChanged { id: got, state, .. } if *got == id && state.is_terminal()
            );
            seen.push(event);
            if terminal {
                return seen;
            }
        }
    }

    fn run(&self) -> OperationId {
        self.handle.dispatch(
            Actor::Human,
            CommandKind::RunPrepareCommit {
                message: String::new(),
            },
        )
    }

    fn list(&self) -> Vec<HookInfo> {
        let id = self.handle.dispatch(Actor::Human, CommandKind::ListHooks);
        match self.until_done(id).into_iter().find_map(|e| match e {
            Event::HooksListed { hooks, .. } => Some(hooks),
            _ => None,
        }) {
            Some(hooks) => hooks,
            None => panic!("ListHooks did not list"),
        }
    }
}

fn terminal(events: &[Event], id: OperationId) -> (OperationState, Option<String>) {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::OperationChanged {
                id: got,
                state,
                detail,
                ..
            } if *got == id => Some((*state, detail.clone())),
            _ => None,
        })
        .expect("the operation ended")
}

fn finished(events: &[Event]) -> (String, bool) {
    events
        .iter()
        .find_map(|e| match e {
            Event::HookFinished {
                message,
                repo_changed,
                ..
            } => Some((message.clone(), *repo_changed)),
            _ => None,
        })
        .expect("the run finished")
}

fn failure(events: &[Event]) -> HookFailure {
    events
        .iter()
        .find_map(|e| match e {
            Event::HookFailed { failure, .. } => Some(failure.clone()),
            _ => None,
        })
        .expect("a hook failed")
}

fn started_hooks(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::HookStarted { hook, .. } => Some(hook.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn happy_path_suggests_the_message_and_reports_no_change() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = script(dir.path(), "llm", &answer("MSG='feat: suggested'"));
    let h = start(&[("llm", &cmd, "")]);

    let id = h.run();
    let events = h.until_done(id);

    assert_eq!(finished(&events), ("feat: suggested".to_owned(), false));
    assert_eq!(started_hooks(&events), ["llm"]);
    assert_eq!(terminal(&events, id).0, OperationState::Done);
    let started = events.iter().find_map(|e| match e {
        Event::HookStarted { step, of, .. } => Some((*step, *of)),
        _ => None,
    });
    assert_eq!(started, Some((1, 1)));
}

#[test]
fn a_chain_runs_in_config_order_and_each_step_gets_the_last_output() {
    let dir = tempfile::tempdir().unwrap();
    let first = script(dir.path(), "first", &answer("MSG=one"));
    let second = script(
        dir.path(),
        "second",
        &answer("printf '%s' \"$IN\" | grep -q '\"message\":\"one\"' || exit 3\nMSG=one+two"),
    );
    let h = start(&[("first", &first, ""), ("second", &second, "")]);

    let id = h.run();
    let events = h.until_done(id);

    assert_eq!(started_hooks(&events), ["first", "second"]);
    assert_eq!(finished(&events).0, "one+two");
    assert_eq!(terminal(&events, id).0, OperationState::Done);
}

#[test]
fn a_step_that_outlives_its_timeout_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let slow = script(dir.path(), "slow", "sleep 30");
    let h = start(&[("slow", &slow, "\ttimeout = 300ms\n")]);

    let began = Instant::now();
    let id = h.run();
    let events = h.until_done(id);

    assert!(began.elapsed() < Duration::from_secs(10));
    assert_eq!(failure(&events), HookFailure::Timeout { timeout_ms: 300 });
    let (state, detail) = terminal(&events, id);
    assert_eq!(state, OperationState::Failed);
    assert_eq!(detail.as_deref(), Some("hook slow timed out after 300ms"));
}

#[test]
fn cancel_kills_the_running_step_and_ends_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let slow = script(dir.path(), "slow", "sleep 30");
    let h = start(&[("slow", &slow, "")]);

    let id = h.run();
    // Wait until the step is running before asking for the cancel.
    loop {
        if let Event::HookStarted { .. } = h.events.recv_blocking().unwrap() {
            break;
        }
    }
    let began = Instant::now();
    h.handle
        .dispatch(Actor::Human, CommandKind::CancelHook { run: id });
    let rest = h.until_done(id);

    assert!(began.elapsed() < Duration::from_secs(10));
    assert!(rest.iter().any(|e| matches!(
        e,
        Event::HookCancelled { hook, .. } if hook == "slow"
    )));
    assert_eq!(terminal(&rest, id).0, OperationState::Cancelled);
}

#[test]
fn a_non_zero_exit_breaks_the_hook_and_shows_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let broken = script(
        dir.path(),
        "broken",
        "cat >/dev/null\necho 'no key' >&2\nexit 2",
    );
    let h = start(&[("broken", &broken, "")]);

    let id = h.run();
    let events = h.until_done(id);

    assert_eq!(
        failure(&events),
        HookFailure::Exited {
            code: Some(2),
            stderr: "no key".into(),
        }
    );
    let (state, detail) = terminal(&events, id);
    assert_eq!(state, OperationState::Failed);
    assert_eq!(
        detail.as_deref(),
        Some("hook broken exited with code 2: no key")
    );
}

#[test]
fn garbage_on_stdout_is_invalid_output() {
    let dir = tempfile::tempdir().unwrap();
    let noisy = script(dir.path(), "noisy", "cat >/dev/null\necho hello");
    let h = start(&[("noisy", &noisy, "")]);

    let id = h.run();
    let events = h.until_done(id);

    assert!(matches!(
        failure(&events),
        HookFailure::InvalidOutput { .. }
    ));
    assert_eq!(terminal(&events, id).0, OperationState::Failed);
}

#[test]
fn an_answer_to_another_invocation_is_discarded() {
    let dir = tempfile::tempdir().unwrap();
    let stale = script(
        dir.path(),
        "stale",
        "cat >/dev/null\nprintf '%s\\n' '{\"invocation_id\":\"prep-0-1\",\"output\":{\"message\":\"old\"}}'",
    );
    let h = start(&[("stale", &stale, "")]);

    let id = h.run();
    let events = h.until_done(id);

    assert_eq!(
        failure(&events),
        HookFailure::InvocationMismatch {
            expected: format!("prep-{}-1", id.0),
            got: "prep-0-1".into(),
        }
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::HookFinished { .. }))
    );
    assert_eq!(terminal(&events, id).0, OperationState::Failed);
}

#[test]
fn a_match_that_does_not_fit_the_branch_is_not_run() {
    let dir = tempfile::tempdir().unwrap();
    let release = script(dir.path(), "release", "exit 9");
    let main = script(dir.path(), "main", &answer("MSG=main-only"));
    let h = start(&[
        ("release", &release, "\tmatch = branch:release/*\n"),
        ("main", &main, "\tmatch = branch:main\n"),
    ]);

    let id = h.run();
    let events = h.until_done(id);

    assert_eq!(started_hooks(&events), ["main"]);
    assert_eq!(finished(&events).0, "main-only");
    let listed = h.list();
    let release_hook = listed.iter().find(|h| h.name == "release").unwrap();
    assert!(!release_hook.applies_to_branch);
}

#[test]
fn a_run_with_no_applicable_hook_fails_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let other = script(dir.path(), "other", &answer("MSG=x"));
    let h = start(&[("other", &other, "\tmatch = branch:release/*\n")]);

    let id = h.run();
    let events = h.until_done(id);

    let (state, detail) = terminal(&events, id);
    assert_eq!(state, OperationState::Failed);
    assert_eq!(
        detail.as_deref(),
        Some("no prepare-commit hook applies to this repository")
    );
}

#[test]
fn a_repo_hook_needs_approval_then_runs_until_its_command_changes() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let first = script(
        dir.path(),
        "repo-first",
        &format!("touch {}\n{}", marker.display(), answer("MSG=from-repo")),
    );
    let h = start(&[]);
    git(
        &h.repo,
        &[
            "config",
            "--local",
            "gitmaster.prepare-commit.team.command",
            &first,
        ],
    );

    // Listed, but not run until approved.
    let listed = h.list();
    let hook = listed.iter().find(|h| h.name == "team").unwrap();
    assert_eq!(hook.scope, HookScope::Repo);
    assert_eq!(hook.state, HookState::NeedsApproval);
    let hash = hook.command_hash.clone();

    let id = h.run();
    let events = h.until_done(id);
    let (state, detail) = terminal(&events, id);
    assert_eq!(state, OperationState::Failed);
    assert!(detail.unwrap().contains("needs approval"));
    assert!(started_hooks(&events).is_empty());
    assert!(!marker.exists());

    // A hash that is not the listed one is refused.
    let refused = h.handle.dispatch(
        Actor::Human,
        CommandKind::ApproveHook {
            name: "team".into(),
            command_hash: "0".repeat(64),
        },
    );
    assert_eq!(
        terminal(&h.until_done(refused), refused).0,
        OperationState::Failed
    );

    let approve = h.handle.dispatch(
        Actor::Human,
        CommandKind::ApproveHook {
            name: "team".into(),
            command_hash: hash,
        },
    );
    let events = h.until_done(approve);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::HooksListed { hooks, .. } if hooks.iter().any(|h| h.name == "team" && h.state == HookState::Enabled)
    )));
    assert!(
        h.approvals.exists(),
        "the approval is stored outside the repo"
    );

    let id = h.run();
    let events = h.until_done(id);
    assert_eq!(finished(&events).0, "from-repo");
    assert!(marker.exists());

    // Changing the command text makes the approval stop applying.
    std::fs::remove_file(&marker).unwrap();
    let changed = script(dir.path(), "repo-second", &answer("MSG=changed"));
    git(
        &h.repo,
        &[
            "config",
            "--local",
            "gitmaster.prepare-commit.team.command",
            &changed,
        ],
    );
    let listed = h.list();
    let hook = listed.iter().find(|h| h.name == "team").unwrap();
    assert_eq!(hook.state, HookState::NeedsApproval);

    let id = h.run();
    let events = h.until_done(id);
    assert_eq!(terminal(&events, id).0, OperationState::Failed);
    assert!(!marker.exists());
}

#[test]
fn a_repo_hook_cannot_take_the_name_of_a_user_hook() {
    let dir = tempfile::tempdir().unwrap();
    let user = script(dir.path(), "user", &answer("MSG=u"));
    let h = start(&[("llm", &user, "")]);
    git(
        &h.repo,
        &[
            "config",
            "--local",
            "gitmaster.prepare-commit.llm.command",
            "exit 1",
        ],
    );

    let listed = h.list();
    let repo_hook = listed.iter().find(|h| h.scope == HookScope::Repo).unwrap();
    assert!(
        matches!(&repo_hook.state, HookState::Invalid { reason } if reason.contains("same name"))
    );
}

#[test]
fn a_moved_head_during_a_slow_hook_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let slow = script(
        dir.path(),
        "slow",
        &format!("sleep 2\n{}", answer("MSG=late")),
    );
    let h = start(&[("slow", &slow, "")]);

    let id = h.run();
    loop {
        if let Event::HookStarted { .. } = h.events.recv_blocking().unwrap() {
            break;
        }
    }
    git(&h.repo, &["commit", "-q", "-m", "moves head"]);
    let events = h.until_done(id);

    assert_eq!(finished(&events), ("late".to_owned(), true));
}

#[test]
fn a_changed_index_during_a_slow_hook_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let slow = script(
        dir.path(),
        "slow",
        &format!("sleep 2\n{}", answer("MSG=late")),
    );
    let h = start(&[("slow", &slow, "")]);

    let id = h.run();
    loop {
        if let Event::HookStarted { .. } = h.events.recv_blocking().unwrap() {
            break;
        }
    }
    std::fs::write(h.repo.join("b.txt"), "new\n").unwrap();
    git(&h.repo, &["add", "b.txt"]);
    let events = h.until_done(id);

    assert_eq!(finished(&events), ("late".to_owned(), true));
}

#[test]
fn a_large_staged_diff_reaches_the_hook_as_a_file() {
    let dir = tempfile::tempdir().unwrap();
    // The hook answers `has-diff` only if the file it names holds the change.
    let check = script(
        dir.path(),
        "check",
        &answer(
            "P=$(printf '%s' \"$IN\" | sed -n 's/.*\"staged_diff_path\":\"\\([^\"]*\\)\".*/\\1/p')\nif grep -q 'marker-line' \"$P\"; then MSG=has-diff; else MSG=missing; fi",
        ),
    );
    let h = start(&[("check", &check, "")]);
    let big = "marker-line\n".repeat(10_000);
    std::fs::write(h.repo.join("big.txt"), big).unwrap();
    git(&h.repo, &["add", "big.txt"]);

    let id = h.run();
    let events = h.until_done(id);

    assert_eq!(finished(&events).0, "has-diff");
}

#[test]
fn a_run_with_no_repository_open_fails_before_starting_anything() {
    let dir = tempfile::tempdir().unwrap();
    let approvals = dir.path().join("approvals.json");
    let handle = spawn_with(
        Box::new(SubprocessGit::default()),
        ApprovalStore::at(approvals),
    );
    let events = handle.subscribe();
    let id = handle.dispatch(
        Actor::Human,
        CommandKind::RunPrepareCommit {
            message: String::new(),
        },
    );
    let mut seen = Vec::new();
    loop {
        let event = events.recv_blocking().unwrap();
        let done = matches!(
            &event,
            Event::OperationChanged { id: got, state, .. } if *got == id && state.is_terminal()
        );
        seen.push(event);
        if done {
            break;
        }
    }
    let (state, detail) = terminal(&seen, id);
    assert_eq!(state, OperationState::Failed);
    assert_eq!(detail.as_deref(), Some("no repository is open"));
}

#[test]
fn listing_reports_every_section_with_its_state() {
    let dir = tempfile::tempdir().unwrap();
    let ok = script(dir.path(), "ok", &answer("MSG=x"));
    let h = start(&[
        ("llm", &ok, "\ttimeout = 45s\n"),
        ("off", &ok, "\tenabled = false\n"),
        ("broken", &ok, "\ttimeout = soon\n"),
    ]);

    let hooks = h.list();

    let llm = hooks.iter().find(|h| h.name == "llm").unwrap();
    assert_eq!(llm.state, HookState::Enabled);
    assert_eq!(llm.timeout_ms, 45_000);
    assert_eq!(llm.scope, HookScope::User);
    let off = hooks.iter().find(|h| h.name == "off").unwrap();
    assert_eq!(off.state, HookState::Disabled);
    let broken = hooks.iter().find(|h| h.name == "broken").unwrap();
    assert!(matches!(&broken.state, HookState::Invalid { reason } if reason.contains("timeout")));
    assert!(h.global.exists());
    assert!(!h.approvals.exists(), "listing approves nothing");
}

#[test]
fn the_engine_log_records_each_step_with_its_command_hash_and_exit_code() {
    use gm_core::Engine;
    use gm_core::hook::{HookOutcome, sha256_hex};

    let dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    repo(repo_dir.path());
    let ok = script(dir.path(), "ok", &answer("MSG=fine"));
    let broken = script(dir.path(), "broken", "cat >/dev/null\nexit 2");
    let global = user_config(dir.path(), &[("ok", &ok, ""), ("broken", &broken, "")]);
    let git = SubprocessGit {
        global_config: Some(global),
        ..SubprocessGit::default()
    };
    // The engine is driven on this thread: the steps run synchronously.
    let mut engine = Engine::with_approvals(
        Box::new(git),
        ApprovalStore::at(dir.path().join("approvals.json")),
    );
    let command = |kind| gm_schema::Command {
        actor: Actor::Human,
        kind,
    };
    engine.handle(
        OperationId(1),
        &command(CommandKind::OpenRepo {
            path: repo_dir.path().display().to_string(),
        }),
    );
    engine.handle(
        OperationId(2),
        &command(CommandKind::RunPrepareCommit {
            message: String::new(),
        }),
    );
    for plan in engine.take_jobs() {
        let mut reports = Vec::new();
        plan.execute(|report| reports.push(report));
        for report in reports {
            engine.hook_report(report);
        }
    }

    let log = engine.hook_log();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].run, OperationId(2));
    assert_eq!(log[0].hook, "ok");
    assert_eq!(log[0].command, ok);
    assert_eq!(log[0].command_hash, sha256_hex(ok.as_bytes()));
    assert_eq!(log[0].exit_code, Some(0));
    assert_eq!(log[0].outcome, HookOutcome::Ok);
    assert_eq!(log[1].hook, "broken");
    assert_eq!(log[1].exit_code, Some(2));
    assert!(matches!(log[1].outcome, HookOutcome::Failed(_)));
}

#[test]
fn a_user_hook_still_runs_while_a_repo_hook_awaits_approval() {
    let dir = tempfile::tempdir().unwrap();
    let user = script(dir.path(), "user", &answer("MSG=from-user"));
    let h = start(&[("llm", &user, "")]);
    let team = script(dir.path(), "team", "exit 7");
    git(
        &h.repo,
        &[
            "config",
            "--local",
            "gitmaster.prepare-commit.team.command",
            team.as_str(),
        ],
    );

    let id = h.run();
    let events = h.until_done(id);

    assert_eq!(started_hooks(&events), ["llm"]);
    assert_eq!(finished(&events).0, "from-user");
    let listed = h.list();
    let team_hook = listed.iter().find(|h| h.name == "team").unwrap();
    assert_eq!(team_hook.state, HookState::NeedsApproval);
}
