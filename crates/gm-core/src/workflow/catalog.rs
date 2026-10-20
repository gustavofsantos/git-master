//! The actions, wait events and triggers a workflow may name, with each
//! action's effect level, declared outcomes (docs/10, D63, D78) and the typed
//! `with` fields the driver reads. Static in WF0; JSON Schema of inputs and
//! outputs comes with WF1.

/// Effect level of an action (D20, D64).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Read,
    Propose,
    Apply,
    /// `approve`: pauses for the human.
    Pause,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionSpec {
    pub name: &'static str,
    pub level: Level,
    /// Every outcome the action can end with, including `done` and `failed`
    /// (and `interrupted` for apply actions, D80).
    pub outcomes: &'static [&'static str],
    /// The trigger phase the action is limited to, such as `transform` for
    /// `set`; `None` when it is valid everywhere.
    pub phase: Option<&'static str>,
    /// The `with` keys the driver reads, typed. Empty when the action reads
    /// none, or when the driver does not run it yet; the validator checks
    /// nothing for an action with no fields.
    pub fields: &'static [FieldSpec],
}

/// One `with` key of an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldSpec {
    pub name: &'static str,
    pub kind: FieldKind,
    pub required: bool,
    /// One line for the editor, in English.
    pub help: &'static str,
}

/// The type of a `with` value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// A string; may hold `${...}` templates.
    Text,
    /// An array of strings, such as an argv; each item may hold templates.
    TextList,
    Bool,
    /// `<positive integer><s|m|h>`, such as `30s`.
    Duration,
    /// A string that is one of these values.
    Choice(&'static [&'static str]),
    /// Any value: the action takes the whole table.
    Any,
}

/// Outcomes with a default transition when the file writes none (D78):
/// `done` goes to the next state; `failed`, `interrupted` and `timeout` to
/// `end.failed`.
pub const DEFAULTED: &[&str] = &["done", "failed", "interrupted", "timeout"];

/// Phases a `before.<op>` or `after.<op>` trigger may name.
const PHASE_OPS: &[&str] = &["commit", "restack", "branch_create", "conflict_resolve"];

/// Fields a `transform.<field>` trigger may name.
const TRANSFORM_FIELDS: &[&str] = &["commit_message", "branch_name", "conflict_hunk"];

/// Every action, in catalog order.
static ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "noop",
        level: Level::Read,
        outcomes: &["done", "failed"],
        phase: None,
        fields: &[],
    },
    ActionSpec {
        name: "notify",
        level: Level::Read,
        outcomes: &["done", "failed"],
        phase: None,
        fields: &[FieldSpec {
            name: "text",
            kind: FieldKind::Text,
            required: true,
            help: "The line the notification shows.",
        }],
    },
    ActionSpec {
        name: "approve",
        level: Level::Pause,
        outcomes: &["approved", "rejected", "failed"],
        phase: None,
        fields: &[FieldSpec {
            name: "summary",
            kind: FieldKind::Text,
            required: false,
            help: "What the person is asked to approve.",
        }],
    },
    ActionSpec {
        name: "exec",
        level: Level::Read,
        outcomes: &["done", "failed", "timeout"],
        phase: None,
        fields: &[
            FieldSpec {
                name: "argv",
                kind: FieldKind::TextList,
                required: true,
                help: "The program and its arguments. Run directly, never through a shell.",
            },
            FieldSpec {
                name: "stdin",
                kind: FieldKind::Text,
                required: false,
                help: "Text written to the program's standard input.",
            },
            FieldSpec {
                name: "timeout",
                kind: FieldKind::Duration,
                required: false,
                help: "How long the program may run, such as `30s`, `2m` or `1h`. Default `30s`.",
            },
            FieldSpec {
                name: "format",
                kind: FieldKind::Choice(&["json"]),
                required: false,
                help: "`json` parses stdout and exposes it as `output.json`.",
            },
        ],
    },
    ActionSpec {
        name: "commit.suggest_message",
        level: Level::Propose,
        outcomes: &["done", "failed"],
        phase: None,
        fields: &[FieldSpec {
            name: "message",
            kind: FieldKind::Text,
            required: true,
            help: "The suggested commit message, usually `${states.<id>.output.text}`.",
        }],
    },
    // The driver runs `stage.all` with no `with` key.
    ActionSpec {
        name: "stage.all",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
        fields: &[],
    },
    ActionSpec {
        name: "commit.create",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
        fields: &[FieldSpec {
            name: "message",
            kind: FieldKind::Text,
            required: true,
            help: "The commit message.",
        }],
    },
    ActionSpec {
        name: "branch.create",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
        fields: &[
            FieldSpec {
                name: "name",
                kind: FieldKind::Text,
                required: true,
                help: "The name of the new branch.",
            },
            FieldSpec {
                name: "checkout",
                kind: FieldKind::Bool,
                required: false,
                help: "`true` to check out the new branch. Default `false`.",
            },
        ],
    },
    ActionSpec {
        name: "branch.checkout",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
        fields: &[FieldSpec {
            name: "branch",
            kind: FieldKind::Text,
            required: true,
            help: "The branch to check out.",
        }],
    },
    ActionSpec {
        name: "fetch",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
        fields: &[], // Not run by the driver yet.
    },
    ActionSpec {
        name: "set",
        level: Level::Propose,
        outcomes: &["done", "failed"],
        phase: Some("transform"),
        // `set` returns its whole `with` table, so the validator does not check it.
        fields: &[FieldSpec {
            name: "value",
            kind: FieldKind::Any,
            required: true,
            help: "The value the transform returns.",
        }],
    },
    ActionSpec {
        name: "stack.tree",
        level: Level::Read,
        outcomes: &["done", "failed"],
        phase: None,
        fields: &[], // Not run by the driver yet.
    },
    ActionSpec {
        name: "stack.stale",
        level: Level::Read,
        outcomes: &["done", "failed"],
        phase: None,
        fields: &[], // Not run by the driver yet.
    },
    ActionSpec {
        name: "restack.plan",
        level: Level::Propose,
        outcomes: &["done", "failed"],
        phase: None,
        fields: &[], // Not run by the driver yet.
    },
    ActionSpec {
        name: "reparent.plan",
        level: Level::Propose,
        outcomes: &["done", "failed"],
        phase: None,
        fields: &[], // Not run by the driver yet.
    },
    // No `interrupted`: restack has its own recovery (D14, D80).
    ActionSpec {
        name: "restack.apply",
        level: Level::Apply,
        outcomes: &["done", "conflict", "stale", "failed"],
        phase: None,
        fields: &[], // Not run by the driver yet.
    },
    ActionSpec {
        name: "reparent.apply",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
        fields: &[], // Not run by the driver yet.
    },
    ActionSpec {
        name: "restack.undo",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
        fields: &[], // Not run by the driver yet.
    },
    ActionSpec {
        name: "push.lease",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
        fields: &[], // Not run by the driver yet.
    },
];

/// Every action the catalog knows, in table order (the editor's action picker
/// lists them in this order).
pub fn actions() -> &'static [ActionSpec] {
    ACTIONS
}

/// The action with this name.
pub fn action(name: &str) -> Option<&'static ActionSpec> {
    ACTIONS.iter().find(|spec| spec.name == name)
}

/// The group an action sits in when a picker lists them: the part before the
/// first `.`, or `general` for a bare name such as `noop` or `exec`.
#[must_use]
pub fn domain(action: &str) -> &str {
    action
        .split_once('.')
        .map_or("general", |(domain, _)| domain)
}

/// Outcomes of a wait state on this event, without `timeout` (which exists
/// only when the state sets one).
pub fn wait_outcomes(event: &str) -> Option<&'static [&'static str]> {
    match event {
        "restack.resumed" => Some(&["resumed", "aborted"]),
        _ => None,
    }
}

/// The events a workflow may name as `trigger.on` without a phase or a field.
const EVENTS: &[&str] = &[
    "manual",
    "repo.opened",
    "refs.changed",
    "commit.created",
    "checkout.done",
    "fetch.done",
    "restack.done",
    "stack.parent_moved",
    "stack.parent_merged",
    "composer.opened",
];

/// Every trigger a workflow may name, in the order a picker lists them: the
/// events, then `before.<op>` and `after.<op>` for each phase, then
/// `transform.<field>` for each field.
#[must_use]
pub fn triggers() -> Vec<String> {
    let mut all: Vec<String> = EVENTS.iter().map(|on| (*on).to_owned()).collect();
    for prefix in ["before", "after"] {
        all.extend(PHASE_OPS.iter().map(|op| format!("{prefix}.{op}")));
    }
    all.extend(
        TRANSFORM_FIELDS
            .iter()
            .map(|field| format!("transform.{field}")),
    );
    all
}

/// Whether a workflow may use this as `trigger.on`.
pub fn is_known_trigger(on: &str) -> bool {
    if EVENTS.contains(&on) {
        return true;
    }
    if let Some(op) = on
        .strip_prefix("before.")
        .or_else(|| on.strip_prefix("after."))
    {
        return PHASE_OPS.contains(&op);
    }
    on.strip_prefix("transform.")
        .is_some_and(|field| TRANSFORM_FIELDS.contains(&field))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_action_can_fail() {
        for spec in actions() {
            assert!(
                spec.outcomes.contains(&"failed"),
                "{} has no `failed` outcome",
                spec.name
            );
        }
    }

    #[test]
    fn only_apply_actions_other_than_restack_apply_can_be_interrupted() {
        for spec in actions() {
            let expected = spec.level == Level::Apply && spec.name != "restack.apply";
            assert_eq!(
                spec.outcomes.contains(&"interrupted"),
                expected,
                "{} interrupted mismatch",
                spec.name
            );
        }
    }

    #[test]
    fn restack_apply_declares_its_own_outcomes() {
        let spec = action("restack.apply").expect("restack.apply exists");
        assert_eq!(spec.outcomes, &["done", "conflict", "stale", "failed"]);
    }

    #[test]
    fn action_names_are_unique() {
        let mut seen = HashSet::new();
        for spec in actions() {
            assert!(seen.insert(spec.name), "duplicate action {}", spec.name);
        }
    }

    #[test]
    fn set_is_limited_to_transform() {
        assert_eq!(action("set").expect("set exists").phase, Some("transform"));
        for spec in actions().iter().filter(|spec| spec.name != "set") {
            assert_eq!(spec.phase, None, "{} must not be phase-limited", spec.name);
        }
    }

    #[test]
    fn unknown_action_is_none() {
        assert!(action("nope").is_none());
    }

    #[test]
    fn every_listed_action_resolves_to_itself() {
        for spec in actions() {
            assert_eq!(action(spec.name), Some(spec));
        }
    }

    #[test]
    fn field_names_are_unique_and_documented() {
        for spec in actions() {
            let mut seen = HashSet::new();
            for field in spec.fields {
                assert!(
                    seen.insert(field.name),
                    "{} has field {} twice",
                    spec.name,
                    field.name
                );
                assert!(
                    !field.help.is_empty(),
                    "{}.{} has no help",
                    spec.name,
                    field.name
                );
                if let FieldKind::Choice(choices) = field.kind {
                    assert!(
                        !choices.is_empty(),
                        "{}.{} has no choices",
                        spec.name,
                        field.name
                    );
                }
            }
        }
    }

    #[test]
    fn fields_of_the_driven_actions_match_the_driver() {
        let required = |name: &str| -> Vec<&str> {
            action(name)
                .expect("action exists")
                .fields
                .iter()
                .filter(|field| field.required)
                .map(|field| field.name)
                .collect()
        };
        assert_eq!(required("notify"), ["text"]);
        assert_eq!(required("exec"), ["argv"]);
        assert_eq!(required("commit.suggest_message"), ["message"]);
        assert_eq!(required("commit.create"), ["message"]);
        assert_eq!(required("branch.create"), ["name"]);
        assert_eq!(required("branch.checkout"), ["branch"]);
        assert!(required("approve").is_empty());
        assert!(action("stage.all").expect("stage.all").fields.is_empty());
        assert!(action("noop").expect("noop").fields.is_empty());
    }

    #[test]
    fn exec_format_takes_only_json() {
        let format = action("exec")
            .expect("exec")
            .fields
            .iter()
            .find(|field| field.name == "format")
            .expect("format");
        assert_eq!(format.kind, FieldKind::Choice(&["json"]));
    }

    #[test]
    fn domain_is_the_part_before_the_first_dot() {
        assert_eq!(domain("commit.create"), "commit");
        assert_eq!(domain("restack.plan"), "restack");
        assert_eq!(domain("stack.tree"), "stack");
        assert_eq!(domain("noop"), "general");
        assert_eq!(domain("exec"), "general");
        assert_eq!(domain("set"), "general");
    }

    #[test]
    fn restack_resumed_waits_on_resumed_or_aborted() {
        assert_eq!(
            wait_outcomes("restack.resumed"),
            Some(&["resumed", "aborted"][..])
        );
        assert!(wait_outcomes("restack.nope").is_none());
    }

    #[test]
    fn plain_triggers_are_known() {
        for on in [
            "manual",
            "repo.opened",
            "refs.changed",
            "commit.created",
            "checkout.done",
            "fetch.done",
            "restack.done",
            "stack.parent_moved",
            "stack.parent_merged",
            "composer.opened",
        ] {
            assert!(is_known_trigger(on), "{on} should be known");
        }
    }

    #[test]
    fn phase_triggers_are_known_for_each_op() {
        for op in ["commit", "restack", "branch_create", "conflict_resolve"] {
            assert!(is_known_trigger(&format!("before.{op}")), "before.{op}");
            assert!(is_known_trigger(&format!("after.{op}")), "after.{op}");
        }
        assert!(is_known_trigger("before.restack"));
    }

    #[test]
    fn every_listed_trigger_is_known_and_the_list_is_complete() {
        let all = triggers();
        assert_eq!(all.first().map(String::as_str), Some("manual"));
        // Ten events, four phase ops twice, three fields.
        assert_eq!(all.len(), 10 + 2 * 4 + 3);
        for on in &all {
            assert!(is_known_trigger(on), "{on} should be known");
        }
    }

    #[test]
    fn transform_triggers_are_known_for_each_field() {
        for field in ["commit_message", "branch_name", "conflict_hunk"] {
            assert!(is_known_trigger(&format!("transform.{field}")), "{field}");
        }
        assert!(is_known_trigger("transform.commit_message"));
    }

    #[test]
    fn unknown_triggers_are_rejected() {
        for on in [
            "before.nope",
            "after.nope",
            "before.",
            "transform.",
            "transform.nope",
            "before.after.commit",
            "manuals",
            "manual ",
            "",
        ] {
            assert!(!is_known_trigger(on), "{on:?} should be rejected");
        }
    }
}
