//! Branch stacks (D03, D13, D138). Pure: the engine reads the recorded links and
//! the graph facts through the port, and this module turns them into trees and
//! states. Nothing here runs Git, so the rules are tested on their own.
//!
//! A link is `branch.<name>.stackParent` with its `stackParentBase`. The links of
//! the local branches form a forest: a branch whose parent has no link is the
//! root of a tree, and so is a branch whose parent is gone. Links that close a
//! loop belong to no tree.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use gm_schema::{Stack, StackBranch, StackSnapshot, StackState};

/// The two config keys of a branch's link (D13), as `git config` spells them.
pub const PARENT_KEY: &str = "stackParent";
pub const BASE_KEY: &str = "stackParentBase";

/// A recorded parent: `branch.<branch>.stackParent = parent`, with its base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackLink {
    pub branch: String,
    pub parent: String,
    /// `stackParentBase`; `None` when the key is missing.
    pub base: Option<String>,
}

/// What the graph says about one link whose parent exists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeFacts {
    /// The full id of the parent's tip.
    pub parent_tip: String,
    /// Commits of the branch that the parent does not have.
    pub ahead: u32,
    /// Commits of the parent that the branch does not have.
    pub behind: u32,
    /// The base resolves to the branch tip or one of its ancestors.
    pub base_in_branch: bool,
    /// The merge base of the branch and its parent strictly descends from the base.
    pub merge_base_past_base: bool,
    /// The branch's own commits, `base..tip`, newest first, at most
    /// `OWN_COMMITS_LIMIT`; empty when the base is not a commit (D139).
    pub own: Vec<String>,
}

/// A check the engine runs for a parent that is not a root: whether its changes
/// are already in the root of its stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeCheck {
    pub parent: String,
    /// The parent's own recorded base, where its own commits start.
    pub parent_base: Option<String>,
    pub root: String,
}

/// The trees the links of the local branches make, before any fact is read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Forest {
    /// Every link of a local branch that is in a tree, by branch.
    links: BTreeMap<String, StackLink>,
    /// The children of each branch, by name.
    children: BTreeMap<String, Vec<String>>,
    /// The roots, by name: branches with children and no link to an existing
    /// parent, and branches whose parent is gone.
    roots: Vec<String>,
    /// Branches whose links form a loop.
    looped: Vec<String>,
    /// The local branches outside any loop: the ones that may be parents.
    locals: HashSet<String>,
}

impl Forest {
    /// The forest of `links` over the local branches `locals`. A link of a branch
    /// that is not local is left out: Git keeps the config of a branch only while
    /// the branch exists, so it is a leftover.
    #[must_use]
    pub fn new(links: &[StackLink], locals: &[String]) -> Self {
        let locals: HashSet<String> = locals.iter().cloned().collect();
        let by_branch: BTreeMap<String, StackLink> = links
            .iter()
            .filter(|link| locals.contains(&link.branch))
            .map(|link| (link.branch.clone(), link.clone()))
            .collect();
        let looped = loops(&by_branch, &locals);
        let links: BTreeMap<String, StackLink> = by_branch
            .into_iter()
            .filter(|(branch, _)| !looped.contains(branch))
            .collect();
        // A branch of a loop is in no tree, so it cannot be a parent either.
        let locals: HashSet<String> = locals
            .into_iter()
            .filter(|name| !looped.contains(name))
            .collect();
        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut roots = BTreeSet::new();
        for (branch, link) in &links {
            if locals.contains(&link.parent) {
                children
                    .entry(link.parent.clone())
                    .or_default()
                    .push(branch.clone());
                if !links.contains_key(&link.parent) {
                    roots.insert(link.parent.clone());
                }
            } else {
                roots.insert(branch.clone());
            }
        }
        for list in children.values_mut() {
            list.sort();
        }
        Self {
            links,
            children,
            roots: roots.into_iter().collect(),
            looped: looped.into_iter().collect(),
            locals,
        }
    }

    /// The links whose parent exists, which need `EdgeFacts`, in tree order.
    #[must_use]
    pub fn edges(&self) -> Vec<&StackLink> {
        self.walk()
            .into_iter()
            .filter_map(|(name, _)| self.links.get(name))
            .filter(|link| self.locals.contains(&link.parent))
            .collect()
    }

    /// One check per parent that has a parent itself: the root its changes
    /// would land in. A root has nowhere to be merged into.
    #[must_use]
    pub fn merge_checks(&self) -> Vec<MergeCheck> {
        let mut checks = Vec::new();
        for root in &self.roots {
            self.collect_checks(root, root, &mut checks);
        }
        checks
    }

    fn collect_checks(&self, root: &str, branch: &str, checks: &mut Vec<MergeCheck>) {
        for child in self.children.get(branch).into_iter().flatten() {
            if self.children.contains_key(child)
                && let Some(link) = self.links.get(child)
                && self.locals.contains(&link.parent)
            {
                checks.push(MergeCheck {
                    parent: child.clone(),
                    parent_base: link.base.clone(),
                    root: root.to_owned(),
                });
            }
            self.collect_checks(root, child, checks);
        }
    }

    /// Every branch of every tree with its depth, each parent before its children.
    fn walk(&self) -> Vec<(&str, u32)> {
        let mut out = Vec::new();
        for root in &self.roots {
            self.walk_from(root, 0, &mut out);
        }
        out
    }

    fn walk_from<'a>(&'a self, branch: &'a str, depth: u32, out: &mut Vec<(&'a str, u32)>) {
        out.push((branch, depth));
        for child in self.children.get(branch).into_iter().flatten() {
            self.walk_from(child, depth + 1, out);
        }
    }

    /// The snapshot, from the facts of each edge (by child) and the parents found
    /// merged. An edge without facts counts as current, with no counts.
    #[must_use]
    pub fn assemble(
        &self,
        facts: &HashMap<String, EdgeFacts>,
        merged: &HashSet<String>,
    ) -> StackSnapshot {
        let mut stacks = Vec::new();
        for root in &self.roots {
            let mut order = Vec::new();
            self.walk_from(root, 0, &mut order);
            let mut branches: Vec<StackBranch> = order
                .iter()
                .map(|(name, depth)| {
                    let link = self.links.get(*name);
                    let (state, ahead, behind, commits) = match link {
                        None => (StackState::Root, 0, 0, Vec::new()),
                        Some(link) if !self.locals.contains(&link.parent) => {
                            (StackState::ParentGone, 0, 0, Vec::new())
                        }
                        Some(link) => {
                            let edge = facts.get(*name).cloned().unwrap_or_default();
                            let state = edge_state(link, &edge, merged.contains(&link.parent));
                            (state, edge.ahead, edge.behind, edge.own)
                        }
                    };
                    StackBranch {
                        name: (*name).to_owned(),
                        parent: link.map(|link| link.parent.clone()),
                        depth: *depth,
                        state,
                        ahead,
                        behind,
                        restack: 0,
                        commits,
                    }
                })
                .collect();
            count_restacks(&mut branches);
            stacks.push(Stack { branches });
        }
        StackSnapshot {
            stacks,
            looped: self.looped.clone(),
        }
    }
}

/// The branches whose links, followed parent by parent, come back to themselves.
fn loops(links: &BTreeMap<String, StackLink>, locals: &HashSet<String>) -> BTreeSet<String> {
    let mut looped = BTreeSet::new();
    for start in links.keys() {
        let mut seen = Vec::new();
        let mut at = start.as_str();
        while let Some(link) = links.get(at) {
            if let Some(index) = seen.iter().position(|name| *name == at) {
                looped.extend(seen[index..].iter().map(|name: &&str| (*name).to_owned()));
                break;
            }
            seen.push(at);
            if !locals.contains(&link.parent) {
                break;
            }
            at = &link.parent;
        }
    }
    looped
}

/// The state of a branch whose parent exists, by the order of D138.
fn edge_state(link: &StackLink, edge: &EdgeFacts, parent_merged: bool) -> StackState {
    if parent_merged {
        return StackState::ParentMerged;
    }
    let Some(base) = &link.base else {
        return StackState::DivergedBase;
    };
    if !edge.base_in_branch || edge.merge_base_past_base {
        return StackState::DivergedBase;
    }
    if *base == edge.parent_tip {
        StackState::Current
    } else {
        StackState::Stale
    }
}

/// Sets `restack` on each branch of one tree in walk order: its stale children
/// plus every branch under them.
fn count_restacks(branches: &mut [StackBranch]) {
    let sizes: Vec<u32> = (0..branches.len())
        .map(|index| subtree_size(branches, index))
        .collect();
    for index in 0..branches.len() {
        let depth = branches[index].depth;
        let mut total = 0;
        for child in index + 1..branches.len() {
            if branches[child].depth <= depth {
                break;
            }
            if branches[child].depth == depth + 1 && branches[child].state == StackState::Stale {
                total += sizes[child];
            }
        }
        branches[index].restack = total;
    }
}

/// The branch at `index` and every branch under it, in a walk-ordered tree.
fn subtree_size(branches: &[StackBranch], index: usize) -> u32 {
    let depth = branches[index].depth;
    let under = branches[index + 1..]
        .iter()
        .take_while(|branch| branch.depth > depth)
        .count();
    u32::try_from(under + 1).unwrap_or(u32::MAX)
}

/// Why `parent` cannot become the stack parent of `branch`, as the sentence the
/// user reads; `None` when it can. `links` are every recorded link, `locals`
/// every local branch. The shared history is checked by the engine.
#[must_use]
pub fn set_parent_problem(
    branch: &str,
    parent: &str,
    links: &[StackLink],
    locals: &[String],
) -> Option<String> {
    if !locals.iter().any(|name| name == branch) {
        return Some(format!("There is no local branch {branch}."));
    }
    if !locals.iter().any(|name| name == parent) {
        return Some(format!(
            "The parent must be a local branch; {parent} is not one."
        ));
    }
    if branch == parent {
        return Some("A branch cannot be its own parent.".to_owned());
    }
    // Walk up from the new parent: meeting the branch means it is under it.
    let by_branch: HashMap<&str, &str> = links
        .iter()
        .map(|link| (link.branch.as_str(), link.parent.as_str()))
        .collect();
    let mut at = parent;
    let mut steps = 0;
    while let Some(next) = by_branch.get(at) {
        if *next == branch {
            return Some(format!(
                "{parent} is under {branch} in its stack, so it cannot be its parent."
            ));
        }
        at = next;
        steps += 1;
        if steps > by_branch.len() {
            break;
        }
    }
    None
}

/// The parent `ReparentToGrandparent` gives `branch`: the recorded parent of its
/// old parent when that is a local branch other than `branch`, else the default
/// branch the caller found (`fallback`). `None` when there is none, or it is
/// the branch itself.
#[must_use]
pub fn grandparent(
    branch: &str,
    links: &[StackLink],
    locals: &[String],
    fallback: Option<&str>,
) -> Option<String> {
    let parent = links
        .iter()
        .find(|link| link.branch == branch)?
        .parent
        .as_str();
    let recorded = links
        .iter()
        .find(|link| link.branch == parent)
        .map(|link| link.parent.as_str())
        .filter(|name| *name != branch && locals.iter().any(|local| local == name));
    recorded
        .or(fallback)
        .filter(|name| *name != branch && *name != parent)
        .map(str::to_owned)
}

/// The link of each branch from the `branch.*` config entries, keyed as
/// `git config` prints them: section and key in lower case, the branch name as
/// written. Entries without a parent are dropped.
#[must_use]
pub fn links_from_config(entries: &[(String, String)]) -> Vec<StackLink> {
    let parent_suffix = format!(".{}", PARENT_KEY.to_ascii_lowercase());
    let base_suffix = format!(".{}", BASE_KEY.to_ascii_lowercase());
    let mut parents: BTreeMap<String, String> = BTreeMap::new();
    let mut bases: HashMap<String, String> = HashMap::new();
    for (key, value) in entries {
        let Some(rest) = key.strip_prefix("branch.") else {
            continue;
        };
        // The base suffix ends with the parent suffix plus "base", so test it first.
        if let Some(name) = rest.strip_suffix(&base_suffix) {
            bases.insert(name.to_owned(), value.trim().to_owned());
        } else if let Some(name) = rest.strip_suffix(&parent_suffix) {
            let value = value.trim();
            if !value.is_empty() {
                parents.insert(name.to_owned(), value.to_owned());
            }
        }
    }
    parents
        .into_iter()
        .map(|(branch, parent)| StackLink {
            base: bases.get(&branch).filter(|base| !base.is_empty()).cloned(),
            branch,
            parent,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(branch: &str, parent: &str, base: &str) -> StackLink {
        StackLink {
            branch: branch.to_owned(),
            parent: parent.to_owned(),
            base: Some(base.to_owned()),
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    fn current(tip: &str) -> EdgeFacts {
        EdgeFacts {
            parent_tip: tip.to_owned(),
            ahead: 1,
            behind: 0,
            base_in_branch: true,
            merge_base_past_base: false,
            own: vec![format!("{tip}-own")],
        }
    }

    /// The ledger stack of docs/guide/stacks.md.
    fn ledger() -> (Vec<StackLink>, Vec<String>) {
        (
            vec![
                link("ledger-base", "main", "m1"),
                link("ledger-api", "ledger-base", "b1"),
                link("ledger-ui", "ledger-base", "b1"),
                link("ledger-e2e", "ledger-ui", "u1"),
            ],
            names(&[
                "main",
                "ledger-base",
                "ledger-api",
                "ledger-ui",
                "ledger-e2e",
                "lone",
            ]),
        )
    }

    fn tree(snapshot: &StackSnapshot) -> Vec<String> {
        snapshot
            .stacks
            .iter()
            .flat_map(|stack| &stack.branches)
            .map(|branch| format!("{}{}", "  ".repeat(branch.depth as usize), branch.name))
            .collect()
    }

    #[test]
    fn the_ledger_stack_is_one_tree_rooted_at_main() {
        let (links, locals) = ledger();
        let forest = Forest::new(&links, &locals);
        let snapshot = forest.assemble(&HashMap::new(), &HashSet::new());
        assert_eq!(
            tree(&snapshot),
            [
                "main",
                "  ledger-base",
                "    ledger-api",
                "    ledger-ui",
                "      ledger-e2e"
            ]
        );
        assert_eq!(snapshot.stacks[0].root(), "main");
        assert_eq!(snapshot.stacks[0].branches[0].state, StackState::Root);
        assert!(snapshot.branch("lone").is_none());
        assert!(snapshot.looped.is_empty());
    }

    #[test]
    fn edges_follow_the_walk_and_merge_checks_skip_the_root() {
        let (links, locals) = ledger();
        let forest = Forest::new(&links, &locals);
        let edges: Vec<&str> = forest
            .edges()
            .iter()
            .map(|link| link.branch.as_str())
            .collect();
        assert_eq!(
            edges,
            ["ledger-base", "ledger-api", "ledger-ui", "ledger-e2e"]
        );
        let all = forest.merge_checks();
        let checks: Vec<(&str, &str)> = all
            .iter()
            .map(|check| (check.parent.as_str(), check.root.as_str()))
            .collect();
        // `main` is the root; ledger-base and ledger-ui have children.
        assert_eq!(checks.len(), 2);
        assert!(checks.contains(&("ledger-base", "main")));
        assert!(checks.contains(&("ledger-ui", "main")));
    }

    #[test]
    fn a_stale_child_puts_its_subtree_on_the_parents_restack_count() {
        let (links, locals) = ledger();
        let forest = Forest::new(&links, &locals);
        let mut facts = HashMap::new();
        facts.insert("ledger-base".to_owned(), current("m1"));
        facts.insert("ledger-api".to_owned(), current("b1"));
        // ledger-base moved on: ledger-ui recorded b1, the tip is now b2.
        facts.insert("ledger-ui".to_owned(), current("b2"));
        facts.insert("ledger-e2e".to_owned(), current("u1"));
        let snapshot = forest.assemble(&facts, &HashSet::new());
        let base = snapshot.branch("ledger-base").unwrap();
        assert_eq!(base.restack, 2, "ledger-ui and ledger-e2e");
        assert_eq!(
            snapshot.branch("ledger-ui").unwrap().state,
            StackState::Stale
        );
        assert_eq!(
            snapshot.branch("ledger-api").unwrap().state,
            StackState::Current
        );
        assert_eq!(snapshot.branch("main").unwrap().restack, 0);
        assert_eq!(snapshot.branch("ledger-api").unwrap().ahead, 1);
    }

    #[test]
    fn a_gone_parent_makes_the_child_a_root_of_its_own() {
        let links = vec![link("ui", "base", "b1"), link("e2e", "ui", "u1")];
        let locals = names(&["ui", "e2e"]);
        let forest = Forest::new(&links, &locals);
        assert_eq!(
            forest.edges().len(),
            1,
            "only ui -> e2e has a parent to read"
        );
        let snapshot = forest.assemble(&HashMap::new(), &HashSet::new());
        assert_eq!(tree(&snapshot), ["ui", "  e2e"]);
        let ui = snapshot.branch("ui").unwrap();
        assert_eq!(ui.state, StackState::ParentGone);
        assert_eq!(ui.parent.as_deref(), Some("base"));
        assert!(ui.state.parent_left());
    }

    #[test]
    fn states_follow_the_order_of_d138() {
        let plain = link("c", "p", "b1");
        let mut edge = current("b2");
        assert_eq!(edge_state(&plain, &edge, false), StackState::Stale);
        edge.parent_tip = "b1".into();
        assert_eq!(edge_state(&plain, &edge, false), StackState::Current);
        edge.merge_base_past_base = true;
        assert_eq!(edge_state(&plain, &edge, false), StackState::DivergedBase);
        assert_eq!(edge_state(&plain, &edge, true), StackState::ParentMerged);
        edge.merge_base_past_base = false;
        edge.base_in_branch = false;
        assert_eq!(edge_state(&plain, &edge, false), StackState::DivergedBase);
        let no_base = StackLink {
            base: None,
            ..plain
        };
        assert_eq!(
            edge_state(&no_base, &current("b1"), false),
            StackState::DivergedBase
        );
    }

    #[test]
    fn loops_belong_to_no_tree() {
        let links = vec![
            link("a", "b", "x"),
            link("b", "a", "x"),
            link("c", "a", "x"),
            link("d", "main", "x"),
        ];
        let locals = names(&["a", "b", "c", "d", "main"]);
        let snapshot = Forest::new(&links, &locals).assemble(&HashMap::new(), &HashSet::new());
        assert_eq!(snapshot.looped, ["a", "b"]);
        // c points into the loop, whose branches cannot be parents: c stands alone.
        assert_eq!(tree(&snapshot), ["c", "main", "  d"]);
        assert_eq!(snapshot.branch("c").unwrap().state, StackState::ParentGone);
    }

    #[test]
    fn a_link_of_a_branch_that_is_gone_is_left_out() {
        let links = vec![link("gone", "main", "x")];
        let snapshot =
            Forest::new(&links, &names(&["main"])).assemble(&HashMap::new(), &HashSet::new());
        assert!(snapshot.stacks.is_empty());
    }

    #[test]
    fn set_parent_refuses_self_unknown_and_descendants() {
        let (links, locals) = ledger();
        assert!(set_parent_problem("ledger-api", "ledger-ui", &links, &locals).is_none());
        assert!(set_parent_problem("lone", "main", &links, &locals).is_none());
        assert_eq!(
            set_parent_problem("lone", "lone", &links, &locals).unwrap(),
            "A branch cannot be its own parent."
        );
        assert!(
            set_parent_problem("ledger-base", "ledger-e2e", &links, &locals)
                .unwrap()
                .contains("under ledger-base")
        );
        assert!(
            set_parent_problem("lone", "origin/main", &links, &locals)
                .unwrap()
                .contains("local branch")
        );
        assert!(set_parent_problem("ghost", "main", &links, &locals).is_some());
    }

    #[test]
    fn grandparent_is_the_old_parents_parent_or_the_fallback() {
        let (links, locals) = ledger();
        assert_eq!(
            grandparent("ledger-e2e", &links, &locals, Some("main")).as_deref(),
            Some("ledger-base")
        );
        let orphan = vec![link("ui", "base", "b1")];
        assert_eq!(
            grandparent("ui", &orphan, &names(&["ui", "main"]), Some("main")).as_deref(),
            Some("main")
        );
        assert_eq!(grandparent("ui", &orphan, &names(&["ui"]), None), None);
        assert_eq!(grandparent("lone", &links, &locals, Some("main")), None);
    }

    #[test]
    fn config_entries_become_links_keeping_the_branch_case() {
        let entries = vec![
            ("branch.Feature.x.stackparent".to_owned(), "main".to_owned()),
            (
                "branch.Feature.x.stackparentbase".to_owned(),
                "abc".to_owned(),
            ),
            (
                "branch.only-base.stackparentbase".to_owned(),
                "abc".to_owned(),
            ),
            ("branch.no-base.stackparent".to_owned(), "main".to_owned()),
            ("branch.other.remote".to_owned(), "origin".to_owned()),
        ];
        assert_eq!(
            links_from_config(&entries),
            [
                StackLink {
                    branch: "Feature.x".into(),
                    parent: "main".into(),
                    base: Some("abc".into()),
                },
                StackLink {
                    branch: "no-base".into(),
                    parent: "main".into(),
                    base: None,
                },
            ]
        );
    }
}
