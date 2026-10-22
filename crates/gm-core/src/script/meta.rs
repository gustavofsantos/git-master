//! `M.meta` (docs/reference/workflow-api.md): what a script says about itself, read without
//! running any of its logic. The engine hands over the table as JSON; this module checks it, with
//! messages that name the valid choices so a person or an agent can correct the file from them.

use serde_json::{Map, Value};

use crate::workflow::catalog;

/// The fields `M.meta` may hold.
pub const FIELDS: &[&str] = &["name", "description", "trigger", "concurrency", "inputs"];
/// The fields of `M.meta.trigger`.
pub const TRIGGER_FIELDS: &[&str] = &["on", "match", "key"];
/// The fields of one input.
pub const INPUT_FIELDS: &[&str] = &["type", "prompt"];
/// The input types of v1.
pub const INPUT_TYPES: &[&str] = &["string"];

const NAME_MAX: usize = 80;
const DESCRIPTION_MAX: usize = 200;

/// What happens when a trigger fires during a run (D65).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Concurrency {
    #[default]
    Queue,
    Replace,
    Skip,
}

impl Concurrency {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Replace => "replace",
            Self::Skip => "skip",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trigger {
    /// `manual`, an event, `before.<op>`, `after.<op>` or `transform.<field>`.
    pub on: String,
    /// The hook-style filter of an event trigger, such as `branch:feat/*`.
    pub filter: Option<String>,
    /// The keyboard shortcut of a manual workflow, in the app's notation.
    pub key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputSpec {
    pub name: String,
    pub ty: String,
    /// What the dialog asks; the name when the script gives none.
    pub prompt: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub name: String,
    pub description: String,
    pub trigger: Trigger,
    pub concurrency: Concurrency,
    /// Sorted by name: a Lua table has no order.
    pub inputs: Vec<InputSpec>,
}

/// One problem in `M.meta`. `field` is the path, such as `trigger.on` or `inputs.ticket.type`;
/// the loader turns it into a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaProblem {
    pub field: String,
    pub message: String,
}

fn problem(field: &str, message: impl Into<String>) -> MetaProblem {
    MetaProblem {
        field: field.to_owned(),
        message: message.into(),
    }
}

/// Reads and checks `M.meta`. Every problem is reported, not only the first.
pub fn parse(meta: &Value) -> Result<Meta, Vec<MetaProblem>> {
    let Some(object) = meta.as_object() else {
        return Err(vec![problem("", "M.meta must be a table")]);
    };
    let mut problems = Vec::new();
    for key in object.keys() {
        if !FIELDS.contains(&key.as_str()) {
            problems.push(problem(
                key,
                format!(
                    "unknown field `{key}` in M.meta. Valid fields: {}",
                    FIELDS.join(", ")
                ),
            ));
        }
    }
    let name = name(object, &mut problems);
    let description = description(object, &mut problems);
    let trigger = trigger(object, &mut problems);
    let concurrency = concurrency(object, &mut problems);
    let inputs = inputs(object, &mut problems);
    if let Some(trigger) = &trigger
        && trigger.on != "manual"
        && !inputs.is_empty()
    {
        problems.push(problem(
            "inputs",
            format!(
                "inputs are typed before a manual run; the trigger `{}` cannot have them",
                trigger.on
            ),
        ));
    }
    match (name, trigger) {
        (Some(name), Some(trigger)) if problems.is_empty() => Ok(Meta {
            name,
            description,
            trigger,
            concurrency,
            inputs,
        }),
        _ => Err(problems),
    }
}

fn name(object: &Map<String, Value>, problems: &mut Vec<MetaProblem>) -> Option<String> {
    match object.get("name") {
        None => {
            problems.push(problem("name", "M.meta.name is required"));
            None
        }
        Some(Value::String(text)) => {
            if text.trim().is_empty() {
                problems.push(problem("name", "M.meta.name must not be empty"));
                None
            } else if text.chars().count() > NAME_MAX || text.chars().any(char::is_control) {
                problems.push(problem(
                    "name",
                    format!("M.meta.name must be one line of at most {NAME_MAX} characters"),
                ));
                None
            } else {
                Some(text.clone())
            }
        }
        Some(_) => {
            problems.push(problem("name", "M.meta.name must be text"));
            None
        }
    }
}

fn description(object: &Map<String, Value>, problems: &mut Vec<MetaProblem>) -> String {
    match object.get("description") {
        None => String::new(),
        Some(Value::String(text)) => {
            if text.chars().count() > DESCRIPTION_MAX || text.contains('\n') {
                problems.push(problem(
                    "description",
                    format!(
                        "M.meta.description must be one line of at most {DESCRIPTION_MAX} characters"
                    ),
                ));
            }
            text.clone()
        }
        Some(_) => {
            problems.push(problem("description", "M.meta.description must be text"));
            String::new()
        }
    }
}

fn trigger(object: &Map<String, Value>, problems: &mut Vec<MetaProblem>) -> Option<Trigger> {
    let Some(value) = object.get("trigger") else {
        problems.push(problem(
            "trigger",
            "M.meta.trigger is required, such as { on = \"manual\" }",
        ));
        return None;
    };
    let Some(table) = value.as_object() else {
        problems.push(problem("trigger", "M.meta.trigger must be a table"));
        return None;
    };
    for key in table.keys() {
        if !TRIGGER_FIELDS.contains(&key.as_str()) {
            problems.push(problem(
                &format!("trigger.{key}"),
                format!(
                    "unknown field `{key}` in M.meta.trigger. Valid fields: {}",
                    TRIGGER_FIELDS.join(", ")
                ),
            ));
        }
    }
    let on = match table.get("on") {
        None => {
            problems.push(problem("trigger.on", "M.meta.trigger.on is required"));
            return None;
        }
        Some(Value::String(on)) => on.clone(),
        Some(_) => {
            problems.push(problem("trigger.on", "M.meta.trigger.on must be text"));
            return None;
        }
    };
    if !catalog::is_known_trigger(&on) {
        problems.push(problem(
            "trigger.on",
            format!(
                "unknown trigger `{on}`. Valid triggers: {}",
                catalog::triggers().join(", ")
            ),
        ));
    }
    let filter = optional_text(table, "match", problems);
    let key = optional_text(table, "key", problems);
    if filter.is_some() && on == "manual" {
        problems.push(problem(
            "trigger.match",
            "M.meta.trigger.match narrows an event trigger; a manual workflow has nothing to match",
        ));
    }
    if let Some(key) = &key {
        if on != "manual" {
            problems.push(problem(
                "trigger.key",
                "M.meta.trigger.key is the shortcut of a manual workflow",
            ));
        } else if let Err(why) = check_shortcut(key) {
            problems.push(problem("trigger.key", why));
        }
    }
    Some(Trigger { on, filter, key })
}

fn optional_text(
    table: &Map<String, Value>,
    key: &str,
    problems: &mut Vec<MetaProblem>,
) -> Option<String> {
    match table.get(key) {
        None => None,
        Some(Value::String(text)) if !text.trim().is_empty() => Some(text.clone()),
        Some(_) => {
            problems.push(problem(
                &format!("trigger.{key}"),
                format!("M.meta.trigger.{key} must be non-empty text"),
            ));
            None
        }
    }
}

fn concurrency(object: &Map<String, Value>, problems: &mut Vec<MetaProblem>) -> Concurrency {
    match object.get("concurrency").map(Value::as_str) {
        None => Concurrency::default(),
        Some(Some("queue")) => Concurrency::Queue,
        Some(Some("replace")) => Concurrency::Replace,
        Some(Some("skip")) => Concurrency::Skip,
        Some(_) => {
            problems.push(problem(
                "concurrency",
                "M.meta.concurrency must be one of: queue, replace, skip",
            ));
            Concurrency::default()
        }
    }
}

fn inputs(object: &Map<String, Value>, problems: &mut Vec<MetaProblem>) -> Vec<InputSpec> {
    let Some(value) = object.get("inputs") else {
        return Vec::new();
    };
    let Some(table) = value.as_object() else {
        problems.push(problem(
            "inputs",
            "M.meta.inputs must be a table of named inputs",
        ));
        return Vec::new();
    };
    let mut specs = Vec::new();
    for (name, spec) in table {
        let path = format!("inputs.{name}");
        if !is_identifier(name) {
            problems.push(problem(
                &path,
                format!(
                    "the input name `{name}` must be letters, digits and underscores, not starting with a digit, so that input.inputs.{name} works"
                ),
            ));
            continue;
        }
        let Some(fields) = spec.as_object() else {
            problems.push(problem(
                &path,
                format!("input `{name}` must be a table, such as {{ type = \"string\", prompt = \"...\" }}"),
            ));
            continue;
        };
        for key in fields.keys() {
            if !INPUT_FIELDS.contains(&key.as_str()) {
                problems.push(problem(
                    &format!("{path}.{key}"),
                    format!(
                        "unknown field `{key}` in input `{name}`. Valid fields: {}",
                        INPUT_FIELDS.join(", ")
                    ),
                ));
            }
        }
        let ty = match fields.get("type").map(Value::as_str) {
            None => "string".to_owned(),
            Some(Some(ty)) if INPUT_TYPES.contains(&ty) => ty.to_owned(),
            Some(_) => {
                problems.push(problem(
                    &format!("{path}.type"),
                    format!(
                        "the type of input `{name}` must be one of: {}",
                        INPUT_TYPES.join(", ")
                    ),
                ));
                continue;
            }
        };
        let prompt = match fields.get("prompt") {
            None => name.clone(),
            Some(Value::String(text)) if !text.trim().is_empty() => text.clone(),
            Some(_) => {
                problems.push(problem(
                    &format!("{path}.prompt"),
                    format!("the prompt of input `{name}` must be non-empty text"),
                ));
                continue;
            }
        };
        specs.push(InputSpec {
            name: name.clone(),
            ty,
            prompt,
        });
    }
    specs.sort_by(|a, b| a.name.cmp(&b.name));
    specs
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The modifiers a shortcut may use, in the app's notation (`secondary` is Cmd on macOS and Ctrl
/// elsewhere).
const MODIFIERS: &[&str] = &["secondary", "ctrl", "alt", "shift", "cmd"];
/// The keys with a name.
const NAMED_KEYS: &[&str] = &[
    "enter",
    "escape",
    "tab",
    "space",
    "backspace",
    "delete",
    "up",
    "down",
    "left",
    "right",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
];

/// A shortcut is `modifier-…-key`, such as `secondary-shift-b`. It needs a modifier other than
/// Shift, or it would take the key away from typing.
pub fn check_shortcut(text: &str) -> Result<(), String> {
    let parts: Vec<&str> = text.split('-').collect();
    let Some((key, modifiers)) = parts.split_last() else {
        return Err("the shortcut is empty".to_owned());
    };
    let example = "such as secondary-shift-b";
    let valid_key = NAMED_KEYS.contains(key)
        || (key.chars().count() == 1 && key.chars().all(|c| c.is_ascii_graphic()));
    if !valid_key {
        return Err(format!(
            "the key `{key}` of the shortcut is not a letter, digit, punctuation mark or one of: {}",
            NAMED_KEYS.join(", ")
        ));
    }
    if let Some(bad) = modifiers.iter().find(|m| !MODIFIERS.contains(m)) {
        return Err(format!(
            "`{bad}` is not a modifier. Valid modifiers: {}",
            MODIFIERS.join(", ")
        ));
    }
    if !modifiers.iter().any(|m| *m != "shift") {
        return Err(format!(
            "a shortcut needs a modifier other than shift ({example})"
        ));
    }
    let mut seen = modifiers.to_vec();
    seen.sort_unstable();
    seen.dedup();
    if seen.len() != modifiers.len() {
        return Err("the shortcut repeats a modifier".to_owned());
    }
    Ok(())
}

/// The 1-based line of the first mention of the last part of `field` in `source`, as a key
/// (`name =`, `["name"] =`) or the value of a field. A best guess for `file:line:` output; 0 when
/// nothing matches.
#[must_use]
pub fn locate_line(source: &str, field: &str) -> usize {
    let Some(last) = field.rsplit('.').next().filter(|s| !s.is_empty()) else {
        return source
            .lines()
            .position(|l| l.contains("M.meta"))
            .map_or(0, |i| i + 1);
    };
    let needles = [
        format!("{last} ="),
        format!("[\"{last}\"]"),
        format!("{last}="),
    ];
    source
        .lines()
        .position(|line| needles.iter().any(|n| line.contains(n.as_str())))
        .map_or(0, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn ok(meta: &Value) -> Meta {
        parse(meta).unwrap_or_else(|p| panic!("{p:?}"))
    }

    fn fields(meta: &Value) -> Vec<String> {
        parse(meta)
            .unwrap_err()
            .into_iter()
            .map(|p| p.field)
            .collect()
    }

    #[test]
    fn the_documented_example_is_valid() {
        let meta = ok(&json!({
            "name": "branch-from-ticket",
            "description": "Create a branch from a ticket key",
            "trigger": { "on": "manual" },
            "concurrency": "queue",
            "inputs": { "ticket": { "type": "string", "prompt": "Ticket key" } },
        }));
        assert_eq!(meta.name, "branch-from-ticket");
        assert_eq!(meta.trigger.on, "manual");
        assert_eq!(meta.concurrency, Concurrency::Queue);
        assert_eq!(
            meta.inputs,
            [InputSpec {
                name: "ticket".into(),
                ty: "string".into(),
                prompt: "Ticket key".into()
            }]
        );
    }

    #[test]
    fn only_name_and_trigger_are_required() {
        let meta = ok(&json!({ "name": "w", "trigger": { "on": "before.restack" } }));
        assert_eq!(meta.description, "");
        assert_eq!(meta.concurrency, Concurrency::Queue);
        assert!(meta.inputs.is_empty());
        assert!(parse(&json!({})).unwrap_err().len() >= 2);
    }

    #[test]
    fn an_unknown_field_lists_the_valid_ones() {
        let problems =
            parse(&json!({ "name": "w", "trigger": { "on": "manual" }, "color": 1 })).unwrap_err();
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0]
                .message
                .contains("Valid fields: name, description, trigger")
        );
    }

    #[test]
    fn an_unknown_trigger_lists_the_valid_ones() {
        let problems = parse(&json!({ "name": "w", "trigger": { "on": "whenever" } })).unwrap_err();
        assert_eq!(problems[0].field, "trigger.on");
        assert!(problems[0].message.contains("manual"));
        assert!(problems[0].message.contains("stack.parent_moved"));
    }

    #[test]
    fn every_catalog_trigger_is_accepted() {
        for on in catalog::triggers() {
            let mut meta = json!({ "name": "w", "trigger": { "on": on } });
            meta["trigger"]["on"] = json!(on);
            assert!(parse(&meta).is_ok(), "{on}");
        }
    }

    #[test]
    fn match_belongs_to_events_and_key_to_manual() {
        let manual_match =
            json!({ "name": "w", "trigger": { "on": "manual", "match": "branch:x" } });
        assert_eq!(fields(&manual_match), ["trigger.match"]);
        let event_key =
            json!({ "name": "w", "trigger": { "on": "commit.created", "key": "secondary-b" } });
        assert_eq!(fields(&event_key), ["trigger.key"]);
        assert!(parse(&json!({ "name": "w", "trigger": { "on": "commit.created", "match": "branch:feat/*" } })).is_ok());
    }

    #[test]
    fn shortcuts_follow_the_notation_of_the_app() {
        for good in [
            "secondary-shift-b",
            "ctrl-alt-1",
            "cmd-enter",
            "alt-f5",
            "secondary-/",
        ] {
            assert!(check_shortcut(good).is_ok(), "{good}");
        }
        for bad in [
            "b",
            "shift-b",
            "secondary-",
            "secondary-shift",
            "hyper-b",
            "secondary-secondary-b",
            "secondary-bb",
        ] {
            assert!(check_shortcut(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn concurrency_is_one_of_three() {
        let meta =
            |c: Value| json!({ "name": "w", "trigger": { "on": "manual" }, "concurrency": c });
        assert_eq!(
            ok(&meta(json!("replace"))).concurrency,
            Concurrency::Replace
        );
        assert_eq!(ok(&meta(json!("skip"))).concurrency, Concurrency::Skip);
        assert_eq!(fields(&meta(json!("later"))), ["concurrency"]);
        assert_eq!(fields(&meta(json!(3))), ["concurrency"]);
    }

    #[test]
    fn inputs_need_a_name_that_works_in_lua_and_a_known_type() {
        let with =
            |inputs: Value| json!({ "name": "w", "trigger": { "on": "manual" }, "inputs": inputs });
        assert_eq!(
            fields(&with(json!({ "my-input": {} }))),
            ["inputs.my-input"]
        );
        assert_eq!(
            fields(&with(json!({ "n": { "type": "date" } }))),
            ["inputs.n.type"]
        );
        assert_eq!(
            fields(&with(json!({ "n": { "prompt": "" } }))),
            ["inputs.n.prompt"]
        );
        assert_eq!(
            fields(&with(json!({ "n": { "label": "x" } }))),
            ["inputs.n.label"]
        );
        let meta = ok(&with(json!({ "b": {}, "a": { "prompt": "A?" } })));
        assert_eq!(meta.inputs[0].name, "a");
        assert_eq!(
            meta.inputs[1].prompt, "b",
            "the prompt defaults to the name"
        );
        assert_eq!(meta.inputs[1].ty, "string");
    }

    #[test]
    fn an_event_run_cannot_have_inputs() {
        let meta =
            json!({ "name": "w", "trigger": { "on": "commit.created" }, "inputs": { "x": {} } });
        assert_eq!(fields(&meta), ["inputs"]);
    }

    #[test]
    fn every_problem_is_reported() {
        let meta = json!({ "name": 3, "trigger": { "on": "nope" }, "concurrency": "x" });
        assert_eq!(fields(&meta), ["name", "trigger.on", "concurrency"]);
    }

    #[test]
    fn a_line_is_found_for_a_field() {
        let source = "local M = {}\nM.meta = {\n  name = \"w\",\n  trigger = { on = \"nope\" },\n  concurrency = \"x\",\n}\n";
        assert_eq!(locate_line(source, "name"), 3);
        assert_eq!(locate_line(source, "trigger.on"), 4);
        assert_eq!(locate_line(source, "concurrency"), 5);
        assert_eq!(locate_line(source, ""), 2);
        assert_eq!(locate_line(source, "missing"), 0);
    }
}
