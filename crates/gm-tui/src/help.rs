//! The Keys screen (`?`): every action bound in Normal mode, with its keys and what it
//! does, grouped as the menu groups them. A screen on the stack, so it scrolls, searches
//! and folds like Status, and `q` or `Esc` goes back (D241 (1)).

use crate::nav::Mode;
use crate::nav::action::Registry;
use crate::nav::input::Input;
use crate::nav::keys::show_sequence;
use crate::nav::sections::Section;
use crate::status::Item;

/// The Keys screen's sections, from the registry and the Normal map.
pub fn sections(registry: &Registry, input: &Input) -> Vec<Section<Item>> {
    let Some(map) = input.map(Mode::Normal) else {
        return Vec::new();
    };
    let mut out: Vec<Section<Item>> = Vec::new();
    for spec in registry.all() {
        let keys = map.keys_of(spec.name);
        if keys.is_empty() {
            continue;
        }
        let keys: Vec<String> = keys.iter().map(|k| show_sequence(k)).collect();
        let line = Section::leaf(
            format!("binding/{}", spec.name),
            Item::Binding {
                keys: keys.join(", "),
                description: spec.description,
            },
        );
        // A group gathers its actions wherever the registry declares them.
        let found = out.iter_mut().find(
            |group| matches!(group.item, Item::KeyGroup { title, .. } if title == spec.group),
        );
        match found {
            Some(group) => {
                group.children.push(line);
                if let Item::KeyGroup { count, .. } = &mut group.item {
                    *count += 1;
                }
            }
            None => out.push(Section::with(
                format!("keys/{}", spec.group),
                Item::KeyGroup {
                    title: spec.group,
                    count: 1,
                },
                vec![line],
            )),
        }
    }
    out
}
