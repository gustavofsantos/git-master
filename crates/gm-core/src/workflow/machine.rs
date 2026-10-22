//! The pure transition function of a workflow run (design/workflow-scripts.md, D78, D80):
//! given the definition, the run so far and the outcome of the current
//! state, it returns the next run, the transition to record and the effect
//! the engine must carry out. No I/O; the engine owns effects and storage.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::catalog::{self, Level};
use super::expr::{self, ExprError, Scope};
use super::model::{Approval, Definition, End, State, StateKind, Target};

/// Hard cap of transitions per run (D78).
pub const MAX_TRANSITIONS: u32 = 1000;

/// What the run reads besides state outputs. Stored whole by the engine, so a
/// resume can rebuild it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Context {
    pub inputs: Value,
    pub trigger: Value,
    pub repo: Value,
}

/// A run in progress or ended. Plain data, so the engine can store it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub position: Position,
    /// Latest output of each top-level state, by id.
    pub outputs: BTreeMap<String, Value>,
    /// Times each guarded or limited edge was taken.
    #[serde(with = "edge_uses")]
    pub edge_uses: BTreeMap<EdgeKey, u32>,
    pub transitions: u32,
    /// Output of the latest `set`, returned by a `transform.*` run.
    pub last_set: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Position {
    /// Running or waiting in a top-level state; `inner` while inside a
    /// `for_each`.
    At {
        state: String,
        inner: Option<Inner>,
    },
    Ended {
        end: End,
        reason: Option<String>,
    },
}

/// Where a `for_each` stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Inner {
    /// Index of the current item.
    pub index: usize,
    /// The items, rendered once when the `for_each` was entered.
    pub items: Vec<Value>,
    /// Current state of the nested machine.
    pub state: String,
    /// Outputs of the nested states for the current item.
    pub outputs: BTreeMap<String, Value>,
}

/// One edge: the state (`each/plan` for a nested one), the outcome, and the
/// index of the transition in that outcome's list.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EdgeKey {
    pub state: String,
    pub outcome: String,
    pub index: usize,
}

/// What the engine must do next.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Dispatch the action of `state` with the rendered input.
    Run {
        state: String,
        action: String,
        input: Value,
        level: Level,
        approval: Approval,
    },
    /// Park the run until `event` (or the timeout).
    Wait {
        state: String,
        event: String,
        timeout: Option<std::time::Duration>,
    },
    /// The run ended. `value` is the last `set` of a transform run.
    Finish {
        end: End,
        reason: Option<String>,
        value: Option<Value>,
    },
}

/// The transition to append to the run's log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub from: String,
    pub outcome: String,
    pub to: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub run: Run,
    pub record: Record,
    pub effect: Effect,
}

/// Misuse of the function, not a failure of the run: a failing expression
/// ends the run in `end.failed` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineError(pub String);

impl std::fmt::Display for MachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MachineError {}

/// Enters the start state.
pub fn start(definition: &Definition, context: &Context) -> Result<(Run, Effect), MachineError> {
    let mut run = Run {
        position: Position::At {
            state: definition.start.clone(),
            inner: None,
        },
        outputs: BTreeMap::new(),
        edge_uses: BTreeMap::new(),
        transitions: 0,
        last_set: None,
    };
    let effect = walk(definition, context, &mut run, Flow::Enter)?;
    Ok((run, effect))
}

/// The effect of the run's current position, without changing the run. Used
/// to pick up a run after a restart: entering the position renders its input
/// again. An ended run gives its `Finish` effect.
pub fn current_effect(
    definition: &Definition,
    context: &Context,
    run: &Run,
) -> Result<Effect, MachineError> {
    match &run.position {
        Position::Ended { end, reason } => Ok(Effect::Finish {
            end: *end,
            reason: reason.clone(),
            value: run.last_set.clone(),
        }),
        Position::At { .. } => walk(definition, context, &mut run.clone(), Flow::Enter),
    }
}

/// Applies the outcome of the current state and enters the next one.
pub fn next(
    definition: &Definition,
    context: &Context,
    run: &Run,
    outcome: &str,
    output: Value,
) -> Result<Step, MachineError> {
    let place = locate(definition, &run.position)?;
    if !accepts(&place, outcome) {
        return Err(MachineError(format!(
            "outcome `{outcome}` is not valid for state `{}`",
            place.path
        )));
    }
    let from = place.path.clone();
    let mut run = run.clone();
    let effect = walk(
        definition,
        context,
        &mut run,
        Flow::Outcome {
            outcome: outcome.to_owned(),
            output,
        },
    )?;
    let record = Record {
        from,
        outcome: outcome.to_owned(),
        to: describe(&run.position),
    };
    Ok(Step {
        run,
        record,
        effect,
    })
}

/// What the walk does next: enter the state at the position, conclude an
/// outcome of the state at the position, or stop with an effect.
enum Flow {
    Enter,
    Outcome { outcome: String, output: Value },
    Effect(Effect),
}

/// Where the transition of an outcome goes.
enum Route {
    /// A state of the same level (top level or the enclosing `for_each`).
    Goto(String),
    /// The run leaves this level with `end` (and a reason at top level).
    End(End, Option<String>),
}

/// The state a run is at, resolved against the definition.
struct Place<'d> {
    /// `id`, or `each/id` inside a `for_each`.
    path: String,
    state: &'d State,
    /// The states of the level the state sits in.
    siblings: &'d [State],
    /// Inside a `for_each`: its id and the name of its item.
    each: Option<(&'d str, &'d str)>,
}

/// Runs the walk from `flow` until it yields an effect. Every loop either
/// stops or counts a transition, so the cap bounds it.
fn walk(
    definition: &Definition,
    context: &Context,
    run: &mut Run,
    mut flow: Flow,
) -> Result<Effect, MachineError> {
    loop {
        flow = match flow {
            Flow::Enter => enter(definition, context, run)?,
            Flow::Outcome { outcome, output } => {
                conclude(definition, context, run, &outcome, output)?
            }
            Flow::Effect(effect) => return Ok(effect),
        };
    }
}

/// Enters the state at the position: renders its input, or starts a
/// `for_each` over its items.
fn enter(definition: &Definition, context: &Context, run: &mut Run) -> Result<Flow, MachineError> {
    let place = locate(definition, &run.position)?;
    let state = place.state;
    match &state.kind {
        StateKind::Action {
            action,
            with,
            approval,
        } => {
            let rendered = {
                let outputs = visible_outputs(run);
                let item = current_item(run, place.each.map(|(_, name)| name));
                expr::render_value(with, &scope_of(context, &outputs, item))
            };
            Ok(Flow::Effect(match rendered {
                Ok(input) => Effect::Run {
                    state: place.path,
                    action: action.clone(),
                    input,
                    level: level_of(action),
                    approval: *approval,
                },
                Err(error) => finish(run, End::Failed, Some(error.to_string())),
            }))
        }
        StateKind::Wait { event, timeout } => Ok(Flow::Effect(Effect::Wait {
            state: place.path,
            event: event.clone(),
            timeout: *timeout,
        })),
        StateKind::ForEach { items, states, .. } => {
            if place.each.is_some() {
                return Err(MachineError(format!(
                    "`{}` nests a for_each, only one level is allowed",
                    place.path
                )));
            }
            let rendered = {
                let outputs = visible_outputs(run);
                expr::render(items, &scope_of(context, &outputs, None))
            };
            let list = match rendered {
                Ok(Value::Array(list)) => list,
                Ok(_) => {
                    let reason = format!("`{}` items is not a list", place.path);
                    return Ok(Flow::Effect(finish(run, End::Failed, Some(reason))));
                }
                Err(error) => {
                    return Ok(Flow::Effect(finish(
                        run,
                        End::Failed,
                        Some(error.to_string()),
                    )));
                }
            };
            let Some(first) = states.first() else {
                let reason = format!("`{}` has no nested states", place.path);
                return Ok(Flow::Effect(finish(run, End::Failed, Some(reason))));
            };
            if list.is_empty() {
                return Ok(Flow::Outcome {
                    outcome: "done".to_owned(),
                    output: json!({"items": 0}),
                });
            }
            run.position = Position::At {
                state: state.id.clone(),
                inner: Some(Inner {
                    index: 0,
                    items: list,
                    state: first.id.clone(),
                    outputs: BTreeMap::new(),
                }),
            };
            Ok(Flow::Enter)
        }
    }
}

/// Stores the output of the state at the position, then takes the transition
/// of `outcome`.
fn conclude(
    definition: &Definition,
    context: &Context,
    run: &mut Run,
    outcome: &str,
    output: Value,
) -> Result<Flow, MachineError> {
    let place = locate(definition, &run.position)?;
    if let StateKind::Action { action, .. } = &place.state.kind
        && action == "set"
    {
        run.last_set = Some(
            output
                .get("value")
                .cloned()
                .unwrap_or_else(|| output.clone()),
        );
    }
    let id = place.state.id.clone();
    match &mut run.position {
        Position::At {
            inner: Some(inner), ..
        } => {
            inner.outputs.insert(id, output);
        }
        _ => {
            run.outputs.insert(id, output);
        }
    }

    run.transitions += 1;
    if run.transitions > MAX_TRANSITIONS {
        return Ok(Flow::Effect(finish(
            run,
            End::Failed,
            Some("transition cap reached".to_owned()),
        )));
    }

    match route(context, run, &place, outcome) {
        Route::Goto(id) => {
            retarget(&mut run.position, id);
            Ok(Flow::Enter)
        }
        Route::End(end, reason) => match place.each {
            None => Ok(Flow::Effect(finish(run, end, reason))),
            Some((owner, _)) => leave_each(definition, run, owner, end),
        },
    }
}

/// Moves to another state of the same level.
fn retarget(position: &mut Position, id: String) {
    if let Position::At { state, inner } = position {
        match inner {
            Some(inner) => inner.state = id,
            None => *state = id,
        }
    }
}

/// A nested state ended. `done` moves to the next item, or completes the
/// `for_each` as `done`; `failed` and `blocked` complete it as `failed`.
fn leave_each(
    definition: &Definition,
    run: &mut Run,
    owner: &str,
    end: End,
) -> Result<Flow, MachineError> {
    let Position::At {
        inner: Some(inner), ..
    } = &mut run.position
    else {
        return Err(MachineError(
            "a nested state ended outside a for_each".to_owned(),
        ));
    };
    if end != End::Done {
        let index = inner.index;
        run.position = Position::At {
            state: owner.to_owned(),
            inner: None,
        };
        return Ok(Flow::Outcome {
            outcome: "failed".to_owned(),
            output: json!({"index": index}),
        });
    }
    if inner.index + 1 < inner.items.len() {
        inner.index += 1;
        inner.outputs.clear();
        inner.state = first_nested(definition, owner)?;
        return Ok(Flow::Enter);
    }
    let count = inner.items.len();
    run.position = Position::At {
        state: owner.to_owned(),
        inner: None,
    };
    Ok(Flow::Outcome {
        outcome: "done".to_owned(),
        output: json!({"items": count}),
    })
}

/// Picks the transition of `outcome` for the state at `place`. Records the
/// use of the edge it takes.
fn route(context: &Context, run: &mut Run, place: &Place<'_>, outcome: &str) -> Route {
    let Some(rule) = place.state.rule(outcome) else {
        return default_route(run, place, outcome);
    };
    let chosen = {
        let outputs = visible_outputs(run);
        let item = current_item(run, place.each.map(|(_, name)| name));
        let scope = scope_of(context, &outputs, item);
        let mut chosen = None;
        for (index, transition) in rule.transitions.iter().enumerate() {
            if let Some(limit) = transition.limit {
                let used = run
                    .edge_uses
                    .get(&edge(place, outcome, index))
                    .copied()
                    .unwrap_or(0);
                if used >= limit {
                    continue;
                }
            }
            if let Some(guard) = &transition.guard {
                match expr::eval_condition(guard, &scope) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(error) => return Route::End(End::Failed, Some(error.to_string())),
                }
            }
            chosen = Some((index, transition));
            break;
        }
        chosen
    };
    let Some((index, transition)) = chosen else {
        return Route::End(
            End::Failed,
            Some(format!("no transition for outcome `{outcome}`")),
        );
    };
    *run.edge_uses
        .entry(edge(place, outcome, index))
        .or_insert(0) += 1;
    match &transition.to {
        Target::State(id) => Route::Goto(id.clone()),
        Target::End(end) => match (&transition.reason, place.each) {
            (Some(template), None) => match render_reason(template, context, run) {
                Ok(text) => Route::End(*end, Some(text)),
                Err(error) => Route::End(End::Failed, Some(error.to_string())),
            },
            _ => Route::End(*end, None),
        },
    }
}

/// The transition of an outcome the rule does not list (D78's short form).
fn default_route(run: &Run, place: &Place<'_>, outcome: &str) -> Route {
    match outcome {
        "done" => match next_sibling(place) {
            Some(id) => Route::Goto(id.to_owned()),
            None => Route::End(End::Done, None),
        },
        "failed" | "interrupted" | "timeout" => {
            Route::End(End::Failed, Some(failure_reason(run, place, outcome)))
        }
        other => Route::End(
            End::Failed,
            Some(format!("no transition for outcome `{other}`")),
        ),
    }
}

/// Why a state's failure ended the run: the `error` or the `stderr` its
/// output carries (an `exec` that could not start, say), else the outcome.
fn failure_reason(run: &Run, place: &Place<'_>, outcome: &str) -> String {
    let id = place.state.id.as_str();
    let output = visible_outputs(run).get(id).cloned();
    let text = |key: &str| {
        output
            .as_ref()
            .and_then(|value| value.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let detail = text("error").or_else(|| text("stderr"));
    match detail {
        Some(detail) => format!("`{id}` {outcome}: {detail}"),
        None => format!("`{id}` {outcome}"),
    }
}

fn next_sibling<'d>(place: &Place<'d>) -> Option<&'d str> {
    let index = place
        .siblings
        .iter()
        .position(|state| state.id == place.state.id)?;
    place.siblings.get(index + 1).map(|state| state.id.as_str())
}

fn edge(place: &Place<'_>, outcome: &str, index: usize) -> EdgeKey {
    EdgeKey {
        state: place.path.clone(),
        outcome: outcome.to_owned(),
        index,
    }
}

/// Resolves the state at the position against the definition.
fn locate<'d>(definition: &'d Definition, position: &Position) -> Result<Place<'d>, MachineError> {
    let Position::At { state: id, inner } = position else {
        return Err(MachineError("the run has ended".to_owned()));
    };
    let top = definition.state(id).ok_or_else(|| unknown_state(id))?;
    let Some(inner) = inner else {
        return Ok(Place {
            path: id.clone(),
            state: top,
            siblings: &definition.states,
            each: None,
        });
    };
    let StateKind::ForEach { item, states, .. } = &top.kind else {
        return Err(MachineError(format!("`{id}` is not a for_each state")));
    };
    let state = states
        .iter()
        .find(|nested| nested.id == inner.state)
        .ok_or_else(|| unknown_state(&inner.state))?;
    Ok(Place {
        path: format!("{id}/{}", inner.state),
        state,
        siblings: states,
        each: Some((top.id.as_str(), item.as_str())),
    })
}

fn first_nested(definition: &Definition, owner: &str) -> Result<String, MachineError> {
    match definition.state(owner).map(|state| &state.kind) {
        Some(StateKind::ForEach { states, .. }) => states
            .first()
            .map(|state| state.id.clone())
            .ok_or_else(|| MachineError(format!("for_each `{owner}` has no states"))),
        _ => Err(MachineError(format!("`{owner}` is not a for_each state"))),
    }
}

fn unknown_state(id: &str) -> MachineError {
    MachineError(format!("unknown state `{id}`"))
}

/// Whether the state at `place` can end with `outcome`.
fn accepts(place: &Place<'_>, outcome: &str) -> bool {
    match &place.state.kind {
        StateKind::Action { action, .. } => {
            catalog::action(action).is_some_and(|spec| spec.outcomes.contains(&outcome))
        }
        StateKind::Wait { event, timeout } => {
            catalog::wait_outcomes(event).is_some_and(|outcomes| outcomes.contains(&outcome))
                || outcome == "failed"
                || (outcome == "timeout" && timeout.is_some())
        }
        StateKind::ForEach { .. } => false,
    }
}

fn level_of(action: &str) -> Level {
    catalog::action(action).map_or(Level::Read, |spec| spec.level)
}

/// The outputs an expression sees: the top level, overlaid by the nested
/// outputs of the current item. Borrowed unless an overlay is needed.
fn visible_outputs(run: &Run) -> Cow<'_, BTreeMap<String, Value>> {
    match &run.position {
        Position::At {
            inner: Some(inner), ..
        } if !inner.outputs.is_empty() => {
            let mut merged = run.outputs.clone();
            merged.extend(inner.outputs.clone());
            Cow::Owned(merged)
        }
        _ => Cow::Borrowed(&run.outputs),
    }
}

/// The current `for_each` item, under the item's name.
fn current_item<'a>(run: &'a Run, name: Option<&'a str>) -> Option<(&'a str, &'a Value)> {
    match (&run.position, name) {
        (
            Position::At {
                inner: Some(inner), ..
            },
            Some(name),
        ) => inner.items.get(inner.index).map(|item| (name, item)),
        _ => None,
    }
}

fn scope_of<'a>(
    context: &'a Context,
    outputs: &'a BTreeMap<String, Value>,
    item: Option<(&'a str, &'a Value)>,
) -> Scope<'a> {
    Scope {
        inputs: &context.inputs,
        trigger: &context.trigger,
        repo: &context.repo,
        states: outputs,
        item,
    }
}

/// Renders a transition reason at top level. A non-string value is written
/// in its JSON form.
fn render_reason(template: &str, context: &Context, run: &Run) -> Result<String, ExprError> {
    let outputs = visible_outputs(run);
    let text = match expr::render(template, &scope_of(context, &outputs, None))? {
        Value::String(text) => text,
        other => other.to_string(),
    };
    Ok(text)
}

/// Ends the run and returns the effect that tells the engine so.
fn finish(run: &mut Run, end: End, reason: Option<String>) -> Effect {
    run.position = Position::Ended {
        end,
        reason: reason.clone(),
    };
    Effect::Finish {
        end,
        reason,
        value: run.last_set.clone(),
    }
}

/// The position as the transition log names it.
fn describe(position: &Position) -> String {
    match position {
        Position::At { state, inner: None } => state.clone(),
        Position::At {
            state,
            inner: Some(inner),
        } => format!("{state}/{}", inner.state),
        Position::Ended { end, .. } => end_name(*end).to_owned(),
    }
}

fn end_name(end: End) -> &'static str {
    match end {
        End::Done => "end.done",
        End::Failed => "end.failed",
        End::Blocked => "end.blocked",
    }
}

/// `edge_uses` as a list of `(edge, uses)` pairs: JSON maps need string keys.
mod edge_uses {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::EdgeKey;

    pub fn serialize<S: Serializer>(map: &BTreeMap<EdgeKey, u32>, s: S) -> Result<S::Ok, S::Error> {
        map.iter().collect::<Vec<_>>().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<EdgeKey, u32>, D::Error> {
        Ok(Vec::<(EdgeKey, u32)>::deserialize(d)?.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use proptest::prelude::*;

    use super::super::parse::parse;
    use super::*;

    const RESTACK_CHILDREN: &str =
        include_str!("../../tests/workflows/restack-children-on-parent-move.toml");
    const SUGGEST_MESSAGE: &str = include_str!("../../tests/workflows/suggest-commit-message.toml");
    const BLOCK_RESTACK: &str =
        include_str!("../../tests/workflows/block-restack-on-protected.toml");
    const RESTACK_STALE: &str =
        include_str!("../../tests/workflows/restack-stale-stacks-after-fetch.toml");
    const COMMIT_LLM: &str = include_str!("../../tests/workflows/commit-message-with-llm.toml");

    fn load(source: &str) -> Definition {
        parse(source).unwrap_or_else(|diagnostic| panic!("fixture does not parse: {diagnostic:?}"))
    }

    fn context(trigger: Value) -> Context {
        Context {
            inputs: json!({}),
            trigger,
            repo: json!({"staged_diff": "diff --git a/x b/x"}),
        }
    }

    fn run_effect(
        state: &str,
        action: &str,
        input: Value,
        level: Level,
        approval: Approval,
    ) -> Effect {
        Effect::Run {
            state: state.to_owned(),
            action: action.to_owned(),
            input,
            level,
            approval,
        }
    }

    fn finished(end: End, reason: Option<&str>, value: Option<Value>) -> Effect {
        Effect::Finish {
            end,
            reason: reason.map(str::to_owned),
            value,
        }
    }

    /// The state an effect is about, for `Run` and `Wait`.
    fn state_of(effect: &Effect) -> Option<&str> {
        match effect {
            Effect::Run { state, .. } | Effect::Wait { state, .. } => Some(state),
            Effect::Finish { .. } => None,
        }
    }

    #[test]
    fn restack_children_plans_applies_and_notifies() {
        let def = load(RESTACK_CHILDREN);
        let ctx = context(json!({"parent": "main"}));
        let (run, effect) = start(&def, &ctx).unwrap();
        assert_eq!(
            effect,
            run_effect(
                "plan",
                "restack.plan",
                json!({"root": "main"}),
                Level::Propose,
                Approval::Ask,
            )
        );

        let step = next(
            &def,
            &ctx,
            &run,
            "done",
            json!({"branch_count": 2, "plan_id": "p1"}),
        )
        .unwrap();
        assert_eq!(
            step.effect,
            run_effect(
                "apply",
                "restack.apply",
                json!({"plan_id": "p1"}),
                Level::Apply,
                Approval::Auto,
            )
        );
        assert_eq!(
            step.record,
            Record {
                from: "plan".into(),
                outcome: "done".into(),
                to: "apply".into(),
            }
        );

        let step = next(&def, &ctx, &step.run, "done", json!({})).unwrap();
        assert_eq!(
            step.effect,
            run_effect(
                "notify",
                "notify",
                json!({"text": "Restacked 2 branches on main"}),
                Level::Read,
                Approval::Ask,
            )
        );
        let step = next(&def, &ctx, &step.run, "done", json!({})).unwrap();
        assert_eq!(step.effect, finished(End::Done, None, None));
        assert_eq!(step.record.to, "end.done");
    }

    #[test]
    fn restack_children_stale_replans_until_the_limit() {
        let def = load(RESTACK_CHILDREN);
        let ctx = context(json!({"parent": "main"}));
        let (run, _) = start(&def, &ctx).unwrap();
        let step = next(
            &def,
            &ctx,
            &run,
            "done",
            json!({"branch_count": 2, "plan_id": "p1"}),
        )
        .unwrap();
        let mut run = step.run;

        for _ in 0..3 {
            let stale = next(&def, &ctx, &run, "stale", json!({})).unwrap();
            assert_eq!(state_of(&stale.effect), Some("plan"));
            let replanned = next(
                &def,
                &ctx,
                &stale.run,
                "done",
                json!({"branch_count": 2, "plan_id": "p1"}),
            )
            .unwrap();
            assert_eq!(state_of(&replanned.effect), Some("apply"));
            run = replanned.run;
        }

        let exhausted = next(&def, &ctx, &run, "stale", json!({})).unwrap();
        assert_eq!(
            exhausted.effect,
            finished(End::Failed, Some("no transition for outcome `stale`"), None)
        );
    }

    #[test]
    fn restack_children_conflict_waits_then_notifies() {
        let def = load(RESTACK_CHILDREN);
        let ctx = context(json!({"parent": "main"}));
        let (run, _) = start(&def, &ctx).unwrap();
        let applied = next(
            &def,
            &ctx,
            &run,
            "done",
            json!({"branch_count": 2, "plan_id": "p1"}),
        )
        .unwrap();

        let conflict = next(&def, &ctx, &applied.run, "conflict", json!({})).unwrap();
        assert_eq!(
            conflict.effect,
            Effect::Wait {
                state: "wait_for_resolution".into(),
                event: "restack.resumed".into(),
                timeout: Some(Duration::from_hours(24)),
            }
        );

        let resumed = next(&def, &ctx, &conflict.run, "resumed", json!({})).unwrap();
        assert_eq!(state_of(&resumed.effect), Some("notify"));
        assert_eq!(resumed.record.from, "wait_for_resolution");
    }

    #[test]
    fn restack_children_without_branches_ends_done_after_plan() {
        let def = load(RESTACK_CHILDREN);
        let ctx = context(json!({"parent": "main"}));
        let (run, _) = start(&def, &ctx).unwrap();
        let step = next(
            &def,
            &ctx,
            &run,
            "done",
            json!({"branch_count": 0, "plan_id": "p0"}),
        )
        .unwrap();
        assert_eq!(step.effect, finished(End::Done, None, None));
        assert_eq!(
            step.record,
            Record {
                from: "plan".into(),
                outcome: "done".into(),
                to: "end.done".into(),
            }
        );
    }

    #[test]
    fn restack_children_missing_trigger_ends_failed() {
        let def = load(RESTACK_CHILDREN);
        let (_, effect) = start(&def, &context(json!({}))).unwrap();
        assert!(matches!(
            effect,
            Effect::Finish {
                end: End::Failed,
                reason: Some(_),
                value: None,
            }
        ));
    }

    #[test]
    fn suggest_retries_empty_text_twice_then_suggests() {
        let def = load(SUGGEST_MESSAGE);
        let ctx = context(json!({}));
        let (mut run, effect) = start(&def, &ctx).unwrap();
        assert_eq!(state_of(&effect), Some("ask"));

        for _ in 0..2 {
            let step = next(&def, &ctx, &run, "done", json!({"text": ""})).unwrap();
            assert_eq!(state_of(&step.effect), Some("ask"));
            run = step.run;
        }
        let step = next(&def, &ctx, &run, "done", json!({"text": ""})).unwrap();
        assert_eq!(
            step.effect,
            run_effect(
                "suggest",
                "commit.suggest_message",
                json!({"message": ""}),
                Level::Propose,
                Approval::Ask,
            )
        );
    }

    #[test]
    fn suggest_renders_the_generated_text_into_the_message() {
        let def = load(SUGGEST_MESSAGE);
        let ctx = context(json!({}));
        let (run, _) = start(&def, &ctx).unwrap();
        let retry = next(&def, &ctx, &run, "done", json!({"text": ""})).unwrap();
        let step = next(
            &def,
            &ctx,
            &retry.run,
            "done",
            json!({"text": "feat: add parser"}),
        )
        .unwrap();
        assert_eq!(
            step.effect,
            run_effect(
                "suggest",
                "commit.suggest_message",
                json!({"message": "feat: add parser"}),
                Level::Propose,
                Approval::Ask,
            )
        );
    }

    #[test]
    fn block_restack_blocks_a_protected_root() {
        let def = load(BLOCK_RESTACK);
        let ctx = context(json!({"root": "main"}));
        let (run, effect) = start(&def, &ctx).unwrap();
        assert_eq!(
            effect,
            run_effect("check", "noop", json!({}), Level::Read, Approval::Ask)
        );
        let step = next(&def, &ctx, &run, "done", json!({})).unwrap();
        assert_eq!(
            step.effect,
            finished(End::Blocked, Some("Restack on main is blocked"), None)
        );
        assert_eq!(step.record.to, "end.blocked");
    }

    #[test]
    fn block_restack_lets_a_feature_root_through() {
        let def = load(BLOCK_RESTACK);
        let ctx = context(json!({"root": "feat"}));
        let (run, _) = start(&def, &ctx).unwrap();
        let step = next(&def, &ctx, &run, "done", json!({})).unwrap();
        assert_eq!(step.effect, finished(End::Done, None, None));
    }

    #[test]
    fn stale_stacks_restack_each_root_in_order() {
        let def = load(RESTACK_STALE);
        let ctx = context(json!({}));
        let (run, _) = start(&def, &ctx).unwrap();
        let step = next(&def, &ctx, &run, "done", json!({"roots": ["a", "b"]})).unwrap();
        assert_eq!(
            step.effect,
            run_effect(
                "each/plan",
                "restack.plan",
                json!({"root": "a"}),
                Level::Propose,
                Approval::Ask,
            )
        );
        assert_eq!(step.record.to, "each/plan");

        let step = next(
            &def,
            &ctx,
            &step.run,
            "done",
            json!({"branch_count": 1, "plan_id": "pa"}),
        )
        .unwrap();
        assert_eq!(
            step.effect,
            run_effect(
                "each/apply",
                "restack.apply",
                json!({"plan_id": "pa"}),
                Level::Apply,
                Approval::Ask,
            )
        );
        let step = next(&def, &ctx, &step.run, "done", json!({})).unwrap();
        assert_eq!(
            step.effect,
            run_effect(
                "each/plan",
                "restack.plan",
                json!({"root": "b"}),
                Level::Propose,
                Approval::Ask,
            )
        );
        let step = next(
            &def,
            &ctx,
            &step.run,
            "done",
            json!({"branch_count": 1, "plan_id": "pb"}),
        )
        .unwrap();
        assert_eq!(state_of(&step.effect), Some("each/apply"));
        let step = next(&def, &ctx, &step.run, "done", json!({})).unwrap();
        assert_eq!(step.effect, finished(End::Done, None, None));
        assert_eq!(step.record.from, "each/apply");
        assert_eq!(step.record.to, "end.done");
    }

    #[test]
    fn stale_stacks_with_no_roots_end_done() {
        let def = load(RESTACK_STALE);
        let ctx = context(json!({}));
        let (run, _) = start(&def, &ctx).unwrap();
        let step = next(&def, &ctx, &run, "done", json!({"roots": []})).unwrap();
        assert_eq!(step.effect, finished(End::Done, None, None));
        assert_eq!(step.record.from, "roots");
        assert_eq!(step.record.to, "end.done");
    }

    #[test]
    fn stale_stacks_fail_when_a_nested_plan_fails() {
        let def = load(RESTACK_STALE);
        let ctx = context(json!({}));
        let (run, _) = start(&def, &ctx).unwrap();
        let step = next(&def, &ctx, &run, "done", json!({"roots": ["a"]})).unwrap();
        let step = next(&def, &ctx, &step.run, "failed", json!({})).unwrap();
        assert_eq!(
            step.effect,
            finished(End::Failed, Some("`each` failed"), None)
        );
        assert_eq!(step.record.from, "each/plan");
        assert_eq!(step.record.to, "end.failed");
    }

    #[test]
    fn stale_stacks_nested_conflict_waits_for_that_item() {
        let def = load(RESTACK_STALE);
        let ctx = context(json!({}));
        let (run, _) = start(&def, &ctx).unwrap();
        let step = next(&def, &ctx, &run, "done", json!({"roots": ["a"]})).unwrap();
        let step = next(
            &def,
            &ctx,
            &step.run,
            "done",
            json!({"branch_count": 1, "plan_id": "pa"}),
        )
        .unwrap();
        let step = next(&def, &ctx, &step.run, "conflict", json!({})).unwrap();
        assert_eq!(
            step.effect,
            Effect::Wait {
                state: "each/wait_conflict".into(),
                event: "restack.resumed".into(),
                timeout: Some(Duration::from_hours(24)),
            }
        );
    }

    #[test]
    fn commit_message_set_value_comes_back_in_finish() {
        let def = load(COMMIT_LLM);
        let ctx = context(json!({}));
        let (run, effect) = start(&def, &ctx).unwrap();
        assert_eq!(state_of(&effect), Some("llm"));

        let step = next(&def, &ctx, &run, "done", json!({"text": "feat: x"})).unwrap();
        assert_eq!(
            step.effect,
            run_effect(
                "out",
                "set",
                json!({"value": "feat: x"}),
                Level::Propose,
                Approval::Ask,
            )
        );

        let step = next(&def, &ctx, &step.run, "done", json!({"value": "feat: x"})).unwrap();
        assert_eq!(
            step.effect,
            finished(End::Done, None, Some(json!("feat: x")))
        );
    }

    #[test]
    fn current_effect_re_enters_the_position_without_changing_the_run() {
        let def = load(RESTACK_CHILDREN);
        let ctx = context(json!({"parent": "main"}));
        let (run, effect) = start(&def, &ctx).unwrap();
        let before = run.clone();
        assert_eq!(current_effect(&def, &ctx, &run).unwrap(), effect);
        assert_eq!(run, before);

        let step = next(
            &def,
            &ctx,
            &run,
            "done",
            json!({"branch_count": 2, "plan_id": "p1"}),
        )
        .unwrap();
        assert_eq!(current_effect(&def, &ctx, &step.run).unwrap(), step.effect);
    }

    #[test]
    fn current_effect_of_an_ended_run_is_its_finish() {
        let def = load(BLOCK_RESTACK);
        let ctx = context(json!({"root": "feat"}));
        let (run, _) = start(&def, &ctx).unwrap();
        let ended = next(&def, &ctx, &run, "done", json!({})).unwrap();
        assert_eq!(
            current_effect(&def, &ctx, &ended.run).unwrap(),
            finished(End::Done, None, None)
        );
    }

    #[test]
    fn next_after_the_end_is_a_machine_error() {
        let def = load(BLOCK_RESTACK);
        let ctx = context(json!({"root": "feat"}));
        let (run, _) = start(&def, &ctx).unwrap();
        let ended = next(&def, &ctx, &run, "done", json!({})).unwrap();
        assert!(next(&def, &ctx, &ended.run, "done", json!({})).is_err());
    }

    #[test]
    fn unknown_outcome_is_a_machine_error() {
        let def = load(RESTACK_CHILDREN);
        let ctx = context(json!({"parent": "main"}));
        let (run, _) = start(&def, &ctx).unwrap();
        // `plan` declares `done` and `failed` only.
        assert!(next(&def, &ctx, &run, "stale", json!({})).is_err());
    }

    #[test]
    fn transition_cap_ends_the_run_failed() {
        let def = load(RESTACK_CHILDREN);
        let ctx = context(json!({"parent": "main"}));
        let (mut run, _) = start(&def, &ctx).unwrap();
        run.transitions = MAX_TRANSITIONS;
        let step = next(
            &def,
            &ctx,
            &run,
            "done",
            json!({"branch_count": 2, "plan_id": "p1"}),
        )
        .unwrap();
        assert_eq!(
            step.effect,
            finished(End::Failed, Some("transition cap reached"), None)
        );
    }

    /// One pick of a random walk: an index into the current outcome set, a
    /// branch count and a plan id.
    type Choice = (usize, u32, String);

    fn choice() -> impl Strategy<Value = Choice> {
        (any::<usize>(), 0_u32..4, "[a-z0-9]{1,6}")
    }

    /// The outcomes the current state can end with.
    fn outcome_set(definition: &Definition, run: &Run) -> Vec<&'static str> {
        let Ok(place) = locate(definition, &run.position) else {
            return Vec::new();
        };
        match &place.state.kind {
            StateKind::Action { action, .. } => {
                catalog::action(action).map_or_else(Vec::new, |spec| spec.outcomes.to_vec())
            }
            StateKind::Wait { event, timeout } => {
                let mut outcomes =
                    catalog::wait_outcomes(event).map_or_else(Vec::new, <[&str]>::to_vec);
                outcomes.push("failed");
                if timeout.is_some() {
                    outcomes.push("timeout");
                }
                outcomes
            }
            StateKind::ForEach { .. } => Vec::new(),
        }
    }

    /// Walks the run with picks from the outcome sets. Returns the effects,
    /// the final run and the number of calls made.
    fn play(
        definition: &Definition,
        context: &Context,
        choices: &[Choice],
    ) -> (Vec<Effect>, Run, usize) {
        let (mut run, effect) = start(definition, context).unwrap();
        let mut effects = vec![effect];
        let mut calls = 0;
        for (pick, count, plan_id) in choices {
            if matches!(run.position, Position::Ended { .. }) {
                break;
            }
            let outcomes = outcome_set(definition, &run);
            if outcomes.is_empty() {
                break;
            }
            let outcome = outcomes[*pick % outcomes.len()];
            let output = json!({"branch_count": count, "plan_id": plan_id});
            let step = next(definition, context, &run, outcome, output).unwrap();
            calls += 1;
            effects.push(step.effect);
            run = step.run;
        }
        (effects, run, calls)
    }

    proptest! {
        #[test]
        fn random_outcomes_end_within_fifty_calls(
            choices in proptest::collection::vec(choice(), 50)
        ) {
            let def = load(RESTACK_CHILDREN);
            let ctx = context(json!({"parent": "main"}));
            let (_, run, calls) = play(&def, &ctx, &choices);
            let ended = matches!(run.position, Position::Ended { .. });
            prop_assert!(ended);
            prop_assert!(calls <= 50);
            prop_assert!(run.transitions <= MAX_TRANSITIONS);
        }

        #[test]
        fn same_outcomes_give_the_same_effects(
            choices in proptest::collection::vec(choice(), 50)
        ) {
            let def = load(RESTACK_CHILDREN);
            let ctx = context(json!({"parent": "main"}));
            let (effects_a, run_a, _) = play(&def, &ctx, &choices);
            let (effects_b, run_b, _) = play(&def, &ctx, &choices);
            prop_assert_eq!(effects_a, effects_b);
            prop_assert_eq!(run_a, run_b);
        }
    }
}
