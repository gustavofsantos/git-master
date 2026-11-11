//! The changes between two revisions (D231): the checks, the base the diff starts
//! from, the totals and the limit of the file list are pure; the adapter starts
//! git. A range read runs as a job on a thread of its own, and its answer is
//! dropped when a newer request superseded it, as a detail read is (H3).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gm_schema::{CommitFile, DETAIL_FILE_LIMIT, OperationId, RangeMode, RevisionRange};

use crate::port::{GitError, RangeRead, TagsRead};

/// How many tags `ListTags` lists, newest first (D231 (4b)).
pub const TAG_LIMIT: usize = 500;

/// One range read, run on a thread of its own. `stop` is shared with the engine,
/// which sets it when the read is superseded.
pub struct RangeJob {
    pub id: OperationId,
    pub read: RangeRead,
    pub stop: Arc<AtomicBool>,
}

/// What a range job answers. A failure carries git's own message.
pub struct RangeReport {
    pub id: OperationId,
    pub outcome: Result<RevisionRange, String>,
}

impl RangeJob {
    /// Runs the read to its end, or to the stop request.
    #[must_use]
    pub fn run(self) -> RangeReport {
        let outcome = (self.read)(&self.stop).map_err(|error| error.0);
        RangeReport {
            id: self.id,
            outcome,
        }
    }
}

/// Refuses a revision that is empty or that Git would read as an option. `side`
/// (`from` or `to`) names it in the message.
pub fn check_revision(side: &str, rev: &str) -> Result<(), GitError> {
    if rev.trim().is_empty() {
        return Err(GitError(format!("{side}: the revision is empty")));
    }
    if rev.starts_with('-') {
        return Err(GitError(format!("{side}: {rev:?} is not a revision")));
    }
    Ok(())
}

/// One tag listing, run on a thread of its own. `stop` is shared with the engine.
pub struct TagsJob {
    pub id: OperationId,
    pub read: TagsRead,
    pub stop: Arc<AtomicBool>,
}

/// What a tags job answers. A failure carries git's own message.
pub struct TagsReport {
    pub id: OperationId,
    pub outcome: Result<Vec<String>, String>,
}

impl TagsJob {
    /// Runs the listing to its end, or to the stop request.
    #[must_use]
    pub fn run(self) -> TagsReport {
        let outcome = (self.read)(&self.stop).map_err(|error| error.0);
        TagsReport {
            id: self.id,
            outcome,
        }
    }
}

/// The message of a range whose two sides share no history. Both sides are named as
/// given.
#[must_use]
pub fn no_common_ancestor(from: &str, to: &str) -> String {
    format!("{from} and {to} have no common ancestor")
}

/// The tag names of `git for-each-ref --format=%(refname:short) refs/tags`, one per
/// line, in Git's order.
#[must_use]
pub fn tag_names(raw: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(raw)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The full id the diff starts from: `from_oid` for `Direct`, the merge base of
/// the two for `SinceCommonAncestor`. `from` and `to` are the revisions as the
/// caller named them, for the message when the two share no history.
pub fn base_of(
    mode: RangeMode,
    from: &str,
    to: &str,
    from_oid: &str,
    merge_base: Option<String>,
) -> Result<String, GitError> {
    match mode {
        RangeMode::Direct => Ok(from_oid.to_owned()),
        RangeMode::SinceCommonAncestor => {
            merge_base.ok_or_else(|| GitError(no_common_ancestor(from, to)))
        }
    }
}

/// The merge base Git printed. Git prints nothing, and exits 1, when the two
/// share no history: that is `None`.
pub fn merge_base_from(raw: &[u8]) -> Result<Option<String>, GitError> {
    if String::from_utf8_lossy(raw).trim().is_empty() {
        return Ok(None);
    }
    one_id(raw).map(Some)
}

/// The one id Git printed, without its line end.
pub fn one_id(raw: &[u8]) -> Result<String, GitError> {
    let text = String::from_utf8_lossy(raw);
    let id = text.trim();
    if id.is_empty() || id.contains(char::is_whitespace) {
        return Err(GitError(format!("git printed no single id: {text:?}")));
    }
    Ok(id.to_owned())
}

/// The file list of a range and its totals.
#[derive(Debug, PartialEq, Eq)]
pub struct Listing {
    pub files: Vec<CommitFile>,
    pub more_files: usize,
    pub added: u32,
    pub deleted: u32,
}

/// Totals every changed file in `all`, in Git's order, then keeps the first
/// `DETAIL_FILE_LIMIT` of them and counts the rest in `more_files`. The totals
/// cover the files past the limit too.
#[must_use]
pub fn listing(mut all: Vec<CommitFile>) -> Listing {
    let added = all
        .iter()
        .fold(0_u32, |sum, file| sum.saturating_add(file.added));
    let deleted = all
        .iter()
        .fold(0_u32, |sum, file| sum.saturating_add(file.deleted));
    let more_files = all.len().saturating_sub(DETAIL_FILE_LIMIT);
    all.truncate(DETAIL_FILE_LIMIT);
    Listing {
        files: all,
        more_files,
        added,
        deleted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::Change;

    fn file(path: &str, added: u32, deleted: u32) -> CommitFile {
        CommitFile {
            path: path.into(),
            old_path: None,
            change: Change::Modified,
            added,
            deleted,
            binary: false,
        }
    }

    #[test]
    fn a_revision_that_is_empty_or_starts_with_a_dash_is_refused_by_name() {
        assert_eq!(
            check_revision("from", "").unwrap_err().0,
            "from: the revision is empty"
        );
        assert_eq!(
            check_revision("to", "   ").unwrap_err().0,
            "to: the revision is empty"
        );
        assert_eq!(
            check_revision("to", "--output=x").unwrap_err().0,
            "to: \"--output=x\" is not a revision"
        );
    }

    #[test]
    fn an_expression_git_resolves_is_accepted() {
        for rev in ["main", "HEAD~3", "v1.2^", "origin/main", "a1b2c3d"] {
            assert!(check_revision("from", rev).is_ok(), "{rev}");
        }
    }

    #[test]
    fn direct_starts_at_the_from_tree() {
        let base = base_of(RangeMode::Direct, "v1", "main", "aaa", Some("bbb".into()));
        assert_eq!(base.unwrap(), "aaa");
    }

    #[test]
    fn since_common_ancestor_starts_at_the_merge_base() {
        let base = base_of(
            RangeMode::SinceCommonAncestor,
            "v1",
            "main",
            "aaa",
            Some("bbb".into()),
        );
        assert_eq!(base.unwrap(), "bbb");
    }

    #[test]
    fn since_common_ancestor_without_a_merge_base_names_both_revisions() {
        let error = base_of(RangeMode::SinceCommonAncestor, "left", "right", "aaa", None);
        assert_eq!(
            error.unwrap_err().0,
            "left and right have no common ancestor"
        );
    }

    #[test]
    fn an_empty_merge_base_output_means_no_common_ancestor() {
        assert_eq!(merge_base_from(b"").unwrap(), None);
        assert_eq!(merge_base_from(b"\n").unwrap(), None);
        assert_eq!(
            merge_base_from(b"0123456789abcdef0123456789abcdef01234567\n")
                .unwrap()
                .as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
    }

    #[test]
    fn tag_names_keep_each_line_in_order_and_drop_blank_ones() {
        assert_eq!(
            tag_names(b"v2.0\nv1.0\n\nrelease-candidate\n"),
            ["v2.0", "v1.0", "release-candidate"]
        );
        assert!(tag_names(b"").is_empty());
    }

    #[test]
    fn one_id_takes_the_single_line_git_printed() {
        assert_eq!(one_id(b"abc123\n").unwrap(), "abc123");
        assert!(one_id(b"").is_err());
        assert!(one_id(b"abc 123\n").is_err());
    }

    #[test]
    fn listing_totals_every_file_and_keeps_the_first_limit() {
        let all: Vec<CommitFile> = (0..DETAIL_FILE_LIMIT + 3)
            .map(|i| file(&format!("f{i}"), 2, 1))
            .collect();
        let listed = listing(all);
        assert_eq!(listed.files.len(), DETAIL_FILE_LIMIT);
        assert_eq!(listed.files[0].path, "f0");
        assert_eq!(listed.more_files, 3);
        let limit = u32::try_from(DETAIL_FILE_LIMIT).unwrap();
        assert_eq!(listed.added, 2 * (limit + 3));
        assert_eq!(listed.deleted, limit + 3);
    }

    #[test]
    fn listing_of_a_short_list_counts_no_more_files() {
        let listed = listing(vec![file("a", 1, 0), file("b", 0, 4)]);
        assert_eq!(listed.files.len(), 2);
        assert_eq!(listed.more_files, 0);
        assert_eq!((listed.added, listed.deleted), (1, 4));
    }

    #[test]
    fn listing_of_nothing_is_empty() {
        assert_eq!(
            listing(Vec::new()),
            Listing {
                files: Vec::new(),
                more_files: 0,
                added: 0,
                deleted: 0,
            }
        );
    }
}
