// Ported from rvw (local checkout ~/Projects/rvw): crates/rvw-gui/src/text_area/keys.rs, MIT, same author.
// Spike copy, see ../FINDINGS.md.
//! The area's actions and their key bindings.

use gpui::{App, KeyBinding, actions};

use super::KEY_CONTEXT;

actions!(
    rvw_text_area,
    [
        Newline,
        Submit,
        Cancel,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        LineStart,
        LineEnd,
        SelectLineStart,
        SelectLineEnd,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        SelectAll,
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        ShowCharacterPalette,
    ]
);

/// Registers the area's keys under the `RvwTextArea` key context. Call once
/// at startup. `secondary` is Cmd on macOS and Ctrl elsewhere; where Ctrl
/// already moves by word, line moves use `cmd-` (macOS) and Home/End.
pub fn bind_keys(cx: &mut App) {
    let c = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("enter", Newline, c),
        KeyBinding::new("shift-enter", Newline, c),
        KeyBinding::new("secondary-enter", Submit, c),
        KeyBinding::new("escape", Cancel, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("alt-left", WordLeft, c),
        KeyBinding::new("ctrl-left", WordLeft, c),
        KeyBinding::new("alt-right", WordRight, c),
        KeyBinding::new("ctrl-right", WordRight, c),
        KeyBinding::new("shift-alt-left", SelectWordLeft, c),
        KeyBinding::new("shift-ctrl-left", SelectWordLeft, c),
        KeyBinding::new("shift-alt-right", SelectWordRight, c),
        KeyBinding::new("shift-ctrl-right", SelectWordRight, c),
        KeyBinding::new("home", LineStart, c),
        KeyBinding::new("end", LineEnd, c),
        KeyBinding::new("cmd-left", LineStart, c),
        KeyBinding::new("cmd-right", LineEnd, c),
        KeyBinding::new("shift-home", SelectLineStart, c),
        KeyBinding::new("shift-end", SelectLineEnd, c),
        KeyBinding::new("shift-cmd-left", SelectLineStart, c),
        KeyBinding::new("shift-cmd-right", SelectLineEnd, c),
        KeyBinding::new("secondary-up", DocStart, c),
        KeyBinding::new("secondary-down", DocEnd, c),
        KeyBinding::new("ctrl-home", DocStart, c),
        KeyBinding::new("ctrl-end", DocEnd, c),
        KeyBinding::new("secondary-shift-up", SelectDocStart, c),
        KeyBinding::new("secondary-shift-down", SelectDocEnd, c),
        KeyBinding::new("ctrl-shift-home", SelectDocStart, c),
        KeyBinding::new("ctrl-shift-end", SelectDocEnd, c),
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("alt-backspace", DeleteWordLeft, c),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, c),
        KeyBinding::new("alt-delete", DeleteWordRight, c),
        KeyBinding::new("ctrl-delete", DeleteWordRight, c),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, c),
        KeyBinding::new("secondary-a", SelectAll, c),
        KeyBinding::new("secondary-z", Undo, c),
        KeyBinding::new("secondary-shift-z", Redo, c),
        KeyBinding::new("secondary-y", Redo, c),
        KeyBinding::new("secondary-x", Cut, c),
        KeyBinding::new("secondary-c", Copy, c),
        KeyBinding::new("secondary-v", Paste, c),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, c),
    ]);
}
