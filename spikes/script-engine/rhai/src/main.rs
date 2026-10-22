//! Spike S-D, Rhai engine. One JSON object per line on stdout; a human summary on stderr.
//! Usage: spike-script-rhai <battery>   where battery is a|b|c|d|f|all, or an internal
//! child command (b-one <case>, d-rss <kind>, d-parallel <threads> <runs>).

mod alloc;
mod battery_a;
mod battery_b;
mod battery_c;
mod battery_d;
mod battery_f;
mod sandbox;

use serde_json::{json, Value};
use std::io::Write;

#[global_allocator]
static GLOBAL: alloc::Counting = alloc::Counting;

/// Prints one JSON line to stdout.
pub fn emit(v: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
}

/// Human summary line on stderr.
pub fn note(s: impl AsRef<str>) {
    eprintln!("{}", s.as_ref());
}

/// Resident set high-water mark of this process, in KiB (Linux /proc).
pub fn vm_hwm_kib() -> Option<u64> {
    proc_status_kib("VmHWM:")
}

pub fn vm_rss_kib() -> Option<u64> {
    proc_status_kib("VmRSS:")
}

fn proc_status_kib(key: &str) -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    s.lines()
        .find(|l| l.starts_with(key))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("all");
    match cmd {
        "a" => battery_a::run(),
        "b" => battery_b::run(),
        "c" => battery_c::run(),
        "d" => battery_d::run(),
        "f" => battery_f::run(),
        "all" => {
            battery_a::run();
            battery_b::run();
            battery_c::run();
            battery_d::run();
            battery_f::run();
        }
        "b-one" => battery_b::child(&args[1..]),
        "d-rss" => battery_d::rss_child(&args[1..]),
        "d-parallel" => battery_d::parallel_child(&args[1..]),
        "x-probe" => probe(&args[1..]),
        other => {
            eprintln!("unknown battery: {other}");
            std::process::exit(2);
        }
    }
}

/// Investigation helper: `x-probe <file> [mem-limit-mib|off] [ops|off]`. Runs one module with
/// the spike budgets (overridden by the arguments) and prints the outcome, time and peak bytes.
fn probe(args: &[String]) {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    let src = std::fs::read_to_string(&args[0]).expect("read file");
    let mut b = sandbox::Budgets::SPIKE;
    if let Some(m) = args.get(1) {
        b.mem = if m == "off" { None } else { Some(m.parse::<usize>().unwrap() << 20) };
    }
    if let Some(o) = args.get(2) {
        b.max_ops = if o == "off" { 0 } else { o.parse().unwrap() };
    }
    let out = sandbox::run_thread(sandbox::job(&src, b, true, Arc::new(AtomicBool::new(false)), sandbox::channel().1));
    let line = match &out.result {
        Ok(v) => format!("ok {v:?}"),
        Err(f) => format!("err {} {}", f.reason.label(), f.message),
    };
    emit(json!({"probe": line, "elapsed_ms": out.elapsed.as_secs_f64() * 1000.0, "peak_bytes": out.mem_peak, "ctx_calls": out.ctx_calls}));
}
