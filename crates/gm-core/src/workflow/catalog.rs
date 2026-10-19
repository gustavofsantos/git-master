//! The actions, wait events and triggers a workflow may name, with each
//! action's effect level and declared outcomes (docs/10, D63, D78). Static
//! in WF0; JSON Schema of inputs and outputs comes with WF1.

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
    },
    ActionSpec {
        name: "notify",
        level: Level::Read,
        outcomes: &["done", "failed"],
        phase: None,
    },
    ActionSpec {
        name: "approve",
        level: Level::Pause,
        outcomes: &["approved", "rejected", "failed"],
        phase: None,
    },
    ActionSpec {
        name: "exec",
        level: Level::Read,
        outcomes: &["done", "failed", "timeout"],
        phase: None,
    },
    ActionSpec {
        name: "commit.suggest_message",
        level: Level::Propose,
        outcomes: &["done", "failed"],
        phase: None,
    },
    ActionSpec {
        name: "stage.all",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
    },
    ActionSpec {
        name: "commit.create",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
    },
    ActionSpec {
        name: "branch.create",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
    },
    ActionSpec {
        name: "branch.checkout",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
    },
    ActionSpec {
        name: "fetch",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
    },
    ActionSpec {
        name: "set",
        level: Level::Propose,
        outcomes: &["done", "failed"],
        phase: Some("transform"),
    },
    ActionSpec {
        name: "stack.tree",
        level: Level::Read,
        outcomes: &["done", "failed"],
        phase: None,
    },
    ActionSpec {
        name: "stack.stale",
        level: Level::Read,
        outcomes: &["done", "failed"],
        phase: None,
    },
    ActionSpec {
        name: "restack.plan",
        level: Level::Propose,
        outcomes: &["done", "failed"],
        phase: None,
    },
    ActionSpec {
        name: "reparent.plan",
        level: Level::Propose,
        outcomes: &["done", "failed"],
        phase: None,
    },
    // No `interrupted`: restack has its own recovery (D14, D80).
    ActionSpec {
        name: "restack.apply",
        level: Level::Apply,
        outcomes: &["done", "conflict", "stale", "failed"],
        phase: None,
    },
    ActionSpec {
        name: "reparent.apply",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
    },
    ActionSpec {
        name: "restack.undo",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
    },
    ActionSpec {
        name: "push.lease",
        level: Level::Apply,
        outcomes: &["done", "failed", "interrupted"],
        phase: None,
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

/// Outcomes of a wait state on this event, without `timeout` (which exists
/// only when the state sets one).
pub fn wait_outcomes(event: &str) -> Option<&'static [&'static str]> {
    match event {
        "restack.resumed" => Some(&["resumed", "aborted"]),
        _ => None,
    }
}

/// Whether a workflow may use this as `trigger.on`.
pub fn is_known_trigger(on: &str) -> bool {
    if matches!(
        on,
        "manual"
            | "repo.opened"
            | "refs.changed"
            | "commit.created"
            | "checkout.done"
            | "fetch.done"
            | "restack.done"
            | "stack.parent_moved"
            | "stack.parent_merged"
            | "composer.opened"
    ) {
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
