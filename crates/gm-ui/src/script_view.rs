//! The text of the Script tab of the Workflows page (D233): the workflow file as it is on disk,
//! read-only, with line numbers and the viewer's colors. The person edits the file in their own
//! editor; this module only turns the text and its highlight spans into lines to draw. The
//! highlighting runs off the UI thread (`shell_workflows`), so the text shows plain first.

use gm_schema::HighlightSpan;

use crate::code_page::{Piece, styled};

/// The script on screen: the text, where each of its lines lies, and the colors of the text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScriptText {
    source: String,
    /// The byte range of each line in `source`, without its line end.
    lines: Vec<(usize, usize)>,
    spans: Vec<HighlightSpan>,
    /// Whether `spans` are the colors of `source`; false while the highlighter works.
    colored: bool,
    /// The widest line, in characters: how far the text scrolls sideways.
    columns: usize,
}

impl ScriptText {
    /// `source` without colors. `None` when the script on screen already shows that text.
    pub(crate) fn plain(&self, source: &str) -> Option<Self> {
        (self.source != source).then(|| Self::build(source, Vec::new(), false))
    }

    /// `source` with the colors of `spans`.
    pub(crate) fn colored(source: &str, spans: Vec<HighlightSpan>) -> Self {
        Self::build(source, spans, true)
    }

    fn build(source: &str, spans: Vec<HighlightSpan>, colored: bool) -> Self {
        let mut lines = Vec::new();
        let mut columns = 0;
        let mut start = 0;
        let body = source.strip_suffix('\n').unwrap_or(source);
        if !source.is_empty() {
            for raw in body.split('\n') {
                let line = raw.strip_suffix('\r').unwrap_or(raw);
                columns = columns.max(line.chars().count());
                lines.push((start, start + line.len()));
                start += raw.len() + 1;
            }
        }
        Self {
            source: source.to_owned(),
            lines,
            spans,
            colored,
            columns,
        }
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    pub(crate) fn is_colored(&self) -> bool {
        self.colored
    }

    pub(crate) fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub(crate) fn columns(&self) -> usize {
        self.columns
    }

    /// The pieces of line `index` from the column `from`, as the file viewer draws a line.
    pub(crate) fn line(&self, index: usize, from: usize) -> Vec<Piece> {
        match self.lines.get(index) {
            Some(&(start, end)) => styled(&self.source[start..end], start, from, &self.spans),
            None => Vec::new(),
        }
    }
}

/// The index of the line a diagnostic names (1-based; 0 means it names none), kept inside the
/// text.
pub(crate) fn line_index(line: u32, line_count: usize) -> Option<usize> {
    let index = usize::try_from(line).ok()?.checked_sub(1)?;
    (index < line_count).then_some(index)
}

#[cfg(test)]
mod tests {
    use gm_schema::SyntaxRole;

    use super::*;

    fn text_of(pieces: &[Piece]) -> String {
        pieces.iter().map(|piece| piece.text.as_str()).collect()
    }

    #[test]
    fn a_source_is_split_into_its_lines_without_the_line_ends() {
        let script = ScriptText::colored("local M = {}\r\n\nreturn M\n", Vec::new());
        assert_eq!(script.line_count(), 3);
        assert_eq!(text_of(&script.line(0, 0)), "local M = {}");
        assert_eq!(text_of(&script.line(1, 0)), "");
        assert_eq!(text_of(&script.line(2, 0)), "return M");
        assert_eq!(script.columns(), 12);
        assert_eq!(ScriptText::colored("", Vec::new()).line_count(), 0);
        assert_eq!(ScriptText::colored("a", Vec::new()).line_count(), 1);
        assert_eq!(ScriptText::colored("\n", Vec::new()).line_count(), 1);
    }

    #[test]
    fn spans_color_the_pieces_of_the_line_they_fall_on() {
        let source = "local a\nreturn a\n";
        let spans = vec![
            HighlightSpan {
                start: 0,
                end: 5,
                role: SyntaxRole::Keyword,
            },
            HighlightSpan {
                start: 8,
                end: 14,
                role: SyntaxRole::Keyword,
            },
        ];
        let script = ScriptText::colored(source, spans);
        assert!(script.is_colored());
        let first = script.line(0, 0);
        assert_eq!(first[0].text, "local");
        assert_eq!(first[0].role, Some(SyntaxRole::Keyword));
        let second = script.line(1, 0);
        assert_eq!(second[0].text, "return");
        assert_eq!(second[0].role, Some(SyntaxRole::Keyword));
        assert_eq!(text_of(&second), "return a");
    }

    #[test]
    fn a_line_scrolled_sideways_starts_at_the_column_and_keeps_its_colors() {
        let spans = vec![HighlightSpan {
            start: 0,
            end: 5,
            role: SyntaxRole::Keyword,
        }];
        let script = ScriptText::colored("local año = 1\n", spans);
        let shown = script.line(0, 3);
        assert_eq!(text_of(&shown), "al año = 1");
        assert_eq!(shown[0].role, Some(SyntaxRole::Keyword));
        assert_eq!(text_of(&script.line(0, 7)), "ño = 1");
        assert!(script.line(0, 99).is_empty());
        assert!(script.line(9, 0).is_empty());
    }

    #[test]
    fn the_plain_lines_are_built_only_for_another_text() {
        let shown = ScriptText::default();
        let plain = shown.plain("return 1\n").expect("a new text");
        assert!(!plain.is_colored());
        assert_eq!(plain.source(), "return 1\n");
        assert_eq!(plain.plain("return 1\n"), None);
        assert!(plain.plain("return 2\n").is_some());
        assert_eq!(ScriptText::default().plain(""), None);
    }

    #[test]
    fn a_diagnostic_line_is_an_index_inside_the_text() {
        assert_eq!(line_index(1, 3), Some(0));
        assert_eq!(line_index(3, 3), Some(2));
        assert_eq!(line_index(0, 3), None);
        assert_eq!(line_index(4, 3), None);
    }
}
