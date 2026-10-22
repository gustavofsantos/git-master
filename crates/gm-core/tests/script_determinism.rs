//! Determinism (D09, D99): the same script, with the same inputs and the same recorded results of
//! its calls, makes the same sequence of `ctx` calls with the same resolved arguments. That is
//! what makes the journal a faithful record and keeps replay possible later (Q23). Property tests
//! over generated scripts, with the real Luau sandbox.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gm_core::script::fake::ScriptedHost;
use gm_core::script::luau::LuauEngine;
use gm_core::script::port::{Budgets, HostReply, RunEnd, RunRequest, ScriptEngine};
use gm_core::script::surface::CtxSurface;
use proptest::prelude::*;
use serde_json::{Value, json};

fn run(source: &str, input: &Value, host: &mut ScriptedHost) -> RunEnd {
    let surface = CtxSurface::from_catalog();
    let request = RunRequest {
        chunk: "gen.lua",
        source,
        input,
        surface: &surface,
        budgets: Budgets::default(),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    LuauEngine::new().run(&request, host)
}

/// One generated statement of a script.
#[derive(Clone, Debug)]
enum Op {
    Notify(String),
    Now,
    Random,
    Branch(String),
    TryRestack,
    /// `for i = 1, n do ctx.notify("loop" .. i) end`
    Loop(u8),
    /// Branches on the result of the last `now()`.
    IfEven,
    /// Reads a field of the input into a call.
    FromInput,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        "[a-z]{1,8}".prop_map(Op::Notify),
        Just(Op::Now),
        Just(Op::Random),
        "[a-z]{1,6}/[a-z]{1,6}".prop_map(Op::Branch),
        Just(Op::TryRestack),
        (0u8..5).prop_map(Op::Loop),
        Just(Op::IfEven),
        Just(Op::FromInput),
    ]
}

fn script(ops: &[Op]) -> String {
    let mut body = String::from("local last = 0\n");
    for op in ops {
        match op {
            Op::Notify(text) => {
                let _ = writeln!(body, "ctx.notify(\"{text}\")");
            }
            Op::Now => body += "last = ctx.now()\n",
            Op::Random => body += "last = ctx.random()\n",
            Op::Branch(name) => {
                let _ = writeln!(body, "ctx.branch.create({{ name = \"{name}\" }})");
            }
            Op::TryRestack => {
                body += "local t = ctx.try(function() return ctx.restack.apply({ plan_id = \"p\" }) end)\nctx.notify(t.outcome)\n";
            }
            Op::Loop(n) => {
                let _ = writeln!(body, "for i = 1, {n} do ctx.notify(\"loop\" .. i) end");
            }
            Op::IfEven => {
                body += "if type(last) == \"number\" and last % 2 == 0 then ctx.notify(\"even\") else ctx.notify(\"odd\") end\n";
            }
            Op::FromInput => body += "ctx.notify(input.inputs.ticket)\n",
        }
    }
    format!("local M = {{}}\nfunction M.execute(ctx, input)\n{body}return last\nend\nreturn M\n")
}

/// The scripted answers: `restack.apply` ends in a conflict, `now` in a given number.
fn host(clock: i64) -> ScriptedHost {
    ScriptedHost::new()
        .reply("now", HostReply::Value(json!(clock)))
        .reply(
            "restack.apply",
            HostReply::Outcome {
                outcome: "conflict".into(),
                value: Value::Null,
                message: "src/a.rs".into(),
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn the_same_script_and_results_make_the_same_calls(
        ops in prop::collection::vec(op(), 0..14),
        clock in 0i64..1_000_000,
        ticket in "[A-Z]{2,4}-[0-9]{1,4}",
    ) {
        let source = script(&ops);
        let input = json!({ "inputs": { "ticket": ticket } });
        let mut first = host(clock);
        let mut second = host(clock);
        let end_first = run(&source, &input, &mut first);
        let end_second = run(&source, &input, &mut second);
        prop_assert_eq!(&end_first, &end_second);
        prop_assert_eq!(&first.calls, &second.calls);
        prop_assert!(matches!(end_first, RunEnd::Returned(_)), "{:?} for\n{}", end_first, source);
    }

    /// A different recorded result changes the calls only where the script reads it.
    #[test]
    fn a_script_that_never_reads_a_result_makes_the_same_calls_whatever_it_is(
        names in prop::collection::vec("[a-z]{1,6}/[a-z]{1,6}", 1..6),
        a in 0i64..1_000_000,
        b in 0i64..1_000_000,
    ) {
        let ops: Vec<Op> = names.into_iter().flat_map(|n| [Op::Now, Op::Branch(n)]).collect();
        let source = script(&ops);
        let input = json!({ "inputs": { "ticket": "T-1" } });
        let (mut left, mut right) = (host(a), host(b));
        run(&source, &input, &mut left);
        run(&source, &input, &mut right);
        // `now` has a different result in each run, but nothing reads it.
        prop_assert_eq!(&left.calls, &right.calls);
    }

    /// Data crosses into Lua and back unchanged: a script that returns its input returns the same
    /// JSON. Containers are never empty, since an empty Lua table is both a list and an object.
    #[test]
    fn data_round_trips_through_the_sandbox(value in json_value()) {
        let source = "local M = {}\nfunction M.execute(ctx, input) return input.x end\nreturn M\n";
        let input = json!({ "x": value });
        let mut host = ScriptedHost::new();
        let end = run(source, &input, &mut host);
        prop_assert_eq!(end, RunEnd::Returned(value));
    }
}

/// JSON without null and without empty containers, which the boundary does not keep apart from
/// "absent".
fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        (-1_000_000i64..1_000_000).prop_map(|n| json!(n)),
        "[ -~]{0,12}".prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 1..4).prop_map(Value::Array),
            prop::collection::hash_map("[a-z]{1,5}", inner, 1..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}
