//! The workflow examples under `tests/workflows/`: every valid one loads, every
//! malformed one under `invalid/` fails at the line its `# expect:` comment
//! names, and a random walk through each valid one reaches its end. The walk
//! answers each effect the way the engine would, with a random outcome from
//! the catalog. No window and no repository needed.

use std::fs;
use std::path::{Path, PathBuf};

use gm_core::workflow::catalog;
use gm_core::workflow::machine::{self, Context, Effect, MAX_TRANSITIONS, Position, Run};
use gm_core::workflow::model::Definition;
use gm_core::workflow::{self, Diagnostic, Severity};
use proptest::collection::vec;
use proptest::prelude::*;
use serde_json::{Value, json};

/// Effects a walk may take before it must have finished.
const CALL_LIMIT: usize = 200;

fn examples_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/workflows"))
}

/// The `*.toml` files directly in `dir`, sorted by name.
fn toml_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();
    files
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .expect("a file name")
        .to_string_lossy()
        .into_owned()
}

fn render(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every valid example, parsed and validated, with its file name.
fn valid_examples() -> Vec<(String, Definition)> {
    let files = toml_files(&examples_dir());
    assert!(
        files.len() >= 6,
        "expected at least 6 examples, found {}",
        files.len()
    );
    files
        .iter()
        .map(|path| {
            let name = file_name(path);
            let source = fs::read_to_string(path).expect("readable example");
            let (definition, _) = workflow::load(&source)
                .unwrap_or_else(|errors| panic!("{name} does not load:\n{}", render(&errors)));
            (name, definition)
        })
        .collect()
}

#[test]
fn every_example_loads_without_errors() {
    let files = toml_files(&examples_dir());
    assert!(
        files.len() >= 6,
        "expected at least 6 examples, found {}",
        files.len()
    );
    for path in &files {
        let name = file_name(path);
        let source = fs::read_to_string(path).expect("readable example");
        let (_, warnings) = workflow::load(&source)
            .unwrap_or_else(|errors| panic!("{name} does not load:\n{}", render(&errors)));
        let errors: Vec<&Diagnostic> = warnings
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{name}:\n{}", render(&warnings));
    }
}

/// The line and the text a malformed example must report, read from its
/// first line: `# expect: line <N>: <text>`.
fn expectation(source: &str) -> (usize, String) {
    let first = source.lines().next().unwrap_or_default();
    let rest = first.strip_prefix("# expect: line ").unwrap_or_else(|| {
        panic!("first line must be `# expect: line <N>: <text>`, got {first:?}")
    });
    let (line, text) = rest.split_once(": ").unwrap_or_else(|| {
        panic!("first line must be `# expect: line <N>: <text>`, got {first:?}")
    });
    let line = line
        .parse()
        .unwrap_or_else(|e| panic!("bad line in {first:?}: {e}"));
    (line, text.to_owned())
}

#[test]
fn every_malformed_example_fails_at_its_expected_line() {
    let files = toml_files(&examples_dir().join("invalid"));
    assert!(
        files.len() >= 8,
        "expected at least 8 malformed examples, found {}",
        files.len()
    );
    for path in &files {
        let name = file_name(path);
        let source = fs::read_to_string(path).expect("readable example");
        let (line, text) = expectation(&source);
        let Err(diagnostics) = workflow::load(&source) else {
            panic!("{name} loads, but it is malformed");
        };
        assert!(
            diagnostics
                .iter()
                .any(|d| d.line == line && d.message.contains(&text)),
            "{name}: expected line {line} containing {text:?}, got:\n{}",
            render(&diagnostics)
        );
    }
}

/// The fields the examples read from the trigger, the inputs and the
/// repository, with values for each.
fn context() -> Context {
    Context {
        inputs: json!({"ticket": "GM-42"}),
        trigger: json!({
            "parent": "feature/parent",
            "root": "feature/root",
            "ticket": "GM-42",
        }),
        repo: json!({"staged_diff": "diff --git a/src/lib.rs b/src/lib.rs"}),
    }
}

/// One random answer: the selector of the outcome, and the output fields.
#[derive(Clone, Debug)]
struct Pick {
    choice: usize,
    branch_count: u64,
    text_empty: bool,
    roots: usize,
}

fn pick() -> impl Strategy<Value = Pick> {
    (0usize..16, 0u64..3, any::<bool>(), 0usize..3).prop_map(
        |(choice, branch_count, text_empty, roots)| Pick {
            choice,
            branch_count,
            text_empty,
            roots,
        },
    )
}

/// The output a state returns: every field an example reads.
fn output(pick: &Pick) -> Value {
    json!({
        "branch_count": pick.branch_count,
        "plan_id": "plan-1",
        "text": if pick.text_empty { "" } else { "msg" },
        "roots": (0..pick.roots).map(|i| format!("root-{i}")).collect::<Vec<_>>(),
        "value": "msg",
    })
}

/// The outcome and the output the engine would report for an effect.
fn answer(effect: &Effect, pick: &Pick) -> (String, Value) {
    let outcome = match effect {
        Effect::Run { action, .. } => {
            let spec = catalog::action(action).unwrap_or_else(|| panic!("unknown action {action}"));
            spec.outcomes[pick.choice % spec.outcomes.len()].to_owned()
        }
        Effect::Wait { timeout, .. } => {
            // `timeout` exists only on a wait state that sets one.
            let mut outcomes = vec!["resumed", "aborted"];
            if timeout.is_some() {
                outcomes.push("timeout");
            }
            outcomes[pick.choice % outcomes.len()].to_owned()
        }
        Effect::Finish { .. } => unreachable!("the walk stops at Finish"),
    };
    (outcome, output(pick))
}

/// One outcome of a state, with the output it returned.
type Answer = (String, Value);

/// A finished walk: the final run and its effect, and the answers given.
#[derive(Debug, PartialEq)]
struct Walk {
    run: Run,
    finish: Effect,
    answers: Vec<Answer>,
}

/// Runs the definition from its start, answering each effect with the next
/// pick (the picks cycle), until `Finish`. Fails when that takes more than
/// `CALL_LIMIT` calls or when `next` rejects an outcome the catalog lists.
fn walk(definition: &Definition, context: &Context, picks: &[Pick]) -> Result<Walk, String> {
    let (mut run, mut effect) =
        machine::start(definition, context).map_err(|e| format!("start: {e}"))?;
    let mut answers = Vec::new();
    while !matches!(effect, Effect::Finish { .. }) {
        if answers.len() == CALL_LIMIT {
            return Err(format!(
                "no finish within {CALL_LIMIT} calls; stopped at {effect:?}"
            ));
        }
        let (outcome, output) = answer(&effect, &picks[answers.len() % picks.len()]);
        let step =
            machine::next(definition, context, &run, &outcome, output.clone()).map_err(|e| {
                format!(
                    "call {} at {:?}: outcome `{outcome}` rejected: {e}",
                    answers.len(),
                    run.position
                )
            })?;
        answers.push((outcome, output));
        run = step.run;
        effect = step.effect;
    }
    Ok(Walk {
        run,
        finish: effect,
        answers,
    })
}

/// Starts the definition and applies the recorded answers in order.
fn replay(
    definition: &Definition,
    context: &Context,
    answers: &[Answer],
) -> Result<(Run, Effect), String> {
    let (mut run, mut effect) =
        machine::start(definition, context).map_err(|e| format!("start: {e}"))?;
    for (outcome, output) in answers {
        let step = machine::next(definition, context, &run, outcome, output.clone())
            .map_err(|e| format!("replay: outcome `{outcome}` rejected: {e}"))?;
        run = step.run;
        effect = step.effect;
    }
    Ok((run, effect))
}

proptest! {
    #[test]
    fn random_walk_reaches_its_end(picks in vec(pick(), 1..=64)) {
        for (name, definition) in valid_examples() {
            let walked = walk(&definition, &context(), &picks)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            prop_assert!(
                matches!(walked.run.position, Position::Ended { .. }),
                "{name}: finished without an end: {:?}",
                walked.run.position
            );
            prop_assert!(
                walked.run.transitions <= MAX_TRANSITIONS,
                "{name}: {} transitions, cap {MAX_TRANSITIONS}",
                walked.run.transitions
            );
            prop_assert!(
                walked.answers.len() <= CALL_LIMIT,
                "{name}: {} calls",
                walked.answers.len()
            );
        }
    }

    #[test]
    fn replaying_the_same_outcomes_gives_the_same_run(picks in vec(pick(), 1..=64)) {
        for (name, definition) in valid_examples() {
            let ctx = context();
            let walked = walk(&definition, &ctx, &picks)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let first = replay(&definition, &ctx, &walked.answers)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let second = replay(&definition, &ctx, &walked.answers)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            prop_assert_eq!(&first, &second, "{}: replays differ", name);
            prop_assert_eq!(&first.0, &walked.run, "{}: replay differs from the walk", name);
            prop_assert_eq!(&first.1, &walked.finish, "{}: replay ends differently", name);
        }
    }
}
