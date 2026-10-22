//! Text search with `git grep` (docs/09, N3, D44). The pieces here are pure,
//! so they are tested without a repository: the checked request and the
//! arguments it makes, the parsing of Git's records, the limits, the order of
//! the answer and the shape of a result line. The adapter starts Git and feeds
//! it records.
//!
//! The pattern always follows `-e` and the pathspecs follow `--`, so text typed
//! in the box is never read as an option. Git runs on one thread: with
//! several, the lines that arrive before a limit stops the search depend on
//! timing, and the same search would not give the same answer twice.

use std::collections::BTreeMap;

use gm_schema::{
    GREP_FILE_LIMIT, GREP_LINE_LIMIT, GREP_TOTAL_LIMIT, GrepFile, GrepLine, GrepOptions,
    GrepResult, GrepScope,
};
use regex::{Regex, RegexBuilder};

/// A search the engine accepted: the text, how to read it and where to look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrepRequest {
    pub query: String,
    pub options: GrepOptions,
    /// Pathspecs for Git, after `--`. Empty searches the whole worktree.
    pub paths: Vec<String>,
}

impl GrepRequest {
    /// Checks the text and turns the `paths` box into pathspecs: words
    /// separated by spaces, and a word starting with `!` excludes (`:(exclude)`).
    pub fn new(query: &str, options: GrepOptions, paths: &str) -> Result<Self, String> {
        if query.is_empty() {
            return Err("the search text is empty".to_owned());
        }
        if query.contains(['\n', '\r', '\0']) {
            return Err("the search text must be one line".to_owned());
        }
        let paths = paths
            .split_whitespace()
            .map(|word| match word.strip_prefix('!') {
                Some(rest) if !rest.is_empty() => format!(":(exclude){rest}"),
                _ => word.to_owned(),
            })
            .collect();
        Ok(Self {
            query: query.to_owned(),
            options,
            paths,
        })
    }

    /// The arguments of `git grep`. Binary files are skipped (`-I`); untracked
    /// files that `.gitignore` does not hide are searched when the option is on,
    /// as Quick Open lists them; `-z` makes the output safe for any name.
    #[must_use]
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            "grep",
            "-I",
            "-n",
            "--column",
            "-z",
            "--no-color",
            "--threads=1",
        ]
        .map(str::to_owned)
        .into();
        if self.options.scope == GrepScope::TrackedAndNew {
            args.push("--untracked".to_owned());
        }
        if !self.options.case_sensitive {
            args.push("-i".to_owned());
        }
        if self.options.whole_word {
            args.push("-w".to_owned());
        }
        args.push(if self.options.regex { "-E" } else { "-F" }.to_owned());
        args.push("-e".to_owned());
        args.push(self.query.clone());
        args.push("--".to_owned());
        args.extend(self.paths.iter().cloned());
        args
    }
}

/// One record of Git's output: `path NUL line NUL column NUL text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record<'a> {
    pub path: &'a [u8],
    pub line: u32,
    /// 1-based byte column of the first match.
    pub column: usize,
    pub text: &'a [u8],
}

/// Reads one record, without the newline that ends it. `None` for anything
/// else, such as a path that holds a newline and split the record in two.
#[must_use]
pub fn parse_record(record: &[u8]) -> Option<Record<'_>> {
    let mut parts = record.splitn(4, |&b| b == 0);
    let path = parts.next()?;
    let line = std::str::from_utf8(parts.next()?).ok()?.parse().ok()?;
    let column = std::str::from_utf8(parts.next()?).ok()?.parse().ok()?;
    let text = parts.next()?;
    (!path.is_empty() && line > 0).then_some(Record {
        path,
        line,
        column,
        text,
    })
}

/// What a record did to the collector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// A line past `GREP_TOTAL_LIMIT` arrived: nothing more is wanted.
    Full,
}

/// Gathers the records of one search under the limits and orders the answer.
#[derive(Clone)]
pub struct Collector {
    query: String,
    highlight: Option<Regex>,
    files: BTreeMap<String, Vec<GrepLine>>,
    more: BTreeMap<String, usize>,
    listed: usize,
    truncated: bool,
}

impl Collector {
    #[must_use]
    pub fn new(request: &GrepRequest) -> Self {
        Self {
            query: request.query.clone(),
            highlight: highlighter(request),
            files: BTreeMap::new(),
            more: BTreeMap::new(),
            listed: 0,
            truncated: false,
        }
    }

    /// Takes one record (a line of Git's output, without its newline). A record
    /// that does not parse is skipped.
    pub fn push(&mut self, raw: &[u8]) -> Flow {
        let Some(record) = parse_record(raw) else {
            return Flow::Continue;
        };
        let path = String::from_utf8_lossy(record.path).into_owned();
        let in_file = self.files.entry(path.clone()).or_default().len();
        if in_file >= GREP_FILE_LIMIT {
            *self.more.entry(path).or_default() += 1;
            return Flow::Continue;
        }
        if self.listed >= GREP_TOTAL_LIMIT {
            // A listable line after the last one allowed: there is more.
            self.truncated = true;
            return Flow::Full;
        }
        let line = shape_line(
            record.text,
            record.column,
            self.highlight.as_ref(),
            record.line,
        );
        if let Some(lines) = self.files.get_mut(&path) {
            lines.push(line);
        }
        self.listed += 1;
        Flow::Continue
    }

    /// The answer: files by path, each with its lines in line order.
    #[must_use]
    pub fn finish(self) -> GrepResult {
        let Self {
            query,
            files,
            mut more,
            listed,
            truncated,
            ..
        } = self;
        let files = files
            .into_iter()
            .filter(|(_, lines)| !lines.is_empty())
            .map(|(path, mut lines)| {
                lines.sort_by_key(|line| line.line);
                let more = more.remove(&path).unwrap_or(0);
                GrepFile { path, lines, more }
            })
            .collect();
        GrepResult {
            query,
            files,
            listed,
            truncated,
        }
    }
}

/// The regular expression that finds the matches to highlight in a line. It
/// mirrors the options: a fixed string is escaped, `-i` and `-w` are honoured.
/// `None` when the pattern is one the `regex` crate cannot read (a back
/// reference, say): the line is then shown without highlight.
fn highlighter(request: &GrepRequest) -> Option<Regex> {
    let body = if request.options.regex {
        request.query.clone()
    } else {
        regex::escape(&request.query)
    };
    let pattern = if request.options.whole_word {
        format!(r"\b(?:{body})\b")
    } else {
        body
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!request.options.case_sensitive)
        .size_limit(1 << 20)
        .build()
        .ok()
}

/// Shapes a matching line for a result row: no indentation, no line end, at
/// most `GREP_LINE_LIMIT` bytes around the first match, and the matches
/// marked. A line that is not UTF-8 is shown with replacement characters and
/// no marks, because the column Git gave is a byte of the original.
#[must_use]
pub fn shape_line(raw: &[u8], column: usize, highlight: Option<&Regex>, number: u32) -> GrepLine {
    let Ok(text) = std::str::from_utf8(raw) else {
        let lossy = String::from_utf8_lossy(raw);
        let lossy = lossy.trim_end_matches('\r');
        return cut(lossy, lossy.len() - lossy.trim_start().len(), number, &[]);
    };
    let text = text.trim_end_matches('\r');
    let indent = text.len() - text.trim_start().len();
    // The window starts at the match when the line is longer than the limit.
    let first = column.saturating_sub(1).clamp(indent, text.len());
    let start = if text.len() - indent > GREP_LINE_LIMIT {
        floor_boundary(text, first.saturating_sub(GREP_LINE_LIMIT / 4).max(indent))
    } else {
        indent
    };
    let marks: Vec<(usize, usize)> = highlight
        .map(|re| re.find_iter(text).map(|m| (m.start(), m.end())).collect())
        .unwrap_or_default();
    cut(text, start, number, &marks)
}

/// The window of `text` from `start`, at most the limit long, with the marks
/// that fall in it moved to the window.
fn cut(text: &str, start: usize, number: u32, marks: &[(usize, usize)]) -> GrepLine {
    let content_start = text.len() - text.trim_start().len();
    let end = floor_boundary(text, (start + GREP_LINE_LIMIT).min(text.len()));
    let window = &text[start..end];
    let ranges = marks
        .iter()
        .filter_map(|&(a, b)| {
            let (a, b) = (a.max(start), b.min(end));
            (a < b).then(|| (a - start, b - start))
        })
        .collect();
    GrepLine {
        line: number,
        text: window.to_owned(),
        ranges,
        cut_start: start > content_start,
        cut_end: end < text.len(),
    }
}

/// The largest index at or below `at` that is a character boundary of `text`.
fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn options(case_sensitive: bool, whole_word: bool, regex: bool) -> GrepOptions {
        GrepOptions {
            case_sensitive,
            whole_word,
            regex,
            scope: GrepScope::TrackedAndNew,
        }
    }

    fn request(query: &str) -> GrepRequest {
        GrepRequest::new(query, GrepOptions::default(), "").unwrap()
    }

    fn record(path: &str, line: u32, column: usize, text: &str) -> Vec<u8> {
        format!("{path}\0{line}\0{column}\0{text}").into_bytes()
    }

    #[test]
    fn the_default_is_a_fixed_string_in_any_case() {
        let args = request("needle").args();
        assert!(args.contains(&"-F".to_owned()));
        assert!(args.contains(&"-i".to_owned()));
        assert!(!args.contains(&"-E".to_owned()));
        assert!(!args.contains(&"-w".to_owned()));
    }

    #[test]
    fn the_options_choose_the_flags() {
        let args = GrepRequest::new("a", options(true, true, true), "")
            .unwrap()
            .args();
        assert!(args.contains(&"-E".to_owned()));
        assert!(args.contains(&"-w".to_owned()));
        assert!(!args.contains(&"-i".to_owned()), "case sensitive has no -i");
        assert!(!args.contains(&"-F".to_owned()));
    }

    #[test]
    fn new_files_are_searched_unless_the_option_is_off() {
        assert!(request("a").args().contains(&"--untracked".to_owned()));
        let off = GrepOptions {
            scope: GrepScope::TrackedOnly,
            ..GrepOptions::default()
        };
        let args = GrepRequest::new("a", off, "").unwrap().args();
        assert!(!args.contains(&"--untracked".to_owned()));
    }

    #[test]
    fn the_pattern_follows_dash_e_and_the_pathspecs_follow_two_dashes() {
        for query in ["-v", "--help", "-e", "--", "-"] {
            let args = GrepRequest::new(query, GrepOptions::default(), "src -x")
                .unwrap()
                .args();
            let e = args.iter().position(|a| a == "-e").unwrap();
            assert_eq!(args[e + 1], query);
            // The separator is the last `--`, which the query itself may be.
            let dashes = args.iter().rposition(|a| a == "--").unwrap();
            assert!(dashes > e + 1, "{query}");
            assert_eq!(&args[dashes + 1..], ["src", "-x"], "{query}");
        }
    }

    #[test]
    fn git_runs_on_one_thread_so_a_limit_cuts_at_the_same_place_every_time() {
        assert!(request("a").args().contains(&"--threads=1".to_owned()));
    }

    #[test]
    fn a_word_starting_with_a_bang_excludes() {
        let request =
            GrepRequest::new("a", GrepOptions::default(), "crates  !docs :!tests !").unwrap();
        assert_eq!(request.paths, ["crates", ":(exclude)docs", ":!tests", "!"]);
    }

    #[test]
    fn an_empty_or_multi_line_search_is_refused() {
        assert!(GrepRequest::new("", GrepOptions::default(), "").is_err());
        assert!(GrepRequest::new("a\nb", GrepOptions::default(), "").is_err());
        assert!(GrepRequest::new("a\rb", GrepOptions::default(), "").is_err());
        // Spaces are part of the text.
        assert_eq!(request("  x ").query, "  x ");
    }

    #[test]
    fn a_record_has_a_path_a_line_a_column_and_the_text() {
        let raw = record("d e/f g.txt", 12, 5, "say hello\tthere");
        let parsed = parse_record(&raw).unwrap();
        assert_eq!(parsed.path, b"d e/f g.txt");
        assert_eq!((parsed.line, parsed.column), (12, 5));
        assert_eq!(parsed.text, b"say hello\tthere");
    }

    #[test]
    fn the_text_may_hold_any_byte_but_the_newline() {
        let mut raw = record("a", 1, 1, "x");
        raw.extend_from_slice(b"\0y\xff");
        assert_eq!(parse_record(&raw).unwrap().text, b"x\0y\xff");
    }

    #[test]
    fn a_record_that_is_not_one_is_skipped() {
        for raw in [
            &b""[..],
            b"a.txt",
            b"a.txt\x001\x00",
            b"a.txt\x00x\x001\x00t",
            b"a.txt\x000\x001\x00t",
            b"\x001\x001\x00t",
        ] {
            assert_eq!(parse_record(raw), None, "{raw:?}");
        }
        let mut collector = Collector::new(&request("t"));
        assert_eq!(collector.push(b"garbage"), Flow::Continue);
        assert!(collector.finish().files.is_empty());
    }

    #[test]
    fn the_answer_is_sorted_by_path_and_line_whatever_the_arrival_order() {
        let mut collector = Collector::new(&request("x"));
        for (path, line) in [("b.rs", 3), ("a.rs", 9), ("b.rs", 1), ("a.rs", 2)] {
            collector.push(&record(path, line, 1, "x"));
        }

        let result = collector.finish();

        let order: Vec<(String, Vec<u32>)> = result
            .files
            .iter()
            .map(|f| (f.path.clone(), f.lines.iter().map(|l| l.line).collect()))
            .collect();
        assert_eq!(
            order,
            [
                ("a.rs".to_owned(), vec![2, 9]),
                ("b.rs".to_owned(), vec![1, 3])
            ]
        );
        assert_eq!(result.listed, 4);
        assert!(!result.truncated);
    }

    #[test]
    fn a_file_lists_a_hundred_lines_and_counts_the_rest() {
        let mut collector = Collector::new(&request("x"));
        for line in 1..=130 {
            assert_eq!(
                collector.push(&record("big.rs", line, 1, "x")),
                Flow::Continue
            );
        }

        let result = collector.finish();

        assert_eq!(result.files[0].lines.len(), GREP_FILE_LIMIT);
        assert_eq!(result.files[0].more, 30);
        assert_eq!(result.listed, GREP_FILE_LIMIT);
        assert!(
            !result.truncated,
            "per-file overflow is not the total limit"
        );
    }

    #[test]
    fn the_search_stops_at_the_line_after_the_thousandth() {
        let mut collector = Collector::new(&request("x"));
        // Fifty files of twenty lines: exactly the total limit.
        for file in 0..50 {
            for line in 1..=20 {
                let path = format!("f{file:02}.rs");
                assert_eq!(collector.push(&record(&path, line, 1, "x")), Flow::Continue);
            }
        }
        // Exactly the limit and nothing more: not truncated.
        assert!(!collector.clone().finish().truncated);

        assert_eq!(collector.push(&record("g.rs", 1, 1, "x")), Flow::Full);
        let result = collector.finish();
        assert_eq!(result.listed, GREP_TOTAL_LIMIT);
        assert!(result.truncated);
        assert!(result.files.iter().all(|f| f.path != "g.rs"));
    }

    #[test]
    fn a_match_is_highlighted_in_the_line_without_its_indentation() {
        let re = highlighter(&request("hello"));
        let line = shape_line(b"    say Hello, hello", 9, re.as_ref(), 7);
        assert_eq!(line.text, "say Hello, hello");
        assert_eq!(line.ranges, [(4, 9), (11, 16)]);
        assert_eq!(line.line, 7);
        assert!(!line.cut_start && !line.cut_end);
    }

    #[test]
    fn case_sensitivity_whole_word_and_regex_shape_the_highlight() {
        let marks = |query: &str, o: GrepOptions, text: &str| {
            let request = GrepRequest::new(query, o, "").unwrap();
            let re = highlighter(&request);
            shape_line(text.as_bytes(), 1, re.as_ref(), 1).ranges
        };
        assert_eq!(marks("ab", options(true, false, false), "ab AB"), [(0, 2)]);
        assert_eq!(
            marks("ab", options(false, false, false), "ab AB"),
            [(0, 2), (3, 5)]
        );
        assert_eq!(
            marks("ab", options(false, true, false), "ab cab ab_"),
            [(0, 2)]
        );
        assert_eq!(
            marks("a.c", options(false, false, false), "abc a.c"),
            [(4, 7)]
        );
        assert_eq!(
            marks("a.c", options(false, false, true), "abc a.c"),
            [(0, 3), (4, 7)]
        );
    }

    #[test]
    fn a_pattern_the_highlighter_cannot_read_shows_the_line_unmarked() {
        let request = GrepRequest::new(r"(a)\1", options(true, false, true), "").unwrap();
        assert!(highlighter(&request).is_none());
        let line = shape_line(b"aa", 1, None, 1);
        assert_eq!(line.text, "aa");
        assert!(line.ranges.is_empty());
    }

    #[test]
    fn a_line_end_is_dropped() {
        let line = shape_line(b"text\r", 1, None, 1);
        assert_eq!(line.text, "text");
    }

    #[test]
    fn a_long_line_is_cut_around_the_match_on_a_character_boundary() {
        let before = "é".repeat(400);
        let text = format!("{before}NEEDLE{}", "ü".repeat(400));
        let column = before.len() + 1;
        let re = highlighter(&request("needle"));

        let line = shape_line(text.as_bytes(), column, re.as_ref(), 1);

        assert!(line.text.len() <= GREP_LINE_LIMIT);
        assert!(line.cut_start && line.cut_end);
        let &(a, b) = line.ranges.first().unwrap();
        assert_eq!(&line.text[a..b], "NEEDLE");
    }

    #[test]
    fn a_long_line_with_the_match_at_the_start_is_cut_at_the_end_only() {
        let text = format!("needle {}", "x".repeat(1000));
        let re = highlighter(&request("needle"));
        let line = shape_line(text.as_bytes(), 1, re.as_ref(), 1);
        assert!(!line.cut_start && line.cut_end);
        assert_eq!(line.ranges, [(0, 6)]);
        assert_eq!(line.text.len(), GREP_LINE_LIMIT);
    }

    #[test]
    fn a_line_that_is_not_utf8_is_shown_unmarked() {
        let re = highlighter(&request("caf"));
        let line = shape_line(b"caf\xe9 caf", 1, re.as_ref(), 1);
        assert_eq!(line.text, "caf\u{fffd} caf");
        assert!(line.ranges.is_empty());
    }

    #[test]
    fn a_line_of_only_blanks_is_empty_text() {
        let line = shape_line(b"   \t", 1, None, 1);
        assert_eq!(line.text, "");
    }

    proptest! {
        #[test]
        fn a_shaped_line_is_valid_text_with_marks_inside_it(
            text in "\\PC{0,900}",
            column in 0usize..1000,
            query in "[a-zA-Z]{1,3}",
        ) {
            let request = GrepRequest::new(&query, GrepOptions::default(), "").unwrap();
            let re = highlighter(&request);
            let line = shape_line(text.as_bytes(), column, re.as_ref(), 1);

            prop_assert!(line.text.len() <= GREP_LINE_LIMIT);
            for &(a, b) in &line.ranges {
                prop_assert!(a < b && b <= line.text.len());
                prop_assert!(line.text.is_char_boundary(a) && line.text.is_char_boundary(b));
                prop_assert!(line.text[a..b].eq_ignore_ascii_case(&query));
            }
        }

        #[test]
        fn the_answer_does_not_depend_on_the_order_records_arrive_in(
            seed in prop::collection::vec((0u8..5, 1u32..40), 1..80),
            rotate in 0usize..80,
        ) {
            let records: Vec<Vec<u8>> = seed
                .iter()
                .map(|&(file, line)| record(&format!("f{file}.rs"), line, 1, "x"))
                .collect();
            let run = |records: &[Vec<u8>]| {
                let mut collector = Collector::new(&request("x"));
                for r in records {
                    collector.push(r);
                }
                collector.finish()
            };
            let mut rotated = records.clone();
            rotated.rotate_left(rotate % records.len());
            prop_assert_eq!(run(&records), run(&rotated));
        }
    }
}
