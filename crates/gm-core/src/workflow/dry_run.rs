//! Dry run of a workflow (docs/10): walks the transition function from the
//! start and records each step, without performing a side effect. `noop` and
//! `set` are answered as the driver answers them; every other action is not
//! run and gets an assumed outcome, so the walk goes on past it. No I/O.

use serde_json::{Value, json};

use super::catalog::{self, Level};
use super::machine::{self, Context, Effect};
use super::model::{Definition, End};

/// Steps a dry run records before it stops. Bounds a loop the walk cannot see
/// through, such as a cycle of `noop` states.
pub const MAX_STEPS: usize = 200;

/// The actions the driver carries out (its `run_action` and `start_exec` /
/// `await_approval`). A dry run of any other action stops with
/// `Unavailable`. Keep in step with the driver's dispatch.
const IMPLEMENTED: &[&str] = &[
    "noop",
    "notify",
    "approve",
    "exec",
    "commit.suggest_message",
    "set",
    "stage.all",
    "commit.create",
    "branch.create",
    "branch.checkout",
];

/// The walk of one workflow: the steps it records and why it stopped.
#[derive(Clone, Debug, PartialEq)]
pub struct DryRun {
    pub steps: Vec<DryStep>,
    pub stop: DryStop,
}

/// One state the walk visited.
#[derive(Clone, Debug, PartialEq)]
pub struct DryStep {
    /// Path of the state (`each/plan` inside a `for_each`).
    pub state: String,
    pub action: String,
    pub level: Level,
    /// The rendered input: for `exec`, the argv and stdin it would send.
    pub input: Value,
    /// The answer was computed as the driver computes it (`noop`, `set`).
    /// Nothing ran in any case.
    pub executed: bool,
    /// The outcome and output were assumed, not computed: the action was not
    /// run (`approve` is assumed approved).
    pub assumed: bool,
    pub outcome: String,
    pub output: Value,
}

/// Why the walk stopped.
#[derive(Clone, Debug, PartialEq)]
pub enum DryStop {
    /// The run would end here.
    Ended { end: End, reason: Option<String> },
    /// The run would wait for this engine event.
    Waiting { event: String },
    /// The driver does not carry out this action yet.
    Unavailable { action: String },
    /// The machine refused the definition or the context.
    Error(String),
    /// `MAX_STEPS` steps were recorded.
    StepLimit,
}

/// Walks `definition` from its start with `context`, without performing any
/// step. See the module docs for how each action is answered.
#[must_use]
pub fn dry_run(definition: &Definition, context: &Context) -> DryRun {
    let mut steps = Vec::new();
    let (mut run, mut effect) = match machine::start(definition, context) {
        Ok(started) => started,
        Err(error) => {
            return DryRun {
                steps,
                stop: DryStop::Error(error.to_string()),
            };
        }
    };
    loop {
        let (state, action, input, level) = match effect {
            Effect::Run {
                state,
                action,
                input,
                level,
                ..
            } => (state, action, input, level),
            Effect::Wait { event, .. } => {
                return DryRun {
                    steps,
                    stop: DryStop::Waiting { event },
                };
            }
            Effect::Finish { end, reason, .. } => {
                return DryRun {
                    steps,
                    stop: DryStop::Ended { end, reason },
                };
            }
        };
        if !IMPLEMENTED.contains(&action.as_str()) {
            return DryRun {
                steps,
                stop: DryStop::Unavailable { action },
            };
        }
        if steps.len() == MAX_STEPS {
            return DryRun {
                steps,
                stop: DryStop::StepLimit,
            };
        }

        let (executed, assumed, outcome, output) = answer(&action, &input);
        steps.push(DryStep {
            state,
            action,
            level,
            input,
            executed,
            assumed,
            outcome: outcome.clone(),
            output: output.clone(),
        });
        match machine::next(definition, context, &run, &outcome, output) {
            Ok(step) => {
                run = step.run;
                effect = step.effect;
            }
            Err(error) => {
                return DryRun {
                    steps,
                    stop: DryStop::Error(error.to_string()),
                };
            }
        }
    }
}

/// The answer of a step that is not run: `(executed, assumed, outcome, output)`.
fn answer(action: &str, input: &Value) -> (bool, bool, String, Value) {
    match action {
        "noop" => (true, false, "done".to_owned(), json!({})),
        "set" => (true, false, "done".to_owned(), input.clone()),
        // The human's approval is assumed: the dry run shows what follows it.
        "approve" => (false, true, "approved".to_owned(), json!({})),
        "exec" => (false, true, "done".to_owned(), exec_placeholder(input)),
        other => (false, true, assumed_outcome(other), json!({})),
    }
}

/// What a dry run shows in place of an `exec` answer it never got.
pub const EXEC_PLACEHOLDER: &str = "<output of exec>";

/// An `exec` output with the shape the driver gives (`exec::run`), so a later
/// state that reads `output.text` renders instead of failing: the text is
/// `EXEC_PLACEHOLDER`, and `format = "json"` adds an empty `json`.
fn exec_placeholder(input: &Value) -> Value {
    let mut output = json!({
        "text": EXEC_PLACEHOLDER,
        "exit_code": 0,
        "stderr": "",
        "truncated": false,
    });
    if input.get("format").and_then(Value::as_str) == Some("json") {
        output["json"] = json!({});
    }
    output
}

/// `done` when the action can end with it, else the first outcome the catalog
/// lists for it.
fn assumed_outcome(action: &str) -> String {
    catalog::action(action).map_or_else(
        || "done".to_owned(),
        |spec| {
            if spec.outcomes.contains(&"done") {
                "done".to_owned()
            } else {
                spec.outcomes
                    .first()
                    .copied()
                    .unwrap_or("failed")
                    .to_owned()
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::load;
    use super::super::machine::Context;
    use super::super::model::{Definition, End};
    use super::super::parse::parse;
    use super::*;

    /// A definition that passes the validator.
    fn loaded(source: &str) -> Definition {
        load(source)
            .unwrap_or_else(|diagnostics| panic!("fixture does not load: {diagnostics:?}"))
            .0
    }

    /// A definition the parser accepts but the validator would refuse: a
    /// `set` outside a transform, or a cycle with no limit. The dry run walks
    /// it all the same.
    fn parsed(source: &str) -> Definition {
        parse(source).unwrap_or_else(|diagnostic| panic!("fixture does not parse: {diagnostic:?}"))
    }

    fn context(inputs: Value) -> Context {
        Context {
            inputs,
            trigger: json!({"on": "manual"}),
            repo: json!({"root": "/repo", "branch": "main", "staged_diff": ""}),
        }
    }

    #[test]
    fn linear_workflow_records_exec_and_commit_without_running_them() {
        let def = parsed(
            r#"
schema = 1
name = "linear"

[trigger]
on = "manual"

[[state]]
id = "msg"
action = "set"
with = { value = "feat: x" }

[[state]]
id = "ask"
action = "exec"
with = { argv = ["git", "rev-parse", "HEAD"], stdin = "${states.msg.output.value}" }

[[state]]
id = "commit"
action = "commit.create"
with = { message = "${states.msg.output.value}" }
"#,
        );

        let dry = dry_run(&def, &context(json!({})));

        assert_eq!(
            dry.stop,
            DryStop::Ended {
                end: End::Done,
                reason: None
            }
        );
        assert_eq!(dry.steps.len(), 3);
        assert_eq!(dry.steps[0].state, "msg");
        assert!(dry.steps[0].executed);
        assert!(!dry.steps[0].assumed);
        assert_eq!(dry.steps[0].output, json!({"value": "feat: x"}));

        let ask = &dry.steps[1];
        assert_eq!(ask.action, "exec");
        assert_eq!(ask.level, Level::Read);
        assert_eq!(
            ask.input,
            json!({"argv": ["git", "rev-parse", "HEAD"], "stdin": "feat: x"})
        );
        assert!(!ask.executed);
        assert!(ask.assumed);
        assert_eq!(ask.outcome, "done");
        assert_eq!(ask.output["text"], EXEC_PLACEHOLDER);

        let commit = &dry.steps[2];
        assert_eq!(commit.action, "commit.create");
        assert_eq!(commit.level, Level::Apply);
        assert_eq!(commit.input, json!({"message": "feat: x"}));
        assert!(!commit.executed);
        assert!(commit.assumed);
    }

    #[test]
    fn a_state_that_reads_exec_text_renders_the_placeholder() {
        let def = loaded(include_str!(
            "../../tests/workflows/suggest-commit-message.toml"
        ));

        let dry = dry_run(&def, &context(json!({})));

        assert_eq!(
            dry.stop,
            DryStop::Ended {
                end: End::Done,
                reason: None
            }
        );
        assert_eq!(dry.steps.len(), 2);
        assert_eq!(dry.steps[1].input, json!({"message": EXEC_PLACEHOLDER}));
    }

    #[test]
    fn approve_is_assumed_approved_and_the_walk_follows_it() {
        let def = loaded(
            r#"
schema = 1
name = "ask-first"

[trigger]
on = "manual"

[[state]]
id = "ask"
action = "approve"
with = { summary = "Continue?" }
on.approved = "note"
on.rejected = "end.failed"

[[state]]
id = "note"
action = "notify"
with = { text = "went ahead" }
"#,
        );

        let dry = dry_run(&def, &context(json!({})));

        assert_eq!(
            dry.stop,
            DryStop::Ended {
                end: End::Done,
                reason: None
            }
        );
        assert_eq!(dry.steps.len(), 2);
        let ask = &dry.steps[0];
        assert_eq!(ask.action, "approve");
        assert_eq!(ask.input, json!({"summary": "Continue?"}));
        assert!(!ask.executed);
        assert!(ask.assumed);
        assert_eq!(ask.outcome, "approved");
        assert_eq!(dry.steps[1].state, "note");
        assert_eq!(dry.steps[1].outcome, "done");
        assert!(dry.steps[1].assumed);
    }

    #[test]
    fn a_guard_on_a_set_output_picks_the_branch() {
        let source = r#"
schema = 1
name = "branch-on-value"

[trigger]
on = "transform.commit_message"

[[state]]
id = "pick"
action = "set"
with = { branch = "feat" }

[[state]]
id = "check"
action = "noop"
with = {}
on.done = [
  { to = "main_path", if = '${states.pick.output.branch} == "main"' },
  { to = "feature_path", if = '${states.pick.output.branch} == "feat"' },
]

[[state]]
id = "main_path"
action = "notify"
with = { text = "main" }

[[state]]
id = "feature_path"
action = "notify"
with = { text = "feature" }
"#;
        let def = loaded(source);

        let dry = dry_run(&def, &context(json!({})));

        let states: Vec<&str> = dry.steps.iter().map(|step| step.state.as_str()).collect();
        assert_eq!(states, ["pick", "check", "feature_path"]);
        assert!(dry.steps[1].executed, "noop is computed, not assumed");
        assert_eq!(
            dry.stop,
            DryStop::Ended {
                end: End::Done,
                reason: None
            }
        );
    }

    #[test]
    fn a_wait_stops_the_walk_after_the_steps_before_it() {
        let def = parsed(
            r#"
schema = 1
name = "pause"

[trigger]
on = "manual"

[[state]]
id = "note"
action = "notify"
with = { text = "before" }

[[state]]
id = "pause"
wait = "restack.resumed"
"#,
        );

        let dry = dry_run(&def, &context(json!({})));

        assert_eq!(dry.steps.len(), 1);
        assert_eq!(
            dry.stop,
            DryStop::Waiting {
                event: "restack.resumed".to_owned()
            }
        );
    }

    #[test]
    fn for_each_over_a_set_output_walks_each_item() {
        let def = parsed(
            r#"
schema = 1
name = "each-item"

[trigger]
on = "transform.commit_message"

[[state]]
id = "list"
action = "set"
with = { items = ["a", "b"] }

[[state]]
id = "each"
for_each = "${states.list.output.items}"
as = "name"
states = [
  { id = "say", action = "notify", with = { text = "${name}" } },
]
"#,
        );

        let dry = dry_run(&def, &context(json!({})));

        let states: Vec<&str> = dry.steps.iter().map(|step| step.state.as_str()).collect();
        assert_eq!(states, ["list", "each/say", "each/say"]);
        assert_eq!(dry.steps[1].input, json!({"text": "a"}));
        assert_eq!(dry.steps[2].input, json!({"text": "b"}));
        assert_eq!(
            dry.stop,
            DryStop::Ended {
                end: End::Done,
                reason: None
            }
        );
    }

    #[test]
    fn a_loop_without_a_limit_stops_at_the_step_cap() {
        let def = parsed(
            r#"
schema = 1
name = "spin"

[trigger]
on = "manual"

[[state]]
id = "a"
action = "noop"
with = {}
on.done = "b"

[[state]]
id = "b"
action = "noop"
with = {}
on.done = "a"
"#,
        );

        let dry = dry_run(&def, &context(json!({})));

        assert_eq!(dry.steps.len(), MAX_STEPS);
        assert_eq!(dry.stop, DryStop::StepLimit);
    }

    #[test]
    fn an_action_the_driver_lacks_stops_the_walk_before_it_is_recorded() {
        let def = loaded(
            r#"
schema = 1
name = "plan-only"

[trigger]
on = "manual"

[[state]]
id = "note"
action = "notify"
with = { text = "start" }

[[state]]
id = "plan"
action = "restack.plan"
with = { root = "main" }
"#,
        );

        let dry = dry_run(&def, &context(json!({})));

        assert_eq!(dry.steps.len(), 1);
        assert_eq!(
            dry.stop,
            DryStop::Unavailable {
                action: "restack.plan".to_owned()
            }
        );
    }

    #[test]
    fn an_expression_error_ends_the_run_failed_as_the_machine_says() {
        let def = parsed(
            r#"
schema = 1
name = "broken-ref"

[trigger]
on = "manual"

[[state]]
id = "say"
action = "notify"
with = { text = "${states.nope.output.text}" }
"#,
        );

        let dry = dry_run(&def, &context(json!({})));

        assert!(dry.steps.is_empty());
        let DryStop::Ended {
            end: End::Failed,
            reason: Some(reason),
        } = dry.stop
        else {
            panic!("expected a failed end, got {:?}", dry.stop);
        };
        assert!(reason.contains("nope"), "reason: {reason}");
    }
}
