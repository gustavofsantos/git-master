//! A key map: key sequences bound to action names, as a trie (D241). A leaf is an action;
//! an inner node is a prefix, and pressing it waits for the next key while the menu shows
//! what may follow. The leader is an ordinary prefix.

use std::collections::BTreeMap;

use super::keys::{Key, parse_sequence, show_sequence};

/// What a node of the trie holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    /// The action bound here, for a leaf.
    pub action: Option<&'static str>,
    /// What may follow, for a prefix.
    pub children: BTreeMap<Key, Node>,
    /// The name the menu gives this prefix, such as `Diff…`.
    pub label: Option<String>,
}

/// What a key does where the map is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step<'a> {
    /// The sequence names this action.
    Action(&'static str),
    /// The sequence is a prefix: wait for the next key; the node lists what may follow.
    Prefix(&'a Node),
    /// Nothing is bound to it.
    Unbound,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyMap {
    root: Node,
}

impl KeyMap {
    /// Binds `sequence` (in the notation, such as `<space> s`) to `action`. A sequence
    /// that would make a leaf a prefix, or a prefix a leaf, or that is bound already, is
    /// refused with the reason; the map is left as it was.
    pub fn bind(&mut self, sequence: &str, action: &'static str) -> Result<(), String> {
        let keys = parse_sequence(sequence)?;
        let mut node = &mut self.root;
        for (at, key) in keys.iter().enumerate() {
            if node.action.is_some() {
                return Err(format!(
                    "{} is bound to an action, so {sequence:?} cannot follow it",
                    show_sequence(&keys[..at])
                ));
            }
            node = node.children.entry(*key).or_default();
        }
        if let Some(existing) = node.action {
            return Err(format!("{sequence:?} is bound to {existing} already"));
        }
        if !node.children.is_empty() {
            return Err(format!("{sequence:?} is a prefix of other keys"));
        }
        node.action = Some(action);
        Ok(())
    }

    /// Names the prefix `sequence` for the menu, such as `Diff…`.
    pub fn label(&mut self, sequence: &str, label: &str) -> Result<(), String> {
        let keys = parse_sequence(sequence)?;
        let mut node = &mut self.root;
        for key in &keys {
            node = node.children.entry(*key).or_default();
        }
        node.label = Some(label.to_owned());
        Ok(())
    }

    /// Removes whatever `sequence` binds, leaf or prefix. Used when a person's map rebinds
    /// (Q50); used by tests until then.
    #[cfg(test)]
    pub fn unbind(&mut self, sequence: &str) -> Result<(), String> {
        let keys = parse_sequence(sequence)?;
        let (last, path) = keys.split_last().ok_or("an empty key sequence")?;
        let mut node = &mut self.root;
        for key in path {
            let Some(next) = node.children.get_mut(key) else {
                return Ok(());
            };
            node = next;
        }
        node.children.remove(last);
        Ok(())
    }

    /// What `keys`, pressed in order from the top, do.
    pub fn step(&self, keys: &[Key]) -> Step<'_> {
        let mut node = &self.root;
        for key in keys {
            match node.children.get(key) {
                Some(next) => node = next,
                None => return Step::Unbound,
            }
        }
        match node.action {
            Some(action) if !keys.is_empty() => Step::Action(action),
            _ if keys.is_empty() || !node.children.is_empty() => Step::Prefix(node),
            _ => Step::Unbound,
        }
    }

    /// Every binding, as (sequence, action), in key order: for the keys screen.
    pub fn bindings(&self) -> Vec<(Vec<Key>, &'static str)> {
        fn walk(node: &Node, path: &mut Vec<Key>, out: &mut Vec<(Vec<Key>, &'static str)>) {
            if let Some(action) = node.action {
                out.push((path.clone(), action));
            }
            for (key, child) in &node.children {
                path.push(*key);
                walk(child, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut Vec::new(), &mut out);
        out
    }

    /// The keys that run `action`, shortest first, for showing its key next to it.
    pub fn keys_of(&self, action: &str) -> Vec<Vec<Key>> {
        let mut found: Vec<Vec<Key>> = self
            .bindings()
            .into_iter()
            .filter(|(_, bound)| *bound == action)
            .map(|(keys, _)| keys)
            .collect();
        found.sort_by_key(Vec::len);
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<Key> {
        parse_sequence(text).unwrap()
    }

    #[test]
    fn a_prefix_waits_and_a_leaf_acts() {
        let mut map = KeyMap::default();
        map.bind("j", "cursor.down").unwrap();
        map.bind("g g", "cursor.top").unwrap();
        map.bind("<space> s", "stage").unwrap();
        map.label("<space>", "Actions").unwrap();
        assert_eq!(map.step(&keys("j")), Step::Action("cursor.down"));
        let Step::Prefix(node) = map.step(&keys("<space>")) else {
            panic!("not a prefix")
        };
        assert_eq!(node.label.as_deref(), Some("Actions"));
        assert_eq!(node.children.len(), 1);
        assert_eq!(map.step(&keys("<space> s")), Step::Action("stage"));
        assert_eq!(map.step(&keys("g g")), Step::Action("cursor.top"));
        assert_eq!(map.step(&keys("g x")), Step::Unbound);
        assert_eq!(map.step(&keys("x")), Step::Unbound);
    }

    #[test]
    fn a_binding_that_clashes_is_refused_and_changes_nothing() {
        let mut map = KeyMap::default();
        map.bind("g g", "cursor.top").unwrap();
        map.bind("G", "cursor.bottom").unwrap();
        assert!(map.bind("g", "x").is_err(), "g is a prefix");
        assert!(map.bind("G x", "x").is_err(), "G is a leaf");
        assert!(map.bind("g g", "x").is_err(), "bound already");
        assert_eq!(map.step(&keys("g g")), Step::Action("cursor.top"));
    }

    #[test]
    fn the_leader_can_move_to_another_key() {
        let mut map = KeyMap::default();
        map.bind("<space> s", "stage").unwrap();
        map.unbind("<space>").unwrap();
        map.bind(", s", "stage").unwrap();
        assert_eq!(map.step(&keys("<space> s")), Step::Unbound);
        assert_eq!(map.step(&keys(", s")), Step::Action("stage"));
        assert_eq!(map.keys_of("stage"), [keys(", s")]);
    }
}
