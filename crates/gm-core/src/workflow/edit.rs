//! Structural edits of a workflow file (design/workflow-scripts.md): add, remove and move a
//! top-level state, set its transitions, action and `with` fields, and edit
//! the trigger, the inputs and the name of the workflow. Text in, text out.
//! The file is edited with `toml_edit`, so comments and formatting of every
//! part the edit does not touch come back byte for byte. Only top-level
//! states are edited here; nested states of a `for_each` are edited in the
//! TOML tab.

use std::fmt;

use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use super::catalog::{self, FieldSpec};
use super::parse;
use super::validate::field_problem;

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

/// A value written into a `with` key. Its kind must fit the catalog field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldValue {
    Text(String),
    TextList(Vec<String>),
    Bool(bool),
}

/// Sets the action of the top-level action state `state`. Keys of its `with`
/// the new action does not know are removed, and a `with` left empty goes too.
/// `set` and actions with no fields keep `with` as it is. Refuses an unknown
/// action and a state that runs none.
pub fn set_action(source: &str, state: &str, action: &str) -> Result<String, EditError> {
    let spec =
        catalog::action(action).ok_or_else(|| EditError(format!("unknown action `{action}`")))?;
    let mut doc = document(source)?;
    let position = locate(&doc, state)?;
    let table = state_mut(&mut doc, position, state)?;
    let slot = table
        .get_mut("action")
        .and_then(Item::as_value_mut)
        .ok_or_else(|| EditError(format!("`{state}` is not an action state")))?;
    replace_string(slot, action);
    if action != "set" && !spec.fields.is_empty() {
        retain_with(table, |key| {
            spec.fields.iter().any(|field| field.name == key)
        });
    }
    finish(&doc, source)
}

/// Writes `value` into the `with` table of the top-level action state `state`,
/// creating the table when it is absent. The other keys keep their order. An
/// empty text for a key that is not required removes the key. The value must
/// fit the catalog field; `set` and actions with no fields take any key.
pub fn set_field(
    source: &str,
    state: &str,
    field: &str,
    value: &FieldValue,
) -> Result<String, EditError> {
    let mut doc = document(source)?;
    let position = locate(&doc, state)?;
    let table = state_mut(&mut doc, position, state)?;
    let spec = field_of(&action_of(table, state)?, field)?;
    let required = spec.is_some_and(|spec| spec.required);
    let empty_text = matches!(value, FieldValue::Text(text) if text.is_empty());
    if empty_text && !required {
        remove_with_key(table, field);
    } else {
        if let Some(spec) = spec
            && let Some(problem) = field_problem(&spec, &json_of(value))
        {
            return Err(EditError(format!("`with.{field}` {problem}")));
        }
        set_key(with_table(table)?, field, toml_value(value));
        tidy_with(table);
    }
    finish(&doc, source)
}

/// Removes `field` from the `with` table of the top-level state `state`. A
/// `with` left empty goes too. A missing key changes nothing.
pub fn remove_field(source: &str, state: &str, field: &str) -> Result<String, EditError> {
    let mut doc = document(source)?;
    let position = locate(&doc, state)?;
    remove_with_key(state_mut(&mut doc, position, state)?, field);
    finish(&doc, source)
}

/// Sets `[trigger] on`, and `match` to `matcher`. An empty or absent matcher
/// removes `match`. The `key` of the trigger is kept.
pub fn set_trigger(source: &str, on: &str, matcher: Option<&str>) -> Result<String, EditError> {
    if !catalog::is_known_trigger(on) {
        return Err(EditError(format!("unknown trigger `{on}`")));
    }
    let mut doc = document(source)?;
    let trigger = doc
        .get_mut("trigger")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| EditError("the file has no `[trigger]`".to_owned()))?;
    set_key(trigger, "on", Value::from(on));
    match matcher.filter(|text| !text.is_empty()) {
        Some(matcher) => set_key(trigger, "match", Value::from(matcher)),
        None => {
            trigger.remove("match");
        }
    }
    finish(&doc, source)
}

/// Adds or updates the input `name` under `[inputs]`, as
/// `name = { type = "string", prompt = "..." }`. The table is created before
/// `[trigger]` when absent. On an existing input only `prompt` changes; an
/// absent or empty prompt removes it.
pub fn set_input(source: &str, name: &str, prompt: Option<&str>) -> Result<String, EditError> {
    if !super::expr::is_ident(name) {
        return Err(invalid_name(name));
    }
    let mut doc = document(source)?;
    if doc.get("inputs").is_none() {
        open_inputs(&mut doc)?;
    }
    let inputs = doc
        .get_mut("inputs")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| EditError("`inputs` is not a table".to_owned()))?;
    let prompt = prompt.filter(|text| !text.is_empty());
    if let Some(entry) = inputs.get_mut(name) {
        let fields = entry
            .as_table_like_mut()
            .ok_or_else(|| EditError(format!("input `{name}` is not a table")))?;
        match prompt {
            Some(text) => set_key(fields, "prompt", Value::from(text)),
            None => {
                fields.remove("prompt");
            }
        }
        tidy(entry);
    } else {
        let mut fields = InlineTable::new();
        fields.insert("type", Value::from("string"));
        if let Some(text) = prompt {
            fields.insert("prompt", Value::from(text));
        }
        fields.fmt();
        inputs.insert(name, Item::Value(Value::InlineTable(fields)));
    }
    finish(&doc, source)
}

/// Removes the input `name`. An `[inputs]` table left empty goes too. A
/// reference to the input elsewhere is left; the validator reports it.
pub fn remove_input(source: &str, name: &str) -> Result<String, EditError> {
    let mut doc = document(source)?;
    let unknown = || EditError(format!("unknown input `{name}`"));
    let inputs = doc
        .get_mut("inputs")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(unknown)?;
    if inputs.remove(name).is_none() {
        return Err(unknown());
    }
    if inputs.is_empty() {
        doc.as_table_mut().remove("inputs");
    }
    finish(&doc, source)
}

/// Sets the top-level `name` and `description`. `name: None` keeps the name;
/// an empty or absent description removes the key. A name cannot be empty.
pub fn set_meta(
    source: &str,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<String, EditError> {
    let mut doc = document(source)?;
    let root = doc.as_table_mut();
    if let Some(name) = name {
        if name.is_empty() {
            return Err(EditError("a workflow needs a name".to_owned()));
        }
        set_key(root, "name", Value::from(name));
    }
    match description.filter(|text| !text.is_empty()) {
        Some(text) => set_key(root, "description", Value::from(text)),
        None => {
            root.remove("description");
        }
    }
    finish(&doc, source)
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
    replace_value(slot, Value::from(text));
}

/// Replaces a value and keeps the spacing and comments around it.
fn replace_value(slot: &mut Value, new: Value) {
    let decor = slot.decor().clone();
    *slot = new;
    *slot.decor_mut() = decor;
}

/// The top-level state at `position`, as a table.
fn state_mut<'d>(
    doc: &'d mut DocumentMut,
    position: usize,
    id: &str,
) -> Result<&'d mut Table, EditError> {
    states_mut(doc)?
        .get_mut(position)
        .ok_or_else(|| unknown_state(id))
}

/// The action a state runs; an error for a state that runs none.
fn action_of(table: &Table, state: &str) -> Result<String, EditError> {
    table
        .get("action")
        .and_then(Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| EditError(format!("`{state}` is not an action state")))
}

/// The catalog field `field` of `action`. `None` for `set` and for an action
/// with no fields, which take any key; an error for a key the action lacks.
fn field_of(action: &str, field: &str) -> Result<Option<FieldSpec>, EditError> {
    let spec =
        catalog::action(action).ok_or_else(|| EditError(format!("unknown action `{action}`")))?;
    if action == "set" || spec.fields.is_empty() {
        return Ok(None);
    }
    spec.fields
        .iter()
        .find(|known| known.name == field)
        .copied()
        .map(Some)
        .ok_or_else(|| EditError(format!("`{field}` is not a field of action `{action}`")))
}

/// The value as the validator reads it.
fn json_of(value: &FieldValue) -> serde_json::Value {
    match value {
        FieldValue::Text(text) => serde_json::Value::from(text.as_str()),
        FieldValue::TextList(items) => serde_json::Value::from(items.clone()),
        FieldValue::Bool(flag) => serde_json::Value::from(*flag),
    }
}

/// The value as TOML.
fn toml_value(value: &FieldValue) -> Value {
    match value {
        FieldValue::Text(text) => Value::from(text.as_str()),
        FieldValue::TextList(items) => Value::Array(items.iter().map(String::as_str).collect()),
        FieldValue::Bool(flag) => Value::from(*flag),
    }
}

/// The `with` table of a state, created as an empty inline table when absent.
fn with_table(table: &mut Table) -> Result<&mut dyn TableLike, EditError> {
    if table.get("with").is_none() {
        table.insert("with", Item::Value(Value::InlineTable(InlineTable::new())));
    }
    table
        .get_mut("with")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| EditError("`with` is not a table".to_owned()))
}

/// Sets `key` to `value`. An existing value keeps its spacing and comments; a
/// new key goes last in its table.
fn set_key(table: &mut dyn TableLike, key: &str, value: Value) {
    match table.get_mut(key) {
        Some(item) => match item.as_value_mut() {
            Some(slot) => replace_value(slot, value),
            None => *item = Item::Value(value),
        },
        None => {
            table.insert(key, Item::Value(value));
        }
    }
}

/// Removes `key` from the `with` table of a state. A `with` left empty goes
/// too; a missing key changes nothing.
fn remove_with_key(table: &mut Table, key: &str) {
    let Some(with) = table.get_mut("with").and_then(Item::as_table_like_mut) else {
        return;
    };
    if with.remove(key).is_none() {
        return;
    }
    if with.is_empty() {
        table.remove("with");
    } else {
        tidy_with(table);
    }
}

/// Removes the keys of a state's `with` that `keep` refuses. A `with` left
/// empty goes too; nothing changes when every key is kept.
fn retain_with(table: &mut Table, keep: impl Fn(&str) -> bool) {
    let Some(with) = table.get_mut("with").and_then(Item::as_table_like_mut) else {
        return;
    };
    let unknown: Vec<String> = with
        .iter()
        .map(|(key, _)| key.to_owned())
        .filter(|key| !keep(key.as_str()))
        .collect();
    if unknown.is_empty() {
        return;
    }
    for key in &unknown {
        with.remove(key);
    }
    if with.is_empty() {
        table.remove("with");
    } else {
        tidy_with(table);
    }
}

/// Normalizes the spacing of an inline table an edit changed, so that it reads
/// `{ a = 1, b = 2 }` whatever it held before. Other items are left alone.
fn tidy(item: &mut Item) {
    if let Item::Value(Value::InlineTable(table)) = item {
        table.fmt();
    }
}

/// `tidy` for the `with` item of a state, when it exists.
fn tidy_with(table: &mut Table) {
    if let Some(with) = table.get_mut("with") {
        tidy(with);
    }
}

/// Adds an empty `[inputs]` table right before `[trigger]`. TOML headers are
/// ordered by their position, so the headers from `[trigger]` on move down one.
fn open_inputs(doc: &mut DocumentMut) -> Result<(), EditError> {
    let at = doc
        .get("trigger")
        .and_then(Item::as_table)
        .and_then(Table::position)
        .ok_or_else(|| EditError("the file has no `[trigger]`".to_owned()))?;
    shift_positions(doc.as_table_mut(), at);
    let mut inputs = Table::new();
    inputs.set_position(Some(at));
    doc.insert("inputs", Item::Table(inputs));
    Ok(())
}

/// Moves every header at position `from` or later one position down, in
/// `table` and in the tables nested under it.
fn shift_positions(table: &mut Table, from: isize) {
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(child) => {
                bump_position(child, from);
                shift_positions(child, from);
            }
            Item::ArrayOfTables(children) => {
                for child in children.iter_mut() {
                    bump_position(child, from);
                    shift_positions(child, from);
                }
            }
            _ => {}
        }
    }
}

fn bump_position(table: &mut Table, from: isize) {
    if let Some(position) = table.position()
        && position >= from
    {
        table.set_position(Some(position + 1));
    }
}

fn invalid_name(name: &str) -> EditError {
    EditError(format!(
        "invalid input name `{name}`: use letters, digits and `_`, starting with a letter or `_`"
    ))
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

    /// A file with a commented trigger and two action states; `ask` carries
    /// a `with` table, `say` another.
    const FLOW: &str = "\
schema = 1
name = \"w\"

[trigger]
on = \"manual\" # when it runs
key = \"cmd-m\"

# The first step.
[[state]]
id = \"ask\" # inline note
action = \"exec\"
with = { argv = [\"llm\", \"-p\"], timeout = \"30s\" }
on.done = \"say\"

[[state]]
id = \"say\"
action = \"notify\"
with = { text = \"hi\" }
";

    /// A branch-creating state whose `with` holds keys of two actions.
    const BRANCH: &str = "\
schema = 1
name = \"w\"

[trigger]
on = \"manual\"

[[state]]
id = \"make\"
action = \"exec\"
with = { argv = [\"x\"], name = \"feat/a\", checkout = true }
";

    /// A state that runs `branch.create`, with a bool key.
    const CREATE: &str = "\
schema = 1
name = \"w\"

[trigger]
on = \"manual\"

[[state]]
id = \"make\"
action = \"branch.create\"
with = { name = \"feat/a\", checkout = true }
";

    #[test]
    fn set_action_to_the_same_action_changes_nothing() {
        assert_eq!(ok(set_action(FLOW, "ask", "exec")), FLOW);
    }

    #[test]
    fn set_action_keeps_with_for_an_action_with_no_fields() {
        let out = ok(set_action(FLOW, "ask", "fetch"));
        assert_eq!(
            out,
            FLOW.replacen("action = \"exec\"", "action = \"fetch\"", 1)
        );
    }

    #[test]
    fn set_action_drops_the_keys_the_new_action_does_not_know() {
        let out = ok(set_action(FLOW, "ask", "notify"));
        let expected = FLOW.replacen(
            "action = \"exec\"\nwith = { argv = [\"llm\", \"-p\"], timeout = \"30s\" }\n",
            "action = \"notify\"\n",
            1,
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn set_action_keeps_the_keys_the_new_action_knows() {
        let out = ok(set_action(BRANCH, "make", "branch.create"));
        let expected = BRANCH
            .replacen("action = \"exec\"", "action = \"branch.create\"", 1)
            .replacen(
                "with = { argv = [\"x\"], name = \"feat/a\", checkout = true }",
                "with = { name = \"feat/a\", checkout = true }",
                1,
            );
        assert_eq!(out, expected);
    }

    #[test]
    fn set_action_refuses_an_unknown_action_and_a_state_that_runs_none() {
        assert!(err(set_action(FLOW, "ask", "nope")).contains("unknown action `nope`"));
        let wait = "schema = 1\nname = \"w\"\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"w\"\nwait = \"restack.resumed\"\n";
        assert!(err(set_action(wait, "w", "noop")).contains("`w` is not an action state"));
    }

    #[test]
    fn set_field_adds_a_key_last_and_keeps_the_others() {
        let out = ok(set_field(
            FLOW,
            "ask",
            "stdin",
            &FieldValue::Text("hello".to_owned()),
        ));
        let expected = FLOW.replacen(
            "with = { argv = [\"llm\", \"-p\"], timeout = \"30s\" }",
            "with = { argv = [\"llm\", \"-p\"], timeout = \"30s\", stdin = \"hello\" }",
            1,
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn set_field_replaces_a_key_in_place() {
        let out = ok(set_field(
            FLOW,
            "ask",
            "timeout",
            &FieldValue::Text("2m".to_owned()),
        ));
        assert_eq!(
            out,
            FLOW.replacen("timeout = \"30s\"", "timeout = \"2m\"", 1)
        );
    }

    #[test]
    fn set_field_writes_a_list_and_a_bool() {
        let list = ok(set_field(
            FLOW,
            "ask",
            "argv",
            &FieldValue::TextList(vec!["git".to_owned(), "status".to_owned()]),
        ));
        assert_eq!(
            list,
            FLOW.replacen(
                "argv = [\"llm\", \"-p\"]",
                "argv = [\"git\", \"status\"]",
                1
            )
        );
        let flag = ok(set_field(
            CREATE,
            "make",
            "checkout",
            &FieldValue::Bool(false),
        ));
        assert_eq!(
            flag,
            CREATE.replacen("checkout = true", "checkout = false", 1)
        );
    }

    #[test]
    fn set_field_creates_the_with_table_when_absent() {
        let out = ok(set_field(
            COMMENTED,
            "a",
            "note",
            &FieldValue::Text("x".to_owned()),
        ));
        assert_eq!(
            out,
            COMMENTED.replacen(
                "on.done = \"b\"\n",
                "on.done = \"b\"\nwith = { note = \"x\" }\n",
                1
            )
        );
    }

    #[test]
    fn set_field_with_an_empty_text_removes_an_optional_key() {
        let out = ok(set_field(
            FLOW,
            "ask",
            "timeout",
            &FieldValue::Text(String::new()),
        ));
        assert_eq!(out, FLOW.replacen(", timeout = \"30s\"", "", 1));
    }

    #[test]
    fn set_field_refuses_a_value_that_does_not_fit() {
        let text = |value: &str| FieldValue::Text(value.to_owned());
        assert!(
            err(set_field(FLOW, "ask", "timeout", &text("10x")))
                .contains("`with.timeout` must be a positive whole number")
        );
        assert!(
            err(set_field(FLOW, "ask", "argv", &text("git")))
                .contains("`with.argv` must be an array of strings")
        );
        assert!(
            err(set_field(CREATE, "make", "checkout", &text("yes")))
                .contains("`with.checkout` must be true or false")
        );
        assert!(
            err(set_field(FLOW, "ask", "format", &text("yaml")))
                .contains("`with.format` must be one of \"json\"")
        );
        assert!(
            err(set_field(FLOW, "ask", "shell", &text("bash")))
                .contains("`shell` is not a field of action `exec`")
        );
    }

    #[test]
    fn remove_field_drops_one_key_and_an_emptied_with() {
        assert_eq!(
            ok(remove_field(FLOW, "ask", "argv")),
            FLOW.replacen("argv = [\"llm\", \"-p\"], ", "", 1)
        );
        assert_eq!(
            ok(remove_field(FLOW, "say", "text")),
            FLOW.replacen("with = { text = \"hi\" }\n", "", 1)
        );
        assert_eq!(ok(remove_field(FLOW, "ask", "stdin")), FLOW);
    }

    #[test]
    fn set_trigger_writes_on_and_match_and_keeps_the_key() {
        let matched = ok(set_trigger(FLOW, "before.restack", Some("branch:feat/*")));
        assert_eq!(
            matched,
            FLOW.replacen("on = \"manual\"", "on = \"before.restack\"", 1)
                .replacen(
                    "key = \"cmd-m\"\n",
                    "key = \"cmd-m\"\nmatch = \"branch:feat/*\"\n",
                    1
                )
        );
        assert_eq!(ok(set_trigger(&matched, "manual", None)), FLOW);
    }

    #[test]
    fn set_trigger_refuses_an_unknown_trigger() {
        assert!(err(set_trigger(FLOW, "nope", None)).contains("unknown trigger `nope`"));
    }

    #[test]
    fn set_input_creates_the_inputs_table_before_the_trigger() {
        let added = ok(set_input(FLOW, "ticket", Some("Ticket key")));
        assert_eq!(
            added,
            FLOW.replacen(
                "[trigger]\n",
                "[inputs]\nticket = { type = \"string\", prompt = \"Ticket key\" }\n\n[trigger]\n",
                1
            )
        );
        let bare = ok(set_input(&added, "ticket", None));
        assert!(bare.contains("ticket = { type = \"string\" }\n"), "{bare}");
        assert_eq!(ok(remove_input(&added, "ticket")), FLOW);
    }

    #[test]
    fn set_input_adds_to_an_existing_table_and_updates_a_prompt() {
        let source = "schema = 1\nname = \"w\"\n\n[inputs]\nfirst = { type = \"string\" }\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"a\"\naction = \"noop\"\n";
        let out = ok(set_input(source, "ticket", Some("Ticket")));
        assert_eq!(
            out,
            "schema = 1\nname = \"w\"\n\n[inputs]\nfirst = { type = \"string\" }\nticket = { type = \"string\", prompt = \"Ticket\" }\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"a\"\naction = \"noop\"\n"
        );
        let renamed = ok(set_input(&out, "ticket", Some("Issue")));
        assert!(renamed.contains("prompt = \"Issue\""), "{renamed}");
        assert!(!renamed.contains("prompt = \"Ticket\""), "{renamed}");
    }

    #[test]
    fn remove_input_drops_an_emptied_inputs_table() {
        let source = "schema = 1\nname = \"w\"\n\n[inputs]\nfirst = { type = \"string\" }\nsecond = { type = \"string\" }\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"a\"\naction = \"noop\"\n";
        let one = ok(remove_input(source, "first"));
        assert!(
            one.contains("[inputs]\nsecond = { type = \"string\" }\n"),
            "{one}"
        );
        let none = ok(remove_input(&one, "second"));
        assert!(!none.contains("[inputs]"), "{none}");
        assert!(err(remove_input(FLOW, "zz")).contains("unknown input `zz`"));
    }

    #[test]
    fn set_input_refuses_a_name_that_is_not_an_identifier() {
        assert!(err(set_input(FLOW, "bad-name", None)).contains("invalid input name `bad-name`"));
    }

    #[test]
    fn set_meta_writes_name_and_description_and_removes_an_empty_one() {
        let described = ok(set_meta(FLOW, Some("renamed"), Some("Does a thing")));
        assert_eq!(
            described,
            FLOW.replacen(
                "name = \"w\"\n",
                "name = \"renamed\"\ndescription = \"Does a thing\"\n",
                1
            )
        );
        assert_eq!(
            ok(set_meta(&described, None, None)),
            FLOW.replacen("name = \"w\"\n", "name = \"renamed\"\n", 1)
        );
        assert!(err(set_meta(FLOW, Some(""), None)).contains("needs a name"));
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
