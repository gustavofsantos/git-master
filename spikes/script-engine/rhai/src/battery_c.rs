//! Battery C: cancellation latency. The driver flips the flag at a pseudo-random point
//! (5 to 50 ms into the run) and measures the time until the run thread returns.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::emit;
use crate::note;
use crate::sandbox::{self, Budgets, Reason, RUN_STACK};

const TRIALS: usize = 50;

struct Scenario {
    name: &'static str,
    src: &'static str,
    budgets: Budgets,
    /// Send a wake-up value on the channel as well as flipping the flag.
    wake: bool,
}

fn scenarios() -> Vec<Scenario> {
    let off = Budgets::OFF;
    vec![
        Scenario {
            name: "tight-pure-loop",
            src: "fn execute(ctx, input) { let i = 0; loop { i += 1; } }",
            budgets: off,
            wake: false,
        },
        Scenario {
            name: "loop-calling-host-fn",
            src: "fn execute(ctx, input) { loop { ctx.now(); } }",
            budgets: off,
            wake: false,
        },
        Scenario {
            name: "blocked-block_recv (wake sent)",
            src: "fn execute(ctx, input) { ctx.block_recv() }",
            budgets: off,
            wake: true,
        },
        Scenario {
            name: "blocked-block_recv (poll only)",
            src: "fn execute(ctx, input) { ctx.block_recv() }",
            budgets: off,
            wake: false,
        },
        Scenario {
            name: "loop-inside-try",
            src: "fn execute(ctx, input) { loop { try { let i = 0; loop { i += 1; } } catch (e) { } } }",
            budgets: off,
            wake: false,
        },
        Scenario {
            name: "deep-recursion-then-loop (900 levels)",
            src: "fn r(n) { if n == 0 { let i = 0; loop { i += 1; } } 1 + r(n - 1) }\n\
                  fn execute(ctx, input) { r(900) }",
            budgets: Budgets { max_call_levels: 1000, ..off },
            wake: false,
        },
    ]
}

/// Small deterministic generator, so the random cancel points repeat between runs.
struct Xorshift(u64);
impl Xorshift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let idx = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[idx]
}

pub fn run() {
    note(format!("C: cancellation latency, {TRIALS} trials each"));
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    for s in scenarios() {
        let mut latencies = Vec::with_capacity(TRIALS);
        let mut not_cancelled = Vec::new();
        for trial in 0..TRIALS {
            let cancel = Arc::new(AtomicBool::new(false));
            let (tx, rx) = sandbox::channel();
            let job = sandbox::job(s.src, s.budgets, true, cancel.clone(), rx);
            let handle = thread::Builder::new()
                .stack_size(RUN_STACK)
                .spawn(move || sandbox::run_here(job))
                .expect("spawn");
            let delay_ms = 5 + rng.next() % 46;
            thread::sleep(Duration::from_millis(delay_ms));
            let t_cancel = Instant::now();
            cancel.store(true, Ordering::Release);
            if s.wake {
                let _ = tx.send(sandbox::wake_value());
            }
            let out = handle.join().expect("run thread");
            drop(tx);
            match &out.result {
                Err(f) if f.reason == Reason::Cancelled => {
                    let lat = out.finished.saturating_duration_since(t_cancel);
                    latencies.push(lat.as_secs_f64() * 1000.0);
                }
                other => not_cancelled.push(format!("trial {trial}: {other:?}")),
            }
        }
        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let verdict = if not_cancelled.is_empty() { "contained" } else { "escaped" };
        let (min, med, p99, max) = if latencies.is_empty() {
            (f64::NAN, f64::NAN, f64::NAN, f64::NAN)
        } else {
            (
                latencies[0],
                percentile(&latencies, 0.5),
                percentile(&latencies, 0.99),
                *latencies.last().unwrap(),
            )
        };
        note(format!(
            "  {:<38} n={} min {:.3} median {:.3} p99 {:.3} max {:.3} ms  not-cancelled {}",
            s.name,
            latencies.len(),
            min,
            med,
            p99,
            max,
            not_cancelled.len()
        ));
        emit(json!({
            "test": format!("C.{}", s.name),
            "verdict": verdict,
            "trials": TRIALS,
            "cancelled": latencies.len(),
            "latency_ms_min": min,
            "latency_ms_median": med,
            "latency_ms_p99": p99,
            "latency_ms_max": max,
            "not_cancelled": not_cancelled,
            "wake_sent": s.wake,
        }));
    }
}
