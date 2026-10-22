//! Definitions by a plain walk of the syntax tree, with no query engine (spike
//! S-B). The same rules as `queries/rust-defs.scm`, the Python tags query and
//! the Go tags query with its alias fix, written as a match on node kinds.

use tree_sitter::Node;

/// What a node defines, as the tags kind the query version would give it.
fn classify(lang: &str, node: Node<'_>) -> Option<&'static str> {
    let kind = node.kind();
    match lang {
        "rust" => match kind {
            "function_item" => Some(
                if node
                    .parent()
                    .is_some_and(|p| p.kind() == "declaration_list")
                {
                    "definition.method"
                } else {
                    "definition.function"
                },
            ),
            "struct_item" | "enum_item" | "union_item" => Some("definition.class"),
            // An alias at the top of a file or of a module, not an associated type.
            "type_item" => {
                let parent = node.parent()?;
                let top = parent.kind() == "source_file"
                    || (parent.kind() == "declaration_list"
                        && parent.parent().is_some_and(|g| g.kind() == "mod_item"));
                top.then_some("definition.class")
            }
            "trait_item" => Some("definition.interface"),
            "mod_item" => Some("definition.module"),
            "macro_definition" => Some("definition.macro"),
            _ => None,
        },
        "python" => match kind {
            "function_definition" => Some("definition.function"),
            "class_definition" => Some("definition.class"),
            _ => None,
        },
        "go" => match kind {
            "function_declaration" => Some("definition.function"),
            "method_declaration" => Some("definition.method"),
            "type_spec" | "type_alias" => Some("definition.type"),
            _ => None,
        },
        _ => None,
    }
}

/// Calls `found(kind, name_node)` for every definition in the tree.
pub fn definitions<'t>(lang: &str, root: Node<'t>, mut found: impl FnMut(&'static str, Node<'t>)) {
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        if let Some(kind) = classify(lang, node) {
            if let Some(name) = node.child_by_field_name("name") {
                found(kind, name);
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}
