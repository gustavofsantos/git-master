//! The keyboard shortcuts of manual workflows (`trigger.key`, checked by `M.meta` in the engine):
//! one spelling of a shortcut, so two spellings of one key compare equal, the clash rule, and the
//! plan of the bindings the window makes. Plain data and pure functions; the bindings themselves
//! are made in `lib.rs`, and the warnings are shown by the Script tab.

/// One shortcut that will run a workflow: the workflow and the shortcut as the window binds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowKey {
    pub workflow: String,
    /// The shortcut as `M.meta` wrote it, such as `secondary-shift-b`.
    pub keystroke: String,
}

/// A shortcut that is not bound, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyWarning {
    pub workflow: String,
    /// Such as `key secondary-s is already used by Save Workflow`.
    pub message: String,
}

/// The bindings of the manual workflows, and the shortcuts that were left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyPlan {
    pub bound: Vec<WorkflowKey>,
    pub warnings: Vec<KeyWarning>,
}

/// The modifiers in the order `canonical` writes them.
const ORDER: [&str; 4] = ["ctrl", "alt", "shift", "cmd"];

/// A shortcut in one spelling, so that two spellings of the same key compare equal: `secondary`
/// is `cmd` on macOS and `ctrl` elsewhere, the modifiers come in one order and the key is lower
/// case. `None` when the text is not a shortcut.
#[must_use]
pub fn canonical(keystroke: &str, mac: bool) -> Option<String> {
    let parts: Vec<&str> = keystroke.split('-').collect();
    let (key, modifiers) = parts.split_last()?;
    if key.is_empty() {
        return None;
    }
    let mut on = [false; 4];
    for modifier in modifiers {
        let name = match *modifier {
            "secondary" if mac => "cmd",
            "secondary" => "ctrl",
            other => other,
        };
        let index = ORDER.iter().position(|known| *known == name)?;
        on[index] = true;
    }
    let mut text = String::new();
    for (name, _) in ORDER.iter().zip(on).filter(|(_, on)| *on) {
        text.push_str(name);
        text.push('-');
    }
    text.push_str(&key.to_lowercase());
    Some(text)
}

/// The shortcuts of the manual workflows, given as `(workflow, shortcut)` in listing order, and
/// the bindings the window already has, given as `(shortcut, what it does)`. A shortcut that
/// clashes with a binding of the window, or with an earlier workflow's, is not bound: its
/// warning says what uses the key. The first workflow keeps a key two of them ask for.
#[must_use]
pub fn plan(manual: &[(String, String)], built_in: &[(String, String)], mac: bool) -> KeyPlan {
    // The owner of each shortcut taken so far, by its spelling.
    let mut taken: Vec<(String, String)> = built_in
        .iter()
        .filter_map(|(keystroke, what)| Some((canonical(keystroke, mac)?, what.clone())))
        .collect();
    let mut plan = KeyPlan::default();
    for (workflow, keystroke) in manual {
        let Some(spelling) = canonical(keystroke, mac) else {
            plan.warnings.push(KeyWarning {
                workflow: workflow.clone(),
                message: format!("key {keystroke} is not a shortcut"),
            });
            continue;
        };
        if let Some((_, owner)) = taken.iter().find(|(taken, _)| *taken == spelling) {
            plan.warnings.push(KeyWarning {
                workflow: workflow.clone(),
                message: format!("key {keystroke} is already used by {owner}"),
            });
            continue;
        }
        taken.push((spelling, format!("workflow {workflow}")));
        plan.bound.push(WorkflowKey {
            workflow: workflow.clone(),
            keystroke: keystroke.clone(),
        });
    }
    plan
}

/// An action's name in words, as the menus write it: `gitmaster::OpenPalette` is `Open Palette`.
#[must_use]
pub fn action_words(name: &str) -> String {
    let short = name.rsplit("::").next().unwrap_or(name);
    let mut words = String::with_capacity(short.len() + 4);
    for (index, c) in short.chars().enumerate() {
        if index > 0 && c.is_uppercase() {
            words.push(' ');
        }
        words.push(c);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    #[test]
    fn two_spellings_of_one_shortcut_are_the_same() {
        assert_eq!(
            canonical("secondary-shift-b", false).as_deref(),
            Some("ctrl-shift-b")
        );
        assert_eq!(
            canonical("shift-secondary-B", false),
            canonical("secondary-shift-b", false)
        );
        assert_eq!(
            canonical("alt-secondary-c", false),
            canonical("ctrl-alt-c", false)
        );
        assert_eq!(
            canonical("secondary-shift-b", true).as_deref(),
            Some("shift-cmd-b")
        );
        assert_eq!(
            canonical("ctrl-secondary-b", false).as_deref(),
            Some("ctrl-b")
        );
    }

    #[test]
    fn text_that_is_not_a_shortcut_has_no_spelling() {
        assert_eq!(canonical("hyper-b", false), None);
        assert_eq!(canonical("ctrl-", false), None);
        assert_eq!(canonical("", false), None);
    }

    #[test]
    fn a_free_shortcut_is_bound_in_listing_order() {
        let manual = pairs(&[
            ("first", "secondary-shift-b"),
            ("second", "alt-secondary-x"),
        ]);
        let plan = plan(&manual, &[], false);
        assert!(plan.warnings.is_empty());
        let bound: Vec<(&str, &str)> = plan
            .bound
            .iter()
            .map(|key| (key.workflow.as_str(), key.keystroke.as_str()))
            .collect();
        assert_eq!(
            bound,
            [
                ("first", "secondary-shift-b"),
                ("second", "alt-secondary-x")
            ]
        );
    }

    #[test]
    fn a_shortcut_the_window_uses_is_not_bound_and_says_by_what() {
        let manual = pairs(&[("flow", "secondary-s")]);
        let built_in = pairs(&[("ctrl-s", "Save Workflow")]);
        let plan = plan(&manual, &built_in, false);
        assert!(plan.bound.is_empty());
        assert_eq!(
            plan.warnings,
            [KeyWarning {
                workflow: "flow".into(),
                message: "key secondary-s is already used by Save Workflow".into(),
            }]
        );
    }

    #[test]
    fn a_clash_with_the_window_is_judged_on_macos_with_cmd() {
        let manual = pairs(&[("flow", "secondary-s")]);
        let built_in = pairs(&[("cmd-s", "Save Workflow")]);
        assert!(plan(&manual, &built_in, true).bound.is_empty());
        assert_eq!(plan(&manual, &built_in, false).bound.len(), 1);
    }

    #[test]
    fn the_first_workflow_keeps_a_shortcut_two_of_them_ask_for() {
        let manual = pairs(&[("first", "secondary-shift-b"), ("second", "ctrl-shift-b")]);
        let plan = plan(&manual, &[], false);
        assert_eq!(plan.bound.len(), 1);
        assert_eq!(plan.bound[0].workflow, "first");
        assert_eq!(
            plan.warnings[0].message,
            "key ctrl-shift-b is already used by workflow first"
        );
        assert_eq!(plan.warnings[0].workflow, "second");
    }

    #[test]
    fn an_action_name_reads_in_words() {
        assert_eq!(action_words("gitmaster::OpenPalette"), "Open Palette");
        assert_eq!(action_words("text_area::SelectDocEnd"), "Select Doc End");
        assert_eq!(action_words("Undo"), "Undo");
    }
}
