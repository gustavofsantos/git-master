//! The tests that push or restack are hermetic (R2h, D212 (2)): a gate file in the user's
//! configuration directory must not change what they do. The test runs itself in a child process
//! with `XDG_CONFIG_HOME` pointed at a directory whose `gates.json` blocks every push, so the
//! process environment of the test binary is never changed. In the child, an engine with its user
//! directory left alone is vetoed (the control), and an engine that set an empty user directory
//! pushes.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::{Engine, reviewers};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

const CHILD: &str = "GM_HERMETIC_CHILD";
const TEST: &str = "an_engine_with_an_empty_user_directory_pushes_past_a_blocking_user_gate_file";

/// A gate that blocks every push: no reviewer named `security` is defined.
const BLOCKING: &str =
    r#"{"gates":[{"id":"always","kind":"fresh_verdict","reviewers":["security"],"on":["push"]}]}"#;

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

/// `origin` (bare, with `main`) and `repo` on `feat` with one commit that is not pushed.
fn fixture(base: &Path) -> (PathBuf, PathBuf) {
    let origin = base.join("origin.git");
    git(
        base,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            &origin.to_string_lossy(),
        ],
    );
    let repo = base.join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f.txt"), "one\ntwo\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(
        &repo,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    );
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("f.txt"), "one\nthree\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "feat change"]);
    (origin, repo)
}

/// Opens `repo` on `engine` and pushes its checked-out branch, running the remote job as the
/// window does. Returns every event.
fn open_and_push(engine: &mut Engine, repo: &Path) -> Vec<Event> {
    let opened = engine.handle(
        OperationId(1),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::OpenRepo {
                path: repo.to_string_lossy().into_owned(),
            },
        },
    );
    assert!(reached(&opened, OperationState::Done), "open: {opened:?}");
    let mut events = engine.handle(
        OperationId(2),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::PushWithLease,
        },
    );
    for job in engine.take_remote_jobs() {
        let report = job.run();
        events.extend(engine.remote_report(report));
    }
    events
}

fn reached(events: &[Event], wanted: OperationState) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::OperationChanged { state, .. } if *state == wanted))
}

#[test]
fn an_engine_with_an_empty_user_directory_pushes_past_a_blocking_user_gate_file() {
    if std::env::var_os(CHILD).is_some() {
        run_child();
        return;
    }
    let config = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(config.path().join("gitmaster")).unwrap();
    std::fs::write(config.path().join("gitmaster").join("gates.json"), BLOCKING).unwrap();
    let home = tempfile::tempdir().unwrap();
    let output = Process::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("XDG_CONFIG_HOME", config.path())
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The child: the configuration directory of this process blocks the push.
fn run_child() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, repo) = fixture(dir.path());

    // The control: an engine that keeps the defaults reads the blocking file and is vetoed.
    let mut plain = Engine::new(Box::new(SubprocessGit::default()));
    let control = open_and_push(&mut plain, &repo);
    assert!(
        reached(&control, OperationState::Vetoed),
        "the control must be blocked by the configuration: {control:?}"
    );
    assert!(
        git(
            &origin,
            &["for-each-ref", "--format=%(objectname)", "refs/heads/feat"]
        )
        .is_empty(),
        "the control must not have pushed"
    );

    // The test's own engine: an empty user directory, so the push goes out.
    let user = dir.path().join("user");
    std::fs::create_dir(&user).unwrap();
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    engine.set_gates_user_dir(Some(user.clone()));
    engine.set_reviewer_user_dir(Some(reviewers::user_dir(&user)));
    let pushed = open_and_push(&mut engine, &repo);
    assert!(reached(&pushed, OperationState::Done), "{pushed:?}");
    assert_eq!(
        git(&origin, &["rev-parse", "refs/heads/feat"]),
        git(&repo, &["rev-parse", "feat"])
    );
}
