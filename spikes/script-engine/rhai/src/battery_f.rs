//! Battery F: API facts the design needs. Each item is a small probe with its evidence.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::json;

use crate::battery_d::MODULE_40;
use crate::emit;
use crate::note;
use crate::sandbox::{self, Budgets};

fn probe_run(src: &str) -> sandbox::Out {
    sandbox::run_here(sandbox::job(src, Budgets::SPIKE, true, Arc::new(AtomicBool::new(false)), sandbox::channel().1))
}

fn outcome_text(r: &Result<rhai::Dynamic, sandbox::Failure>) -> String {
    match r {
        Ok(v) => format!("ok {v:?}"),
        Err(f) => format!("err {}: {}", f.reason.label(), f.message),
    }
}

pub fn run() {
    note("F: API facts");

    // 1. The module shape: execute(ctx, input) works; a module without execute fails clearly.
    let ok = probe_run(MODULE_40);
    let missing = probe_run("fn run(ctx, input) { 1 }");
    emit(json!({
        "test": "F.module-shape",
        "verdict": "n/a",
        "execute_defined": outcome_text(&ok.result),
        "no_execute": outcome_text(&missing.result),
        "ctx_notes": ok.notes,
    }));

    // 2. M.meta without ctx and without host functions.
    let meta = sandbox::read_meta(MODULE_40);
    let meta_text = match &meta {
        Ok(v) => format!("{v:?}"),
        Err(f) => format!("err {}: {}", f.reason.label(), f.message),
    };
    let meta_with_ctx = sandbox::read_meta("const META = #{ a: 1 };\nctx.notify(\"x\");\n");
    emit(json!({
        "test": "F.meta-without-ctx",
        "verdict": "n/a",
        "meta": meta_text,
        "top_level_ctx_call": match &meta_with_ctx {
            Ok(v) => format!("ok {v:?}"),
            Err(f) => format!("err {}: {}", f.reason.label(), f.message),
        },
    }));

    // 3. Syntax errors: message, line, column.
    let bad = [
        ("missing operand", "fn execute(ctx, input) {\n    let x = ;\n}\n"),
        ("unclosed block", "fn execute(ctx, input) {\n    let x = 1;\n"),
        ("reserved word", "fn execute(ctx, input) {\n    let yield = 1;\n}\n"),
        ("eval disabled", "fn execute(ctx, input) {\n    eval(\"1\")\n}\n"),
    ];
    for (name, src) in bad {
        let r = sandbox::read_meta(src);
        let compiled = sandbox_compile(src);
        emit(json!({
            "test": format!("F.syntax-error.{name}"),
            "verdict": "n/a",
            "message": compiled,
            "read_meta": match r { Ok(_) => "ok".to_string(), Err(f) => f.message },
        }));
    }

    // 4. error() values: tables and functions reach the host intact.
    let table = probe_run("fn execute(ctx, input) { throw #{ code: 42, path: \"a/b\" }; }");
    let func = probe_run("fn execute(ctx, input) { throw || 1; }");
    emit(json!({
        "test": "F.error-values",
        "verdict": "n/a",
        "table_reason": table.result.as_ref().err().map(|f| f.reason.label()),
        "table_payload": table.result.as_ref().err().and_then(|f| f.payload.clone()),
        "function_payload": func.result.as_ref().err().and_then(|f| f.payload.clone()),
    }));

    // 4b. `ctx.try` cannot be spelled: `try` is a keyword and cannot follow a dot.
    let try_name = sandbox::sandbox_engine(&Budgets::SPIKE, true).compile(
        "fn execute(ctx, input) { ctx.try(|| { 1 }) }",
    );
    let try_run_name = sandbox::sandbox_engine(&Budgets::SPIKE, true).compile(
        "fn execute(ctx, input) { ctx.try_run(|| { 1 }) }",
    );
    emit(json!({
        "test": "F.ctx-try-name",
        "verdict": "n/a",
        "ctx.try": match try_name { Ok(_) => "compiled".to_string(), Err(pe) => format!("{} (line {:?}, column {:?})", pe.0, pe.1.line(), pe.1.position()) },
        "ctx.try_run": match try_run_name { Ok(_) => "compiled".to_string(), Err(pe) => pe.0.to_string() },
    }));

    // 5. Listing ctx's methods: the definitions writer (feature `internals`).
    let e = sandbox::sandbox_engine(&Budgets::SPIKE, true);
    let defs = e.definitions();
    let single = defs.single_file();
    let files: Vec<String> = defs.iter_files().map(|(name, _)| name).collect();
    let ctx_lines: Vec<&str> = single
        .lines()
        .filter(|l| l.contains("Ctx") || l.contains("notify") || l.contains("block_recv"))
        .take(12)
        .collect();
    emit(json!({
        "test": "F.ctx-method-listing",
        "verdict": "n/a",
        "definitions_files": files,
        "ctx_lines": ctx_lines,
        "unknown_method": outcome_text(&probe_run("fn execute(ctx, input) { ctx.secret() }").result),
    }));
    note(format!("  definitions: {} files; ctx methods found in the single-file output: {}", files.len(), ctx_lines.len()));
}

fn sandbox_compile(src: &str) -> String {
    let e = sandbox::sandbox_engine(&Budgets::SPIKE, true);
    match e.compile(src) {
        Ok(_) => "compiled".to_string(),
        Err(pe) => format!(
            "{} at line {:?}, column {:?}",
            pe.0,
            pe.1.line(),
            pe.1.position()
        ),
    }
}
