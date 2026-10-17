//! Counts of the working copy, for the column heading and the status bar.
//! Pure data: computed from the status the engine reported.

use gm_schema::StatusEntry;

/// How many entries have each kind of change. A file staged and unstaged
/// counts under both `staged` and `unstaged`; conflicted and untracked files
/// count only under their own field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
}

#[must_use]
pub fn summarize(entries: &[StatusEntry]) -> Summary {
    let mut summary = Summary::default();
    for entry in entries {
        if entry.conflicted {
            summary.conflicted += 1;
        } else if entry.untracked {
            summary.untracked += 1;
        } else {
            summary.staged += usize::from(entry.staged.is_some());
            summary.unstaged += usize::from(entry.unstaged.is_some());
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::Change::Modified;

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

    #[test]
    fn a_file_staged_and_unstaged_counts_under_both() {
        let both = StatusEntry {
            staged: Some(Modified),
            unstaged: Some(Modified),
            ..entry("a")
        };
        assert_eq!(
            summarize(&[both]),
            Summary {
                staged: 1,
                unstaged: 1,
                untracked: 0,
                conflicted: 0,
            }
        );
    }

    #[test]
    fn untracked_and_conflicted_have_their_own_fields() {
        let untracked = StatusEntry {
            untracked: true,
            ..entry("u")
        };
        let conflicted = StatusEntry {
            conflicted: true,
            staged: Some(Modified),
            unstaged: Some(Modified),
            ..entry("c")
        };
        assert_eq!(
            summarize(&[untracked, conflicted]),
            Summary {
                staged: 0,
                unstaged: 0,
                untracked: 1,
                conflicted: 1,
            }
        );
    }

    #[test]
    fn ten_thousand_entries_are_counted() {
        let entries: Vec<StatusEntry> = (0..10_000)
            .map(|n| StatusEntry {
                unstaged: Some(Modified),
                ..entry(&format!("dir/{n}.rs"))
            })
            .collect();
        assert_eq!(summarize(&entries).unstaged, 10_000);
    }
}
