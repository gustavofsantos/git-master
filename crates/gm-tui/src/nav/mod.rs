//! The companion's navigator (D241, D242, `design/tui-navigation.md`): keys and key maps,
//! modes, the action registry, the screen stack, the prompt guard and the section tree.
//! Plain Rust, with no terminal or drawing in it.

pub mod action;
pub mod defaults;
pub mod input;
pub mod keymap;
pub mod keys;
pub mod prompt;
pub mod sections;
pub mod stack;

/// What a key means depends on the mode (D241 (2)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mode {
    /// Moving and acting.
    Normal,
    /// Lines are selected: an action applies to them.
    Visual,
    /// Typing in a text box.
    Insert,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Normal, Mode::Visual, Mode::Insert];

    /// As the mode line shows it.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Visual => "VISUAL",
            Mode::Insert => "INSERT",
        }
    }
}
