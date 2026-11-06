//! The rows of the sidebar's WORKTREES section. Pure: the view draws what this
//! lists.
//!
//! The main checkout comes first, then the other worktrees in the order git
//! lists them. A bare entry has no checkout to show, so it is left out.

use std::path::Path;

use gm_schema::WorktreeEntry;

/// One row of the section.
#[derive(Debug, PartialEq, Eq)]
pub struct Row<'a> {
    pub entry: &'a WorktreeEntry,
    /// The branch name, or the folder name when there is no branch.
    pub name: String,
    /// The worktree this window shows.
    pub here: bool,
    /// Git can prune it, because its folder is gone.
    pub missing: bool,
    /// The colour slot, which only the name decides: the same name always
    /// has the same slot, across sessions.
    pub lane: usize,
}

/// The rows for `entries`, for a window showing the worktree at `here`.
#[must_use]
pub fn rows<'a>(entries: &'a [WorktreeEntry], here: Option<&str>, lanes: usize) -> Vec<Row<'a>> {
    let mut rows: Vec<Row<'a>> = entries
        .iter()
        .filter(|entry| !entry.bare)
        .map(|entry| {
            let name = display_name(entry);
            Row {
                entry,
                here: here.is_some_and(|root| same_path(root, &entry.path)),
                missing: entry.prunable.is_some(),
                lane: lane_of(&name, lanes),
                name,
            }
        })
        .collect();
    // A stable sort, so the rest keep git's order.
    rows.sort_by_key(|row| !row.entry.is_main);
    rows
}

/// What a double-click on a row does.
#[derive(Debug, PartialEq, Eq)]
pub enum Open<'a> {
    /// The window's own worktree: nothing to open.
    Nothing,
    /// The folder is gone, so there is nothing to open until it is pruned.
    Missing,
    /// The folder to open in a window of its own.
    Path(&'a str),
}

/// What a double-click on `row` does.
#[must_use]
pub fn open_target<'a>(row: &Row<'a>) -> Open<'a> {
    if row.here {
        Open::Nothing
    } else if row.missing {
        Open::Missing
    } else {
        Open::Path(&row.entry.path)
    }
}

/// The branch the worktree has, or else its folder name. The sidebar and the
/// palette both name a worktree this way.
#[must_use]
pub fn display_name(entry: &WorktreeEntry) -> String {
    if let Some(branch) = &entry.branch {
        return branch.clone();
    }
    Path::new(&entry.path).file_name().map_or_else(
        || entry.path.clone(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Paths compare without a trailing separator.
pub(crate) fn same_path(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// The colour slot of `name`: FNV-1a of its bytes, modulo `lanes`.
#[must_use]
pub fn lane_of(name: &str, lanes: usize) -> usize {
    let lanes = u64::try_from(lanes).unwrap_or(1).max(1);
    usize::try_from(name_hash(name) % lanes).unwrap_or(0)
}

/// FNV-1a, 64 bit. Fixed by its definition, so it is the same in every
/// session and build (unlike the standard hasher, which is seeded).
#[must_use]
pub fn name_hash(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, branch: Option<&str>) -> WorktreeEntry {
        WorktreeEntry {
            path: path.into(),
            head: Some("abc".into()),
            branch: branch.map(Into::into),
            detached: branch.is_none(),
            bare: false,
            is_main: false,
            locked: None,
            prunable: None,
        }
    }

    fn main(path: &str, branch: &str) -> WorktreeEntry {
        WorktreeEntry {
            is_main: true,
            ..entry(path, Some(branch))
        }
    }

    #[test]
    fn the_main_checkout_comes_first_and_the_rest_keep_git_order() {
        let entries = [
            main("/w/ledger", "main"),
            entry("/w/ledger-ui", Some("ui")),
            entry("/w/ledger-api", Some("api")),
        ];
        let rows = rows(&entries, None, 6);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["main", "ui", "api"]);
    }

    #[test]
    fn a_main_that_git_did_not_put_first_still_leads() {
        let entries = [entry("/w/a", Some("a")), main("/w/main", "main")];
        let rows = rows(&entries, None, 6);
        assert_eq!(rows[0].name, "main");
        assert!(rows[0].entry.is_main);
    }

    #[test]
    fn only_the_window_s_own_worktree_is_here() {
        let entries = [main("/w/ledger", "main"), entry("/w/ledger-ui", Some("ui"))];
        let here: Vec<bool> = rows(&entries, Some("/w/ledger-ui"), 6)
            .iter()
            .map(|r| r.here)
            .collect();
        assert_eq!(here, [false, true]);
    }

    #[test]
    fn a_trailing_separator_does_not_change_which_is_here() {
        let entries = [main("/w/ledger", "main")];
        assert!(rows(&entries, Some("/w/ledger/"), 6)[0].here);
    }

    #[test]
    fn nothing_is_here_when_the_window_has_no_root() {
        let entries = [main("/w/ledger", "main")];
        assert!(!rows(&entries, None, 6)[0].here);
    }

    #[test]
    fn a_prunable_worktree_is_missing() {
        let mut gone = entry("/w/gone", Some("gone"));
        gone.prunable = Some("gitdir file points to non-existent location".into());
        let entries = [main("/w/ledger", "main"), gone];
        let missing: Vec<bool> = rows(&entries, None, 6).iter().map(|r| r.missing).collect();
        assert_eq!(missing, [false, true]);
    }

    #[test]
    fn a_locked_worktree_gets_no_tag() {
        let mut locked = entry("/w/locked", Some("locked"));
        locked.locked = Some("on a USB stick".into());
        let entries = [main("/w/ledger", "main"), locked];
        let rows = rows(&entries, None, 6);
        assert!(!rows[1].missing && !rows[1].here);
    }

    #[test]
    fn a_detached_worktree_is_named_after_its_folder() {
        let entries = [main("/w/ledger", "main"), entry("/w/scratch-review", None)];
        let rows = rows(&entries, None, 6);
        assert_eq!(rows[1].name, "scratch-review");
    }

    #[test]
    fn a_double_click_opens_the_other_worktrees_folder() {
        let entries = [main("/r", "main"), entry("/r-feature", Some("feature"))];
        let rows = rows(&entries, Some("/r"), 4);
        assert_eq!(open_target(&rows[0]), Open::Nothing);
        assert_eq!(open_target(&rows[1]), Open::Path("/r-feature"));
    }

    #[test]
    fn a_missing_worktree_is_not_opened() {
        let mut gone = entry("/r-gone", Some("gone"));
        gone.prunable = Some(String::new());
        let entries = [main("/r", "main"), gone];
        let rows = rows(&entries, Some("/r"), 4);
        assert_eq!(open_target(&rows[1]), Open::Missing);
    }

    #[test]
    fn a_bare_entry_is_left_out() {
        let mut bare = entry("/srv/ledger.git", None);
        bare.bare = true;
        bare.is_main = true;
        let entries = [bare, entry("/w/ledger-ui", Some("ui"))];
        let rows = rows(&entries, None, 6);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["ui"]);
    }

    #[test]
    fn the_hash_is_fnv1a_and_stable() {
        // The FNV-1a 64-bit test vector for "a".
        assert_eq!(name_hash("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(name_hash(""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn the_same_name_always_gets_the_same_lane_within_range() {
        for name in ["main", "ledger-ui", "feature/x", "spike-fx"] {
            let lane = lane_of(name, 6);
            assert!(lane < 6);
            assert_eq!(lane, lane_of(name, 6));
        }
    }

    #[test]
    fn a_zero_lane_count_cannot_divide_by_zero() {
        assert_eq!(lane_of("main", 0), 0);
    }

    #[test]
    fn a_row_takes_the_lane_of_its_name() {
        let entries = [main("/w/ledger", "main")];
        let rows = rows(&entries, None, 6);
        assert_eq!(rows[0].lane, lane_of("main", 6));
    }
}
