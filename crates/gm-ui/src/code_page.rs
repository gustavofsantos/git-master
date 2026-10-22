//! The model of the Code page (design/code-navigation.md, N2): the folder tree and the viewer of
//! one file. Nothing here touches GPUI. The tree is read a folder at a time
//! from the engine, which lists a folder from the file list it holds in memory,
//! and the viewer shows one file read from disk. A reply to a request that a
//! newer one superseded is dropped, as everywhere else.

use std::collections::{BTreeMap, BTreeSet};

use gm_schema::{DirEntry, DirListing, FileBody, FileView, OperationId};
use unicode_width::UnicodeWidthStr;

/// One row of the tree as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// Path from the root of the worktree. For a folder, the path to list.
    pub path: String,
    /// The name shown: a folder whose only content is a folder is joined with
    /// it (`a/b/c`).
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

/// The text of a file as lines, found once so the viewer can draw any of them
/// without splitting the whole file again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Text {
    source: String,
    /// Where each line starts in `text`.
    starts: Vec<usize>,
    /// The widest line, in columns, with a tab at four.
    columns: usize,
}

/// A tab stop, in columns, as in the diff.
const TAB: usize = 4;

impl Text {
    #[must_use]
    pub fn new(text: String) -> Self {
        let mut starts = Vec::new();
        if !text.is_empty() {
            starts.push(0);
        }
        for (at, _) in text.match_indices('\n') {
            // A final newline ends the last line; it does not start another.
            if at + 1 < text.len() {
                starts.push(at + 1);
            }
        }
        let mut shown = Self {
            source: text,
            starts,
            columns: 0,
        };
        shown.columns = (0..shown.line_count())
            .map(|i| columns_of(shown.line(i)))
            .max()
            .unwrap_or(0);
        shown
    }

    #[must_use]
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// Line `index` (0-based) without its line end, or `""` past the end.
    #[must_use]
    pub fn line(&self, index: usize) -> &str {
        let Some(&start) = self.starts.get(index) else {
            return "";
        };
        let end = self
            .starts
            .get(index + 1)
            .copied()
            .unwrap_or(self.source.len());
        self.source[start..end]
            .trim_end_matches('\n')
            .trim_end_matches('\r')
    }

    /// The width of the widest line, in columns.
    #[must_use]
    pub fn columns(&self) -> usize {
        self.columns
    }
}

/// The width of `line` in columns, a tab counted to the next stop.
#[must_use]
pub fn columns_of(line: &str) -> usize {
    if !line.contains('\t') {
        return line.width();
    }
    let mut column = 0;
    for ch in line.chars() {
        if ch == '\t' {
            column += TAB - column % TAB;
        } else {
            column += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    column
}

/// `line` as it is drawn when the view is scrolled `from` columns to the right:
/// tabs become spaces up to the next stop, then the first `from` columns are
/// dropped. A wide character cut in two leaves a space for its second half.
#[must_use]
pub fn visible(line: &str, from: usize) -> String {
    let mut out = String::new();
    let mut column = 0;
    for ch in line.chars() {
        let width = if ch == '\t' {
            TAB - column % TAB
        } else {
            unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)
        };
        let end = column + width;
        if end <= from {
            column = end;
            continue;
        }
        if column < from {
            // Only the right half of this character is in view.
            out.extend(std::iter::repeat_n(' ', end - from));
        } else if ch == '\t' {
            out.extend(std::iter::repeat_n(' ', width));
        } else {
            out.push(ch);
        }
        column = end;
    }
    out
}

/// What the viewer shows of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(Text),
    Binary,
    Missing,
    Symlink(String),
    Directory,
}

/// A file as the viewer holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub path: String,
    pub size: u64,
    pub content: Content,
    /// The text is the start of a larger file.
    pub truncated: bool,
}

impl Shown {
    fn new(view: FileView) -> Self {
        let content = match view.body {
            FileBody::Text { text } => Content::Text(Text::new(text)),
            FileBody::Binary => Content::Binary,
            FileBody::Missing => Content::Missing,
            FileBody::Symlink { target } => Content::Symlink(target),
            FileBody::Directory => Content::Directory,
        };
        Self {
            path: view.path,
            size: view.size,
            content,
            truncated: view.truncated,
        }
    }
}

/// The viewer's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Viewer {
    /// No file chosen yet.
    #[default]
    Empty,
    /// The read in flight. The file shown before stays on screen under the
    /// loading line until the answer comes, so the view does not blank.
    Loading {
        id: OperationId,
        path: String,
    },
    Shown(Shown),
    Failed {
        path: String,
        message: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodePage {
    /// What each folder holds, by path (`""` is the root).
    listings: BTreeMap<String, Vec<DirEntry>>,
    expanded: BTreeSet<String>,
    /// Folder listings in flight.
    requests: BTreeMap<OperationId, String>,
    /// A path to show in the tree once the folders above it are listed.
    pending_reveal: Option<String>,
    /// How many files the list holds, as the last listing said.
    pub files: usize,
    /// The last listing said the list is still being read.
    pub indexing: bool,
    /// The file chosen, by path.
    pub selected: Option<String>,
    pub viewer: Viewer,
    /// The file on screen while a newer one loads.
    shown: Option<Shown>,
    /// The line to bring into view once the file is shown (1-based).
    pub goto: Option<u32>,
}

/// `dir` and `name` as one path.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

impl CodePage {
    /// Forgets everything: another repository was opened.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The rows of the tree, folders opened as the user left them.
    #[must_use]
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        self.collect("", 0, &mut rows);
        rows
    }

    fn collect(&self, dir: &str, depth: usize, rows: &mut Vec<TreeRow>) {
        let Some(entries) = self.listings.get(dir) else {
            return;
        };
        for entry in entries {
            let path = join(dir, &entry.name);
            let expanded = entry.is_dir && self.expanded.contains(&path);
            rows.push(TreeRow {
                path: path.clone(),
                name: entry.name.clone(),
                depth,
                is_dir: entry.is_dir,
                expanded,
            });
            if expanded {
                self.collect(&path, depth + 1, rows);
            }
        }
    }

    /// Whether the root has been listed.
    #[must_use]
    pub fn has_root(&self) -> bool {
        self.listings.contains_key("")
    }

    /// Opens or closes folder `dir`. Returns whether its listing is needed: it
    /// was opened and the engine has not listed it yet.
    pub fn toggle(&mut self, dir: &str) -> bool {
        if self.expanded.remove(dir) {
            return false;
        }
        self.expanded.insert(dir.to_owned());
        !self.listings.contains_key(dir)
    }

    /// The folders to list again when the file list changed: the root and every
    /// folder that is open.
    #[must_use]
    pub fn folders_to_refresh(&self) -> Vec<String> {
        std::iter::once(String::new())
            .chain(self.expanded.iter().cloned())
            .collect()
    }

    /// Records the listing of `dir` just asked for.
    pub fn listing_requested(&mut self, id: OperationId, dir: &str) {
        self.requests.insert(id, dir.to_owned());
    }

    /// Takes a listing when it answers a request of ours. Returns the folders
    /// to list next, for a path that is being revealed.
    pub fn listed(&mut self, id: OperationId, listing: DirListing) -> Vec<String> {
        let Some(asked) = self.requests.remove(&id) else {
            return Vec::new();
        };
        self.files = listing.files;
        self.indexing = listing.indexing;
        // While the list is still being read the engine answers with what it
        // has, possibly nothing; a folder it has not heard of yet is not "empty".
        if !(listing.indexing && listing.entries.is_empty()) {
            self.listings.insert(asked, listing.entries);
        }
        self.reveal_step()
    }

    /// A listing request ended with no answer.
    pub fn listing_failed(&mut self, id: OperationId) {
        self.requests.remove(&id);
    }

    /// Shows `path` in the tree: opens the folders above it. Returns the
    /// folders whose listing is needed to go on; `listed` returns the next ones.
    pub fn reveal(&mut self, path: &str) -> Vec<String> {
        self.pending_reveal = Some(path.to_owned());
        self.reveal_step()
    }

    /// Opens, from the root down, the folders that hold the path being
    /// revealed, as far as the listings go.
    fn reveal_step(&mut self) -> Vec<String> {
        let Some(path) = self.pending_reveal.clone() else {
            return Vec::new();
        };
        let mut dir = String::new();
        loop {
            let Some(entries) = self.listings.get(&dir) else {
                // The folder is not listed: ask, unless a request is in flight.
                return if self.requests.values().any(|asked| *asked == dir) {
                    Vec::new()
                } else {
                    vec![dir]
                };
            };
            let next = entries.iter().find_map(|entry| {
                let joined = join(&dir, &entry.name);
                (entry.is_dir && path.starts_with(&format!("{joined}/"))).then_some(joined)
            });
            let Some(folder) = next else {
                self.pending_reveal = None;
                return Vec::new();
            };
            self.expanded.insert(folder.clone());
            dir = folder;
        }
    }

    /// Starts showing `path`: the read `id` was just sent. The file on screen
    /// stays until the answer comes. `goto` is the line to bring into view.
    pub fn file_requested(&mut self, id: OperationId, path: &str, goto: Option<u32>) {
        self.selected = Some(path.to_owned());
        self.goto = goto;
        self.viewer = Viewer::Loading {
            id,
            path: path.to_owned(),
        };
    }

    /// Shows the file when it answers the read in flight.
    pub fn file_received(&mut self, id: OperationId, view: FileView) -> bool {
        if !matches!(&self.viewer, Viewer::Loading { id: waiting, .. } if *waiting == id) {
            return false;
        }
        let shown = Shown::new(view);
        self.shown = Some(shown.clone());
        self.viewer = Viewer::Shown(shown);
        true
    }

    /// The read in flight failed, with the engine's reason.
    pub fn file_failed(&mut self, id: OperationId, message: &str) {
        if let Viewer::Loading { id: waiting, path } = &self.viewer
            && *waiting == id
        {
            self.viewer = Viewer::Failed {
                path: path.clone(),
                message: message.to_owned(),
            };
        }
    }

    /// The read in flight was cancelled because another one replaced it: the
    /// newer read keeps the loading state. When none did, the file on screen
    /// before comes back.
    pub fn file_cancelled(&mut self, id: OperationId) {
        if matches!(&self.viewer, Viewer::Loading { id: waiting, .. } if *waiting == id) {
            self.viewer = self.shown.clone().map_or(Viewer::Empty, Viewer::Shown);
        }
    }

    /// The file to draw: the one just read, or the previous one while the next
    /// loads.
    #[must_use]
    pub fn on_screen(&self) -> Option<&Shown> {
        match &self.viewer {
            Viewer::Shown(shown) => Some(shown),
            Viewer::Loading { .. } => self.shown.as_ref(),
            _ => None,
        }
    }

    /// The 1-based line to bring into view, once, when a file with that many
    /// lines is on screen.
    pub fn take_goto(&mut self) -> Option<usize> {
        let line = self.goto?;
        let Viewer::Shown(shown) = &self.viewer else {
            return None;
        };
        let lines = match &shown.content {
            Content::Text(text) => Some(text.line_count()),
            _ => None,
        };
        self.goto = None;
        let line = usize::try_from(line).unwrap_or(usize::MAX).max(1);
        Some(line.min(lines?.max(1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> DirEntry {
        DirEntry {
            name: name.into(),
            is_dir,
        }
    }

    fn listing(dir: &str, entries: &[DirEntry]) -> DirListing {
        DirListing {
            dir: dir.into(),
            entries: entries.to_vec(),
            files: 10,
            indexing: false,
        }
    }

    /// A page whose root holds `src/` and `README.md`, and `src` holds `lib.rs`.
    fn page() -> CodePage {
        let mut page = CodePage::default();
        page.listing_requested(OperationId(1), "");
        page.listed(
            OperationId(1),
            listing("", &[entry("src", true), entry("README.md", false)]),
        );
        page.listing_requested(OperationId(2), "src");
        page.listed(OperationId(2), listing("src", &[entry("lib.rs", false)]));
        page
    }

    fn names(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| {
                format!(
                    "{}{}{}",
                    "  ".repeat(r.depth),
                    r.name,
                    if r.is_dir { "/" } else { "" }
                )
            })
            .collect()
    }

    #[test]
    fn closed_folders_show_only_their_own_row() {
        assert_eq!(names(&page().rows()), ["src/", "README.md"]);
    }

    #[test]
    fn an_open_folder_shows_its_entries_one_level_in() {
        let mut page = page();
        assert!(!page.toggle("src"), "its listing is already held");
        assert_eq!(names(&page.rows()), ["src/", "  lib.rs", "README.md"]);
        assert!(page.rows()[0].expanded);

        page.toggle("src");
        assert_eq!(names(&page.rows()), ["src/", "README.md"]);
    }

    #[test]
    fn opening_a_folder_that_was_not_listed_asks_for_its_listing() {
        let mut page = page();
        page.listings.remove("src");
        assert!(page.toggle("src"));
        // Opened, but empty until the listing comes.
        assert_eq!(names(&page.rows()), ["src/", "README.md"]);
    }

    #[test]
    fn a_listing_that_is_not_ours_or_was_answered_twice_is_ignored() {
        let mut page = CodePage::default();
        page.listing_requested(OperationId(1), "");
        page.listed(OperationId(7), listing("", &[entry("a", false)]));
        assert!(!page.has_root());
        page.listed(OperationId(1), listing("", &[entry("a", false)]));
        assert!(page.has_root());
        page.listed(OperationId(1), listing("", &[entry("b", false)]));
        assert_eq!(names(&page.rows()), ["a"]);
    }

    #[test]
    fn an_empty_answer_while_the_list_is_read_is_not_kept_as_an_empty_folder() {
        let mut page = CodePage::default();
        page.listing_requested(OperationId(1), "");
        let mut reading = listing("", &[]);
        reading.indexing = true;
        page.listed(OperationId(1), reading);

        assert!(!page.has_root());
        assert!(page.indexing);
    }

    #[test]
    fn the_folders_to_refresh_are_the_root_and_the_open_ones() {
        let mut page = page();
        assert_eq!(page.folders_to_refresh(), [""]);
        page.toggle("src");
        assert_eq!(page.folders_to_refresh(), ["", "src"]);
    }

    #[test]
    fn revealing_a_file_opens_the_folders_above_it() {
        let mut page = page();

        let needed = page.reveal("src/lib.rs");

        assert!(needed.is_empty());
        assert_eq!(names(&page.rows()), ["src/", "  lib.rs", "README.md"]);
    }

    #[test]
    fn revealing_goes_through_joined_folders_and_asks_for_the_listings_it_lacks() {
        let mut page = CodePage::default();
        page.listing_requested(OperationId(1), "");
        page.listed(
            OperationId(1),
            listing(
                "",
                &[entry("crates/gm-ui/src", true), entry("README.md", false)],
            ),
        );

        let needed = page.reveal("crates/gm-ui/src/lib.rs");
        assert_eq!(needed, ["crates/gm-ui/src"]);

        page.listing_requested(OperationId(2), "crates/gm-ui/src");
        let next = page.listed(
            OperationId(2),
            listing("crates/gm-ui/src", &[entry("lib.rs", false)]),
        );
        assert!(next.is_empty());
        assert_eq!(
            names(&page.rows()),
            ["crates/gm-ui/src/", "  lib.rs", "README.md"]
        );
    }

    #[test]
    fn revealing_does_not_ask_twice_for_a_listing_in_flight() {
        let mut page = CodePage::default();
        page.listing_requested(OperationId(1), "");
        assert!(page.reveal("a/b.rs").is_empty());
        // Once the root is there, the next folder is asked for.
        let next = page.listed(OperationId(1), listing("", &[entry("a", true)]));
        assert_eq!(next, ["a"]);
    }

    #[test]
    fn revealing_a_path_that_is_not_in_the_tree_ends_quietly() {
        let mut page = page();
        assert!(page.reveal("nope/x.rs").is_empty());
        assert_eq!(names(&page.rows()), ["src/", "README.md"]);
        // And does not keep waiting: a later listing does not open anything.
        page.listing_requested(OperationId(9), "src");
        assert!(page.listed(OperationId(9), listing("src", &[])).is_empty());
    }

    fn view(path: &str, body: FileBody) -> FileView {
        FileView {
            path: path.into(),
            size: 7,
            body,
            truncated: false,
        }
    }

    fn text_view(path: &str, text: &str) -> FileView {
        view(path, FileBody::Text { text: text.into() })
    }

    #[test]
    fn a_file_is_shown_when_its_read_answers_and_not_before() {
        let mut page = CodePage::default();
        page.file_requested(OperationId(1), "a.rs", None);
        assert_eq!(page.selected.as_deref(), Some("a.rs"));
        assert!(page.on_screen().is_none());

        assert!(!page.file_received(OperationId(2), text_view("a.rs", "x")));
        assert!(page.file_received(OperationId(1), text_view("a.rs", "x")));
        assert_eq!(page.on_screen().unwrap().path, "a.rs");
        // A second answer to the same read is not taken.
        assert!(!page.file_received(OperationId(1), text_view("a.rs", "y")));
    }

    #[test]
    fn the_previous_file_stays_on_screen_while_the_next_loads() {
        let mut page = CodePage::default();
        page.file_requested(OperationId(1), "a.rs", None);
        page.file_received(OperationId(1), text_view("a.rs", "a\n"));

        page.file_requested(OperationId(2), "b.rs", None);

        assert_eq!(page.on_screen().unwrap().path, "a.rs");
        assert_eq!(page.selected.as_deref(), Some("b.rs"));
        assert!(matches!(page.viewer, Viewer::Loading { .. }));
    }

    #[test]
    fn a_failed_read_shows_the_reason_and_a_cancelled_one_gives_the_file_back() {
        let mut page = CodePage::default();
        page.file_requested(OperationId(1), "a.rs", None);
        page.file_received(OperationId(1), text_view("a.rs", "a\n"));
        page.file_requested(OperationId(2), "b.rs", None);
        page.file_failed(OperationId(2), "cannot read b.rs: denied");
        assert_eq!(
            page.viewer,
            Viewer::Failed {
                path: "b.rs".into(),
                message: "cannot read b.rs: denied".into()
            }
        );

        page.file_requested(OperationId(3), "c.rs", None);
        page.file_cancelled(OperationId(2));
        assert!(
            matches!(
                page.viewer,
                Viewer::Loading {
                    id: OperationId(3),
                    ..
                }
            ),
            "an older cancel changes nothing"
        );
        page.file_cancelled(OperationId(3));
        assert_eq!(page.on_screen().unwrap().path, "a.rs");
    }

    #[test]
    fn the_line_to_go_to_is_taken_once_the_file_is_there_and_kept_inside_it() {
        let mut page = CodePage::default();
        page.file_requested(OperationId(1), "a.rs", Some(500));
        assert_eq!(page.take_goto(), None, "not shown yet");
        page.file_received(OperationId(1), text_view("a.rs", "1\n2\n3\n"));

        assert_eq!(page.take_goto(), Some(3));
        assert_eq!(page.take_goto(), None, "only once");
    }

    #[test]
    fn line_zero_goes_to_the_first_line_and_a_binary_file_has_no_line() {
        let mut page = CodePage::default();
        page.file_requested(OperationId(1), "a.rs", Some(0));
        page.file_received(OperationId(1), text_view("a.rs", "x\n"));
        assert_eq!(page.take_goto(), Some(1));

        page.file_requested(OperationId(2), "b.bin", Some(4));
        page.file_received(OperationId(2), view("b.bin", FileBody::Binary));
        assert_eq!(page.take_goto(), None);
        assert_eq!(page.goto, None);
    }

    #[test]
    fn the_other_kinds_of_file_are_kept_as_they_are() {
        let mut page = CodePage::default();
        for (n, body, expected) in [
            (1, FileBody::Binary, Content::Binary),
            (2, FileBody::Missing, Content::Missing),
            (3, FileBody::Directory, Content::Directory),
            (
                4,
                FileBody::Symlink {
                    target: "../x".into(),
                },
                Content::Symlink("../x".into()),
            ),
        ] {
            page.file_requested(OperationId(n), "f", None);
            page.file_received(OperationId(n), view("f", body));
            assert_eq!(page.on_screen().unwrap().content, expected);
        }
    }

    #[test]
    fn reset_forgets_the_tree_and_the_file() {
        let mut page = page();
        page.file_requested(OperationId(5), "a", None);
        page.reset();
        assert_eq!(page, CodePage::default());
    }

    #[test]
    fn text_splits_into_lines_without_their_ends() {
        let text = Text::new("one\ntwo\r\nthree".into());
        assert_eq!(text.line_count(), 3);
        assert_eq!(
            (text.line(0), text.line(1), text.line(2), text.line(3)),
            ("one", "two", "three", "")
        );
    }

    #[test]
    fn a_final_newline_does_not_start_another_line() {
        assert_eq!(Text::new("a\nb\n".into()).line_count(), 2);
        assert_eq!(Text::new("a\n\n".into()).line_count(), 2);
        assert_eq!(Text::new(String::new()).line_count(), 0);
        assert_eq!(Text::new("\n".into()).line_count(), 1);
        assert_eq!(Text::new("\n".into()).line(0), "");
    }

    #[test]
    fn a_line_scrolled_sideways_drops_its_first_columns() {
        assert_eq!(visible("abcdef", 0), "abcdef");
        assert_eq!(visible("abcdef", 2), "cdef");
        assert_eq!(visible("abcdef", 6), "");
        assert_eq!(visible("abcdef", 99), "");
    }

    #[test]
    fn tabs_are_drawn_as_spaces_to_the_next_stop_before_columns_are_dropped() {
        assert_eq!(visible("\tx", 0), "    x");
        assert_eq!(visible("\tx", 2), "  x");
        assert_eq!(visible("ab\tx", 0), "ab  x");
        assert_eq!(visible("\tx", 4), "x");
    }

    #[test]
    fn a_wide_character_cut_by_the_scroll_leaves_a_space() {
        assert_eq!(visible("日本", 1), " 本");
        assert_eq!(visible("日本", 2), "本");
    }

    #[test]
    fn the_width_counts_tabs_to_a_stop_and_wide_characters_double() {
        assert_eq!(columns_of("abc"), 3);
        assert_eq!(columns_of("\tx"), 5);
        assert_eq!(columns_of("ab\tx"), 5);
        assert_eq!(columns_of("日本"), 4);
        assert_eq!(Text::new("ab\n\tcdef\nx\n".into()).columns(), 8);
    }
}
