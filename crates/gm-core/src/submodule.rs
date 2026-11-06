//! Submodules (D175): the check of a submodule path, the `<sub>` field of
//! `git status`, the gitlink of the index and the log of a submodule's new
//! commits. Pure; the Git runs are in `git.rs`.

use std::path::{Component, Path};

use gm_schema::{SubmoduleChange, SubmoduleCommit};

use crate::GitError;

/// The `--format` of the log of a submodule's new commits, read with `-z`: the
/// commit id, the subject, the author name and the author time in Unix seconds.
pub const LOG_FORMAT: &str = "--format=%H%x1f%s%x1f%an%x1f%at";

/// Refuses a path that is not a plain path below the root: an empty one, an
/// absolute one, or one with `..`, `.` or a prefix. The reason is the text the
/// operation fails with.
pub fn check_path(path: &str) -> Result<(), String> {
    let plain = !path.is_empty()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if plain {
        Ok(())
    } else {
        Err(format!("{path:?} is not a path inside the repository"))
    }
}

/// The `<sub>` field of a status entry: `N...` for a file, or `S<c><m><u>` for
/// a submodule (`c` is `C` when the commit changed, `m` is `M` when tracked
/// content changed, `u` is `U` when there are untracked files, `.` otherwise).
pub fn parse_change(field: &str) -> Option<SubmoduleChange> {
    let bytes = field.as_bytes();
    if bytes.len() != 4 || bytes[0] != b'S' {
        return None;
    }
    Some(SubmoduleChange {
        commit_changed: bytes[1] == b'C',
        modified: bytes[2] == b'M',
        untracked: bytes[3] == b'U',
    })
}

/// The commit the index records for the gitlink at `path`, from the output of
/// `git ls-files -s -z -- <path>`. `None` when the index holds no gitlink there.
pub fn recorded_commit(raw: &[u8], path: &str) -> Option<String> {
    raw.split(|b| *b == 0).find_map(|record| {
        let record = String::from_utf8_lossy(record);
        let (meta, name) = record.split_once('\t')?;
        let mut fields = meta.split(' ');
        let (mode, oid) = (fields.next()?, fields.next()?);
        (mode == "160000" && name == path).then(|| oid.to_owned())
    })
}

/// Parses the output of `git log` with `LOG_FORMAT` and `-z`, newest first.
/// `raw` holds up to `limit + 1` commits; the answer keeps `limit` of them and
/// says whether more followed.
pub fn parse_log(raw: &[u8], limit: usize) -> Result<(Vec<SubmoduleCommit>, bool), GitError> {
    let mut commits = String::from_utf8_lossy(raw)
        .split('\0')
        .map(|record| record.trim_start_matches('\n'))
        .filter(|record| !record.is_empty())
        .map(parse_record)
        .collect::<Result<Vec<_>, _>>()?;
    let more = commits.len() > limit;
    commits.truncate(limit);
    Ok((commits, more))
}

fn parse_record(record: &str) -> Result<SubmoduleCommit, GitError> {
    let malformed = || GitError(format!("unexpected submodule log record: {record:?}"));
    let mut fields = record.split('\u{1f}');
    let oid = fields.next().ok_or_else(malformed)?;
    let subject = fields.next().ok_or_else(malformed)?;
    let author_name = fields.next().ok_or_else(malformed)?;
    let author_time = fields
        .next()
        .and_then(|t| t.parse().ok())
        .ok_or_else(malformed)?;
    Ok(SubmoduleCommit {
        oid: oid.to_owned(),
        subject: subject.to_owned(),
        author_name: author_name.to_owned(),
        author_time,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_submodule_field_gives_its_three_flags() {
        assert_eq!(
            parse_change("S.MU"),
            Some(SubmoduleChange {
                commit_changed: false,
                modified: true,
                untracked: true,
            })
        );
        assert_eq!(
            parse_change("SC.."),
            Some(SubmoduleChange {
                commit_changed: true,
                modified: false,
                untracked: false,
            })
        );
        assert_eq!(
            parse_change("S..U"),
            Some(SubmoduleChange {
                commit_changed: false,
                modified: false,
                untracked: true,
            })
        );
    }

    #[test]
    fn a_file_field_is_not_a_submodule() {
        assert_eq!(parse_change("N..."), None);
        assert_eq!(parse_change("S"), None);
    }

    #[test]
    fn only_a_plain_relative_path_is_checked() {
        assert!(check_path("sub").is_ok());
        assert!(check_path("libs/sub").is_ok());
        assert!(check_path("").is_err());
        assert!(check_path("/etc").is_err());
        assert!(check_path("../sub").is_err());
        assert!(check_path("libs/../../sub").is_err());
        assert!(check_path("./sub").is_err());
    }

    #[test]
    fn the_gitlink_of_the_index_gives_the_recorded_commit() {
        let raw = b"160000 1111111111111111111111111111111111111111 0\tsub\x00100644 2222222222222222222222222222222222222222 0\tfile.txt\x00";
        assert_eq!(
            recorded_commit(raw, "sub").as_deref(),
            Some("1111111111111111111111111111111111111111")
        );
        assert_eq!(recorded_commit(raw, "file.txt"), None);
        assert_eq!(recorded_commit(raw, "missing"), None);
    }

    #[test]
    fn the_log_keeps_the_limit_and_says_when_more_follow() {
        let raw = b"aaa\x1ffirst\x1fAna\x1f1700000000\0bbb\x1fsecond\x1fBo\x1f1700000100\0ccc\x1fthird\x1fCy\x1f1700000200\0";
        let (commits, more) = parse_log(raw, 2).unwrap();
        assert!(more);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].oid, "aaa");
        assert_eq!(commits[0].subject, "first");
        assert_eq!(commits[0].author_name, "Ana");
        assert_eq!(commits[0].author_time, 1_700_000_000);
        let (all, more) = parse_log(raw, 3).unwrap();
        assert!(!more);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn an_empty_log_is_no_commits() {
        assert_eq!(parse_log(b"", 200).unwrap(), (Vec::new(), false));
    }

    #[test]
    fn a_malformed_log_record_is_an_error() {
        assert!(parse_log(b"aaa\x1ffirst\0", 200).is_err());
    }
}
