//! Battery D: cost of a fresh VM per run. Sequential timing in this process; resident
//! memory and the 8-thread run in child processes so that each reading is clean.

use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use serde_json::{json, Value};

use crate::emit;
use crate::note;
use crate::sandbox::{self, Budgets};

/// A workflow module of about 40 lines, in the Rhai shape (`META` plus `execute`).
pub const MODULE_40: &str = r#"const META = #{
    name: "sync-repo",
    summary: "Scan a branch and report its messages",
    phases: ["scan", "apply"],
    inputs: #{ repo: "string", branch: "string", count: "int" }
};

fn check_branch(name) {
    if name.len() == 0 {
        throw "empty branch name";
    }
    name
}

fn build_messages(input, count) {
    let parts = [];
    for i in 0..count {
        parts.push(`${input.repo}:${input.branch}:${i}`);
    }
    parts
}

fn scan(ctx, input) {
    let branch = check_branch(input.branch);
    let msgs = build_messages(input, input.count);
    let total = 0;
    for m in msgs {
        ctx.notify(m);
        total += m.len();
    }
    #{ branch: branch, total: total, now: ctx.now() }
}

fn execute(ctx, input) {
    let c = ctx;
    let report = ctx.try_run(|| { scan(c, input) });
    if report.outcome == "error" {
        ctx.notify("scan failed: " + report.message);
        return #{ ok: false };
    }
    report.value
}
"#;

fn pct(sorted: &[f64], p: f64) -> f64 {
    let idx = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[idx]
}

/// One full cycle: engine, hook, ctx, compile, execute, drop. Returns the elapsed microseconds.
fn one_cycle() -> (f64, bool) {
    let (_tx, rx) = sandbox::channel();
    let job = sandbox::job(MODULE_40, Budgets::SPIKE, true, Arc::new(AtomicBool::new(false)), rx);
    let t = Instant::now();
    let out = sandbox::run_here(job);
    let ok = out.result.is_ok();
    drop(out);
    (t.elapsed().as_secs_f64() * 1e6, ok)
}

fn sequential(runs: usize) -> Value {
    for _ in 0..50 {
        one_cycle(); // warm-up, not measured
    }
    let mut us = Vec::with_capacity(runs);
    let mut failures = 0;
    for _ in 0..runs {
        let (t, ok) = one_cycle();
        us.push(t);
        if !ok {
            failures += 1;
        }
    }
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    json!({
        "test": "D.fresh-vm-cycle",
        "verdict": "n/a",
        "runs": runs,
        "failures": failures,
        "us_median": pct(&us, 0.5),
        "us_p99": pct(&us, 0.99),
        "us_min": us[0],
        "us_max": us[us.len() - 1],
    })
}

/// Breakdown: engine build alone, and compile alone (median microseconds over 1000 runs).
fn breakdown() -> Value {
    let mut build = Vec::with_capacity(1000);
    let mut compile = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let t = Instant::now();
        let e = sandbox::sandbox_engine(&Budgets::SPIKE, true);
        build.push(t.elapsed().as_secs_f64() * 1e6);
        let t = Instant::now();
        let ast = e.compile(MODULE_40).expect("compile");
        compile.push(t.elapsed().as_secs_f64() * 1e6);
        drop(ast);
        drop(e);
    }
    build.sort_by(|a, b| a.partial_cmp(b).unwrap());
    compile.sort_by(|a, b| a.partial_cmp(b).unwrap());
    json!({
        "test": "D.breakdown",
        "verdict": "n/a",
        "us_engine_build_median": pct(&build, 0.5),
        "us_engine_build_p99": pct(&build, 0.99),
        "us_compile_median": pct(&compile, 0.5),
        "us_compile_p99": pct(&compile, 0.99),
    })
}

pub fn run() {
    note("D: cost of a fresh VM");
    let bd = breakdown();
    note(format!(
        "  breakdown: engine build median {:.1} us, compile median {:.1} us",
        bd["us_engine_build_median"].as_f64().unwrap(),
        bd["us_compile_median"].as_f64().unwrap()
    ));
    emit(bd);
    let seq = sequential(1000);
    note(format!(
        "  sequential: median {:.1} us, p99 {:.1} us, failures {}",
        seq["us_median"].as_f64().unwrap(),
        seq["us_p99"].as_f64().unwrap(),
        seq["failures"]
    ));
    emit(seq);

    let exe = std::env::current_exe().expect("current exe");
    for kind in ["engine-only", "engine-and-compiled-module"] {
        let output = Command::new(&exe).arg("d-rss").arg(kind).output().expect("spawn d-rss");
        if let Some(v) = parse_child(&output.stdout) {
            note(format!("  idle RSS ({kind}): +{} KiB", v["delta_kib"]));
            emit(v);
        }
    }

    let output = Command::new(&exe).arg("d-parallel").arg("8").arg("1000").output().expect("spawn parallel");
    if let Some(v) = parse_child(&output.stdout) {
        note(format!(
            "  parallel 8x1000: wall {:.2} s, median {:.1} us, p99 {:.1} us, failures {}, hwm {} KiB",
            v["wall_s"].as_f64().unwrap(),
            v["us_median"].as_f64().unwrap(),
            v["us_p99"].as_f64().unwrap(),
            v["failures"],
            v["process_vm_hwm_kib"]
        ));
        emit(v);
    }
}

fn parse_child(stdout: &[u8]) -> Option<Value> {
    let s = String::from_utf8_lossy(stdout);
    let line = s.lines().find(|l| l.starts_with('{'))?;
    serde_json::from_str(line).ok()
}

/// Child: `d-rss <engine-only|engine-and-compiled-module>`.
pub fn rss_child(args: &[String]) {
    let kind = args.first().map(String::as_str).unwrap_or("engine-only");
    let before = crate::vm_rss_kib().unwrap_or(0);
    let mut keep: Vec<Box<dyn std::any::Any>> = Vec::new();
    let e = sandbox::sandbox_engine(&Budgets::SPIKE, true);
    keep.push(Box::new(e));
    if kind == "engine-and-compiled-module" {
        let e2 = sandbox::sandbox_engine(&Budgets::SPIKE, true);
        let ast = e2.compile(MODULE_40).expect("compile");
        keep.push(Box::new(ast));
        keep.push(Box::new(e2));
    }
    let after = crate::vm_rss_kib().unwrap_or(0);
    emit(json!({
        "test": "D.idle-rss",
        "kind": kind,
        "verdict": "n/a",
        "rss_kib_before": before,
        "rss_kib_after": after,
        "delta_kib": after as i64 - before as i64,
    }));
    drop(keep);
}

/// Child: `d-parallel <threads> <runs-per-thread>`.
pub fn parallel_child(args: &[String]) {
    let threads: usize = args.first().and_then(|s| s.parse().ok()).unwrap_or(8);
    let runs: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1000);
    let t0 = Instant::now();
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            thread::Builder::new()
                .stack_size(sandbox::RUN_STACK)
                .spawn(move || {
                    let mut us = Vec::with_capacity(runs);
                    let mut failures = 0usize;
                    for _ in 0..runs {
                        let (t, ok) = one_cycle();
                        us.push(t);
                        if !ok {
                            failures += 1;
                        }
                    }
                    (us, failures)
                })
                .expect("spawn")
        })
        .collect();
    let mut all = Vec::new();
    let mut failures = 0;
    for h in handles {
        let (us, f) = h.join().expect("thread");
        all.extend(us);
        failures += f;
    }
    let wall = t0.elapsed().as_secs_f64();
    all.sort_by(|a, b| a.partial_cmp(b).unwrap());
    emit(json!({
        "test": "D.parallel",
        "verdict": "n/a",
        "threads": threads,
        "runs_per_thread": runs,
        "failures": failures,
        "wall_s": wall,
        "us_median": pct(&all, 0.5),
        "us_p99": pct(&all, 0.99),
        "process_vm_hwm_kib": crate::vm_hwm_kib(),
    }));
}
