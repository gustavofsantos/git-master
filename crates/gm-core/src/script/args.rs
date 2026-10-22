//! Checks the arguments of a `ctx` call against the catalog's fields, before any effect runs. The
//! message names the valid fields so a caller (an agent, D102) can correct the call from it.

use serde_json::Value;

use crate::workflow::catalog::{FieldKind, FieldSpec};
use crate::workflow::exec::parse_timeout;

/// `Ok` when `args` is an object whose keys are all fields, every required field is there and
/// every value has its field's type.
pub fn validate(func: &str, fields: &[FieldSpec], args: &Value) -> Result<(), String> {
    let Some(object) = args.as_object() else {
        return Err(format!("ctx.{func}: the arguments must be a table"));
    };
    if let Some(key) = object
        .keys()
        .find(|key| !fields.iter().any(|f| f.name == key.as_str()))
    {
        let valid: Vec<&str> = fields.iter().map(|f| f.name).collect();
        let valid = if valid.is_empty() {
            "none".to_owned()
        } else {
            valid.join(", ")
        };
        return Err(format!(
            "ctx.{func}: unknown field `{key}`. Valid fields: {valid}"
        ));
    }
    for field in fields {
        match object.get(field.name) {
            None | Some(Value::Null) if field.required => {
                return Err(format!(
                    "ctx.{func}: `{}` is required. {}",
                    field.name, field.help
                ));
            }
            None | Some(Value::Null) => {}
            Some(value) => check_kind(func, field, value)?,
        }
    }
    Ok(())
}

fn check_kind(func: &str, field: &FieldSpec, value: &Value) -> Result<(), String> {
    let wrong = |expected: &str| Err(format!("ctx.{func}: `{}` must be {expected}", field.name));
    match field.kind {
        FieldKind::Text => {
            if value.is_string() {
                Ok(())
            } else {
                wrong("text")
            }
        }
        FieldKind::TextList => match value.as_array() {
            Some(items) if items.iter().all(Value::is_string) => Ok(()),
            _ => wrong("a list of text"),
        },
        FieldKind::Bool => {
            if value.is_boolean() {
                Ok(())
            } else {
                wrong("true or false")
            }
        }
        FieldKind::Duration => match value.as_str().map(parse_timeout) {
            Some(Ok(_)) => Ok(()),
            _ => wrong("a duration such as 30s, 2m or 1h"),
        },
        FieldKind::Choice(options) => match value.as_str() {
            Some(text) if options.contains(&text) => Ok(()),
            _ => wrong(&format!("one of: {}", options.join(", "))),
        },
        FieldKind::Any => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::script::surface::CtxSurface;

    fn check(func: &str, args: Value) -> Result<(), String> {
        let surface = CtxSurface::from_catalog();
        validate(func, surface.get(func).unwrap().fields, &args)
    }

    #[test]
    fn a_complete_call_passes() {
        assert!(check("branch.create", json!({ "name": "x", "checkout": true })).is_ok());
        assert!(check("stage.all", json!({})).is_ok());
        assert!(
            check(
                "exec",
                json!({ "argv": ["git"], "timeout": "30s", "format": "json" })
            )
            .is_ok()
        );
    }

    #[test]
    fn a_missing_required_field_names_it() {
        let error = check("branch.create", json!({})).unwrap_err();
        assert!(error.contains("`name` is required"), "{error}");
    }

    #[test]
    fn an_unknown_field_lists_the_valid_ones() {
        let error = check("branch.create", json!({ "name": "x", "force": true })).unwrap_err();
        assert!(error.contains("unknown field `force`"), "{error}");
        assert!(error.contains("Valid fields: name, checkout"), "{error}");
    }

    #[test]
    fn wrong_types_are_refused() {
        assert!(check("branch.create", json!({ "name": 3 })).is_err());
        assert!(check("exec", json!({ "argv": "git status" })).is_err());
        assert!(check("exec", json!({ "argv": ["git"], "timeout": "soon" })).is_err());
        assert!(check("exec", json!({ "argv": ["git"], "format": "yaml" })).is_err());
        assert!(check("branch.create", json!([1])).is_err());
    }
}
