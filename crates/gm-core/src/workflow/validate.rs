//! The graph and reference rules of a parsed workflow (design/workflow-scripts.md, D63, D78).
//! Errors keep the workflow from running; warnings are shown on the page.

use std::collections::VecDeque;

use serde_json::Value;

use super::Diagnostic;
use super::catalog::{self, DEFAULTED, FieldKind, FieldSpec, Level};
use super::expr;
use super::model::{Approval, Definition, End, State, StateKind, Target, Transition};

/// The roots a template may start with outside a `for_each` item (design/workflow-scripts.md).
const ROOTS: &[&str] = &["inputs", "trigger", "repo", "states"];

/// Every problem in the definition, errors and warnings, ordered by line.
pub fn validate(definition: &Definition) -> Vec<Diagnostic> {
    let mut checker = Checker {
        definition,
        top: Graph::new(&definition.states),
        diagnostics: Vec::new(),
    };
    checker.check_file();
    checker.check_scope(Scope {
        states: &definition.states,
        body: None,
    });
    for state in &definition.states {
        if let StateKind::ForEach { states, item, .. } = &state.kind {
            let body = Body {
                item,
                parent: &state.id,
            };
            checker.check_scope(Scope {
                states,
                body: Some(body),
            });
        }
    }
    let mut diagnostics = checker.diagnostics;
    diagnostics.sort_by_key(|diagnostic| diagnostic.line);
    diagnostics
}

/// One state list: the top level, or the body of one `for_each`.
#[derive(Clone, Copy)]
struct Scope<'a> {
    states: &'a [State],
    body: Option<Body<'a>>,
}

/// The `for_each` a nested scope belongs to.
#[derive(Clone, Copy)]
struct Body<'a> {
    /// The item name, as expressions read it.
    item: &'a str,
    /// The id of the enclosing `for_each` state.
    parent: &'a str,
}

/// A `states.<id>.output` read found in a template.
struct StateRead {
    id: String,
    /// A nested state reads a top-level output.
    from_top: bool,
}

struct Checker<'a> {
    definition: &'a Definition,
    /// The top-level graph, read when a nested state reads a top-level output.
    top: Graph,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Checker<'a> {
    fn error(&mut self, line: usize, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(line, message));
    }

    fn warning(&mut self, line: usize, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::warning(line, message));
    }

    /// File-level rules: the start state, the trigger and `set` in transforms.
    fn check_file(&mut self) {
        let definition = self.definition;
        if definition.state(&definition.start).is_none() {
            self.error(
                0,
                format!("start `{}` is not a top-level state", definition.start),
            );
        }
        if !catalog::is_known_trigger(&definition.trigger.on) {
            self.error(0, format!("unknown trigger `{}`", definition.trigger.on));
        }
        if definition.trigger.is_transform() && !has_action(&definition.states, "set") {
            self.warning(0, "transform workflow has no `set` state");
        }
    }

    fn check_scope(&mut self, scope: Scope<'a>) {
        self.check_duplicates(scope);
        let mut reads = Vec::with_capacity(scope.states.len());
        for state in scope.states {
            reads.push(self.check_state(scope, state));
        }
        let graph = Graph::new(scope.states);
        self.check_reachable(scope, &graph);
        self.check_ends(scope, &graph);
        self.check_cycles(scope, &graph);
        self.check_reads(scope, &graph, &reads);
    }

    fn check_duplicates(&mut self, scope: Scope<'a>) {
        let mut seen: Vec<&str> = Vec::new();
        for state in scope.states {
            let id = state.id.as_str();
            if seen.contains(&id) {
                self.error(state.line, format!("duplicate {} `{id}`", noun(scope)));
            }
            seen.push(id);
            if scope.body.is_some() && self.definition.state(id).is_some() {
                self.error(
                    state.line,
                    format!("nested state `{id}` has the same id as a top-level state"),
                );
            }
        }
    }

    /// Rules on one state, then the reads in its templates it returns.
    fn check_state(&mut self, scope: Scope<'a>, state: &'a State) -> Vec<StateRead> {
        let mut reads = Vec::new();
        self.check_kind(scope, state, &mut reads);
        self.check_rules(scope, state, &mut reads);
        reads
    }

    /// The rules of the kind: the action, the wait event, the item of a
    /// `for_each`, and the templates each one holds.
    fn check_kind(&mut self, scope: Scope<'a>, state: &'a State, reads: &mut Vec<StateRead>) {
        let id = state.id.as_str();
        let line = state.line;
        match &state.kind {
            StateKind::Action {
                action,
                with,
                approval,
            } => {
                self.check_action(state, action, *approval);
                self.check_with(state, action, with);
                let mut texts = Vec::new();
                strings(with, &mut texts);
                for text in texts {
                    self.check_template(scope, state, text, reads);
                }
            }
            StateKind::Wait { event, .. } => {
                if catalog::wait_outcomes(event).is_none() {
                    self.error(
                        line,
                        format!("state `{id}` waits on unknown event `{event}`"),
                    );
                }
            }
            StateKind::ForEach { items, item, .. } => {
                if scope.body.is_some() {
                    self.error(
                        line,
                        format!("state `{id}` is a `for_each` inside a `for_each`"),
                    );
                }
                if ROOTS.contains(&item.as_str()) {
                    self.error(
                        line,
                        format!("state `{id}` names its item `{item}`, a reserved root"),
                    );
                }
                self.check_template(scope, state, items, reads);
            }
        }
    }

    /// The catalog rules of an action: it exists, `approval = "auto"` only on
    /// an apply action, and its phase matches the trigger.
    fn check_action(&mut self, state: &State, action: &str, approval: Approval) {
        let id = state.id.as_str();
        let line = state.line;
        let Some(spec) = catalog::action(action) else {
            self.error(line, format!("state `{id}` uses unknown action `{action}`"));
            return;
        };
        if approval == Approval::Auto && spec.level != Level::Apply {
            self.warning(
                line,
                format!("`approval = \"auto\"` on state `{id}`, which is not an apply action"),
            );
        }
        let trigger = &self.definition.trigger.on;
        if let Some(phase) = spec
            .phase
            .filter(|phase| !trigger.starts_with(&format!("{phase}.")))
        {
            self.error(
                line,
                format!(
                    "state `{id}` uses action `{}`, which is only valid in a `{phase}.*` trigger",
                    spec.name
                ),
            );
        }
    }

    /// The `with` keys of an action against its catalog fields: an unknown key
    /// warns; a missing required key, or a value of the wrong type, is an
    /// error. `set` takes any table, and an action with no fields is not checked.
    fn check_with(&mut self, state: &State, action: &str, with: &Value) {
        let Some(spec) = catalog::action(action) else {
            return;
        };
        let Some(keys) = with.as_object() else {
            return;
        };
        if spec.fields.is_empty() || action == "set" {
            return;
        }
        let id = state.id.as_str();
        let line = state.line;
        for field in spec
            .fields
            .iter()
            .filter(|field| field.required && !keys.contains_key(field.name))
        {
            self.error(
                line,
                format!(
                    "state `{id}` is missing `with.{}`, which action `{action}` needs",
                    field.name
                ),
            );
        }
        for (key, value) in keys {
            match spec.fields.iter().find(|field| field.name == key.as_str()) {
                None => self.warning(
                    line,
                    format!("state `{id}`: `with.{key}` is not a field of action `{action}`"),
                ),
                Some(field) => {
                    if let Some(problem) = field_problem(field, value) {
                        self.error(line, format!("state `{id}`: `with.{key}` {problem}"));
                    }
                }
            }
        }
    }

    /// The outcome rules: each names an outcome the state can produce, every
    /// outcome that is not defaulted has one, and each transition is checked.
    fn check_rules(&mut self, scope: Scope<'a>, state: &'a State, reads: &mut Vec<StateRead>) {
        let id = state.id.as_str();
        let line = state.line;
        let outcomes = outcomes_of(&state.kind);
        for rule in &state.on {
            let cannot_produce = outcomes
                .as_ref()
                .is_some_and(|outcomes| !outcomes.iter().any(|outcome| *outcome == rule.outcome));
            if cannot_produce {
                self.error(
                    line,
                    format!(
                        "state `{id}` has a rule for `{}`, which it cannot produce",
                        rule.outcome
                    ),
                );
            }
            for transition in &rule.transitions {
                self.check_transition(scope, state, transition, reads);
            }
            let fallible_last = rule
                .transitions
                .last()
                .is_some_and(|last| last.guard.is_some() || last.limit.is_some());
            if fallible_last {
                self.warning(
                    line,
                    format!(
                        "the last transition of `on.{}` in state `{id}` has an `if` or a `limit`; the run ends failed when none applies",
                        rule.outcome
                    ),
                );
            }
        }
        if let Some(outcomes) = &outcomes {
            for outcome in outcomes {
                if !DEFAULTED.contains(outcome) && state.rule(outcome).is_none() {
                    self.error(line, format!("state `{id}` has no rule for `{outcome}`"));
                }
            }
        }
    }

    fn check_transition(
        &mut self,
        scope: Scope<'a>,
        state: &State,
        transition: &Transition,
        reads: &mut Vec<StateRead>,
    ) {
        let id = state.id.as_str();
        let line = state.line;
        match &transition.to {
            Target::State(target) => {
                if !scope.states.iter().any(|other| other.id == *target) {
                    self.error(
                        line,
                        format!("state `{id}` targets unknown state `{target}`"),
                    );
                }
            }
            Target::End(End::Blocked) => {
                if !self.definition.trigger.is_before() {
                    self.error(
                        line,
                        format!("state `{id}` targets `end.blocked` outside a `before.*` trigger"),
                    );
                }
            }
            Target::End(_) => {}
        }
        if let Some(guard) = &transition.guard {
            match expr::check_condition(guard) {
                Ok(()) => self.check_template(scope, state, guard, reads),
                Err(err) => self.error(
                    line,
                    format!("state `{id}` has an invalid condition: {err}"),
                ),
            }
        }
        if let Some(reason) = &transition.reason {
            self.check_template(scope, state, reason, reads);
        }
    }

    /// Syntax and references of one template string.
    fn check_template(
        &mut self,
        scope: Scope<'a>,
        state: &State,
        text: &str,
        reads: &mut Vec<StateRead>,
    ) {
        match expr::references(text) {
            Err(err) => self.error(
                state.line,
                format!("state `{}` has an invalid expression: {err}", state.id),
            ),
            Ok(paths) => {
                for path in paths {
                    if let Some(read) = self.check_path(scope, state, &path) {
                        reads.push(read);
                    }
                }
            }
        }
    }

    fn check_path(
        &mut self,
        scope: Scope<'a>,
        state: &State,
        path: &[String],
    ) -> Option<StateRead> {
        let id = state.id.as_str();
        let line = state.line;
        let shown = format!("${{{}}}", path.join("."));
        let root = path.first().map_or("", String::as_str);
        match root {
            "trigger" | "repo" => None,
            "inputs" => {
                let declared = path.get(1).is_some_and(|name| {
                    self.definition
                        .inputs
                        .iter()
                        .any(|input| input.name == *name)
                });
                if !declared {
                    self.error(
                        line,
                        format!("`{shown}` in state `{id}` is not a declared input"),
                    );
                }
                None
            }
            "states" => self.check_state_read(scope, state, path, &shown),
            _ if scope.body.is_some_and(|body| body.item == root) => None,
            _ => {
                self.error(
                    line,
                    format!("`{shown}` in state `{id}` has an unknown root `{root}`"),
                );
                None
            }
        }
    }

    /// `states.<name>.output`: the state must exist in this scope, or at the
    /// top level from inside a `for_each` (nested states win on a clash).
    fn check_state_read(
        &mut self,
        scope: Scope<'a>,
        state: &State,
        path: &[String],
        shown: &str,
    ) -> Option<StateRead> {
        let id = state.id.as_str();
        let line = state.line;
        let Some(name) = path.get(1) else {
            self.error(line, format!("`{shown}` in state `{id}` names no state"));
            return None;
        };
        let local = scope.states.iter().any(|other| other.id == *name);
        let from_top = !local && scope.body.is_some() && self.definition.state(name).is_some();
        if !local && !from_top {
            self.error(
                line,
                format!("`{shown}` in state `{id}` names no state `{name}`"),
            );
            return None;
        }
        if path.get(2).map(String::as_str) != Some("output") {
            self.error(
                line,
                format!("`{shown}` in state `{id}` must read `states.{name}.output`"),
            );
            return None;
        }
        Some(StateRead {
            id: name.clone(),
            from_top,
        })
    }

    fn check_reachable(&mut self, scope: Scope<'a>, graph: &Graph) {
        let start = match scope.body {
            None => scope
                .states
                .iter()
                .position(|state| state.id == self.definition.start),
            Some(_) => (!scope.states.is_empty()).then_some(0),
        };
        let Some(start) = start else {
            return;
        };
        let mut reached = graph.reach(start);
        reached[start] = true;
        let start_id = scope.states[start].id.clone();
        let noun = noun(scope);
        let unreached: Vec<(usize, String)> = scope
            .states
            .iter()
            .zip(reached)
            .filter(|(_, reached)| !reached)
            .map(|(state, _)| (state.line, state.id.clone()))
            .collect();
        for (line, id) in unreached {
            self.error(
                line,
                format!("{noun} `{id}` is not reachable from `{start_id}`"),
            );
        }
    }

    fn check_ends(&mut self, scope: Scope<'a>, graph: &Graph) {
        let noun = noun(scope);
        let stuck: Vec<(usize, String)> = scope
            .states
            .iter()
            .zip(graph.can_end())
            .filter(|(_, ends)| !ends)
            .map(|(state, _)| (state.line, state.id.clone()))
            .collect();
        for (line, id) in stuck {
            self.error(line, format!("{noun} `{id}` has no path to an end state"));
        }
    }

    /// Every cycle must hold an edge with a `limit`: drop those edges and
    /// report each cycle that remains, once.
    fn check_cycles(&mut self, scope: Scope<'a>, graph: &Graph) {
        let free = graph.without_limits();
        let count = scope.states.len();
        let reach: Vec<Vec<bool>> = (0..count).map(|from| free.reach(from)).collect();
        let mut covered = vec![false; count];
        for from in 0..count {
            if covered[from] || !reach[from][from] {
                continue;
            }
            for (other, flag) in covered.iter_mut().enumerate() {
                if reach[from][other] && reach[other][from] {
                    *flag = true;
                }
            }
            let names: Vec<String> = free
                .cycle_through(from)
                .iter()
                .map(|&index| format!("`{}`", scope.states[index].id))
                .collect();
            self.error(
                scope.states[from].line,
                format!("cycle {} has no `limit`", names.join(" → ")),
            );
        }
    }

    /// A read of another state's output needs a path from that state to the
    /// reader, in the same scope. Self-reads are allowed (retry guards).
    fn check_reads(&mut self, scope: Scope<'a>, graph: &Graph, reads: &[Vec<StateRead>]) {
        for (index, state) in scope.states.iter().enumerate() {
            for read in &reads[index] {
                if read.id == state.id {
                    continue;
                }
                let (reached, to) = match scope.body {
                    Some(body) if read.from_top => {
                        (self.reaches_top(&read.id, body.parent), body.parent)
                    }
                    _ => {
                        let from = scope.states.iter().position(|other| other.id == read.id);
                        (
                            from.is_some_and(|from| graph.reach(from)[index]),
                            state.id.as_str(),
                        )
                    }
                };
                if !reached {
                    self.warning(
                        state.line,
                        format!(
                            "`states.{}` is read in state `{}`, but no path leads from `{}` to `{to}`; `{}` may not have run",
                            read.id, state.id, read.id, read.id
                        ),
                    );
                }
            }
        }
    }

    /// Whether the top-level state `from` reaches the top-level state `to`.
    fn reaches_top(&self, from: &str, to: &str) -> bool {
        if from == to {
            return false;
        }
        let states = &self.definition.states;
        match (
            states.iter().position(|state| state.id == from),
            states.iter().position(|state| state.id == to),
        ) {
            (Some(from), Some(to)) => self.top.reach(from)[to],
            _ => false,
        }
    }
}

/// The outcomes a state can end with: the action's list; the wait event's,
/// with `failed` and `timeout` when the state sets one; `done` and `failed`
/// for a `for_each`. `None` when the action or the event is unknown.
fn outcomes_of(kind: &StateKind) -> Option<Vec<&'static str>> {
    match kind {
        StateKind::Action { action, .. } => {
            catalog::action(action).map(|spec| spec.outcomes.to_vec())
        }
        StateKind::Wait { event, timeout } => {
            let mut outcomes = catalog::wait_outcomes(event)?.to_vec();
            outcomes.push("failed");
            if timeout.is_some() {
                outcomes.push("timeout");
            }
            Some(outcomes)
        }
        StateKind::ForEach { .. } => Some(vec!["done", "failed"]),
    }
}

/// Why a `with` value does not fit its catalog field, or `None` when it does.
/// A string that holds a `${...}` template is only checked for its shape: the
/// run renders it later, so a duration or a choice is not known here.
#[must_use]
pub fn field_problem(field: &FieldSpec, value: &Value) -> Option<String> {
    let templated = value.as_str().is_some_and(|text| text.contains("${"));
    match field.kind {
        FieldKind::Any => None,
        FieldKind::Text if value.is_string() => None,
        FieldKind::Text => Some("must be a string".to_owned()),
        FieldKind::TextList
            if value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)) =>
        {
            None
        }
        FieldKind::TextList => Some("must be an array of strings".to_owned()),
        FieldKind::Bool if value.is_boolean() => None,
        FieldKind::Bool => Some("must be true or false".to_owned()),
        FieldKind::Duration | FieldKind::Choice(_) if templated => None,
        FieldKind::Duration
            if value
                .as_str()
                .is_some_and(|text| super::parse::parse_timeout(0, text).is_ok()) =>
        {
            None
        }
        FieldKind::Duration => Some(
            "must be a positive whole number followed by s, m or h, such as \"30s\"".to_owned(),
        ),
        FieldKind::Choice(choices)
            if value.as_str().is_some_and(|text| choices.contains(&text)) =>
        {
            None
        }
        FieldKind::Choice(choices) => {
            let quoted: Vec<String> = choices
                .iter()
                .map(|choice| format!("\"{choice}\""))
                .collect();
            Some(format!("must be one of {}", quoted.join(", ")))
        }
    }
}

/// Whether a state of any depth runs this action.
fn has_action(states: &[State], name: &str) -> bool {
    states.iter().any(|state| match &state.kind {
        StateKind::Action { action, .. } => action == name,
        StateKind::ForEach { states, .. } => has_action(states, name),
        StateKind::Wait { .. } => false,
    })
}

/// Every string inside a `with` value, in order.
fn strings<'v>(value: &'v Value, out: &mut Vec<&'v str>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Array(items) => {
            for item in items {
                strings(item, out);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                strings(item, out);
            }
        }
        _ => {}
    }
}

fn noun(scope: Scope<'_>) -> &'static str {
    if scope.body.is_some() {
        "nested state"
    } else {
        "state"
    }
}

/// The transitions between the states of one scope. `None` is an end state.
struct Graph {
    edges: Vec<Vec<Edge>>,
}

#[derive(Clone, Copy)]
struct Edge {
    to: Option<usize>,
    /// The transition has a `limit`, so it does not make a cycle unbounded.
    limited: bool,
}

impl Graph {
    /// Explicit transitions, plus the defaults of outcomes with no rule (D78):
    /// `done` goes to the next state, or to an end after the last one.
    fn new(states: &[State]) -> Self {
        let position = |id: &str| states.iter().position(|state| state.id == id);
        let edges = states
            .iter()
            .enumerate()
            .map(|(index, state)| {
                let mut edges = Vec::new();
                for rule in &state.on {
                    for transition in &rule.transitions {
                        match &transition.to {
                            Target::State(id) => {
                                if let Some(to) = position(id.as_str()) {
                                    edges.push(Edge {
                                        to: Some(to),
                                        limited: transition.limit.is_some(),
                                    });
                                }
                            }
                            Target::End(_) => edges.push(Edge {
                                to: None,
                                limited: false,
                            }),
                        }
                    }
                }
                if let Some(produced) = outcomes_of(&state.kind) {
                    for outcome in DEFAULTED {
                        if produced.contains(outcome) && state.rule(outcome).is_none() {
                            let next = (*outcome == "done" && index + 1 < states.len())
                                .then_some(index + 1);
                            edges.push(Edge {
                                to: next,
                                limited: false,
                            });
                        }
                    }
                } else {
                    // An unknown action or event is reported on the state
                    // itself; assume it ends, so the error is not repeated.
                    edges.push(Edge {
                        to: None,
                        limited: false,
                    });
                }
                edges
            })
            .collect();
        Self { edges }
    }

    fn successors(&self, from: usize) -> impl Iterator<Item = Option<usize>> + '_ {
        self.edges[from].iter().map(|edge| edge.to)
    }

    /// Nodes reached by one or more edges from `from`.
    fn reach(&self, from: usize) -> Vec<bool> {
        let mut seen = vec![false; self.edges.len()];
        let mut stack: Vec<usize> = self.successors(from).flatten().collect();
        while let Some(node) = stack.pop() {
            if !seen[node] {
                seen[node] = true;
                stack.extend(self.successors(node).flatten());
            }
        }
        seen
    }

    /// Whether each node has a path to an end state.
    fn can_end(&self) -> Vec<bool> {
        let mut ends: Vec<bool> = self
            .edges
            .iter()
            .map(|edges| edges.iter().any(|edge| edge.to.is_none()))
            .collect();
        let mut changed = true;
        while changed {
            changed = false;
            for index in 0..ends.len() {
                if !ends[index] && self.successors(index).flatten().any(|next| ends[next]) {
                    ends[index] = true;
                    changed = true;
                }
            }
        }
        ends
    }

    fn without_limits(&self) -> Self {
        Self {
            edges: self
                .edges
                .iter()
                .map(|edges| edges.iter().filter(|edge| !edge.limited).copied().collect())
                .collect(),
        }
    }

    /// One cycle through `start`, as `start, ..., start`. Only called on a
    /// node that `reach` shows to be on a cycle.
    fn cycle_through(&self, start: usize) -> Vec<usize> {
        let count = self.edges.len();
        let mut parent: Vec<Option<usize>> = vec![None; count];
        let mut seen = vec![false; count];
        let mut queue = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            for next in self.successors(node).flatten() {
                if next == start {
                    let mut path = vec![node];
                    let mut current = node;
                    while current != start {
                        current = parent[current].unwrap_or(start);
                        path.push(current);
                    }
                    path.reverse();
                    path.push(start);
                    return path;
                }
                if !seen[next] {
                    seen[next] = true;
                    parent[next] = Some(node);
                    queue.push_back(next);
                }
            }
        }
        vec![start]
    }
}

#[cfg(test)]
mod tests {
    use crate::workflow::Severity;
    use crate::workflow::load;
    use crate::workflow::parse::parse;

    use super::*;

    const RESTACK_CHILDREN: &str =
        include_str!("../../tests/workflows/restack-children-on-parent-move.toml");
    const RESTACK_STALE: &str =
        include_str!("../../tests/workflows/restack-stale-stacks-after-fetch.toml");
    const BLOCK_RESTACK: &str =
        include_str!("../../tests/workflows/block-restack-on-protected.toml");
    const BRANCH_FROM_TICKET: &str = include_str!("../../tests/workflows/branch-from-ticket.toml");
    const COMMIT_MESSAGE: &str = include_str!("../../tests/workflows/commit-message-with-llm.toml");
    const SUGGEST_COMMIT: &str = include_str!("../../tests/workflows/suggest-commit-message.toml");

    /// A file with a header, a trigger and a body. The body starts on line 7,
    /// and the `top` line (possibly empty) is line 3.
    fn file(top: &str, trigger: &str, body: &str) -> String {
        format!("schema = 1\nname = \"w\"\n{top}\n[trigger]\non = \"{trigger}\"\n\n{body}")
    }

    fn diagnostics(source: &str) -> Vec<Diagnostic> {
        validate(&parse(source).expect("source should parse"))
    }

    /// The file's diagnostics, in order, must be exactly `expected`.
    fn assert_only(source: &str, expected: &[(Severity, usize, &str)]) {
        let found: Vec<(Severity, usize, String)> = diagnostics(source)
            .into_iter()
            .map(|d| (d.severity, d.line, d.message))
            .collect();
        let expected: Vec<(Severity, usize, String)> = expected
            .iter()
            .map(|(severity, line, message)| (*severity, *line, (*message).to_owned()))
            .collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn fixtures_have_no_errors() {
        for source in [
            RESTACK_CHILDREN,
            RESTACK_STALE,
            BLOCK_RESTACK,
            BRANCH_FROM_TICKET,
            COMMIT_MESSAGE,
            SUGGEST_COMMIT,
        ] {
            let found = diagnostics(source);
            let has_error = found.iter().any(|d| d.severity == Severity::Error);
            assert!(!has_error, "fixture has errors: {found:#?}");
        }
    }

    #[test]
    fn duplicate_state_id() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\n\n[[state]]\nid = \"a\"\naction = \"noop\"\n",
        );
        assert_only(&source, &[(Severity::Error, 12, "duplicate state `a`")]);
    }

    #[test]
    fn nested_id_equal_to_top_level_id() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"roots\"\naction = \"stack.stale\"\n\n[[state]]\nid = \"each\"\nfor_each = \"${states.roots.output.roots}\"\nas = \"root\"\nstates = [ { id = \"roots\", action = \"noop\" } ]\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                15,
                "nested state `roots` has the same id as a top-level state",
            )],
        );
    }

    #[test]
    fn start_must_be_top_level() {
        let source = file(
            "start = \"nope\"",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\n",
        );
        assert_only(
            &source,
            &[(Severity::Error, 0, "start `nope` is not a top-level state")],
        );
    }

    #[test]
    fn unknown_trigger() {
        let source = file("", "nope", "[[state]]\nid = \"a\"\naction = \"noop\"\n");
        assert_only(&source, &[(Severity::Error, 0, "unknown trigger `nope`")]);
    }

    #[test]
    fn unknown_action() {
        let source = file("", "manual", "[[state]]\nid = \"a\"\naction = \"nope\"\n");
        assert_only(
            &source,
            &[(Severity::Error, 8, "state `a` uses unknown action `nope`")],
        );
    }

    #[test]
    fn unknown_wait_event() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\nwait = \"nope.event\"\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                8,
                "state `a` waits on unknown event `nope.event`",
            )],
        );
    }

    #[test]
    fn transition_to_unknown_state() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\non.done = \"ghost\"\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                8,
                "state `a` targets unknown state `ghost`",
            )],
        );
    }

    #[test]
    fn rule_for_outcome_the_state_cannot_produce() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\non.approved = \"end.done\"\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                8,
                "state `a` has a rule for `approved`, which it cannot produce",
            )],
        );
    }

    #[test]
    fn wait_timeout_rule_needs_a_timeout() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\nwait = \"restack.resumed\"\non.resumed = \"end.done\"\non.aborted = \"end.failed\"\non.timeout = \"end.failed\"\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                8,
                "state `a` has a rule for `timeout`, which it cannot produce",
            )],
        );
    }

    #[test]
    fn outcome_without_rule_is_an_error() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"restack.apply\"\nwith = { plan_id = \"p\" }\n",
        );
        assert_only(
            &source,
            &[
                (Severity::Error, 8, "state `a` has no rule for `conflict`"),
                (Severity::Error, 8, "state `a` has no rule for `stale`"),
            ],
        );
    }

    #[test]
    fn end_blocked_needs_a_before_trigger() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\non.done = \"end.blocked\"\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                8,
                "state `a` targets `end.blocked` outside a `before.*` trigger",
            )],
        );
    }

    #[test]
    fn phase_action_needs_its_phase() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"set\"\nwith = { value = \"x\" }\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                8,
                "state `a` uses action `set`, which is only valid in a `transform.*` trigger",
            )],
        );
    }

    #[test]
    fn for_each_inside_for_each() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"outer\"\nfor_each = \"${repo.files}\"\nas = \"f\"\nstates = [ { id = \"inner\", for_each = \"${repo.files}\", as = \"g\", states = [ { id = \"x\", action = \"noop\" } ] } ]\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                11,
                "state `inner` is a `for_each` inside a `for_each`",
            )],
        );
    }

    #[test]
    fn item_name_cannot_be_a_root() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"each\"\nfor_each = \"${repo.files}\"\nas = \"states\"\nstates = [ { id = \"x\", action = \"noop\" } ]\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                8,
                "state `each` names its item `states`, a reserved root",
            )],
        );
    }

    #[test]
    fn unreachable_state() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\non.done = \"end.done\"\n\n[[state]]\nid = \"b\"\naction = \"noop\"\n",
        );
        assert_only(
            &source,
            &[(Severity::Error, 13, "state `b` is not reachable from `a`")],
        );
    }

    #[test]
    fn unreachable_nested_state() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"each\"\nfor_each = \"${repo.files}\"\nas = \"f\"\nstates = [ { id = \"a\", action = \"noop\", on.done = \"end.done\" }, { id = \"b\", action = \"noop\" } ]\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                11,
                "nested state `b` is not reachable from `a`",
            )],
        );
    }

    #[test]
    fn state_without_path_to_an_end() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\non.done = \"b\"\non.failed = \"b\"\n\n[[state]]\nid = \"b\"\naction = \"noop\"\non.done = { to = \"a\", limit = 2 }\non.failed = { to = \"a\", limit = 2 }\n",
        );
        assert_only(
            &source,
            &[
                (Severity::Error, 8, "state `a` has no path to an end state"),
                (
                    Severity::Warning,
                    14,
                    "the last transition of `on.done` in state `b` has an `if` or a `limit`; the run ends failed when none applies",
                ),
                (
                    Severity::Warning,
                    14,
                    "the last transition of `on.failed` in state `b` has an `if` or a `limit`; the run ends failed when none applies",
                ),
                (Severity::Error, 14, "state `b` has no path to an end state"),
            ],
        );
    }

    #[test]
    fn cycle_without_limit() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\non.done = \"b\"\n\n[[state]]\nid = \"b\"\naction = \"noop\"\non.done = \"a\"\n",
        );
        assert_only(
            &source,
            &[(Severity::Error, 8, "cycle `a` → `b` → `a` has no `limit`")],
        );
    }

    #[test]
    fn self_edge_without_limit_is_a_cycle() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\non.done = \"a\"\n",
        );
        assert_only(
            &source,
            &[(Severity::Error, 8, "cycle `a` → `a` has no `limit`")],
        );
    }

    #[test]
    fn cycle_with_a_limited_edge_is_bounded() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\non.done = \"b\"\n\n[[state]]\nid = \"b\"\naction = \"noop\"\non.done = [ { to = \"a\", limit = 2 }, { to = \"end.done\" } ]\n",
        );
        assert_only(&source, &[]);
    }

    #[test]
    fn invalid_condition_syntax() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\non.done = [ { if = \"(${trigger.x} == 1\", to = \"end.done\" }, { to = \"end.done\" } ]\n",
        );
        let found = diagnostics(&source);
        assert!(
            found.iter().any(|d| d.severity == Severity::Error
                && d.line == 8
                && d.message.starts_with("state `a` has an invalid condition")),
            "{found:#?}"
        );
    }

    #[test]
    fn invalid_template_syntax() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\nwith = { x = \"${trigger.x\" }\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                8,
                "state `a` has an invalid expression: unclosed `${`",
            )],
        );
    }

    #[test]
    fn unknown_reference_root() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\nwith = { x = \"${nope.y}\" }\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                8,
                "`${nope.y}` in state `a` has an unknown root `nope`",
            )],
        );
    }

    #[test]
    fn undeclared_input() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\nwith = { x = \"${inputs.ticket}\" }\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                8,
                "`${inputs.ticket}` in state `a` is not a declared input",
            )],
        );
    }

    #[test]
    fn declared_input_is_fine() {
        let source = "schema = 1\nname = \"w\"\n\n[inputs]\nticket = { type = \"string\" }\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"a\"\naction = \"noop\"\nwith = { x = \"${inputs.ticket}\" }\n";
        assert_only(source, &[]);
    }

    #[test]
    fn read_of_unknown_state() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\nwith = { x = \"${states.ghost.output.x}\" }\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                8,
                "`${states.ghost.output.x}` in state `a` names no state `ghost`",
            )],
        );
    }

    #[test]
    fn read_must_go_through_output() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\non.done = \"b\"\n\n[[state]]\nid = \"b\"\naction = \"noop\"\nwith = { x = \"${states.a.result}\" }\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Error,
                13,
                "`${states.a.result}` in state `b` must read `states.a.output`",
            )],
        );
    }

    #[test]
    fn auto_approval_on_non_apply_action_warns() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\napproval = \"auto\"\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Warning,
                8,
                "`approval = \"auto\"` on state `a`, which is not an apply action",
            )],
        );
    }

    #[test]
    fn transform_without_set_warns() {
        let source = file(
            "",
            "transform.commit_message",
            "[[state]]\nid = \"a\"\naction = \"noop\"\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Warning,
                0,
                "transform workflow has no `set` state",
            )],
        );
    }

    #[test]
    fn last_transition_with_if_warns() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"noop\"\non.done = [ { if = \"${trigger.x} == 1\", to = \"end.done\" } ]\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Warning,
                8,
                "the last transition of `on.done` in state `a` has an `if` or a `limit`; the run ends failed when none applies",
            )],
        );
    }

    #[test]
    fn read_without_path_from_its_state_warns() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"s\"\naction = \"noop\"\non.done = [ { if = \"${trigger.x} == 1\", to = \"a\" }, { to = \"b\" } ]\n\n[[state]]\nid = \"a\"\naction = \"noop\"\non.done = \"end.done\"\n\n[[state]]\nid = \"b\"\naction = \"noop\"\nwith = { x = \"${states.a.output.y}\" }\n",
        );
        assert_only(
            &source,
            &[(
                Severity::Warning,
                18,
                "`states.a` is read in state `b`, but no path leads from `a` to `b`; `a` may not have run",
            )],
        );
    }

    #[test]
    fn self_read_is_allowed() {
        let source = file(
            "",
            "manual",
            "[[state]]\nid = \"ask\"\naction = \"exec\"\nwith = { argv = [\"x\"], stdin = \"${states.ask.output.text}\" }\non.done = [ { if = \"len(${states.ask.output.text}) == 0\", to = \"ask\", limit = 2 }, { to = \"end.done\" } ]\non.timeout = \"end.failed\"\n",
        );
        assert_only(&source, &[]);
    }

    /// A manual workflow with one state that runs `with` as its action's input.
    fn one_action(action: &str, with: &str) -> String {
        file(
            "",
            "manual",
            &format!("[[state]]\nid = \"a\"\naction = \"{action}\"\nwith = {{ {with} }}\n"),
        )
    }

    #[test]
    fn a_missing_required_field_is_an_error() {
        assert_only(
            &one_action("notify", ""),
            &[(
                Severity::Error,
                8,
                "state `a` is missing `with.text`, which action `notify` needs",
            )],
        );
    }

    #[test]
    fn an_unknown_field_warns() {
        assert_only(
            &one_action("notify", "text = \"x\", colour = \"red\""),
            &[(
                Severity::Warning,
                8,
                "state `a`: `with.colour` is not a field of action `notify`",
            )],
        );
    }

    #[test]
    fn a_field_of_the_wrong_type_is_an_error() {
        assert_only(
            &one_action("exec", "argv = \"git\""),
            &[(
                Severity::Error,
                8,
                "state `a`: `with.argv` must be an array of strings",
            )],
        );
        assert_only(
            &one_action("branch.create", "name = \"x\", checkout = \"yes\""),
            &[(
                Severity::Error,
                8,
                "state `a`: `with.checkout` must be true or false",
            )],
        );
        assert_only(
            &one_action("notify", "text = 3"),
            &[(
                Severity::Error,
                8,
                "state `a`: `with.text` must be a string",
            )],
        );
    }

    #[test]
    fn a_duration_and_a_choice_must_have_their_shape() {
        assert_only(
            &one_action("exec", "argv = [\"x\"], timeout = \"10x\""),
            &[(
                Severity::Error,
                8,
                "state `a`: `with.timeout` must be a positive whole number followed by s, m or h, such as \"30s\"",
            )],
        );
        assert_only(
            &one_action("exec", "argv = [\"x\"], format = \"yaml\""),
            &[(
                Severity::Error,
                8,
                "state `a`: `with.format` must be one of \"json\"",
            )],
        );
        assert_only(
            &one_action(
                "exec",
                "argv = [\"x\"], timeout = \"2m\", format = \"json\"",
            ),
            &[],
        );
    }

    #[test]
    fn a_template_in_a_duration_or_choice_is_not_checked_here() {
        assert_only(
            &one_action(
                "exec",
                "argv = [\"x\"], timeout = \"${trigger.t}\", format = \"${trigger.f}\"",
            ),
            &[],
        );
    }

    #[test]
    fn set_and_actions_without_fields_are_not_checked() {
        let transform = file(
            "",
            "transform.commit_message",
            "[[state]]\nid = \"a\"\naction = \"set\"\nwith = { value = \"x\", anything = 3 }\n",
        );
        assert_only(&transform, &[]);
        assert_only(&one_action("stage.all", "colour = \"red\""), &[]);
    }

    #[test]
    fn an_optional_field_may_be_left_out() {
        let approve = file(
            "",
            "manual",
            "[[state]]\nid = \"a\"\naction = \"approve\"\non.approved = \"end.done\"\non.rejected = \"end.done\"\n",
        );
        assert_only(&approve, &[]);
    }

    #[test]
    fn load_rejects_an_invalid_file_and_keeps_warnings() {
        let invalid = "schema = 1\nname = \"w\"\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"a\"\naction = \"nope\"\n";
        let Err(diagnostics) = load(invalid) else {
            panic!("invalid file should be rejected");
        };
        assert!(
            diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error && d.line == 8)
        );

        let warned = "schema = 1\nname = \"w\"\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"a\"\naction = \"noop\"\napproval = \"auto\"\n";
        let Ok((_, warnings)) = load(warned) else {
            panic!("warnings alone should not reject the file");
        };
        assert_eq!(warnings.len(), 1);
    }
}
