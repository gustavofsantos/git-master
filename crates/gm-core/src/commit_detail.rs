//! The detail of one commit: the output of `git show` parsed into a header and
//! a list of files. Both parsers are pure; the adapter starts git. A detail read
//! runs as a job on a thread of its own, and its answer is dropped when a newer
//! request superseded it (H3).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gm_schema::{Change, CommitDetail, CommitFile, DETAIL_FILE_LIMIT, OperationId};

use crate::port::{DetailRead, GitError};

/// The format of the header read: eight fields separated by the unit separator
/// (0x1f), and the record ended by the record separator (0x1e). The body is
/// the last field, so a body with separators in it still parses whole.
pub const HEADER_FORMAT: &str = "--format=%H%x1f%h%x1f%p%x1f%an%x1f%ae%x1f%ai%x1f%s%x1f%b%x1e";

const FIELD_SEP: char = '\u{1f}';
const RECORD_END: char = '\u{1e}';
const FIELDS: usize = 8;

/// One detail read, run on a thread of its own. `stop` is shared with the
/// engine, which sets it when the read is superseded.
pub struct DetailJob {
    pub id: OperationId,
    pub read: DetailRead,
    pub stop: Arc<AtomicBool>,
}

/// What a detail job answers. A failure carries git's own message.
pub struct DetailReport {
    pub id: OperationId,
    pub outcome: Result<CommitDetail, String>,
}

impl DetailJob {
    /// Runs the read to its end, or to the stop request.
    #[must_use]
    pub fn run(self) -> DetailReport {
        let outcome = (self.read)(&self.stop).map_err(|error| error.0);
        DetailReport {
            id: self.id,
            outcome,
        }
    }
}

/// Parses the header record of `git show -s --format=HEADER_FORMAT`. The
/// files are not part of it: the returned detail has none.
pub fn parse_header(raw: &[u8]) -> Result<CommitDetail, GitError> {
    let text = String::from_utf8_lossy(raw);
    let record = text
        .split(RECORD_END)
        .map(|record| record.trim_start_matches('\n'))
        .find(|record| !record.is_empty())
        .ok_or_else(|| GitError("git show printed no commit".into()))?;
    let fields: Vec<&str> = record.splitn(FIELDS, FIELD_SEP).collect();
    let [oid, short, parents, name, email, date, subject, body] = fields[..] else {
        return Err(GitError(format!("commit header has no fields: {record:?}")));
    };
    if oid.is_empty() || short.is_empty() {
        return Err(GitError(format!("commit header has no id: {record:?}")));
    }
    Ok(CommitDetail {
        oid: oid.to_owned(),
        short: short.to_owned(),
        subject: subject.to_owned(),
        body: body.trim_end_matches('\n').to_owned(),
        author_name: name.to_owned(),
        author_email: email.to_owned(),
        author_date: date.to_owned(),
        parents: parents.split_whitespace().map(str::to_owned).collect(),
        files: Vec::new(),
        more_files: 0,
    })
}

/// Parses `git show --raw -z --numstat --format= --diff-merges=first-parent -M`.
/// Git prints the raw entries first, then one numstat entry per file, in the
/// same order. Returns the first `DETAIL_FILE_LIMIT` files and how many more
/// there are.
pub fn parse_files(raw: &[u8]) -> Result<(Vec<CommitFile>, usize), GitError> {
    let text = String::from_utf8_lossy(raw);
    let mut fields: Vec<&str> = text.split('\0').collect();
    // Every entry ends with a NUL, so the split leaves an empty field at the end.
    if fields.last() == Some(&"") {
        fields.pop();
    }
    let mut changes = Vec::new();
    let mut at = 0;
    while at < fields.len() && fields[at].starts_with(':') {
        let status = fields[at].rsplit(' ').next().unwrap_or_default();
        let change = change_of(status.chars().next())?;
        let paired = matches!(change, Change::Renamed | Change::Copied);
        let width = if paired { 3 } else { 2 };
        let names = fields
            .get(at + 1..at + width)
            .ok_or_else(|| GitError("the file list ends in the middle of an entry".into()))?;
        let (old_path, path) = if paired {
            (Some(names[0].to_owned()), names[1].to_owned())
        } else {
            (None, names[0].to_owned())
        };
        changes.push((change, path, old_path));
        at += width;
    }
    // One numstat entry per file: `added TAB deleted TAB path`. A rename has an
    // empty path there, and its two names follow as fields of their own.
    let mut counts = Vec::new();
    while at < fields.len() {
        let mut parts = fields[at].splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(GitError(format!("bad numstat entry: {:?}", fields[at])));
        };
        counts.push((added, deleted));
        at += if path.is_empty() { 3 } else { 1 };
    }
    if counts.len() != changes.len() {
        return Err(GitError(format!(
            "git listed {} files and {} counts",
            changes.len(),
            counts.len()
        )));
    }
    let more = changes.len().saturating_sub(DETAIL_FILE_LIMIT);
    let files = changes
        .into_iter()
        .zip(counts)
        .take(DETAIL_FILE_LIMIT)
        .map(|((change, path, old_path), (added, deleted))| {
            let binary = added == "-";
            Ok(CommitFile {
                path,
                old_path,
                change,
                added: count(added, binary)?,
                deleted: count(deleted, binary)?,
                binary,
            })
        })
        .collect::<Result<Vec<_>, GitError>>()?;
    Ok((files, more))
}

fn change_of(letter: Option<char>) -> Result<Change, GitError> {
    Ok(match letter {
        Some('A') => Change::Added,
        Some('M') => Change::Modified,
        Some('D') => Change::Deleted,
        Some('R') => Change::Renamed,
        Some('C') => Change::Copied,
        Some('T') => Change::TypeChanged,
        other => {
            return Err(GitError(format!(
                "git reported an unknown change {other:?}"
            )));
        }
    })
}

/// A line count. Git prints `-` for a binary file, which has none.
fn count(text: &str, binary: bool) -> Result<u32, GitError> {
    if binary {
        return Ok(0);
    }
    text.parse()
        .map_err(|_| GitError(format!("git printed a bad line count: {text:?}")))
}
