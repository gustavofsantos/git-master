//! Stashes (B2, D94): the parse of `git stash list` and the branch a stash was
//! made on. Pure; the Git runs are in `git.rs`.

use gm_schema::StashEntry;

use crate::GitError;

/// The field separator of `LIST_FORMAT`: a unit separator, which no reflog
/// subject holds in practice. Fields are read from both ends, so one that does
/// hold it still parses.
const FIELD: char = '\u{1f}';

/// The `--format` of `git stash list`: commit id, `stash@{n}`, the reflog
/// subject and the committer time, one stash a line.
pub const LIST_FORMAT: &str = "--format=%H%x1f%gd%x1f%gs%x1f%ct";

/// Parses the output of `git stash list` with `LIST_FORMAT`, newest first.
/// An empty output is no stashes.
pub fn parse_list(raw: &[u8]) -> Result<Vec<StashEntry>, GitError> {
    String::from_utf8_lossy(raw)
        .lines()
        .filter(|line| !line.is_empty())
        .map(parse_line)
        .collect()
}

fn parse_line(line: &str) -> Result<StashEntry, GitError> {
    let malformed = || GitError(format!("unexpected stash list line: {line:?}"));
    let (oid, rest) = line.split_once(FIELD).ok_or_else(malformed)?;
    let (name, rest) = rest.split_once(FIELD).ok_or_else(malformed)?;
    let (message, time) = rest.rsplit_once(FIELD).ok_or_else(malformed)?;
    let index = name
        .strip_prefix("stash@{")
        .and_then(|digits| digits.strip_suffix('}'))
        .and_then(|digits| digits.parse().ok())
        .ok_or_else(malformed)?;
    let time = time.parse().map_err(|_| malformed())?;
    Ok(StashEntry {
        index,
        oid: oid.to_owned(),
        message: message.to_owned(),
        branch: branch_of(message),
        time,
    })
}

/// The branch a stash was made on, from its subject: `On <branch>: <message>`
/// for a message given with `-m`, or `WIP on <branch>: <short id> <subject>` by
/// default. A branch name cannot hold a colon, so the first `: ` ends it, even
/// when the message after it holds more. `None` for a detached HEAD
/// (`(no branch)`) and for any other subject.
pub fn branch_of(message: &str) -> Option<String> {
    let rest = message
        .strip_prefix("WIP on ")
        .or_else(|| message.strip_prefix("On "))?;
    let (branch, _) = rest.split_once(": ")?;
    (!branch.is_empty() && branch != "(no branch)").then(|| branch.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(oid: &str, n: u32, subject: &str, time: i64) -> String {
        format!("{oid}{FIELD}stash@{{{n}}}{FIELD}{subject}{FIELD}{time}\n")
    }

    #[test]
    fn no_output_is_no_stashes() {
        assert!(parse_list(b"").unwrap().is_empty());
    }

    #[test]
    fn fields_are_read_in_order_newest_first() {
        let raw = format!(
            "{}{}",
            line(
                &"a".repeat(40),
                0,
                "WIP on main: abc1234 init",
                1_700_000_200
            ),
            line(&"b".repeat(40), 1, "On main: my note", 1_700_000_100),
        );
        let stashes = parse_list(raw.as_bytes()).unwrap();
        assert_eq!(stashes.len(), 2);
        assert_eq!(stashes[0].index, 0);
        assert_eq!(stashes[0].oid, "a".repeat(40));
        assert_eq!(stashes[0].message, "WIP on main: abc1234 init");
        assert_eq!(stashes[0].branch.as_deref(), Some("main"));
        assert_eq!(stashes[0].time, 1_700_000_200);
        assert_eq!(stashes[1].index, 1);
        assert_eq!(stashes[1].message, "On main: my note");
        assert_eq!(stashes[1].branch.as_deref(), Some("main"));
    }

    #[test]
    fn a_message_with_colons_and_unicode_is_kept_whole() {
        let subject = "On feature/ñandú: fix: the “quote” — 日本 ✓";
        let stashes = parse_list(line(&"c".repeat(40), 0, subject, 5).as_bytes()).unwrap();
        assert_eq!(stashes[0].message, subject);
        assert_eq!(stashes[0].branch.as_deref(), Some("feature/ñandú"));
    }

    #[test]
    fn a_detached_stash_has_no_branch() {
        let raw = format!(
            "{}{}",
            line(
                &"d".repeat(40),
                0,
                "WIP on (no branch): abc1234 detached",
                7
            ),
            line(&"e".repeat(40), 1, "On (no branch): by hand", 6),
        );
        let stashes = parse_list(raw.as_bytes()).unwrap();
        assert_eq!(stashes[0].branch, None);
        assert_eq!(stashes[0].message, "WIP on (no branch): abc1234 detached");
        assert_eq!(stashes[1].branch, None);
    }

    #[test]
    fn a_field_separator_inside_the_subject_does_not_split_it() {
        let subject = format!("On main: a{FIELD}b");
        let stashes = parse_list(line(&"f".repeat(40), 3, &subject, 9).as_bytes()).unwrap();
        assert_eq!(stashes[0].index, 3);
        assert_eq!(stashes[0].message, subject);
        assert_eq!(stashes[0].time, 9);
    }

    #[test]
    fn a_line_that_does_not_parse_is_an_error_not_a_panic() {
        assert!(parse_list(b"not a stash line\n").is_err());
        let no_index = format!("{}stash{FIELD}x{FIELD}1\n", "a".repeat(40));
        assert!(parse_list(no_index.as_bytes()).is_err());
        let no_time = format!(
            "{}{FIELD}stash@{{0}}{FIELD}On main: x{FIELD}soon\n",
            "a".repeat(40)
        );
        assert!(parse_list(no_time.as_bytes()).is_err());
    }

    #[test]
    fn the_branch_comes_from_the_subject_only_when_it_parses() {
        assert_eq!(branch_of("On main: note").as_deref(), Some("main"));
        assert_eq!(
            branch_of("WIP on release/1.2: abc1234 subject: with colon").as_deref(),
            Some("release/1.2")
        );
        assert_eq!(branch_of("WIP on (no branch): abc subject"), None);
        assert_eq!(branch_of("On (no branch): note"), None);
        assert_eq!(branch_of("On : empty branch"), None);
        assert_eq!(branch_of("On main without a colon"), None);
        assert_eq!(branch_of("just a message"), None);
    }
}
