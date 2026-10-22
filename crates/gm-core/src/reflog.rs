//! The reflog of HEAD (B3, D95): the parse of `git reflog show` and the check of
//! a commit id before a detached checkout. Pure; the Git runs are in `git.rs`.

use gm_schema::ReflogEntry;

use crate::GitError;

/// The field separator of `LIST_FORMAT`: a unit separator, which no reflog
/// message holds in practice.
const FIELD: char = '\u{1f}';

/// The `--format` of `git reflog show`, read with `-z` and `--date=unix`: the
/// commit id, `HEAD@{<unix time of the entry>}`, the reflog message and the
/// commit subject. The time is the entry's, not the commit's (`%ct` would be the
/// commit's), and the index is the position after `skip`.
pub const LIST_FORMAT: &str = "--format=%H%x1f%gd%x1f%gs%x1f%s";

/// Parses the output of `git reflog show` with `LIST_FORMAT` and `-z`, newest
/// first, for the entries that start after `skip`. An empty output is no entries.
pub fn parse_list(raw: &[u8], skip: u32) -> Result<Vec<ReflogEntry>, GitError> {
    String::from_utf8_lossy(raw)
        .split('\0')
        .filter(|record| !record.is_empty())
        .zip(0u32..)
        .map(|(record, n)| parse_record(record, skip.saturating_add(n)))
        .collect()
}

fn parse_record(record: &str, index: u32) -> Result<ReflogEntry, GitError> {
    let malformed = || GitError(format!("unexpected reflog record: {record:?}"));
    let (oid, rest) = record.split_once(FIELD).ok_or_else(malformed)?;
    let (selector, rest) = rest.split_once(FIELD).ok_or_else(malformed)?;
    // The subject is read from the end, so a message keeps any separator it holds.
    // A commit subject that holds one is cut after its last separator.
    let (message, subject) = rest.rsplit_once(FIELD).ok_or_else(malformed)?;
    let time = selector
        .strip_prefix("HEAD@{")
        .and_then(|digits| digits.strip_suffix('}'))
        .and_then(|digits| digits.parse().ok())
        .ok_or_else(malformed)?;
    let (action, detail) = split_message(message);
    Ok(ReflogEntry {
        index,
        oid: oid.to_owned(),
        action: action.to_owned(),
        detail: detail.to_owned(),
        subject: (!subject.is_empty()).then(|| subject.to_owned()),
        time,
    })
}

/// Splits a reflog message at its first `: `: `commit (amend): fix` gives the
/// action `commit (amend)` and the detail `fix`. A message without `: ` is all
/// action, and its detail is empty.
pub fn split_message(message: &str) -> (&str, &str) {
    message.split_once(": ").unwrap_or((message, ""))
}

/// The refusal of a commit id that is not 7 to 64 hex characters, checked before
/// Git runs. `None` when the id is well formed.
pub fn oid_problem(oid: &str) -> Option<&'static str> {
    let well_formed = (7..=64).contains(&oid.len()) && oid.bytes().all(|b| b.is_ascii_hexdigit());
    (!well_formed).then_some("Choose a commit to check out: its id is 7 to 64 hex characters.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(oid: &str, n: u32, message: &str, subject: &str) -> String {
        format!(
            "{oid}{FIELD}HEAD@{{{}}}{FIELD}{message}{FIELD}{subject}\0",
            1_700_000_000 + i64::from(n)
        )
    }

    #[test]
    fn no_output_is_no_entries() {
        assert!(parse_list(b"", 0).unwrap().is_empty());
    }

    #[test]
    fn normal_actions_are_split_at_the_first_colon_space() {
        let raw = [
            record(&"a".repeat(40), 0, "commit: second", "second"),
            record(
                &"b".repeat(40),
                1,
                "checkout: moving from main to topic",
                "first",
            ),
            record(&"c".repeat(40), 2, "reset: moving to HEAD~1", "first"),
            record(&"d".repeat(40), 3, "merge main: Fast-forward", "main tip"),
            record(&"e".repeat(40), 4, "pull: Fast-forward", "pulled"),
        ]
        .concat();
        let entries = parse_list(raw.as_bytes(), 0).unwrap();
        let pairs: Vec<(&str, &str)> = entries
            .iter()
            .map(|e| (e.action.as_str(), e.detail.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("commit", "second"),
                ("checkout", "moving from main to topic"),
                ("reset", "moving to HEAD~1"),
                ("merge main", "Fast-forward"),
                ("pull", "Fast-forward"),
            ]
        );
        assert_eq!(entries[0].oid, "a".repeat(40));
        assert_eq!(entries[0].subject.as_deref(), Some("second"));
        assert_eq!(entries[4].time, 1_700_000_004);
    }

    #[test]
    fn an_amend_and_a_rebase_keep_their_parenthesised_action() {
        let raw = [
            record(&"a".repeat(40), 0, "commit (amend): tidy", "tidy"),
            record(
                &"b".repeat(40),
                1,
                "rebase (finish): refs/heads/topic onto abc1234",
                "work",
            ),
        ]
        .concat();
        let entries = parse_list(raw.as_bytes(), 0).unwrap();
        assert_eq!(entries[0].action, "commit (amend)");
        assert_eq!(entries[0].detail, "tidy");
        assert_eq!(entries[1].action, "rebase (finish)");
        assert_eq!(entries[1].detail, "refs/heads/topic onto abc1234");
    }

    #[test]
    fn a_message_without_a_colon_space_is_all_action() {
        assert_eq!(split_message("commit (initial)"), ("commit (initial)", ""));
        assert_eq!(split_message("pull"), ("pull", ""));
        assert_eq!(split_message("commit:x"), ("commit:x", ""));
        assert_eq!(split_message(""), ("", ""));
    }

    #[test]
    fn only_the_first_colon_space_splits_and_the_detail_keeps_the_rest() {
        assert_eq!(split_message("commit: fix: a: b"), ("commit", "fix: a: b"));
        assert_eq!(split_message("commit: "), ("commit", ""));
    }

    #[test]
    fn a_unicode_detail_is_kept_whole() {
        let subject = "commit: ünï ✓ — 日本 “quotes”";
        let raw = record(&"f".repeat(40), 0, subject, "ünï ✓ — 日本 “quotes”");
        let entries = parse_list(raw.as_bytes(), 0).unwrap();
        assert_eq!(entries[0].action, "commit");
        assert_eq!(entries[0].detail, "ünï ✓ — 日本 “quotes”");
        assert_eq!(entries[0].subject.as_deref(), Some("ünï ✓ — 日本 “quotes”"));
    }

    #[test]
    fn an_empty_subject_is_none() {
        let raw = record(&"a".repeat(40), 0, "commit (empty)", "");
        let entries = parse_list(raw.as_bytes(), 0).unwrap();
        assert_eq!(entries[0].subject, None);
    }

    #[test]
    fn a_field_separator_inside_the_message_does_not_split_it() {
        let message = format!("commit: a{FIELD}b");
        let raw = record(&"a".repeat(40), 0, &message, "s");
        let entries = parse_list(raw.as_bytes(), 0).unwrap();
        assert_eq!(entries[0].detail, format!("a{FIELD}b"));
        assert_eq!(entries[0].subject.as_deref(), Some("s"));
    }

    #[test]
    fn the_index_counts_from_skip() {
        let raw = [
            record(&"a".repeat(40), 0, "commit: x", "x"),
            record(&"b".repeat(40), 1, "commit: y", "y"),
        ]
        .concat();
        let entries = parse_list(raw.as_bytes(), 200).unwrap();
        assert_eq!(entries[0].index, 200);
        assert_eq!(entries[1].index, 201);
    }

    #[test]
    fn a_record_that_does_not_parse_is_an_error_not_a_panic() {
        assert!(parse_list(b"not a record\0", 0).is_err());
        let no_time = format!(
            "{}{FIELD}HEAD@{{soon}}{FIELD}commit: x{FIELD}x\0",
            "a".repeat(40)
        );
        assert!(parse_list(no_time.as_bytes(), 0).is_err());
        let no_selector = format!("{}{FIELD}commit: x{FIELD}x\0", "a".repeat(40));
        assert!(parse_list(no_selector.as_bytes(), 0).is_err());
    }

    #[test]
    fn an_oid_of_7_to_64_hex_characters_is_well_formed() {
        assert_eq!(oid_problem(&"a".repeat(7)), None);
        assert_eq!(oid_problem(&"0".repeat(40)), None);
        assert_eq!(oid_problem(&"F".repeat(64)), None);
        assert!(oid_problem(&"a".repeat(6)).is_some());
        assert!(oid_problem(&"a".repeat(65)).is_some());
        assert!(oid_problem("").is_some());
        assert!(oid_problem("--detach").is_some());
        assert!(oid_problem("main").is_some());
        assert!(oid_problem("abc123g").is_some());
        assert!(oid_problem("HEAD~1").is_some());
        assert!(oid_problem("abc 1234").is_some());
    }
}
