//! The `exec` action (design/workflow-scripts.md, D74): runs a command line directly, never
//! through a shell. Each argument is one argv element, so repository content
//! interpolated into it cannot inject flags or commands.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Used when `with.timeout` is not set.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Stdout beyond this is cut and `truncated` is set in the output.
pub const STDOUT_LIMIT: usize = 1024 * 1024;
/// Stderr kept for the error message.
pub const STDERR_LIMIT: usize = 16 * 1024;

const POLL: Duration = Duration::from_millis(20);
const KEYS: [&str; 4] = ["argv", "stdin", "timeout", "format"];
const ARGV_ERROR: &str = "`argv` must be a non-empty list of strings";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecSpec {
    pub argv: Vec<String>,
    pub stdin: Option<String>,
    pub timeout: Duration,
    /// Where the process runs: the repository root.
    pub cwd: PathBuf,
    /// `format = "json"`: stdout must be JSON, exposed as `output.json`.
    pub json: bool,
    /// The PATH the program is looked up in and runs with: the login shell's,
    /// as git gets it. `None` keeps the app's own, which is the system's
    /// minimal PATH when the window was opened from the Dock or the Finder.
    pub path: Option<String>,
}

/// Reads the rendered `with` of an `exec` state: `argv` (non-empty list of
/// strings), `stdin` (string), `timeout` (`"<n>s|m|h"`), `format` (`"json"`).
/// A leading `~/` in `argv[0]` is the home directory.
pub fn spec(input: &Value, cwd: &Path) -> Result<ExecSpec, String> {
    let fields = input
        .as_object()
        .ok_or_else(|| "`with` must be an object".to_owned())?;
    if let Some(key) = fields.keys().find(|key| !KEYS.contains(&key.as_str())) {
        return Err(format!("unknown key `{key}`"));
    }

    let mut argv = fields
        .get("argv")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .and_then(|items| {
            items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect::<Option<Vec<String>>>()
        })
        .ok_or_else(|| ARGV_ERROR.to_owned())?;
    let expanded = argv[0].strip_prefix("~/").and_then(|rest| {
        std::env::var_os("HOME").map(|home| format!("{}/{rest}", home.to_string_lossy()))
    });
    if let Some(program) = expanded {
        argv[0] = program;
    }

    let stdin = fields
        .get("stdin")
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "`stdin` must be a string".to_owned())
        })
        .transpose()?;

    let timeout = match fields.get("timeout") {
        None => DEFAULT_TIMEOUT,
        Some(value) => parse_timeout(
            value
                .as_str()
                .ok_or_else(|| "`timeout` must be a string such as \"30s\"".to_owned())?,
        )?,
    };

    let json = match fields.get("format") {
        None => false,
        Some(value) if value.as_str() == Some("json") => true,
        Some(_) => return Err("`format` must be \"json\"".to_owned()),
    };

    Ok(ExecSpec {
        argv,
        stdin,
        timeout,
        cwd: cwd.to_path_buf(),
        json,
        path: None,
    })
}

/// `<positive integer><s|m|h>`.
fn parse_timeout(text: &str) -> Result<Duration, String> {
    const SHAPE: &str =
        "`timeout` must be a positive whole number followed by s, m or h, such as \"30s\"";
    let (digits, seconds_per_unit) = if let Some(digits) = text.strip_suffix('s') {
        (digits, 1)
    } else if let Some(digits) = text.strip_suffix('m') {
        (digits, 60)
    } else if let Some(digits) = text.strip_suffix('h') {
        (digits, 3600)
    } else {
        return Err(SHAPE.to_owned());
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(SHAPE.to_owned());
    }
    let amount: u64 = digits
        .parse()
        .map_err(|_| "`timeout` is too long".to_owned())?;
    if amount == 0 {
        return Err(SHAPE.to_owned());
    }
    let seconds = amount
        .checked_mul(seconds_per_unit)
        .ok_or_else(|| "`timeout` is too long".to_owned())?;
    Ok(Duration::from_secs(seconds))
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExecResult {
    /// `done`, `failed` or `timeout` (catalog outcomes of `exec`).
    pub outcome: &'static str,
    /// `{ "text", "exit_code", "stderr", "truncated" }`, plus `"json"` in
    /// JSON format, plus `"error"` when the process could not start or was
    /// cancelled.
    pub output: Value,
}

/// Runs the process in its own process group, feeds `stdin`, reads stdout
/// and stderr, and kills the group on timeout or when `cancel` is set.
pub fn run(spec: &ExecSpec, cancel: &AtomicBool) -> ExecResult {
    let Some((program, args)) = spec.argv.split_first() else {
        return failure("failed", "argv is empty");
    };
    let mut command = Command::new(program);
    if let Some(path) = &spec.path {
        // Set before spawning: the program is then looked up in this PATH too.
        command.env("PATH", path);
    }
    command
        .args(args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return failure("failed", &format!("cannot start `{program}`: {error}")),
    };
    let group = child.id();

    // Writing on its own thread: a process that never reads stdin must not wedge us.
    // Without stdin the pipe is closed at once.
    if let Some(mut stdin) = child.stdin.take()
        && let Some(input) = spec.stdin.clone()
    {
        thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let stdout_reader = child
        .stdout
        .take()
        .map(|pipe| thread::spawn(move || read_capped(pipe, STDOUT_LIMIT)));
    let stderr_reader = child
        .stderr
        .take()
        .map(|pipe| thread::spawn(move || read_capped(pipe, STDERR_LIMIT)));

    let started = Instant::now();
    let status: ExitStatus = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                kill_group(group);
                let _ = child.wait();
                return failure("failed", &format!("cannot wait for `{program}`: {error}"));
            }
        }
        if cancel.load(Ordering::SeqCst) {
            kill_group(group);
            let _ = child.wait();
            return failure("failed", "cancelled");
        }
        if started.elapsed() >= spec.timeout {
            kill_group(group);
            let _ = child.wait();
            return finished("timeout", "", None, "", false);
        }
        thread::sleep(POLL);
    };
    // Whatever the process left behind (a background child) would keep the pipes open.
    kill_group(group);
    let (stdout, truncated) = stdout_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let (stderr, _) = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();

    let text = String::from_utf8_lossy(&stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
    if !status.success() {
        return finished("failed", &text, status.code(), &stderr, truncated);
    }
    let mut result = finished("done", &text, status.code(), &stderr, truncated);
    if spec.json {
        match serde_json::from_slice::<Value>(&stdout) {
            Ok(parsed) => result.output["json"] = parsed,
            Err(error) => {
                result.outcome = "failed";
                result.output["error"] = json!(format!("stdout is not valid JSON: {error}"));
            }
        }
    }
    result
}

/// An outcome with the standard output fields.
fn finished(
    outcome: &'static str,
    text: &str,
    exit_code: Option<i32>,
    stderr: &str,
    truncated: bool,
) -> ExecResult {
    ExecResult {
        outcome,
        output: json!({
            "text": text,
            "exit_code": exit_code,
            "stderr": stderr,
            "truncated": truncated,
        }),
    }
}

/// A `failed` outcome for a process that never produced output.
fn failure(outcome: &'static str, error: &str) -> ExecResult {
    let mut result = finished(outcome, "", None, "", false);
    result.output["error"] = json!(error);
    result
}

/// Reads the pipe to its end. Bytes beyond `limit` are read and discarded, so
/// the process never blocks on a full pipe; the flag says that it was cut.
fn read_capped(mut pipe: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0_u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                let room = limit - bytes.len();
                if read <= room {
                    bytes.extend_from_slice(&chunk[..read]);
                } else {
                    bytes.extend_from_slice(&chunk[..room]);
                    truncated = true;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    (bytes, truncated)
}

/// Kills the process group led by `group`. An already-gone group is not an error.
fn kill_group(group: u32) {
    let _ = Command::new("kill")
        .args(["-s", "KILL", "--", &format!("-{group}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use super::*;

    fn exec(argv: &[&str], stdin: Option<&str>, timeout: Duration, json: bool) -> ExecSpec {
        ExecSpec {
            argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
            stdin: stdin.map(str::to_owned),
            timeout,
            cwd: std::env::temp_dir(),
            json,
            path: None,
        }
    }

    fn go(argv: &[&str]) -> ExecResult {
        run(
            &exec(argv, None, Duration::from_secs(10), false),
            &AtomicBool::new(false),
        )
    }

    fn spec_error(input: &Value) -> String {
        spec(input, &std::env::temp_dir()).unwrap_err()
    }

    #[test]
    fn stdout_is_the_trimmed_text_of_a_done_run() {
        let result = go(&["printf", "hello"]);
        assert_eq!(result.outcome, "done");
        assert_eq!(result.output["text"], "hello");
        assert_eq!(result.output["exit_code"], 0);
        assert_eq!(result.output["truncated"], false);
    }

    #[test]
    fn stdin_reaches_the_process() {
        let result = run(
            &exec(&["cat"], Some("abc"), Duration::from_secs(10), false),
            &AtomicBool::new(false),
        );
        assert_eq!(result.outcome, "done");
        assert_eq!(result.output["text"], "abc");
    }

    #[test]
    fn a_non_zero_exit_fails_with_its_code() {
        let result = go(&["false"]);
        assert_eq!(result.outcome, "failed");
        assert_eq!(result.output["exit_code"], 1);
        assert!(result.output.get("error").is_none());
    }

    #[test]
    fn the_exit_code_of_the_program_is_kept() {
        let result = go(&["sh", "-c", "exit 3"]);
        assert_eq!(result.outcome, "failed");
        assert_eq!(result.output["exit_code"], 3);
    }

    #[test]
    fn a_slow_process_times_out() {
        let started = Instant::now();
        let result = run(
            &exec(&["sleep", "5"], None, Duration::from_secs(1), false),
            &AtomicBool::new(false),
        );
        assert_eq!(result.outcome, "timeout");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_cancelled_run_is_killed_and_fails() {
        let cancel = AtomicBool::new(false);
        let spec = exec(&["sleep", "5"], None, Duration::from_secs(10), false);
        let result = thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(200));
                cancel.store(true, Ordering::SeqCst);
            });
            run(&spec, &cancel)
        });
        assert_eq!(result.outcome, "failed");
        assert_eq!(result.output["error"], "cancelled");
    }

    #[test]
    fn a_missing_program_fails_naming_it() {
        let result = go(&["gm-no-such-program-anywhere"]);
        assert_eq!(result.outcome, "failed");
        let error = result.output["error"].as_str().unwrap();
        assert!(error.contains("gm-no-such-program-anywhere"), "{error}");
        assert_eq!(result.output["exit_code"], Value::Null);
    }

    #[test]
    fn json_format_exposes_the_parsed_value() {
        let result = run(
            &exec(
                &["printf", "{\"a\":1}"],
                None,
                Duration::from_secs(10),
                true,
            ),
            &AtomicBool::new(false),
        );
        assert_eq!(result.outcome, "done");
        assert_eq!(result.output["json"]["a"], 1);
    }

    #[test]
    fn json_format_fails_on_invalid_json() {
        let result = run(
            &exec(&["printf", "nope"], None, Duration::from_secs(10), true),
            &AtomicBool::new(false),
        );
        assert_eq!(result.outcome, "failed");
        let error = result.output["error"].as_str().unwrap();
        assert!(error.starts_with("stdout is not valid JSON: "), "{error}");
        assert!(result.output.get("json").is_none());
    }

    #[test]
    fn arguments_are_passed_literally() {
        let arg = "; rm -rf / $(whoami)";
        let result = go(&["printf", "%s", arg]);
        assert_eq!(result.outcome, "done");
        assert_eq!(result.output["text"], arg);
    }

    #[test]
    fn stdout_beyond_the_limit_is_cut_and_the_process_still_ends() {
        let result = go(&["head", "-c", "2000000", "/dev/zero"]);
        assert_eq!(result.outcome, "done");
        assert_eq!(result.output["truncated"], true);
        assert_eq!(result.output["text"].as_str().unwrap().len(), STDOUT_LIMIT);
    }

    #[test]
    fn spec_reads_argv_stdin_timeout_and_format() {
        let parsed = spec(
            &json!({"argv": ["llm", "-p", "x"], "stdin": "diff", "timeout": "2m", "format": "json"}),
            Path::new("/repo"),
        )
        .unwrap();
        assert_eq!(parsed.argv, ["llm", "-p", "x"]);
        assert_eq!(parsed.stdin.as_deref(), Some("diff"));
        assert_eq!(parsed.timeout, Duration::from_secs(120));
        assert_eq!(parsed.cwd, Path::new("/repo"));
        assert!(parsed.json);

        let defaults = spec(&json!({"argv": ["llm"]}), Path::new("/repo")).unwrap();
        assert_eq!(defaults.timeout, DEFAULT_TIMEOUT);
        assert_eq!(defaults.stdin, None);
        assert!(!defaults.json);
    }

    #[test]
    fn spec_rejects_a_missing_or_empty_argv() {
        assert_eq!(spec_error(&json!({})), ARGV_ERROR);
        assert_eq!(spec_error(&json!({"argv": []})), ARGV_ERROR);
        assert_eq!(spec_error(&json!({"argv": "git"})), ARGV_ERROR);
    }

    #[test]
    fn spec_rejects_a_non_string_argv_item() {
        assert_eq!(spec_error(&json!({"argv": ["git", 3]})), ARGV_ERROR);
    }

    #[test]
    fn spec_rejects_a_bad_timeout() {
        for bad in [
            "",
            "s",
            "0s",
            "-1s",
            "1.5s",
            "10",
            "10d",
            "+5m",
            "99999999999999999999h",
        ] {
            let error = spec_error(&json!({"argv": ["x"], "timeout": bad}));
            assert!(error.starts_with("`timeout`"), "{bad}: {error}");
        }
    }

    #[test]
    fn spec_rejects_an_unknown_key_by_name() {
        let error = spec_error(&json!({"argv": ["x"], "shell": true}));
        assert_eq!(error, "unknown key `shell`");
    }

    #[test]
    fn spec_accepts_only_json_format() {
        assert_eq!(
            spec_error(&json!({"argv": ["x"], "format": "yaml"})),
            "`format` must be \"json\""
        );
    }

    #[test]
    fn spec_rejects_a_non_string_stdin() {
        assert_eq!(
            spec_error(&json!({"argv": ["x"], "stdin": 1})),
            "`stdin` must be a string"
        );
    }

    #[test]
    fn a_leading_tilde_in_argv0_is_the_home_directory() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let parsed = spec(&json!({"argv": ["~/tool", "~/kept"]}), Path::new("/repo")).unwrap();
        assert_eq!(parsed.argv[0], format!("{}/tool", home.to_string_lossy()));
        assert_eq!(parsed.argv[1], "~/kept");
    }

    #[test]
    fn finds_the_program_in_the_given_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::os::unix::fs::symlink("/bin/echo", dir.path().join("gm-probe-echo")).expect("link");
        let mut spec = exec(
            &["gm-probe-echo", "hi"],
            None,
            Duration::from_secs(5),
            false,
        );
        let cancel = AtomicBool::new(false);
        assert_eq!(run(&spec, &cancel).outcome, "failed");
        spec.path = Some(format!("{}:/usr/bin:/bin", dir.path().display()));
        let found = run(&spec, &cancel);
        assert_eq!(found.outcome, "done");
        assert_eq!(found.output["text"], "hi");
    }
}
