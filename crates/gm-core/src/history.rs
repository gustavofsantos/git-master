//! The history of HEAD: the output of `git log` parsed into commits, and the
//! walk read page by page. The parsing is pure and reads any `BufRead`; the
//! adapter starts git and owns the process.

use std::io::BufRead;
use std::sync::Arc;

use gm_schema::{CommitSummary, HISTORY_PAGE, OperationId, RefKind, RefLabel};

use crate::GitError;

/// Stops a walk from another thread: kills the git process behind it, so a
/// read in progress ends. Callable from any thread.
pub type Canceller = Arc<dyn Fn() + Send + Sync>;

/// One page of the commits of HEAD, in walk order (newest first).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryPage {
    pub commits: Vec<CommitSummary>,
    /// No commit follows this page.
    pub done: bool,
    /// HEAD has no commits yet. The page is then empty and done.
    pub unborn: bool,
}

/// The walk of HEAD, read a page at a time. Between pages git waits on a
/// full pipe, so it does no work the list has not asked for (D32).
pub trait HistoryStream: Send {
    /// The next commits, at most `limit`.
    fn next_page(&mut self, limit: usize) -> Result<HistoryPage, GitError>;

    /// Reads past `oid`, which is dropped with the commits before it. Keyset
    /// resume: no `--skip`. Fails when the walk ends without `oid`.
    fn skip_past(&mut self, oid: &str) -> Result<(), GitError>;

    /// Stops the walk from another thread; a read in progress then fails.
    fn canceller(&self) -> Canceller;
}

/// Reads up to `limit` commits. Returns them and whether the walk ended.
pub fn read_page<R: BufRead>(
    input: &mut R,
    limit: usize,
) -> Result<(Vec<CommitSummary>, bool), GitError> {
    let mut commits = Vec::new();
    while commits.len() < limit {
        match read_record(input)? {
            Some(commit) => commits.push(commit),
            None => return Ok((commits, true)),
        }
    }
    Ok((commits, at_end(input)?))
}

/// Reads commits until `oid` has been read. Returns whether it was found.
pub fn skip_past<R: BufRead>(input: &mut R, oid: &str) -> Result<bool, GitError> {
    while let Some(commit) = read_record(input)? {
        if commit.oid == oid {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The next record, or `None` at the end of the output. Git ends the output
/// with a newline after the last record, which is not a record.
fn read_record<R: BufRead>(input: &mut R) -> Result<Option<CommitSummary>, GitError> {
    loop {
        let mut raw = Vec::new();
        let read = input
            .read_until(RECORD_END as u8, &mut raw)
            .map_err(|e| GitError(format!("cannot read git's output: {e}")))?;
        if read == 0 {
            return Ok(None);
        }
        if raw.last() != Some(&(RECORD_END as u8)) {
            if raw.iter().all(u8::is_ascii_whitespace) {
                return Ok(None);
            }
            return Err(GitError(
                "history output ends in the middle of a commit".into(),
            ));
        }
        raw.pop();
        let text = String::from_utf8_lossy(&raw);
        let record = text.trim_start_matches('\n');
        if record.is_empty() {
            continue;
        }
        return parse_record(record).map(Some);
    }
}

/// Whether the walk has no more commits. Waits for git's next byte, and
/// consumes the whitespace that separates records.
fn at_end<R: BufRead>(input: &mut R) -> Result<bool, GitError> {
    loop {
        let buffered = input
            .fill_buf()
            .map_err(|e| GitError(format!("cannot read git's output: {e}")))?;
        if buffered.is_empty() {
            return Ok(true);
        }
        if buffered.iter().any(|byte| !byte.is_ascii_whitespace()) {
            return Ok(false);
        }
        let len = buffered.len();
        input.consume(len);
    }
}

/// One request for a page, run on a thread of its own. The walk comes back in
/// the report, so the engine can read the next page from it.
pub struct HistoryJob {
    pub id: OperationId,
    /// `None` when HEAD is unborn: there is no walk to read.
    pub stream: Option<Box<dyn HistoryStream>>,
    /// Read past this commit first. `None` continues where the stream stands.
    pub after: Option<String>,
}

/// What a job answers. `stream` is `None` only for an unborn HEAD.
pub enum HistoryOutcome {
    Page {
        stream: Option<Box<dyn HistoryStream>>,
        page: HistoryPage,
    },
    Failed(String),
}

pub struct HistoryReport {
    pub id: OperationId,
    pub outcome: HistoryOutcome,
}

impl HistoryJob {
    /// Reads one page. A walk stopped by its canceller fails here, and its
    /// report is dropped by the engine, which asked for it to stop.
    #[must_use]
    pub fn run(self) -> HistoryReport {
        let outcome = match self.stream {
            None => HistoryOutcome::Page {
                stream: None,
                page: HistoryPage {
                    commits: Vec::new(),
                    done: true,
                    unborn: true,
                },
            },
            Some(stream) => read_job(stream, self.after.as_deref()),
        };
        HistoryReport {
            id: self.id,
            outcome,
        }
    }
}

fn read_job(mut stream: Box<dyn HistoryStream>, after: Option<&str>) -> HistoryOutcome {
    let page = match after {
        Some(oid) => stream.skip_past(oid),
        None => Ok(()),
    }
    .and_then(|()| stream.next_page(HISTORY_PAGE));
    match page {
        Ok(page) => HistoryOutcome::Page {
            stream: Some(stream),
            page,
        },
        Err(error) => HistoryOutcome::Failed(error.0),
    }
}

/// The format `git log` is asked for: six fields per commit, separated by
/// the unit separator (0x1f), and each commit ended by the record separator
/// (0x1e). Git writes a newline after each record, which the parser drops.
/// `%P` lists the parents, separated by spaces.
pub const FORMAT: &str = "--format=%H%x1f%P%x1f%h%x1f%at%x1f%D%x1f%s%x1e";

const FIELD_SEP: char = '\u{1f}';
const RECORD_END: char = '\u{1e}';
const FIELDS: usize = 6;

fn parse_record(record: &str) -> Result<CommitSummary, GitError> {
    let fields: Vec<&str> = record.splitn(FIELDS, FIELD_SEP).collect();
    let [oid, parents, short, time, decoration, subject] = fields[..] else {
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
        parents: parents.split_whitespace().map(str::to_owned).collect(),
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

    /// Every commit of `raw`, read in one page.
    fn parse_log(raw: &[u8]) -> Result<Vec<CommitSummary>, GitError> {
        read_page(&mut std::io::Cursor::new(raw), usize::MAX).map(|(commits, _)| commits)
    }

    fn record(fields: &[&str]) -> String {
        format!("\n{}{RECORD_END}", fields.join(&FIELD_SEP.to_string()))
    }

    #[test]
    fn parses_each_record_in_order() {
        let raw = [
            record(&["aaa", "bbb", "aa", "100", "HEAD -> main", "second"]),
            record(&["bbb", "", "bb", "90", "", "first"]),
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
    fn parents_come_first_parent_first_and_a_root_has_none() {
        let raw = [
            record(&["mmm", "aaa bbb ccc", "mm", "3", "", "octopus"]),
            record(&["aaa", "rrr", "aa", "2", "", "child"]),
            record(&["rrr", "", "rr", "1", "", "root"]),
        ]
        .concat();

        let commits = parse_log(raw.as_bytes()).unwrap();

        assert_eq!(commits[0].parents, ["aaa", "bbb", "ccc"]);
        assert_eq!(commits[1].parents, ["rrr"]);
        assert!(commits[2].parents.is_empty());
    }

    #[test]
    fn a_trailing_newline_is_not_a_truncation() {
        let raw = format!("{}\n", record(&["aaa", "", "aa", "1", "", "x"]));
        assert_eq!(parse_log(raw.as_bytes()).unwrap().len(), 1);
    }

    #[test]
    fn empty_output_is_no_commits() {
        assert!(parse_log("".as_bytes()).unwrap().is_empty());
    }

    #[test]
    fn a_subject_may_hold_the_field_separator() {
        let raw = record(&["aaa", "", "aa", "1", "", "a\u{1f}b"]);
        let commits = parse_log(raw.as_bytes()).unwrap();
        assert_eq!(commits[0].subject, "a\u{1f}b");
    }

    #[test]
    fn a_truncated_record_is_an_error_not_a_panic() {
        // No record terminator: the output was cut.
        assert!(parse_log("\naaa\u{1f}\u{1f}aa\u{1f}1".as_bytes()).is_err());
        assert!(parse_log("\naaa\u{1f}\u{1f}aa\u{1f}1\u{1f}\u{1f}subject".as_bytes()).is_err());
        // Terminated, but missing the subject field.
        assert!(parse_log("\naaa\u{1f}\u{1f}aa\u{1f}1\u{1f}\u{1e}".as_bytes()).is_err());
    }

    #[test]
    fn a_bad_time_is_an_error() {
        let raw = record(&["aaa", "", "aa", "soon", "", "x"]);
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
