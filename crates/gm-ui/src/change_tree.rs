//! The rows of the Changes list (UI redesign, slice 2): the changed files grouped under their
//! folder, each with a checkbox that says what is staged, and each folder with a tri-state box
//! that sums its files (D23). Pure: `shell_list` draws what this lists, and a click on a box
//! runs a registry action, so the rules are the menus' and the keys'.
//!
//! "Checked" means "goes into the commit": a file is checked when all its change is staged,
//! mixed when part of it is, and unchecked when none is. A conflicted file has no box: it is
//! resolved, not staged.

use gm_schema::StatusEntry;

use crate::action_registry::Action;

/// The state of a checkbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Check {
    Checked,
    Mixed,
    Unchecked,
}

impl Check {
    /// The state of `entry`'s box; `None` for a conflicted file, which has none.
    #[must_use]
    pub(crate) fn of(entry: &StatusEntry) -> Option<Check> {
        if entry.conflicted {
            return None;
        }
        let staged = entry.staged.is_some();
        let unstaged = entry.unstaged.is_some() || entry.untracked;
        Some(match (staged, unstaged) {
            (true, false) => Check::Checked,
            (true, true) => Check::Mixed,
            _ => Check::Unchecked,
        })
    }

    /// The state of a folder from its files': checked when all are, unchecked when none is
    /// staged at all, mixed otherwise. `None` when no file has a box.
    #[must_use]
    pub(crate) fn sum(checks: impl IntoIterator<Item = Check>) -> Option<Check> {
        let mut sum: Option<Check> = None;
        for check in checks {
            sum = Some(match (sum, check) {
                (None, check) => check,
                (Some(previous), check) if previous == check => check,
                _ => Check::Mixed,
            });
        }
        sum
    }

    /// What a click on a box in this state runs: a checked box unstages, any other stages.
    #[must_use]
    pub(crate) fn file_action(self) -> Action {
        match self {
            Check::Checked => Action::UnstageFile,
            Check::Mixed | Check::Unchecked => Action::StageFile,
        }
    }

    /// What a click on a folder's box runs.
    #[must_use]
    pub(crate) fn folder_action(self) -> Action {
        match self {
            Check::Checked => Action::UnstageFolder,
            Check::Mixed | Check::Unchecked => Action::StageFolder,
        }
    }
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// A folder, by its path with a trailing `/` (empty for the root, which has no row).
    Folder { path: String, check: Option<Check> },
    /// A file, by its index in the status entries, with its name inside the folder.
    File {
        index: usize,
        name: String,
        check: Option<Check>,
        /// Drawn under a folder row.
        indented: bool,
    },
}

/// The folder of `path`, with a trailing `/`; empty for a file at the root.
#[must_use]
pub(crate) fn folder_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(end) => &path[..=end],
        None => "",
    }
}

/// The rows for `entries`, in the order of the entries (the status lists them by path, so
/// each folder's files are adjacent). The files at the root come without a folder row.
#[must_use]
pub(crate) fn rows(entries: &[StatusEntry]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(entries.len() + 8);
    let mut index = 0;
    while index < entries.len() {
        let folder = folder_of(&entries[index].path);
        let end = entries[index..]
            .iter()
            .position(|entry| folder_of(&entry.path) != folder)
            .map_or(entries.len(), |offset| index + offset);
        let group = &entries[index..end];
        if !folder.is_empty() {
            rows.push(Row::Folder {
                path: folder.to_owned(),
                check: Check::sum(group.iter().filter_map(Check::of)),
            });
        }
        rows.extend(group.iter().enumerate().map(|(offset, entry)| Row::File {
            index: index + offset,
            name: entry.path[folder.len()..].to_owned(),
            check: Check::of(entry),
            indented: !folder.is_empty(),
        }));
        index = end;
    }
    rows
}

/// The position in `rows` of the file with entry index `index`.
#[must_use]
pub(crate) fn row_of(rows: &[Row], index: usize) -> Option<usize> {
    rows.iter()
        .position(|row| matches!(row, Row::File { index: at, .. } if *at == index))
}

#[cfg(test)]
mod tests {
    use gm_schema::Change;

    use super::*;

    fn entry(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
            submodule: None,
        }
    }

    const M: Option<Change> = Some(Change::Modified);

    #[test]
    fn a_file_is_checked_when_all_is_staged_and_mixed_when_part_is() {
        assert_eq!(Check::of(&entry("a", M, None)), Some(Check::Checked));
        assert_eq!(Check::of(&entry("a", M, M)), Some(Check::Mixed));
        assert_eq!(Check::of(&entry("a", None, M)), Some(Check::Unchecked));
        let mut untracked = entry("a", None, None);
        untracked.untracked = true;
        assert_eq!(Check::of(&untracked), Some(Check::Unchecked));
        let mut conflicted = entry("a", M, M);
        conflicted.conflicted = true;
        assert_eq!(Check::of(&conflicted), None);
    }

    #[test]
    fn a_folder_sums_its_files() {
        use Check::{Checked, Mixed, Unchecked};
        assert_eq!(Check::sum([Checked, Checked]), Some(Checked));
        assert_eq!(Check::sum([Unchecked, Unchecked]), Some(Unchecked));
        assert_eq!(Check::sum([Checked, Unchecked]), Some(Mixed));
        assert_eq!(Check::sum([Mixed]), Some(Mixed));
        assert_eq!(Check::sum([]), None);
    }

    #[test]
    fn a_click_stages_unless_the_box_is_checked() {
        assert_eq!(Check::Checked.file_action(), Action::UnstageFile);
        assert_eq!(Check::Mixed.file_action(), Action::StageFile);
        assert_eq!(Check::Unchecked.file_action(), Action::StageFile);
        assert_eq!(Check::Checked.folder_action(), Action::UnstageFolder);
        assert_eq!(Check::Mixed.folder_action(), Action::StageFolder);
    }

    #[test]
    fn files_group_under_their_folder_and_root_files_have_no_folder_row() {
        let entries = [
            entry("README.md", None, M),
            entry("src/a.rs", M, None),
            entry("src/b.rs", None, M),
            entry("src/ui/c.rs", M, None),
        ];
        let rows = rows(&entries);
        assert_eq!(
            rows,
            vec![
                Row::File {
                    index: 0,
                    name: "README.md".into(),
                    check: Some(Check::Unchecked),
                    indented: false
                },
                Row::Folder {
                    path: "src/".into(),
                    check: Some(Check::Mixed)
                },
                Row::File {
                    index: 1,
                    name: "a.rs".into(),
                    check: Some(Check::Checked),
                    indented: true
                },
                Row::File {
                    index: 2,
                    name: "b.rs".into(),
                    check: Some(Check::Unchecked),
                    indented: true
                },
                Row::Folder {
                    path: "src/ui/".into(),
                    check: Some(Check::Checked)
                },
                Row::File {
                    index: 3,
                    name: "c.rs".into(),
                    check: Some(Check::Checked),
                    indented: true
                },
            ]
        );
        assert_eq!(row_of(&rows, 2), Some(3));
        assert_eq!(row_of(&rows, 9), None);
    }
}
