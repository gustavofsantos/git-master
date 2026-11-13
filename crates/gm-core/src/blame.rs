//! The lines `git blame --line-porcelain` gives for a range (D144). Git writes every header
//! for every line, so each line carries the path Git reports for it, even when a copy
//! (`-C`) moved it from another file. Nothing here runs Git.

/// One line of the blamed range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    /// 1-based line number in the file as it is at `HEAD` of the blame.
    pub line: u32,
    /// Full id of the commit that last changed the line.
    pub commit: String,
    /// The path Git reports for the line in that commit.
    pub path: String,
    pub author: String,
    /// The author date, in milliseconds since the Unix epoch. It survives a rebase; the
    /// committer date does not (D144).
    pub author_at: i64,
    /// The subject of the commit.
    pub subject: String,
    /// The line's content, without its line break.
    pub text: String,
}

/// The header fields of a line whose content has not come yet.
struct Pending {
    line: u32,
    commit: String,
    path: String,
    author: String,
    author_at: i64,
    subject: String,
}

/// Parses the output of `git blame --line-porcelain`. A line Git has not committed yet (an
/// all-zero id) is left out: no commit owns it.
pub fn parse_line_porcelain(raw: &[u8]) -> Result<Vec<BlameLine>, String> {
    parse(raw, false)
}

/// Parses the output of `git blame --line-porcelain` of a whole file, keeping the lines Git
/// has not committed yet: their commit is the all-zero id (see `is_uncommitted`).
pub fn parse_line_porcelain_all(raw: &[u8]) -> Result<Vec<BlameLine>, String> {
    parse(raw, true)
}

fn parse(raw: &[u8], keep_uncommitted: bool) -> Result<Vec<BlameLine>, String> {
    let text = String::from_utf8_lossy(raw);
    let mut lines = Vec::new();
    let mut pending: Option<Pending> = None;
    for row in text.split('\n') {
        if let Some(content) = row.strip_prefix('\t') {
            let Some(head) = pending.take() else {
                return Err("blame printed content before a header".to_owned());
            };
            if keep_uncommitted || !is_uncommitted(&head.commit) {
                lines.push(BlameLine {
                    line: head.line,
                    commit: head.commit,
                    path: head.path,
                    author: head.author,
                    author_at: head.author_at,
                    subject: head.subject,
                    text: content.to_owned(),
                });
            }
        } else if let Some(line) = start_of_line(row) {
            pending = Some(Pending {
                line,
                commit: row.split(' ').next().unwrap_or_default().to_owned(),
                path: String::new(),
                author: String::new(),
                author_at: 0,
                subject: String::new(),
            });
        } else if let Some(head) = pending.as_mut() {
            if let Some(name) = row.strip_prefix("author ") {
                head.author.clear();
                head.author.push_str(name);
            } else if let Some(seconds) = row.strip_prefix("author-time ") {
                head.author_at = seconds.parse::<i64>().unwrap_or(0).saturating_mul(1000);
            } else if let Some(subject) = row.strip_prefix("summary ") {
                head.subject.clear();
                head.subject.push_str(subject);
            } else if let Some(path) = row.strip_prefix("filename ") {
                head.path.clear();
                head.path.push_str(path);
            }
        }
    }
    Ok(lines)
}

/// The line number of a group's first row, `<id> <original> <final> [<count>]`, or `None` for
/// any other row.
fn start_of_line(row: &str) -> Option<u32> {
    let mut fields = row.split(' ');
    let id = fields.next()?;
    let original = fields.next()?;
    let final_line = fields.next()?;
    let well_formed = id.len() == 40
        && id.bytes().all(|byte| byte.is_ascii_hexdigit())
        && original.parse::<u32>().is_ok();
    if well_formed {
        final_line.parse().ok()
    } else {
        None
    }
}

pub fn is_uncommitted(commit: &str) -> bool {
    commit.bytes().all(|byte| byte == b'0')
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const ZERO: &str = "0000000000000000000000000000000000000000";

    fn header(commit: &str, original: u32, final_line: u32, path: &str, when: i64) -> String {
        format!(
            "{commit} {original} {final_line} 1\nauthor Ana\nauthor-mail <a@x>\nauthor-time {when}\n\
             author-tz +0000\ncommitter Ana\ncommitter-time {when}\ncommitter-tz +0000\n\
             summary subject {final_line}\nfilename {path}\n"
        )
    }

    #[test]
    fn each_line_carries_its_own_commit_path_and_content() {
        let raw = format!(
            "{}\tfn main() {{\n{}\t    run();\n",
            header(A, 1, 1, "src/main.rs", 100),
            header(B, 7, 2, "src/copied.rs", 200),
        );
        let lines = parse_line_porcelain(raw.as_bytes()).expect("parse");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].commit, A);
        assert_eq!(lines[0].path, "src/main.rs");
        assert_eq!(lines[0].author_at, 100_000);
        assert_eq!(lines[0].subject, "subject 1");
        assert_eq!(lines[0].text, "fn main() {");
        assert_eq!(lines[1].line, 2);
        assert_eq!(lines[1].path, "src/copied.rs");
        assert_eq!(lines[1].author_at, 200_000);
        assert_eq!(lines[1].text, "    run();");
    }

    #[test]
    fn uncommitted_lines_are_left_out() {
        let raw = format!("{}\tnot yet\n", header(ZERO, 1, 1, "a.txt", 0));
        assert!(
            parse_line_porcelain(raw.as_bytes())
                .expect("parse")
                .is_empty()
        );
    }

    #[test]
    fn content_before_a_header_is_an_error() {
        assert!(parse_line_porcelain(b"\tlost\n").is_err());
    }

    #[test]
    fn a_crlf_line_keeps_its_carriage_return() {
        let raw = format!("{}\tlet x = 1;\r\n", header(A, 1, 1, "a.txt", 0));
        let lines = parse_line_porcelain(raw.as_bytes()).expect("parse");
        assert_eq!(lines[0].text, "let x = 1;\r");
    }
}
