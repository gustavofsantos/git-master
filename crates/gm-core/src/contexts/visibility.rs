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
    /// One context, by id, with its ancestors: the memories of a sub-context include those of every
    /// context above it (D222 (2)). The context need not be one the repository is bound to (D222 (3)).
    Context(i64),
}

/// Reads a scope name (D193 (5), D222 (3)). `all`, `bound` or empty is every scope the repository
/// reads by default (`Scope::All`), and `repository` is the repository's own memories. Anything else
/// names a context: first among the `visible` ones (the contexts the repository is bound to and their
/// ancestors), then among every context, by its full path (`Platform / Billing`, whitespace around `/`
/// ignored) or by a name that exactly one context has. A name several contexts have is refused and the
/// message lists their paths; a name no context has is refused as unknown.
pub fn resolve_scope(
    nodes: &[ContextNode],
    visible: &[(i64, u32)],
    text: &str,
) -> Result<Scope, String> {
    let text = text.trim();
    if matches!(text, "" | "all" | "bound") {
        return Ok(Scope::All);
    }
    if text == "repository" {
        return Ok(Scope::Repository);
    }
    let visible_ids: Vec<i64> = visible.iter().map(|&(id, _)| id).collect();
    if let Some(id) = resolve_among(nodes, &visible_ids, text)? {
        return Ok(Scope::Context(id));
    }
    let every: Vec<i64> = nodes.iter().map(|node| node.id).collect();
    match resolve_among(nodes, &every, text)? {
        Some(id) => Ok(Scope::Context(id)),
        None => Err(format!("No context named \"{text}\" exists.")),
    }
}

/// The context among `ids` that `text` names: the one whose full path it is, else the one name that
/// exactly one of them has. `None` when no id matches; refused, listing the paths, when several do.
fn resolve_among(nodes: &[ContextNode], ids: &[i64], text: &str) -> Result<Option<i64>, String> {
    let by_path: Vec<i64> = ids
        .iter()
        .copied()
        .filter(|&id| same_path(&context_path(nodes, id), text))
        .collect();
    let named: Vec<i64> = if by_path.is_empty() {
        ids.iter()
            .copied()
            .filter(|&id| name_of(nodes, id).trim() == text)
            .collect()
    } else {
        by_path
    };
    match named.as_slice() {
        [] => Ok(None),
        [id] => Ok(Some(*id)),
        several => {
            let paths: Vec<String> = several
                .iter()
                .map(|&id| format!("\"{}\"", context_path(nodes, id)))
                .collect();
            Err(format!(
                "\"{text}\" names several contexts: {}. Name one by its full path.",
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
    fn resolve_scope_refuses_a_name_no_context_has() {
        let nodes = tree();
        let visible = visible_contexts(&nodes, WEB);
        let message = resolve_scope(&nodes, &visible, "Nowhere").expect_err("unknown");
        assert_eq!(message, "No context named \"Nowhere\" exists.");
    }

    #[test]
    fn bound_all_and_empty_are_the_default_reach() {
        let nodes = tree();
        let visible = visible_contexts(&nodes, WEB);
        for text in ["", "all", "bound", " bound "] {
            assert_eq!(
                resolve_scope(&nodes, &visible, text),
                Ok(Scope::All),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_context_no_visible_repository_is_bound_to_is_read_by_its_name_and_path() {
        let nodes = tree();
        let visible = visible_contexts(&nodes, WEB);
        assert_eq!(
            resolve_scope(&nodes, &visible, "Partners"),
            Ok(Scope::Context(3)),
            "web does not see Partners, but it can read it"
        );
        assert_eq!(
            resolve_scope(&nodes, &visible, "Platform / Billing"),
            Ok(Scope::Context(2)),
            "a sub-context of a visible context is read by its path"
        );
    }

    #[test]
    fn a_visible_name_wins_over_the_same_name_of_a_context_elsewhere() {
        let mut nodes = tree();
        nodes.push(node(4, "Billing", Some(3)));
        let visible = visible_contexts(&nodes, API);
        assert_eq!(
            resolve_scope(&nodes, &visible, "Billing"),
            Ok(Scope::Context(2)),
            "Platform / Billing is visible to api, Partners / Billing is not"
        );
    }

    #[test]
    fn a_name_several_contexts_have_is_refused_listing_every_path_even_when_not_visible() {
        let mut nodes = tree();
        nodes.push(node(4, "Core", Some(3)));
        nodes.push(node(5, "Core", Some(3)));
        let visible = visible_contexts(&nodes, WEB);
        let message = resolve_scope(&nodes, &visible, "Core").expect_err("ambiguous");
        assert!(message.contains("several"), "{message}");
        assert_eq!(
            message.matches("\"Partners / Core\"").count(),
            2,
            "both paths are listed: {message}"
        );
    }

    #[test]
    fn a_named_context_lists_itself_then_its_ancestors() {
        assert_eq!(visible_contexts(&tree(), &[2]), vec![(2, 1), (1, 2)]);
    }
}
