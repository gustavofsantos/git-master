//! Structural edits of a workflow file (docs/10): add, remove and move a
//! top-level state, and set the transitions of a state. Text in, text out.
//! The file is edited with `toml_edit`, so comments and formatting of every
//! part the edit does not touch come back byte for byte. Only top-level
//! states are edited here; nested states of a `for_each` are edited in the
//! TOML tab.

use std::fmt;

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TableLike, Value};

use super::parse;

/// Where `move_state` sends a state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

/// Why an edit was refused. The message is shown to the person as is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditError(pub String);

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EditError {}

/// Inserts `[[state]]` with `id` and `action` after the state `after`, or at the end when None.
pub fn add_state(
    source: &str,
    after: Option<&str>,
    id: &str,
    action: &str,
) -> Result<String, EditError> {
    check_id(id)?;
    let mut doc = document(source)?;
    if collect_ids(&doc).iter().any(|known| known == id) {
        return Err(EditError(format!("duplicate state id `{id}`")));
    }
    let index = match after {
        None => None,
        Some(after) => Some(locate(&doc, after)?),
    };

    let mut table = Table::new();
    table.insert("id", toml_edit::value(id));
    table.insert("action", toml_edit::value(action));
    table.decor_mut().set_prefix("\n");

    let states = doc
        .entry("state")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or_else(|| EditError("`state` is not an array of tables".to_owned()))?;
    let at = index.map_or(states.len(), |index| index + 1);
    // The new table takes the position of its left neighbour (or of the
    // first state), so it is written right where it sits in the array.
    let mut slots = positions(states);
    let slot = slots.get(at.saturating_sub(1)).copied().flatten();
    slots.insert(at, slot);
    states.insert(at, table);
    assign_positions(states, &slots);
    finish(&doc, source)
}

/// Removes the top-level state `id`. References to it elsewhere are left; the validator reports them.
pub fn remove_state(source: &str, id: &str) -> Result<String, EditError> {
    let mut doc = document(source)?;
    let index = locate(&doc, id)?;
    let states = states_mut(&mut doc)?;
    if states.len() == 1 {
        return Err(EditError(format!(
            "`{id}` is the only state; a workflow needs at least one"
        )));
    }
    states.remove(index);
    finish(&doc, source)
}

/// Swaps the state with its neighbour. Error at the edges.
pub fn move_state(source: &str, id: &str, direction: Direction) -> Result<String, EditError> {
    let mut doc = document(source)?;
    let index = locate(&doc, id)?;
    let len = states_mut(&mut doc)?.len();
    let target = match direction {
        Direction::Up if index == 0 => {
            return Err(EditError(format!("`{id}` is already the first state")));
        }
        Direction::Up => index - 1,
        Direction::Down if index + 1 == len => {
            return Err(EditError(format!("`{id}` is already the last state")));
        }
        Direction::Down => index + 1,
    };
    // Each table carries its own leading comments and blank line, so the
    // state moves with the comments above it.
    let states = states_mut(&mut doc)?;
    let slots = positions(states);
    let table = states.remove(index);
    states.insert(target, table);
    assign_positions(states, &slots);
    finish(&doc, source)
}

/// The document positions of the states, in array order.
fn positions(states: &ArrayOfTables) -> Vec<Option<isize>> {
    states.iter().map(Table::position).collect()
}

/// `toml_edit` writes tables by document position, not by array order, so
/// after a change the states take the positions `slots` in array order.
fn assign_positions(states: &mut ArrayOfTables, slots: &[Option<isize>]) {
    for (table, slot) in states.iter_mut().zip(slots) {
        table.set_position(*slot);
    }
}

/// Sets the target of transition `index` of `on.<outcome>` of state `state`.
/// No rule for the outcome and index 0: creates `on.<outcome>` with that single target.
/// A rule written as a plain string with index 0: replaces the string.
/// A rule written as an array: sets `to` of element `index`, keeping its `if`, `limit`, `reason`.
/// Index out of range: error.
pub fn set_target(
    source: &str,
    state: &str,
    outcome: &str,
    index: usize,
    target: &str,
) -> Result<String, EditError> {
    check_outcome(outcome)?;
    let mut doc = document(source)?;
    let position = locate(&doc, state)?;
    let table = states_mut(&mut doc)?
        .get_mut(position)
        .ok_or_else(|| unknown_state(state))?;

    if table.get("on").is_none() {
        if index != 0 {
            return Err(out_of_range(outcome, index));
        }
        // A new `on` is written in dotted form, like the examples.
        let mut on = Table::new();
        on.set_implicit(true);
        on.set_dotted(true);
        on.insert(outcome, toml_edit::value(target));
        table.insert("on", Item::Table(on));
        return finish(&doc, source);
    }

    let on = table
        .get_mut("on")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| EditError(format!("`on` in state `{state}` is not a table")))?;
    if let Some(rule) = on.get_mut(outcome) {
        set_in_rule(rule, outcome, index, target)?;
    } else {
        if index != 0 {
            return Err(out_of_range(outcome, index));
        }
        on.insert(outcome, toml_edit::value(target));
    }
    finish(&doc, source)
}

/// Removes `on.<outcome>` from the state, so the outcome falls back to its default (D78 short form).
pub fn clear_outcome(source: &str, state: &str, outcome: &str) -> Result<String, EditError> {
    let mut doc = document(source)?;
    let position = locate(&doc, state)?;
    let table = states_mut(&mut doc)?
        .get_mut(position)
        .ok_or_else(|| unknown_state(state))?;
    let Some(on_item) = table.get_mut("on") else {
        return Ok(doc.to_string());
    };
    let on = on_item
        .as_table_like_mut()
        .ok_or_else(|| EditError(format!("`on` in state `{state}` is not a table")))?;
    on.remove(outcome);
    if on.is_empty() {
        table.remove("on");
    }
    finish(&doc, source)
}

/// A state id not used by any top-level or nested state, from `base`: `base`, `base_2`, `base_3`, ...
pub fn fresh_id(source: &str, base: &str) -> Result<String, EditError> {
    check_id(base)?;
    let doc = document(source)?;
    let known = collect_ids(&doc);
    if !known.iter().any(|id| id == base) {
        return Ok(base.to_owned());
    }
    let mut suffix = 2_u32;
    loop {
        let candidate = format!("{base}_{suffix}");
        if !known.contains(&candidate) {
            return Ok(candidate);
        }
        suffix += 1;
    }
}

/// Parses the source as an editable document.
fn document(source: &str) -> Result<DocumentMut, EditError> {
    source
        .parse()
        .map_err(|err| EditError(format!("not valid TOML: {err}")))
}

/// Serializes the edited document. An edit never turns a file that parsed
/// into one that does not; a file that was already broken is edited as is.
fn finish(doc: &DocumentMut, source: &str) -> Result<String, EditError> {
    let text = doc.to_string();
    if parse::parse(source).is_ok() {
        parse::parse(&text).map_err(|diagnostic| {
            EditError(format!(
                "the edit would leave the workflow invalid: {diagnostic}"
            ))
        })?;
    }
    Ok(text)
}

fn states_mut(doc: &mut DocumentMut) -> Result<&mut ArrayOfTables, EditError> {
    doc.get_mut("state")
        .and_then(Item::as_array_of_tables_mut)
        .ok_or_else(|| EditError("the file has no `[[state]]`".to_owned()))
}

/// The index of a top-level state. A nested state is refused with a pointer
/// to the TOML tab; an unknown id is an error.
fn locate(doc: &DocumentMut, id: &str) -> Result<usize, EditError> {
    let top = doc
        .get("state")
        .and_then(Item::as_array_of_tables)
        .and_then(|states| states.iter().position(|table| table_id(table) == Some(id)));
    if let Some(index) = top {
        return Ok(index);
    }
    if collect_ids(doc).iter().any(|known| known == id) {
        return Err(EditError(format!(
            "`{id}` is a nested state; nested states are edited in the TOML tab"
        )));
    }
    Err(unknown_state(id))
}

fn unknown_state(id: &str) -> EditError {
    EditError(format!("unknown state `{id}`"))
}

fn out_of_range(outcome: &str, index: usize) -> EditError {
    EditError(format!("`on.{outcome}` has no transition at index {index}"))
}

/// Every state id in the file, top-level and nested.
fn collect_ids(doc: &DocumentMut) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(states) = doc.get("state").and_then(Item::as_array_of_tables) {
        for table in states {
            push_ids(table, &mut ids);
        }
    }
    ids
}

fn push_ids(state: &dyn TableLike, ids: &mut Vec<String>) {
    if let Some(id) = state.get("id").and_then(Item::as_str) {
        ids.push(id.to_owned());
    }
    match state.get("states") {
        Some(Item::Value(Value::Array(nested))) => {
            for value in nested {
                if let Some(table) = value.as_inline_table() {
                    push_ids(table, ids);
                }
            }
        }
        Some(Item::ArrayOfTables(nested)) => {
            for table in nested {
                push_ids(table, ids);
            }
        }
        _ => {}
    }
}

fn table_id(table: &Table) -> Option<&str> {
    table.get("id").and_then(Item::as_str)
}

/// The identifier rule of a state id. It is the rule `${states.<id>.output}`
/// references are parsed with (`expr`), so every id can be referenced: letters,
/// digits and `_`, starting with a letter or `_`.
fn check_id(id: &str) -> Result<(), EditError> {
    if super::expr::is_ident(id) {
        Ok(())
    } else {
        Err(EditError(format!(
            "invalid state id `{id}`: use letters, digits and `_`, starting with a letter or `_`"
        )))
    }
}

/// Outcome names are keys of the `on` table; the same bare-key characters as ids.
fn check_outcome(outcome: &str) -> Result<(), EditError> {
    if !outcome.is_empty()
        && outcome
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Ok(())
    } else {
        Err(EditError(format!("invalid outcome `{outcome}`")))
    }
}

/// Sets `to` of one transition of an outcome rule. The rule is a string, a
/// table, or an array of either (D78).
fn set_in_rule(
    rule: &mut Item,
    outcome: &str,
    index: usize,
    target: &str,
) -> Result<(), EditError> {
    match rule {
        Item::Value(Value::String(_)) => {
            if index != 0 {
                return Err(out_of_range(outcome, index));
            }
            replace_string(rule.as_value_mut().ok_or_else(|| shape(outcome))?, target);
            Ok(())
        }
        Item::Value(Value::Array(items)) => {
            let item = items
                .get_mut(index)
                .ok_or_else(|| out_of_range(outcome, index))?;
            match item {
                Value::String(_) => {
                    replace_string(item, target);
                    Ok(())
                }
                Value::InlineTable(table) => set_to(table, outcome, target),
                _ => Err(shape(outcome)),
            }
        }
        Item::Value(Value::InlineTable(table)) => {
            if index != 0 {
                return Err(out_of_range(outcome, index));
            }
            set_to(table, outcome, target)
        }
        Item::Table(table) => {
            if index != 0 {
                return Err(out_of_range(outcome, index));
            }
            set_to(table, outcome, target)
        }
        Item::ArrayOfTables(tables) => {
            let table = tables
                .get_mut(index)
                .ok_or_else(|| out_of_range(outcome, index))?;
            set_to(table, outcome, target)
        }
        _ => Err(shape(outcome)),
    }
}

fn set_to(table: &mut dyn TableLike, outcome: &str, target: &str) -> Result<(), EditError> {
    let slot = table
        .get_mut("to")
        .and_then(Item::as_value_mut)
        .ok_or_else(|| EditError(format!("a transition of `on.{outcome}` has no `to`")))?;
    replace_string(slot, target);
    Ok(())
}

/// Replaces a string value and keeps the spacing and comments around it.
fn replace_string(slot: &mut Value, text: &str) {
    let decor = slot.decor().clone();
    *slot = Value::from(text);
    *slot.decor_mut() = decor;
}

fn shape(outcome: &str) -> EditError {
    EditError(format!(
        "`on.{outcome}` is not a target, a table or an array of those"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/workflows");

    /// A file with comments above a state, an inline comment, a dotted
    /// outcome, an array outcome, and a comment at the end.
    const COMMENTED: &str = "\
schema = 1
name = \"w\"

[trigger]
on = \"manual\"

# First step.
[[state]]
id = \"a\" # inline note
action = \"noop\"
on.done = \"b\"

[[state]]
id = \"b\"
action = \"noop\"
on.done = [
  { if = \"${inputs.x} == 1\", to = \"end.done\" },
  { to = \"end.failed\", limit = 2 },
]
# End of file.
";

    fn ok(result: Result<String, EditError>) -> String {
        result.expect("edit should succeed")
    }

    fn err(result: Result<String, EditError>) -> String {
        result.expect_err("edit should be refused").0
    }

    #[test]
    fn add_after_a_state_keeps_comments() {
        let out = ok(add_state(COMMENTED, Some("a"), "new", "noop"));
        let expected = "\
schema = 1
name = \"w\"

[trigger]
on = \"manual\"

# First step.
[[state]]
id = \"a\" # inline note
action = \"noop\"
on.done = \"b\"

[[state]]
id = \"new\"
action = \"noop\"

[[state]]
id = \"b\"
action = \"noop\"
on.done = [
  { if = \"${inputs.x} == 1\", to = \"end.done\" },
  { to = \"end.failed\", limit = 2 },
]
# End of file.
";
        assert_eq!(out, expected);
    }

    #[test]
    fn add_at_the_end_when_no_anchor() {
        let out = ok(add_state(COMMENTED, None, "new", "noop"));
        let expected = COMMENTED.replace(
            "]\n# End of file.\n",
            "]\n\n[[state]]\nid = \"new\"\naction = \"noop\"\n# End of file.\n",
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn add_refuses_bad_input() {
        assert!(err(add_state(COMMENTED, None, "a", "noop")).contains("duplicate state id `a`"));
        assert!(
            err(add_state(COMMENTED, Some("zz"), "new", "noop")).contains("unknown state `zz`")
        );
        assert!(err(add_state(COMMENTED, None, "bad-id", "noop")).contains("invalid state id"));
        assert!(err(add_state(COMMENTED, None, "1st", "noop")).contains("invalid state id"));
        assert!(err(add_state("not = [toml", None, "x", "noop")).contains("not valid TOML"));
    }

    #[test]
    fn add_refuses_a_nested_id_as_duplicate_and_anchor() {
        let source = "schema = 1\nname = \"w\"\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"each\"\nfor_each = \"${inputs.xs}\"\nas = \"x\"\nstates = [ { id = \"inner\", action = \"noop\" } ]\n";
        assert!(
            err(add_state(source, None, "inner", "noop")).contains("duplicate state id `inner`")
        );
        assert!(err(add_state(source, Some("inner"), "new", "noop")).contains("TOML tab"));
    }

    #[test]
    fn remove_takes_the_state_and_its_comments() {
        let out = ok(remove_state(COMMENTED, "a"));
        let expected = "\
schema = 1
name = \"w\"

[trigger]
on = \"manual\"

[[state]]
id = \"b\"
action = \"noop\"
on.done = [
  { if = \"${inputs.x} == 1\", to = \"end.done\" },
  { to = \"end.failed\", limit = 2 },
]
# End of file.
";
        assert_eq!(out, expected);
    }

    #[test]
    fn remove_refuses_the_last_state_and_unknown_ids() {
        let single = "schema = 1\nname = \"w\"\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"a\"\naction = \"noop\"\n";
        assert!(err(remove_state(single, "a")).contains("only state"));
        assert!(err(remove_state(COMMENTED, "zz")).contains("unknown state `zz`"));
    }

    #[test]
    fn move_down_then_up_by_name() {
        let down = ok(move_state(COMMENTED, "a", Direction::Down));
        let expected_down = "\
schema = 1
name = \"w\"

[trigger]
on = \"manual\"

[[state]]
id = \"b\"
action = \"noop\"
on.done = [
  { if = \"${inputs.x} == 1\", to = \"end.done\" },
  { to = \"end.failed\", limit = 2 },
]

# First step.
[[state]]
id = \"a\" # inline note
action = \"noop\"
on.done = \"b\"
# End of file.
";
        assert_eq!(down, expected_down);
        assert_eq!(ok(move_state(&down, "a", Direction::Up)), COMMENTED);
    }

    #[test]
    fn move_refuses_the_edges() {
        assert!(err(move_state(COMMENTED, "a", Direction::Up)).contains("already the first"));
        assert!(err(move_state(COMMENTED, "b", Direction::Down)).contains("already the last"));
    }

    #[test]
    fn set_target_replaces_a_plain_string() {
        let out = ok(set_target(COMMENTED, "a", "done", 0, "end.done"));
        let expected = COMMENTED.replacen("on.done = \"b\"", "on.done = \"end.done\"", 1);
        assert_eq!(out, expected);
    }

    #[test]
    fn set_target_changes_only_to_of_an_array_element() {
        let first = ok(set_target(COMMENTED, "b", "done", 0, "end.failed"));
        let expected = COMMENTED.replacen(
            "{ if = \"${inputs.x} == 1\", to = \"end.done\" },",
            "{ if = \"${inputs.x} == 1\", to = \"end.failed\" },",
            1,
        );
        assert_eq!(first, expected);

        let second = ok(set_target(COMMENTED, "b", "done", 1, "b"));
        let expected = COMMENTED.replacen(
            "{ to = \"end.failed\", limit = 2 },",
            "{ to = \"b\", limit = 2 },",
            1,
        );
        assert_eq!(second, expected);
    }

    #[test]
    fn set_target_creates_a_dotted_outcome() {
        let out = ok(set_target(COMMENTED, "a", "failed", 0, "end.failed"));
        assert!(
            out.contains("on.done = \"b\"\non.failed = \"end.failed\"\n\n[[state]]\nid = \"b\""),
            "{out}"
        );
    }

    #[test]
    fn set_target_creates_on_when_the_state_has_none() {
        let source = "schema = 1\nname = \"w\"\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"a\"\naction = \"noop\"\n";
        let out = ok(set_target(source, "a", "done", 0, "end.done"));
        assert_eq!(out, format!("{source}on.done = \"end.done\"\n"));
        assert!(err(set_target(source, "a", "done", 1, "end.done")).contains("index 1"));
    }

    #[test]
    fn set_target_refuses_bad_input() {
        assert!(err(set_target(COMMENTED, "a", "done", 1, "x")).contains("index 1"));
        assert!(err(set_target(COMMENTED, "zz", "done", 0, "x")).contains("unknown state"));
        assert!(err(set_target(COMMENTED, "a", "bad outcome", 0, "x")).contains("invalid outcome"));
    }

    #[test]
    fn clear_outcome_drops_the_rule_and_keeps_the_rest() {
        let out = ok(clear_outcome(COMMENTED, "a", "done"));
        assert!(!out.contains("on.done = \"b\""));
        assert!(
            out.contains("action = \"noop\"\n\n[[state]]\nid = \"b\""),
            "{out}"
        );
        let out = ok(clear_outcome(COMMENTED, "b", "done"));
        assert!(!out.contains("on.done = ["));
        assert!(
            out.contains("id = \"b\"\naction = \"noop\"\n# End of file.\n"),
            "{out}"
        );
        assert_eq!(ok(clear_outcome(COMMENTED, "a", "failed")), COMMENTED);
    }

    #[test]
    fn fresh_id_counts_nested_ids_too() {
        assert_eq!(fresh_id(COMMENTED, "new").expect("free"), "new");
        assert_eq!(fresh_id(COMMENTED, "a").expect("a is taken"), "a_2");
        let source = "schema = 1\nname = \"w\"\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"step\"\naction = \"noop\"\n\n[[state]]\nid = \"step_2\"\naction = \"noop\"\n\n[[state]]\nid = \"each\"\nfor_each = \"${inputs.xs}\"\nas = \"x\"\nstates = [ { id = \"step_3\", action = \"noop\" } ]\n";
        assert_eq!(fresh_id(source, "step").expect("step_4"), "step_4");
        assert!(fresh_id(source, "bad-id").is_err());
    }

    #[test]
    fn every_example_survives_move_down_then_up() {
        let mut moved = 0;
        for entry in std::fs::read_dir(EXAMPLES).expect("examples directory") {
            let path = entry.expect("entry").path();
            if path.extension().is_none_or(|ext| ext != "toml") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("example");
            let definition = parse::parse(&source).expect("example parses");
            for state in &definition.states {
                if let Ok(down) = move_state(&source, &state.id, Direction::Down) {
                    moved += 1;
                    assert_eq!(
                        ok(move_state(&down, &state.id, Direction::Up)),
                        source,
                        "{} {}",
                        path.display(),
                        state.id
                    );
                }
            }
        }
        assert!(moved > 0);
    }
}
