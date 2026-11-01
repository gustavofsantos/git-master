//! The branch stacks the sidebar draws (D138): the last `StackSnapshot` the
//! engine sent and the `LoadStacks` in flight. No GPUI here; `Model` routes the
//! engine's events in, and `sidebar` reads what this holds. It also says, in
//! words, what each stack state and indicator means, for the tooltips.

use gm_schema::{OperationId, StackBranch, StackSnapshot, StackState};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stacks {
    /// The last snapshot, kept while a newer one is read so the tree does not blink.
    snapshot: Option<StackSnapshot>,
    /// The `LoadStacks` in flight.
    waiting: Option<OperationId>,
}

impl Stacks {
    /// A read `id` was sent.
    pub fn request(&mut self, id: OperationId) {
        self.waiting = Some(id);
    }

    /// Whether a read is in flight; the window then sends no second one.
    #[must_use]
    pub fn is_waiting(&self) -> bool {
        self.waiting.is_some()
    }

    /// Whether the tree has nothing to show yet: the first read is still running.
    #[must_use]
    pub fn calculating(&self) -> bool {
        self.snapshot.is_none() && self.waiting.is_some()
    }

    /// A snapshot arrived, from a read or after a stack write. The engine answers
    /// in order, so it is never older than the one shown; the read in flight ends
    /// with it when it answers that read or a later command.
    pub fn loaded(&mut self, id: OperationId, snapshot: StackSnapshot) {
        self.snapshot = Some(snapshot);
        if self.waiting.is_some_and(|waiting| id >= waiting) {
            self.waiting = None;
        }
    }

    /// Operation `id` ended. A read that failed ends here, with nothing new.
    pub fn settled(&mut self, id: OperationId) {
        if self.waiting == Some(id) {
            self.waiting = None;
        }
    }

    /// Another repository opened: nothing of the old one is kept.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn snapshot(&self) -> Option<&StackSnapshot> {
        self.snapshot.as_ref()
    }

    /// The stack entry of local branch `name`, if it is in a stack.
    #[must_use]
    pub fn branch(&self, name: &str) -> Option<&StackBranch> {
        self.snapshot.as_ref()?.branch(name)
    }

    /// Whether `name` has a recorded parent, in a stack or in a loop.
    #[must_use]
    pub fn has_parent(&self, name: &str) -> bool {
        self.branch(name)
            .is_some_and(|entry| entry.parent.is_some())
            || self.looped(name)
    }

    /// Whether `name` is in a loop of recorded parents.
    #[must_use]
    pub fn looped(&self, name: &str) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.looped.iter().any(|looped| looped == name))
    }
}

/// The tag a branch shows for its state, with the sentence its tooltip reads.
/// `None` for a root, a current branch and a stale one: staleness is the amber
/// dot, explained by `stale_hint`.
#[must_use]
pub fn state_tag(entry: &StackBranch) -> Option<(&'static str, String)> {
    let parent = entry.parent.as_deref().unwrap_or("its parent");
    match entry.state {
        StackState::Root | StackState::Current | StackState::Stale => None,
        StackState::DivergedBase => Some((
            "diverged",
            format!(
                "The recorded base no longer marks where this branch's own commits start on {parent}. It was rebased or merged outside GitMaster."
            ),
        )),
        StackState::ParentGone => Some((
            "parent gone",
            format!(
                "The parent branch {parent} no longer exists. Reparent to Grandparent is in the menu."
            ),
        )),
        StackState::ParentMerged => Some((
            "parent merged",
            format!(
                "The changes of {parent} are already in the root of the stack. Reparent to Grandparent is in the menu."
            ),
        )),
    }
}

/// The tooltip of the amber dot of a stale branch.
#[must_use]
pub fn stale_hint(entry: &StackBranch) -> String {
    let parent = entry.parent.as_deref().unwrap_or("its parent");
    format!("{parent} has moved since this branch was based on it. It needs a restack.")
}

/// The `↑N` indicator and its tooltip; `None` with nothing ahead.
#[must_use]
pub fn ahead_label(entry: &StackBranch) -> Option<(String, String)> {
    (entry.ahead > 0 && entry.parent.is_some()).then(|| {
        let parent = entry.parent.as_deref().unwrap_or_default();
        let commits = plural(entry.ahead, "commit", "commits");
        (
            format!("↑{}", entry.ahead),
            format!("{} {commits} that {parent} does not have.", entry.ahead),
        )
    })
}

/// The `restack N` indicator and its tooltip; `None` when nothing below waits.
#[must_use]
pub fn restack_label(entry: &StackBranch) -> Option<(String, String)> {
    (entry.restack > 0).then(|| {
        let branches = plural(entry.restack, "branch", "branches");
        (
            format!("restack {}", entry.restack),
            format!(
                "{} {branches} below need a restack, because this branch moved.",
                entry.restack
            ),
        )
    })
}

fn plural(count: u32, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::Stack;

    fn entry(name: &str, parent: Option<&str>, state: StackState) -> StackBranch {
        StackBranch {
            name: name.into(),
            parent: parent.map(str::to_owned),
            depth: u32::from(parent.is_some()),
            state,
            ahead: 0,
            behind: 0,
            restack: 0,
            commits: Vec::new(),
            base: None,
            parent_tip: None,
        }
    }

    fn snapshot() -> StackSnapshot {
        StackSnapshot {
            stacks: vec![Stack {
                branches: vec![
                    entry("main", None, StackState::Root),
                    entry("topic", Some("main"), StackState::Stale),
                ],
            }],
            looped: vec!["a".into()],
        }
    }

    #[test]
    fn calculating_lasts_until_the_first_snapshot() {
        let mut stacks = Stacks::default();
        assert!(!stacks.calculating());
        stacks.request(OperationId(4));
        assert!(stacks.calculating());
        stacks.loaded(OperationId(4), snapshot());
        assert!(!stacks.calculating());
        assert!(!stacks.is_waiting());
        // A later read keeps the old tree on screen while it runs.
        stacks.request(OperationId(9));
        assert!(!stacks.calculating());
        assert!(stacks.is_waiting());
        // An answer to an earlier write does not end the read.
        stacks.loaded(OperationId(7), snapshot());
        assert!(stacks.is_waiting());
        stacks.settled(OperationId(9));
        assert!(!stacks.is_waiting());
    }

    #[test]
    fn a_failed_first_read_stops_calculating() {
        let mut stacks = Stacks::default();
        stacks.request(OperationId(2));
        stacks.settled(OperationId(1));
        assert!(stacks.calculating());
        stacks.settled(OperationId(2));
        assert!(!stacks.calculating());
        assert!(stacks.snapshot().is_none());
    }

    #[test]
    fn parents_are_known_in_trees_and_loops() {
        let mut stacks = Stacks::default();
        stacks.loaded(OperationId(1), snapshot());
        assert!(stacks.has_parent("topic"));
        assert!(!stacks.has_parent("main"));
        assert!(stacks.has_parent("a"));
        assert!(!stacks.has_parent("other"));
        stacks.clear();
        assert!(!stacks.has_parent("topic"));
    }

    #[test]
    fn indicators_read_as_the_docs_say() {
        let mut topic = entry("topic", Some("main"), StackState::Current);
        assert_eq!(ahead_label(&topic), None);
        topic.ahead = 3;
        assert_eq!(ahead_label(&topic).unwrap().0, "↑3");
        topic.ahead = 1;
        assert_eq!(
            ahead_label(&topic).unwrap().1,
            "1 commit that main does not have."
        );
        topic.restack = 2;
        assert_eq!(restack_label(&topic).unwrap().0, "restack 2");
        assert!(state_tag(&topic).is_none());
        topic.state = StackState::ParentMerged;
        assert_eq!(state_tag(&topic).unwrap().0, "parent merged");
        topic.state = StackState::DivergedBase;
        assert_eq!(state_tag(&topic).unwrap().0, "diverged");
        assert!(stale_hint(&topic).starts_with("main has moved"));
    }
}
