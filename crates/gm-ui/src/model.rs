//! The window's state, with no GPUI in it: the status the engine reported, the
//! change tree built from it, the selected file, the diffs being loaded for it
//! and the last message. The view renders it and forwards keys to it.

use std::collections::HashSet;

use gm_schema::{
    Change, DiffSide, Event, FileDiff, OperationId, OperationState, RepoStatus, StatusEntry,
};

use crate::changes::{ChangeTree, Row, Summary, ToggleAction};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Model {
    status: Option<RepoStatus>,
    tree: ChangeTree,
    filter: String,
    /// Full paths of folders whose rows are hidden.
    collapsed: HashSet<String>,
    /// Path of the selected file. Kept across status changes while it exists.
    selected: Option<String>,
    /// Row of the selected file in `tree`; `Some` exactly when `selected` is.
    selected_row: Option<usize>,
    /// The two diffs being loaded for the selected file.
    diffs: Option<DiffSlots>,
    /// Why the last operation failed. Cleared by the next status.
    pub error: Option<String>,
    /// A transient line: a commit made, or a reason a commit did not start.
    pub notice: Option<String>,
    notice_generation: u64,
    pub loading: bool,
}

/// The two sides of the selected file, ready to show. `None` means the side
/// has no diff, or its load failed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diffs {
    pub unstaged: Option<FileDiff>,
    pub staged: Option<FileDiff>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Slot {
    /// Waiting for the `LoadDiff` operation with this id.
    Waiting(OperationId),
    Settled(Option<FileDiff>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiffSlots {
    path: String,
    unstaged: Slot,
    staged: Slot,
}

impl DiffSlots {
    /// The side whose load is `id`, if one is still waiting for it.
    fn waiting_side(&self, id: OperationId) -> Option<DiffSide> {
        if self.unstaged == Slot::Waiting(id) {
            Some(DiffSide::Unstaged)
        } else if self.staged == Slot::Waiting(id) {
            Some(DiffSide::Staged)
        } else {
            None
        }
    }

    fn ready(&self) -> Option<Diffs> {
        match (&self.unstaged, &self.staged) {
            (Slot::Settled(unstaged), Slot::Settled(staged)) => Some(Diffs {
                unstaged: unstaged.clone(),
                staged: staged.clone(),
            }),
            _ => None,
        }
    }
}

impl Model {
    /// Folds one engine event into the state. Returns both diffs once the
    /// selected file's two loads have both finished (succeeded or failed).
    #[must_use]
    pub fn apply(&mut self, event: Event) -> Option<Diffs> {
        match event {
            Event::StatusLoaded { status, .. } => {
                self.status = Some(status);
                self.rebuild();
                self.error = None;
                self.loading = false;
                None
            }
            Event::OperationChanged {
                id,
                state: OperationState::Failed,
                detail,
                ..
            } => {
                self.error = Some(detail.unwrap_or_else(|| "the operation failed".to_owned()));
                self.loading = false;
                self.settle(id, None, None)
            }
            Event::DiffLoaded { id, side, diff } => self.settle(id, Some(side), Some(diff)),
            Event::Committed { oid, .. } => {
                let short: String = oid.chars().take(7).collect();
                self.set_notice(format!("Committed {short}"));
                None
            }
            Event::OperationChanged { .. } => None,
        }
    }

    /// Resolves the diff side waiting for `id`. A `DiffLoaded` must name the
    /// side it waits on; a failure names none.
    fn settle(
        &mut self,
        id: OperationId,
        side: Option<DiffSide>,
        diff: Option<FileDiff>,
    ) -> Option<Diffs> {
        let slots = self.diffs.as_mut()?;
        let waiting = slots.waiting_side(id)?;
        if side.is_some_and(|given| given != waiting) {
            return None;
        }
        let slot = match waiting {
            DiffSide::Unstaged => &mut slots.unstaged,
            DiffSide::Staged => &mut slots.staged,
        };
        *slot = Slot::Settled(diff);
        slots.ready()
    }

    /// Records that the diffs of `path` are being loaded by these operations.
    /// Replaces any earlier request; its replies are dropped from now on.
    pub fn expect_diffs(&mut self, path: String, unstaged: OperationId, staged: OperationId) {
        self.diffs = Some(DiffSlots {
            path,
            unstaged: Slot::Waiting(unstaged),
            staged: Slot::Waiting(staged),
        });
    }

    pub fn forget_diffs(&mut self) {
        self.diffs = None;
    }

    /// Whether the diffs shown or being loaded are those of `path`.
    #[must_use]
    pub fn diffs_are_for(&self, path: &str) -> bool {
        self.diffs.as_ref().is_some_and(|slots| slots.path == path)
    }

    pub fn begin_refresh(&mut self) {
        self.loading = true;
    }

    /// Shows `text` on the notice line. Returns a generation for `expire_notice`.
    pub fn set_notice(&mut self, text: String) -> u64 {
        self.notice = Some(text);
        self.notice_generation += 1;
        self.notice_generation
    }

    /// Clears the notice if it is still the one `generation` named.
    pub fn expire_notice(&mut self, generation: u64) {
        if self.notice_generation == generation {
            self.notice = None;
        }
    }

    pub fn notice_generation(&self) -> u64 {
        self.notice_generation
    }

    /// Sets the filter to the first line of `text`. Returns whether the
    /// selected file changed, so the caller reloads its diffs.
    pub fn set_filter(&mut self, text: &str) -> bool {
        let needle = text.lines().next().unwrap_or("");
        if needle == self.filter {
            return false;
        }
        needle.clone_into(&mut self.filter);
        self.change_selection(Self::rebuild)
    }

    /// Shows or hides a folder's rows. Returns whether the selected file changed.
    pub fn toggle_collapsed(&mut self, folder: &str) -> bool {
        self.change_selection(|model| {
            if !model.collapsed.remove(folder) {
                model.collapsed.insert(folder.to_owned());
            }
            model.rebuild();
        })
    }

    /// Selects the file on `row`. Folders are not selectable. Returns whether
    /// the selected file changed.
    pub fn select_row(&mut self, row: usize) -> bool {
        match self.path_at(row).map(str::to_owned) {
            Some(path) => self.choose(row, path),
            None => false,
        }
    }

    /// Moves the selection to the next (or previous) file row. Returns whether
    /// the selected file changed.
    pub fn select_file_step(&mut self, down: bool) -> bool {
        let target = match (self.selected_row, down) {
            (Some(row), true) => self.tree.next_file(row),
            (Some(row), false) => self.tree.prev_file(row),
            (None, true) => self.file_forward(0),
            (None, false) => self.file_backward(self.tree.row_count()),
        };
        match target {
            Some(row) => self.select_row(row),
            None => false,
        }
    }

    /// What toggling `row` does to the index.
    #[must_use]
    pub fn toggle_action(&self, row: usize) -> ToggleAction {
        self.tree.toggle_action(row)
    }

    #[must_use]
    pub fn tree(&self) -> &ChangeTree {
        &self.tree
    }

    #[must_use]
    pub fn status(&self) -> Option<&RepoStatus> {
        self.status.as_ref()
    }

    /// The status entries, or none before the first status arrives.
    #[must_use]
    pub fn entries(&self) -> &[StatusEntry] {
        self.status
            .as_ref()
            .map_or(&[], |status| status.entries.as_slice())
    }

    #[must_use]
    pub fn filter(&self) -> &str {
        &self.filter
    }

    #[must_use]
    pub fn summary(&self) -> Summary {
        self.tree.summary()
    }

    #[must_use]
    pub fn selected_path(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    #[must_use]
    pub fn selected_row(&self) -> Option<usize> {
        self.selected_row
    }

    /// The count of changed paths, whatever the filter.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries().len()
    }

    /// Runs `change`, then reports whether the selected file changed.
    fn change_selection(&mut self, change: impl FnOnce(&mut Self)) -> bool {
        let before = self.selected.clone();
        change(self);
        self.selected != before
    }

    fn choose(&mut self, row: usize, path: String) -> bool {
        let changed = self.selected.as_deref() != Some(path.as_str());
        self.selected = Some(path);
        self.selected_row = Some(row);
        changed
    }

    /// Rebuilds the tree from the status and filter, then keeps the selection:
    /// by path while the file is listed, else the nearest file to its old row.
    fn rebuild(&mut self) {
        let old_row = self.selected_row;
        self.tree = ChangeTree::build(self.entries(), &self.filter, &self.collapsed);
        let Some(path) = self.selected.clone() else {
            self.selected_row = None;
            return;
        };
        if let Some(row) = self.row_of(&path) {
            self.selected_row = Some(row);
            return;
        }
        let nearest =
            old_row.and_then(|old| self.file_forward(old).or_else(|| self.file_backward(old)));
        let moved = nearest.and_then(|row| self.path_at(row).map(str::to_owned).map(|p| (row, p)));
        if let Some((row, path)) = moved {
            self.selected = Some(path);
            self.selected_row = Some(row);
        } else {
            self.selected = None;
            self.selected_row = None;
        }
    }

    fn row_of(&self, path: &str) -> Option<usize> {
        (0..self.tree.row_count()).find(|&row| self.path_at(row) == Some(path))
    }

    fn path_at(&self, row: usize) -> Option<&str> {
        match self.tree.rows().get(row)? {
            Row::File { path, .. } => Some(path.as_str()),
            Row::Dir { .. } => None,
        }
    }

    /// The first file row at or after `from`.
    fn file_forward(&self, from: usize) -> Option<usize> {
        (from..self.tree.row_count()).find(|&row| self.tree.file_at(row).is_some())
    }

    /// The last file row strictly before `before`.
    fn file_backward(&self, before: usize) -> Option<usize> {
        (0..before.min(self.tree.row_count()))
            .rev()
            .find(|&row| self.tree.file_at(row).is_some())
    }
}

/// The two status letters shown before a path: staged, then unstaged.
/// `?` marks an untracked path, `U` a conflicted one, a space an unchanged side.
#[must_use]
pub fn entry_label(entry: &StatusEntry) -> (char, char) {
    if entry.untracked {
        ('?', '?')
    } else if entry.conflicted {
        ('U', 'U')
    } else {
        (change_letter(entry.staged), change_letter(entry.unstaged))
    }
}

fn change_letter(change: Option<Change>) -> char {
    match change {
        Some(Change::Added) => 'A',
        Some(Change::Modified) => 'M',
        Some(Change::Deleted) => 'D',
        Some(Change::Renamed) => 'R',
        Some(Change::Copied) => 'C',
        Some(Change::TypeChanged) => 'T',
        None => ' ',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{Actor, BranchInfo};

    fn entry(path: &str) -> StatusEntry {
        StatusEntry {
            path: path.to_owned(),
            orig_path: None,
            staged: None,
            unstaged: Some(Change::Modified),
            untracked: false,
            conflicted: false,
        }
    }

    fn status(paths: &[&str]) -> RepoStatus {
        RepoStatus {
            root: "/r".to_owned(),
            branch: BranchInfo::default(),
            entries: paths.iter().map(|p| entry(p)).collect(),
        }
    }

    fn loaded(paths: &[&str]) -> Event {
        Event::StatusLoaded {
            id: OperationId(1),
            status: status(paths),
        }
    }

    fn file_diff(path: &str) -> FileDiff {
        FileDiff {
            path: path.to_owned(),
            old_path: None,
            binary: false,
            hunks: Vec::new(),
        }
    }

    fn diff_loaded(id: u64, side: DiffSide, path: &str) -> Event {
        Event::DiffLoaded {
            id: OperationId(id),
            side,
            diff: file_diff(path),
        }
    }

    fn failed(id: u64, detail: Option<&str>) -> Event {
        Event::OperationChanged {
            id: OperationId(id),
            actor: Actor::Human,
            state: OperationState::Failed,
            detail: detail.map(str::to_owned),
        }
    }

    /// A model with `paths` loaded and `path` selected, plus two pending diffs.
    fn selected(paths: &[&str], path: &str) -> Model {
        let mut model = Model::default();
        let _ = model.apply(loaded(paths));
        let row = model.row_of(path).expect("path is listed");
        let _ = model.select_row(row);
        model.expect_diffs(path.to_owned(), OperationId(10), OperationId(11));
        model
    }

    #[test]
    fn a_status_replaces_the_old_one_and_ends_loading() {
        let mut model = Model {
            loading: true,
            error: Some("old".into()),
            ..Model::default()
        };
        let _ = model.apply(loaded(&["a", "b", "c"]));
        assert_eq!(model.entry_count(), 3);
        assert!(!model.loading);
        assert_eq!(model.error, None);
    }

    #[test]
    fn a_failed_operation_sets_the_error_and_ends_loading() {
        let mut model = Model {
            loading: true,
            ..Model::default()
        };
        let _ = model.apply(failed(2, Some("not supported")));
        assert_eq!(model.error.as_deref(), Some("not supported"));
        assert!(!model.loading);
    }

    #[test]
    fn a_failed_operation_without_detail_still_says_so() {
        let mut model = Model::default();
        let _ = model.apply(failed(2, None));
        assert_eq!(model.error.as_deref(), Some("the operation failed"));
    }

    #[test]
    fn selection_stays_on_the_same_path_when_rows_move() {
        let mut model = selected(&["a/x", "b", "c"], "b");
        let _ = model.apply(loaded(&["z", "b", "c", "a/x"]));
        assert_eq!(model.selected_path(), Some("b"));
        assert_eq!(model.selected_row(), model.row_of("b"));
    }

    #[test]
    fn a_vanished_selection_moves_to_the_nearest_file() {
        let mut model = selected(&["a", "b", "c"], "b");
        let _ = model.apply(loaded(&["a", "c"]));
        assert_eq!(model.selected_path(), Some("c"));
        let _ = model.apply(loaded(&["a"]));
        assert_eq!(model.selected_path(), Some("a"));
        let _ = model.apply(loaded(&[]));
        assert_eq!(model.selected_path(), None);
        assert_eq!(model.selected_row(), None);
    }

    #[test]
    fn a_filter_that_hides_the_selection_selects_the_nearest_file() {
        let mut model = selected(&["a", "bb", "c"], "bb");
        assert!(model.set_filter("c"));
        assert_eq!(model.selected_path(), Some("c"));
        assert!(!model.set_filter("")); // the selection is kept, so no reload
        assert_eq!(model.selected_path(), Some("c"));
    }

    #[test]
    fn a_filter_keeps_the_selection_when_the_file_still_matches() {
        let mut model = selected(&["a", "ab", "c"], "ab");
        assert!(!model.set_filter("a"));
        assert_eq!(model.selected_path(), Some("ab"));
    }

    #[test]
    fn only_the_first_line_of_the_filter_counts() {
        let mut model = selected(&["a", "b"], "a");
        let _ = model.set_filter("b\nignored");
        assert_eq!(model.filter(), "b");
    }

    #[test]
    fn collapsing_the_folder_of_the_selection_moves_it() {
        let mut model = selected(&["dir/x", "other"], "dir/x");
        assert!(model.toggle_collapsed("dir"));
        assert_eq!(model.selected_path(), Some("other"));
    }

    #[test]
    fn an_empty_tree_has_no_selection() {
        let mut model = Model::default();
        assert!(!model.select_file_step(true));
        assert!(!model.select_file_step(false));
        assert_eq!(model.selected_path(), None);
    }

    #[test]
    fn steps_skip_folders_and_stop_at_both_ends() {
        let mut model = Model::default();
        let _ = model.apply(loaded(&["dir/a", "dir/b"]));
        assert!(model.select_file_step(true));
        assert_eq!(model.selected_path(), Some("dir/a"));
        assert!(model.select_file_step(true));
        assert_eq!(model.selected_path(), Some("dir/b"));
        assert!(!model.select_file_step(true));
        assert!(model.select_file_step(false));
        assert_eq!(model.selected_path(), Some("dir/a"));
        assert!(!model.select_file_step(false));
    }

    #[test]
    fn clicking_a_folder_does_not_select_anything() {
        let mut model = Model::default();
        let _ = model.apply(loaded(&["dir/a"]));
        assert!(!model.select_row(0));
        assert_eq!(model.selected_path(), None);
    }

    #[test]
    fn both_diffs_are_returned_once_both_arrive() {
        let mut model = selected(&["a"], "a");
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Unstaged, "a")), None);
        let ready = model.apply(diff_loaded(11, DiffSide::Staged, "a"));
        assert_eq!(
            ready,
            Some(Diffs {
                unstaged: Some(file_diff("a")),
                staged: Some(file_diff("a")),
            })
        );
        // A repeated reply for a settled side is ignored.
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Unstaged, "a")), None);
    }

    #[test]
    fn a_stale_diff_is_dropped() {
        let mut model = selected(&["a", "b"], "a");
        model.expect_diffs("b".into(), OperationId(20), OperationId(21));
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Unstaged, "a")), None);
        assert_eq!(model.apply(diff_loaded(11, DiffSide::Staged, "a")), None);
        assert_eq!(model.apply(diff_loaded(20, DiffSide::Unstaged, "b")), None);
        assert!(
            model
                .apply(diff_loaded(21, DiffSide::Staged, "b"))
                .is_some()
        );
    }

    #[test]
    fn a_diff_on_the_wrong_side_is_dropped() {
        let mut model = selected(&["a"], "a");
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Staged, "a")), None);
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Unstaged, "a")), None);
    }

    #[test]
    fn one_side_failing_while_the_other_loads_still_shows_both() {
        let mut model = selected(&["a"], "a");
        assert_eq!(model.apply(failed(10, Some("not supported"))), None);
        assert_eq!(model.error.as_deref(), Some("not supported"));
        let ready = model.apply(diff_loaded(11, DiffSide::Staged, "a"));
        assert_eq!(
            ready,
            Some(Diffs {
                unstaged: None,
                staged: Some(file_diff("a")),
            })
        );
    }

    #[test]
    fn both_sides_failing_is_ready_with_nothing() {
        let mut model = selected(&["a"], "a");
        assert_eq!(model.apply(failed(11, None)), None);
        assert_eq!(model.apply(failed(10, None)), Some(Diffs::default()));
    }

    #[test]
    fn a_failure_of_an_unrelated_operation_settles_nothing() {
        let mut model = selected(&["a"], "a");
        assert_eq!(model.apply(failed(99, Some("stage failed"))), None);
        assert_eq!(model.error.as_deref(), Some("stage failed"));
        assert!(model.diffs_are_for("a"));
    }

    #[test]
    fn a_new_request_drops_the_old_ids() {
        let mut model = selected(&["a", "b"], "a");
        model.forget_diffs();
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Unstaged, "a")), None);
        assert!(!model.diffs_are_for("b"));
    }

    #[test]
    fn a_commit_notice_shows_the_short_id_and_expires_by_generation() {
        let mut model = Model::default();
        let _ = model.apply(Event::Committed {
            id: OperationId(3),
            oid: "0123456789abcdef".into(),
        });
        assert_eq!(model.notice.as_deref(), Some("Committed 0123456"));
        let generation = model.notice_generation();
        let _ = model.set_notice("nothing staged".into());
        model.expire_notice(generation);
        assert_eq!(model.notice.as_deref(), Some("nothing staged"));
    }

    #[test]
    fn labels_show_staged_then_unstaged() {
        let mut modified = entry("a");
        modified.unstaged = Some(Change::Modified);
        assert_eq!(entry_label(&modified), (' ', 'M'));

        let mut added = entry("b");
        added.unstaged = None;
        added.staged = Some(Change::Added);
        assert_eq!(entry_label(&added), ('A', ' '));
    }

    #[test]
    fn every_change_kind_has_its_letter() {
        let letters: Vec<char> = [
            Change::Added,
            Change::Modified,
            Change::Deleted,
            Change::Renamed,
            Change::Copied,
            Change::TypeChanged,
        ]
        .into_iter()
        .map(|change| change_letter(Some(change)))
        .collect();
        assert_eq!(letters, ['A', 'M', 'D', 'R', 'C', 'T']);
    }

    #[test]
    fn untracked_and_conflicted_entries_have_their_own_labels() {
        let mut untracked = entry("new");
        untracked.untracked = true;
        assert_eq!(entry_label(&untracked), ('?', '?'));

        let mut conflicted = entry("both");
        conflicted.conflicted = true;
        assert_eq!(entry_label(&conflicted), ('U', 'U'));
    }
}
