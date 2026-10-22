//! Battery B: budgets. Each budget case runs in its own child process so that the process
//! peak RSS (VmHWM) belongs to that case alone.

use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::battery_a::wrap;
use crate::emit;
use crate::note;
use crate::sandbox::{self, Budgets};

const CASES: &[&str] = &[
    "instructions",
    "wall-clock",
    "memory",
    "nested-array",
    "string-size",
    "stack-depth",
    "ctx-calls",
    "well-behaved",
];

/// The well-behaved script: a million iterations of arithmetic plus some string work.
pub const WELL_BEHAVED: &str = "fn execute(ctx, input) {\n\
    let s = 0;\n\
    let t = 0;\n\
    for i in 0..1000000 {\n\
        s += i * 3 % 7;\n\
        if i % 100000 == 0 { t += (\"ab\" + i.to_string()).len(); }\n\
    }\n\
    s + t\n\
}\n";

fn stack_src() -> String {
    "fn r(n) { 1 + r(n + 1) }\nfn execute(ctx, input) { r(0) }\n".to_string()
}

fn run_case(name: &str) -> Value {
    let spike = Budgets::SPIKE;
    let off = Budgets::OFF;
    let (src, budgets, hook): (String, Budgets, bool) = match name {
        "instructions" => (wrap("loop { }"), spike, true),
        "wall-clock" => (
            wrap("loop { }"),
            Budgets { wall: Some(Duration::from_millis(500)), ..off },
            true,
        ),
        "memory" => (
            wrap("let s = \"x\"; for k in 0..19 { s += s; } let m = #{}; let i = 0; loop { i += 1; m[i.to_string()] = s; }"),
            spike,
            true,
        ),
        "nested-array" => (wrap("let a = [0]; loop { a = [a, a]; }"), spike, true),
        "string-size" => (wrap("let s = \"x\"; loop { s += s; }"), spike, true),
        "stack-depth" => (stack_src(), spike, true),
        "ctx-calls" => (wrap("loop { ctx.count(); }"), spike, true),
        _ => unreachable!(),
    };
    let out = sandbox::run_thread(sandbox::job(&src, budgets, hook, Arc::new(AtomicBool::new(false)), sandbox::channel().1));
    let (reason, message) = match &out.result {
        Ok(v) => ("completed".to_string(), format!("{v:?}")),
        Err(f) => (f.reason.label().to_string(), f.message.clone()),
    };
    json!({
        "test": format!("B.{name}"),
        "verdict": if out.result.is_err() { "contained" } else { "escaped" },
        "reason": reason,
        "message": message,
        "elapsed_ms": out.elapsed.as_secs_f64() * 1000.0,
        "run_peak_bytes": out.mem_peak,
        "ctx_calls": out.ctx_calls,
        "process_vm_hwm_kib": crate::vm_hwm_kib(),
    })
}

/// Child entry: `b-one <case>`. Prints one JSON line.
pub fn child(args: &[String]) {
    let name = args.first().map(String::as_str).unwrap_or("");
    crate::emit(run_case(name));
}

/// Well-behaved timing: hooks off and hooks on, interleaved (off, on, off, on, ...) so that
/// drift from other load on the machine lands on both sides. Nine repetitions each.
fn well_behaved() -> Value {
    let once = |budgets: Budgets, hook: bool| -> f64 {
        let out = sandbox::run_thread(sandbox::job(
            WELL_BEHAVED,
            budgets,
            hook,
            Arc::new(AtomicBool::new(false)),
            sandbox::channel().1,
        ));
        assert!(out.result.is_ok(), "well-behaved script must complete: {:?}", out.result);
        out.elapsed.as_secs_f64() * 1000.0
    };
    let mut off = Vec::new();
    let mut on = Vec::new();
    for _ in 0..9 {
        off.push(once(Budgets::OFF, false));
        on.push(once(Budgets::SPIKE, true));
    }
    let med = |v: &[f64]| {
        let mut s = v.to_vec();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        s[s.len() / 2]
    };
    let ops = sandbox::ops_used(WELL_BEHAVED, sandbox::default_input());
    let (m_off, m_on) = (med(&off), med(&on));
    json!({
        "test": "B.well-behaved",
        "verdict": "n/a",
        "ops_counted": ops,
        "ms_hooks_off": off,
        "ms_hooks_on": on,
        "median_ms_off": m_off,
        "median_ms_on": m_on,
        "hook_overhead_pct": (m_on - m_off) / m_off * 100.0,
        "budgets_tripped": false,
    })
}

/// Parent: runs every budget case in a child process and forwards its line.
pub fn run() {
    note("B: budgets (one child process per case)");
    let exe = std::env::current_exe().expect("current exe");
    for name in CASES {
        if *name == "well-behaved" {
            let v = well_behaved();
            note(format!(
                "  well-behaved: off {:.1} ms, on {:.1} ms, overhead {:.1}%, ops {}",
                v["median_ms_off"].as_f64().unwrap_or(0.0),
                v["median_ms_on"].as_f64().unwrap_or(0.0),
                v["hook_overhead_pct"].as_f64().unwrap_or(0.0),
                v["ops_counted"]
            ));
            emit(v);
            continue;
        }
        let output = Command::new(&exe).arg("b-one").arg(name).output().expect("spawn child");
        let line = String::from_utf8_lossy(&output.stdout);
        match line.lines().find(|l| l.starts_with('{')) {
            Some(l) => {
                let v: Value = serde_json::from_str(l).expect("child json");
                note(format!(
                    "  {:<14} {:<14} {:>8.1} ms  peak {:>10} B  hwm {:>8} KiB  {}",
                    name,
                    v["reason"].as_str().unwrap_or(""),
                    v["elapsed_ms"].as_f64().unwrap_or(0.0),
                    v["run_peak_bytes"],
                    v["process_vm_hwm_kib"],
                    v["message"].as_str().unwrap_or("")
                ));
                emit(v);
            }
            None => {
                note(format!("  {name}: child produced no JSON (status {:?})", output.status));
                emit(json!({"test": format!("B.{name}"), "verdict": "escaped", "reason": "child-failed", "status": format!("{:?}", output.status)}));
            }
        }
    }
}
