//! The `ctx` of one run: the surface's functions as Lua functions that call the host, the helpers,
//! and `ctx.try`. Every function is a scope callback, valid only while the run lasts, and holds
//! the host through the `Bridge`. `ctx.try` runs in Lua, so a table error value passes through
//! untouched (spike S-D).

use std::cell::RefCell;
use std::time::Instant;

use mlua::{
    Error as LuaError, Function, Lua, MultiValue, Result as LuaResult, Scope, Table, Value,
};
use serde_json::{Map, Value as Json, json};

use super::convert::{self, ConvError};
use super::sandbox::{Latch, Record, Run};
use crate::script::port::{CtxHost, HostEnd, HostReply};
use crate::script::surface::{ArgShape, CtxFn, CtxSurface};

/// `ctx.try`. The VM's frame is entered before the call and left after it, and the error of the
/// call is raised again outside it, so an ordinary error keeps its value.
const TRY: &str = r#"
local enter, leave, pcall_, error_ = ...
return function(fn)
  enter()
  local ok, ret = pcall_(fn)
  local last = leave()
  if not ok then error_(ret, 0) end
  if last ~= nil then return last end
  return { outcome = "done", value = ret }
end
"#;

/// The run's state and the host, as the `ctx` functions use them.
pub(super) struct Bridge<'a> {
    run: &'a Run,
    host: RefCell<&'a mut dyn CtxHost>,
    surface: &'a CtxSurface,
}

impl<'a> Bridge<'a> {
    pub(super) fn new(run: &'a Run, host: &'a mut dyn CtxHost, surface: &'a CtxSurface) -> Self {
        Self {
            run,
            host: RefCell::new(host),
            surface,
        }
    }

    /// Asks the host and times the call, so the time does not count against the script.
    fn call_host(&self, func: &str, args: Json) -> HostReply {
        let Ok(mut host) = self.host.try_borrow_mut() else {
            return HostReply::Error("the host is already answering a call".to_owned());
        };
        let started = Instant::now();
        let reply = host.call(func, args);
        self.run.add_host_time(started.elapsed());
        reply
    }

    /// One call on `ctx`, a helper included: charges the budget, asks the host and turns the reply
    /// into the Lua result.
    pub(super) fn invoke(&self, lua: &Lua, func: &str, args: Json) -> LuaResult<Value> {
        self.run.charge_ctx()?;
        let in_try = self.run.in_try();
        match self.call_host(func, args) {
            HostReply::Value(value) => {
                let result = convert::to_lua(lua, &value, false)?;
                if in_try {
                    self.run.set_last(Record::done(value));
                }
                Ok(result)
            }
            HostReply::Outcome {
                outcome,
                value,
                message,
            } => {
                if !in_try {
                    return Err(self.run.raise(Latch::Outcome {
                        func: func.to_owned(),
                        outcome,
                        message,
                    }));
                }
                let record = Record {
                    outcome,
                    value,
                    message,
                };
                let table = record_table(lua, &record)?;
                self.run.set_last(record);
                Ok(Value::Table(table))
            }
            // A plain error: `pcall` can catch it.
            HostReply::Error(message) => Err(LuaError::RuntimeError(message)),
            HostReply::End(end) => Err(self.run.raise(match end {
                HostEnd::Blocked(reason) => Latch::Blocked(reason),
                HostEnd::Failed(reason) => Latch::Reason(reason),
                HostEnd::Cancelled => Latch::Cancelled,
            })),
        }
    }

    /// A catalog function, called from Lua with its arguments.
    fn call_fn(&self, lua: &Lua, spec: &CtxFn, args: MultiValue) -> LuaResult<Value> {
        let json = encode_args(spec, &args.into_vec())?;
        self.invoke(lua, spec.path, json)
    }

    fn enter(&self) -> LuaResult<()> {
        self.run.charge_ctx()?;
        self.run.enter_try();
        Ok(())
    }

    fn leave(&self, lua: &Lua) -> LuaResult<Value> {
        match self.run.leave_try() {
            Some(record) => Ok(Value::Table(record_table(lua, &record)?)),
            None => Ok(Value::Nil),
        }
    }
}

fn fail(message: String) -> LuaError {
    LuaError::RuntimeError(message)
}

/// The JSON arguments of one call: the catalog's field names as keys (D98).
fn encode_args(spec: &CtxFn, args: &[Value]) -> LuaResult<Json> {
    match &spec.shape {
        ArgShape::Table => match args.first() {
            None | Some(Value::Nil) => Ok(Json::Object(Map::new())),
            Some(Value::Table(table)) => Ok(Json::Object(named(spec, table)?)),
            Some(_) => Err(fail(format!("ctx.{} expects a table", spec.path))),
        },
        ArgShape::Positional(names) => {
            let mut map = Map::new();
            for (name, value) in names.iter().zip(args) {
                if !value.is_nil() {
                    let json = convert::to_json(value).map_err(|e| cannot_pass(spec, &e))?;
                    map.insert((*name).to_owned(), json);
                }
            }
            // The options come after the positional arguments. A positional value wins over a
            // field of the same name.
            match args.get(names.len()) {
                None | Some(Value::Nil) => {}
                Some(Value::Table(table)) => {
                    for (key, value) in named(spec, table)? {
                        map.entry(key).or_insert(value);
                    }
                }
                Some(_) => {
                    return Err(fail(format!(
                        "ctx.{} takes its options in a table",
                        spec.path
                    )));
                }
            }
            Ok(Json::Object(map))
        }
    }
}

fn named(spec: &CtxFn, table: &Table) -> LuaResult<Map<String, Json>> {
    match convert::to_json(&Value::Table(table.clone())) {
        Ok(Json::Object(map)) => Ok(map),
        Ok(_) => Err(fail(format!(
            "ctx.{} expects a table of named fields",
            spec.path
        ))),
        Err(e) => Err(cannot_pass(spec, &e)),
    }
}

fn cannot_pass(spec: &CtxFn, error: &ConvError) -> LuaError {
    fail(format!("cannot pass {error} to ctx.{}", spec.path))
}

/// The table of the last call inside a `try`: its outcome, its message, and the fields of its
/// value, or the value itself when it is not a table.
fn record_table(lua: &Lua, record: &Record) -> LuaResult<Table> {
    let table = lua.create_table()?;
    match &record.value {
        Json::Null => {}
        Json::Object(fields) => {
            for (key, value) in fields {
                table.raw_set(key.as_str(), convert::to_lua(lua, value, false)?)?;
            }
        }
        other => table.raw_set("value", convert::to_lua(lua, other, false)?)?,
    }
    if record.outcome != "done" || !record.message.is_empty() {
        table.raw_set("message", record.message.as_str())?;
    }
    table.raw_set("outcome", record.outcome.as_str())?;
    Ok(table)
}

/// Builds `ctx` for one run, and the `try` that runs in Lua.
pub(super) fn install<'s, 'b: 's>(
    lua: &Lua,
    scope: &'s Scope<'s, 'b>,
    bridge: &'s Bridge<'b>,
) -> LuaResult<Table> {
    let root = lua.create_table()?;
    let mut spaces: Vec<(Table, String)> = vec![(root.clone(), String::new())];
    for spec in bridge.surface.functions() {
        let parts: Vec<&str> = spec.path.split('.').collect();
        let Some((leaf, dirs)) = parts.split_last() else {
            continue;
        };
        let mut table = root.clone();
        let mut prefix = String::new();
        for dir in dirs {
            if !prefix.is_empty() {
                prefix.push('.');
            }
            prefix.push_str(dir);
            table = space(lua, &table, dir, &prefix, &mut spaces)?;
        }
        let spec = spec.clone();
        let func =
            scope.create_function(move |lua, args: MultiValue| bridge.call_fn(lua, &spec, args))?;
        table.raw_set(*leaf, func)?;
    }

    for name in ["now", "random"] {
        let func = scope
            .create_function(move |lua, _args: MultiValue| bridge.invoke(lua, name, json!({})))?;
        root.raw_set(name, func)?;
    }
    for name in ["block", "fail"] {
        let func = scope.create_function(move |lua, reason: mlua::String| {
            let reason = reason
                .to_str()
                .map_err(|_| fail("the reason must be text".to_owned()))?;
            bridge.invoke(lua, name, json!({ "reason": &*reason }))
        })?;
        root.raw_set(name, func)?;
    }

    let enter = scope.create_function(move |_, _args: MultiValue| bridge.enter())?;
    let leave = scope.create_function(move |lua, _args: MultiValue| bridge.leave(lua))?;
    let pcall: Function = lua.globals().get("pcall")?;
    let error: Function = lua.globals().get("error")?;
    let try_fn: Function = lua
        .load(TRY)
        .set_name("=ctx.try")
        .call((enter, leave, pcall, error))?;
    root.raw_set("try", try_fn)?;

    for (table, prefix) in &spaces {
        lock_space(lua, scope, bridge.surface, table, prefix)?;
    }
    Ok(root)
}

/// The namespace `name` under `parent`, created on first use.
fn space(
    lua: &Lua,
    parent: &Table,
    name: &str,
    prefix: &str,
    spaces: &mut Vec<(Table, String)>,
) -> LuaResult<Table> {
    match parent.raw_get::<Value>(name)? {
        Value::Nil => {
            let table = lua.create_table()?;
            parent.raw_set(name, table.clone())?;
            spaces.push((table.clone(), prefix.to_owned()));
            Ok(table)
        }
        Value::Table(table) => Ok(table),
        _ => Err(fail(format!(
            "ctx.{prefix} is both a function and a namespace"
        ))),
    }
}

/// Makes a namespace read-only, with a locked metatable. An unknown name raises with the valid
/// names at that level (D102).
fn lock_space<'s>(
    lua: &Lua,
    scope: &'s Scope<'s, '_>,
    surface: &'s CtxSurface,
    table: &Table,
    prefix: &str,
) -> LuaResult<()> {
    let prefix = prefix.to_owned();
    let index =
        scope.create_function(move |_, (_table, key): (Value, Value)| -> LuaResult<()> {
            let name = match key {
                Value::String(text) => text.to_string_lossy(),
                other => format!("[{}]", other.type_name()),
            };
            Err(fail(surface.unknown_name_message(&prefix, &name)))
        })?;
    let meta = lua.create_table()?;
    meta.raw_set("__metatable", "locked")?;
    meta.raw_set("__index", index)?;
    table.set_metatable(Some(meta))?;
    table.set_readonly(true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_done_record_with_a_table_value_has_its_fields() {
        let lua = Lua::new();
        let record = Record::done(json!({"text": "main", "exit_code": 0}));
        let table = record_table(&lua, &record).unwrap();
        assert_eq!(table.get::<String>("text").unwrap(), "main");
        assert_eq!(table.get::<String>("outcome").unwrap(), "done");
        assert!(table.get::<Value>("message").unwrap().is_nil());
    }

    #[test]
    fn an_outcome_record_carries_the_message() {
        let lua = Lua::new();
        let record = Record {
            outcome: "conflict".to_owned(),
            value: Json::Null,
            message: "branch feat/a conflicts".to_owned(),
        };
        let table = record_table(&lua, &record).unwrap();
        assert_eq!(table.get::<String>("outcome").unwrap(), "conflict");
        assert_eq!(
            table.get::<String>("message").unwrap(),
            "branch feat/a conflicts"
        );
    }
}
