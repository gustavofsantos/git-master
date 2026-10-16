//! Commit message composer: the multi-line text area the commit box is made of,
//! plus the commit shortcut and a subject-line check.

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, KeyBinding, Render,
    Window, actions, div, prelude::*,
};

use crate::text_area::{self, TextArea};

actions!(gitmaster_composer, [CommitRequested]);

/// The key context the commit shortcut is bound under.
const KEY_CONTEXT: &str = "Composer";
const SUBJECT_LIMIT: usize = 72;

/// What the composer asks its owner to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerEvent {
    /// The user pressed the commit shortcut (⌘↵ on macOS, Ctrl+↵ elsewhere).
    CommitRequested,
}

/// The commit message box. It fills the height its owner gives it and
/// scrolls its text inside that height.
pub struct Composer {
    area: Entity<TextArea>,
}

impl Composer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let area = cx.new(|cx| TextArea::new("Commit message", cx));
        Self { area }
    }

    /// The message as typed, with `\n` line breaks.
    pub fn text(&self, cx: &App) -> String {
        self.area.read(cx).text().to_owned()
    }

    pub fn clear(&self, cx: &mut Context<Self>) {
        self.area.update(cx, TextArea::clear);
    }

    /// Gives the keyboard to the message box.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.area.update(cx, |area, cx| area.focus(window, cx));
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.area.read(cx).focus_handle(cx)
    }
}

/// Registers the composer's keys and the text area's keys. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    text_area::bind_keys(cx);
    cx.bind_keys([KeyBinding::new(
        "secondary-enter",
        CommitRequested,
        Some(KEY_CONTEXT),
    )]);
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|_, _: &CommitRequested, _, cx| {
                cx.emit(ComposerEvent::CommitRequested);
            }))
            .child(self.area.clone())
    }
}

/// Characters on the first line of `text`, the commit subject.
#[must_use]
pub fn subject_len(text: &str) -> usize {
    text.lines().next().map_or(0, |line| line.chars().count())
}

/// A hint about the subject line, or `None` when the message looks right. A
/// hint is given when the subject is over 72 characters, or when a body follows
/// the subject without a blank line between them. A subject alone gets no hint.
#[must_use]
pub fn subject_hint(text: &str) -> Option<&'static str> {
    if subject_len(text) > SUBJECT_LIMIT {
        return Some("The subject is over 72 characters. Shorten it.");
    }
    let mut lines = text.lines();
    lines.next();
    match lines.next() {
        Some(second) if !second.trim().is_empty() => {
            Some("Add a blank line between the subject and the body.")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_subject() -> String {
        "x".repeat(SUBJECT_LIMIT + 1)
    }

    #[test]
    fn the_commit_shortcut_is_a_valid_key_binding() {
        // A malformed keystroke panics when bound at startup; this catches it earlier.
        let binding = KeyBinding::new("secondary-enter", CommitRequested, Some(KEY_CONTEXT));
        assert_eq!(binding.keystrokes().len(), 1);
    }

    #[test]
    fn subject_len_counts_chars_of_the_first_line() {
        assert_eq!(subject_len(""), 0);
        assert_eq!(subject_len("fix"), 3);
        assert_eq!(subject_len("fix parser\nbody"), 10);
        assert_eq!(subject_len("é日😀\nx"), 3);
        assert_eq!(subject_len("\nfirst line is empty"), 0);
    }

    #[test]
    fn a_subject_of_exactly_72_chars_gets_no_hint() {
        let text = "x".repeat(SUBJECT_LIMIT);
        assert_eq!(subject_hint(&text), None);
    }

    #[test]
    fn a_subject_over_72_chars_gets_a_length_hint() {
        let hint = subject_hint(&long_subject());
        assert_eq!(hint, Some("The subject is over 72 characters. Shorten it."));
    }

    #[test]
    fn a_body_right_after_the_subject_gets_a_blank_line_hint() {
        assert_eq!(
            subject_hint("fix parser\nbody text"),
            Some("Add a blank line between the subject and the body.")
        );
    }

    #[test]
    fn a_blank_line_after_the_subject_gets_no_hint() {
        assert_eq!(subject_hint("fix parser\n\nbody text"), None);
        assert_eq!(subject_hint("fix parser\n   \nbody"), None);
    }

    #[test]
    fn a_subject_alone_or_with_a_trailing_newline_gets_no_hint() {
        assert_eq!(subject_hint(""), None);
        assert_eq!(subject_hint("fix parser"), None);
        assert_eq!(subject_hint("fix parser\n"), None);
    }

    #[test]
    fn the_length_hint_wins_over_the_blank_line_hint() {
        let text = format!("{}\nbody", long_subject());
        assert_eq!(
            subject_hint(&text),
            Some("The subject is over 72 characters. Shorten it.")
        );
    }
}
