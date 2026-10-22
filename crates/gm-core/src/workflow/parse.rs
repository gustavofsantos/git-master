//! Reads a workflow file into the typed model (design/workflow-scripts.md, D61, D78). Only the
//! shape is checked here: TOML syntax, known keys, types, one kind per state.
//! Graph and reference rules live in `validate`.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value as Json};
use toml::{Spanned, Table, Value};

use super::Diagnostic;
use super::model::{
    Approval, Concurrency, Definition, Input, OutcomeRule, State, StateKind, Target, Transition,
    Trigger,
};

/// Parses one file. The error points at the line of the first problem.
pub fn parse(source: &str) -> Result<Definition, Diagnostic> {
    let raw: RawFile = toml::from_str(source).map_err(|e| {
        let line = e.span().map_or(1, |span| line_at(source, span.start));
        Diagnostic::error(line, e.message())
    })?;

    let schema = raw
        .schema
        .ok_or_else(|| Diagnostic::error(1, "missing `schema`"))?;
    if *schema.get_ref() != 1 {
        let message = format!("unsupported `schema` {}, expected 1", schema.get_ref());
        return Err(Diagnostic::error(line_of(source, &schema), message));
    }
    let name = raw
        .name
        .ok_or_else(|| Diagnostic::error(1, "missing `name`"))?;
    let concurrency = parse_concurrency(source, raw.concurrency)?;
    let trigger = parse_trigger(source, raw.trigger)?;
    let inputs = match raw.inputs {
        None => Vec::new(),
        Some(table) => parse_inputs(line_of(source, &table), table.into_inner())?,
    };
    let Some(raw_states) = raw.states else {
        return Err(no_states());
    };
    if raw_states.is_empty() {
        return Err(no_states());
    }
    let states = parse_states(source, raw_states)?;
    let start = raw.start.unwrap_or_else(|| states[0].id.clone());

    Ok(Definition {
        schema: 1,
        name,
        description: raw.description,
        concurrency,
        trigger,
        inputs,
        start,
        states,
    })
}

/// Top level of the file. Unknown keys are errors.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    schema: Option<Spanned<i64>>,
    name: Option<String>,
    description: Option<String>,
    concurrency: Option<Spanned<String>>,
    start: Option<String>,
    trigger: Option<Spanned<RawTrigger>>,
    inputs: Option<Spanned<Table>>,
    #[serde(rename = "state")]
    states: Option<Vec<RawState>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTrigger {
    on: Option<String>,
    #[serde(rename = "match")]
    matcher: Option<String>,
    key: Option<String>,
}

/// One `[[state]]` table, or one nested state of a `for_each`. Which keys
/// are present decides its kind; `id` is spanned for diagnostics.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawState {
    id: Spanned<String>,
    action: Option<String>,
    with: Option<Table>,
    approval: Option<String>,
    wait: Option<String>,
    timeout: Option<String>,
    for_each: Option<String>,
    #[serde(rename = "as")]
    item: Option<String>,
    states: Option<Vec<RawState>>,
    on: Option<Table>,
}

fn no_states() -> Diagnostic {
    Diagnostic::error(1, "no `[[state]]`: a workflow needs at least one state")
}

/// The 1-based line holding this byte offset.
fn line_at(source: &str, offset: usize) -> usize {
    let head = source.get(..offset.min(source.len())).unwrap_or(source);
    1 + head.matches('\n').count()
}

fn line_of<T>(source: &str, spanned: &Spanned<T>) -> usize {
    line_at(source, spanned.span().start)
}

fn parse_concurrency(
    source: &str,
    raw: Option<Spanned<String>>,
) -> Result<Concurrency, Diagnostic> {
    let Some(raw) = raw else {
        return Ok(Concurrency::Queue);
    };
    match raw.get_ref().as_str() {
        "queue" => Ok(Concurrency::Queue),
        "replace" => Ok(Concurrency::Replace),
        "skip" => Ok(Concurrency::Skip),
        other => Err(Diagnostic::error(
            line_of(source, &raw),
            format!("unknown concurrency `{other}`, expected `queue`, `replace` or `skip`"),
        )),
    }
}

fn parse_trigger(source: &str, raw: Option<Spanned<RawTrigger>>) -> Result<Trigger, Diagnostic> {
    let Some(raw) = raw else {
        return Err(Diagnostic::error(1, "missing `[trigger]`"));
    };
    let line = line_of(source, &raw);
    let raw = raw.into_inner();
    let on = raw
        .on
        .ok_or_else(|| Diagnostic::error(line, "missing `on` in `[trigger]`"))?;
    Ok(Trigger {
        on,
        matcher: raw.matcher,
        key: raw.key,
    })
}

/// `[inputs]`: each entry is a table with an optional `type` and `prompt`.
/// Errors point at the `[inputs]` table, since entries carry no spans.
fn parse_inputs(line: usize, table: Table) -> Result<Vec<Input>, Diagnostic> {
    table
        .into_iter()
        .map(|(name, value)| {
            let Value::Table(mut fields) = value else {
                return Err(Diagnostic::error(
                    line,
                    format!("input `{name}` must be a table"),
                ));
            };
            let kind = match fields.remove("type") {
                None => "string".to_owned(),
                Some(Value::String(kind)) if kind == "string" => kind,
                Some(_) => {
                    return Err(Diagnostic::error(
                        line,
                        format!(
                            "input `{name}` has an unsupported type, only `string` is supported"
                        ),
                    ));
                }
            };
            let prompt = take_string(&mut fields, "prompt", line)?;
            if let Some(key) = fields.keys().next() {
                return Err(Diagnostic::error(
                    line,
                    format!("unknown key `{key}` in input `{name}`"),
                ));
            }
            Ok(Input { name, kind, prompt })
        })
        .collect()
}

fn parse_states(source: &str, raws: Vec<RawState>) -> Result<Vec<State>, Diagnostic> {
    raws.into_iter()
        .map(|raw| parse_state(source, raw))
        .collect()
}

fn parse_state(source: &str, mut raw: RawState) -> Result<State, Diagnostic> {
    let line = line_of(source, &raw.id);
    let id = raw.id.get_ref().clone();
    let on_table = raw.on.take();

    let present: Vec<&str> = [
        raw.action.is_some().then_some("action"),
        raw.wait.is_some().then_some("wait"),
        raw.for_each.is_some().then_some("for_each"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let kind = match present.as_slice() {
        [] => {
            return Err(Diagnostic::error(
                line,
                format!("state `{id}` sets none of `action`, `wait` or `for_each`"),
            ));
        }
        ["action"] => action_kind(line, raw)?,
        ["wait"] => wait_kind(line, raw)?,
        ["for_each"] => for_each_kind(source, line, &id, raw)?,
        [first, second] => {
            return Err(Diagnostic::error(
                line,
                format!("state `{id}` sets both `{first}` and `{second}`"),
            ));
        }
        _ => {
            return Err(Diagnostic::error(
                line,
                format!("state `{id}` sets more than one of `action`, `wait` or `for_each`"),
            ));
        }
    };

    let on = match on_table {
        None => Vec::new(),
        Some(table) => parse_on(line, table)?,
    };
    Ok(State { id, line, kind, on })
}

fn action_kind(line: usize, raw: RawState) -> Result<StateKind, Diagnostic> {
    reject(line, raw.timeout.is_some(), "timeout", "a `wait`")?;
    reject(line, raw.item.is_some(), "as", "a `for_each`")?;
    reject(line, raw.states.is_some(), "states", "a `for_each`")?;
    let approval = match raw.approval.as_deref() {
        None | Some("ask") => Approval::Ask,
        Some("auto") => Approval::Auto,
        Some(other) => {
            return Err(Diagnostic::error(
                line,
                format!("unknown approval `{other}`, expected `ask` or `auto`"),
            ));
        }
    };
    let with = raw.with.map_or_else(
        || Json::Object(Map::new()),
        |table| to_json(Value::Table(table)),
    );
    Ok(StateKind::Action {
        action: raw.action.unwrap_or_default(),
        with,
        approval,
    })
}

fn wait_kind(line: usize, raw: RawState) -> Result<StateKind, Diagnostic> {
    reject(line, raw.with.is_some(), "with", "an `action`")?;
    reject(line, raw.approval.is_some(), "approval", "an `action`")?;
    reject(line, raw.item.is_some(), "as", "a `for_each`")?;
    reject(line, raw.states.is_some(), "states", "a `for_each`")?;
    let timeout = raw
        .timeout
        .map(|text| parse_timeout(line, &text))
        .transpose()?;
    Ok(StateKind::Wait {
        event: raw.wait.unwrap_or_default(),
        timeout,
    })
}

fn for_each_kind(
    source: &str,
    line: usize,
    id: &str,
    raw: RawState,
) -> Result<StateKind, Diagnostic> {
    reject(line, raw.with.is_some(), "with", "an `action`")?;
    reject(line, raw.approval.is_some(), "approval", "an `action`")?;
    reject(line, raw.timeout.is_some(), "timeout", "a `wait`")?;
    let item = raw
        .item
        .ok_or_else(|| Diagnostic::error(line, format!("state `{id}` is missing `as`")))?;
    let nested = raw
        .states
        .ok_or_else(|| Diagnostic::error(line, format!("state `{id}` is missing `states`")))?;
    if nested.is_empty() {
        return Err(Diagnostic::error(
            line,
            format!("state `{id}` has an empty `states`"),
        ));
    }
    Ok(StateKind::ForEach {
        items: raw.for_each.unwrap_or_default(),
        item,
        states: parse_states(source, nested)?,
    })
}

/// Rejects a key that only another kind of state takes.
fn reject(line: usize, present: bool, key: &str, owner: &str) -> Result<(), Diagnostic> {
    if present {
        Err(Diagnostic::error(
            line,
            format!("`{key}` is only valid on {owner} state"),
        ))
    } else {
        Ok(())
    }
}

/// `"<positive integer><s|m|h>"`, such as `60s` or `24h`.
pub(super) fn parse_timeout(line: usize, text: &str) -> Result<Duration, Diagnostic> {
    let invalid = || {
        Diagnostic::error(
            line,
            format!(
                "invalid timeout `{text}`, expected a positive integer with unit `s`, `m` or `h`"
            ),
        )
    };
    let multiplier = match text.chars().next_back() {
        Some('s') => 1,
        Some('m') => 60,
        Some('h') => 3600,
        _ => return Err(invalid()),
    };
    // The unit is one ASCII byte, so this slice ends on a character boundary.
    let digits = &text[..text.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let count: u64 = digits.parse().map_err(|_| invalid())?;
    let seconds = count.checked_mul(multiplier).filter(|&s| s > 0);
    seconds.map(Duration::from_secs).ok_or_else(invalid)
}

/// `on`: each outcome maps to a target, a table or an array of those.
/// Outcomes keep the file order.
fn parse_on(line: usize, table: Table) -> Result<Vec<OutcomeRule>, Diagnostic> {
    table
        .into_iter()
        .map(|(outcome, value)| {
            let transitions = match value {
                Value::String(to) => vec![short_transition(&to)],
                Value::Table(fields) => vec![parse_transition(line, &outcome, fields)?],
                Value::Array(items) => {
                    if items.is_empty() {
                        return Err(Diagnostic::error(
                            line,
                            format!("`on.{outcome}` has no transitions"),
                        ));
                    }
                    items
                        .into_iter()
                        .map(|item| match item {
                            Value::String(to) => Ok(short_transition(&to)),
                            Value::Table(fields) => parse_transition(line, &outcome, fields),
                            _ => Err(transition_shape(line, &outcome)),
                        })
                        .collect::<Result<_, _>>()?
                }
                _ => return Err(transition_shape(line, &outcome)),
            };
            Ok(OutcomeRule {
                outcome,
                transitions,
            })
        })
        .collect()
}

fn transition_shape(line: usize, outcome: &str) -> Diagnostic {
    Diagnostic::error(
        line,
        format!("`on.{outcome}` must be a target, a table or an array of those"),
    )
}

fn short_transition(to: &str) -> Transition {
    Transition {
        to: Target::parse(to),
        guard: None,
        limit: None,
        reason: None,
    }
}

fn parse_transition(
    line: usize,
    outcome: &str,
    mut fields: Table,
) -> Result<Transition, Diagnostic> {
    let to = match fields.remove("to") {
        Some(Value::String(to)) => Target::parse(&to),
        Some(_) => {
            return Err(Diagnostic::error(
                line,
                format!("`to` in `on.{outcome}` must be a string"),
            ));
        }
        None => {
            return Err(Diagnostic::error(
                line,
                format!("missing `to` in `on.{outcome}`"),
            ));
        }
    };
    let guard = take_string(&mut fields, "if", line)?;
    let reason = take_string(&mut fields, "reason", line)?;
    let limit = match fields.remove("limit") {
        None => None,
        Some(Value::Integer(n)) => match u32::try_from(n) {
            Ok(n) if n > 0 => Some(n),
            _ => {
                return Err(Diagnostic::error(
                    line,
                    "`limit` must be a positive integer",
                ));
            }
        },
        Some(_) => {
            return Err(Diagnostic::error(
                line,
                "`limit` must be a positive integer",
            ));
        }
    };
    if let Some(key) = fields.keys().next() {
        return Err(Diagnostic::error(
            line,
            format!("unknown key `{key}` in `on.{outcome}`"),
        ));
    }
    if reason.is_some() && !matches!(to, Target::End(_)) {
        return Err(Diagnostic::error(
            line,
            "`reason` is only allowed when `to` is an `end.*` state",
        ));
    }
    Ok(Transition {
        to,
        guard,
        limit,
        reason,
    })
}

fn take_string(fields: &mut Table, key: &str, line: usize) -> Result<Option<String>, Diagnostic> {
    match fields.remove(key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(Diagnostic::error(line, format!("`{key}` must be a string"))),
    }
}

/// Converts a TOML value to JSON: datetimes become strings, and a
/// non-finite float (which has no JSON form) becomes null.
fn to_json(value: Value) -> Json {
    match value {
        Value::String(text) => Json::String(text),
        Value::Integer(n) => Json::from(n),
        Value::Float(f) => serde_json::Number::from_f64(f).map_or(Json::Null, Json::Number),
        Value::Boolean(b) => Json::Bool(b),
        Value::Datetime(date) => Json::String(date.to_string()),
        Value::Array(items) => Json::Array(items.into_iter().map(to_json).collect()),
        Value::Table(fields) => Json::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, to_json(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::workflow::model::End;

    const RESTACK_CHILDREN: &str =
        include_str!("../../tests/workflows/restack-children-on-parent-move.toml");
    const RESTACK_STALE: &str =
        include_str!("../../tests/workflows/restack-stale-stacks-after-fetch.toml");
    const BLOCK_RESTACK: &str =
        include_str!("../../tests/workflows/block-restack-on-protected.toml");
    const BRANCH_FROM_TICKET: &str = include_str!("../../tests/workflows/branch-from-ticket.toml");
    const COMMIT_MESSAGE: &str = include_str!("../../tests/workflows/commit-message-with-llm.toml");
    const SUGGEST_COMMIT: &str = include_str!("../../tests/workflows/suggest-commit-message.toml");

    fn parse_ok(source: &str) -> Definition {
        parse(source).expect("fixture should parse")
    }

    fn parse_err(source: &str) -> Diagnostic {
        parse(source).expect_err("source should be rejected")
    }

    /// A minimal valid file: `[[state]] id = "a"` is on line 8.
    const BASE: &str =
        "schema = 1\nname = \"w\"\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"a\"\n";

    fn with_state(state: &str) -> String {
        format!("{BASE}{state}")
    }

    #[test]
    fn every_fixture_parses() {
        for source in [
            RESTACK_CHILDREN,
            RESTACK_STALE,
            BLOCK_RESTACK,
            BRANCH_FROM_TICKET,
            COMMIT_MESSAGE,
            SUGGEST_COMMIT,
        ] {
            parse_ok(source);
        }
    }

    #[test]
    fn restack_children_has_wait_and_limits() {
        let def = parse_ok(RESTACK_CHILDREN);
        assert_eq!(def.concurrency, Concurrency::Replace);
        assert_eq!(def.start, "plan");
        assert_eq!(def.states.len(), 4);
        assert_eq!(def.trigger.on, "stack.parent_moved");

        let plan = def.state("plan").expect("plan");
        let done = plan.rule("done").expect("done rule");
        assert_eq!(done.transitions.len(), 2);
        assert_eq!(
            done.transitions[0].guard.as_deref(),
            Some("${states.plan.output.branch_count} == 0")
        );
        assert_eq!(done.transitions[0].to, Target::End(End::Done));
        assert_eq!(done.transitions[1].guard, None);
        assert_eq!(done.transitions[1].to, Target::State("apply".into()));

        let apply = def.state("apply").expect("apply");
        assert!(matches!(
            apply.kind,
            StateKind::Action {
                approval: Approval::Auto,
                ..
            }
        ));
        let stale = apply.rule("stale").expect("stale rule");
        assert_eq!(stale.transitions[0].to, Target::State("plan".into()));
        assert_eq!(stale.transitions[0].limit, Some(3));

        let wait = def.state("wait_for_resolution").expect("wait");
        assert_eq!(
            wait.kind,
            StateKind::Wait {
                event: "restack.resumed".into(),
                timeout: Some(Duration::from_hours(24)),
            }
        );
    }

    #[test]
    fn restack_stale_has_nested_for_each() {
        let def = parse_ok(RESTACK_STALE);
        assert_eq!(def.start, "roots");
        assert_eq!(def.states.len(), 2);
        let each = def.state("each").expect("each");
        let StateKind::ForEach {
            items,
            item,
            states,
        } = &each.kind
        else {
            panic!("expected for_each, got {:?}", each.kind);
        };
        assert_eq!(items, "${states.roots.output.roots}");
        assert_eq!(item, "root");
        assert_eq!(states.len(), 3);
        assert!(matches!(states[2].kind, StateKind::Wait { .. }));
        assert_eq!(
            states[1].rule("stale").expect("stale").transitions[0].limit,
            Some(3)
        );
    }

    #[test]
    fn block_restack_reads_blocked_reason() {
        let def = parse_ok(BLOCK_RESTACK);
        assert!(def.trigger.is_before());
        let check = def.state("check").expect("check");
        let rule = check.rule("done").expect("done");
        assert_eq!(rule.transitions[0].to, Target::End(End::Blocked));
        assert_eq!(
            rule.transitions[0].reason.as_deref(),
            Some("Restack on ${trigger.root} is blocked")
        );
        assert_eq!(rule.transitions[1].reason, None);
    }

    #[test]
    fn branch_from_ticket_reads_inputs_and_inline_with() {
        let def = parse_ok(BRANCH_FROM_TICKET);
        assert_eq!(def.inputs.len(), 1);
        assert_eq!(def.inputs[0].name, "ticket");
        assert_eq!(def.inputs[0].kind, "string");
        assert_eq!(def.inputs[0].prompt.as_deref(), Some("Ticket key"));
        let name = def.state("name").expect("name");
        let StateKind::Action { action, with, .. } = &name.kind else {
            panic!("expected action");
        };
        assert_eq!(action, "exec");
        assert_eq!(with["argv"][1], "${inputs.ticket}");
        assert_eq!(
            name.rule("timeout").expect("timeout").transitions[0].to,
            Target::End(End::Failed)
        );
    }

    #[test]
    fn commit_message_converts_with_to_json() {
        let def = parse_ok(COMMIT_MESSAGE);
        assert_eq!(def.trigger.on, "transform.commit_message");
        let StateKind::Action { with, .. } = &def.states[0].kind else {
            panic!("expected action");
        };
        assert_eq!(with["stdin"], "${repo.staged_diff}");
        assert_eq!(with["timeout"], "60s");
        assert!(with["argv"].is_array());
    }

    #[test]
    fn suggest_commit_reads_key_and_default_start() {
        let def = parse_ok(SUGGEST_COMMIT);
        assert_eq!(def.trigger.key.as_deref(), Some("cmd-shift-m"));
        assert_eq!(def.trigger.matcher, None);
        assert_eq!(def.concurrency, Concurrency::Queue);
        assert_eq!(def.start, "ask");
        let ask = def.state("ask").expect("ask");
        assert_eq!(ask.line, 10);
        let done = ask.rule("done").expect("done");
        assert_eq!(done.transitions[0].limit, Some(2));
        assert_eq!(done.transitions[1].limit, None);
    }

    #[test]
    fn inputs_keep_file_order() {
        let source = "schema = 1\nname = \"w\"\n\n[inputs]\nb = { prompt = \"B\" }\na = {}\nc = { type = \"string\" }\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"a\"\naction = \"noop\"\n";
        let def = parse_ok(source);
        let names: Vec<_> = def.inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["b", "a", "c"]);
        assert_eq!(def.inputs[1].kind, "string");
        assert_eq!(def.inputs[1].prompt, None);
    }

    #[test]
    fn start_defaults_to_first_state() {
        let source = with_state("action = \"noop\"\n[[state]]\nid = \"b\"\naction = \"noop\"\n");
        let def = parse_ok(&source);
        assert_eq!(def.start, "a");
        assert_eq!(def.concurrency, Concurrency::Queue);
    }

    #[test]
    fn toml_syntax_error_points_at_its_line() {
        let diag = parse_err("schema = 1\nname = \n");
        assert_eq!(diag.line, 2);
    }

    #[test]
    fn missing_name_is_reported() {
        let diag = parse_err(
            "schema = 1\n[trigger]\non = \"manual\"\n[[state]]\nid = \"a\"\naction = \"noop\"\n",
        );
        assert_eq!(diag, Diagnostic::error(1, "missing `name`"));
    }

    #[test]
    fn wrong_schema_is_reported_at_its_line() {
        let diag = parse_err(&BASE.replacen("schema = 1", "schema = 2", 1));
        assert_eq!(
            diag,
            Diagnostic::error(1, "unsupported `schema` 2, expected 1")
        );
    }

    #[test]
    fn action_and_wait_on_one_state() {
        let diag = parse_err(&with_state("action = \"noop\"\nwait = \"x\"\n"));
        assert_eq!(
            diag,
            Diagnostic::error(8, "state `a` sets both `action` and `wait`")
        );
    }

    #[test]
    fn state_with_no_kind() {
        let diag = parse_err(&with_state("on.done = \"end.done\"\n"));
        assert_eq!(
            diag,
            Diagnostic::error(8, "state `a` sets none of `action`, `wait` or `for_each`")
        );
    }

    #[test]
    fn unknown_state_key_is_an_error() {
        let diag = parse_err(&with_state("action = \"noop\"\nfoo = 1\n"));
        assert_eq!(diag.line, 10);
        assert!(diag.message.contains("foo"), "{}", diag.message);
    }

    #[test]
    fn bad_timeout_is_reported_on_the_state_line() {
        let diag = parse_err(&with_state("wait = \"x\"\ntimeout = \"10x\"\n"));
        assert_eq!(
            diag,
            Diagnostic::error(
                8,
                "invalid timeout `10x`, expected a positive integer with unit `s`, `m` or `h`"
            )
        );
    }

    #[test]
    fn timeouts_accept_units_and_reject_zero() {
        assert_eq!(parse_timeout(1, "90s"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_timeout(1, "2m"), Ok(Duration::from_secs(120)));
        assert_eq!(parse_timeout(1, "24h"), Ok(Duration::from_hours(24)));
        assert!(parse_timeout(1, "0s").is_err());
        assert!(parse_timeout(1, "+5s").is_err());
        assert!(parse_timeout(1, "s").is_err());
        assert!(parse_timeout(1, "5").is_err());
    }

    #[test]
    fn reason_needs_an_end() {
        let diag = parse_err(&with_state(
            "action = \"noop\"\non.done = { to = \"a\", reason = \"no\" }\n",
        ));
        assert_eq!(
            diag,
            Diagnostic::error(8, "`reason` is only allowed when `to` is an `end.*` state")
        );
    }

    #[test]
    fn limit_must_be_positive() {
        let diag = parse_err(&with_state(
            "action = \"noop\"\non.stale = { to = \"a\", limit = 0 }\n",
        ));
        assert_eq!(
            diag,
            Diagnostic::error(8, "`limit` must be a positive integer")
        );
    }

    #[test]
    fn nested_for_each_still_parses() {
        let source = with_state(
            "for_each = \"${inputs.xs}\"\nas = \"x\"\nstates = [\n  { id = \"inner\", for_each = \"${x}\", as = \"y\", states = [ { id = \"leaf\", action = \"noop\" } ] },\n]\n",
        );
        let def = parse_ok(&source);
        let StateKind::ForEach { states, .. } = &def.states[0].kind else {
            panic!("expected for_each");
        };
        assert!(matches!(states[0].kind, StateKind::ForEach { .. }));
    }
}
