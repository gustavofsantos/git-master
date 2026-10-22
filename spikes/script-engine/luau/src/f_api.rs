//! Battery F: API facts the design needs (module format, meta without ctx, syntax errors,
//! error values, listing ctx names, unknown-name messages, type stubs).

use std::sync::Arc;

use mlua::{Error, Function, Lua, Table, Value};

use crate::d_cost::MODULE;
use crate::host::{self, Limits, Opts, Shared, REMOVE_FIELDS, REMOVE_GLOBALS};
use crate::util::emit;

/// A sandboxed VM with no `ctx` and no host functions (only the budget hook).
fn bare_vm() -> (Lua, Arc<Shared>) {
    let shared = Shared::new(Limits::DEFAULT);
    let lua = Lua::new();
    let g = lua.globals();
    for name in REMOVE_GLOBALS {
        g.raw_set(*name, Value::Nil).unwrap();
    }
    for (lib, field) in REMOVE_FIELDS {
        if let Value::Table(t) = g.get::<Value>(*lib).unwrap() {
            t.raw_set(*field, Value::Nil).unwrap();
        }
    }
    lua.set_memory_limit(Limits::DEFAULT.memory).unwrap();
    let sh = shared.clone();
    lua.set_interrupt(move |lua| sh.on_interrupt(lua));
    lua.sandbox(true).unwrap();
    (lua, shared)
}

fn trunc(s: String) -> String {
    if s.len() > 400 { format!("{}...", &s[..400]) } else { s }
}

/// Recursion through an unwrapped pcall (plain Luau), to attribute the wrapper's cost.
pub fn plain_pcall_depth() {
    let lua = Lua::new();
    let src = "local d = 0\nlocal function f() d = d + 1 local ok, e = pcall(f) return ok, e end\nlocal ok, e = pcall(f)\nreturn d";
    let depth: Value = lua.load(src).eval().unwrap();
    emit(serde_json::json!({"test": "F.pcall-recursion-depth.unwrapped", "verdict": "n/a", "observed": host::display(&depth)}));
}

pub fn run() {
    // F1: the module format runs.
    let shared = Shared::new(Limits::DEFAULT);
    let out = host::run_module(MODULE, &shared, Opts::FULL);
    emit(serde_json::json!({
        "test": "F.module-format-runs",
        "verdict": if out.class == host::Class::Completed { "completed" } else { "FAILED" },
        "observed": out.returned.clone().or(out.error.clone()),
    }));

    // F2: read M.meta in a VM with no ctx and no host functions.
    {
        let (lua, shared) = bare_vm();
        let res: Result<(String, String), Error> = (|| {
            let m: Table = lua.load(MODULE).set_name("=workflow").eval()?;
            let meta: Table = m.get("meta")?;
            let _execute: Function = m.get("execute")?;
            Ok((host::display(&meta.get::<Value>("name")?), "execute present".to_string()))
        })();
        emit(serde_json::json!({
            "test": "F.meta-read-without-ctx",
            "verdict": if res.is_ok() { "completed" } else { "FAILED" },
            "observed": match &res { Ok((n, _)) => format!("meta.name={n}"), Err(e) => e.to_string() },
            "steps": shared.steps.load(std::sync::atomic::Ordering::Relaxed),
        }));
    }
    // F2b: a module whose top level touches ctx fails cleanly (ctx is an argument, not a global).
    {
        let (lua, _shared) = bare_vm();
        let src = "local M = {}\nM.meta = { name = ctx.name }\nfunction M.execute(ctx, input) return 1 end\nreturn M\n";
        let res = lua.load(src).set_name("=workflow").eval::<Table>();
        emit(serde_json::json!({
            "test": "F.meta-touches-ctx-at-top-level",
            "verdict": if res.is_err() { "blocked" } else { "FAILED" },
            "observed": res.err().map(|e| e.to_string()),
        }));
    }

    // F3: syntax errors.
    for (name, src) in [
        ("missing-operand", "local M = {}\nfunction M.execute(ctx, input)\n  local x = = 1\nend\nreturn M\n"),
        ("missing-end", "local M = {}\nfunction M.execute(ctx, input)\n  return 1\nreturn M\n"),
        ("unterminated-string", "local M = {}\nlocal s = 'abc\nreturn M\n"),
    ] {
        let lua = Lua::new();
        let res = lua.load(src).set_name("=workflow").into_function();
        let (msg, incomplete) = match &res {
            Err(Error::SyntaxError { message, incomplete_input }) => (message.clone(), *incomplete_input),
            Err(e) => (format!("OTHER: {e}"), false),
            Ok(_) => ("(compiled)".into(), false),
        };
        emit(serde_json::json!({
            "test": format!("F.syntax-error.{name}"),
            "verdict": "n/a",
            "observed": msg,
            "incomplete_input": incomplete,
        }));
    }
    // F3b: default chunk name (no set_name) for comparison.
    {
        let lua = Lua::new();
        let err = lua.load("local x = = 1").into_function().err();
        emit(serde_json::json!({
            "test": "F.syntax-error.default-chunkname",
            "verdict": "n/a",
            "observed": err.map(|e| e.to_string()),
        }));
    }

    // F4: error values as the host sees them (raw Error, then the classified message).
    for (name, body) in [
        ("table", "error({ code = 7 })"),
        ("function", "error(function() end)"),
        ("table-with-__tostring", "error(setmetatable({}, { __tostring = function() return 'custom' end }))"),
        ("nil", "error(nil)"),
        ("number", "error(42)"),
        ("string", "error('plain')"),
    ] {
        let shared = Shared::new(Limits::DEFAULT);
        let vm = host::build_vm(&shared, Opts::FULL).expect("vm");
        let src = format!("local M = {{}}\nfunction M.execute(ctx, input)\n{body}\nend\nreturn M\n");
        let module: Table = vm.lua.load(src).set_name("=workflow").eval().expect("module");
        let execute: Function = module.get("execute").unwrap();
        let res = execute.call::<Value>((vm.ctx.clone(), vm.lua.create_table().unwrap()));
        let raw = match &res {
            Err(e) => trunc(format!("{e:?}")),
            Ok(v) => format!("ok {}", host::display(v)),
        };
        let as_string = match &res {
            Err(e) => host::error_message(e),
            Ok(_) => String::new(),
        };
        emit(serde_json::json!({
            "test": format!("F.error-value.{name}"),
            "verdict": "n/a",
            "raw": raw,
            "message_via_error_message": as_string,
        }));
    }

    // F5: the host can list ctx function names (for "valid names are: ...").
    {
        let shared = Shared::new(Limits::DEFAULT);
        let vm = host::build_vm(&shared, Opts::FULL).expect("vm");
        emit(serde_json::json!({
            "test": "F.ctx-list-names",
            "verdict": "n/a",
            "observed": host::ctx_names(&vm).join(", "),
        }));
    }

    // F5b: unknown ctx name with a __index on ctx (a metatable set before ctx is made read-only).
    {
        let lua = Lua::new();
        let names = ["notify", "now", "count", "block_recv", "try"];
        let ctx = lua.create_table().unwrap();
        for n in names {
            ctx.set(n, lua.create_function(|_, ()| Ok(())).unwrap()).unwrap();
        }
        let meta = lua.create_table().unwrap();
        let list = names.join(", ");
        meta.set(
            "__index",
            lua.create_function(move |_, (_t, key): (Value, String)| -> mlua::Result<Value> {
                Err(Error::RuntimeError(format!("unknown ctx function '{key}'; valid names are: {list}")))
            })
            .unwrap(),
        )
        .unwrap();
        ctx.set_metatable(Some(meta)).unwrap();
        ctx.set_readonly(true);
        lua.globals().raw_set("ctx", ctx).unwrap();
        lua.sandbox(true).unwrap();
        let res = lua.load("return ctx.notfy").eval::<Value>();
        let res2 = lua.load("ctx.notify = nil return 'changed'").eval::<Value>();
        emit(serde_json::json!({
            "test": "F.unknown-ctx-name-message",
            "verdict": "n/a",
            "observed_read": res.err().map(|e| host::error_message(&e)),
            "observed_write_to_readonly": res2.err().map(|e| host::error_message(&e)),
        }));
    }

    // F6: type stubs. No type checker is part of this build.
    emit(serde_json::json!({
        "test": "F.type-stubs",
        "verdict": "n/a",
        "observed": "UNVERIFIED. luau0-src 0.18.3 ships Ast, CodeGen, Common, Compiler, Config, Custom, Require, VM; no Analysis/type checker, so mlua cannot type-check a script or a stub file. Luau's own type-definition syntax (declare ...) was not validated.",
    }));

    // F7: Luau version string and the timeout helper used in tests.
    let lua = Lua::new();
    let v = lua.globals().get::<Value>("_VERSION").map(|v| host::display(&v)).unwrap_or_default();
    emit(serde_json::json!({ "test": "F.luau-version", "verdict": "n/a", "observed": v }));

    // F8: does sandbox(true) itself remove anything? Compare the globals before and after it.
    {
        let lua = Lua::new();
        let names = |lua: &Lua| -> Vec<String> {
            let mut v: Vec<String> = lua.globals().pairs::<String, Value>().filter_map(|p| p.ok().map(|(k, _)| k)).collect();
            v.sort();
            v
        };
        for name in REMOVE_GLOBALS {
            lua.globals().raw_set(*name, Value::Nil).unwrap();
        }
        let before = names(&lua);
        lua.sandbox(true).unwrap();
        let after: Vec<String> = before.iter().filter(|n| matches!(lua.globals().get::<Value>(n.as_str()), Ok(v) if !matches!(v, Value::Nil))).cloned().collect();
        emit(serde_json::json!({
            "test": "F.sandbox-true-removes-nothing",
            "verdict": "n/a",
            "observed": format!("globals before sandbox(true): {} | still reachable after: {} | identical: {}", before.len(), after.len(), before.len() == after.len()),
            "note": "sandbox(true) makes library tables and globals read-only and gives the thread its own globals table; it removes no names",
        }));
    }
}
