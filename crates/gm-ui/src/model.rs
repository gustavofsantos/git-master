//! The window's state, with no GPUI in it: the status the engine reported, the
//! selected file, the diffs being loaded for it and the last message. The view
//! renders it and forwards keys to it.

use gm_schema::{DiffSide, Event, FileDiff, OperationId, OperationState, RepoStatus, StatusEntry};

use crate::branches::Branches;
use crate::changes::{Summary, summarize};
use crate::icon::Icon;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Model {
    status: Option<RepoStatus>,
    /// Path of the selected file. Kept across status changes while it exists.
    selected: Option<String>,
    /// The two diffs being loaded for the selected file.
    diffs: Option<DiffSlots>,
    /// Why the last operation failed. Cleared by the next status.
    pub error: Option<String>,
    /// A transient line: a commit made, or a reason a commit did not start.
    pub notice: Option<String>,
    /// The icon drawn before the notice, if it has one.
    pub notice_icon: Option<Icon>,
    notice_generation: u64,
    pub loading: bool,
    /// The refresh the indicator follows, while it runs.
    refresh: Option<OperationId>,
    /// Set once `refresh` has run past the indicator delay.
    refresh_slow: bool,
    /// The last operation that reached a final state. Ids follow dispatch
    /// order, so every id at or below it has ended.
    settled: Option<OperationId>,
    /// The `OpenRepo` the window waits for. Its `StatusLoaded` replaces the
    /// repository and resets the view state that belonged to the old one.
    pending_open: Option<OperationId>,
    /// Events older than this belong to the repository being left and are dropped.
    floor: Option<OperationId>,
    /// Set by the open that just succeeded, until the view takes it.
    switched: Option<Switched>,
    /// The branch picker's listing, filter and checkout in flight.
    pub branches: Branches,
}

/// A repository replaced by a successful open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Switched {
    /// The root git resolved for the opened folder.
    pub root: String,
    /// Whether the previous repository had the same root.
    pub same_root: bool,
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
        if self.floor.is_some_and(|floor| event_id(&event) < floor) {
            return None;
        }
        if let Event::OperationChanged { id, state, .. } = &event
            && state.is_terminal()
        {
            self.end_refresh(*id);
        }
        match event {
            Event::StatusLoaded { id, status } => {
                // A successful checkout ends with its status; the repaint is the same as any other.
                let _ = self.branches.finish_checkout(id);
                let old_row = self.selected_row();
                if self.pending_open == Some(id) {
                    self.pending_open = None;
                    self.floor = Some(id);
                    let same_root = self
                        .status
                        .as_ref()
                        .is_some_and(|old| old.root == status.root);
                    self.switched = Some(Switched {
                        root: status.root.clone(),
                        same_root,
                    });
                    self.reset_for_repo();
                }
                self.status = Some(status);
                self.reconcile_selection(old_row);
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
                // A refused checkout keeps the branch; git's reason, first line only, is the notice.
                if self.branches.finish_checkout(id) {
                    self.error = Some(checkout_message(detail.as_deref()));
                    return None;
                }
                let _ = self.branches.list_failed(id);
                if self.pending_open == Some(id) {
                    self.pending_open = None;
                }
                self.error = Some(detail.unwrap_or_else(|| "the operation failed".to_owned()));
                self.loading = false;
                self.settle(id, None, None)
            }
            Event::DiffLoaded { id, side, diff } => self.settle(id, Some(side), Some(diff)),
            Event::Committed { oid, .. } => {
                let short: String = oid.chars().take(7).collect();
                self.set_notice(format!("Committed {short}"));
                self.notice_icon = Some(Icon::CheckCircle);
                None
            }
            Event::BranchesLoaded { id, branches } => {
                let _ = self.branches.listed(id, branches);
                None
            }
            // An operation that ends without a status (vetoed, cancelled) ends its checkout too.
            Event::OperationChanged { id, state, .. } => {
                if matches!(
                    state,
                    OperationState::Done | OperationState::Vetoed | OperationState::Cancelled
                ) {
                    let _ = self.branches.finish_checkout(id);
                }
                None
            }
            // The composer's hook chip (slice 2) reads these; nothing shows them yet.
            Event::HooksListed { .. }
            | Event::HookStarted { .. }
            | Event::HookFinished { .. }
            | Event::HookFailed { .. }
            | Event::HookCancelled { .. } => None,
        }
    }

    /// Waits for the `OpenRepo` `id`. Until it succeeds or fails, the current
    /// repository stays on screen.
    pub fn request_open(&mut self, id: OperationId) {
        self.pending_open = Some(id);
        self.loading = true;
    }

    /// Whether an open is in flight. Nothing may change the repository meanwhile.
    #[must_use]
    pub fn is_opening(&self) -> bool {
        self.pending_open.is_some()
    }

    /// The repository a successful open replaced, once. It is taken, so the
    /// caller acts on it only once.
    pub fn take_switched(&mut self) -> Option<Switched> {
        self.switched.take()
    }

    /// Clears what belonged to the repository being left. The status stays
    /// until the new one replaces it.
    fn reset_for_repo(&mut self) {
        self.selected = None;
        self.diffs = None;
        self.notice = None;
        self.notice_icon = None;
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

    /// Starts tracking the refresh `id`. False when it has already ended: the
    /// watcher's notice can arrive after the event that ends the refresh.
    pub fn begin_refresh(&mut self, id: OperationId) -> bool {
        if self.settled.is_some_and(|done| id <= done) {
            return false;
        }
        self.refresh = Some(id);
        self.refresh_slow = false;
        true
    }

    /// Marks the refresh `id` as slow, once the indicator delay has passed.
    /// True when it is still running, so the indicator should show.
    pub fn refresh_is_slow(&mut self, id: OperationId) -> bool {
        if self.refresh != Some(id) {
            return false;
        }
        self.refresh_slow = true;
        true
    }

    /// Whether "Updating…" shows: a refresh has run past the indicator delay.
    /// Quick refreshes never show it, so they do not flash.
    pub fn updating(&self) -> bool {
        self.refresh_slow
    }

    fn end_refresh(&mut self, id: OperationId) {
        self.settled = Some(self.settled.map_or(id, |done| done.max(id)));
        if self.refresh == Some(id) {
            self.refresh = None;
            self.refresh_slow = false;
        }
    }

    /// Shows `text` on the notice line. Returns a generation for `expire_notice`.
    pub fn set_notice(&mut self, text: String) -> u64 {
        self.notice = Some(text);
        self.notice_icon = None;
        self.notice_generation += 1;
        self.notice_generation
    }

    /// Clears the notice if it is still the one `generation` named.
    pub fn expire_notice(&mut self, generation: u64) {
        if self.notice_generation == generation {
            self.notice = None;
            self.notice_icon = None;
        }
    }

    pub fn notice_generation(&self) -> u64 {
        self.notice_generation
    }

    /// Selects the file on `row`. Returns whether the selected file changed.
    pub fn select_row(&mut self, row: usize) -> bool {
        match self.entries().get(row).map(|entry| entry.path.clone()) {
            Some(path) => self.choose(path),
            None => false,
        }
    }

    /// Moves the selection to the next (or previous) file. Returns whether the
    /// selected file changed.
    pub fn select_file_step(&mut self, down: bool) -> bool {
        let count = self.entry_count();
        let target = match (self.selected_row(), down) {
            (Some(row), true) => row.checked_add(1).filter(|&next| next < count),
            (Some(row), false) => row.checked_sub(1),
            (None, true) => (count > 0).then_some(0),
            (None, false) => count.checked_sub(1),
        };
        target.is_some_and(|row| self.select_row(row))
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
    pub fn summary(&self) -> Summary {
        summarize(self.entries())
    }

    #[must_use]
    pub fn selected_path(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The row of the selected file in the list.
    #[must_use]
    pub fn selected_row(&self) -> Option<usize> {
        let path = self.selected.as_deref()?;
        self.entries().iter().position(|entry| entry.path == path)
    }

    /// The count of changed paths.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries().len()
    }

    /// Selects `path`. Returns whether the selected file changed.
    fn choose(&mut self, path: String) -> bool {
        let changed = self.selected.as_deref() != Some(path.as_str());
        self.selected = Some(path);
        changed
    }

    /// Keeps the selection by path while the file is listed. When it is gone,
    /// the file now at its old row is selected, or the last one.
    fn reconcile_selection(&mut self, old_row: Option<usize>) {
        if self.selected.is_none() || self.selected_row().is_some() {
            return;
        }
        let count = self.entry_count();
        let next = old_row.filter(|_| count > 0).map(|row| row.min(count - 1));
        self.selected = next.map(|row| self.entries()[row].path.clone());
    }
}

/// The notice for a refused checkout: git's first non-empty line.
fn checkout_message(detail: Option<&str>) -> String {
    detail
        .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
        .unwrap_or("the checkout failed")
        .to_owned()
}

fn event_id(event: &Event) -> OperationId {
    match event {
        Event::StatusLoaded { id, .. }
        | Event::OperationChanged { id, .. }
        | Event::DiffLoaded { id, .. }
        | Event::Committed { id, .. }
        | Event::BranchesLoaded { id, .. }
        | Event::HooksListed { id, .. }
        | Event::HookStarted { id, .. }
        | Event::HookFinished { id, .. }
        | Event::HookFailed { id, .. }
        | Event::HookCancelled { id, .. } => *id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{Actor, BranchInfo, Change};

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
        let row = model
            .entries()
            .iter()
            .position(|entry| entry.path == path)
            .expect("path is listed");
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
        assert_eq!(model.selected_row(), Some(1));
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
    fn an_empty_list_has_no_selection() {
        let mut model = Model::default();
        assert!(!model.select_file_step(true));
        assert!(!model.select_file_step(false));
        assert_eq!(model.selected_path(), None);
    }

    #[test]
    fn steps_move_through_the_files_and_stop_at_both_ends() {
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

    fn status_at(root: &str, paths: &[&str]) -> RepoStatus {
        RepoStatus {
            root: root.to_owned(),
            branch: BranchInfo::default(),
            entries: paths.iter().map(|p| entry(p)).collect(),
        }
    }

    /// A model showing repository `/r` with `a` selected and a notice up, with
    /// `a`'s diffs pending as ids 10 and 11.
    fn busy() -> Model {
        let mut model = Model::default();
        let _ = model.apply(loaded(&["a", "dir/b"]));
        let _ = model.set_notice("nothing staged".into());
        let _ = model.select_row(0);
        model.expect_diffs("a".to_owned(), OperationId(10), OperationId(11));
        model
    }

    #[test]
    fn a_successful_open_resets_what_belonged_to_the_old_repo() {
        let mut model = busy();
        model.request_open(OperationId(20));
        assert!(model.is_opening());
        let _ = model.apply(Event::StatusLoaded {
            id: OperationId(20),
            status: status_at("/b", &["x"]),
        });
        assert!(!model.is_opening());
        assert_eq!(model.selected_path(), None);
        assert!(!model.diffs_are_for("a"));
        assert_eq!(model.notice, None);
        assert_eq!(model.entries().len(), 1);
        assert_eq!(
            model.take_switched(),
            Some(Switched {
                root: "/b".into(),
                same_root: false,
            })
        );
        assert_eq!(model.take_switched(), None);
    }

    #[test]
    fn reopening_the_same_root_says_so() {
        let mut model = busy();
        model.request_open(OperationId(20));
        let _ = model.apply(Event::StatusLoaded {
            id: OperationId(20),
            status: status_at("/r", &["a"]),
        });
        assert_eq!(model.take_switched().map(|s| s.same_root), Some(true));
    }

    #[test]
    fn a_failed_open_keeps_the_current_repo_and_its_state() {
        let mut model = busy();
        model.request_open(OperationId(20));
        let _ = model.apply(failed(20, Some("not a git repository")));
        assert!(!model.is_opening());
        assert_eq!(model.error.as_deref(), Some("not a git repository"));
        assert_eq!(model.status().map(|s| s.root.as_str()), Some("/r"));
        assert_eq!(model.selected_path(), Some("a"));
        assert!(model.diffs_are_for("a"));
        assert_eq!(model.take_switched(), None);
    }

    #[test]
    fn a_diff_requested_for_the_old_repo_is_dropped_after_the_switch() {
        let mut model = busy();
        model.request_open(OperationId(20));
        let _ = model.apply(Event::StatusLoaded {
            id: OperationId(20),
            status: status_at("/b", &["a"]),
        });
        // Both replies to the old repo's diff requests arrive late.
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Unstaged, "a")), None);
        assert_eq!(model.apply(diff_loaded(11, DiffSide::Staged, "a")), None);
        assert!(!model.diffs_are_for("a"));
    }

    #[test]
    fn an_event_older_than_the_switch_changes_nothing() {
        let mut model = busy();
        model.request_open(OperationId(20));
        let _ = model.apply(Event::StatusLoaded {
            id: OperationId(20),
            status: status_at("/b", &["x"]),
        });
        let _ = model.apply(failed(5, Some("stale failure")));
        assert_eq!(model.error, None);
    }

    fn branch(name: &str, current: bool) -> gm_schema::BranchEntry {
        gm_schema::BranchEntry {
            name: name.to_owned(),
            is_remote: false,
            is_current: current,
            upstream: None,
        }
    }

    #[test]
    fn a_refused_checkout_shows_git_first_line_and_keeps_the_branch() {
        let mut model = Model::default();
        let _ = model.apply(loaded(&["a"]));
        model.branches.begin_checkout(OperationId(7));
        let _ = model.apply(failed(
            7,
            Some("error: local changes would be overwritten\n\tapi/routes.rs\nAborting"),
        ));
        assert_eq!(
            model.error.as_deref(),
            Some("error: local changes would be overwritten")
        );
        assert!(!model.branches.checkout_in_flight());
        assert_eq!(model.status().map(|s| s.root.as_str()), Some("/r"));
        assert!(!model.is_opening());
    }

    #[test]
    fn a_refused_checkout_without_detail_says_so() {
        let mut model = Model::default();
        model.branches.begin_checkout(OperationId(7));
        let _ = model.apply(failed(7, Some("  \n")));
        assert_eq!(model.error.as_deref(), Some("the checkout failed"));
    }

    #[test]
    fn a_successful_checkout_ends_with_its_status() {
        let mut model = Model::default();
        model.branches.begin_checkout(OperationId(7));
        let _ = model.apply(Event::StatusLoaded {
            id: OperationId(7),
            status: status_at("/r", &["a"]),
        });
        assert!(!model.branches.checkout_in_flight());
        assert_eq!(model.error, None);
    }

    #[test]
    fn a_vetoed_checkout_ends_too() {
        let mut model = Model::default();
        model.branches.begin_checkout(OperationId(7));
        let _ = model.apply(Event::OperationChanged {
            id: OperationId(7),
            actor: Actor::Human,
            state: OperationState::Vetoed,
            detail: None,
        });
        assert!(!model.branches.checkout_in_flight());
    }

    #[test]
    fn a_listing_reply_to_an_older_request_is_dropped() {
        let mut model = Model::default();
        model.branches.request(OperationId(2));
        let _ = model.apply(Event::BranchesLoaded {
            id: OperationId(1),
            branches: vec![branch("old", true)],
        });
        assert!(model.branches.lines().is_empty());
        let _ = model.apply(Event::BranchesLoaded {
            id: OperationId(2),
            branches: vec![branch("new", true)],
        });
        assert_eq!(
            model.branches.highlighted().map(|b| b.name.as_str()),
            Some("new")
        );
    }

    #[test]
    fn a_failed_listing_is_recorded_as_a_failed_listing() {
        let mut model = Model::default();
        model.branches.request(OperationId(2));
        let _ = model.apply(failed(2, Some("not a repository")));
        assert!(model.branches.is_failed());
        assert_eq!(model.error.as_deref(), Some("not a repository"));
    }

    fn done(id: u64) -> Event {
        Event::OperationChanged {
            id: OperationId(id),
            actor: Actor::Human,
            state: OperationState::Done,
            detail: None,
        }
    }

    #[test]
    fn a_refresh_does_not_set_loading() {
        let mut model = Model::default();
        assert!(model.begin_refresh(OperationId(4)));
        assert!(!model.loading);
    }

    #[test]
    fn updating_shows_only_once_the_refresh_is_slow() {
        let mut model = Model::default();
        assert!(model.begin_refresh(OperationId(9)));
        assert!(!model.updating(), "a fresh refresh does not show it");
        assert!(model.refresh_is_slow(OperationId(9)));
        assert!(model.updating());
        let _ = model.apply(done(9));
        assert!(!model.updating(), "it ends with the refresh");
    }

    #[test]
    fn a_quick_refresh_never_shows_updating() {
        let mut model = Model::default();
        assert!(model.begin_refresh(OperationId(9)));
        let _ = model.apply(done(9));
        assert!(
            !model.refresh_is_slow(OperationId(9)),
            "the timer finds it over"
        );
        assert!(!model.updating());
    }

    #[test]
    fn a_notice_that_arrives_after_its_refresh_ended_is_ignored() {
        let mut model = Model::default();
        let _ = model.apply(done(9));
        assert!(!model.begin_refresh(OperationId(9)));
        assert!(!model.begin_refresh(OperationId(8)));
        assert!(model.begin_refresh(OperationId(10)));
    }
}
