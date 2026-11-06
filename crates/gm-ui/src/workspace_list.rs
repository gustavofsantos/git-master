//! The rows of the sidebar's WORKSPACES section, and the listing they come from (D183 (2)).
//! Pure: the view draws what this lists. docs/guide/workspaces.md, "The Workspaces section".

use std::path::Path;

use gm_schema::{OperationId, WorkspaceKind, WorkspaceRow};

use crate::worktree_list::same_path;

/// One row of the section.
#[derive(Debug, PartialEq, Eq)]
pub struct Row<'a> {
    pub workspace: &'a WorkspaceRow,
    /// The workspace this window shows.
    pub here: bool,
    /// Removed when it is closed, so the row carries the **ephemeral** badge.
    pub ephemeral: bool,
}

/// The rows for `workspaces`, by name, for a window showing the folder `here`.
#[must_use]
pub fn rows<'a>(workspaces: &'a [WorkspaceRow], here: Option<&str>) -> Vec<Row<'a>> {
    let mut rows: Vec<Row<'a>> = workspaces
        .iter()
        .map(|workspace| Row {
            workspace,
            here: here.is_some_and(|root| same_path(root, &workspace.path)),
            ephemeral: workspace.kind == WorkspaceKind::Ephemeral,
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

    fn row(name: &str, path: &str, kind: WorkspaceKind) -> WorkspaceRow {
        WorkspaceRow {
            name: name.to_owned(),
            kind,
            path: path.to_owned(),
            members: 2,
        }
    }

    #[test]
    fn the_rows_are_listed_by_name() {
        let all = [
            row("checkout-v2", "/w/checkout-v2", WorkspaceKind::Durable),
            row("billing", "/w/billing", WorkspaceKind::Durable),
        ];
        let names: Vec<&str> = rows(&all, None)
            .iter()
            .map(|row| row.workspace.name.as_str())
            .collect();
        assert_eq!(names, ["billing", "checkout-v2"]);
    }

    #[test]
    fn only_the_workspace_the_window_shows_is_here() {
        let all = [
            row("billing", "/w/billing", WorkspaceKind::Durable),
            row("checkout", "/w/checkout", WorkspaceKind::Durable),
        ];
        let here: Vec<bool> = rows(&all, Some("/w/checkout/"))
            .iter()
            .map(|row| row.here)
            .collect();
        assert_eq!(here, [false, true]);
    }

    #[test]
    fn nothing_is_here_without_a_window_root() {
        let all = [row("billing", "/w/billing", WorkspaceKind::Durable)];
        assert!(!rows(&all, None)[0].here);
    }

    #[test]
    fn an_ephemeral_workspace_carries_the_badge_and_a_durable_one_does_not() {
        let all = [
            row("scratch", "/w/scratch", WorkspaceKind::Ephemeral),
            row("billing", "/w/billing", WorkspaceKind::Durable),
        ];
        let shown = rows(&all, None);
        let ephemeral = |name: &str| {
            shown
                .iter()
                .find(|row| row.workspace.name == name)
                .is_some_and(|row| row.ephemeral)
        };
        assert!(ephemeral("scratch"));
        assert!(!ephemeral("billing"));
    }

    #[test]
    fn a_click_opens_the_folder_except_on_the_window_s_own_row() {
        let all = [
            row("billing", "/w/billing", WorkspaceKind::Durable),
            row("checkout", "/w/checkout", WorkspaceKind::Durable),
        ];
        let shown = rows(&all, Some("/w/checkout"));
        assert_eq!(open_target(&shown[0]), Open::Path("/w/billing"));
        assert_eq!(open_target(&shown[1]), Open::Nothing);
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
        let answer = vec![row("billing", "/w/billing", WorkspaceKind::Durable)];
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
