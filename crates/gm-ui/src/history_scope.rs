//! The scope of the history list (H6, D34, D139): which commits it reads. The
//! rules are pure and tested here; the pill and its menu are in `shell_history`.
//! The engine resolves a scope to the refs it walks.

use gm_schema::HistoryScope;

use crate::stacks::Stacks;

/// Why Current stack cannot be chosen.
pub(crate) const NO_STACK: &str = "The checked-out branch is in no stack.";
/// Why One ref cannot be chosen from the menu.
pub(crate) const PICK_IN_SIDEBAR: &str = "Click a branch in the sidebar.";

/// The scope the person chose for this window, and the scope of the list on
/// screen. With no choice, the scope follows HEAD (D139).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScopeChoice {
    chosen: Option<HistoryScope>,
    loaded: Option<HistoryScope>,
}

impl ScopeChoice {
    /// The scope to read now: the choice, else Current stack when HEAD's branch
    /// is in a stack, else Current branch. A choice of Current stack that no
    /// longer applies falls back too.
    #[must_use]
    pub(crate) fn effective(&self, head: Option<&str>, stacks: &Stacks) -> HistoryScope {
        let stacked = in_stack(head, stacks);
        match &self.chosen {
            Some(HistoryScope::CurrentStack) if !stacked => HistoryScope::CurrentBranch,
            Some(scope) => scope.clone(),
            None if stacked => HistoryScope::CurrentStack,
            None => HistoryScope::CurrentBranch,
        }
    }

    pub(crate) fn choose(&mut self, scope: HistoryScope) {
        self.chosen = Some(scope);
    }

    /// The list now shows `scope`.
    pub(crate) fn loaded(&mut self, scope: HistoryScope) {
        self.loaded = Some(scope);
    }

    /// Whether the list on screen is of another scope than the one to read now:
    /// the window then reads it again.
    #[must_use]
    pub(crate) fn stale(&self, head: Option<&str>, stacks: &Stacks) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|loaded| *loaded != self.effective(head, stacks))
    }

    /// Another repository opened: the choice and the list belong to the old one.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

fn in_stack(head: Option<&str>, stacks: &Stacks) -> bool {
    head.and_then(|name| stacks.branch(name)).is_some()
}

/// The text of the pill for `scope`.
#[must_use]
pub(crate) fn label(scope: &HistoryScope) -> String {
    match scope {
        HistoryScope::CurrentBranch => "Scope: current branch".to_owned(),
        HistoryScope::CurrentStack => "Scope: current stack".to_owned(),
        HistoryScope::AllRefs => "Scope: all refs".to_owned(),
        HistoryScope::Ref { name } => format!("Scope: {name}"),
    }
}

/// One line of the pill's menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScopeOption {
    pub(crate) title: &'static str,
    /// What choosing the line reads; `None` for One ref, chosen in the sidebar.
    pub(crate) scope: Option<HistoryScope>,
    pub(crate) state: Result<(), &'static str>,
    /// The line is the scope on screen.
    pub(crate) current: bool,
}

/// The lines of the menu, in the order of the docs, for the scope `now`.
#[must_use]
pub(crate) fn options(now: &HistoryScope, head: Option<&str>, stacks: &Stacks) -> Vec<ScopeOption> {
    let stacked = in_stack(head, stacks);
    let line = |title, scope: Option<HistoryScope>, state| ScopeOption {
        current: match (&scope, now) {
            (Some(scope), now) => scope == now,
            (None, HistoryScope::Ref { .. }) => true,
            (None, _) => false,
        },
        title,
        scope,
        state,
    };
    vec![
        line("Current branch", Some(HistoryScope::CurrentBranch), Ok(())),
        line(
            "Current stack",
            Some(HistoryScope::CurrentStack),
            if stacked { Ok(()) } else { Err(NO_STACK) },
        ),
        line("All refs", Some(HistoryScope::AllRefs), Ok(())),
        line("One ref", None, Err(PICK_IN_SIDEBAR)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{OperationId, Stack, StackBranch, StackSnapshot, StackState};

    fn stacks() -> Stacks {
        let entry = |name: &str, parent: Option<&str>| StackBranch {
            name: name.into(),
            parent: parent.map(str::to_owned),
            depth: u32::from(parent.is_some()),
            state: StackState::Current,
            ahead: 0,
            behind: 0,
            restack: 0,
            commits: Vec::new(),
            base: None,
            parent_tip: None,
        };
        let mut stacks = Stacks::default();
        stacks.loaded(
            OperationId(1),
            StackSnapshot {
                stacks: vec![Stack {
                    branches: vec![entry("main", None), entry("topic", Some("main"))],
                }],
                looped: Vec::new(),
            },
        );
        stacks
    }

    #[test]
    fn the_default_follows_whether_head_is_in_a_stack() {
        let choice = ScopeChoice::default();
        let stacks = stacks();
        assert_eq!(
            choice.effective(Some("topic"), &stacks),
            HistoryScope::CurrentStack
        );
        assert_eq!(
            choice.effective(Some("lone"), &stacks),
            HistoryScope::CurrentBranch
        );
        assert_eq!(choice.effective(None, &stacks), HistoryScope::CurrentBranch);
        // Before the first snapshot nothing is known to be stacked.
        assert_eq!(
            choice.effective(Some("topic"), &Stacks::default()),
            HistoryScope::CurrentBranch
        );
    }

    #[test]
    fn a_choice_wins_until_another_repository_opens() {
        let stacks = stacks();
        let mut choice = ScopeChoice::default();
        choice.choose(HistoryScope::AllRefs);
        assert_eq!(
            choice.effective(Some("topic"), &stacks),
            HistoryScope::AllRefs
        );
        choice.choose(HistoryScope::CurrentStack);
        // Checked out elsewhere, the stack no longer applies.
        assert_eq!(
            choice.effective(Some("lone"), &stacks),
            HistoryScope::CurrentBranch
        );
        choice.reset();
        assert_eq!(
            choice.effective(Some("topic"), &stacks),
            HistoryScope::CurrentStack
        );
    }

    #[test]
    fn the_list_is_stale_when_the_first_snapshot_changes_the_default() {
        let mut choice = ScopeChoice::default();
        assert!(!choice.stale(Some("topic"), &Stacks::default()));
        choice.loaded(choice.effective(Some("topic"), &Stacks::default()));
        assert!(!choice.stale(Some("topic"), &Stacks::default()));
        assert!(choice.stale(Some("topic"), &stacks()));
        choice.loaded(HistoryScope::CurrentStack);
        assert!(!choice.stale(Some("topic"), &stacks()));
    }

    #[test]
    fn the_menu_marks_the_scope_on_screen_and_says_why_a_line_is_off() {
        let stacks = stacks();
        let lines = options(&HistoryScope::CurrentStack, Some("topic"), &stacks);
        let titles: Vec<&str> = lines.iter().map(|line| line.title).collect();
        assert_eq!(
            titles,
            ["Current branch", "Current stack", "All refs", "One ref"]
        );
        assert!(lines[1].current && lines[1].state.is_ok());
        assert_eq!(lines[3].state, Err(PICK_IN_SIDEBAR));
        let off = options(&HistoryScope::CurrentBranch, Some("lone"), &stacks);
        assert_eq!(off[1].state, Err(NO_STACK));
        assert!(off[0].current);
        let one = options(
            &HistoryScope::Ref {
                name: "topic".into(),
            },
            Some("lone"),
            &stacks,
        );
        assert!(one[3].current);
        assert_eq!(label(&HistoryScope::CurrentStack), "Scope: current stack");
        assert_eq!(
            label(&HistoryScope::Ref {
                name: "topic".into()
            }),
            "Scope: topic"
        );
    }
}
