//! Which contexts a repository sees, and how a context is named there (design/threads.md,
//! "Visibility (pure)"; D191 (3), D193 (5)). Everything here is a function of the context tree and
//! the repository's memberships, so it is tested without a database file.

use std::collections::{BTreeMap, HashSet};

/// A context as the visibility rules see it: its id, its name and its parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextNode {
    pub id: i64,
    pub name: String,
    pub parent: Option<i64>,
}

/// Every context a repository sees, with its distance: each context the repository belongs to is at
/// distance 1, its parent at 2, and so on up to the root (D191 (3)). A context reached at two
/// distances keeps the smaller one. Sorted by distance, then by id.
#[must_use]
pub fn visible_contexts(nodes: &[ContextNode], member_of: &[i64]) -> Vec<(i64, u32)> {
    let mut best: BTreeMap<i64, u32> = BTreeMap::new();
    for &start in member_of {
        for (step, node) in lineage(nodes, start).into_iter().enumerate() {
            let distance = u32::try_from(step + 1).unwrap_or(u32::MAX);
            best.entry(node.id)
                .and_modify(|kept| *kept = (*kept).min(distance))
                .or_insert(distance);
        }
    }
    let mut visible: Vec<(i64, u32)> = best.into_iter().collect();
    visible.sort_by_key(|&(id, distance)| (distance, id));
    visible
}

/// The names from the root down to context `id`, joined with ` / `. Empty when `id` is unknown.
#[must_use]
pub fn context_path(nodes: &[ContextNode], id: i64) -> String {
    lineage(nodes, id)
        .iter()
        .rev()
        .map(|node| node.name.as_str())
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Whether giving context `id` the parent `new_parent` (`None` for the root) would make a cycle: the
/// new parent is `id` itself or one of its descendants (D191 (2), `UpdateContext` refuses it).
#[must_use]
pub fn would_cycle(nodes: &[ContextNode], id: i64, new_parent: Option<i64>) -> bool {
    let Some(parent) = new_parent else {
        return false;
    };
    lineage(nodes, parent).iter().any(|node| node.id == id)
}

/// What a scope named in a request means (D193 (5)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every scope the repository sees.
    All,
    /// The repository's own memories only.
    Repository,
    /// One visible context, by id.
    Context(i64),
}

/// Reads a scope name: `all` or empty is every scope, `repository` is the repository, and anything
/// else names a visible context by its full path (`Platform / Billing`, whitespace around `/`
/// ignored) or by a name that exactly one visible context has. A name no visible context has, or one
/// several have, is refused; the message says which.
pub fn resolve_scope(
    nodes: &[ContextNode],
    visible: &[(i64, u32)],
    text: &str,
) -> Result<Scope, String> {
    let text = text.trim();
    if text.is_empty() || text == "all" {
        return Ok(Scope::All);
    }
    if text == "repository" {
        return Ok(Scope::Repository);
    }
    let by_path = visible
        .iter()
        .map(|&(id, _)| id)
        .find(|&id| same_path(&context_path(nodes, id), text));
    if let Some(id) = by_path {
        return Ok(Scope::Context(id));
    }
    let named: Vec<i64> = visible
        .iter()
        .map(|&(id, _)| id)
        .filter(|&id| name_of(nodes, id).trim() == text)
        .collect();
    match named.as_slice() {
        [] => Err(format!(
            "No context named \"{text}\" is visible from this repository."
        )),
        [id] => Ok(Scope::Context(*id)),
        several => {
            let paths: Vec<String> = several
                .iter()
                .map(|&id| format!("\"{}\"", context_path(nodes, id)))
                .collect();
            Err(format!(
                "\"{text}\" names several visible contexts: {}. Name one by its full path.",
                paths.join(", ")
            ))
        }
    }
}

/// The node of context `id` followed by the nodes of its ancestors, up to the root. A cycle in the
/// stored tree ends the walk at the node that repeats; an unknown id gives nothing.
fn lineage(nodes: &[ContextNode], id: i64) -> Vec<&ContextNode> {
    let mut walked = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(id);
    while let Some(current_id) = current {
        let Some(node) = nodes.iter().find(|node| node.id == current_id) else {
            break;
        };
        if !seen.insert(current_id) {
            break;
        }
        walked.push(node);
        current = node.parent;
    }
    walked
}

/// The name of context `id`, or empty when it is unknown.
fn name_of(nodes: &[ContextNode], id: i64) -> &str {
    nodes
        .iter()
        .find(|node| node.id == id)
        .map_or("", |node| node.name.as_str())
}

/// Two paths are the same when their names match part by part, whitespace around each `/` ignored.
fn same_path(path: &str, text: &str) -> bool {
    path.split('/')
        .map(str::trim)
        .eq(text.split('/').map(str::trim))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i64, name: &str, parent: Option<i64>) -> ContextNode {
        ContextNode {
            id,
            name: name.to_owned(),
            parent,
        }
    }

    /// Platform > Billing; Partners at the root.
    fn tree() -> Vec<ContextNode> {
        vec![
            node(1, "Platform", None),
            node(2, "Billing", Some(1)),
            node(3, "Partners", None),
        ]
    }

    /// The repositories of the design's example, by the contexts each belongs to:
    /// api is in Platform, Billing and Partners; web in Platform; portal in Partners.
    const API: &[i64] = &[1, 2, 3];
    const WEB: &[i64] = &[1];
    const PORTAL: &[i64] = &[3];

    #[test]
    fn api_sees_its_contexts_and_their_ancestors_at_the_nearest_distance() {
        assert_eq!(
            visible_contexts(&tree(), API),
            vec![(1, 1), (2, 1), (3, 1)],
            "Platform is at distance 1, not 2 through Billing"
        );
    }

    #[test]
    fn web_sees_platform_only_and_portal_sees_partners_only() {
        assert_eq!(visible_contexts(&tree(), WEB), vec![(1, 1)]);
        assert_eq!(visible_contexts(&tree(), PORTAL), vec![(3, 1)]);
    }

    #[test]
    fn a_sub_context_is_not_visible_from_its_parents_other_repositories() {
        let visible = visible_contexts(&tree(), WEB);
        assert!(
            visible.iter().all(|&(id, _)| id != 2),
            "Billing stays out of web"
        );
    }

    #[test]
    fn an_ancestor_reached_by_two_routes_takes_the_distance_that_is_nearer() {
        let nodes = vec![
            node(1, "Root", None),
            node(2, "Mid", Some(1)),
            node(3, "Leaf", Some(2)),
        ];
        assert_eq!(
            visible_contexts(&nodes, &[3, 1]),
            vec![(1, 1), (3, 1), (2, 2)]
        );
    }

    #[test]
    fn a_cycle_in_the_stored_tree_ends_the_walk() {
        let nodes = vec![node(1, "A", Some(2)), node(2, "B", Some(1))];
        assert_eq!(visible_contexts(&nodes, &[1]), vec![(1, 1), (2, 2)]);
        assert_eq!(context_path(&nodes, 1), "B / A");
        assert!(
            would_cycle(&nodes, 1, Some(2)),
            "the walk ends and finds the context"
        );
        assert!(
            !would_cycle(&nodes, 3, Some(1)),
            "an unrelated context ends the walk"
        );
    }

    #[test]
    fn an_unknown_membership_gives_nothing() {
        assert_eq!(visible_contexts(&tree(), &[99]), Vec::new());
    }

    #[test]
    fn would_cycle_refuses_self_child_and_grandchild_but_not_unrelated() {
        let mut nodes = tree();
        nodes.push(node(4, "Invoices", Some(2)));
        assert!(would_cycle(&nodes, 1, Some(1)), "self");
        assert!(would_cycle(&nodes, 1, Some(2)), "child");
        assert!(would_cycle(&nodes, 1, Some(4)), "grandchild");
        assert!(!would_cycle(&nodes, 3, Some(2)), "unrelated");
        assert!(!would_cycle(&nodes, 2, None), "to the root is no cycle");
    }

    #[test]
    fn context_path_joins_the_names_from_the_root_down() {
        assert_eq!(context_path(&tree(), 2), "Platform / Billing");
        assert_eq!(context_path(&tree(), 1), "Platform");
        assert_eq!(context_path(&tree(), 99), "");
    }

    /// `tree` plus a second `Core` under Partners, for the ambiguous name.
    fn with_two_cores() -> Vec<ContextNode> {
        let mut nodes = tree();
        nodes.push(node(4, "Core", Some(1)));
        nodes.push(node(5, "Core", Some(3)));
        nodes
    }

    #[test]
    fn resolve_scope_reads_the_keywords_and_the_full_path() {
        let nodes = tree();
        let visible = visible_contexts(&nodes, API);
        assert_eq!(resolve_scope(&nodes, &visible, ""), Ok(Scope::All));
        assert_eq!(resolve_scope(&nodes, &visible, " all "), Ok(Scope::All));
        assert_eq!(
            resolve_scope(&nodes, &visible, "repository"),
            Ok(Scope::Repository)
        );
        assert_eq!(
            resolve_scope(&nodes, &visible, "Platform / Billing"),
            Ok(Scope::Context(2))
        );
        assert_eq!(
            resolve_scope(&nodes, &visible, "Platform/Billing"),
            Ok(Scope::Context(2)),
            "whitespace around / is ignored"
        );
    }

    #[test]
    fn resolve_scope_takes_a_name_that_is_unique_among_the_visible_contexts() {
        let nodes = tree();
        let visible = visible_contexts(&nodes, API);
        assert_eq!(
            resolve_scope(&nodes, &visible, "Partners"),
            Ok(Scope::Context(3))
        );
    }

    #[test]
    fn resolve_scope_refuses_an_ambiguous_name_and_names_the_candidates() {
        let nodes = with_two_cores();
        let visible = visible_contexts(&nodes, &[4, 5]);
        let message = resolve_scope(&nodes, &visible, "Core").expect_err("ambiguous");
        assert!(message.contains("\"Platform / Core\""), "{message}");
        assert!(message.contains("\"Partners / Core\""), "{message}");
        assert_eq!(
            resolve_scope(&nodes, &visible, "Partners / Core"),
            Ok(Scope::Context(5)),
            "a full path resolves the ambiguity"
        );
    }

    #[test]
    fn resolve_scope_refuses_a_name_no_visible_context_has() {
        let mut nodes = tree();
        nodes.push(node(4, "Infra", Some(1)));
        let visible = visible_contexts(&nodes, WEB);
        let message = resolve_scope(&nodes, &visible, "Infra").expect_err("invisible");
        assert_eq!(
            message,
            "No context named \"Infra\" is visible from this repository."
        );
    }
}
