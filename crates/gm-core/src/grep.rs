//! Text search with `git grep` (design/code-navigation.md, N3, D44). The pieces here are pure,
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
use regex::bytes::{Regex, RegexBuilder};

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

/// The searches a request becomes when the project has submodules (D175): the
/// repository's own search, and one search per submodule that is searched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Split {
    /// `None` when every include word went to a submodule, so the repository is
    /// not searched at all.
    pub repo: Option<GrepRequest>,
    /// The submodules searched, by path, each with its own pathspecs.
    pub submodules: Vec<(String, GrepRequest)>,
}

impl GrepRequest {
    /// Routes the pathspecs of the request to the repository and to each of
    /// `submodules` (paths from the root, one level). A word `P` or `P/` names
    /// the submodule `P` as a whole; `P/x` is `x` inside it. An exclude word
    /// `P` skips the submodule. When there are include words and none names
    /// `P`, `P` is not searched. The words that name no submodule go to the
    /// repository as they are.
    #[must_use]
    pub fn split(&self, submodules: &[String]) -> Split {
        let has_includes = self.paths.iter().any(|w| exclude_body(w).is_none());
        let mut searched = Vec::new();
        for module in submodules {
            let mut whole = false;
            let mut skip = false;
            let mut includes = Vec::new();
            let mut excludes = Vec::new();
            for word in &self.paths {
                match exclude_body(word) {
                    Some(body) => match inside(body, module) {
                        Some("") => skip = true,
                        Some(rest) => excludes.push(format!(":(exclude){rest}")),
                        None => {}
                    },
                    None => match inside(word, module) {
                        Some("") => whole = true,
                        Some(rest) => includes.push(rest.to_owned()),
                        None => {}
                    },
                }
            }
            if skip || (has_includes && !whole && includes.is_empty()) {
                continue;
            }
            // A word naming the whole submodule searches all of it: no include words.
            let mut paths = if whole { Vec::new() } else { includes };
            paths.extend(excludes);
            searched.push((
                module.clone(),
                GrepRequest {
                    query: self.query.clone(),
                    options: self.options,
                    paths,
                },
            ));
        }
        let repo_paths: Vec<String> = self
            .paths
            .iter()
            .filter(|word| {
                let body = exclude_body(word).unwrap_or(word.as_str());
                !submodules.iter().any(|m| inside(body, m).is_some())
            })
            .cloned()
            .collect();
        let repo_has_includes = repo_paths.iter().any(|w| exclude_body(w).is_none());
        let repo = (!has_includes || repo_has_includes).then(|| GrepRequest {
            query: self.query.clone(),
            options: self.options,
            paths: repo_paths,
        });
        Split {
            repo,
            submodules: searched,
        }
    }
}

/// The body of an exclude word: `:(exclude)x` (as a request makes it), or the
/// short forms `:!x` and `:^x`. `None` for an include word.
fn exclude_body(word: &str) -> Option<&str> {
    word.strip_prefix(":(exclude)")
        .or_else(|| word.strip_prefix(":!"))
        .or_else(|| word.strip_prefix(":^"))
}

/// What is left of `word` inside the submodule `module`: `""` for the
/// submodule itself (`P` or `P/`), `x` for `P/x`, `None` when the word does not
/// name anything inside it.
fn inside<'a>(word: &'a str, module: &str) -> Option<&'a str> {
    let rest = word.strip_prefix(module)?;
    match rest.strip_prefix('/') {
        Some(rest) => Some(rest),
        None if rest.is_empty() => Some(""),
        None => None,
    }
}

/// Puts the files of a result under `folder/`, for the result of a submodule.
#[must_use]
pub fn under_folder(mut result: GrepResult, folder: &str) -> GrepResult {
    for file in &mut result.files {
        file.path = format!("{folder}/{}", file.path);
    }
    result
}

/// Joins the results of the parts of one search (the repository and its
/// submodules) into one answer under the limits: files by full path, lines in
/// line order, at most `GREP_TOTAL_LIMIT` lines in all. A part that was cut
/// short, or lines that do not fit, make the answer cut off.
#[must_use]
pub fn merge(query: &str, parts: Vec<GrepResult>) -> GrepResult {
    let mut truncated = parts.iter().any(|part| part.truncated);
    let mut files: Vec<GrepFile> = parts.into_iter().flat_map(|part| part.files).collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut listed = 0;
    let mut kept = Vec::new();
    for file in files {
        let mut lines = Vec::new();
        for line in file.lines {
            if listed >= GREP_TOTAL_LIMIT {
                truncated = true;
                break;
            }
            listed += 1;
            lines.push(line);
        }
        if !lines.is_empty() {
            kept.push(GrepFile {
                path: file.path,
                lines,
                more: file.more,
            });
        }
    }
    GrepResult {
        query: query.to_owned(),
        files: kept,
        listed,
        truncated,
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
    // ASCII-only case folding and word boundaries, as `git grep -i` and `-w` match: with
    // Unicode on, `k` would also mark the Kelvin sign and `s` the long s, which Git does not
    // match. The regex runs on bytes, so `.` may stop inside a character; `shape_line` widens
    // each mark to whole characters.
    RegexBuilder::new(&pattern)
        .unicode(false)
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
        .map(|re| {
            re.find_iter(text.as_bytes())
                .map(|m| {
                    (
                        floor_boundary(text, m.start()),
                        ceil_boundary(text, m.end()),
                    )
                })
                .filter(|(a, b)| a < b)
                .collect()
        })
        .unwrap_or_default();
    cut(text, start, number, &marks)
}

/// The window of `text` from `start`, at most the limit long, with the marks
/// that fall in it moved to the window.
fn cut(text: &str, start: usize, number: u32, marks: &[(usize, usize)]) -> GrepLine {
    let content_start = text.len() - text.trim_start().len();
    let end = floor_boundary(text, (start + GREP_LINE_LIMIT).min(text.len()));
    // A match is never cut in half: an edge that falls inside one moves out of it, so the
    // window holds each match whole or not at all. Matches do not overlap, and the window's
    // first match starts after `start`, so the start only moves forward to an earlier match's
    // end and the end only moves back to a later match's start: the window never grows.
    let start = marks
        .iter()
        .find(|&&(a, b)| a < start && start < b)
        .map_or(start, |&(_, b)| b);
    let end = marks
        .iter()
        .find(|&&(a, b)| a < end && end < b)
        .map_or(end, |&(a, _)| a.max(start));
    let window = &text[start..end];
    let ranges = marks
        .iter()
        .filter(|&&(a, b)| start <= a && b <= end && a < b)
        .map(|&(a, b)| (a - start, b - start))
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
fn ceil_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

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

    fn split(paths: &str, modules: &[&str]) -> Split {
        let request = GrepRequest::new("x", GrepOptions::default(), paths).unwrap();
        let modules: Vec<String> = modules.iter().map(|m| (*m).to_owned()).collect();
        request.split(&modules)
    }

    /// The pathspecs of submodule `module`, if it is searched.
    fn inside_paths(split: &Split, module: &str) -> Option<Vec<String>> {
        split
            .submodules
            .iter()
            .find(|(m, _)| m == module)
            .map(|(_, request)| request.paths.clone())
    }

    fn repo_paths(split: &Split) -> Option<Vec<String>> {
        split.repo.as_ref().map(|request| request.paths.clone())
    }

    #[test]
    fn no_words_search_every_submodule_as_a_whole_and_the_repository() {
        let s = split("", &["sub"]);
        assert_eq!(inside_paths(&s, "sub"), Some(Vec::new()));
        assert_eq!(repo_paths(&s), Some(Vec::new()));
    }

    #[test]
    fn a_word_naming_a_submodule_searches_it_whole_and_not_the_repository() {
        for word in ["sub", "sub/"] {
            let s = split(word, &["sub"]);
            assert_eq!(inside_paths(&s, "sub"), Some(Vec::new()), "{word}");
            assert_eq!(repo_paths(&s), None, "{word}");
        }
    }

    #[test]
    fn a_word_inside_a_submodule_goes_to_it_with_the_folder_stripped() {
        let s = split("sub/src", &["sub"]);
        assert_eq!(inside_paths(&s, "sub"), Some(vec!["src".to_owned()]));
        assert_eq!(repo_paths(&s), None);
    }

    #[test]
    fn an_unrelated_include_word_skips_every_submodule() {
        let s = split("docs", &["sub"]);
        assert_eq!(inside_paths(&s, "sub"), None);
        assert_eq!(repo_paths(&s), Some(vec!["docs".to_owned()]));
    }

    #[test]
    fn words_split_between_the_repository_and_a_submodule() {
        let s = split("docs sub/src", &["sub"]);
        assert_eq!(inside_paths(&s, "sub"), Some(vec!["src".to_owned()]));
        assert_eq!(repo_paths(&s), Some(vec!["docs".to_owned()]));
    }

    #[test]
    fn a_name_that_only_starts_like_a_submodule_is_not_routed() {
        let s = split("subway", &["sub"]);
        assert_eq!(inside_paths(&s, "sub"), None);
        assert_eq!(repo_paths(&s), Some(vec!["subway".to_owned()]));
    }

    #[test]
    fn an_exclude_naming_a_submodule_skips_it() {
        for word in ["!sub", "!sub/", ":!sub"] {
            let s = split(word, &["sub"]);
            assert_eq!(inside_paths(&s, "sub"), None, "{word}");
            assert_eq!(repo_paths(&s), Some(Vec::new()), "{word}");
        }
    }

    #[test]
    fn an_exclude_inside_a_submodule_goes_to_it_and_not_the_repository() {
        let s = split("!sub/tests", &["sub"]);
        assert_eq!(
            inside_paths(&s, "sub"),
            Some(vec![":(exclude)tests".to_owned()])
        );
        assert_eq!(repo_paths(&s), Some(Vec::new()));
    }

    #[test]
    fn an_exclude_of_the_repository_still_reaches_it_and_the_submodule_is_searched() {
        let s = split("!docs", &["sub"]);
        assert_eq!(inside_paths(&s, "sub"), Some(Vec::new()));
        assert_eq!(repo_paths(&s), Some(vec![":(exclude)docs".to_owned()]));
    }

    #[test]
    fn an_include_inside_and_an_exclude_inside_combine() {
        let s = split("sub/src !sub/src/gen", &["sub"]);
        assert_eq!(
            inside_paths(&s, "sub"),
            Some(vec!["src".to_owned(), ":(exclude)src/gen".to_owned()])
        );
        assert_eq!(repo_paths(&s), None);
    }

    #[test]
    fn a_submodule_named_by_a_whole_word_drops_its_include_words_inside() {
        let s = split("sub sub/src", &["sub"]);
        assert_eq!(inside_paths(&s, "sub"), Some(Vec::new()));
        assert_eq!(repo_paths(&s), None);
    }

    #[test]
    fn two_submodules_are_routed_by_their_own_names() {
        let s = split("a/x b/y", &["a", "b"]);
        assert_eq!(inside_paths(&s, "a"), Some(vec!["x".to_owned()]));
        assert_eq!(inside_paths(&s, "b"), Some(vec!["y".to_owned()]));
        assert_eq!(repo_paths(&s), None);
    }

    #[test]
    fn a_split_keeps_the_query_and_the_options_of_the_request() {
        let options = GrepOptions {
            case_sensitive: true,
            whole_word: true,
            regex: true,
            scope: GrepScope::TrackedOnly,
        };
        let request = GrepRequest::new("a.b", options, "sub").unwrap();
        let split = request.split(&["sub".to_owned()]);
        let (_, inner) = &split.submodules[0];
        assert_eq!(inner.query, "a.b");
        assert_eq!(inner.options, options);
    }

    fn result(files: &[(&str, usize)]) -> GrepResult {
        let files: Vec<GrepFile> = files
            .iter()
            .map(|&(path, count)| GrepFile {
                path: path.to_owned(),
                lines: (1..=count)
                    .map(|line| GrepLine {
                        line: u32::try_from(line).unwrap(),
                        text: "x".to_owned(),
                        ranges: Vec::new(),
                        cut_start: false,
                        cut_end: false,
                    })
                    .collect(),
                more: 0,
            })
            .collect();
        GrepResult {
            query: "x".to_owned(),
            listed: files.iter().map(|f| f.lines.len()).sum(),
            files,
            truncated: false,
        }
    }

    #[test]
    fn a_submodule_result_gets_its_folder_in_front_of_every_path() {
        let shown = under_folder(result(&[("src/a.rs", 1)]), "sub");
        assert_eq!(shown.files[0].path, "sub/src/a.rs");
    }

    #[test]
    fn merged_parts_are_ordered_by_full_path_and_keep_their_lines() {
        let merged = merge(
            "x",
            vec![
                under_folder(result(&[("b.rs", 1)]), "sub"),
                result(&[("z.rs", 2), ("a.rs", 1)]),
            ],
        );

        let paths: Vec<&str> = merged.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.rs", "sub/b.rs", "z.rs"]);
        assert_eq!(merged.listed, 4);
        assert!(!merged.truncated);
        assert_eq!(merged.query, "x");
    }

    #[test]
    fn merging_cuts_at_the_total_limit_across_parts() {
        let repo = result(&[("a.rs", GREP_TOTAL_LIMIT - 5)]);
        let sub = under_folder(result(&[("f.rs", 20)]), "sub");

        let merged = merge("x", vec![repo, sub]);

        assert_eq!(merged.listed, GREP_TOTAL_LIMIT);
        assert!(merged.truncated);
        // The five lines that still fit come from the submodule, then the rest is cut.
        let last = merged.files.last().unwrap();
        assert_eq!((last.path.as_str(), last.lines.len()), ("sub/f.rs", 5));
    }

    #[test]
    fn a_part_cut_short_makes_the_merged_answer_cut_off() {
        let mut short = result(&[("a.rs", 3)]);
        short.truncated = true;

        let merged = merge("x", vec![short, result(&[("b.rs", 1)])]);

        assert!(merged.truncated);
        assert_eq!(merged.listed, 4);
    }

    #[test]
    fn exactly_the_limit_across_parts_is_not_cut_off() {
        let merged = merge(
            "x",
            vec![
                result(&[("a.rs", GREP_TOTAL_LIMIT / 2)]),
                under_folder(result(&[("b.rs", GREP_TOTAL_LIMIT / 2)]), "sub"),
            ],
        );

        assert_eq!(merged.listed, GREP_TOTAL_LIMIT);
        assert!(!merged.truncated);
    }

    #[test]
    fn case_folds_as_git_does_ascii_only() {
        let request = GrepRequest::new("k", GrepOptions::default(), "").unwrap();
        let re = highlighter(&request);
        // The Kelvin sign folds to k in Unicode, but `git grep -i` does not match it.
        let line = shape_line("a\u{212A}b K k".as_bytes(), 1, re.as_ref(), 1);
        assert_eq!(line.ranges, vec![(6, 7), (8, 9)]);
        let request = GrepRequest::new("é", GrepOptions::default(), "").unwrap();
        let line = shape_line("café".as_bytes(), 1, highlighter(&request).as_ref(), 1);
        assert_eq!(line.ranges, vec![(3, 5)]);
    }

    #[test]
    fn a_regex_mark_that_stops_inside_a_character_covers_the_whole_character() {
        let options = GrepOptions {
            regex: true,
            ..GrepOptions::default()
        };
        let request = GrepRequest::new("a.", options, "").unwrap();
        let line = shape_line("xaéy".as_bytes(), 1, highlighter(&request).as_ref(), 1);
        assert_eq!(line.ranges, vec![(1, 4)]);
    }

    #[test]
    fn a_match_across_the_end_of_the_window_is_left_out_whole_not_cut_in_half() {
        let request = GrepRequest::new("up", GrepOptions::default(), "").unwrap();
        let re = highlighter(&request);
        // "up" at bytes 299..301 sits across the 300-byte window of a line matched at column 1.
        let text = format!("up{}up{}", "x".repeat(297), "y".repeat(100));
        let line = shape_line(text.as_bytes(), 1, re.as_ref(), 1);
        assert!(line.cut_end);
        assert_eq!(line.text.len(), 299);
        assert_eq!(line.ranges, vec![(0, 2)]);
    }

    #[test]
    fn a_match_across_the_start_of_the_window_is_left_out_whole_not_cut_in_half() {
        let request = GrepRequest::new("abc", GrepOptions::default(), "").unwrap();
        let re = highlighter(&request);
        // The window starts 75 bytes before the match at byte 500, inside the "abc" at 424..427.
        let text = format!(
            "{}abc{}abc{}",
            "x".repeat(424),
            "y".repeat(73),
            "z".repeat(400)
        );
        let line = shape_line(text.as_bytes(), 501, re.as_ref(), 1);
        assert!(line.cut_start);
        for &(a, b) in &line.ranges {
            assert_eq!(&line.text[a..b], "abc");
        }
        assert_eq!(line.ranges.len(), 1);
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
