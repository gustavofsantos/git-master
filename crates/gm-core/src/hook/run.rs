//! Runs a chain of hook steps off the engine thread. A `Plan` is data: the
//! engine builds it, and `execute` runs it on its own thread, reporting each
//! step back. Nothing here touches the repository's index or working tree.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use gm_schema::{
    CONTRACT_VERSION, HookFailure, HookInput, HookOutput, HookPayload, HookRepo, OperationId,
    PREPARE_COMMIT,
};

use crate::command_log::{CommandLog, CommandRecord, Source};

/// Staged diffs above this size go to a temporary file (design/hooks-transitional.md).
pub const INLINE_DIFF_LIMIT: usize = 64 * 1024;

/// The most stdout a step may print; more is cut and then fails as invalid output.
const MAX_STDOUT: u64 = 1024 * 1024;
/// The most stderr kept; only its first lines reach the user.
const MAX_STDERR: u64 = 8 * 1024;
const STDERR_LINES: usize = 5;
const POLL: Duration = Duration::from_millis(10);

/// One script of the chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    pub command: String,
    pub timeout: Duration,
}

/// Everything a run needs, captured when it was accepted.
#[derive(Clone, Debug)]
pub struct Plan {
    pub run: OperationId,
    pub root: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub staged_diff: String,
    pub message: String,
    pub steps: Vec<Step>,
    /// The login-shell PATH, read when each step starts; the steps run with it.
    pub login_path: Arc<OnceLock<String>>,
    /// Set by `CancelHook`; the running step is killed and the run ends cancelled.
    pub cancel: Arc<AtomicBool>,
    /// Where each step that ends is recorded, when the adapter keeps a log.
    pub log: Option<CommandLog>,
}

/// What a run tells the engine, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    Started {
        run: OperationId,
        hook: String,
        step: u32,
        of: u32,
    },
    /// A step exited 0 and answered as the contract asks.
    StepDone {
        run: OperationId,
        hook: String,
    },
    Finished {
        run: OperationId,
        message: String,
    },
    Failed {
        run: OperationId,
        hook: String,
        failure: HookFailure,
    },
    Cancelled {
        run: OperationId,
        hook: String,
    },
}

enum StepError {
    Cancelled,
    Failed(HookFailure),
}

/// Delivers the reports of one run. A run that ends without a terminal report,
/// by a panic or an early return, delivers `Failed` on drop, so the engine
/// never waits for a run that is gone.
struct Reporter<F: FnMut(Report)> {
    deliver: F,
    run: OperationId,
    /// The step the run was in, for the failure report.
    hook: String,
    terminal: bool,
}

impl<F: FnMut(Report)> Reporter<F> {
    fn send(&mut self, report: Report) {
        match &report {
            Report::Started { hook, .. } => self.hook.clone_from(hook),
            Report::Finished { .. } | Report::Failed { .. } | Report::Cancelled { .. } => {
                self.terminal = true;
            }
            Report::StepDone { .. } => {}
        }
        (self.deliver)(report);
    }
}

impl<F: FnMut(Report)> Drop for Reporter<F> {
    fn drop(&mut self) {
        if !self.terminal {
            (self.deliver)(Report::Failed {
                run: self.run,
                hook: self.hook.clone(),
                failure: HookFailure::CannotStart {
                    detail: "the hook run stopped without an answer".to_owned(),
                },
            });
        }
    }
}

impl Plan {
    /// Runs the steps in order, delivering each report. Stops at the first
    /// step that fails or is cancelled. However the run ends, its last report
    /// is terminal, and the staged-diff file is deleted.
    pub fn execute(self, deliver: impl FnMut(Report)) {
        let hook = self
            .steps
            .first()
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let mut reporter = Reporter {
            deliver,
            run: self.run,
            hook,
            terminal: false,
        };
        self.execute_steps(&mut reporter);
    }

    fn record(&self, step: &Step, started: Instant, result: &Result<String, StepError>) {
        if let Some(log) = &self.log {
            log.record(step_record(&self.root, step, started.elapsed(), result));
        }
    }

    fn execute_steps<F: FnMut(Report)>(self, reporter: &mut Reporter<F>) {
        let of = u32::try_from(self.steps.len()).unwrap_or(u32::MAX);
        // Held until the chain ends, then deleted.
        let mut diff_file = None;
        let inline = self.staged_diff.len() <= INLINE_DIFF_LIMIT;
        if !inline {
            match write_temp_diff(&self.staged_diff) {
                Ok(file) => diff_file = Some(file),
                Err(error) => {
                    let hook = self
                        .steps
                        .first()
                        .map(|s| s.name.clone())
                        .unwrap_or_default();
                    reporter.send(Report::Failed {
                        run: self.run,
                        hook,
                        failure: HookFailure::CannotStart {
                            detail: format!("cannot write the staged diff: {error}"),
                        },
                    });
                    return;
                }
            }
        }
        let root = self.root.display().to_string();
        let mut message = self.message.clone();
        for (index, step) in self.steps.iter().enumerate() {
            let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
            if self.cancel.load(Ordering::SeqCst) {
                reporter.send(Report::Cancelled {
                    run: self.run,
                    hook: step.name.clone(),
                });
                return;
            }
            reporter.send(Report::Started {
                run: self.run,
                hook: step.name.clone(),
                step: number,
                of,
            });
            let invocation_id = format!("prep-{}-{number}", self.run.0);
            let payload = HookPayload {
                message: message.clone(),
                staged_diff: inline.then(|| self.staged_diff.clone()),
                staged_diff_path: diff_file
                    .as_ref()
                    .map(|file: &tempfile::NamedTempFile| file.path().display().to_string()),
            };
            let input = HookInput {
                schema_version: CONTRACT_VERSION,
                event: PREPARE_COMMIT.to_owned(),
                invocation_id: invocation_id.clone(),
                repo: HookRepo {
                    root: root.clone(),
                    branch: self.branch.clone(),
                    head: self.head.clone(),
                },
                input: payload,
            };
            let json = serde_json::to_vec(&input).unwrap_or_default();
            let started = Instant::now();
            let result = run_step(
                step,
                &invocation_id,
                &json,
                &self.root,
                &self.login_path,
                &self.cancel,
            );
            self.record(step, started, &result);
            match result {
                Ok(next) => {
                    message = next;
                    reporter.send(Report::StepDone {
                        run: self.run,
                        hook: step.name.clone(),
                    });
                }
                Err(StepError::Cancelled) => {
                    reporter.send(Report::Cancelled {
                        run: self.run,
                        hook: step.name.clone(),
                    });
                    return;
                }
                Err(StepError::Failed(failure)) => {
                    reporter.send(Report::Failed {
                        run: self.run,
                        hook: step.name.clone(),
                        failure,
                    });
                    return;
                }
            }
        }
        reporter.send(Report::Finished {
            run: self.run,
            message,
        });
    }
}

/// The log's entry for one step: its command, exit code, duration and a
/// one-line outcome. Neither the input (the staged diff) nor the output is kept.
fn step_record(
    root: &std::path::Path,
    step: &Step,
    took: Duration,
    result: &Result<String, StepError>,
) -> CommandRecord {
    let (code, outcome) = match result {
        Ok(_) => (Some(0), "ok".to_owned()),
        Err(StepError::Cancelled) => (None, "cancelled".to_owned()),
        Err(StepError::Failed(failure)) => match failure {
            HookFailure::Exited { code, .. } => (
                *code,
                code.map_or_else(
                    || "killed by a signal".to_owned(),
                    |code| format!("exit {code}"),
                ),
            ),
            HookFailure::Timeout { timeout_ms } => {
                (None, format!("timed out after {timeout_ms} ms"))
            }
            // These come only after the step exited 0.
            HookFailure::InvalidOutput { .. } => (Some(0), "invalid output".to_owned()),
            HookFailure::InvocationMismatch { .. } => {
                (Some(0), "answered another invocation".to_owned())
            }
            HookFailure::CannotStart { .. } => (None, "cannot start".to_owned()),
        },
    };
    CommandRecord {
        source: Source::Hook {
            name: step.name.clone(),
            outcome,
        },
        program: "sh".to_owned(),
        dir: root.to_path_buf(),
        args: vec![step.command.clone()],
        code,
        stdout_bytes: 0,
        stdout: String::new(),
        stderr: String::new(),
        took,
    }
}

fn write_temp_diff(diff: &str) -> std::io::Result<tempfile::NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("gm-staged-")
        .suffix(".diff")
        .tempfile()?;
    file.write_all(diff.as_bytes())?;
    file.flush()?;
    Ok(file)
}

/// Runs one step: `sh -c` in the repository root with the login-shell PATH
/// when it is known, the input on stdin, the answer read from stdout. The step gets a process group of its own, so a
/// timeout or cancel takes its children with it.
fn run_step(
    step: &Step,
    invocation_id: &str,
    input: &[u8],
    root: &std::path::Path,
    login_path: &OnceLock<String>,
    cancel: &AtomicBool,
) -> Result<String, StepError> {
    let mut command = Command::new("sh");
    if let Some(path) = login_path.get() {
        command.env("PATH", path);
    }
    command
        .arg("-c")
        .arg(&step.command)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn().map_err(|error| {
        StepError::Failed(HookFailure::CannotStart {
            detail: error.to_string(),
        })
    })?;
    let group = child.id();

    // Writing on its own thread: a step that never reads stdin must not wedge us.
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_vec();
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let stdout = child.stdout.take();
    let stdout_reader = thread::spawn(move || read_capped(stdout, MAX_STDOUT));
    let stderr = child.stderr.take();
    let stderr_reader = thread::spawn(move || read_capped(stderr, MAX_STDERR));

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                kill_group(group);
                return Err(StepError::Failed(HookFailure::CannotStart {
                    detail: error.to_string(),
                }));
            }
        }
        if cancel.load(Ordering::SeqCst) {
            kill_group(group);
            let _ = child.wait();
            return Err(StepError::Cancelled);
        }
        if started.elapsed() >= step.timeout {
            kill_group(group);
            let _ = child.wait();
            return Err(StepError::Failed(HookFailure::Timeout {
                timeout_ms: u64::try_from(step.timeout.as_millis()).unwrap_or(u64::MAX),
            }));
        }
        thread::sleep(POLL);
    };
    // Whatever the step left behind (a background child) would keep the pipes open.
    kill_group(group);
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    if !status.success() {
        return Err(StepError::Failed(HookFailure::Exited {
            code: status.code(),
            stderr: first_lines(&stderr, STDERR_LINES),
        }));
    }
    let output: HookOutput = serde_json::from_slice(&stdout).map_err(|error| {
        StepError::Failed(HookFailure::InvalidOutput {
            detail: error.to_string(),
        })
    })?;
    if output.schema_version != CONTRACT_VERSION {
        return Err(StepError::Failed(HookFailure::InvalidOutput {
            detail: format!("unsupported schema_version {}", output.schema_version),
        }));
    }
    if output.invocation_id != invocation_id {
        return Err(StepError::Failed(HookFailure::InvocationMismatch {
            expected: invocation_id.to_owned(),
            got: output.invocation_id,
        }));
    }
    Ok(output.output.message)
}

fn read_capped(pipe: Option<impl Read>, limit: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(pipe) = pipe {
        let _ = pipe.take(limit).read_to_end(&mut bytes);
    }
    bytes
}

/// Kills the process group led by `group`. An already-gone group is not an error.
fn kill_group(group: u32) {
    let _ = Command::new("kill")
        .args(["-s", "KILL", "--", &format!("-{group}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn first_lines(bytes: &[u8], count: usize) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .take(count)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// The one line a user sees for a failed hook.
#[must_use]
pub fn failure_summary(hook: &str, failure: &HookFailure) -> String {
    match failure {
        HookFailure::Timeout { timeout_ms } => {
            let after = if timeout_ms % 1000 == 0 {
                format!("{}s", timeout_ms / 1000)
            } else {
                format!("{timeout_ms}ms")
            };
            format!("hook {hook} timed out after {after}")
        }
        HookFailure::Exited { code, stderr } => {
            let first = stderr.lines().next().unwrap_or("").trim();
            let how = code.map_or_else(
                || "was killed by a signal".to_owned(),
                |code| format!("exited with code {code}"),
            );
            if first.is_empty() {
                format!("hook {hook} {how}")
            } else {
                format!("hook {hook} {how}: {first}")
            }
        }
        HookFailure::InvalidOutput { detail } => {
            format!("hook {hook} printed invalid output: {detail}")
        }
        HookFailure::InvocationMismatch { .. } => {
            format!("hook {hook} answered a different invocation")
        }
        HookFailure::CannotStart { detail } => format!("hook {hook} cannot start: {detail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staged_files() -> usize {
        std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("gm-staged-"))
            .count()
    }

    #[test]
    fn a_panicking_run_still_ends_with_a_failed_report_and_no_temp_file() {
        let before = staged_files();
        let (tx, rx) = std::sync::mpsc::channel();
        let plan = Plan {
            run: OperationId(9),
            root: std::env::temp_dir(),
            branch: None,
            head: None,
            // Over the inline limit, so a temporary file exists while the run is in flight.
            staged_diff: "x".repeat(INLINE_DIFF_LIMIT + 1),
            message: String::new(),
            steps: vec![Step {
                name: "llm".into(),
                command: "cat >/dev/null".into(),
                timeout: Duration::from_secs(5),
            }],
            login_path: Arc::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            log: None,
        };

        // Panic on the first report, which is sent after the diff file is written.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            plan.execute(move |report| {
                assert!(!matches!(report, Report::Started { .. }), "forced panic");
                let _ = tx.send(report);
            });
        }));

        assert!(result.is_err());
        let reports: Vec<Report> = rx.try_iter().collect();
        assert_eq!(
            reports,
            [Report::Failed {
                run: OperationId(9),
                hook: "llm".into(),
                failure: HookFailure::CannotStart {
                    detail: "the hook run stopped without an answer".into(),
                },
            }]
        );
        assert_eq!(staged_files(), before, "the staged diff file is deleted");
    }

    #[test]
    fn a_step_finds_its_program_on_the_login_path() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("gm-login-only-tool");
        std::fs::write(
            &tool,
            "#!/bin/sh\nprintf '%s' '{\"invocation_id\":\"prep-11-1\",\"output\":{\"message\":\"from login path\"}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let login_path = Arc::new(OnceLock::new());
        login_path
            .set(format!("{}:/usr/bin:/bin", dir.path().display()))
            .unwrap();
        let plan = Plan {
            run: OperationId(11),
            root: std::env::temp_dir(),
            branch: None,
            head: None,
            staged_diff: String::new(),
            message: String::new(),
            steps: vec![Step {
                name: "llm".into(),
                command: "gm-login-only-tool".into(),
                timeout: Duration::from_secs(5),
            }],
            login_path,
            cancel: Arc::new(AtomicBool::new(false)),
            log: None,
        };
        let mut reports = Vec::new();
        plan.execute(|report| reports.push(report));
        assert_eq!(
            reports.last(),
            Some(&Report::Finished {
                run: OperationId(11),
                message: "from login path".into(),
            })
        );
    }

    #[test]
    fn first_lines_keeps_only_the_head_of_stderr() {
        let text = b"one\ntwo\nthree\nfour\nfive\nsix\n";
        assert_eq!(first_lines(text, 2), "one\ntwo");
        assert_eq!(first_lines(b"  \n", 2), "");
    }

    #[test]
    fn a_failure_reads_as_one_line() {
        let exited = HookFailure::Exited {
            code: Some(2),
            stderr: "boom\nmore".into(),
        };
        assert_eq!(
            failure_summary("llm", &exited),
            "hook llm exited with code 2: boom"
        );
        let timeout = HookFailure::Timeout { timeout_ms: 30_000 };
        assert_eq!(
            failure_summary("llm", &timeout),
            "hook llm timed out after 30s"
        );
        let signal = HookFailure::Exited {
            code: None,
            stderr: String::new(),
        };
        assert_eq!(
            failure_summary("llm", &signal),
            "hook llm was killed by a signal"
        );
    }
}
