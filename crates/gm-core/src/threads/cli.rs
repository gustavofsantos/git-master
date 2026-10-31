//! `gitmaster thread …` and `gitmaster harness …` (docs/reference/cli.md, design/threads.md).
//!
//! One-shot commands: each reads its input, does its work and exits. Nothing here opens a
//! window or keeps a process. `thread hook` runs from the agent's hooks and never changes the
//! outcome of the agent's tool call: it always exits `0` and prints nothing to standard output
//! (design/threads.md, "The hook command (D124)"). Everything that depends on the process (the
//! environment, the clock, the running binary) comes in through `Env`, so the commands are
//! tested without touching it.

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::config::{self, ThreadsConfig};
use super::harness::{self, HarnessAdapter, InstallContext};
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
       gitmaster harness install <id> [--run]
       gitmaster harness uninstall <id> [--run]
";

/// What the commands need from the process. Built once by `Env::from_process`.
#[derive(Clone, Debug)]
pub struct Env {
    /// The running `gitmaster`; an integration names it.
    pub binary: PathBuf,
    /// `GitMaster`'s data directory. Integrations live under `agents/<id>/` in it.
    pub data_dir: PathBuf,
    /// The settings file, when one can be found.
    pub settings: Option<PathBuf>,
    /// The time of this invocation, in milliseconds since the Unix epoch.
    pub now_ms: i64,
}

impl Env {
    /// The environment of the running process. `data_dir` is empty when no home directory is
    /// known; the commands that write refuse that.
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            binary: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gitmaster")),
            data_dir: data_dir_from(|name| std::env::var_os(name)),
            settings: config::settings_path(),
            now_ms: now_ms(),
        }
    }
}

/// `$XDG_DATA_HOME/gitmaster` when that is absolute, else `$HOME/.local/share/gitmaster`. On
/// macOS, `$HOME/Library/Application Support/gitmaster`. Empty when no base is known.
fn data_dir_from(lookup: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    let home = lookup("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home.map(|home| home.join("Library/Application Support"))
    } else {
        lookup("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| home.map(|home| home.join(".local/share")))
    };
    base.map(|base| base.join("gitmaster")).unwrap_or_default()
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
pub fn harness_command(
    words: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    env: &Env,
) -> i32 {
    harness_with(words, out, err, env, &registry())
}

/// `harness_command` with the adapters passed in.
fn harness_with(
    words: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    env: &Env,
    adapters: &[&dyn HarnessAdapter],
) -> i32 {
    let Some((command, rest)) = words.split_first() else {
        return usage(err, HARNESS_USAGE);
    };
    let command = command.as_str();
    if !matches!(command, "list" | "install" | "uninstall") {
        let _ = writeln!(err, "gitmaster harness: unknown subcommand `{command}`");
        return usage(err, HARNESS_USAGE);
    }
    if command == "list" {
        if !rest.is_empty() {
            return usage(err, HARNESS_USAGE);
        }
        return list(out, adapters);
    }
    let (id, run) = match parse_target(rest) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "gitmaster harness {command}: {message}");
            return usage(err, HARNESS_USAGE);
        }
    };
    let Some(adapter) = find(adapters, id) else {
        let _ = writeln!(
            err,
            "unknown harness '{id}'; known: {}",
            known_list(adapters)
        );
        return 2;
    };
    if command == "install" {
        install(adapter, env, out, err, run)
    } else {
        uninstall(adapter, env, out, err, run)
    }
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

/// The id and the `--run` flag of `install` or `uninstall`.
fn parse_target(words: &[String]) -> Result<(&str, bool), String> {
    let mut id = None;
    let mut run = false;
    for word in words {
        match word.as_str() {
            "--run" => run = true,
            flag if flag.starts_with('-') => return Err(format!("unknown option `{flag}`")),
            value if id.is_none() => id = Some(value),
            _ => return Err("expected one harness id".to_owned()),
        }
    }
    id.map(|id| (id, run))
        .ok_or_else(|| "missing harness id".to_owned())
}

/// `harness install <id> [--run]`: writes the integration under `agents/<id>/`, prints the
/// steps that add it and, with `run`, executes them.
fn install(
    adapter: &dyn HarnessAdapter,
    env: &Env,
    out: &mut dyn Write,
    err: &mut dyn Write,
    run: bool,
) -> i32 {
    let Some(data) = data_dir_of(env, err) else {
        return 1;
    };
    let dir = agent_dir(data, adapter.id());
    let integration = adapter.integration(&context(&env.binary, data));
    if let Err(message) = write_files(&dir, &integration.files) {
        let _ = writeln!(err, "gitmaster: {message}");
        return 1;
    }
    let _ = writeln!(
        out,
        "Generated the {} integration in {}",
        adapter.display_name(),
        dir.display()
    );
    numbered(out, "Run these commands to add it:", &integration.steps);
    if run {
        run_steps(&integration.steps, out, err)
    } else {
        0
    }
}

/// `harness uninstall <id> [--run]`: prints the steps that remove the integration, runs them
/// with `run`, then removes its directory. The directory stays when a step fails.
fn uninstall(
    adapter: &dyn HarnessAdapter,
    env: &Env,
    out: &mut dyn Write,
    err: &mut dyn Write,
    run: bool,
) -> i32 {
    let Some(data) = data_dir_of(env, err) else {
        return 1;
    };
    let dir = agent_dir(data, adapter.id());
    let integration = adapter.integration(&context(&env.binary, data));
    numbered(
        out,
        "Run these commands to remove it:",
        &integration.uninstall_steps,
    );
    if run {
        let code = run_steps(&integration.uninstall_steps, out, err);
        if code != 0 {
            let _ = writeln!(
                err,
                "the integration was kept; fix the failure and run uninstall again"
            );
            return code;
        }
    }
    if !dir.starts_with(data) || dir == data {
        let _ = writeln!(err, "gitmaster: refusing to remove {}", dir.display());
        return 1;
    }
    match fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            let _ = writeln!(err, "gitmaster: cannot remove {}: {error}", dir.display());
            return 1;
        }
    }
    let _ = writeln!(out, "Removed the integration in {}", dir.display());
    let _ = writeln!(out, "Stored threads are kept.");
    0
}

/// Writes each generated file under `dir`, atomically. A path that is absolute or contains
/// anything but plain names is refused before any file is written.
fn write_files(dir: &Path, files: &[(PathBuf, String)]) -> Result<(), String> {
    if let Some((relative, _)) = files
        .iter()
        .find(|(relative, _)| !is_plain_relative(relative))
    {
        return Err(format!(
            "refusing the integration path {}",
            relative.display()
        ));
    }
    for (relative, contents) in files {
        let target = dir.join(relative);
        write_atomic(&target, contents)
            .map_err(|error| format!("cannot write {}: {error}", target.display()))?;
    }
    Ok(())
}

fn is_plain_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

/// Writes `contents` to a temporary file beside `target`, then renames it over `target`.
fn write_atomic(target: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut temp = target.as_os_str().to_owned();
    temp.push(format!(".tmp-{}", std::process::id()));
    let temp = PathBuf::from(temp);
    fs::write(&temp, contents)?;
    fs::rename(&temp, target).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// Runs each step with `sh -c`, in order, and stops at the first failure. Returns `0` when all
/// succeed, `1` otherwise.
fn run_steps(steps: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    for (index, step) in steps.iter().enumerate() {
        let number = index + 1;
        match Command::new("sh").arg("-c").arg(step).output() {
            Ok(output) => {
                let _ = out.write_all(&output.stdout);
                let _ = err.write_all(&output.stderr);
                if !output.status.success() {
                    let _ = writeln!(err, "step {number} failed: {step}");
                    return 1;
                }
            }
            Err(error) => {
                let _ = writeln!(err, "step {number} could not start: {error}");
                return 1;
            }
        }
    }
    0
}

fn numbered(out: &mut dyn Write, heading: &str, steps: &[String]) {
    if steps.is_empty() {
        return;
    }
    let _ = writeln!(out, "{heading}");
    for (index, step) in steps.iter().enumerate() {
        let _ = writeln!(out, "  {}. {step}", index + 1);
    }
}

/// The data directory, when it is absolute. Writing a relative path would land in the working
/// directory, so the commands that write refuse it.
fn data_dir_of<'a>(env: &'a Env, err: &mut dyn Write) -> Option<&'a Path> {
    if env.data_dir.is_absolute() {
        return Some(env.data_dir.as_path());
    }
    let _ = writeln!(
        err,
        "gitmaster: no data directory; set HOME or XDG_DATA_HOME"
    );
    None
}

fn agent_dir(data: &Path, id: &str) -> PathBuf {
    data.join("agents").join(id)
}

fn context(binary: &Path, data: &Path) -> InstallContext {
    InstallContext {
        binary: binary.to_path_buf(),
        data_dir: data.to_path_buf(),
    }
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

fn known_list(adapters: &[&dyn HarnessAdapter]) -> String {
    let mut ids: Vec<&str> = adapters.iter().map(|adapter| adapter.id()).collect();
    ids.sort_unstable();
    ids.join(", ")
}

/// Writes a usage text and returns the usage exit status.
fn usage(err: &mut dyn Write, text: &str) -> i32 {
    let _ = err.write_all(text.as_bytes());
    2
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::threads::harness::fake::FakeAdapter;
    use crate::threads::harness::{
        Capabilities, HookInput, Integration, LogChunk, ParseError, SessionLog,
    };
    use gm_schema::threads::ThreadFilter;

    /// An adapter for the tests. It parses the fake harness's payloads and generates an
    /// integration whose hooks file names the binary and whose steps the test sets.
    struct TestAdapter {
        steps: Vec<String>,
        uninstall: Vec<String>,
        /// Adds a file whose path leaves the agent directory.
        escape: bool,
    }

    impl TestAdapter {
        fn plain() -> Self {
            Self {
                steps: vec!["true".to_owned()],
                uninstall: vec!["true".to_owned()],
                escape: false,
            }
        }
    }

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

        fn integration(&self, ctx: &InstallContext) -> Integration {
            let command = format!("'{}' thread hook --harness test", ctx.binary.display());
            let hooks = json!({"hooks": [{"type": "command", "command": command}]});
            let mut files = vec![(PathBuf::from("hooks/hooks.json"), hooks.to_string())];
            if self.escape {
                files.push((PathBuf::from("../escape.txt"), "x".to_owned()));
            }
            Integration {
                files,
                steps: self.steps.clone(),
                uninstall_steps: self.uninstall.clone(),
            }
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

        fn integration(&self, _ctx: &InstallContext) -> Integration {
            Integration {
                files: Vec::new(),
                steps: Vec::new(),
                uninstall_steps: Vec::new(),
            }
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

        fn env(&self, binary: &str) -> Env {
            Env {
                binary: PathBuf::from(binary),
                data_dir: self.root().join("data"),
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

    fn run_harness(
        list: &[&str],
        env: &Env,
        adapters: &[&dyn HarnessAdapter],
    ) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = harness_with(&words(list), &mut out, &mut err, env, adapters);
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
    fn data_dir_follows_xdg_then_home() {
        let lookup = |vars: &[(&str, &str)]| {
            let map: HashMap<String, String> = vars
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect();
            move |name: &str| map.get(name).map(OsString::from)
        };
        if cfg!(target_os = "macos") {
            return;
        }
        let absolute = lookup(&[("XDG_DATA_HOME", "/xdg"), ("HOME", "/home/me")]);
        assert_eq!(data_dir_from(absolute), PathBuf::from("/xdg/gitmaster"));
        let relative = lookup(&[("XDG_DATA_HOME", "xdg"), ("HOME", "/home/me")]);
        assert_eq!(
            data_dir_from(relative),
            PathBuf::from("/home/me/.local/share/gitmaster")
        );
        let nothing = lookup(&[]);
        assert_eq!(data_dir_from(nothing), PathBuf::new());
    }

    #[test]
    fn thread_without_a_known_word_is_a_usage_error() {
        let sandbox = Sandbox::new();
        let env = sandbox.env("/bin/gitmaster");
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
        let mut env = sandbox.env("/bin/gitmaster");
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
        let sandbox = Sandbox::new();
        let env = sandbox.env("/bin/gitmaster");
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
        let mut env = sandbox.env("/bin/gitmaster");
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
        let sandbox = Sandbox::new();
        let env = sandbox.env("/bin/gitmaster");
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
        let env = sandbox.env("/bin/gitmaster");
        let adapters: [&dyn HarnessAdapter; 1] = [&TestAdapter::plain()];
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
        let env = sandbox.env("/bin/gitmaster");
        let adapters: [&dyn HarnessAdapter; 2] = [&TestAdapter::plain(), &Boom];
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
        let env = sandbox.env("/bin/gitmaster");
        let adapters: [&dyn HarnessAdapter; 1] = [&TestAdapter::plain()];
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
        let sandbox = Sandbox::new();
        let env = sandbox.env("/bin/gitmaster");
        let fake = FakeAdapter;
        let test = TestAdapter::plain();
        let adapters: [&dyn HarnessAdapter; 2] = [&test, &fake];
        let (code, out, _) = run_harness(&["list"], &env, &adapters);
        assert_eq!(code, 0);
        assert_eq!(out, "fake  Fake harness (tests)\ntest  Test harness\n");
    }

    #[test]
    fn the_real_registry_lists_claude_code() {
        let sandbox = Sandbox::new();
        let env = sandbox.env("/bin/gitmaster");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = harness_command(&words(&["list"]), &mut out, &mut err, &env);
        assert_eq!(code, 0);
        assert!(String::from_utf8_lossy(&out).starts_with("claude-code  Claude Code"));
    }

    #[test]
    fn harness_rejects_unknown_ids_and_bad_words() {
        let sandbox = Sandbox::new();
        let env = sandbox.env("/bin/gitmaster");
        let test = TestAdapter::plain();
        let adapters: [&dyn HarnessAdapter; 1] = [&test];
        let (code, _, err) = run_harness(&["install", "nope"], &env, &adapters);
        assert_eq!(code, 2);
        assert_eq!(err, "unknown harness 'nope'; known: test\n");
        let (code, _, _) = run_harness(&["install"], &env, &adapters);
        assert_eq!(code, 2);
        let (code, _, _) = run_harness(&["status", "test"], &env, &adapters);
        assert_eq!(code, 2);
        let (code, _, _) = run_harness(&["bogus"], &env, &adapters);
        assert_eq!(code, 2);
    }

    #[test]
    fn install_writes_the_integration_and_lists_the_steps() {
        let sandbox = Sandbox::new();
        let env = sandbox.env("/opt/gm/gitmaster");
        let test = TestAdapter {
            steps: vec!["echo added".to_owned()],
            ..TestAdapter::plain()
        };
        let adapters: [&dyn HarnessAdapter; 1] = [&test];
        let (code, out, _) = run_harness(&["install", "test"], &env, &adapters);
        assert_eq!(code, 0);
        let dir = env.data_dir.join("agents/test");
        assert!(out.starts_with(&format!(
            "Generated the Test harness integration in {}\n",
            dir.display()
        )));
        assert!(out.contains("Run these commands to add it:\n  1. echo added\n"));
        let hooks = fs::read_to_string(dir.join("hooks/hooks.json")).expect("hooks");
        assert!(hooks.contains("'/opt/gm/gitmaster' thread hook --harness test"));
    }

    #[test]
    fn install_with_run_stops_at_the_first_failing_step() {
        let sandbox = Sandbox::new();
        let env = sandbox.env("/opt/gm/gitmaster");
        let first = sandbox.root().join("first");
        let second = sandbox.root().join("second");
        let test = TestAdapter {
            steps: vec![
                format!("touch {}", first.display()),
                "false".to_owned(),
                format!("touch {}", second.display()),
            ],
            ..TestAdapter::plain()
        };
        let adapters: [&dyn HarnessAdapter; 1] = [&test];
        let (code, _, err) = run_harness(&["install", "test", "--run"], &env, &adapters);
        assert_eq!(code, 1);
        assert!(err.contains("step 2 failed: false"), "{err}");
        assert!(first.exists());
        assert!(!second.exists());
    }

    #[test]
    fn install_with_run_succeeds_when_every_step_does() {
        let sandbox = Sandbox::new();
        let env = sandbox.env("/opt/gm/gitmaster");
        let marker = sandbox.root().join("marker");
        let test = TestAdapter {
            steps: vec![format!("touch {}", marker.display())],
            ..TestAdapter::plain()
        };
        let adapters: [&dyn HarnessAdapter; 1] = [&test];
        let (code, _, _) = run_harness(&["install", "test", "--run"], &env, &adapters);
        assert_eq!(code, 0);
        assert!(marker.exists());
    }

    #[test]
    fn install_refuses_a_path_that_leaves_the_agent_directory() {
        let sandbox = Sandbox::new();
        let env = sandbox.env("/opt/gm/gitmaster");
        let test = TestAdapter {
            escape: true,
            ..TestAdapter::plain()
        };
        let adapters: [&dyn HarnessAdapter; 1] = [&test];
        let (code, _, err) = run_harness(&["install", "test"], &env, &adapters);
        assert_eq!(code, 1);
        assert!(err.contains("refusing the integration path"), "{err}");
        assert!(!env.data_dir.join("escape.txt").exists());
        assert!(!env.data_dir.join("agents").exists());
    }

    #[test]
    fn install_needs_an_absolute_data_directory() {
        let sandbox = Sandbox::new();
        let mut env = sandbox.env("/opt/gm/gitmaster");
        env.data_dir = PathBuf::new();
        let test = TestAdapter::plain();
        let adapters: [&dyn HarnessAdapter; 1] = [&test];
        let (code, _, err) = run_harness(&["install", "test"], &env, &adapters);
        assert_eq!(code, 1);
        assert!(err.contains("no data directory"));
    }

    #[test]
    fn uninstall_removes_the_directory_and_keeps_threads() {
        let sandbox = Sandbox::new();
        let env = sandbox.env("/opt/gm/gitmaster");
        let test = TestAdapter {
            uninstall: vec!["echo removed".to_owned()],
            ..TestAdapter::plain()
        };
        let adapters: [&dyn HarnessAdapter; 1] = [&test];
        run_harness(&["install", "test"], &env, &adapters);
        let (code, out, _) = run_harness(&["uninstall", "test"], &env, &adapters);
        assert_eq!(code, 0);
        assert!(out.contains("Run these commands to remove it:\n  1. echo removed\n"));
        assert!(out.contains("Stored threads are kept.\n"));
        assert!(!env.data_dir.join("agents/test").exists());
        assert!(env.data_dir.exists());
    }

    #[test]
    fn uninstall_keeps_the_integration_when_a_step_fails() {
        let sandbox = Sandbox::new();
        let env = sandbox.env("/opt/gm/gitmaster");
        let test = TestAdapter {
            uninstall: vec!["false".to_owned()],
            ..TestAdapter::plain()
        };
        let adapters: [&dyn HarnessAdapter; 1] = [&test];
        run_harness(&["install", "test"], &env, &adapters);
        let (code, _, err) = run_harness(&["uninstall", "test", "--run"], &env, &adapters);
        assert_eq!(code, 1);
        assert!(err.contains("step 1 failed"), "{err}");
        assert!(env.data_dir.join("agents/test/hooks/hooks.json").exists());
    }
}
