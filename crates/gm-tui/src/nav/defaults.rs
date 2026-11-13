//! The default key maps and the actions they name (D241 (5), D242 (2)): the universal keys,
//! vim's for muscle memory, the leader `Space` and its menu, and what is borrowed from
//! less and the Unix terminal. A person's map, later, replaces bindings by action name.

use std::collections::BTreeMap;

use super::Mode;
use super::action::Behavior::{OneShot, Repeat};
use super::action::{Behavior, Registry, Spec};
use super::input::Input;
use super::keymap::KeyMap;

const MOVE: &[Mode] = &[Mode::Normal, Mode::Visual];
const INSERT: &[Mode] = &[Mode::Insert];

const fn spec(
    name: &'static str,
    title: &'static str,
    description: &'static str,
    group: &'static str,
    modes: &'static [Mode],
    behavior: Behavior,
) -> Spec {
    Spec {
        name,
        title,
        description,
        group,
        modes,
        behavior,
    }
}

/// Every action of the companion, in menu order within each group.
const SPECS: &[Spec] = &[
    spec(
        "cursor.down",
        "down",
        "Move the cursor down a row.",
        "Move",
        MOVE,
        OneShot,
    ),
    spec(
        "cursor.up",
        "up",
        "Move the cursor up a row.",
        "Move",
        MOVE,
        OneShot,
    ),
    spec(
        "cursor.page_down",
        "half page down",
        "Move the cursor half a screen down.",
        "Move",
        MOVE,
        OneShot,
    ),
    spec(
        "cursor.page_up",
        "half page up",
        "Move the cursor half a screen up.",
        "Move",
        MOVE,
        OneShot,
    ),
    spec(
        "cursor.top",
        "top",
        "Go to the first row.",
        "Move",
        MOVE,
        OneShot,
    ),
    spec(
        "cursor.bottom",
        "bottom",
        "Go to the last row.",
        "Move",
        MOVE,
        OneShot,
    ),
    spec(
        "hunk.next",
        "next hunk",
        "Go to the next hunk.",
        "Move",
        MOVE,
        Repeat,
    ),
    spec(
        "hunk.previous",
        "previous hunk",
        "Go to the previous hunk.",
        "Move",
        MOVE,
        Repeat,
    ),
    spec(
        "select.toggle",
        "select lines",
        "Start or end a selection of lines.",
        "Move",
        MOVE,
        OneShot,
    ),
    spec(
        "select.down",
        "extend down",
        "Extend the selection down a row.",
        "Move",
        MOVE,
        OneShot,
    ),
    spec(
        "select.up",
        "extend up",
        "Extend the selection up a row.",
        "Move",
        MOVE,
        OneShot,
    ),
    spec(
        "fold.toggle",
        "fold",
        "Fold or unfold the section at the cursor.",
        "Fold",
        MOVE,
        OneShot,
    ),
    spec(
        "fold.open",
        "unfold",
        "Unfold the section at the cursor.",
        "Fold",
        MOVE,
        OneShot,
    ),
    spec(
        "fold.close",
        "fold",
        "Fold the section at the cursor.",
        "Fold",
        MOVE,
        OneShot,
    ),
    spec(
        "fold.open_all",
        "unfold all",
        "Unfold every section.",
        "Fold",
        MOVE,
        OneShot,
    ),
    spec(
        "fold.close_all",
        "fold all",
        "Fold every section.",
        "Fold",
        MOVE,
        OneShot,
    ),
    spec(
        "open",
        "open",
        "Open what is at the cursor.",
        "Go to",
        MOVE,
        OneShot,
    ),
    spec(
        "screen.back",
        "back",
        "Clear the selection, or go back one screen.",
        "Go to",
        MOVE,
        OneShot,
    ),
    spec(
        "search.start",
        "search",
        "Search the screen.",
        "Search",
        MOVE,
        OneShot,
    ),
    spec(
        "search.next",
        "next match",
        "Go to the next match.",
        "Search",
        MOVE,
        Repeat,
    ),
    spec(
        "search.previous",
        "previous match",
        "Go to the previous match.",
        "Search",
        MOVE,
        Repeat,
    ),
    spec(
        "stage",
        "stage",
        "Stage what is under the cursor or selected.",
        "Stage",
        MOVE,
        Repeat,
    ),
    spec(
        "unstage",
        "unstage",
        "Unstage what is under the cursor or selected.",
        "Stage",
        MOVE,
        Repeat,
    ),
    spec(
        "stage.file",
        "stage file",
        "Stage the whole file at the cursor.",
        "Stage",
        MOVE,
        OneShot,
    ),
    spec(
        "review.comment",
        "comment",
        "Start a review thread on the selected lines.",
        "Review",
        MOVE,
        OneShot,
    ),
    spec(
        "thread.reply",
        "reply",
        "Reply to the thread at the cursor.",
        "Review",
        MOVE,
        OneShot,
    ),
    spec(
        "thread.resolve",
        "resolve",
        "Resolve the thread at the cursor.",
        "Review",
        MOVE,
        OneShot,
    ),
    spec(
        "thread.reopen",
        "reopen",
        "Reopen the resolved thread at the cursor.",
        "Review",
        MOVE,
        OneShot,
    ),
    spec(
        "thread.delete",
        "delete",
        "Delete the thread at the cursor, after asking.",
        "Review",
        MOVE,
        OneShot,
    ),
    spec(
        "copy.reference",
        "copy @file:lines",
        "Copy a reference to the lines, for your agent's prompt.",
        "Review",
        MOVE,
        OneShot,
    ),
    spec(
        "go.threads",
        "threads",
        "Open the Threads screen.",
        "Go to",
        MOVE,
        OneShot,
    ),
    spec(
        "file.open",
        "file",
        "Open the whole file on the File screen.",
        "Go to",
        MOVE,
        OneShot,
    ),
    spec(
        "blame.open",
        "blame",
        "Show who last changed each line of the file on the Blame screen.",
        "Go to",
        MOVE,
        OneShot,
    ),
    spec(
        "origin.open",
        "why",
        "Show how the selected lines, or the line, got where they are.",
        "Go to",
        MOVE,
        OneShot,
    ),
    spec(
        "thread.show",
        "show in diff",
        "Show the thread under its lines on Status.",
        "Go to",
        MOVE,
        OneShot,
    ),
    spec(
        "filter.open",
        "open",
        "Show the threads not resolved.",
        "Filter",
        MOVE,
        OneShot,
    ),
    spec(
        "filter.resolved",
        "resolved",
        "Show the resolved threads.",
        "Filter",
        MOVE,
        OneShot,
    ),
    spec(
        "filter.outdated",
        "outdated",
        "Show the threads whose lines are not in the file any more.",
        "Filter",
        MOVE,
        OneShot,
    ),
    spec(
        "filter.needs_answer",
        "needs answer",
        "Show the agent's questions no one has answered.",
        "Filter",
        MOVE,
        OneShot,
    ),
    spec(
        "filter.agent",
        "agent",
        "Show the threads an agent started.",
        "Filter",
        MOVE,
        OneShot,
    ),
    spec(
        "filter.all",
        "all",
        "Show every thread.",
        "Filter",
        MOVE,
        OneShot,
    ),
    spec(
        "go.branch",
        "branch changes",
        "Show the branch's changes against its base.",
        "Diff",
        MOVE,
        OneShot,
    ),
    spec(
        "help.keys",
        "keys",
        "Show every key of this screen.",
        "Help",
        MOVE,
        OneShot,
    ),
    spec(
        "terminal.suspend",
        "suspend",
        "Hand the terminal to the shell; fg brings the companion back.",
        "Help",
        MOVE,
        OneShot,
    ),
    spec(
        "terminal.redraw",
        "redraw",
        "Draw the screen again.",
        "Help",
        MOVE,
        OneShot,
    ),
    spec(
        "quit",
        "quit",
        "Quit the companion.",
        "Help",
        MOVE,
        Behavior::OneShot,
    ),
    spec(
        "text.send",
        "send",
        "Send what the box holds.",
        "Write",
        INSERT,
        OneShot,
    ),
    spec(
        "text.cancel",
        "cancel",
        "Close the box, asking first when it holds text.",
        "Write",
        INSERT,
        OneShot,
    ),
    spec(
        "text.newline",
        "new line",
        "Start a new line.",
        "Write",
        INSERT,
        OneShot,
    ),
    spec(
        "text.backspace",
        "erase",
        "Erase the last character.",
        "Write",
        INSERT,
        OneShot,
    ),
    spec(
        "screen.close",
        "back or quit",
        "Clear the selection, go back one screen, or quit on Status.",
        "Go to",
        MOVE,
        OneShot,
    ),
    spec(
        "text.type",
        "next type",
        "Choose the next type of comment: change, question, nit.",
        "Write",
        INSERT,
        OneShot,
    ),
];

/// The registry of every action.
pub fn registry() -> Registry {
    Registry::new(SPECS.to_vec())
}

/// The default bindings, one list per mode: (sequence, action).
const NORMAL: &[(&str, &str)] = &[
    ("<down>", "cursor.down"),
    ("j", "cursor.down"),
    ("<up>", "cursor.up"),
    ("k", "cursor.up"),
    ("<pagedown>", "cursor.page_down"),
    ("<c-d>", "cursor.page_down"),
    ("<pageup>", "cursor.page_up"),
    ("<c-u>", "cursor.page_up"),
    ("<home>", "cursor.top"),
    ("g g", "cursor.top"),
    ("<end>", "cursor.bottom"),
    ("G", "cursor.bottom"),
    ("] c", "hunk.next"),
    ("[ c", "hunk.previous"),
    ("V", "select.toggle"),
    ("<s-down>", "select.down"),
    ("<s-up>", "select.up"),
    ("<tab>", "fold.toggle"),
    ("z a", "fold.toggle"),
    ("z o", "fold.open"),
    ("z c", "fold.close"),
    ("z R", "fold.open_all"),
    ("z M", "fold.close_all"),
    ("<cr>", "open"),
    ("<esc>", "screen.back"),
    ("q", "screen.close"),
    ("/", "search.start"),
    ("n", "search.next"),
    ("N", "search.previous"),
    ("?", "help.keys"),
    ("<c-z>", "terminal.suspend"),
    ("<c-l>", "terminal.redraw"),
    ("<space> s", "stage"),
    ("<space> u", "unstage"),
    ("<space> S", "stage.file"),
    ("<space> c", "review.comment"),
    ("<space> r", "thread.reply"),
    ("<space> R", "thread.resolve"),
    ("<space> o", "thread.reopen"),
    ("<space> D", "thread.delete"),
    ("<space> y", "copy.reference"),
    ("<space> t", "go.threads"),
    ("<space> g", "thread.show"),
    ("g f", "file.open"),
    ("g b", "blame.open"),
    ("<space> w", "origin.open"),
    ("<space> f o", "filter.open"),
    ("<space> f r", "filter.resolved"),
    ("<space> f u", "filter.outdated"),
    ("<space> f n", "filter.needs_answer"),
    ("<space> f a", "filter.agent"),
    ("<space> f A", "filter.all"),
    ("<space> d b", "go.branch"),
    ("<space> ?", "help.keys"),
];

const INSERT_KEYS: &[(&str, &str)] = &[
    ("<c-s>", "text.send"),
    ("<esc>", "text.cancel"),
    ("<cr>", "text.newline"),
    ("<bs>", "text.backspace"),
    ("<tab>", "text.type"),
];

const LABELS: &[(&str, &str)] = &[
    ("<space>", "Actions"),
    ("<space> d", "Diff"),
    ("<space> f", "Filter"),
    ("g", "Go"),
    ("z", "Fold"),
    ("]", "Next"),
    ("[", "Previous"),
];

/// The default input: the maps of every mode. Visual takes Normal's keys.
///
/// # Panics
///
/// If the built-in bindings clash, which a test rules out.
pub fn input() -> Input {
    let mut normal = KeyMap::default();
    for (keys, action) in NORMAL {
        normal
            .bind(keys, action)
            .expect("the default map does not clash");
    }
    for (keys, label) in LABELS {
        normal.label(keys, label).expect("a valid label");
    }
    let mut insert = KeyMap::default();
    for (keys, action) in INSERT_KEYS {
        insert
            .bind(keys, action)
            .expect("the default map does not clash");
    }
    Input::new(BTreeMap::from([
        (Mode::Normal, normal.clone()),
        (Mode::Visual, normal),
        (Mode::Insert, insert),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_binding_names_an_action_of_its_mode() {
        let registry = registry();
        let input = input();
        for mode in Mode::ALL {
            for (keys, action) in input.map(mode).unwrap().bindings() {
                let spec = registry
                    .get(action)
                    .unwrap_or_else(|| panic!("{action} is not in the registry"));
                assert!(
                    spec.modes.contains(&mode),
                    "{action} bound in {mode:?} by {keys:?}"
                );
            }
        }
    }

    #[test]
    fn every_action_has_a_description_for_the_keys_screen_and_a_palette() {
        for spec in registry().all() {
            assert!(!spec.description.is_empty(), "{}", spec.name);
            assert!(spec.description.ends_with('.'), "{}", spec.name);
        }
    }
}
