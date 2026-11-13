//! Routes a key to what it does (D241, `design/tui-navigation.md`, Input routing): through
//! the pending prefix, if any, and the key map of the mode. Two keys are safe whatever the
//! map says: `Ctrl+C` quits, and `Esc` leaves a pending prefix.

use std::collections::BTreeMap;

use super::Mode;
use super::keymap::{KeyMap, Node, Step};
use super::keys::{Code, Key};

/// What a key turned out to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Run this action.
    Action(&'static str),
    /// A prefix: the keys so far, waiting for the next one; the menu shows what may follow.
    Pending(Vec<Key>),
    /// The pending prefix was left (`Esc`, or a key it does not bind).
    Left,
    /// Typed text, in Insert mode.
    Type(char),
    Quit,
    /// Nothing is bound to it here.
    Unbound,
}

/// The key maps, one per mode, and the prefix pressed so far.
#[derive(Clone, Debug, Default)]
pub struct Input {
    maps: BTreeMap<Mode, KeyMap>,
    pending: Vec<Key>,
}

impl Input {
    pub fn new(maps: BTreeMap<Mode, KeyMap>) -> Self {
        Self {
            maps,
            pending: Vec::new(),
        }
    }

    pub fn map(&self, mode: Mode) -> Option<&KeyMap> {
        self.maps.get(&mode)
    }

    /// The prefix pressed so far, and the node that lists what may follow it.
    pub fn pending(&self, mode: Mode) -> Option<(&[Key], &Node)> {
        if self.pending.is_empty() {
            return None;
        }
        match self.maps.get(&mode)?.step(&self.pending) {
            Step::Prefix(node) => Some((&self.pending, node)),
            _ => None,
        }
    }

    /// Leaves the pending prefix, as a menu entry that ran does.
    pub fn leave(&mut self) {
        self.pending.clear();
    }

    /// Goes back to the prefix `keys`, as a menu that stays open for repeating does.
    pub fn keep(&mut self, keys: &[Key]) {
        self.pending = keys.to_vec();
    }

    /// What `key` does in `mode`.
    pub fn feed(&mut self, mode: Mode, key: Key) -> Outcome {
        if key == Key::ctrl('c') {
            self.pending.clear();
            return Outcome::Quit;
        }
        let waiting = !self.pending.is_empty();
        if waiting && key == Key::plain(Code::Esc) {
            self.pending.clear();
            return Outcome::Left;
        }
        let mut keys = self.pending.clone();
        keys.push(key);
        let step = self
            .maps
            .get(&mode)
            .map_or(Step::Unbound, |map| map.step(&keys));
        match step {
            Step::Action(action) => {
                self.pending.clear();
                Outcome::Action(action)
            }
            Step::Prefix(_) => {
                self.pending = keys;
                Outcome::Pending(self.pending.clone())
            }
            Step::Unbound if waiting => {
                self.pending.clear();
                Outcome::Left
            }
            Step::Unbound => match key.code {
                Code::Char(c) if mode == Mode::Insert && !key.ctrl && !key.alt => Outcome::Type(c),
                _ => Outcome::Unbound,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Input {
        let mut normal = KeyMap::default();
        normal.bind("j", "cursor.down").unwrap();
        normal.bind("<space> s", "stage").unwrap();
        normal.bind("<c-c>", "something.else").unwrap();
        let mut insert = KeyMap::default();
        insert.bind("<c-s>", "text.send").unwrap();
        Input::new(BTreeMap::from([
            (Mode::Normal, normal),
            (Mode::Insert, insert),
        ]))
    }

    #[test]
    fn a_prefix_waits_for_its_next_key() {
        let mut input = input();
        assert_eq!(
            input.feed(Mode::Normal, Key::char('j')),
            Outcome::Action("cursor.down")
        );
        assert_eq!(
            input.feed(Mode::Normal, Key::char(' ')),
            Outcome::Pending(vec![Key::char(' ')])
        );
        assert!(input.pending(Mode::Normal).is_some());
        assert_eq!(
            input.feed(Mode::Normal, Key::char('s')),
            Outcome::Action("stage")
        );
        assert!(input.pending(Mode::Normal).is_none());
    }

    #[test]
    fn esc_and_an_unbound_key_leave_a_prefix() {
        let mut input = input();
        input.feed(Mode::Normal, Key::char(' '));
        assert_eq!(
            input.feed(Mode::Normal, Key::plain(Code::Esc)),
            Outcome::Left
        );
        input.feed(Mode::Normal, Key::char(' '));
        assert_eq!(input.feed(Mode::Normal, Key::char('x')), Outcome::Left);
        assert_eq!(input.feed(Mode::Normal, Key::char('x')), Outcome::Unbound);
    }

    #[test]
    fn ctrl_c_quits_whatever_the_map_binds() {
        let mut input = input();
        input.feed(Mode::Normal, Key::char(' '));
        assert_eq!(input.feed(Mode::Normal, Key::ctrl('c')), Outcome::Quit);
    }

    #[test]
    fn insert_mode_types_what_its_map_does_not_bind() {
        let mut input = input();
        assert_eq!(input.feed(Mode::Insert, Key::char('j')), Outcome::Type('j'));
        assert_eq!(
            input.feed(Mode::Insert, Key::ctrl('s')),
            Outcome::Action("text.send")
        );
        assert_eq!(input.feed(Mode::Insert, Key::ctrl('x')), Outcome::Unbound);
    }
}
