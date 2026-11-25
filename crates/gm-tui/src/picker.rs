//! The file picker (`Space o`, D258): a query typed over the screen, the files of the
//! worktree it matches as the engine ranks them (`SearchFiles`, N1), and the one to open on
//! the File screen. Pure: the overlay, the keys and the drawing live in `ui.rs` and
//! `draw.rs`.

use gm_schema::{CommandKind, FileHit, FileSearch, OperationId};

#[derive(Debug, Default)]
pub struct Picker {
    pub query: String,
    /// The answer to the newest search, once it came.
    pub search: Option<FileSearch>,
    /// The hit under the cursor.
    pub selected: usize,
    /// Why the newest search failed.
    pub error: Option<String>,
    /// The newest search sent: only its answer is taken, an older one is late.
    asked: Option<OperationId>,
}

impl Picker {
    /// The search of the query as it stands. The engine supersedes the one in flight.
    pub fn command(&self) -> CommandKind {
        CommandKind::SearchFiles {
            query: self.query.clone(),
        }
    }

    pub fn type_text(&mut self, text: &str) {
        self.query.push_str(text);
    }

    /// Erases the last character; false when there was none.
    pub fn backspace(&mut self) -> bool {
        self.query.pop().is_some()
    }

    /// The engine took a search as operation `id`.
    pub fn sent(&mut self, id: OperationId) {
        self.asked = Some(id);
        self.error = None;
    }

    /// Takes the answer of `id` when it is the newest search; false for a late one.
    pub fn received(&mut self, id: OperationId, search: &FileSearch) -> bool {
        if self.asked != Some(id) {
            return false;
        }
        // The cursor stays on the same file when it is still listed, else goes to the best.
        let kept = self
            .hit()
            .and_then(|hit| search.hits.iter().position(|h| h.path == hit.path));
        self.selected = kept.unwrap_or(0);
        self.search = Some(search.clone());
        true
    }

    pub fn failed(&mut self, id: OperationId, reason: &str) {
        if self.asked == Some(id) {
            self.error = Some(reason.to_owned());
        }
    }

    pub fn hits(&self) -> &[FileHit] {
        self.search.as_ref().map_or(&[], |search| &search.hits)
    }

    pub fn hit(&self) -> Option<&FileHit> {
        self.hits().get(self.selected)
    }

    /// Moves the cursor by `by` hits, wrapping around the list as a picker does.
    pub fn move_by(&mut self, by: isize) {
        let count = self.hits().len();
        if count == 0 {
            return;
        }
        let count = isize::try_from(count).unwrap_or(isize::MAX);
        let at = isize::try_from(self.selected).unwrap_or(0);
        self.selected = usize::try_from((at + by).rem_euclid(count)).unwrap_or(0);
    }

    /// The file to open and the line the query ended with (`foo.rs:120`).
    pub fn chosen(&self) -> Option<(String, Option<u32>)> {
        let hit = self.hit()?;
        let line = self.search.as_ref().and_then(|search| search.line);
        Some((hit.path.clone(), line))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(query: &str, paths: &[&str], line: Option<u32>) -> FileSearch {
        FileSearch {
            query: query.to_owned(),
            hits: paths
                .iter()
                .map(|path| FileHit {
                    path: (*path).to_owned(),
                    ranges: Vec::new(),
                })
                .collect(),
            matched: paths.len(),
            files: 10,
            indexing: false,
            line,
        }
    }

    #[test]
    fn only_the_newest_answer_is_taken() {
        let mut picker = Picker::default();
        picker.sent(OperationId(1));
        picker.sent(OperationId(2));
        assert!(!picker.received(OperationId(1), &found("a", &["a.rs"], None)));
        assert!(picker.hits().is_empty());
        assert!(picker.received(OperationId(2), &found("ab", &["ab.rs"], None)));
        assert_eq!(picker.chosen(), Some(("ab.rs".to_owned(), None)));
        picker.failed(OperationId(1), "late");
        assert_eq!(picker.error, None);
        picker.failed(OperationId(2), "no index");
        assert_eq!(picker.error.as_deref(), Some("no index"));
    }

    #[test]
    fn the_cursor_wraps_and_follows_its_file_across_answers() {
        let mut picker = Picker::default();
        picker.sent(OperationId(1));
        picker.received(OperationId(1), &found("", &["a", "b", "c"], None));
        picker.move_by(-1);
        assert_eq!(picker.hit().map(|h| h.path.as_str()), Some("c"));
        picker.move_by(1);
        assert_eq!(picker.selected, 0);
        picker.move_by(1);
        picker.sent(OperationId(2));
        picker.received(OperationId(2), &found("", &["x", "b"], Some(12)));
        assert_eq!(picker.chosen(), Some(("b".to_owned(), Some(12))));
        picker.sent(OperationId(3));
        picker.received(OperationId(3), &found("", &["y"], None));
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn typing_builds_the_query_the_search_sends() {
        let mut picker = Picker::default();
        picker.type_text("lib");
        picker.type_text(":4");
        assert!(picker.backspace());
        assert_eq!(
            picker.command(),
            CommandKind::SearchFiles {
                query: "lib:".to_owned()
            }
        );
        let mut empty = Picker::default();
        assert!(!empty.backspace());
        empty.move_by(1);
        assert_eq!(empty.chosen(), None);
    }
}
