//! The dry run of a script (design/workflow-scripts.md, D85, D107): the script runs in the sandbox
//! with every `ctx` call answered by the dry host, so the calls it would make are on record and
//! nothing is performed. The CLI (`gitmaster workflow dry-run`) and the Workflows page both use it.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::args;
use super::port::{Budgets, CtxHost, HostEnd, HostReply, RunEnd, RunRequest, ScriptEngine};
use super::surface::CtxSurface;
use crate::script::catalog::Level;

/// One `ctx` call of a dry run.
#[derive(Clone, Debug, PartialEq)]
pub struct DryCall {
    pub func: String,
    pub args: Value,
    /// `read`, `propose`, `apply`, `pause` or `helper`; empty for a call the catalog does not know.
    pub level: String,
    /// What the dry run says about the call, in words.
    pub note: String,
    /// The answer was computed as the host computes it; nothing ran.
    pub executed: bool,
    /// The answer was simulated, because the call would reach the engine; nothing ran.
    pub assumed: bool,
    /// `done`, `rejected`, `blocked` or `failed`.
    pub outcome: String,
}

/// What a dry run made: every call in order, and how the run ended.
pub struct DryRun {
    pub calls: Vec<DryCall>,
    pub end: RunEnd,
}

/// One numbered line per call, as the CLI prints them.
#[must_use]
pub fn lines(calls: &[DryCall]) -> Vec<String> {
    calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            format!(
                "{}. {}  {}",
                index + 1,
                call_text(&call.func, &call.args),
                call.note
            )
        })
        .collect()
}

/// Runs `source` with `input` under the default budgets, answering every call with the dry host.
/// Nothing is performed and nothing is recorded. The caller checks the text first.
pub fn walk(engine: &dyn ScriptEngine, chunk: &str, source: &str, input: &Value) -> DryRun {
    let surface = CtxSurface::from_catalog();
    let request = RunRequest {
        chunk,
        source,
        input,
        surface: &surface,
        budgets: Budgets::default(),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let mut host = DryHost::new();
    let end = engine.run(&request, &mut host);
    DryRun {
        calls: host.calls,
        end,
    }
}

/// The words a run's end is shown with.
#[must_use]
pub fn end_line(end: &RunEnd) -> String {
    match end {
        RunEnd::Returned(value) => format!("returned: {value}"),
        RunEnd::Blocked(reason) => format!("ended: blocked: {reason}"),
        RunEnd::Failed(failure) => format!("ended: failed: {failure}"),
        RunEnd::Cancelled => "ended: cancelled".to_owned(),
    }
}

/// The host of a dry run. It records every call, and answers the ones that would reach the
/// engine with a simulated result of the same shape (D85).
pub struct DryHost {
    surface: CtxSurface,
    calls: Vec<DryCall>,
    /// A simulated result was already given, so the calls after it are estimates.
    simulated: bool,
}

impl DryHost {
    #[must_use]
    pub fn new() -> Self {
        Self {
            surface: CtxSurface::from_catalog(),
            calls: Vec::new(),
            simulated: false,
        }
    }

    fn record(func: &str, args: &Value, level: &str, note: &str) -> DryCall {
        DryCall {
            func: func.to_owned(),
            args: args.clone(),
            level: level.to_owned(),
            note: note.to_owned(),
            executed: false,
            assumed: false,
            outcome: "done".to_owned(),
        }
    }

    fn simulate(&mut self, func: &str, args: &Value) -> HostReply {
        let Some(spec) = self.surface.get(func) else {
            let message = format!("ctx.{func} is not a function of ctx");
            let mut call = Self::record(func, args, "", &format!("rejected: {message}"));
            "rejected".clone_into(&mut call.outcome);
            self.calls.push(call);
            return HostReply::Error(message);
        };
        let (level, fields) = (spec.level, spec.fields);
        if let Err(message) = args::validate(func, fields, args) {
            let mut call = Self::record(
                func,
                args,
                level_name(level),
                &format!("rejected: {message}"),
            );
            "rejected".clone_into(&mut call.outcome);
            self.calls.push(call);
            return HostReply::Error(message);
        }
        let (what, result) = simulated_result(func, args);
        let estimate = self.simulated;
        self.simulated = true;
        let mark = if estimate { "  (estimate)" } else { "" };
        let note = format!("{}, simulated: {what}{mark}", level_word(level, func));
        let mut call = Self::record(func, args, level_name(level), &note);
        call.assumed = true;
        self.calls.push(call);
        HostReply::Value(result)
    }
}

impl Default for DryHost {
    fn default() -> Self {
        Self::new()
    }
}

impl CtxHost for DryHost {
    fn call(&mut self, func: &str, args: Value) -> HostReply {
        match func {
            "now" => {
                let mut call = Self::record(func, &args, "helper", "helper, real clock");
                call.executed = true;
                self.calls.push(call);
                HostReply::Value(json!(now_millis()))
            }
            "random" => {
                let mut call = Self::record(func, &args, "helper", "helper, fixed at 0.5");
                call.executed = true;
                self.calls.push(call);
                HostReply::Value(json!(0.5))
            }
            "notify" => {
                let mut call = Self::record(func, &args, "read", "read, recorded only");
                call.executed = true;
                self.calls.push(call);
                HostReply::Value(Value::Null)
            }
            "block" | "fail" => {
                let reason = reason_of(&args);
                let mut call = Self::record(func, &args, "helper", "ends the run");
                call.executed = true;
                let outcome = if func == "block" { "blocked" } else { "failed" };
                outcome.clone_into(&mut call.outcome);
                self.calls.push(call);
                HostReply::End(if func == "block" {
                    HostEnd::Blocked(reason)
                } else {
                    HostEnd::Failed(reason)
                })
            }
            _ => self.simulate(func, &args),
        }
    }
}

/// The catalog's level, as the journal writes it.
fn level_name(level: Level) -> &'static str {
    match level {
        Level::Read => "read",
        Level::Propose => "propose",
        Level::Apply => "apply",
        Level::Pause => "pause",
    }
}

fn level_word(level: Level, func: &str) -> &'static str {
    match level {
        Level::Read => "read",
        Level::Propose => "propose",
        Level::Apply => "applies",
        Level::Pause if func == "wait" => "waits",
        Level::Pause => "asks",
    }
}

/// What a call answers in a dry run, and the words that say what it is.
fn simulated_result(func: &str, args: &Value) -> (&'static str, Value) {
    match func {
        "exec" => ("a program never runs", exec_result(args)),
        "approve" => ("assumed approved", json!(true)),
        "wait" => ("assumed resumed", json!({"outcome": "resumed"})),
        "commit.create" => ("nothing is applied", json!({"commit": "0000000"})),
        "restack.plan" | "reparent.plan" => (
            "nothing is applied",
            json!({"plan_id": "dry-run", "branch_count": 1}),
        ),
        "stack.tree" | "stack.stale" => ("nothing is read", json!({"branches": []})),
        "verdicts" => ("nothing is read", json!([])),
        _ => ("nothing is applied", Value::Null),
    }
}

fn exec_result(args: &Value) -> Value {
    let mut result = json!({"text": "", "exit_code": 0, "stderr": "", "truncated": false});
    if args.get("format").and_then(Value::as_str) == Some("json") {
        result["json"] = json!({});
    }
    result
}

/// `ctx.name(args)`, with the arguments as JSON; `()` when there are none.
pub(crate) fn call_text(func: &str, args: &Value) -> String {
    match args.as_object() {
        Some(map) if map.is_empty() => format!("ctx.{func}()"),
        _ => format!("ctx.{func}({args})"),
    }
}

pub(crate) fn reason_of(args: &Value) -> String {
    args.get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dry_host_records_and_marks_estimates() {
        let mut host = DryHost::new();
        host.call(
            "exec",
            json!({"argv": ["ticket-branch-name", "ABC-1"], "timeout": "30s"}),
        );
        host.call("approve", json!({"summary": "Create branch ?"}));
        assert_eq!(
            lines(&host.calls)[0],
            "1. ctx.exec({\"argv\":[\"ticket-branch-name\",\"ABC-1\"],\"timeout\":\"30s\"})  read, simulated: a program never runs"
        );
        assert_eq!(
            lines(&host.calls)[1],
            "2. ctx.approve({\"summary\":\"Create branch ?\"})  asks, simulated: assumed approved  (estimate)"
        );
    }

    #[test]
    fn a_dry_host_answers_with_the_shape_of_each_result() {
        let mut host = DryHost::new();
        let exec = host.call("exec", json!({"argv": ["x"], "format": "json"}));
        assert_eq!(
            exec,
            HostReply::Value(
                json!({"text": "", "exit_code": 0, "stderr": "", "truncated": false, "json": {}})
            )
        );
        assert_eq!(
            host.call("wait", json!({"event": "restack.resumed"})),
            HostReply::Value(json!({"outcome": "resumed"}))
        );
        assert_eq!(
            host.call("commit.create", json!({"message": "m"})),
            HostReply::Value(json!({"commit": "0000000"}))
        );
        assert_eq!(
            host.call("branch.create", json!({"name": "b"})),
            HostReply::Value(Value::Null)
        );
        assert_eq!(
            host.call("stack.tree", json!({})),
            HostReply::Value(json!({"branches": []}))
        );
        assert_eq!(
            host.call("verdicts", json!({"reviewer": "security"})),
            HostReply::Value(json!([]))
        );
        assert!(
            lines(&host.calls)
                .last()
                .unwrap()
                .contains("simulated: nothing is read")
        );
    }

    #[test]
    fn a_rejected_call_is_recorded_and_is_not_a_simulated_result() {
        let mut host = DryHost::new();
        let reply = host.call("exec", json!({"bogus": 1}));
        assert!(matches!(reply, HostReply::Error(_)), "{reply:?}");
        assert!(lines(&host.calls)[0].ends_with("rejected: ctx.exec: unknown field `bogus`. Valid fields: argv, stdin, timeout, format"), "{:?}", lines(&host.calls));
        host.call("approve", json!({"summary": "x"}));
        assert!(
            !lines(&host.calls)[1].contains("(estimate)"),
            "{:?}",
            lines(&host.calls)
        );
    }

    #[test]
    fn helpers_are_answered_without_a_simulation() {
        let mut host = DryHost::new();
        assert!(
            matches!(host.call("now", json!({})), HostReply::Value(v) if v.as_u64().is_some_and(|ms| ms > 0))
        );
        assert_eq!(host.call("random", json!({})), HostReply::Value(json!(0.5)));
        assert_eq!(
            host.call("block", json!({"reason": "no"})),
            HostReply::End(HostEnd::Blocked("no".into()))
        );
        assert_eq!(
            host.call("fail", json!({"reason": "bad"})),
            HostReply::End(HostEnd::Failed("bad".into()))
        );
        assert!(!host.simulated);
    }
}
