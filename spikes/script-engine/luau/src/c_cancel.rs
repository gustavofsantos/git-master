//! Battery C: cancellation latency, 50 trials per scenario, cancel at random points.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::host::{self, Class, Limits, Opts, Reason, Shared};
use crate::util::{emit, stats};

const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn module(body: &str) -> String {
    format!("local M = {{}}\nfunction M.execute(ctx, input)\n{body}\nend\nreturn M\n")
}

/// No budget but cancel may fire: only the cancel path can end these runs.
const NO_BUDGET: Limits = Limits { steps: u64::MAX, wall: Duration::from_secs(120), memory: 0, string_bytes: usize::MAX, ctx_calls: u64::MAX };

pub fn run() {
    let scenarios: Vec<(&str, String)> = vec![
        ("tight-loop", module("while true do end")),
        ("host-call-loop (ctx.now)", module("while true do ctx.now() end")),
        ("blocked in ctx.block_recv", module("ctx.block_recv()\nreturn 'x'")),
        (
            "coroutine.wrap spin",
            module("local f = coroutine.wrap(function() while true do end end)\nf()"),
        ),
        (
            "pcall-in-loop",
            module("while true do pcall(function() for i = 1, 1000 do end end) end"),
        ),
        (
            "deep recursion (15000 frames, spin at bottom)",
            module(
                "local function down(n)\n\
                   if n == 0 then while true do end end\n\
                   return 1 + down(n - 1)\n\
                 end\n\
                 while true do down(15000) end",
            ),
        ),
    ];

    let mut rng = Rng(SEED);
    for (name, src) in &scenarios {
        let mut samples = Vec::new();
        let mut not_cancelled = 0usize;
        let mut ended_before_cancel = 0usize;
        let mut classes: Vec<String> = Vec::new();
        for _ in 0..50 {
            let shared = Shared::new(NO_BUDGET);
            let sh = Arc::clone(&shared);
            let src2 = src.clone();
            let handle = thread::spawn(move || {
                let out = host::run_module(&src2, &sh, Opts::FULL);
                (out.class, out.ended)
            });
            let wait_ms = 30 + rng.next() % 220;
            thread::sleep(Duration::from_millis(wait_ms));
            let t0 = Instant::now();
            shared.cancel();
            let (class, ended) = handle.join().expect("run thread");
            classes.push(class.label());
            if class != Class::Budget(Reason::Cancelled) {
                not_cancelled += 1;
            }
            if ended < t0 {
                ended_before_cancel += 1;
            }
            samples.push(ended.saturating_duration_since(t0).as_secs_f64() * 1000.0);
        }
        let s = stats(&samples);
        let verdict = if not_cancelled == 0 { "contained" } else { "FAILED" };
        emit(serde_json::json!({
            "test": format!("C.{name}"),
            "verdict": verdict,
            "latency_ms": s.json(),
            "not_ended_by_cancel": not_cancelled,
            "ended_before_cancel": ended_before_cancel,
            "samples_ms": samples,
            "classes_seen": classes.iter().collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>(),
        }));
    }

    // Functional: the host value reaches a blocked ctx.block_recv().
    let shared = Shared::new(NO_BUDGET);
    let sh = Arc::clone(&shared);
    let handle = thread::spawn(move || host::run_module(&module("return ctx.block_recv()"), &sh, Opts::FULL));
    thread::sleep(Duration::from_millis(30));
    shared.send_value("host-value");
    let out = handle.join().expect("run thread");
    emit(serde_json::json!({
        "test": "C.block_recv.delivers-value",
        "verdict": if out.returned.as_deref() == Some("host-value") { "completed" } else { "FAILED" },
        "observed": out.returned.clone().or(out.error.clone()),
    }));

    // Wall budget also ends a blocked receive (no cancel, no value).
    let lim = Limits { wall: Duration::from_millis(300), ..NO_BUDGET };
    let shared = Shared::new(lim);
    let out = host::run_module(&module("return ctx.block_recv()"), &shared, Opts::FULL);
    emit(serde_json::json!({
        "test": "C.block_recv.wall-budget",
        "verdict": if matches!(out.class, Class::Budget(Reason::Wall)) { "contained" } else { "FAILED" },
        "class": out.class.label(),
        "observed": out.error.clone(),
        "ms": out.elapsed.as_secs_f64() * 1000.0,
    }));
}
