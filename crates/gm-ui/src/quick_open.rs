//! The model of Quick Open (docs/09, N1): the query typed, the search in
//! flight, the last answer and the highlighted row. Nothing here touches GPUI,
//! and nothing searches: the engine does, over the file list it holds in memory,
//! and answers with the best files. A reply to a search that a newer one
//! superseded is dropped, so an answer never lands under a query it was not
//! made for.

use gm_schema::{FileHit, FileSearch, OperationId};

/// What Enter does on the highlighted file: open `path`, at `line` if the query
/// ended with `:line`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub path: String,
    pub line: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuickOpen {
    pub open: bool,
    query: String,
    /// The search in flight, if any.
    request: Option<OperationId>,
    /// The newest answer. It may be for an older query than `query`: it stays
    /// on screen until the answer to the newer one comes, so the list does not
    /// blink while typing.
    answer: Option<FileSearch>,
    highlighted: usize,
}

impl QuickOpen {
    /// Shows Quick Open empty. The list the engine holds is asked for again by
    /// the caller, so a file made a moment ago is found.
    pub fn open(&mut self) {
        *self = Self {
            open: true,
            ..Self::default()
        };
    }

    pub fn close(&mut self) {
        self.open = false;
        self.request = None;
    }

    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Takes the text of the input. Returns whether it changed, which is when a
    /// new search is due.
    pub fn set_query(&mut self, text: &str) -> bool {
        if self.query == text {
            return false;
        }
        text.clone_into(&mut self.query);
        true
    }

    /// Records the search just sent. Any earlier one is superseded by it.
    pub fn requested(&mut self, id: OperationId) {
        self.request = Some(id);
    }

    /// Shows the answer when it is the search in flight. Returns whether it was.
    pub fn received(&mut self, id: OperationId, search: FileSearch) -> bool {
        if self.request != Some(id) {
            return false;
        }
        self.request = None;
        self.highlighted = 0;
        self.answer = Some(search);
        true
    }

    /// A search ended without an answer: it failed, or was cancelled.
    pub fn failed(&mut self, id: OperationId) {
        if self.request == Some(id) {
            self.request = None;
        }
    }

    /// Whether `id` is the search in flight.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.request == Some(id)
    }

    #[must_use]
    pub fn hits(&self) -> &[FileHit] {
        self.answer.as_ref().map_or(&[], |a| &a.hits)
    }

    #[must_use]
    pub fn highlighted(&self) -> usize {
        self.highlighted
    }

    /// Moves the highlight, stopping at the first and the last row.
    pub fn move_highlight(&mut self, delta: isize) {
        let last = self.hits().len().saturating_sub(1);
        self.highlighted = self.highlighted.saturating_add_signed(delta).min(last);
    }

    /// Highlights row `index`, if there is one.
    pub fn highlight(&mut self, index: usize) {
        if index < self.hits().len() {
            self.highlighted = index;
        }
    }

    /// What Enter opens: the highlighted file, with the line the query named.
    /// `None` when the answer lists nothing.
    #[must_use]
    pub fn choice(&self) -> Option<Choice> {
        let hit = self.hits().get(self.highlighted)?;
        Some(Choice {
            path: hit.path.clone(),
            line: self.answer.as_ref().and_then(|a| a.line),
        })
    }

    /// The one line under the list, which says what the list is.
    #[must_use]
    pub fn status(&self) -> String {
        let Some(answer) = &self.answer else {
            return if self.request.is_some() {
                "Reading the file list…".to_owned()
            } else {
                String::new()
            };
        };
        let files = count(answer.files, "file");
        let reading = if answer.indexing {
            " (still reading the list)"
        } else {
            ""
        };
        if self.query.trim().is_empty() {
            return format!("Type to search {files}{reading}");
        }
        if answer.hits.is_empty() {
            return format!("No file matches in {files}{reading}");
        }
        if answer.matched > answer.hits.len() {
            return format!(
                "{} of {} matches in {files}{reading}",
                answer.hits.len(),
                answer.matched
            );
        }
        format!("{} in {files}{reading}", count(answer.matched, "match"))
    }
}

fn count(n: usize, noun: &str) -> String {
    let mut digits = n.to_string();
    // Thousands are grouped, so a big repository reads at a glance.
    let mut at = digits.len();
    while at > 3 {
        at -= 3;
        digits.insert(at, ',');
    }
    format!("{digits} {noun}{}", if n == 1 { "" } else { "s" })
}

/// Splits `path` into the segments to draw: each is text and whether it is
/// highlighted. `ranges` are byte ranges of `path`, in order, as the engine
/// gives them; one that does not fall on character boundaries is ignored.
#[must_use]
pub fn segments<'a>(path: &'a str, ranges: &[(usize, usize)]) -> Vec<(&'a str, bool)> {
    let mut out = Vec::new();
    let mut at = 0;
    for &(start, end) in ranges {
        if start < at
            || end <= start
            || end > path.len()
            || !path.is_char_boundary(start)
            || !path.is_char_boundary(end)
        {
            continue;
        }
        if start > at {
            out.push((&path[at..start], false));
        }
        out.push((&path[start..end], true));
        at = end;
    }
    if at < path.len() {
        out.push((&path[at..], false));
    }
    out
}

/// The path split into the folder and the file name, for a row that shows the
/// name first and the folder fainter. The folder keeps its trailing `/`.
#[must_use]
pub fn split_name(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(at) => path.split_at(at + 1),
        None => ("", path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str) -> FileHit {
        FileHit {
            path: path.into(),
            ranges: Vec::new(),
        }
    }

    fn search(query: &str, paths: &[&str], matched: usize, files: usize) -> FileSearch {
        FileSearch {
            query: query.into(),
            hits: paths.iter().map(|p| hit(p)).collect(),
            matched,
            files,
            indexing: false,
            line: None,
        }
    }

    fn ready(paths: &[&str]) -> QuickOpen {
        let mut quick = QuickOpen::default();
        quick.open();
        quick.set_query("x");
        quick.requested(OperationId(1));
        quick.received(OperationId(1), search("x", paths, paths.len(), 100));
        quick
    }

    #[test]
    fn opening_starts_over_and_closing_forgets_the_search_in_flight() {
        let mut quick = ready(&["a.rs"]);
        quick.open();
        assert!(quick.open);
        assert_eq!(quick.query(), "");
        assert!(quick.hits().is_empty());

        quick.requested(OperationId(2));
        quick.close();
        assert!(!quick.open);
        assert!(!quick.owns(OperationId(2)));
    }

    #[test]
    fn a_changed_query_asks_for_a_search_and_the_same_text_does_not() {
        let mut quick = QuickOpen::default();
        assert!(quick.set_query("dp"));
        assert!(!quick.set_query("dp"));
        assert!(quick.set_query("dpa"));
    }

    #[test]
    fn only_the_answer_to_the_search_in_flight_is_shown() {
        let mut quick = QuickOpen::default();
        quick.open();
        quick.requested(OperationId(1));
        quick.requested(OperationId(2));

        assert!(!quick.received(OperationId(1), search("a", &["old.rs"], 1, 1)));
        assert!(quick.hits().is_empty());
        assert!(quick.received(OperationId(2), search("ab", &["new.rs"], 1, 1)));
        assert_eq!(quick.hits()[0].path, "new.rs");
        // The same reply twice is not shown twice.
        assert!(!quick.received(OperationId(2), search("ab", &["other.rs"], 1, 1)));
        assert_eq!(quick.hits()[0].path, "new.rs");
    }

    #[test]
    fn the_last_answer_stays_while_the_next_is_awaited() {
        let mut quick = ready(&["a.rs", "b.rs"]);
        quick.set_query("xy");
        quick.requested(OperationId(2));

        assert_eq!(quick.hits().len(), 2);
    }

    #[test]
    fn a_new_answer_puts_the_highlight_on_its_first_row() {
        let mut quick = ready(&["a.rs", "b.rs", "c.rs"]);
        quick.move_highlight(2);
        assert_eq!(quick.highlighted(), 2);

        quick.requested(OperationId(2));
        quick.received(OperationId(2), search("y", &["d.rs", "e.rs"], 2, 100));

        assert_eq!(quick.highlighted(), 0);
    }

    #[test]
    fn the_highlight_stops_at_both_ends() {
        let mut quick = ready(&["a.rs", "b.rs", "c.rs"]);
        quick.move_highlight(-1);
        assert_eq!(quick.highlighted(), 0);
        quick.move_highlight(1);
        quick.move_highlight(1);
        quick.move_highlight(1);
        quick.move_highlight(1);
        assert_eq!(quick.highlighted(), 2);
        quick.highlight(1);
        assert_eq!(quick.highlighted(), 1);
        quick.highlight(9);
        assert_eq!(quick.highlighted(), 1);
    }

    #[test]
    fn moving_over_an_empty_list_does_nothing() {
        let mut quick = QuickOpen::default();
        quick.move_highlight(1);
        quick.move_highlight(-1);
        assert_eq!(quick.highlighted(), 0);
        assert_eq!(quick.choice(), None);
    }

    #[test]
    fn enter_opens_the_highlighted_file_at_the_line_the_query_named() {
        let mut quick = QuickOpen::default();
        quick.open();
        quick.requested(OperationId(1));
        let mut answer = search("b.rs:42", &["a/b.rs", "c/b.rs"], 2, 9);
        answer.line = Some(42);
        quick.received(OperationId(1), answer);
        quick.move_highlight(1);

        assert_eq!(
            quick.choice(),
            Some(Choice {
                path: "c/b.rs".into(),
                line: Some(42)
            })
        );
    }

    #[test]
    fn a_failed_search_stops_waiting() {
        let mut quick = QuickOpen::default();
        quick.open();
        quick.requested(OperationId(1));
        quick.failed(OperationId(2));
        assert!(quick.owns(OperationId(1)));
        quick.failed(OperationId(1));
        assert!(!quick.owns(OperationId(1)));
    }

    #[test]
    fn the_status_line_says_what_the_list_is() {
        let mut quick = QuickOpen::default();
        assert_eq!(quick.status(), "");
        quick.open();
        quick.requested(OperationId(1));
        assert_eq!(quick.status(), "Reading the file list…");

        quick.received(OperationId(1), search("", &[], 0, 12_345));
        assert_eq!(quick.status(), "Type to search 12,345 files");

        quick.set_query("zz");
        quick.requested(OperationId(2));
        quick.received(OperationId(2), search("zz", &[], 0, 1));
        assert_eq!(quick.status(), "No file matches in 1 file");

        quick.requested(OperationId(3));
        quick.received(OperationId(3), search("z", &["a"], 1, 200));
        assert_eq!(quick.status(), "1 match in 200 files");

        quick.requested(OperationId(4));
        quick.received(OperationId(4), search("z", &["a", "b"], 130, 1_000));
        assert_eq!(quick.status(), "2 of 130 matches in 1,000 files");

        quick.requested(OperationId(5));
        let mut reading = search("z", &["a"], 1, 5);
        reading.indexing = true;
        quick.received(OperationId(5), reading);
        assert_eq!(
            quick.status(),
            "1 match in 5 files (still reading the list)"
        );
    }

    #[test]
    fn segments_alternate_plain_and_highlighted_text() {
        assert_eq!(
            segments("src/diff_pane.rs", &[(4, 5), (9, 13)]),
            [
                ("src/", false),
                ("d", true),
                ("iff_", false),
                ("pane", true),
                (".rs", false)
            ]
        );
        assert_eq!(segments("a.rs", &[]), [("a.rs", false)]);
        assert_eq!(segments("", &[]), []);
    }

    #[test]
    fn segments_ignore_ranges_that_would_cut_a_character_or_run_past_the_end() {
        // "ç" is two bytes: 1..2 cuts it.
        assert_eq!(segments("aço", &[(1, 2)]), [("aço", false)]);
        assert_eq!(segments("abc", &[(2, 9)]), [("abc", false)]);
        assert_eq!(segments("abc", &[(2, 2)]), [("abc", false)]);
        // Overlapping or out-of-order ranges do not repeat text.
        assert_eq!(
            segments("abcdef", &[(2, 4), (1, 3)]),
            [("ab", false), ("cd", true), ("ef", false)]
        );
    }

    #[test]
    fn a_path_splits_into_its_folder_and_its_name() {
        assert_eq!(
            split_name("crates/gm-ui/src/lib.rs"),
            ("crates/gm-ui/src/", "lib.rs")
        );
        assert_eq!(split_name("README.md"), ("", "README.md"));
    }
}
