//! `gitmaster thread …` and `gitmaster harness list` (docs/reference/cli.md, design/threads.md).
//!
//! One-shot commands: each reads its input, does its work and exits. Nothing here opens a
//! window or keeps a process. `thread hook` runs from the agent's hooks and never changes the
//! outcome of the agent's tool call: it always exits `0` and prints nothing to standard output
//! (design/threads.md, "The hook command (D124)"). Everything that depends on the process (the
//! environment, the clock, the running binary) comes in through `Env`, so the commands are
//! tested without touching it.

use std::io::{Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::config::{self, ThreadsConfig};
use super::harness::{self, HarnessAdapter};
use super::pipeline::{self, Context};
use super::redact::Redactor;
use super::store::{HookStore, ThreadStore};

/// The largest hook payload read from standard input, in bytes. A larger one is dropped.
const PAYLOAD_LIMIT: u64 = 8 * 1024 * 1024;

const THREAD_USAGE: &str = "usage: gitmaster thread hook --harness <id>
       gitmaster thread redact-test [--text <text>]
";

const REDACT_USAGE: &str = "usage: gitmaster thread redact-test [--text <text>]
";

const HARNESS_USAGE: &str = "usage: gitmaster harness list
";

/// What the commands need from the process. Built once by `Env::from_process`.
#[derive(Clone, Debug)]
pub struct Env {
    /// The settings file, when one can be found.
    pub settings: Option<PathBuf>,
    /// The time of this invocation, in milliseconds since the Unix epoch.
    pub now_ms: i64,
}

impl Env {
    /// The environment of the running process.
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            settings: config::settings_path(),
            now_ms: now_ms(),
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Runs `gitmaster thread <args…>`. `words` are the words after `thread`.
///
/// `hook` always returns `0` and prints nothing. `redact-test` returns `0`, `1` for a pattern
/// that does not build, or `2` for a wrong command line. Any other first word is a usage error.
pub fn thread_command(
    words: &[String],
    stdin: &mut dyn Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    thread_with(words, stdin, out, err, &Env::from_process(), &registry())
}

/// `thread_command` with the environment and the adapters passed in.
fn thread_with(
    words: &[String],
    stdin: &mut dyn Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
    env: &Env,
    adapters: &[&dyn HarnessAdapter],
) -> i32 {
    let Some((word, rest)) = words.split_first() else {
        return usage(err, THREAD_USAGE);
    };
    match word.as_str() {
        "hook" => {
            guarded_hook(rest, stdin, env, adapters);
            0
        }
        "redact-test" => redact_test(rest, stdin, out, err, env),
        other => {
            let _ = writeln!(err, "gitmaster thread: unknown subcommand `{other}`");
            usage(err, THREAD_USAGE)
        }
    }
}

/// Runs the hook body with panics silenced, so nothing reaches the agent whatever happens.
fn guarded_hook(
    rest: &[String],
    stdin: &mut dyn Read,
    env: &Env,
    adapters: &[&dyn HarnessAdapter],
) {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let _ = panic::catch_unwind(AssertUnwindSafe(|| hook(rest, stdin, env, adapters)));
    panic::set_hook(previous);
}

/// The hook body. Every failure ends it quietly: the payload is dropped, and when the
/// repository is known a line goes to its capture log.
fn hook(rest: &[String], stdin: &mut dyn Read, env: &Env, adapters: &[&dyn HarnessAdapter]) {
    let Some(id) = harness_option(rest) else {
        return;
    };
    let mut payload = Vec::new();
    let read = stdin
        .take(PAYLOAD_LIMIT + 1)
        .read_to_end(&mut payload)
        .is_ok();
    let oversized = u64::try_from(payload.len()).is_ok_and(|len| len > PAYLOAD_LIMIT);
    if !read || oversized {
        return;
    }
    let cwd = cwd_of(&payload);
    let Some(adapter) = find(adapters, id) else {
        if let Some(repo) = cwd.as_deref().and_then(pipeline::locate_repo) {
            pipeline::append_log(
                &repo.common_dir,
                &format!("unknown harness '{id}': event dropped"),
            );
        }
        return;
    };
    let Some(repo) = cwd.as_deref().and_then(pipeline::locate_repo) else {
        return;
    };
    let config = load_config(env);
    if !config.capture {
        return;
    }
    let store = match ThreadStore::open_for_hook(&repo.common_dir) {
        Ok(store) => store,
        Err(error) => {
            pipeline::append_log(&repo.common_dir, &format!("store: {error}"));
            return;
        }
    };
    let ctx = Context {
        config: &config,
        now_ms: env.now_ms,
    };
    let _ = match store {
        HookStore::Ready(mut store) => pipeline::ingest(adapter, &payload, &repo, &ctx, &mut store),
        HookStore::Deferred(reader) => {
            pipeline::ingest_deferred(adapter, &payload, &repo, &ctx, &reader)
        }
    };
}

/// The value after `--harness`. Other words are ignored, so a changed hook never fails on them.
fn harness_option(words: &[String]) -> Option<&str> {
    let index = words.iter().position(|word| word.as_str() == "--harness")?;
    words.get(index + 1).map(String::as_str)
}

/// The `cwd` field of a payload. `None` when the payload is not an object with a text `cwd`.
fn cwd_of(payload: &[u8]) -> Option<PathBuf> {
    #[derive(Deserialize)]
    struct CwdOnly {
        cwd: Option<String>,
    }
    let parsed: CwdOnly = serde_json::from_slice(payload).ok()?;
    parsed.cwd.filter(|cwd| !cwd.is_empty()).map(PathBuf::from)
}

fn load_config(env: &Env) -> ThreadsConfig {
    env.settings
        .as_deref()
        .map_or_else(ThreadsConfig::default, ThreadsConfig::load)
}

/// `thread redact-test [--text <text>]`: prints the redacted sample and one line per hit.
fn redact_test(
    rest: &[String],
    stdin: &mut dyn Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
    env: &Env,
) -> i32 {
    let mut inline = None;
    let mut words = rest.iter();
    while let Some(word) = words.next() {
        if word.as_str() != "--text" {
            let _ = writeln!(
                err,
                "gitmaster thread redact-test: unknown argument `{word}`"
            );
            return usage(err, REDACT_USAGE);
        }
        let Some(value) = words.next() else {
            let _ = writeln!(err, "gitmaster thread redact-test: --text needs a value");
            return usage(err, REDACT_USAGE);
        };
        inline = Some(value.clone());
    }
    let text = if let Some(text) = inline {
        text
    } else {
        let mut bytes = Vec::new();
        if stdin.read_to_end(&mut bytes).is_err() {
            let _ = writeln!(err, "gitmaster thread redact-test: cannot read the sample");
            return 1;
        }
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let config = load_config(env);
    let redactor = match Redactor::new(&config.redact) {
        Ok(redactor) => redactor,
        Err(problems) => {
            for problem in problems {
                let _ = writeln!(err, "pattern {}: {}", problem.name, problem.message);
            }
            return 1;
        }
    };
    let redacted = redactor.redact(&text);
    let _ = out.write_all(redacted.text.as_bytes());
    if !redacted.text.is_empty() && !redacted.text.ends_with('\n') {
        let _ = out.write_all(b"\n");
    }
    for hit in &redacted.hits {
        let _ = writeln!(err, "redacted: {} x{}", hit.pattern, hit.count);
    }
    0
}

/// Runs `gitmaster harness <args…>`. `words` are the words after `harness`.
pub fn harness_command(words: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    harness_with(words, out, err, &registry())
}

/// `harness_command` with the adapters passed in.
fn harness_with(
    words: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    adapters: &[&dyn HarnessAdapter],
) -> i32 {
    let Some((command, rest)) = words.split_first() else {
        return usage(err, HARNESS_USAGE);
    };
    if command.as_str() != "list" {
        let _ = writeln!(err, "gitmaster harness: unknown subcommand `{command}`");
        return usage(err, HARNESS_USAGE);
    }
    if !rest.is_empty() {
        return usage(err, HARNESS_USAGE);
    }
    list(out, adapters)
}

/// `harness list`: one id and display name per line, sorted by id.
fn list(out: &mut dyn Write, adapters: &[&dyn HarnessAdapter]) -> i32 {
    let mut sorted = adapters.to_vec();
    sorted.sort_by_key(|adapter| adapter.id());
    for adapter in sorted {
        let _ = writeln!(out, "{}  {}", adapter.id(), adapter.display_name());
    }
    0
}

/// The adapters of this build.
fn registry() -> Vec<&'static dyn HarnessAdapter> {
    harness::known_ids()
        .into_iter()
        .filter_map(harness::lookup)
        .collect()
}

fn find<'a>(adapters: &[&'a dyn HarnessAdapter], id: &str) -> Option<&'a dyn HarnessAdapter> {
    adapters.iter().copied().find(|adapter| adapter.id() == id)
}

/// Writes a usage text and returns the usage exit status.
fn usage(err: &mut dyn Write, text: &str) -> i32 {
    let _ = err.write_all(text.as_bytes());
    2
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use serde_json::json;

    use super::*;
    use crate::threads::harness::fake::FakeAdapter;
    use crate::threads::harness::{Capabilities, HookInput, LogChunk, ParseError, SessionLog};
    use gm_schema::threads::ThreadFilter;

    /// An adapter for the tests. It parses the fake harness's payloads.
    struct TestAdapter;

    impl HarnessAdapter for TestAdapter {
        fn id(&self) -> &'static str {
            "test"
        }

        fn display_name(&self) -> &'static str {
            "Test harness"
        }

        fn capabilities(&self) -> Capabilities {
            FakeAdapter.capabilities()
        }

        fn parse_hook(&self, payload: &[u8]) -> Result<HookInput, ParseError> {
            FakeAdapter.parse_hook(payload)
        }

        fn read_session_log(&self, path: &Path, from: u64) -> Result<LogChunk, ParseError> {
            FakeAdapter.read_session_log(path, from)
        }

        fn discover(&self, _worktree: &Path) -> Vec<SessionLog> {
            Vec::new()
        }
    }

    /// An adapter whose parser panics, to check that the hook swallows it.
    struct Boom;

    impl HarnessAdapter for Boom {
        fn id(&self) -> &'static str {
            "boom"
        }

        fn display_name(&self) -> &'static str {
            "Boom"
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }

        fn parse_hook(&self, _payload: &[u8]) -> Result<HookInput, ParseError> {
            panic!("parser failed");
        }

        fn read_session_log(&self, _path: &Path, _from: u64) -> Result<LogChunk, ParseError> {
            Err(ParseError("no log".to_owned()))
        }

        fn discover(&self, _worktree: &Path) -> Vec<SessionLog> {
            Vec::new()
        }
    }

    struct Sandbox {
        dir: tempfile::TempDir,
    }

    impl Sandbox {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().expect("temp dir"),
            }
        }

        fn root(&self) -> &Path {
            self.dir.path()
        }

        fn env() -> Env {
            Env {
                settings: None,
                now_ms: 1_000,
            }
        }

        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.root().join(name);
            fs::write(&path, text).expect("write fixture");
            path
        }

        /// A git repository in a new directory under the sandbox.
        fn repo(&self) -> PathBuf {
            let repo = self.root().join("repo");
            fs::create_dir_all(&repo).expect("repo dir");
            let status = Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repo)
                .status()
                .expect("git init");
            assert!(status.success());
            repo
        }
    }

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| (*word).to_owned()).collect()
    }

    /// Runs `thread` with the test adapter and returns (exit, stdout, stderr).
    fn run_thread(
        list: &[&str],
        stdin: &[u8],
        env: &Env,
        adapters: &[&dyn HarnessAdapter],
    ) -> (i32, String, String) {
        let mut input = stdin;
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = thread_with(&words(list), &mut input, &mut out, &mut err, env, adapters);
        (
            code,
            String::from_utf8(out).expect("utf-8"),
            String::from_utf8(err).expect("utf-8"),
        )
    }

    fn run_harness(list: &[&str], adapters: &[&dyn HarnessAdapter]) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = harness_with(&words(list), &mut out, &mut err, adapters);
        (
            code,
            String::from_utf8(out).expect("utf-8"),
            String::from_utf8(err).expect("utf-8"),
        )
    }

    fn hook_payload(event: &str, session: &str, cwd: &Path) -> Vec<u8> {
        json!({"event": event, "session": session, "id": format!("{event}:{session}"), "at": 1, "cwd": cwd.display().to_string()})
            .to_string()
            .into_bytes()
    }

    fn stored_threads(repo: &Path) -> u32 {
        let store = ThreadStore::open(&repo.join(".git")).expect("store");
        let threads = store
            .list_threads(&ThreadFilter::default(), None, 100)
            .expect("list");
        u32::try_from(threads.len()).expect("count")
    }

    #[test]
    fn thread_without_a_known_word_is_a_usage_error() {
        let env = Sandbox::env();
        let (code, out, err) = run_thread(&[], b"", &env, &[]);
        assert_eq!((code, out.as_str()), (2, ""));
        assert!(err.contains("usage: gitmaster thread hook"));
        let (code, _, err) = run_thread(&["hooks"], b"", &env, &[]);
        assert_eq!(code, 2);
        assert!(err.contains("unknown subcommand `hooks`"));
    }

    #[test]
    fn redact_test_prints_the_redacted_text_and_the_hits() {
        let sandbox = Sandbox::new();
        let settings = sandbox.write(
            "settings.json",
            r#"{"threads":{"redact":{"patterns":[{"name":"ticket","regex":"TKT-\\d+"}]}}}"#,
        );
        let mut env = Sandbox::env();
        env.settings = Some(settings);
        let (code, out, err) = run_thread(
            &["redact-test", "--text", "see TKT-42 and TKT-7"],
            b"",
            &env,
            &[],
        );
        assert_eq!(code, 0);
        assert_eq!(out, "see [REDACTED:ticket] and [REDACTED:ticket]\n");
        assert_eq!(err, "redacted: ticket x2\n");
    }

    #[test]
    fn redact_test_reads_the_sample_from_stdin() {
        let env = Sandbox::env();
        let (code, out, err) = run_thread(&["redact-test"], b"plain text\n", &env, &[]);
        assert_eq!(code, 0);
        assert_eq!(out, "plain text\n");
        assert_eq!(err, "");
    }

    #[test]
    fn redact_test_reports_invalid_patterns_and_exits_one() {
        let sandbox = Sandbox::new();
        let settings = sandbox.write(
            "settings.json",
            r#"{"threads":{"redact":{"patterns":[{"name":"bad","regex":"("}]}}}"#,
        );
        let mut env = Sandbox::env();
        env.settings = Some(settings);
        let (code, out, err) = run_thread(&["redact-test", "--text", "x"], b"", &env, &[]);
        assert_eq!(code, 1);
        assert_eq!(out, "");
        assert!(
            err.starts_with("pattern bad: the regex does not build"),
            "{err}"
        );
    }

    #[test]
    fn redact_test_rejects_a_wrong_command_line() {
        let env = Sandbox::env();
        let (code, _, err) = run_thread(&["redact-test", "--color"], b"", &env, &[]);
        assert_eq!(code, 2);
        assert!(err.contains("unknown argument `--color`"));
        let (code, _, _) = run_thread(&["redact-test", "--text"], b"", &env, &[]);
        assert_eq!(code, 2);
    }

    #[test]
    fn hook_stores_the_events_of_the_repository_it_runs_in() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo();
        let env = Sandbox::env();
        let adapters: [&dyn HarnessAdapter; 1] = [&TestAdapter];
        let payload = hook_payload("session_start", "s1", &repo);
        let (code, out, err) =
            run_thread(&["hook", "--harness", "test"], &payload, &env, &adapters);
        assert_eq!((code, out.as_str(), err.as_str()), (0, "", ""));
        assert_eq!(stored_threads(&repo), 1);
    }

    #[test]
    fn hook_is_silent_and_exits_zero_on_every_failure() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo();
        let outside = sandbox.root().join("outside");
        fs::create_dir_all(&outside).expect("dir");
        let env = Sandbox::env();
        let adapters: [&dyn HarnessAdapter; 2] = [&TestAdapter, &Boom];
        let good = hook_payload("session_start", "s1", &repo);
        let cases: Vec<(Vec<&str>, Vec<u8>)> = vec![
            (vec!["hook", "--harness", "test"], b"not json".to_vec()),
            (vec!["hook", "--harness", "test"], Vec::new()),
            (
                vec!["hook", "--harness", "test"],
                hook_payload("session_start", "s1", &outside),
            ),
            (vec!["hook", "--harness", "unknown"], good.clone()),
            (vec!["hook", "--harness"], good.clone()),
            (vec!["hook"], good.clone()),
            (vec!["hook", "--harness", "boom"], good),
        ];
        for (list, payload) in &cases {
            let (code, out, _) = run_thread(list, payload, &env, &adapters);
            assert_eq!((code, out.as_str()), (0, ""), "{list:?}");
        }
        assert_eq!(stored_threads(&repo), 0);
        let log = fs::read_to_string(repo.join(".git/gitmaster/threads.log")).expect("log");
        assert!(log.contains("unknown harness 'unknown'"), "{log}");
    }

    #[test]
    fn hook_drops_a_payload_over_the_limit_without_storing_it() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo();
        let env = Sandbox::env();
        let adapters: [&dyn HarnessAdapter; 1] = [&TestAdapter];
        let mut input = std::io::repeat(b' ').take(PAYLOAD_LIMIT + 1);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = thread_with(
            &words(&["hook", "--harness", "test"]),
            &mut input,
            &mut out,
            &mut err,
            &env,
            &adapters,
        );
        assert_eq!((code, out.len(), err.len()), (0, 0, 0));
        assert!(!repo.join(".git/gitmaster/threads.db").exists());
    }

    #[test]
    fn harness_list_prints_the_ids_sorted() {
        let fake = FakeAdapter;
        let test = TestAdapter;
        let adapters: [&dyn HarnessAdapter; 2] = [&test, &fake];
        let (code, out, _) = run_harness(&["list"], &adapters);
        assert_eq!(code, 0);
        assert_eq!(out, "fake  Fake harness (tests)\ntest  Test harness\n");
    }

    #[test]
    fn the_real_registry_lists_claude_code() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = harness_command(&words(&["list"]), &mut out, &mut err);
        assert_eq!(code, 0);
        assert!(String::from_utf8_lossy(&out).starts_with("claude-code  Claude Code"));
    }

    #[test]
    fn harness_rejects_the_removed_install_commands_and_bad_words() {
        let (code, _, err) = run_harness(&["install", "test"], &[&TestAdapter]);
        assert_eq!(code, 2);
        assert_eq!(
            err,
            "gitmaster harness: unknown subcommand `install`\nusage: gitmaster harness list\n"
        );
        let (code, _, _) = run_harness(&["uninstall", "test"], &[&TestAdapter]);
        assert_eq!(code, 2);
        let (code, _, _) = run_harness(&["list", "extra"], &[&TestAdapter]);
        assert_eq!(code, 2);
        let (code, _, _) = run_harness(&[], &[&TestAdapter]);
        assert_eq!(code, 2);
    }
}
