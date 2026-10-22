//! The shape of `ctx`, generated from the action catalog (design/workflow-scripts.md, D98): each
//! catalog action becomes a function with the same fields, level and outcomes, so the script
//! surface cannot drift from the engine's actions. The VM adapter builds the Lua tables from
//! this, the host validates arguments against it, and `.api/` documentation (WS1) is written from
//! it.

use crate::workflow::catalog::{self, ActionSpec, FieldKind, FieldSpec, Level};

/// How a call takes its arguments in Lua.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgShape {
    /// One optional table keyed by field name: `ctx.branch.create({ name = "x" })`.
    Table,
    /// The listed fields are positional, in order, and an optional table after them carries the
    /// remaining fields: `ctx.exec(argv, { timeout = "30s" })`.
    Positional(&'static [&'static str]),
}

/// One function of `ctx`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CtxFn {
    /// The catalog name, such as `branch.create`. The Lua path is `ctx.` plus this.
    pub path: &'static str,
    pub level: Level,
    pub outcomes: &'static [&'static str],
    pub fields: &'static [FieldSpec],
    pub shape: ArgShape,
}

/// The functions of `ctx` that are not catalog actions. They have no effect level.
pub const HELPERS: &[&str] = &["now", "random", "block", "fail", "try"];

/// Helpers that call the host, in the order the VM registers them. `try` is the VM's own.
pub const HOST_HELPERS: &[&str] = &["now", "random", "block", "fail"];

/// The one event `ctx.wait` knows in v1 (the catalog's `wait_outcomes`).
pub const WAIT_EVENTS: &[&str] = &["restack.resumed"];

static WAIT_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "event",
        kind: FieldKind::Choice(WAIT_EVENTS),
        required: true,
        help: "The event to wait for.",
    },
    FieldSpec {
        name: "timeout",
        kind: FieldKind::Duration,
        required: false,
        help: "How long to wait, such as 24h. Default: no timeout.",
    },
];

/// Outcomes of `ctx.wait`. All three are returned to the script as `{ outcome = ... }`; none
/// raises (D105).
pub const WAIT_OUTCOMES: &[&str] = &["resumed", "aborted", "timeout"];

/// Catalog actions that exist only for the old TOML state machine.
const NOT_IN_CTX: &[&str] = &["noop", "set"];

/// Positional shapes by catalog name. Every other action takes one table.
fn shape_of(name: &str) -> ArgShape {
    match name {
        "notify" => ArgShape::Positional(&["text"]),
        "approve" => ArgShape::Positional(&["summary"]),
        "exec" => ArgShape::Positional(&["argv"]),
        "wait" => ArgShape::Positional(&["event"]),
        _ => ArgShape::Table,
    }
}

/// Every function of `ctx`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CtxSurface {
    functions: Vec<CtxFn>,
}

impl CtxSurface {
    /// The surface of the current catalog: its actions, minus the ones the state machine alone
    /// used, plus `wait`.
    #[must_use]
    pub fn from_catalog() -> Self {
        let mut functions: Vec<CtxFn> = catalog::actions()
            .iter()
            .filter(|spec| !NOT_IN_CTX.contains(&spec.name))
            .map(Self::function)
            .collect();
        functions.push(CtxFn {
            path: "wait",
            level: Level::Pause,
            outcomes: WAIT_OUTCOMES,
            fields: WAIT_FIELDS,
            shape: shape_of("wait"),
        });
        Self { functions }
    }

    fn function(spec: &ActionSpec) -> CtxFn {
        CtxFn {
            path: spec.name,
            level: spec.level,
            outcomes: spec.outcomes,
            fields: spec.fields,
            shape: shape_of(spec.name),
        }
    }

    #[must_use]
    pub fn functions(&self) -> &[CtxFn] {
        &self.functions
    }

    /// The function at this catalog name.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&CtxFn> {
        self.functions.iter().find(|f| f.path == path)
    }

    /// The names directly under `prefix` (`""` for `ctx` itself): functions and namespaces, sorted
    /// and without duplicates. A namespace is the part before the next dot, so `branch` for
    /// `branch.create`. The helpers are listed at the top level.
    #[must_use]
    pub fn names_under(&self, prefix: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for f in &self.functions {
            let rest = if prefix.is_empty() {
                Some(f.path)
            } else {
                f.path
                    .strip_prefix(prefix)
                    .and_then(|rest| rest.strip_prefix('.'))
            };
            if let Some(rest) = rest {
                let head = rest.split('.').next().unwrap_or(rest);
                names.push(head.to_owned());
            }
        }
        if prefix.is_empty() {
            names.extend(HELPERS.iter().map(|h| (*h).to_owned()));
        }
        names.sort();
        names.dedup();
        names
    }

    /// Whether `prefix` names a namespace (`branch`), not a function.
    #[must_use]
    pub fn is_namespace(&self, prefix: &str) -> bool {
        self.functions.iter().any(|f| {
            f.path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('.'))
        })
    }

    /// The error text for an unknown name at `prefix`: it lists the valid names at that level, so
    /// a caller can correct itself from the message (D102).
    #[must_use]
    pub fn unknown_name_message(&self, prefix: &str, name: &str) -> String {
        let path = if prefix.is_empty() {
            format!("ctx.{name}")
        } else {
            format!("ctx.{prefix}.{name}")
        };
        let level = if prefix.is_empty() {
            "ctx".to_owned()
        } else {
            format!("ctx.{prefix}")
        };
        format!(
            "{path} does not exist. Valid names at {level}: {}",
            self.names_under(prefix).join(", ")
        )
    }
}

impl CtxFn {
    /// The names of the positional arguments, in order.
    #[must_use]
    pub fn positional(&self) -> &'static [&'static str] {
        match self.shape {
            ArgShape::Table => &[],
            ArgShape::Positional(names) => names,
        }
    }

    /// The Lua signature, for the generated documentation: `ctx.exec(argv, opts)`.
    #[must_use]
    pub fn signature(&self) -> String {
        match &self.shape {
            ArgShape::Table => {
                if self.fields.is_empty() {
                    format!("ctx.{}()", self.path)
                } else {
                    let names: Vec<&str> = self.fields.iter().map(|f| f.name).collect();
                    format!("ctx.{}({{ {} }})", self.path, names.join(", "))
                }
            }
            ArgShape::Positional(names) => {
                let rest = self.fields.iter().any(|f| !names.contains(&f.name));
                let mut args: Vec<&str> = names.to_vec();
                if rest {
                    args.push("opts");
                }
                format!("ctx.{}({})", self.path, args.join(", "))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_action_is_a_ctx_function_but_the_state_machine_ones() {
        let surface = CtxSurface::from_catalog();
        for spec in catalog::actions() {
            let in_ctx = surface.get(spec.name).is_some();
            assert_eq!(in_ctx, !NOT_IN_CTX.contains(&spec.name), "{}", spec.name);
        }
        assert!(surface.get("wait").is_some());
        assert!(surface.get("set").is_none());
        assert!(surface.get("noop").is_none());
    }

    #[test]
    fn levels_and_outcomes_come_from_the_catalog() {
        let surface = CtxSurface::from_catalog();
        let restack = surface.get("restack.apply").unwrap();
        assert_eq!(restack.level, Level::Apply);
        assert_eq!(restack.outcomes, &["done", "conflict", "stale", "failed"]);
        assert_eq!(surface.get("approve").unwrap().level, Level::Pause);
        assert_eq!(surface.get("exec").unwrap().level, Level::Read);
    }

    #[test]
    fn positional_calls_are_the_four_the_docs_name() {
        let surface = CtxSurface::from_catalog();
        let positional: Vec<&str> = surface
            .functions()
            .iter()
            .filter(|f| matches!(f.shape, ArgShape::Positional(_)))
            .map(|f| f.path)
            .collect();
        assert_eq!(positional, ["notify", "approve", "exec", "wait"]);
        // A positional name is a field of the action.
        for f in surface.functions() {
            for name in f.positional() {
                assert!(
                    f.fields.iter().any(|field| field.name == *name),
                    "{}",
                    f.path
                );
            }
        }
    }

    #[test]
    fn names_under_lists_functions_namespaces_and_helpers() {
        let surface = CtxSurface::from_catalog();
        let top = surface.names_under("");
        for name in ["branch", "exec", "now", "try", "wait", "restack", "fetch"] {
            assert!(top.contains(&name.to_owned()), "{name} in {top:?}");
        }
        assert_eq!(surface.names_under("branch"), ["checkout", "create"]);
        assert!(surface.is_namespace("restack"));
        assert!(!surface.is_namespace("exec"));
    }

    #[test]
    fn an_unknown_name_lists_the_valid_ones() {
        let surface = CtxSurface::from_catalog();
        let message = surface.unknown_name_message("branch", "make");
        assert_eq!(
            message,
            "ctx.branch.make does not exist. Valid names at ctx.branch: checkout, create"
        );
        assert!(
            surface
                .unknown_name_message("", "nope")
                .contains("Valid names at ctx:")
        );
    }

    #[test]
    fn signatures_match_the_documented_forms() {
        let surface = CtxSurface::from_catalog();
        assert_eq!(
            surface.get("exec").unwrap().signature(),
            "ctx.exec(argv, opts)"
        );
        assert_eq!(
            surface.get("notify").unwrap().signature(),
            "ctx.notify(text)"
        );
        assert_eq!(
            surface.get("stage.all").unwrap().signature(),
            "ctx.stage.all()"
        );
        assert_eq!(
            surface.get("branch.create").unwrap().signature(),
            "ctx.branch.create({ name, checkout })"
        );
        assert_eq!(
            surface.get("wait").unwrap().signature(),
            "ctx.wait(event, opts)"
        );
    }
}
