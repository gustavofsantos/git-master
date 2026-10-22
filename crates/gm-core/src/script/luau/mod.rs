//! The Luau adapter of the `ScriptEngine` port (D104). Every call builds a new VM and drops it at
//! the end. `sandbox` builds the VM, with the budgets and the latch; `ctx` builds `ctx` as Lua
//! functions over the host; `convert` moves plain data across.

mod convert;
mod ctx;
mod sandbox;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mlua::{Error as LuaError, Function, Lua, Table, Value};
use serde_json::Value as Json;

use super::port::{
    BudgetKind, Budgets, CtxHost, Failure, RunEnd, RunRequest, ScriptDiagnostic, ScriptEngine,
};
use sandbox::Run;

/// Luau through `mlua`. Stateless: every call builds a new VM.
#[derive(Clone, Copy, Debug, Default)]
pub struct LuauEngine;

impl LuauEngine {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ScriptEngine for LuauEngine {
    fn check(&self, chunk: &str, source: &str) -> Result<(), Vec<ScriptDiagnostic>> {
        let run = fresh_run();
        let (_lua, module) =
            load(&run, chunk, source).map_err(|problem| vec![problem.diagnostic(chunk, &run)])?;
        match module.get::<Value>("execute") {
            Ok(Value::Function(_)) => Ok(()),
            _ => Err(vec![ScriptDiagnostic {
                line: 0,
                message: "M.execute must be a function".to_owned(),
            }]),
        }
    }

    fn read_meta(&self, chunk: &str, source: &str) -> Result<Json, Vec<ScriptDiagnostic>> {
        let run = fresh_run();
        let (_lua, module) =
            load(&run, chunk, source).map_err(|problem| vec![problem.diagnostic(chunk, &run)])?;
        let meta = module
            .get::<Value>("meta")
            .map_err(|error| vec![Problem::Lua(error).diagnostic(chunk, &run)])?;
        let line_zero = |message: String| vec![ScriptDiagnostic { line: 0, message }];
        match meta {
            Value::Nil => Err(line_zero("M.meta is missing".to_owned())),
            Value::Table(_) => {
                convert::to_json(&meta).map_err(|e| line_zero(format!("M.meta cannot hold {e}")))
            }
            _ => Err(line_zero("M.meta must be a table".to_owned())),
        }
    }

    fn run(&self, request: &RunRequest<'_>, host: &mut dyn CtxHost) -> RunEnd {
        let run = Arc::new(Run::new(request.budgets, Arc::clone(&request.cancel)));
        execute(request, host, &run)
    }
}

/// A run that nobody can cancel: `check` and `read_meta`.
fn fresh_run() -> Arc<Run> {
    Arc::new(Run::new(
        Budgets::default(),
        Arc::new(AtomicBool::new(false)),
    ))
}

/// Why a module did not load.
enum Problem {
    /// A syntax or runtime error, or a budget end.
    Lua(LuaError),
    /// The chunk ran, but did not return a table.
    Shape(String),
}

impl Problem {
    /// The diagnostic for `check` and `read_meta`. The line is the one the VM names.
    fn diagnostic(self, chunk: &str, run: &Run) -> ScriptDiagnostic {
        match self {
            Self::Shape(message) => ScriptDiagnostic { line: 0, message },
            Self::Lua(error) => {
                if let Some(latch) = run.latched() {
                    return ScriptDiagnostic {
                        line: 0,
                        message: latch.message(),
                    };
                }
                let (line, message) = split_location(chunk, &error_text(&error));
                ScriptDiagnostic { line, message }
            }
        }
    }
}

/// Compiles the chunk and runs its top level in a fresh VM with no `ctx`. Returns the VM and the
/// table the chunk returned.
fn load(run: &Arc<Run>, chunk: &str, source: &str) -> Result<(Lua, Table), Problem> {
    let lua = sandbox::new_vm(run).map_err(Problem::Lua)?;
    // The `=` makes the name literal: errors read `chunk:LINE:`, not `[string "chunk"]:LINE:`.
    let function = lua
        .load(source)
        .set_name(format!("={chunk}"))
        .into_function()
        .map_err(Problem::Lua)?;
    let value: Value = function.call(()).map_err(Problem::Lua)?;
    match value {
        Value::Table(module) => Ok((lua, module)),
        _ => Err(Problem::Shape("the chunk must return a table M".to_owned())),
    }
}

/// Runs `M.execute(ctx, input)` in a fresh VM and says how it ended.
fn execute(request: &RunRequest<'_>, host: &mut dyn CtxHost, run: &Arc<Run>) -> RunEnd {
    let (lua, module) = match load(run, request.chunk, request.source) {
        Ok(loaded) => loaded,
        Err(Problem::Lua(error)) => return failed(run, &error),
        Err(Problem::Shape(message)) => return RunEnd::Failed(Failure::Script(message)),
    };
    let execute: Function = match module.get::<Value>("execute") {
        Ok(Value::Function(function)) => function,
        _ => {
            return RunEnd::Failed(Failure::Script("M.execute must be a function".to_owned()));
        }
    };
    let bridge = ctx::Bridge::new(run, host, request.surface);
    let end = lua.scope(|scope| {
        let ctx = ctx::install(&lua, scope, &bridge)?;
        let input = convert::to_lua(&lua, request.input, true)?;
        Ok(match execute.call::<Value>((ctx, input)) {
            Ok(value) => returned(&value),
            Err(error) => failed(run, &error),
        })
    });
    let end = end.unwrap_or_else(|error| failed(run, &error));
    // A latched end wins over whatever the script returned or raised.
    run.latched().map_or(end, |latch| latch.end())
}

fn returned(value: &Value) -> RunEnd {
    match convert::to_json(value) {
        Ok(json) => RunEnd::Returned(json),
        Err(error) => RunEnd::Failed(Failure::Script(format!("execute returned {error}"))),
    }
}

/// The end of a run that raised `error`, when no end is latched.
fn failed(run: &Run, error: &LuaError) -> RunEnd {
    if let Some(latch) = run.latched() {
        return latch.end();
    }
    let text = error_text(error);
    if matches!(error, LuaError::MemoryError(_))
        || text.contains("not enough memory")
        || text.contains("memory allocation error")
        || text.contains("table overflow")
    {
        return RunEnd::Failed(Failure::Budget(BudgetKind::Memory));
    }
    let message = text;
    if message.contains("stack overflow") {
        return RunEnd::Failed(Failure::Budget(BudgetKind::StackDepth));
    }
    RunEnd::Failed(Failure::Script(message))
}

/// The message of a Lua error, without the traceback mlua attaches.
fn error_text(error: &LuaError) -> String {
    let text = match error {
        LuaError::RuntimeError(message)
        | LuaError::MemoryError(message)
        | LuaError::SyntaxError { message, .. } => message.clone(),
        LuaError::CallbackError { cause, .. } => return error_text(cause),
        other => other.to_string(),
    };
    match text.find("\nstack traceback:") {
        Some(index) => text[..index].to_owned(),
        None => text,
    }
}

/// Splits a `chunk:LINE: ` prefix off a message. A message without the prefix has line 0.
fn split_location(chunk: &str, message: &str) -> (usize, String) {
    let fallback = || (0, message.to_owned());
    let Some(rest) = message
        .strip_prefix(chunk)
        .and_then(|rest| rest.strip_prefix(':'))
    else {
        return fallback();
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return fallback();
    }
    let Some(text) = rest[digits..].strip_prefix(':') else {
        return fallback();
    };
    match rest[..digits].parse::<usize>() {
        Ok(line) => (line, text.trim_start().to_owned()),
        Err(_) => fallback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_comes_from_the_chunk_prefix() {
        assert_eq!(
            split_location("flow.lua", "flow.lua:3: Expected 'end'"),
            (3, "Expected 'end'".to_owned())
        );
        assert_eq!(
            split_location("flow.lua", "flow.lua:120: boom"),
            (120, "boom".to_owned())
        );
    }

    #[test]
    fn a_message_without_the_prefix_has_line_zero() {
        assert_eq!(
            split_location("flow.lua", "budget exceeded"),
            (0, "budget exceeded".to_owned())
        );
        assert_eq!(
            split_location("flow.lua", "other.lua:3: x"),
            (0, "other.lua:3: x".to_owned())
        );
        assert_eq!(
            split_location("flow.lua", "flow.lua::x"),
            (0, "flow.lua::x".to_owned())
        );
        assert_eq!(
            split_location("flow.lua", "flow.lua:x: y"),
            (0, "flow.lua:x: y".to_owned())
        );
    }

    #[test]
    fn the_traceback_is_not_part_of_the_message() {
        let error =
            LuaError::RuntimeError("flow:2: boom\nstack traceback:\n  [C]: in ?".to_owned());
        assert_eq!(error_text(&error), "flow:2: boom");
    }
}
