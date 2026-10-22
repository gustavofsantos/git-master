//! Values across the boundary: Lua to JSON for the host and for the module's `meta` and `execute`
//! result, JSON to Lua for replies and for `input`. Only plain data crosses; a function, a thread,
//! a userdata or a buffer is refused.

use mlua::{Lua, Result as LuaResult, Table, Value};
use serde_json::{Map, Number, Value as Json};

/// Deeper tables are refused. It also stops a cyclic table from recursing forever.
const MAX_DEPTH: usize = 32;

/// Why a Lua value cannot cross. The display reads after "cannot pass", "execute returned" or
/// "M.meta cannot hold".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ConvError {
    /// A value of a type with no JSON form, such as `function`.
    Type(&'static str),
    /// A table with keys that are neither text nor a list without gaps.
    Keys,
    /// A number that is infinite or NaN.
    Number,
    /// Text that is not UTF-8.
    Text,
    /// Nested deeper than `MAX_DEPTH`, which includes cycles.
    Depth,
}

impl std::fmt::Display for ConvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Type(kind) => write!(f, "a {kind}"),
            Self::Keys => {
                f.write_str("a table whose keys are neither text nor a list without gaps")
            }
            Self::Number => f.write_str("a number that is not finite"),
            Self::Text => f.write_str("text that is not valid UTF-8"),
            Self::Depth => f.write_str("a table nested too deeply, or cyclic"),
        }
    }
}

/// The JSON of one Lua value.
pub(super) fn to_json(value: &Value) -> Result<Json, ConvError> {
    walk(value, 0)
}

fn walk(value: &Value, depth: usize) -> Result<Json, ConvError> {
    match value {
        Value::Nil => Ok(Json::Null),
        Value::Boolean(b) => Ok(Json::Bool(*b)),
        Value::Integer(i) => Ok(Json::from(*i)),
        Value::Number(n) => number(*n),
        Value::String(s) => s
            .to_str()
            .map(|text| Json::String(text.to_string()))
            .map_err(|_| ConvError::Text),
        Value::Table(table) => table_to_json(table, depth),
        other => Err(ConvError::Type(other.type_name())),
    }
}

fn number(n: f64) -> Result<Json, ConvError> {
    if !n.is_finite() {
        return Err(ConvError::Number);
    }
    // A whole number is sent as an integer, so `30` does not arrive as `30.0`. Rust prints a whole
    // f64 without a fraction, so the text parses as an integer exactly when the value is whole.
    if let Ok(whole) = n.to_string().parse::<i64>() {
        return Ok(Json::from(whole));
    }
    Number::from_f64(n)
        .map(Json::Number)
        .ok_or(ConvError::Number)
}

fn table_to_json(table: &Table, depth: usize) -> Result<Json, ConvError> {
    if depth >= MAX_DEPTH {
        return Err(ConvError::Depth);
    }
    let mut entries: Vec<(Value, Value)> = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        entries.push(pair.map_err(|_| ConvError::Keys)?);
    }
    if entries.is_empty() {
        return Ok(Json::Object(Map::new()));
    }
    if let Some(list) = as_list(&entries, depth)? {
        return Ok(Json::Array(list));
    }
    // Sorted by key, so the same table converts to the same JSON text whatever order Lua visits
    // it in and whatever features `serde_json` was built with (`preserve_order` keeps the order
    // of insertion). The journal, the dry run and the tests all read this text.
    let mut named: Vec<(String, &Value)> = Vec::with_capacity(entries.len());
    for (key, value) in &entries {
        let Value::String(key) = key else {
            return Err(ConvError::Keys);
        };
        let key = key.to_str().map_err(|_| ConvError::Text)?.to_string();
        named.push((key, value));
    }
    named.sort_by(|a, b| a.0.cmp(&b.0));
    let mut map = Map::new();
    for (key, value) in named {
        map.insert(key, walk(value, depth + 1)?);
    }
    Ok(Json::Object(map))
}

/// The entries as a list, when their keys are exactly 1 to n.
fn as_list(entries: &[(Value, Value)], depth: usize) -> Result<Option<Vec<Json>>, ConvError> {
    let n = entries.len();
    let mut slots: Vec<Option<Json>> = vec![None; n];
    for (key, value) in entries {
        // A key outside 1..=n makes this a map, not a list.
        let Some(slot) = list_index(key)
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| usize::try_from(index).ok())
            .filter(|slot| *slot < n)
        else {
            return Ok(None);
        };
        if slots[slot].is_some() {
            return Ok(None);
        }
        slots[slot] = Some(walk(value, depth + 1)?);
    }
    Ok(Some(
        slots.into_iter().map(|s| s.unwrap_or(Json::Null)).collect(),
    ))
}

/// The whole-number index a key names, if it is one.
fn list_index(key: &Value) -> Option<i64> {
    match key {
        Value::Integer(index) => Some(*index),
        Value::Number(index) => index.to_string().parse().ok(),
        _ => None,
    }
}

/// The Lua value of one JSON value. A frozen value has every table read-only, so a script
/// cannot change `input`.
pub(super) fn to_lua(lua: &Lua, json: &Json, frozen: bool) -> LuaResult<Value> {
    Ok(match json {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        Json::String(s) => Value::String(lua.create_string(s)?),
        Json::Array(items) => {
            let table = lua.create_table()?;
            for (i, item) in items.iter().enumerate() {
                table.raw_seti(i + 1, to_lua(lua, item, frozen)?)?;
            }
            seal(&table, frozen);
            Value::Table(table)
        }
        Json::Object(map) => {
            let table = lua.create_table()?;
            for (key, item) in map {
                table.raw_set(key.as_str(), to_lua(lua, item, frozen)?)?;
            }
            seal(&table, frozen);
            Value::Table(table)
        }
    })
}

fn seal(table: &Table, frozen: bool) {
    if frozen {
        table.set_readonly(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn eval(lua: &Lua, source: &str) -> Value {
        lua.load(source).eval().expect("chunk evaluates")
    }

    #[test]
    fn empty_table_is_an_empty_object_and_lists_are_arrays() {
        let lua = Lua::new();
        assert_eq!(to_json(&eval(&lua, "return {}")), Ok(json!({})));
        assert_eq!(to_json(&eval(&lua, "return {1, 2}")), Ok(json!([1, 2])));
        assert_eq!(
            to_json(&eval(&lua, "return {a = {b = {'x', true}}, n = 1.5}")),
            Ok(json!({"a": {"b": ["x", true]}, "n": 1.5}))
        );
    }

    #[test]
    fn whole_numbers_are_integers() {
        let lua = Lua::new();
        assert_eq!(to_json(&eval(&lua, "return 30")), Ok(json!(30)));
        assert_eq!(to_json(&eval(&lua, "return -2")), Ok(json!(-2)));
    }

    #[test]
    fn functions_threads_and_gaps_are_refused() {
        let lua = Lua::new();
        assert_eq!(
            to_json(&eval(&lua, "return function() end")),
            Err(ConvError::Type("function"))
        );
        assert_eq!(
            to_json(&eval(&lua, "return {f = coroutine.create(function() end)}")),
            Err(ConvError::Type("thread"))
        );
        assert_eq!(
            to_json(&eval(&lua, "return {1, nil, 3}")),
            Err(ConvError::Keys)
        );
        assert_eq!(
            to_json(&eval(&lua, "return {[true] = 1}")),
            Err(ConvError::Keys)
        );
    }

    #[test]
    fn a_cycle_is_refused_by_depth() {
        let lua = Lua::new();
        let value = eval(&lua, "local t = {} t.self = t return t");
        assert_eq!(to_json(&value), Err(ConvError::Depth));
    }

    #[test]
    fn json_round_trips_and_freezes() {
        let lua = Lua::new();
        let original = json!({"inputs": {"ticket": "ABC-1"}, "list": [1, "two", 3.5]});
        let value = to_lua(&lua, &original, true).unwrap();
        assert_eq!(to_json(&value), Ok(original));
        let Value::Table(table) = value else {
            panic!("object expected");
        };
        assert!(table.is_readonly());
        let inner: Table = table.get("inputs").unwrap();
        assert!(inner.is_readonly());
    }
}
