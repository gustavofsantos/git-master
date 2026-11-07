//! The rows of the sidebar's WORKSPACES section, and the listing they come from (D183 (2)).
//! Pure: the view draws what this lists. docs/guide/workspaces.md, "The Workspaces section".

use std::path::Path;

use gm_schema::{OperationId, WorkspaceRow};

use crate::worktree_list::same_path;

/// One row of the section.
#[derive(Debug, PartialEq, Eq)]
pub struct Row<'a> {
    pub workspace: &'a WorkspaceRow,
    /// The workspace this window shows.
    pub here: bool,
}

/// The rows for `workspaces`, by name, for a window showing the folder `here`.
#[must_use]
pub fn rows<'a>(workspaces: &'a [WorkspaceRow], here: Option<&str>) -> Vec<Row<'a>> {
    let mut rows: Vec<Row<'a>> = workspaces
        .iter()
        .map(|workspace| Row {
            workspace,
            here: is_own_window(&workspace.path, here),
        })
        .collect();
    rows.sort_by(|a, b| a.workspace.name.cmp(&b.workspace.name));
    rows
}

/// What a click on a row does.
#[derive(Debug, PartialEq, Eq)]
pub enum Open<'a> {
    /// The window's own workspace: nothing to open.
    Nothing,
    /// The folder to open in a window of its own.
    Path(&'a str),
}

/// What a click on `row` does.
#[must_use]
pub fn open_target<'a>(row: &Row<'a>) -> Open<'a> {
    if row.here {
        Open::Nothing
    } else {
        Open::Path(&row.workspace.path)
    }
}

/// The folder the × and the menu of `row` remove. The window's own workspace has one too: its
/// removal closes the window afterwards (D186 (3)).
#[must_use]
pub fn remove_target<'a>(row: &Row<'a>) -> &'a str {
    &row.workspace.path
}

/// Whether the workspace at `path` is the one the window shows (`here`), so that removing it is
/// the window's own removal and runs with `closing` (D186 (3)).
#[must_use]
pub fn is_own_window(path: &str, here: Option<&str>) -> bool {
    here.is_some_and(|root| same_path(root, path))
}

/// What the window does once a removal has settled.
#[derive(Debug, PartialEq, Eq)]
pub enum AfterRemoval {
    KeepWindow,
    CloseWindow,
}

/// The window closes when its own workspace was removed, since its repository went with the
/// folder. Another workspace's removal keeps the window open (D186 (3)).
#[must_use]
pub fn after_removal(removed: &str, here: Option<&str>) -> AfterRemoval {
    if is_own_window(removed, here) {
        AfterRemoval::CloseWindow
    } else {
        AfterRemoval::KeepWindow
    }
}

/// The name a workspace folder has, which is the last part of its path.
#[must_use]
pub fn folder_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(
        || path.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The workspaces under the root, as the last listing answered them (D183 (2)). Only the
/// newest listing is applied: an answer to an older request is dropped by its id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceList {
    /// The listing in flight, if any.
    pending: Option<OperationId>,
    workspaces: Vec<WorkspaceRow>,
    /// The root the listing was made from; `None` until the first answer.
    root: Option<String>,
}

impl WorkspaceList {
    /// Starts a listing that `id` answers. It replaces the one in flight.
    pub fn request(&mut self, id: OperationId) {
        self.pending = Some(id);
    }

    /// Whether `id` is the listing the list waits for.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.pending == Some(id)
    }

    /// Keeps the answer to the listing `id`, when it is the one in flight.
    pub fn listed(&mut self, id: OperationId, workspaces: Vec<WorkspaceRow>, root: String) -> bool {
        if !self.owns(id) {
            return false;
        }
        self.pending = None;
        self.workspaces = workspaces;
        self.root = Some(root);
        true
    }

    /// The workspaces last listed.
    #[must_use]
    pub fn workspaces(&self) -> &[WorkspaceRow] {
        &self.workspaces
    }

    /// The root the workspaces were listed under, once a listing arrived.
    #[must_use]
    pub fn root(&self) -> Option<&str> {
        self.root.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, path: &str) -> WorkspaceRow {
        WorkspaceRow {
            name: name.to_owned(),
            path: path.to_owned(),
            members: 2,
        }
    }

    #[test]
    fn the_rows_are_listed_by_name() {
        let all = [
            row("checkout-v2", "/w/checkout-v2"),
            row("billing", "/w/billing"),
        ];
        let names: Vec<&str> = rows(&all, None)
            .iter()
            .map(|row| row.workspace.name.as_str())
            .collect();
        assert_eq!(names, ["billing", "checkout-v2"]);
    }

    #[test]
    fn only_the_workspace_the_window_shows_is_here() {
        let all = [row("billing", "/w/billing"), row("checkout", "/w/checkout")];
        let here: Vec<bool> = rows(&all, Some("/w/checkout/"))
            .iter()
            .map(|row| row.here)
            .collect();
        assert_eq!(here, [false, true]);
    }

    #[test]
    fn nothing_is_here_without_a_window_root() {
        let all = [row("billing", "/w/billing")];
        assert!(!rows(&all, None)[0].here);
    }

    #[test]
    fn a_click_opens_the_folder_except_on_the_window_s_own_row() {
        let all = [row("billing", "/w/billing"), row("checkout", "/w/checkout")];
        let shown = rows(&all, Some("/w/checkout"));
        assert_eq!(open_target(&shown[0]), Open::Path("/w/billing"));
        assert_eq!(open_target(&shown[1]), Open::Nothing);
    }

    #[test]
    fn the_remove_button_of_every_row_targets_its_own_folder_the_window_s_included() {
        let all = [row("billing", "/w/billing"), row("checkout", "/w/checkout")];
        let shown = rows(&all, Some("/w/checkout"));
        assert_eq!(remove_target(&shown[0]), "/w/billing");
        assert_eq!(remove_target(&shown[1]), "/w/checkout");
    }

    #[test]
    fn removing_the_window_s_own_workspace_is_the_window_s_removal() {
        assert!(is_own_window("/w/checkout", Some("/w/checkout/")));
        assert!(!is_own_window("/w/billing", Some("/w/checkout")));
        assert!(!is_own_window("/w/checkout", None));
    }

    #[test]
    fn the_window_closes_when_its_own_workspace_is_removed_and_stays_for_another() {
        assert_eq!(
            after_removal("/w/checkout", Some("/w/checkout/")),
            AfterRemoval::CloseWindow
        );
        assert_eq!(
            after_removal("/w/billing", Some("/w/checkout")),
            AfterRemoval::KeepWindow
        );
        assert_eq!(after_removal("/w/checkout", None), AfterRemoval::KeepWindow);
    }

    #[test]
    fn the_folder_name_is_the_last_part_of_the_path() {
        assert_eq!(
            folder_name("/home/me/workspaces/checkout-v2"),
            "checkout-v2"
        );
        assert_eq!(
            folder_name("/home/me/workspaces/checkout-v2/"),
            "checkout-v2"
        );
    }

    #[test]
    fn only_the_newest_listing_is_applied() {
        let mut list = WorkspaceList::default();
        list.request(OperationId(1));
        list.request(OperationId(2));
        let answer = vec![row("billing", "/w/billing")];
        assert!(!list.listed(OperationId(1), answer.clone(), "/w".into()));
        assert!(list.workspaces().is_empty());
        assert!(list.listed(OperationId(2), answer, "/w".into()));
        assert_eq!(list.workspaces().len(), 1);
        assert_eq!(list.root(), Some("/w"));
    }

    #[test]
    fn an_answer_with_no_request_is_dropped() {
        let mut list = WorkspaceList::default();
        assert!(!list.listed(OperationId(7), Vec::new(), "/w".into()));
        assert_eq!(list.root(), None);
    }
}
