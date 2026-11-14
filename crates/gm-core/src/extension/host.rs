//! The host side of an extension's `ctx` (design/extensions.md): read-only calls under the
//! extension's lists, a wall clock for the whole run, and a record of every call for the
//! Extensions page. `run_slot` runs one slot function and reads its document.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gm_schema::ext::Document;
use serde_json::{Value, json};

use super::http::{self, Credentials, Network};
use super::{Extension, PRELUDE, surface};
use crate::script::exec;
use crate::script::port::{Budgets, CtxHost, HostEnd, HostReply, RunEnd, RunRequest, ScriptEngine};

/// The wall clock of one run, host calls included.
pub const RUN_TIME: Duration = Duration::from_secs(15);
/// The default timeout of one `exec` or `http.get`.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Calls on `ctx` in one run.
pub const CTX_CALLS: u32 = 50;
/// Lines of `ctx.log` kept per run, and their length.
const LOG_LINES: usize = 100;
const LOG_CHARS: usize = 500;

/// What a run may reach besides its own logic.
pub struct Reach<'a> {
    /// Where `exec` runs: the repository's root.
    pub root: PathBuf,
    /// `exec` and `http.get` run only for a reviewed version (D101), or when the person allows
    /// it on the command line.
    pub effects: bool,
    /// The PATH programs are looked up in, as the workflows' `exec` gets it.
    pub path: Option<String>,
    pub network: &'a dyn Network,
    pub credentials: &'a dyn Credentials,
    pub cancel: Arc<AtomicBool>,
}

/// One call, as the Extensions page lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallRecord {
    pub func: String,
    /// The program and its arguments, or the address. Never a credential.
    pub summary: String,
    pub outcome: String,
    pub duration: Duration,
}

/// How a slot run ended.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotRun {
    /// The document, empty for `nil`, or why there is none.
    pub result: Result<Document, String>,
    pub calls: Vec<CallRecord>,
    pub logs: Vec<String>,
    pub duration: Duration,
    pub cancelled: bool,
}

struct Host<'a> {
    meta: &'a super::Meta,
    reach: &'a Reach<'a>,
    deadline: Instant,
    calls: Vec<CallRecord>,
    logs: Vec<String>,
}

const NOT_REVIEWED: &str =
    "runs only when this version of the extension is reviewed in the app, or with --allow-exec";

impl Host<'_> {
    fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    fn record(&mut self, func: &str, summary: String, outcome: &str, started: Instant) {
        self.calls.push(CallRecord {
            func: func.to_owned(),
            summary,
            outcome: outcome.to_owned(),
            duration: started.elapsed(),
        });
    }

    fn exec(&mut self, mut args: Value) -> HostReply {
        if !self.reach.effects {
            return HostReply::Error(format!("ctx.exec {NOT_REVIEWED}"));
        }
        let program = args
            .get("argv")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if !self.meta.exec.contains(&program) {
            return HostReply::Error(format!(
                "ctx.exec: {program:?} is not in meta.exec; add it there to run it"
            ));
        }
        if let Some(map) = args.as_object_mut() {
            map.entry("timeout")
                .or_insert_with(|| json!(format!("{}s", CALL_TIMEOUT.as_secs())));
        }
        let mut spec = match exec::spec(&args, &self.reach.root) {
            Ok(spec) => spec,
            Err(message) => return HostReply::Error(format!("ctx.exec: {message}")),
        };
        spec.timeout = spec.timeout.min(self.remaining());
        spec.path.clone_from(&self.reach.path);
        let summary = spec.argv.join(" ");
        let started = Instant::now();
        let result = exec::run(&spec, &self.reach.cancel);
        self.record("exec", summary, result.outcome, started);
        if self.reach.cancel.load(Ordering::SeqCst) {
            return HostReply::End(HostEnd::Cancelled);
        }
        let output = result.output;
        // A program that ran and exited non-zero is an answer the script reads (`exit_code`);
        // only a program that could not run, timed out or printed no JSON when asked is a failure.
        let ran = output.get("error").is_none() && !output["exit_code"].is_null();
        match result.outcome {
            "done" => HostReply::Value(output),
            "failed" if ran => HostReply::Value(output),
            outcome => {
                let message = output
                    .get("error")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("{program} {outcome}"), str::to_owned);
                HostReply::Outcome {
                    outcome: outcome.to_owned(),
                    value: output,
                    message,
                }
            }
        }
    }

    fn http_get(&mut self, args: &Value) -> HostReply {
        if !self.reach.effects {
            return HostReply::Error(format!("ctx.http.get {NOT_REVIEWED}"));
        }
        let mut get = match http::read_args(args, CALL_TIMEOUT) {
            Ok(get) => get,
            Err(message) => return HostReply::Error(format!("ctx.http.get: {message}")),
        };
        get.timeout = get.timeout.min(self.remaining());
        let started = Instant::now();
        let outcome = http::get(
            &get,
            &self.meta.http,
            &self.meta.credentials,
            self.reach.network,
            self.reach.credentials,
        );
        let summary = format!("GET {}", get.url);
        match outcome {
            http::Outcome::Done(value) => {
                let status = value["status"].as_u64().unwrap_or_default();
                self.record("http.get", summary, &status.to_string(), started);
                HostReply::Value(value)
            }
            http::Outcome::Timeout => {
                self.record("http.get", summary, "timeout", started);
                HostReply::Outcome {
                    outcome: "timeout".to_owned(),
                    value: Value::Null,
                    message: format!("GET {} timed out", get.url),
                }
            }
            http::Outcome::Failed(message) => {
                self.record("http.get", summary, "failed", started);
                HostReply::Outcome {
                    outcome: "failed".to_owned(),
                    value: Value::Null,
                    message,
                }
            }
        }
    }
}

impl CtxHost for Host<'_> {
    fn call(&mut self, func: &str, args: Value) -> HostReply {
        if self.reach.cancel.load(Ordering::SeqCst) {
            return HostReply::End(HostEnd::Cancelled);
        }
        if self.remaining().is_zero() {
            return HostReply::End(HostEnd::Failed(format!(
                "the run took longer than {} s",
                RUN_TIME.as_secs()
            )));
        }
        match func {
            "exec" => self.exec(args),
            "http.get" => self.http_get(&args),
            "log" => {
                let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
                if self.logs.len() < LOG_LINES {
                    self.logs.push(text.chars().take(LOG_CHARS).collect());
                }
                HostReply::Value(Value::Null)
            }
            "now" => HostReply::Value(json!(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs())
            )),
            "fail" => HostReply::End(HostEnd::Failed(
                args.get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("failed")
                    .to_owned(),
            )),
            other => HostReply::Error(format!(
                "ctx.{other} is not available to extensions; extensions have ctx.exec, ctx.http.get, ctx.log, ctx.now and ctx.fail"
            )),
        }
    }
}

/// Runs `M.slots[slot](ctx, input)` of `extension` and reads what it returned.
pub fn run_slot(
    engine: &dyn ScriptEngine,
    extension: &Extension,
    slot: &str,
    input: &Value,
    reach: &Reach<'_>,
) -> SlotRun {
    let started = Instant::now();
    let surface = surface();
    let budgets = Budgets {
        ctx_calls: CTX_CALLS,
        ..Budgets::default()
    };
    let request = RunRequest {
        chunk: &extension.file_name,
        source: &extension.source,
        input,
        surface: &surface,
        budgets,
        cancel: Arc::clone(&reach.cancel),
    };
    let mut host = Host {
        meta: &extension.meta,
        reach,
        deadline: started + RUN_TIME,
        calls: Vec::new(),
        logs: Vec::new(),
    };
    let end = engine.run_slot(&request, slot, PRELUDE, &mut host);
    let mut cancelled = false;
    let result = match end {
        RunEnd::Returned(value) => Document::parse(&value).map_err(|e| e.to_string()),
        RunEnd::Failed(failure) => Err(failure.to_string()),
        RunEnd::Blocked(reason) => Err(reason),
        RunEnd::Cancelled => {
            cancelled = true;
            Err("cancelled".to_owned())
        }
    };
    SlotRun {
        result,
        calls: host.calls,
        logs: host.logs,
        duration: started.elapsed(),
        cancelled,
    }
}
