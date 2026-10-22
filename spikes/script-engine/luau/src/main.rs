//! Spike S-D, Luau engine (mlua 0.11, feature `luau`). See ../SPEC.md and ../RESULTS.md.

mod a_escape;
mod b_budget;
mod c_cancel;
mod d_cost;
mod f_api;
mod host;
mod util;

/// Ad-hoc: reads a module from stdin, runs it with the default budgets, prints the outcome.
fn probe() {
    use std::io::Read;
    let mut src = String::new();
    std::io::stdin().read_to_string(&mut src).expect("stdin");
    let shared = host::Shared::new(host::Limits::DEFAULT);
    let out = host::run_module(&src, &shared, host::Opts::FULL);
    println!("{}", serde_json::json!({"class": out.class.label(), "returned": out.returned, "error": out.error, "steps": out.steps, "ms": out.elapsed.as_secs_f64()*1000.0}));
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    match arg.as_str() {
        "a" => a_escape::run(),
        "b" => b_budget::run(),
        "bo" => b_budget::overhead(),
        "c" => c_cancel::run(),
        "d" => d_cost::run(),
        "f" => f_api::run(),
        "probe" => probe(),
        "pcall-depth" => f_api::plain_pcall_depth(),
        "all" => {
            a_escape::run();
            b_budget::run();
            c_cancel::run();
            d_cost::run();
            f_api::run();
        }
        other => {
            eprintln!("usage: spike-script-luau [a|b|c|d|f|all]  (got {other})");
            std::process::exit(2);
        }
    }
}
