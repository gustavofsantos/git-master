//! Submodules in the file list and the text search (D175). The pieces here are
//! pure: they read the bytes Git printed and build the bytes the file index
//! and the search expect. The adapter in `git.rs` runs Git in the repository
//! and in each checked-out submodule, and joins the answers with these
//! functions. Only one level is read: a submodule's own submodules are not.

use std::collections::HashSet;
use std::path::Path;

/// Mode of a gitlink (a submodule) in the index.
const GITLINK_MODE: &str = "160000";

/// The paths of the gitlinks in the output of `git ls-files -z --stage`. Each
/// record is `<mode> SP <object> SP <stage> TAB <path>` ended by NUL.
#[must_use]
pub fn gitlinks(stage: &[u8]) -> Vec<String> {
    stage
        .split(|&b| b == 0)
        .filter_map(|record| {
            let tab = record.iter().position(|&b| b == b'\t')?;
            let (head, path) = (&record[..tab], &record[tab + 1..]);
            let mode = head.split(|&b| b == b' ').next()?;
            (mode == GITLINK_MODE.as_bytes() && !path.is_empty())
                .then(|| String::from_utf8_lossy(path).into_owned())
        })
        .collect()
}

/// The main checkout of a submodule from its common Git directory, as `git rev-parse
/// --git-common-dir` prints it (D181). A linked worktree shares the `.git` of the repository it was
/// made from, so that repository's folder is the checkout. A submodule's own Git directory is not
/// in a checkout; its folder is the checkout.
#[must_use]
pub fn main_checkout(folder: &str, common_dir: &str) -> String {
    let common = Path::new(common_dir);
    match (common.file_name(), common.parent()) {
        (Some(name), Some(repo)) if name == ".git" => repo.to_string_lossy().into_owned(),
        _ => folder.to_owned(),
    }
}

/// `raw` (records `T path` ended by NUL, as `git ls-files -v` prints them) without
/// the records whose path is one of `drop`.
#[must_use]
pub fn without_paths(raw: &[u8], drop: &[String]) -> Vec<u8> {
    let drop: HashSet<&[u8]> = drop.iter().map(String::as_bytes).collect();
    let mut out = Vec::with_capacity(raw.len());
    for record in raw.split_inclusive(|&b| b == 0) {
        let path = record.strip_suffix(&[0]).unwrap_or(record);
        let keep = path.get(2..).is_none_or(|p| !drop.contains(p));
        if keep {
            out.extend_from_slice(record);
        }
    }
    out
}

/// `raw` (records `T path` ended by NUL) with every path put under `folder/`.
/// A record that is not `T path` is dropped, as the index drops it too.
#[must_use]
pub fn under_folder(raw: &[u8], folder: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + folder.len() * 8);
    for record in raw.split(|&b| b == 0) {
        if record.len() <= 2 || record[1] != b' ' {
            continue;
        }
        out.extend_from_slice(&record[..2]);
        out.extend_from_slice(folder.as_bytes());
        out.push(b'/');
        out.extend_from_slice(&record[2..]);
        out.push(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_gitlinks_of_the_index_are_read() {
        let stage = b"100644 aaaa 0\tREADME.md\x00160000 bbbb 0\tservice-a\x00160000 cccc 0\tnested/lib\x00";

        assert_eq!(gitlinks(stage), ["service-a", "nested/lib"]);
    }

    #[test]
    fn a_linked_worktree_member_is_checked_out_in_the_repository_it_was_made_from() {
        assert_eq!(
            main_checkout("/ws/billing", "/home/me/repo/.git"),
            "/home/me/repo"
        );
    }

    #[test]
    fn a_submodule_with_its_own_git_directory_is_checked_out_in_its_folder() {
        assert_eq!(
            main_checkout("/ws/billing", "/ws/.git/modules/billing"),
            "/ws/billing"
        );
    }

    #[test]
    fn a_path_with_a_tab_keeps_its_tail() {
        let stage = b"160000 bbbb 0\tweird\tname\0";
        assert_eq!(gitlinks(stage), ["weird\tname"]);
    }

    #[test]
    fn an_empty_or_malformed_index_has_no_gitlinks() {
        assert!(gitlinks(b"").is_empty());
        assert!(gitlinks(b"160000 bbbb 0 no-tab\0").is_empty());
    }

    #[test]
    fn the_gitlink_records_are_removed_and_the_rest_kept() {
        let raw = b"H service-a\0H README.md\0? new.txt\0H service-a2\0";

        let kept = without_paths(raw, &["service-a".to_owned()]);

        assert_eq!(kept, b"H README.md\0? new.txt\0H service-a2\0");
    }

    #[test]
    fn removing_nothing_keeps_the_bytes() {
        let raw = b"H a.rs\0H b.rs\0";
        assert_eq!(without_paths(raw, &[]), raw);
    }

    #[test]
    fn paths_are_put_under_the_submodule_folder_with_their_status() {
        let raw = b"H src/main.rs\0? notes.txt\0";

        let under = under_folder(raw, "service-a");

        assert_eq!(under, b"H service-a/src/main.rs\0? service-a/notes.txt\0");
    }

    #[test]
    fn a_record_without_a_status_is_dropped_when_prefixing() {
        assert_eq!(under_folder(b"H\0Hx\0H a\0", "s"), b"H s/a\0");
    }
}
