//! The model of the text search page (docs/09, N3): the text typed, the
//! options, the search in flight, the last answer and which files are folded.
//! Nothing here touches GPUI, and nothing searches: the engine runs `git grep`
//! and answers with the lines. A reply to a search that a newer one superseded
//! is dropped, so an answer never lands under a query it was not made for.

use std::collections::BTreeSet;

use gm_schema::{GREP_TOTAL_LIMIT, GrepOptions, GrepResult, GrepScope, OperationId};

/// How long typing must pause before a search starts, in milliseconds.
pub const DEBOUNCE_MS: u64 = 250;

/// One row of the result list. All rows are one height, so the list can be
/// virtualized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// The header of file `file` (an index into the answer's files).
    File { file: usize },
    /// Line `line` of file `file`.
    Line { file: usize, line: usize },
}

/// Where the search is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Idle,
    Searching(OperationId),
    Done,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextSearch {
    query: String,
    pub options: GrepOptions,
    paths: String,
    stage: Stage,
    /// The newest answer. It stays on screen while the next search runs, so the
    /// list does not blink while typing.
    result: Option<GrepResult>,
    /// Files folded in the list, by path.
    folded: BTreeSet<String>,
    /// Counts the edits, so a delayed search can tell it was overtaken.
    generation: u64,
}

impl TextSearch {
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    #[must_use]
    pub fn paths(&self) -> &str {
        &self.paths
    }

    /// Whether there is nothing to search for.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.query.is_empty()
    }

    /// Takes the text of the query box. Returns whether it changed.
    pub fn set_query(&mut self, text: &str) -> bool {
        if self.query == text {
            return false;
        }
        text.clone_into(&mut self.query);
        true
    }

    /// Takes the text of the files box. Returns whether it changed.
    pub fn set_paths(&mut self, text: &str) -> bool {
        if self.paths == text {
            return false;
        }
        text.clone_into(&mut self.paths);
        true
    }

    /// Flips an option. The caller searches again.
    pub fn toggle(&mut self, option: Toggle) {
        match option {
            Toggle::Case => self.options.case_sensitive = !self.options.case_sensitive,
            Toggle::Word => self.options.whole_word = !self.options.whole_word,
            Toggle::Regex => self.options.regex = !self.options.regex,
            Toggle::Untracked => {
                self.options.scope = match self.options.scope {
                    GrepScope::TrackedAndNew => GrepScope::TrackedOnly,
                    GrepScope::TrackedOnly => GrepScope::TrackedAndNew,
                };
            }
        }
    }

    /// Marks an edit and returns its number: a delayed search runs only if no
    /// later edit has been made.
    pub fn edited(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    #[must_use]
    pub fn is_latest(&self, generation: u64) -> bool {
        self.generation == generation
    }

    /// Records the search just sent. Any earlier one is superseded by it.
    pub fn begin(&mut self, id: OperationId) {
        self.stage = Stage::Searching(id);
    }

    /// Forgets the answer and the search: the query was emptied.
    pub fn clear(&mut self) {
        self.stage = Stage::Idle;
        self.result = None;
        self.folded.clear();
    }

    /// Shows the answer when it is the search in flight. Returns whether it was.
    pub fn received(&mut self, id: OperationId, result: GrepResult) -> bool {
        if self.stage != Stage::Searching(id) {
            return false;
        }
        // A new query starts with every file open; a refinement keeps the folds.
        if self
            .result
            .as_ref()
            .is_none_or(|old| old.query != result.query)
        {
            self.folded.clear();
        }
        self.result = Some(result);
        self.stage = Stage::Done;
        true
    }

    /// The search in flight failed, with Git's message. The last answer is
    /// dropped: it was for another query.
    pub fn failed(&mut self, id: OperationId, message: &str) {
        if self.stage == Stage::Searching(id) {
            self.result = None;
            self.stage = Stage::Failed(message.to_owned());
        }
    }

    /// The search in flight ended with no answer and no reason (it was
    /// cancelled by something other than a newer search).
    pub fn cancelled(&mut self, id: OperationId) {
        if self.stage == Stage::Searching(id) {
            self.stage = if self.result.is_some() {
                Stage::Done
            } else {
                Stage::Idle
            };
        }
    }

    #[must_use]
    pub fn result(&self) -> Option<&GrepResult> {
        self.result.as_ref()
    }

    #[must_use]
    pub fn is_searching(&self) -> bool {
        matches!(self.stage, Stage::Searching(_))
    }

    /// Folds or unfolds a file's lines.
    pub fn toggle_file(&mut self, path: &str) {
        if !self.folded.remove(path) {
            self.folded.insert(path.to_owned());
        }
    }

    #[must_use]
    pub fn is_folded(&self, path: &str) -> bool {
        self.folded.contains(path)
    }

    /// The rows of the list: each file's header, then its lines unless it is folded.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        let Some(result) = &self.result else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (file, entry) in result.files.iter().enumerate() {
            rows.push(Row::File { file });
            if !self.is_folded(&entry.path) {
                rows.extend((0..entry.lines.len()).map(|line| Row::Line { file, line }));
            }
        }
        rows
    }

    /// The line under the boxes: what the list is, or why it is empty.
    #[must_use]
    pub fn summary(&self) -> String {
        if let Stage::Failed(message) = &self.stage {
            return message.clone();
        }
        let searching = self.is_searching();
        let Some(result) = &self.result else {
            return if searching {
                "Searching…".to_owned()
            } else if self.is_blank() {
                "Type a word or phrase to search the files of the worktree.".to_owned()
            } else {
                String::new()
            };
        };
        let files = result.files.len();
        let more: usize = result.files.iter().map(|f| f.more).sum();
        let found = if result.listed == 0 {
            "No matches".to_owned()
        } else {
            let total = result.listed + more;
            format!(
                "{} in {}",
                plural(total, "match", "matches"),
                plural(files, "file", "files")
            )
        };
        let tail = if result.truncated {
            format!(" — stopped at {GREP_TOTAL_LIMIT}, narrow the search")
        } else if searching {
            " — searching…".to_owned()
        } else {
            String::new()
        };
        format!("{found}{tail}")
    }
}

/// An option of the search that the page toggles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    Case,
    Word,
    Regex,
    Untracked,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    let mut digits = n.to_string();
    let mut at = digits.len();
    while at > 3 {
        at -= 3;
        digits.insert(at, ',');
    }
    format!("{digits} {}", if n == 1 { one } else { many })
}

/// The label of a file header's count: `3 matches`, or `100+30 matches` when
/// the file's matches were cut at the per-file limit.
#[must_use]
pub fn file_count(listed: usize, more: usize) -> String {
    if more == 0 {
        plural(listed, "match", "matches")
    } else {
        format!("{listed}+{more} matches")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{GrepFile, GrepLine};

    fn line(n: u32) -> GrepLine {
        GrepLine {
            line: n,
            text: "x".into(),
            ranges: Vec::new(),
            cut_start: false,
            cut_end: false,
        }
    }

    fn file(path: &str, lines: &[u32], more: usize) -> GrepFile {
        GrepFile {
            path: path.into(),
            lines: lines.iter().map(|&n| line(n)).collect(),
            more,
        }
    }

    fn result(query: &str, files: Vec<GrepFile>) -> GrepResult {
        let listed = files.iter().map(|f| f.lines.len()).sum();
        GrepResult {
            query: query.into(),
            files,
            listed,
            truncated: false,
        }
    }

    fn searched(files: Vec<GrepFile>) -> TextSearch {
        let mut search = TextSearch::default();
        search.set_query("x");
        search.begin(OperationId(1));
        search.received(OperationId(1), result("x", files));
        search
    }

    #[test]
    fn an_edit_that_changes_the_text_is_reported_and_the_same_text_is_not() {
        let mut search = TextSearch::default();
        assert!(search.set_query("a"));
        assert!(!search.set_query("a"));
        assert!(search.set_paths("src"));
        assert!(!search.set_paths("src"));
        assert_eq!(search.query(), "a");
        assert_eq!(search.paths(), "src");
    }

    #[test]
    fn only_the_latest_edit_may_start_a_delayed_search() {
        let mut search = TextSearch::default();
        let first = search.edited();
        let second = search.edited();
        assert!(!search.is_latest(first));
        assert!(search.is_latest(second));
    }

    #[test]
    fn the_options_toggle_one_at_a_time() {
        let mut search = TextSearch::default();
        search.toggle(Toggle::Case);
        search.toggle(Toggle::Regex);
        search.toggle(Toggle::Untracked);
        assert_eq!(
            search.options,
            GrepOptions {
                case_sensitive: true,
                whole_word: false,
                regex: true,
                scope: GrepScope::TrackedOnly,
            }
        );
        search.toggle(Toggle::Case);
        assert!(!search.options.case_sensitive);
    }

    #[test]
    fn only_the_answer_to_the_search_in_flight_is_shown() {
        let mut search = TextSearch::default();
        search.begin(OperationId(1));
        search.begin(OperationId(2));

        assert!(!search.received(OperationId(1), result("a", vec![file("old.rs", &[1], 0)])));
        assert!(search.result().is_none());
        assert!(search.received(OperationId(2), result("ab", vec![file("new.rs", &[1], 0)])));
        assert_eq!(search.result().unwrap().files[0].path, "new.rs");
        assert!(!search.is_searching());
        assert!(!search.received(
            OperationId(2),
            result("ab", vec![file("other.rs", &[1], 0)])
        ));
    }

    #[test]
    fn the_last_answer_stays_while_the_next_search_runs() {
        let mut search = searched(vec![file("a.rs", &[1], 0)]);
        search.begin(OperationId(2));
        assert!(search.is_searching());
        assert_eq!(search.rows().len(), 2);
        assert_eq!(search.summary(), "1 match in 1 file — searching…");
    }

    #[test]
    fn rows_are_a_header_then_the_lines_of_each_file() {
        let search = searched(vec![file("a.rs", &[1, 5], 0), file("b.rs", &[2], 0)]);

        assert_eq!(
            search.rows(),
            [
                Row::File { file: 0 },
                Row::Line { file: 0, line: 0 },
                Row::Line { file: 0, line: 1 },
                Row::File { file: 1 },
                Row::Line { file: 1, line: 0 },
            ]
        );
    }

    #[test]
    fn a_folded_file_shows_only_its_header_and_unfolds_again() {
        let mut search = searched(vec![file("a.rs", &[1, 5], 0), file("b.rs", &[2], 0)]);

        search.toggle_file("a.rs");
        assert!(search.is_folded("a.rs"));
        assert_eq!(search.rows().len(), 3);

        search.toggle_file("a.rs");
        assert_eq!(search.rows().len(), 5);
    }

    #[test]
    fn a_refinement_keeps_the_folds_and_a_new_query_opens_everything() {
        let mut search = searched(vec![file("a.rs", &[1], 0)]);
        search.toggle_file("a.rs");

        search.begin(OperationId(2));
        search.received(OperationId(2), result("x", vec![file("a.rs", &[1, 2], 0)]));
        assert!(
            search.is_folded("a.rs"),
            "the same query again keeps the folds"
        );

        search.begin(OperationId(3));
        search.received(OperationId(3), result("y", vec![file("a.rs", &[1], 0)]));
        assert!(!search.is_folded("a.rs"));
    }

    #[test]
    fn a_failed_search_shows_gits_message_and_drops_the_old_answer() {
        let mut search = searched(vec![file("a.rs", &[1], 0)]);
        search.begin(OperationId(2));

        search.failed(OperationId(9), "not ours");
        assert!(search.is_searching());
        search.failed(OperationId(2), "fatal: Unmatched ( or \\(");

        assert!(search.result().is_none());
        assert!(search.rows().is_empty());
        assert_eq!(search.summary(), "fatal: Unmatched ( or \\(");
    }

    #[test]
    fn a_cancelled_search_gives_the_last_answer_back() {
        let mut search = searched(vec![file("a.rs", &[1], 0)]);
        search.begin(OperationId(2));
        search.cancelled(OperationId(2));
        assert!(!search.is_searching());
        assert_eq!(search.summary(), "1 match in 1 file");

        let mut empty = TextSearch::default();
        empty.begin(OperationId(1));
        empty.cancelled(OperationId(1));
        assert!(!empty.is_searching());
    }

    #[test]
    fn clearing_forgets_everything_about_the_old_search() {
        let mut search = searched(vec![file("a.rs", &[1], 0)]);
        search.toggle_file("a.rs");
        search.clear();
        assert!(search.result().is_none());
        assert!(!search.is_folded("a.rs"));
        assert!(search.rows().is_empty());
    }

    #[test]
    fn the_summary_says_what_the_list_is() {
        let mut search = TextSearch::default();
        assert_eq!(
            search.summary(),
            "Type a word or phrase to search the files of the worktree."
        );
        search.set_query("x");
        search.begin(OperationId(1));
        assert_eq!(search.summary(), "Searching…");

        search.received(OperationId(1), result("x", Vec::new()));
        assert_eq!(search.summary(), "No matches");

        search.begin(OperationId(2));
        search.received(
            OperationId(2),
            result("x", vec![file("a.rs", &[1, 2], 30), file("b.rs", &[3], 0)]),
        );
        assert_eq!(search.summary(), "33 matches in 2 files");

        search.begin(OperationId(3));
        let mut full = result("x", vec![file("a.rs", &[1], 0)]);
        full.truncated = true;
        search.received(OperationId(3), full);
        assert_eq!(
            search.summary(),
            "1 match in 1 file — stopped at 1000, narrow the search"
        );
    }

    #[test]
    fn a_files_count_shows_the_matches_it_cut() {
        assert_eq!(file_count(1, 0), "1 match");
        assert_eq!(file_count(3, 0), "3 matches");
        assert_eq!(file_count(100, 30), "100+30 matches");
    }
}
