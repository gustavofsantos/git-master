//! The window's state, with no GPUI in it: what the engine said so far and
//! which row is selected. The view renders it and forwards keys to it.

use gm_schema::{Change, Event, OperationState, RepoStatus, StatusEntry};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Model {
    pub status: Option<RepoStatus>,
    /// Why the last operation failed. Cleared by the next successful status.
    pub error: Option<String>,
    pub loading: bool,
    pub selected: usize,
}

impl Model {
    /// Folds one engine event into the state.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::StatusLoaded { status, .. } => {
                self.selected = clamp(self.selected, status.entries.len());
                self.status = Some(status);
                self.error = None;
                self.loading = false;
            }
            Event::OperationChanged {
                state: OperationState::Failed,
                detail,
                ..
            } => {
                self.error = Some(detail.unwrap_or_else(|| "the operation failed".to_owned()));
                self.loading = false;
            }
            Event::OperationChanged { .. } => {}
        }
    }

    pub fn begin_refresh(&mut self) {
        self.loading = true;
    }

    /// Moves the selection down one row; stops at the last row.
    pub fn select_next(&mut self) {
        self.selected = clamp(self.selected.saturating_add(1), self.entry_count());
    }

    /// Moves the selection up one row; stops at the first row.
    pub fn select_prev(&mut self) {
        self.selected = clamp(self.selected.saturating_sub(1), self.entry_count());
    }

    pub fn entry_count(&self) -> usize {
        self.status
            .as_ref()
            .map_or(0, |status| status.entries.len())
    }
}

/// Keeps `index` on a row that exists; zero when there are no rows.
fn clamp(index: usize, len: usize) -> usize {
    index.min(len.saturating_sub(1))
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
    use gm_schema::{Actor, BranchInfo, OperationId};

    fn entry(path: &str) -> StatusEntry {
        StatusEntry {
            path: path.to_owned(),
            orig_path: None,
            staged: None,
            unstaged: None,
            untracked: false,
            conflicted: false,
        }
    }

    fn status(count: usize) -> RepoStatus {
        RepoStatus {
            root: "/r".to_owned(),
            branch: BranchInfo::default(),
            entries: (0..count).map(|i| entry(&format!("f{i}"))).collect(),
        }
    }

    fn loaded(count: usize) -> Event {
        Event::StatusLoaded {
            id: OperationId(1),
            status: status(count),
        }
    }

    fn operation(state: OperationState, detail: Option<&str>) -> Event {
        Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state,
            detail: detail.map(str::to_owned),
        }
    }

    #[test]
    fn a_status_replaces_the_old_one_and_ends_loading() {
        let mut model = Model {
            loading: true,
            error: Some("old".into()),
            ..Model::default()
        };
        model.apply(loaded(3));
        assert_eq!(model.entry_count(), 3);
        assert!(!model.loading);
        assert_eq!(model.error, None);
    }

    #[test]
    fn a_shorter_status_pulls_the_selection_back_onto_a_row() {
        let mut model = Model::default();
        model.apply(loaded(5));
        model.selected = 4;
        model.apply(loaded(2));
        assert_eq!(model.selected, 1);
        model.apply(loaded(0));
        assert_eq!(model.selected, 0);
    }

    #[test]
    fn a_failed_operation_sets_the_error_and_ends_loading() {
        let mut model = Model {
            loading: true,
            ..Model::default()
        };
        model.apply(operation(
            OperationState::Failed,
            Some("not a git repository"),
        ));
        assert_eq!(model.error.as_deref(), Some("not a git repository"));
        assert!(!model.loading);
    }

    #[test]
    fn a_failed_operation_without_detail_still_says_so() {
        let mut model = Model::default();
        model.apply(operation(OperationState::Failed, None));
        assert_eq!(model.error.as_deref(), Some("the operation failed"));
    }

    #[test]
    fn other_operation_states_change_nothing() {
        let mut model = Model {
            loading: true,
            ..Model::default()
        };
        model.apply(operation(OperationState::Executing, None));
        assert!(model.loading);
        assert_eq!(model.error, None);
    }

    #[test]
    fn selection_stops_at_both_ends() {
        let mut model = Model::default();
        model.apply(loaded(2));
        model.select_prev();
        assert_eq!(model.selected, 0);
        model.select_next();
        model.select_next();
        model.select_next();
        assert_eq!(model.selected, 1);
    }

    #[test]
    fn selection_without_rows_stays_at_zero() {
        let mut model = Model::default();
        model.select_next();
        assert_eq!(model.selected, 0);
        model.select_prev();
        assert_eq!(model.selected, 0);
    }

    #[test]
    fn begin_refresh_sets_loading() {
        let mut model = Model::default();
        model.begin_refresh();
        assert!(model.loading);
    }

    #[test]
    fn labels_show_staged_then_unstaged() {
        let mut modified = entry("a");
        modified.unstaged = Some(Change::Modified);
        assert_eq!(entry_label(&modified), (' ', 'M'));

        let mut added = entry("b");
        added.staged = Some(Change::Added);
        assert_eq!(entry_label(&added), ('A', ' '));

        let mut both = entry("c");
        both.staged = Some(Change::Modified);
        both.unstaged = Some(Change::Deleted);
        assert_eq!(entry_label(&both), ('M', 'D'));
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
