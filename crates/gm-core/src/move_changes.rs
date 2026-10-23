//! Moving the change of one file to another branch or worktree (D109). Pure:
//! the version floor, the path rules, the per-file plan and the order of the
//! blockers. The Git runs and the writes are in the engine.

use gm_schema::{MoveBlocker, OperationId};

/// The oldest Git this feature works with: `merge-tree --write-tree` (D109).
pub const MIN_GIT: (u32, u32, u32) = (2, 38, 0);

/// How many leading bytes are scanned for a NUL to call a file binary, as Git does.
const BINARY_SCAN: usize = 8000;

/// What to do with one file in the target (D109, the per-file table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilePlan {
    /// The target already has the change, or there is nothing to do.
    NoOp,
    /// Write these bytes as a new file.
    Create(Vec<u8>),
    /// Write these bytes over the file.
    Replace(Vec<u8>),
    /// Remove the file from the target.
    Delete,
    /// Run the three-way merge `git merge-file -p` on the file.
    Merge,
    /// Refuse the move for this file.
    Blocked(MoveBlocker),
}

/// Reads `git --version` output, such as `git version 2.39.3 (Apple Git-146)`,
/// into major, minor and patch. A missing patch is 0. `None` when the text
/// does not start with `git version` or has no major and minor numbers.
#[must_use]
pub fn parse_git_version(raw: &str) -> Option<(u32, u32, u32)> {
    let version = raw.trim().strip_prefix("git version")?.trim_start();
    let token = version.split_whitespace().next()?;
    let mut parts = token.split('.').map(leading_number);
    let major = parts.next()??;
    let minor = parts.next()??;
    let patch = parts.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

/// The number at the start of `part`, such as `0` in `0-rc1`.
fn leading_number(part: &str) -> Option<u32> {
    let digits: &str = &part[..part
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(part.len())];
    digits.parse().ok()
}

/// The refusal for an installed Git below `MIN_GIT`, or `None` when it is new
/// enough. Text that does not parse is refused too, with the text as found.
#[must_use]
pub fn git_blocker(raw: &str) -> Option<MoveBlocker> {
    match parse_git_version(raw) {
        Some(version) if version >= MIN_GIT => None,
        Some((major, minor, patch)) => Some(MoveBlocker::GitTooOld {
            found: format!("{major}.{minor}.{patch}"),
        }),
        None => Some(MoveBlocker::GitTooOld {
            found: raw.trim().to_owned(),
        }),
    }
}

/// True for a relative path that cannot leave the worktree: not empty, no NUL,
/// no backslash (a Windows separator or a UNC prefix), no drive prefix such as
/// `C:`, no leading or trailing slash, and no empty, `.` or `..` component.
#[must_use]
pub fn safe_relative(path: &str) -> bool {
    if path.is_empty() || path.contains(['\0', '\\']) {
        return false;
    }
    let mut components = path.split('/');
    let first = components.next().unwrap_or_default();
    let drive = first.as_bytes();
    if drive.len() >= 2 && drive[0].is_ascii_alphabetic() && drive[1] == b':' {
        return false;
    }
    std::iter::once(first)
        .chain(components)
        .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// True when the first `BINARY_SCAN` bytes of `bytes` hold a NUL.
fn is_binary(bytes: Option<&[u8]>) -> bool {
    bytes.is_some_and(|bytes| bytes.iter().take(BINARY_SCAN).any(|&byte| byte == 0))
}

/// Plans one file of a move (D109). `base` is the file at the source HEAD,
/// `theirs` the file in the snapshot (the change being moved) and `ours` the
/// file on disk in the target; `None` means absent.
#[must_use]
pub fn plan_file_move(base: Option<&[u8]>, theirs: Option<&[u8]>, ours: Option<&[u8]>) -> FilePlan {
    let Some(theirs) = theirs else {
        return match ours {
            None => FilePlan::NoOp,
            Some(_) if ours == base => FilePlan::Delete,
            Some(_) => FilePlan::Blocked(MoveBlocker::Conflict { paths: Vec::new() }),
        };
    };
    let Some(base) = base else {
        return match ours {
            None => FilePlan::Create(theirs.to_vec()),
            Some(ours) if ours == theirs => FilePlan::NoOp,
            Some(_) => FilePlan::Blocked(MoveBlocker::AddOverExisting),
        };
    };
    if is_binary(Some(base)) || is_binary(Some(theirs)) || is_binary(ours) {
        return if ours == Some(theirs) {
            FilePlan::NoOp
        } else if ours == Some(base) {
            FilePlan::Replace(theirs.to_vec())
        } else {
            FilePlan::Blocked(MoveBlocker::BinaryDiffers)
        };
    }
    let Some(ours) = ours else {
        return FilePlan::Blocked(MoveBlocker::Conflict { paths: Vec::new() });
    };
    if theirs == base {
        FilePlan::NoOp
    } else if ours == base {
        FilePlan::Replace(theirs.to_vec())
    } else if ours == theirs {
        FilePlan::NoOp
    } else {
        FilePlan::Merge
    }
}

/// The rank of a blocker in the order the human reads them: the ones that stop
/// everything first, then the ones about the branch and worktree, then the
/// ones about the file.
fn rank(blocker: &MoveBlocker) -> u8 {
    match blocker {
        MoveBlocker::GitTooOld { .. } => 0,
        MoveBlocker::NothingToMove => 1,
        MoveBlocker::Conflicted => 2,
        MoveBlocker::SameBranch => 3,
        MoveBlocker::SameWorktree => 4,
        MoveBlocker::InvalidName { .. } => 5,
        MoveBlocker::BranchExists => 6,
        MoveBlocker::BranchNotFound => 7,
        MoveBlocker::BranchCheckedOutElsewhere { .. } => 8,
        MoveBlocker::WorktreeNotFound => 9,
        MoveBlocker::WorktreeMissing => 10,
        MoveBlocker::UnsafePath => 11,
        MoveBlocker::Conflict { .. } => 12,
        MoveBlocker::BinaryDiffers => 13,
        MoveBlocker::AddOverExisting => 14,
    }
}

/// Sorts blockers into the order `rank` gives. Equal ranks keep their order.
#[must_use]
pub fn order_blockers(mut blockers: Vec<MoveBlocker>) -> Vec<MoveBlocker> {
    blockers.sort_by_key(rank);
    blockers
}

/// The one-line English sentence that tells the human why a move is refused.
#[must_use]
pub fn describe(blocker: &MoveBlocker) -> String {
    match blocker {
        MoveBlocker::NothingToMove => "the file has no change to move".to_owned(),
        MoveBlocker::Conflicted => "the file is conflicted; resolve the conflict first".to_owned(),
        MoveBlocker::SameBranch => "that is the branch you are on".to_owned(),
        MoveBlocker::SameWorktree => "that is the worktree you are in".to_owned(),
        MoveBlocker::BranchExists => "the branch already exists".to_owned(),
        MoveBlocker::BranchNotFound => "there is no such branch".to_owned(),
        MoveBlocker::InvalidName { reason } => reason.clone(),
        MoveBlocker::BranchCheckedOutElsewhere { path } => {
            format!("the branch is checked out in {path}")
        }
        MoveBlocker::WorktreeNotFound => "no worktree has that folder".to_owned(),
        MoveBlocker::WorktreeMissing => "the worktree folder is missing".to_owned(),
        MoveBlocker::UnsafePath => "the path is a symbolic link or leaves the worktree".to_owned(),
        MoveBlocker::GitTooOld { found } => {
            format!("moving needs Git 2.38 or newer (found {found})")
        }
        MoveBlocker::Conflict { paths } if paths.is_empty() => {
            "the change conflicts with the target".to_owned()
        }
        MoveBlocker::Conflict { paths } => {
            format!("the change conflicts with {}", paths.join(", "))
        }
        MoveBlocker::BinaryDiffers => {
            "a binary file differs from the version the change started from".to_owned()
        }
        MoveBlocker::AddOverExisting => "a file with other content already exists there".to_owned(),
    }
}

/// The commit message of a branch move, trimmed. Refused when it is empty.
pub fn commit_message(message: Option<&str>) -> Result<String, String> {
    match message.map(str::trim) {
        Some(text) if !text.is_empty() => Ok(text.to_owned()),
        _ => Err("a commit message is required".to_owned()),
    }
}

/// The safety ref that keeps the snapshot of operation `id` (D109, Q24).
#[must_use]
pub fn move_ref_name(id: OperationId) -> String {
    format!("refs/gitmaster/move/{}", id.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_versions_git_prints() {
        assert_eq!(parse_git_version("git version 2.43.0\n"), Some((2, 43, 0)));
        assert_eq!(
            parse_git_version("git version 2.39.3 (Apple Git-146)"),
            Some((2, 39, 3))
        );
        assert_eq!(
            parse_git_version("git version 2.45.1.windows.1"),
            Some((2, 45, 1))
        );
        assert_eq!(parse_git_version("git version 2.38"), Some((2, 38, 0)));
        assert_eq!(
            parse_git_version("git version 2.38.0-rc1"),
            Some((2, 38, 0))
        );
    }

    #[test]
    fn refuses_text_that_is_not_a_version() {
        assert_eq!(parse_git_version(""), None);
        assert_eq!(parse_git_version("git version"), None);
        assert_eq!(parse_git_version("git version 2"), None);
        assert_eq!(parse_git_version("git version x.y.z"), None);
        assert_eq!(parse_git_version("hg version 2.38.0"), None);
    }

    #[test]
    fn git_below_the_floor_is_blocked_with_its_version() {
        assert_eq!(git_blocker("git version 2.38.0"), None);
        assert_eq!(git_blocker("git version 2.43.0"), None);
        assert_eq!(
            git_blocker("git version 2.34.1\n"),
            Some(MoveBlocker::GitTooOld {
                found: "2.34.1".into()
            })
        );
        assert_eq!(
            git_blocker("git version 2.37"),
            Some(MoveBlocker::GitTooOld {
                found: "2.37.0".into()
            })
        );
    }

    #[test]
    fn unparsable_git_version_is_blocked_with_the_raw_text() {
        assert_eq!(
            git_blocker("  weird output \n"),
            Some(MoveBlocker::GitTooOld {
                found: "weird output".into()
            })
        );
    }

    #[test]
    fn accepts_plain_relative_paths() {
        assert!(safe_relative("a"));
        assert!(safe_relative("src/main.rs"));
        assert!(safe_relative(".gitignore"));
        assert!(safe_relative("a..b/c"));
        assert!(safe_relative("dir/.hidden"));
    }

    #[test]
    fn refuses_paths_that_leave_or_are_not_relative() {
        for path in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a/..",
            "..",
            "./a",
            "a/./b",
            "a/",
            "a//b",
            "\\\\server\\share",
            "C:\\x",
            "C:x",
            "a\\b",
            "a\0b",
        ] {
            assert!(!safe_relative(path), "{path:?} must be refused");
        }
    }

    #[test]
    fn deleted_in_source_and_absent_in_target_is_a_no_op() {
        assert_eq!(plan_file_move(Some(b"a"), None, None), FilePlan::NoOp);
        assert_eq!(plan_file_move(None, None, None), FilePlan::NoOp);
    }

    #[test]
    fn deleted_in_source_and_unchanged_in_target_is_deleted() {
        assert_eq!(
            plan_file_move(Some(b"a"), None, Some(b"a")),
            FilePlan::Delete
        );
    }

    #[test]
    fn deleted_in_source_and_changed_in_target_is_a_conflict() {
        assert_eq!(
            plan_file_move(Some(b"a"), None, Some(b"b")),
            FilePlan::Blocked(MoveBlocker::Conflict { paths: Vec::new() })
        );
    }

    #[test]
    fn added_in_source_and_absent_in_target_is_created() {
        assert_eq!(
            plan_file_move(None, Some(b"new"), None),
            FilePlan::Create(b"new".to_vec())
        );
    }

    #[test]
    fn added_in_source_and_same_content_in_target_is_a_no_op() {
        assert_eq!(
            plan_file_move(None, Some(b"new"), Some(b"new")),
            FilePlan::NoOp
        );
    }

    #[test]
    fn added_in_source_over_other_content_is_refused() {
        assert_eq!(
            plan_file_move(None, Some(b"new"), Some(b"other")),
            FilePlan::Blocked(MoveBlocker::AddOverExisting)
        );
    }

    #[test]
    fn text_unchanged_in_snapshot_is_a_no_op() {
        assert_eq!(
            plan_file_move(Some(b"a"), Some(b"a"), Some(b"b")),
            FilePlan::NoOp
        );
    }

    #[test]
    fn text_with_target_at_base_is_replaced() {
        assert_eq!(
            plan_file_move(Some(b"a"), Some(b"b"), Some(b"a")),
            FilePlan::Replace(b"b".to_vec())
        );
    }

    #[test]
    fn text_with_target_already_equal_is_a_no_op() {
        assert_eq!(
            plan_file_move(Some(b"a"), Some(b"b"), Some(b"b")),
            FilePlan::NoOp
        );
    }

    #[test]
    fn text_changed_on_both_sides_is_merged() {
        assert_eq!(
            plan_file_move(Some(b"a\n"), Some(b"b\n"), Some(b"c\n")),
            FilePlan::Merge
        );
    }

    #[test]
    fn text_the_target_deleted_is_a_conflict() {
        assert_eq!(
            plan_file_move(Some(b"a"), Some(b"b"), None),
            FilePlan::Blocked(MoveBlocker::Conflict { paths: Vec::new() })
        );
    }

    #[test]
    fn binary_with_target_already_equal_is_a_no_op() {
        assert_eq!(
            plan_file_move(Some(b"\0a"), Some(b"\0b"), Some(b"\0b")),
            FilePlan::NoOp
        );
    }

    #[test]
    fn binary_with_target_at_base_is_replaced() {
        assert_eq!(
            plan_file_move(Some(b"\0a"), Some(b"\0b"), Some(b"\0a")),
            FilePlan::Replace(b"\0b".to_vec())
        );
    }

    #[test]
    fn binary_that_differs_from_base_in_target_is_refused() {
        assert_eq!(
            plan_file_move(Some(b"\0a"), Some(b"\0b"), Some(b"\0c")),
            FilePlan::Blocked(MoveBlocker::BinaryDiffers)
        );
    }

    #[test]
    fn a_nul_in_the_base_alone_makes_the_file_binary() {
        assert_eq!(
            plan_file_move(Some(b"x\0"), Some(b"b"), Some(b"c")),
            FilePlan::Blocked(MoveBlocker::BinaryDiffers)
        );
    }

    #[test]
    fn a_nul_in_the_target_alone_makes_the_file_binary() {
        assert_eq!(
            plan_file_move(Some(b"a"), Some(b"b\0"), Some(b"a")),
            FilePlan::Replace(b"b\0".to_vec())
        );
    }

    #[test]
    fn a_nul_after_the_scan_window_is_not_binary() {
        let mut base = vec![b'a'; BINARY_SCAN];
        base.push(0);
        let theirs = b"text".to_vec();
        let ours = base.clone();
        assert_eq!(
            plan_file_move(Some(&base), Some(&theirs), Some(&ours)),
            FilePlan::Replace(theirs)
        );
    }

    #[test]
    fn a_binary_file_deleted_in_the_target_is_refused() {
        assert_eq!(
            plan_file_move(Some(b"\0a"), Some(b"\0b"), None),
            FilePlan::Blocked(MoveBlocker::BinaryDiffers)
        );
    }

    #[test]
    fn blockers_are_ordered_by_rank_and_stable() {
        let blockers = vec![
            MoveBlocker::AddOverExisting,
            MoveBlocker::BinaryDiffers,
            MoveBlocker::Conflict {
                paths: vec!["a".into()],
            },
            MoveBlocker::UnsafePath,
            MoveBlocker::WorktreeMissing,
            MoveBlocker::WorktreeNotFound,
            MoveBlocker::BranchCheckedOutElsewhere { path: "/w".into() },
            MoveBlocker::BranchNotFound,
            MoveBlocker::BranchExists,
            MoveBlocker::InvalidName { reason: "x".into() },
            MoveBlocker::SameWorktree,
            MoveBlocker::SameBranch,
            MoveBlocker::Conflicted,
            MoveBlocker::NothingToMove,
            MoveBlocker::GitTooOld {
                found: "2.34.1".into(),
            },
            MoveBlocker::Conflict { paths: Vec::new() },
        ];
        assert_eq!(
            order_blockers(blockers),
            vec![
                MoveBlocker::GitTooOld {
                    found: "2.34.1".into()
                },
                MoveBlocker::NothingToMove,
                MoveBlocker::Conflicted,
                MoveBlocker::SameBranch,
                MoveBlocker::SameWorktree,
                MoveBlocker::InvalidName { reason: "x".into() },
                MoveBlocker::BranchExists,
                MoveBlocker::BranchNotFound,
                MoveBlocker::BranchCheckedOutElsewhere { path: "/w".into() },
                MoveBlocker::WorktreeNotFound,
                MoveBlocker::WorktreeMissing,
                MoveBlocker::UnsafePath,
                MoveBlocker::Conflict {
                    paths: vec!["a".into()]
                },
                MoveBlocker::Conflict { paths: Vec::new() },
                MoveBlocker::BinaryDiffers,
                MoveBlocker::AddOverExisting,
            ]
        );
    }

    #[test]
    fn every_blocker_has_a_one_line_sentence() {
        let blockers = [
            MoveBlocker::NothingToMove,
            MoveBlocker::Conflicted,
            MoveBlocker::SameBranch,
            MoveBlocker::SameWorktree,
            MoveBlocker::BranchExists,
            MoveBlocker::BranchNotFound,
            MoveBlocker::InvalidName {
                reason: "a branch name cannot be empty".into(),
            },
            MoveBlocker::BranchCheckedOutElsewhere {
                path: "/w/dev".into(),
            },
            MoveBlocker::WorktreeNotFound,
            MoveBlocker::WorktreeMissing,
            MoveBlocker::UnsafePath,
            MoveBlocker::GitTooOld {
                found: "2.34.1".into(),
            },
            MoveBlocker::Conflict {
                paths: vec!["a.txt".into()],
            },
            MoveBlocker::Conflict { paths: Vec::new() },
            MoveBlocker::BinaryDiffers,
            MoveBlocker::AddOverExisting,
        ];
        for blocker in blockers {
            let text = describe(&blocker);
            assert!(!text.is_empty(), "{blocker:?} has no sentence");
            assert!(!text.contains('\n'), "{blocker:?} spans lines: {text:?}");
        }
    }

    #[test]
    fn describes_each_refusal_in_plain_words() {
        assert_eq!(
            describe(&MoveBlocker::NothingToMove),
            "the file has no change to move"
        );
        assert_eq!(
            describe(&MoveBlocker::Conflicted),
            "the file is conflicted; resolve the conflict first"
        );
        assert_eq!(
            describe(&MoveBlocker::SameBranch),
            "that is the branch you are on"
        );
        assert_eq!(
            describe(&MoveBlocker::SameWorktree),
            "that is the worktree you are in"
        );
        assert_eq!(
            describe(&MoveBlocker::BranchExists),
            "the branch already exists"
        );
        assert_eq!(
            describe(&MoveBlocker::BranchNotFound),
            "there is no such branch"
        );
        assert_eq!(
            describe(&MoveBlocker::InvalidName {
                reason: "bad name".into()
            }),
            "bad name"
        );
        assert_eq!(
            describe(&MoveBlocker::BranchCheckedOutElsewhere {
                path: "/w/dev".into()
            }),
            "the branch is checked out in /w/dev"
        );
        assert_eq!(
            describe(&MoveBlocker::WorktreeNotFound),
            "no worktree has that folder"
        );
        assert_eq!(
            describe(&MoveBlocker::WorktreeMissing),
            "the worktree folder is missing"
        );
        assert_eq!(
            describe(&MoveBlocker::UnsafePath),
            "the path is a symbolic link or leaves the worktree"
        );
        assert_eq!(
            describe(&MoveBlocker::GitTooOld {
                found: "2.34.1".into()
            }),
            "moving needs Git 2.38 or newer (found 2.34.1)"
        );
        assert_eq!(
            describe(&MoveBlocker::Conflict {
                paths: vec!["a.txt".into(), "b/c.txt".into()]
            }),
            "the change conflicts with a.txt, b/c.txt"
        );
        assert_eq!(
            describe(&MoveBlocker::Conflict { paths: Vec::new() }),
            "the change conflicts with the target"
        );
        assert_eq!(
            describe(&MoveBlocker::BinaryDiffers),
            "a binary file differs from the version the change started from"
        );
        assert_eq!(
            describe(&MoveBlocker::AddOverExisting),
            "a file with other content already exists there"
        );
    }

    #[test]
    fn the_commit_message_is_trimmed_and_must_not_be_empty() {
        assert_eq!(
            commit_message(Some("  move a.txt\n")).as_deref(),
            Ok("move a.txt")
        );
        assert_eq!(
            commit_message(None),
            Err("a commit message is required".to_owned())
        );
        assert_eq!(
            commit_message(Some(" \n\t ")),
            Err("a commit message is required".to_owned())
        );
    }

    #[test]
    fn the_safety_ref_is_named_by_the_operation() {
        assert_eq!(move_ref_name(OperationId(7)), "refs/gitmaster/move/7");
    }
}
