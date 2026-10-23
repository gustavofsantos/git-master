// Ported from rvw (local checkout ~/Projects/rvw): crates/rvw-view/src/editor.rs, MIT, same author.
// Copied into gm-ui from the b-text spike (removed from the tree; see git history: git show 7932c61:spikes/b-text/FINDINGS.md).
//! A multi-line text buffer with a cursor, a selection and undo.
//!
//! Offsets are byte offsets and always sit on char boundaries. Newlines are
//! `\n` only. Consecutive typed characters, and consecutive backspaces, share
//! one undo step; any other edit or cursor move ends that run.

use std::collections::VecDeque;
use std::ops::Range;

/// Undo history holds at most this many steps.
const HISTORY_CAP: usize = 200;

/// Text, cursor and anchor at one point in time.
type Snapshot = (String, usize, usize);

#[derive(Debug, Clone, Default)]
pub struct Editor {
    text: String,
    anchor: usize,
    cursor: usize,
    /// The column Up and Down keep trying to return to.
    preferred_col: Option<usize>,
    /// The kind of the last edit, while a coalescing run may continue.
    last_edit: Option<EditKind>,
    undo: VecDeque<Snapshot>,
    redo: VecDeque<Snapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Typing,
    Backspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Space,
    Newline,
    Punct,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn class_of(c: char) -> CharClass {
    if c == '\n' {
        CharClass::Newline
    } else if c.is_whitespace() {
        CharClass::Space
    } else if is_word(c) {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

impl Editor {
    /// An empty editor.
    pub fn new() -> Editor {
        Editor::default()
    }

    /// An editor holding `text`, with the cursor at the end and no history.
    #[cfg(test)]
    pub fn from_text(text: &str) -> Editor {
        let text = normalize_newlines(text);
        let end = text.len();
        Editor {
            text,
            anchor: end,
            cursor: end,
            ..Editor::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// True when the text holds only whitespace, or nothing at all.
    #[cfg(test)]
    pub fn is_blank(&self) -> bool {
        self.text.chars().all(char::is_whitespace)
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The selected range, ordered start..end. Empty when nothing is selected.
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.cursor
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    /// Replaces the whole text, puts the cursor at the end and clears history.
    pub fn set_text(&mut self, text: &str) {
        self.text = normalize_newlines(text);
        self.anchor = self.text.len();
        self.cursor = self.anchor;
        self.reset_run();
        self.undo.clear();
        self.redo.clear();
    }

    /// Moves the cursor to `offset`. With `extend`, the anchor stays put.
    pub fn set_cursor(&mut self, offset: usize, extend: bool) {
        let offset = self.clamp(offset);
        self.place(offset, extend);
    }

    /// Replaces the selection with `s`, or inserts it at the cursor.
    pub fn insert(&mut self, s: &str) {
        let s = normalize_newlines(s);
        let range = self.selection();
        if s.is_empty() && range.is_empty() {
            return;
        }
        let single_char = s.chars().count() == 1 && s != "\n";
        let kind = single_char.then_some(EditKind::Typing);
        self.begin_edit(kind);
        self.text.replace_range(range.clone(), &s);
        let end = range.start + s.len();
        self.anchor = end;
        self.cursor = end;
        self.preferred_col = None;
    }

    /// Deletes the selection, or the character before the cursor.
    pub fn backspace(&mut self) {
        let (range, kind) = if self.has_selection() {
            (self.selection(), None)
        } else {
            (
                self.prev_boundary(self.cursor)..self.cursor,
                Some(EditKind::Backspace),
            )
        };
        self.delete_range(range, kind);
    }

    /// Deletes the selection, or the character after the cursor.
    pub fn delete(&mut self) {
        let range = if self.has_selection() {
            self.selection()
        } else {
            self.cursor..self.next_boundary(self.cursor)
        };
        self.delete_range(range, None);
    }

    /// Deletes the selection, or back to the start of the previous word.
    pub fn delete_word_left(&mut self) {
        let range = if self.has_selection() {
            self.selection()
        } else {
            self.word_back(self.cursor)..self.cursor
        };
        self.delete_range(range, None);
    }

    /// Deletes the selection, or forward to the start of the next word.
    pub fn delete_word_right(&mut self) {
        let range = if self.has_selection() {
            self.selection()
        } else {
            self.cursor..self.word_forward(self.cursor)
        };
        self.delete_range(range, None);
    }

    /// Deletes the selection, or back to the start of the cursor's line. At the
    /// start of a line it removes the newline before it.
    pub fn delete_to_line_start(&mut self) {
        let range = if self.has_selection() {
            self.selection()
        } else {
            let line_start = self.line_start_of(self.cursor);
            let start = if self.cursor == line_start {
                line_start.saturating_sub(1)
            } else {
                line_start
            };
            start..self.cursor
        };
        self.delete_range(range, None);
    }

    /// Moves one character left. A selection collapses to its start.
    pub fn move_left(&mut self, extend: bool) {
        let target = if self.has_selection() && !extend {
            self.selection().start
        } else {
            self.prev_boundary(self.cursor)
        };
        self.place(target, extend);
    }

    /// Moves one character right. A selection collapses to its end.
    pub fn move_right(&mut self, extend: bool) {
        let target = if self.has_selection() && !extend {
            self.selection().end
        } else {
            self.next_boundary(self.cursor)
        };
        self.place(target, extend);
    }

    pub fn word_left(&mut self, extend: bool) {
        let target = self.word_back(self.cursor);
        self.place(target, extend);
    }

    pub fn word_right(&mut self, extend: bool) {
        let target = self.word_forward(self.cursor);
        self.place(target, extend);
    }

    /// Moves to the start of the cursor's logical line.
    pub fn line_start(&mut self, extend: bool) {
        let target = self.line_start_of(self.cursor);
        self.place(target, extend);
    }

    /// Moves to the end of the cursor's logical line.
    pub fn line_end(&mut self, extend: bool) {
        let target = self.line_end_of(self.cursor);
        self.place(target, extend);
    }

    pub fn doc_start(&mut self, extend: bool) {
        self.place(0, extend);
    }

    pub fn doc_end(&mut self, extend: bool) {
        let end = self.len();
        self.place(end, extend);
    }

    /// Moves up one line, keeping the preferred column. On the first line it
    /// moves to the start of the document.
    pub fn move_up(&mut self, extend: bool) {
        let (line, col) = self.line_col(self.cursor);
        let want = self.preferred_col.unwrap_or(col);
        let target = if line == 0 {
            0
        } else {
            self.offset_at_col(self.line_range(line - 1), want)
        };
        self.place(target, extend);
        self.preferred_col = Some(want);
    }

    /// Moves down one line, keeping the preferred column. On the last line it
    /// moves to the end of the document.
    pub fn move_down(&mut self, extend: bool) {
        let (line, col) = self.line_col(self.cursor);
        let want = self.preferred_col.unwrap_or(col);
        let target = if line + 1 >= self.line_count() {
            self.len()
        } else {
            self.offset_at_col(self.line_range(line + 1), want)
        };
        self.place(target, extend);
        self.preferred_col = Some(want);
    }

    pub fn select_all(&mut self) {
        let end = self.len();
        self.select(0, end);
    }

    /// Selects the run under `offset`: a word, a whitespace run within one
    /// line, or a punctuation run. A newline is its own run. An offset at the
    /// end of the text selects the run before it.
    pub fn select_word_at(&mut self, offset: usize) {
        let off = self.clamp(offset);
        let pivot = if off < self.len() {
            off
        } else {
            self.prev_boundary(off)
        };
        let Some(class) = self.text[pivot..].chars().next().map(class_of) else {
            self.select(0, 0);
            return;
        };
        let start = self.text[..pivot]
            .char_indices()
            .rev()
            .take_while(|&(_, c)| class_of(c) == class)
            .last()
            .map_or(pivot, |(i, _)| i);
        let end = self.text[pivot..]
            .char_indices()
            .take_while(|&(_, c)| class_of(c) == class)
            .last()
            .map_or(pivot, |(i, c)| pivot + i + c.len_utf8());
        self.select(start, end);
    }

    /// Selects the whole logical line under `offset`, without its newline.
    pub fn select_line_at(&mut self, offset: usize) {
        let off = self.clamp(offset);
        let start = self.line_start_of(off);
        let end = self.line_end_of(off);
        self.select(start, end);
    }

    /// Reverts the last edit step. Returns false when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop_back() else {
            return false;
        };
        let current = self.snapshot();
        self.redo.push_back(current);
        self.restore(previous);
        true
    }

    /// Re-applies the last undone step. Returns false when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop_back() else {
            return false;
        };
        self.push_undo();
        self.restore(next);
        true
    }

    /// The (line, column) of `offset`, both 0-based; the column counts chars.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let off = self.clamp(offset);
        let line = self.text[..off].matches('\n').count();
        let col = self.text[self.line_start_of(off)..off].chars().count();
        (line, col)
    }

    /// The number of lines. Always at least 1.
    pub fn line_count(&self) -> usize {
        self.text.matches('\n').count() + 1
    }

    /// The byte range of `line`, without its newline. Lines past the end
    /// give the last line.
    pub fn line_range(&self, line: usize) -> Range<usize> {
        let line = line.min(self.line_count() - 1);
        let start: usize = self
            .text
            .split('\n')
            .take(line)
            .map(|segment| segment.len() + 1)
            .sum();
        let len = self.text.split('\n').nth(line).map_or(0, str::len);
        start..start + len
    }

    /// Clamps `offset` to the text and down to the nearest char boundary.
    fn clamp(&self, offset: usize) -> usize {
        let mut off = offset.min(self.text.len());
        while !self.text.is_char_boundary(off) {
            off -= 1;
        }
        off
    }

    fn snapshot(&self) -> Snapshot {
        (self.text.clone(), self.cursor, self.anchor)
    }

    fn restore(&mut self, (text, cursor, anchor): Snapshot) {
        self.text = text;
        self.cursor = cursor;
        self.anchor = anchor;
        self.reset_run();
    }

    fn push_undo(&mut self) {
        let snapshot = self.snapshot();
        self.undo.push_back(snapshot);
        if self.undo.len() > HISTORY_CAP {
            self.undo.pop_front();
        }
    }

    /// Records undo before an edit of `kind`, unless it continues the last run.
    /// Call it only for an edit that changes the text.
    fn begin_edit(&mut self, kind: Option<EditKind>) {
        let continues_run = kind.is_some() && kind == self.last_edit && !self.has_selection();
        if !continues_run {
            self.push_undo();
        }
        self.redo.clear();
        self.last_edit = kind;
    }

    fn delete_range(&mut self, range: Range<usize>, kind: Option<EditKind>) {
        if range.is_empty() {
            return;
        }
        self.begin_edit(kind);
        self.text.replace_range(range.clone(), "");
        self.anchor = range.start;
        self.cursor = range.start;
        self.preferred_col = None;
    }

    /// Moves the cursor, collapsing the selection unless `extend` is set.
    fn place(&mut self, offset: usize, extend: bool) {
        if !extend {
            self.anchor = offset;
        }
        self.cursor = offset;
        self.reset_run();
    }

    fn select(&mut self, anchor: usize, cursor: usize) {
        self.anchor = anchor;
        self.cursor = cursor;
        self.reset_run();
    }

    /// Any cursor change ends a coalescing run and forgets the preferred column.
    fn reset_run(&mut self) {
        self.last_edit = None;
        self.preferred_col = None;
    }

    fn prev_boundary(&self, offset: usize) -> usize {
        self.text[..offset]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.text[offset..]
            .chars()
            .next()
            .map_or(offset, |c| offset + c.len_utf8())
    }

    /// Start of the logical line containing `offset`.
    fn line_start_of(&self, offset: usize) -> usize {
        self.text[..offset].rfind('\n').map_or(0, |i| i + 1)
    }

    /// End of the logical line containing `offset`, before its newline.
    fn line_end_of(&self, offset: usize) -> usize {
        self.text[offset..]
            .find('\n')
            .map_or(self.len(), |i| offset + i)
    }

    /// The offset `col` chars into `range`, or the range end if it is shorter.
    fn offset_at_col(&self, range: Range<usize>, col: usize) -> usize {
        self.text[range.clone()]
            .char_indices()
            .nth(col)
            .map_or(range.end, |(i, _)| range.start + i)
    }

    /// Start of the word to the left of `from`: skips whitespace, then one run
    /// of word chars or one run of punctuation.
    fn word_back(&self, from: usize) -> usize {
        let mut chars = self.text[..from].char_indices().rev().peekable();
        let mut pos = from;
        while let Some(&(i, c)) = chars.peek() {
            if !c.is_whitespace() {
                break;
            }
            pos = i;
            chars.next();
        }
        if let Some(&(_, first)) = chars.peek() {
            let word = is_word(first);
            while let Some(&(i, c)) = chars.peek() {
                if c.is_whitespace() || is_word(c) != word {
                    break;
                }
                pos = i;
                chars.next();
            }
        }
        pos
    }

    /// End of the word to the right of `from`: skips whitespace, then one run
    /// of word chars or one run of punctuation.
    fn word_forward(&self, from: usize) -> usize {
        let mut chars = self.text[from..].chars().peekable();
        let mut pos = from;
        while let Some(&c) = chars.peek() {
            if !c.is_whitespace() {
                break;
            }
            pos += c.len_utf8();
            chars.next();
        }
        if let Some(&first) = chars.peek() {
            let word = is_word(first);
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || is_word(c) != word {
                    break;
                }
                pos += c.len_utf8();
                chars.next();
            }
        }
        pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> Editor {
        let mut e = Editor::new();
        for c in s.chars() {
            e.insert(&c.to_string());
        }
        e
    }

    #[test]
    fn typing_and_backspace() {
        let mut e = typed("hi");
        assert_eq!(e.text(), "hi");
        assert_eq!(e.cursor(), 2);
        e.backspace();
        assert_eq!(e.text(), "h");
        e.backspace();
        e.backspace();
        assert_eq!(e.text(), "");
        assert!(e.is_empty());
        assert!(e.is_blank());
    }

    #[test]
    fn multibyte_chars_move_and_delete_whole() {
        let mut e = Editor::from_text("aé日😀");
        assert_eq!(e.len(), 10);
        e.move_left(false);
        assert_eq!(e.cursor(), 6);
        e.move_left(false);
        assert_eq!(e.cursor(), 3);
        e.move_left(false);
        assert_eq!(e.cursor(), 1);
        e.move_right(false);
        assert_eq!(e.cursor(), 3);
        e.backspace();
        assert_eq!(e.text(), "a日😀");
        assert_eq!(e.cursor(), 1);
        e.delete();
        assert_eq!(e.text(), "a😀");
        e.move_right(false);
        e.backspace();
        assert_eq!(e.text(), "a");
        assert_eq!(e.cursor(), 1);
    }

    #[test]
    fn newlines_normalise_crlf_and_lone_cr() {
        let mut e = Editor::new();
        e.insert("a\r\nb");
        assert_eq!(e.text(), "a\nb");
        assert_eq!(e.cursor(), 3);
        e.insert("\r");
        assert_eq!(e.text(), "a\nb\n");
        assert_eq!(Editor::from_text("x\r\ny").text(), "x\ny");
    }

    #[test]
    fn extend_then_typing_replaces_selection() {
        let mut e = Editor::from_text("hello world");
        e.set_cursor(0, false);
        for _ in 0..5 {
            e.move_right(true);
        }
        assert_eq!(e.selection(), 0..5);
        assert_eq!(e.selected_text(), "hello");
        e.insert("bye");
        assert_eq!(e.text(), "bye world");
        assert_eq!(e.cursor(), 3);
        assert!(!e.has_selection());
    }

    #[test]
    fn selection_extends_backwards() {
        let mut e = Editor::from_text("hello world");
        for _ in 0..5 {
            e.move_left(true);
        }
        assert_eq!(e.selection(), 6..11);
        assert_eq!(e.selected_text(), "world");
    }

    #[test]
    fn plain_moves_collapse_selection_to_its_edges() {
        let mut e = Editor::from_text("abcdef");
        e.set_cursor(1, false);
        e.set_cursor(4, true);
        e.move_left(false);
        assert_eq!(e.cursor(), 1);
        assert!(!e.has_selection());
        e.set_cursor(1, false);
        e.set_cursor(4, true);
        e.move_right(false);
        assert_eq!(e.cursor(), 4);
        assert!(!e.has_selection());
    }

    #[test]
    fn word_moves_over_identifiers_and_punctuation() {
        let mut e = Editor::from_text("foo_bar baz.qux");
        e.set_cursor(0, false);
        let mut right = Vec::new();
        for _ in 0..4 {
            e.word_right(false);
            right.push(e.cursor());
        }
        assert_eq!(right, vec![7, 11, 12, 15]);
        let mut left = Vec::new();
        for _ in 0..4 {
            e.word_left(false);
            left.push(e.cursor());
        }
        assert_eq!(left, vec![12, 11, 8, 0]);
    }

    #[test]
    fn word_move_with_extend_selects_the_word() {
        let mut e = Editor::from_text("foo bar");
        e.set_cursor(0, false);
        e.word_right(true);
        assert_eq!(e.selected_text(), "foo");
    }

    #[test]
    fn delete_word_left_and_right() {
        let mut e = Editor::from_text("foo_bar baz.qux");
        e.delete_word_left();
        assert_eq!(e.text(), "foo_bar baz.");
        e.delete_word_left();
        assert_eq!(e.text(), "foo_bar baz");
        e.delete_word_left();
        assert_eq!(e.text(), "foo_bar ");
        e.delete_word_left();
        assert_eq!(e.text(), "");

        let mut e = Editor::from_text("foo bar");
        e.set_cursor(0, false);
        e.delete_word_right();
        assert_eq!(e.text(), " bar");
        assert_eq!(e.cursor(), 0);
        e.delete_word_right();
        assert_eq!(e.text(), "");
    }

    #[test]
    fn delete_to_line_start_in_middle_and_at_line_start() {
        let mut e = Editor::from_text("one\ntwo three");
        e.set_cursor(9, false);
        e.delete_to_line_start();
        assert_eq!(e.text(), "one\nhree");
        assert_eq!(e.cursor(), 4);

        e.set_cursor(4, false);
        e.delete_to_line_start();
        assert_eq!(e.text(), "onehree");
        assert_eq!(e.cursor(), 3);
    }

    #[test]
    fn delete_to_line_start_at_document_start_is_a_no_op() {
        let mut e = Editor::from_text("abc");
        e.set_cursor(0, false);
        e.delete_to_line_start();
        assert_eq!(e.text(), "abc");
        assert!(!e.undo());
    }

    #[test]
    fn up_and_down_keep_the_preferred_column() {
        let mut e = Editor::from_text("long line here\nab\nlonger text");
        e.set_cursor(10, false);
        e.move_down(false);
        assert_eq!(e.cursor(), 17);
        e.move_down(false);
        assert_eq!(e.cursor(), 28);
        e.move_up(false);
        assert_eq!(e.cursor(), 17);
        e.move_up(false);
        assert_eq!(e.cursor(), 10);
    }

    #[test]
    fn a_horizontal_move_resets_the_preferred_column() {
        let mut e = Editor::from_text("long line here\nab\nlonger text");
        e.set_cursor(10, false);
        e.move_down(false);
        e.move_left(false);
        assert_eq!(e.cursor(), 16);
        e.move_down(false);
        assert_eq!(e.cursor(), 19);
    }

    #[test]
    fn up_on_first_line_and_down_on_last_line_go_to_document_edges() {
        let mut e = Editor::from_text("abc\ndef");
        e.set_cursor(2, false);
        e.move_up(false);
        assert_eq!(e.cursor(), 0);
        e.set_cursor(5, false);
        e.move_down(false);
        assert_eq!(e.cursor(), 7);
    }

    #[test]
    fn select_all_and_select_word_at() {
        let mut e = Editor::from_text("abc");
        e.select_all();
        assert_eq!(e.selected_text(), "abc");

        let mut e = Editor::from_text("foo bar.baz");
        e.select_word_at(1);
        assert_eq!(e.selection(), 0..3);
        e.select_word_at(3);
        assert_eq!(e.selected_text(), " ");
        e.select_word_at(7);
        assert_eq!(e.selected_text(), ".");
        e.select_word_at(4);
        assert_eq!(e.selected_text(), "bar");
        e.select_word_at(11);
        assert_eq!(e.selection(), 8..11);
    }

    #[test]
    fn select_word_at_does_not_cross_a_newline() {
        let mut e = Editor::from_text("a \n b");
        e.select_word_at(1);
        assert_eq!(e.selected_text(), " ");
        e.select_word_at(2);
        assert_eq!(e.selected_text(), "\n");
    }

    #[test]
    fn select_word_at_on_empty_text_selects_nothing() {
        let mut e = Editor::new();
        e.select_word_at(5);
        assert!(!e.has_selection());
    }

    #[test]
    fn select_line_at_excludes_newline() {
        let mut e = Editor::from_text("one\ntwo\nthree");
        e.select_line_at(5);
        assert_eq!(e.selected_text(), "two");
        e.select_line_at(0);
        assert_eq!(e.selected_text(), "one");
        e.select_line_at(3);
        assert_eq!(e.selected_text(), "one");
    }

    #[test]
    fn typed_chars_coalesce_into_one_undo_step() {
        let mut e = typed("abc");
        assert!(e.undo());
        assert_eq!(e.text(), "");
        assert!(!e.undo());
        assert!(e.redo());
        assert_eq!(e.text(), "abc");
        assert!(!e.redo());
    }

    #[test]
    fn newline_breaks_coalescing() {
        let mut e = typed("ab");
        e.insert("\n");
        e.insert("c");
        assert_eq!(e.text(), "ab\nc");
        assert!(e.undo());
        assert_eq!(e.text(), "ab\n");
        assert!(e.undo());
        assert_eq!(e.text(), "ab");
        assert!(e.undo());
        assert_eq!(e.text(), "");
        assert!(!e.undo());
    }

    #[test]
    fn cursor_move_breaks_coalescing() {
        let mut e = typed("a");
        e.set_cursor(0, false);
        e.insert("b");
        assert_eq!(e.text(), "ba");
        assert!(e.undo());
        assert_eq!(e.text(), "a");
        assert!(e.undo());
        assert_eq!(e.text(), "");
    }

    #[test]
    fn backspace_run_coalesces_and_breaks_when_kind_changes() {
        let mut e = Editor::from_text("abc");
        e.backspace();
        e.backspace();
        assert_eq!(e.text(), "a");
        assert!(e.undo());
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), 3);
        assert!(!e.undo());

        let mut e = typed("a");
        e.backspace();
        assert!(e.undo());
        assert_eq!(e.text(), "a");
        assert!(e.undo());
        assert_eq!(e.text(), "");
    }

    #[test]
    fn paste_is_one_undo_step() {
        let mut e = Editor::from_text("x");
        e.insert("hello");
        assert_eq!(e.text(), "xhello");
        assert!(e.undo());
        assert_eq!(e.text(), "x");
        assert_eq!(e.cursor(), 1);
    }

    #[test]
    fn replacing_selection_is_its_own_step_and_restores_selection() {
        let mut e = Editor::from_text("abc");
        e.select_all();
        e.insert("x");
        e.insert("y");
        assert_eq!(e.text(), "xy");
        assert!(e.undo());
        assert_eq!(e.text(), "abc");
        assert_eq!(e.selection(), 0..3);
        assert!(!e.undo());
    }

    #[test]
    fn deleting_a_selection_is_undoable_and_redoable() {
        let mut e = Editor::from_text("abc");
        e.select_all();
        e.backspace();
        assert_eq!(e.text(), "");
        assert!(e.undo());
        assert_eq!(e.text(), "abc");
        assert_eq!(e.selection(), 0..3);
        assert!(e.redo());
        assert_eq!(e.text(), "");
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut e = typed("a");
        assert!(e.undo());
        e.insert("b");
        assert!(!e.redo());
        assert_eq!(e.text(), "b");
    }

    #[test]
    fn history_is_capped_at_200_steps() {
        let mut e = Editor::new();
        for _ in 0..250 {
            e.insert("\n");
        }
        assert_eq!(e.line_count(), 251);
        let mut undone = 0;
        while e.undo() {
            undone += 1;
        }
        assert_eq!(undone, 200);
        assert_eq!(e.line_count(), 51);
    }

    #[test]
    fn set_text_clears_history() {
        let mut e = typed("a");
        e.set_text("zz");
        assert_eq!(e.cursor(), 2);
        assert!(!e.undo());
    }

    #[test]
    fn line_col_and_line_range_on_empty_text() {
        let e = Editor::new();
        assert_eq!(e.line_col(0), (0, 0));
        assert_eq!(e.line_count(), 1);
        assert_eq!(e.line_range(0), 0..0);
    }

    #[test]
    fn line_col_and_line_range_when_text_ends_in_newline() {
        let e = Editor::from_text("a\nbc\n");
        assert_eq!(e.line_count(), 3);
        assert_eq!(e.line_col(3), (1, 1));
        assert_eq!(e.line_col(5), (2, 0));
        assert_eq!(e.line_range(0), 0..1);
        assert_eq!(e.line_range(1), 2..4);
        assert_eq!(e.line_range(2), 5..5);
    }

    #[test]
    fn line_col_and_line_range_for_a_middle_line() {
        let e = Editor::from_text("x\nhello\ny");
        assert_eq!(e.line_count(), 3);
        assert_eq!(e.line_col(5), (1, 3));
        assert_eq!(e.line_range(1), 2..7);
    }

    #[test]
    fn line_col_counts_chars_not_bytes() {
        let e = Editor::from_text("é\nab");
        assert_eq!(e.line_col(4), (1, 1));
    }

    #[test]
    fn offsets_inside_a_multibyte_char_clamp_down() {
        let mut e = Editor::from_text("é");
        e.set_cursor(1, false);
        assert_eq!(e.cursor(), 0);
        assert_eq!(e.line_col(1), (0, 0));
    }

    #[test]
    fn blank_means_only_whitespace() {
        assert!(Editor::from_text(" \n\t").is_blank());
        assert!(Editor::new().is_blank());
        assert!(!Editor::from_text("a ").is_blank());
    }

    #[test]
    fn line_moves_and_document_moves() {
        let mut e = Editor::from_text("one\ntwo");
        e.line_start(false);
        assert_eq!(e.cursor(), 4);
        e.line_end(false);
        assert_eq!(e.cursor(), 7);
        e.doc_start(true);
        assert_eq!(e.selection(), 0..7);
        e.doc_end(false);
        assert_eq!(e.cursor(), 7);
    }
}
