//! Parses the output of `git log` for the history list. Pure: the adapter runs
//! git and passes the bytes here.

use gm_schema::{CommitSummary, RefKind, RefLabel};

use crate::GitError;

/// The format `git log` is asked for: five fields per commit, separated by
/// the unit separator (0x1f), and each commit ended by the record separator
/// (0x1e). Git writes a newline after each record, which the parser drops.
pub const FORMAT: &str = "--format=%H%x1f%h%x1f%at%x1f%D%x1f%s%x1e";

const FIELD_SEP: char = '\u{1f}';
const RECORD_END: char = '\u{1e}';
const FIELDS: usize = 5;

/// Parses the records of `FORMAT`, in the order git printed them.
pub fn parse_log(raw: &[u8]) -> Result<Vec<CommitSummary>, GitError> {
    let text = String::from_utf8_lossy(raw);
    let text = text.trim_end_matches('\n');
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let Some(records) = text.strip_suffix(RECORD_END) else {
        return Err(GitError(
            "history output ends in the middle of a commit".into(),
        ));
    };
    let mut commits = Vec::new();
    for record in records.split(RECORD_END) {
        let record = record.trim_start_matches('\n');
        if record.is_empty() {
            continue;
        }
        commits.push(parse_record(record)?);
    }
    Ok(commits)
}

fn parse_record(record: &str) -> Result<CommitSummary, GitError> {
    let fields: Vec<&str> = record.splitn(FIELDS, FIELD_SEP).collect();
    let [oid, short, time, decoration, subject] = fields[..] else {
        return Err(GitError(format!(
            "history record has no fields: {record:?}"
        )));
    };
    if oid.is_empty() || short.is_empty() {
        return Err(GitError(format!("history record has no id: {record:?}")));
    }
    let author_time = time
        .parse::<i64>()
        .map_err(|_| GitError(format!("history record has a bad time: {time:?}")))?;
    Ok(CommitSummary {
        oid: oid.to_owned(),
        short: short.to_owned(),
        subject: subject.to_owned(),
        refs: parse_decoration(decoration),
        author_time,
    })
}

/// Parses `%D`, e.g. `HEAD -> main, origin/main, tag: v1.0`. `HEAD` always
/// comes first, as a label of its own. `origin/HEAD` is not a branch to show.
pub fn parse_decoration(decoration: &str) -> Vec<RefLabel> {
    let mut head = Vec::new();
    let mut refs = Vec::new();
    for item in decoration.split(", ").filter(|item| !item.is_empty()) {
        if item == "HEAD" {
            head.push(label("HEAD", RefKind::Head));
        } else if let Some(branch) = item.strip_prefix("HEAD -> ") {
            head.push(label("HEAD", RefKind::Head));
            refs.push(label(branch, RefKind::Branch));
        } else if let Some(tag) = item.strip_prefix("tag: ") {
            refs.push(label(tag, RefKind::Tag));
        } else if !item.ends_with("/HEAD") {
            refs.push(label(item, RefKind::Branch));
        }
    }
    head.extend(refs);
    head
}

fn label(name: &str, kind: RefKind) -> RefLabel {
    RefLabel {
        name: name.to_owned(),
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(fields: &[&str]) -> String {
        format!("\n{}{RECORD_END}", fields.join(&FIELD_SEP.to_string()))
    }

    #[test]
    fn parses_each_record_in_order() {
        let raw = [
            record(&["aaa", "aa", "100", "HEAD -> main", "second"]),
            record(&["bbb", "bb", "90", "", "first"]),
        ]
        .concat();

        let commits = parse_log(raw.as_bytes()).unwrap();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "second");
        assert_eq!(commits[0].author_time, 100);
        assert_eq!(commits[1].oid, "bbb");
        assert!(commits[1].refs.is_empty());
    }

    #[test]
    fn a_trailing_newline_is_not_a_truncation() {
        let raw = format!("{}\n", record(&["aaa", "aa", "1", "", "x"]));
        assert_eq!(parse_log(raw.as_bytes()).unwrap().len(), 1);
    }

    #[test]
    fn empty_output_is_no_commits() {
        assert!(parse_log("".as_bytes()).unwrap().is_empty());
    }

    #[test]
    fn a_subject_may_hold_the_field_separator() {
        let raw = record(&["aaa", "aa", "1", "", "a\u{1f}b"]);
        let commits = parse_log(raw.as_bytes()).unwrap();
        assert_eq!(commits[0].subject, "a\u{1f}b");
    }

    #[test]
    fn a_truncated_record_is_an_error_not_a_panic() {
        // No record terminator: the output was cut.
        assert!(parse_log("\naaa\u{1f}aa\u{1f}1".as_bytes()).is_err());
        assert!(parse_log("\naaa\u{1f}aa\u{1f}1\u{1f}\u{1f}subject".as_bytes()).is_err());
        // Terminated, but missing the subject field.
        assert!(parse_log("\naaa\u{1f}aa\u{1f}1\u{1f}\u{1e}".as_bytes()).is_err());
    }

    #[test]
    fn a_bad_time_is_an_error() {
        let raw = record(&["aaa", "aa", "soon", "", "x"]);
        assert!(parse_log(raw.as_bytes()).is_err());
    }

    #[test]
    fn decoration_puts_head_first_and_names_its_branch() {
        let refs = parse_decoration("HEAD -> main, origin/main, tag: v1.0");
        assert_eq!(
            refs,
            [
                label("HEAD", RefKind::Head),
                label("main", RefKind::Branch),
                label("origin/main", RefKind::Branch),
                label("v1.0", RefKind::Tag),
            ]
        );
    }

    #[test]
    fn a_detached_head_is_one_head_label() {
        assert_eq!(parse_decoration("HEAD"), [label("HEAD", RefKind::Head)]);
    }

    #[test]
    fn origin_head_is_not_shown_and_empty_decoration_has_no_labels() {
        assert!(parse_decoration("").is_empty());
        assert_eq!(
            parse_decoration("origin/HEAD, feature/x"),
            [label("feature/x", RefKind::Branch)]
        );
    }

    #[test]
    fn a_branch_with_a_slash_keeps_its_whole_name() {
        assert_eq!(
            parse_decoration("feature/ledger/api"),
            [label("feature/ledger/api", RefKind::Branch)]
        );
    }
}
