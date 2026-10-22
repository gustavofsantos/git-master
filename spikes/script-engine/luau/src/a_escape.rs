//! Battery A: escape attempts, tampering between runs, and the sandbox inventory.

use crate::host::{self, Class, Limits, Opts, Outcome, Shared};
use crate::util::emit;

#[derive(Clone, Copy, Debug)]
enum Kind {
    /// Script returns "ESCAPED: ..." if it got the capability; anything else is blocked.
    Capability,
    /// Resource bomb: script returns "ESCAPED: ..." if it ran to completion unbounded.
    Bomb,
    /// Write that succeeds only inside this run is contained, not escaped.
    RunLocal,
    /// Reported, not judged.
    Info,
}

struct Case {
    name: &'static str,
    kind: Kind,
    /// Body of `M.execute(ctx, input)`.
    body: String,
}

fn module(body: &str) -> String {
    format!("local M = {{}}\nfunction M.execute(ctx, input)\n{body}\nend\nreturn M\n")
}

/// `pcall` the expression; the script reports ESCAPED if the call succeeded with a value.
fn probe(expr: &str) -> String {
    format!(
        "local ok, r = pcall(function()\n{expr}\nend)\n\
         if ok and r ~= nil then return \"ESCAPED: \" .. tostring(r) end\n\
         return \"blocked: \" .. tostring(r)"
    )
}

/// Reference-only probe: reads a global without calling it (used where calling would be unsafe).
fn presence(path: &str) -> String {
    format!("local v = {path}\nif v ~= nil then return \"ESCAPED: present \" .. type(v) end\nreturn \"blocked: absent\"")
}

fn cases() -> Vec<Case> {
    use Kind::*;
    let c = |name, kind, body: String| Case { name, kind, body: module(&body) };
    vec![
        c("os.execute", Capability, probe("return os.execute('echo PWNED')")),
        c("os.getenv", Capability, probe("return os.getenv('HOME')")),
        c("os.time", Capability, probe("return os.time()")),
        c("os.clock", Capability, probe("return os.clock()")),
        c("os.date", Capability, probe("return os.date('%Y')")),
        c("os.exit (reference only)", Capability, presence("os and os.exit")),
        c("io.open", Capability, probe("return io.open('/etc/hostname', 'r')")),
        c("io.popen", Capability, probe("return io.popen('echo PWNED')")),
        c("io.read (reference only)", Capability, presence("io and io.read")),
        c("load", Capability, probe("return load('return 1 + 1')()")),
        c("loadstring", Capability, probe("return loadstring('return 2')()")),
        c("dofile", Capability, probe("return dofile('/etc/hostname')")),
        c("loadfile", Capability, probe("return loadfile('/etc/hostname')")),
        c("require", Capability, probe("return require('os')")),
        c("package (reference only)", Capability, presence("package")),
        c("string.dump", Capability, probe("return string.dump(function() end)")),
        c("debug.getinfo", Capability, probe("return debug.getinfo(1)")),
        c("debug.sethook (reference only)", Capability, presence("debug and debug.sethook")),
        c("debug.getregistry", Capability, probe("return debug.getregistry()")),
        c("collectgarbage", Capability, probe("return collectgarbage('count')")),
        c("gcinfo", Capability, probe("return gcinfo()")),
        c("math.random", Capability, probe("return math.random(1, 100)")),
        c("math.randomseed", Capability, probe("math.randomseed(42)\nreturn 'called'")),
        c("print (reference only)", Capability, presence("print")),
        c("warn (reference only)", Capability, presence("warn")),
        c("getfenv", Capability, probe("return getfenv(1)")),
        c("setfenv (reference only)", Capability, presence("setfenv")),
        c("newproxy (reference only)", Capability, presence("newproxy")),
        c("setmetatable(_G, ...)", Capability, probe("setmetatable(_G, {})\nreturn 'changed'")),
        c("rawset(_G, ...)", Capability, probe("rawset(_G, 'leak', 1)\nreturn 'changed'")),
        c("_G.leak = 1", Capability, probe("_G.leak = 1\nreturn 'changed'")),
        c("_ENV.leak = 1", Capability, probe("_ENV.leak = 1\nreturn 'changed'")),
        c(
            "getmetatable('').__index write",
            RunLocal,
            "local ok, e = pcall(function() getmetatable('').__index = {} end)\n\
             if ok then return \"ESCAPED: string metatable writable (run-local)\" end\n\
             return \"blocked: \" .. tostring(e)"
                .to_string(),
        ),
        c("getmetatable(ctx)", Capability, probe("return getmetatable(ctx)")),
        c("ctx.__index", Capability, probe("return ctx.__index")),
        c("ctx.notify = nil", Capability, probe("ctx.notify = nil\nreturn 'changed'")),
        c("ctx.leak = 1", Capability, probe("ctx.leak = 1\nreturn 'changed'")),
        c("rawset(ctx, ...)", Capability, probe("rawset(ctx, 'x', 1)\nreturn 'changed'")),
        c("input.repo = 'x'", Capability, probe("input.repo = 'x'\nreturn 'changed'")),
        c(
            "iterate ctx (pairs)",
            Capability,
            "local known = {notify=true, now=true, count=true, block_recv=true, try=true}\n\
             local extra, names = {}, {}\n\
             for k in pairs(ctx) do names[#names+1] = k; if not known[k] then extra[#extra+1] = k end end\n\
             table.sort(names)\n\
             if #extra > 0 then return 'ESCAPED: extra keys ' .. table.concat(extra, ',') end\n\
             return 'blocked: only public names: ' .. table.concat(names, ',')"
                .to_string(),
        ),
        c(
            "coroutine yield across ctx.try",
            Capability,
            "local crossed = false\n\
             local co = coroutine.create(function()\n\
               return ctx.try(function() coroutine.yield('x'); crossed = true end)\n\
             end)\n\
             local ok, r = coroutine.resume(co)\n\
             if crossed then return 'ESCAPED: yield crossed the host call' end\n\
             local msg = type(r) == 'table' and tostring(r.message) or tostring(r)\n\
             return 'blocked: resume=' .. tostring(ok) .. ' detail=' .. msg"
                .to_string(),
        ),
        c(
            "coroutine.wrap infinite loop",
            Bomb,
            "local f = coroutine.wrap(function() while true do end end)\nf()\nreturn 'ESCAPED: finished'".into(),
        ),
        c("infinite loop", Bomb, "while true do end\nreturn 'ESCAPED: finished'".into()),
        c(
            "string.rep memory bomb (2^31)",
            Bomb,
            "local s = string.rep('x', 2147483648)\nreturn 'ESCAPED: built ' .. #s".into(),
        ),
        c(
            "table growth bomb (1e9)",
            Bomb,
            "local t = {}\nfor i = 1, 1000000000 do t[i] = i end\nreturn 'ESCAPED: finished'".into(),
        ),
        c(
            "runaway concatenation",
            Bomb,
            "local s = 'x'\nfor i = 1, 60 do s = s .. s end\nreturn 'ESCAPED: built ' .. #s".into(),
        ),
        c(
            "deep recursion (1e6, non-tail)",
            Bomb,
            "local function f(n) return 1 + f(n + 1) end\nreturn 'ESCAPED: ' .. tostring(f(1))".into(),
        ),
        c(
            "ctx.notify flood (100000 calls)",
            Bomb,
            "for i = 1, 100000 do ctx.notify('x') end\nreturn 'ESCAPED: finished'".into(),
        ),
        c(
            "pcall swallows budget (loop)",
            Bomb,
            "local n = 0\n\
             while true do\n\
               pcall(function() while true do end end)\n\
               n = n + 1\n\
               if n > 1000 then return 'ESCAPED: caught ' .. n end\n\
             end"
                .into(),
        ),
        c(
            "pcall swallows budget (then continues)",
            Bomb,
            "local ok = pcall(function() while true do end end)\n\
             return 'ESCAPED: continued after pcall, ok=' .. tostring(ok)"
                .into(),
        ),
        c(
            "ctx.try swallows budget",
            Bomb,
            "local r = ctx.try(function() while true do end end)\n\
             return 'ESCAPED: try returned ' .. r.outcome"
                .into(),
        ),
        c("error(table) uncaught", Info, "error({ code = 7 })".into()),
        c("error(function) uncaught", Info, "error(function() end)".into()),
        c(
            "error(table with __tostring) uncaught",
            Info,
            "error(setmetatable({}, { __tostring = function() return 'custom' end }))".into(),
        ),
        c("error(nil) uncaught", Info, "error(nil)".into()),
    ]
}

fn judge(kind: Kind, out: &Outcome) -> &'static str {
    match out.class {
        Class::Budget(_) | Class::StackOverflow => "contained",
        _ => match kind {
            Kind::Info => "n/a",
            Kind::RunLocal => {
                if out.returned.as_deref().is_some_and(|s| s.starts_with("ESCAPED")) {
                    "contained"
                } else {
                    "blocked"
                }
            }
            Kind::Capability | Kind::Bomb => {
                if out.returned.as_deref().is_some_and(|s| s.starts_with("ESCAPED")) {
                    "escaped"
                } else {
                    "blocked"
                }
            }
        },
    }
}

fn run_case(src: &str) -> Outcome {
    let shared = Shared::new(Limits::DEFAULT);
    host::run_module(src, &shared, Opts::FULL)
}

pub fn run() {
    // Inventory: globals of a plain Lua::new() (names listed by pairs) and which of them
    // are still reachable in the sandboxed VM with the host model installed.
    {
        let plain = mlua::Lua::new();
        let mut names: Vec<String> = plain.globals().pairs::<String, mlua::Value>().filter_map(|p| p.ok().map(|(k, _)| k)).collect();
        names.sort();
        let shared = Shared::new(Limits::DEFAULT);
        let vm = host::build_vm(&shared, Opts::FULL).expect("build vm");
        let g = vm.lua.globals();
        let present: Vec<&str> = names.iter().map(|n| n.as_str()).filter(|n| !matches!(g.get::<mlua::Value>(*n), Ok(mlua::Value::Nil) | Err(_))).collect();
        let absent: Vec<&str> = names.iter().map(|n| n.as_str()).filter(|n| !present.contains(n)).collect();
        emit(serde_json::json!({"test": "A.inventory.globals-plain-luau", "verdict": "n/a", "observed": names.join(" ")}));
        emit(serde_json::json!({"test": "A.inventory.globals-present-sandboxed", "verdict": "n/a", "observed": present.join(" ")}));
        emit(serde_json::json!({"test": "A.inventory.globals-removed", "verdict": "n/a", "observed": absent.join(" ")}));
        for lib in ["string", "table", "math", "coroutine", "utf8", "bit32", "buffer", "vector"] {
            if let Ok(mlua::Value::Table(t)) = g.get::<mlua::Value>(lib) {
                let mut f: Vec<String> = t.pairs::<String, mlua::Value>().filter_map(|p| p.ok().map(|(k, _)| k)).collect();
                f.sort();
                emit(serde_json::json!({"test": format!("A.inventory.{lib}"), "verdict": "n/a", "observed": f.join(" ")}));
            }
        }
        emit(serde_json::json!({"test": "A.inventory.version", "verdict": "n/a", "observed": g.get::<mlua::Value>("_VERSION").map(|v| host::display(&v)).unwrap_or_default()}));
    }

    for case in cases() {
        let src = case.body.clone();
        let out = run_case(&src);
        let verdict = judge(case.kind, &out);
        emit(serde_json::json!({
            "test": format!("A.{}", case.name),
            "kind": format!("{:?}", case.kind),
            "verdict": verdict,
            "class": out.class.label(),
            "observed": out.returned.clone().or(out.error.clone()),
            "steps": out.steps,
            "ms": out.elapsed.as_secs_f64() * 1000.0,
        }));
    }

    // Tampering between runs: run 1 tries to change shared builtins; run 2 (new VM) checks.
    let tamper1 = module(
        "local res = {}\n\
         local function try(name, f) local ok, e = pcall(f); res[#res+1] = name .. '=' .. (ok and 'changed' or 'refused') end\n\
         try('string.format', function() string.format = function() return 'PWNED' end end)\n\
         try('table.insert', function() table.insert = function() end end)\n\
         try('ctx.notify', function() ctx.notify = function() end end)\n\
         try('math.pi', function() math.pi = 3 end)\n\
         try('string.rep', function() string.rep = function() return 'x' end end)\n\
         try('_G.leak', function() _G.leak = 1 end)\n\
         leak = 'bare-global'\n\
         try('string-meta', function() getmetatable('').__index = {} end)\n\
         return table.concat(res, ' ')",
    );
    let tamper2 = module(
        "local seen = {\n\
           tostring(_G.leak), tostring(leak), string.format('%d', 7), tostring(table.insert ~= nil),\n\
           tostring(math.pi), ('x'):upper(), tostring(string.rep('a', 2)),\n\
         }\n\
         ctx.notify('probe')\n\
         return table.concat(seen, ' | ')",
    );
    let t1 = run_case(&tamper1);
    let t2 = run_case(&tamper2);
    let expected = "nil | nil | 7 | true | 3.141592653589793 | X | aa";
    let observed2 = t2.returned.clone().unwrap_or_else(|| t2.error.clone().unwrap_or_default());
    let verdict = if observed2 == expected && t2.class == Class::Completed { "blocked" } else { "escaped" };
    emit(serde_json::json!({
        "test": "A.tamper.between-runs",
        "verdict": verdict,
        "observed": format!("run1: {} ({}) | run2: {}", t1.returned.clone().or(t1.error.clone()).unwrap_or_default(), t1.class.label(), observed2),
        "expected_run2": expected,
    }));
}
