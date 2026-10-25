//! Conflict markers in a file's text (D-conflict resolution, slice C1). Pure:
//! the parser, the model of a resolution and the render back to text. No I/O,
//! no Git. A file with no unresolved hunk renders back byte for byte.

use std::fmt;

const OPEN: &str = "<<<<<<<";
const BASE: &str = "|||||||";
const SEPARATOR: &str = "=======";
const CLOSE: &str = ">>>>>>>";

/// A file split into the text around its conflicts, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictDoc {
    pub segments: Vec<Segment>,
}

/// One piece of a file: text that does not conflict, or a conflict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    /// Non-conflicting text, kept byte for byte with its line endings.
    Text(String),
    /// A conflict between the two sides.
    Conflict(Hunk),
}

/// One conflict: the two sides, the base of a zdiff3 or diff3 file, the labels
/// Git printed after the markers and the resolution once the human picks one.
///
/// `eol` is the line ending of the marker lines (`\n` or `\r\n`) and
/// `close_eol` the one of the closing marker, empty when the file ends
/// without a newline. Both keep the render byte exact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub left: String,
    pub base: Option<String>,
    pub right: String,
    pub left_label: String,
    pub base_label: String,
    pub right_label: String,
    pub resolution: Option<Resolution>,
    pub eol: String,
    pub close_eol: String,
}

/// What the human chose for a hunk. `Both` is the left side then the right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Left,
    Right,
    Both,
    Custom(String),
}

/// A malformed conflict, with the 1-based line where it was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub kind: ParseErrorKind,
}

/// Why the markers do not form a conflict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// A conflict opened at `line` is never closed by `>>>>>>>`.
    Unterminated,
    /// `>>>>>>>` came before `=======`.
    MissingSeparator,
    /// A marker that cannot appear where it is, such as a stray `=======`.
    UnexpectedMarker,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.kind {
            ParseErrorKind::Unterminated => "conflict is never closed by a >>>>>>> marker",
            ParseErrorKind::MissingSeparator => "closing marker before the ======= separator",
            ParseErrorKind::UnexpectedMarker => "conflict marker where it cannot appear",
        };
        write!(f, "line {}: {reason}", self.line)
    }
}

impl std::error::Error for ParseError {}

/// Where the parser is inside a conflict.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Left,
    Base,
    Right,
}

/// A conflict whose closing marker has not been read yet.
struct Open {
    line: usize,
    stage: Stage,
    hunk: Hunk,
}

/// Splits `text` into its conflicts. Accepts LF and CRLF, a last line without a
/// newline, and empty sides. Malformed or unterminated markers are an error
/// with the line number; this never panics.
///
/// # Errors
///
/// Returns the first marker that does not form a well-formed conflict.
pub fn parse(text: &str) -> Result<ConflictDoc, ParseError> {
    let mut segments = Vec::new();
    let mut plain = String::new();
    let mut open: Option<Open> = None;
    for (index, raw) in text.split_inclusive('\n').enumerate() {
        let line = index + 1;
        let content = strip_eol(raw);
        let eol = &raw[content.len()..];
        match open.take() {
            None => {
                if let Some(label) = marker_label(content, OPEN) {
                    if !plain.is_empty() {
                        segments.push(Segment::Text(std::mem::take(&mut plain)));
                    }
                    open = Some(Open {
                        line,
                        stage: Stage::Left,
                        hunk: new_hunk(label, eol),
                    });
                } else if is_stray(content) {
                    return Err(error(line, ParseErrorKind::UnexpectedMarker));
                } else {
                    plain.push_str(raw);
                }
            }
            Some(mut current) => {
                if marker_label(content, OPEN).is_some() {
                    return Err(error(current.line, ParseErrorKind::Unterminated));
                }
                match (current.stage, content) {
                    (Stage::Left, _) if marker_label(content, BASE).is_some() => {
                        current.hunk.base = Some(String::new());
                        marker_label(content, BASE)
                            .unwrap_or_default()
                            .clone_into(&mut current.hunk.base_label);
                        current.stage = Stage::Base;
                    }
                    (Stage::Left | Stage::Base, SEPARATOR) => current.stage = Stage::Right,
                    (Stage::Left | Stage::Base, _) if marker_label(content, CLOSE).is_some() => {
                        return Err(error(line, ParseErrorKind::MissingSeparator));
                    }
                    (Stage::Right, _) if marker_label(content, CLOSE).is_some() => {
                        marker_label(content, CLOSE)
                            .unwrap_or_default()
                            .clone_into(&mut current.hunk.right_label);
                        eol.clone_into(&mut current.hunk.close_eol);
                        segments.push(Segment::Conflict(current.hunk));
                        continue;
                    }
                    _ if is_stray(content) => {
                        return Err(error(line, ParseErrorKind::UnexpectedMarker));
                    }
                    (Stage::Left, _) => current.hunk.left.push_str(raw),
                    (Stage::Base, _) => {
                        current
                            .hunk
                            .base
                            .get_or_insert_with(String::new)
                            .push_str(raw);
                    }
                    (Stage::Right, _) => current.hunk.right.push_str(raw),
                }
                open = Some(current);
            }
        }
    }
    if let Some(current) = open {
        return Err(error(current.line, ParseErrorKind::Unterminated));
    }
    if !plain.is_empty() {
        segments.push(Segment::Text(plain));
    }
    Ok(ConflictDoc { segments })
}

impl ConflictDoc {
    /// How many conflicts the file has, resolved or not.
    #[must_use]
    pub fn conflict_count(&self) -> usize {
        self.hunks().count()
    }

    /// How many conflicts have a resolution.
    #[must_use]
    pub fn resolved_count(&self) -> usize {
        self.hunks()
            .filter(|hunk| hunk.resolution.is_some())
            .count()
    }

    /// True when every conflict has a resolution (a file with none is resolved).
    #[must_use]
    pub fn is_fully_resolved(&self) -> bool {
        self.resolved_count() == self.conflict_count()
    }

    /// Sets the resolution of the conflict at `index`, counting conflicts only.
    /// Does nothing when there is no such conflict.
    pub fn set_resolution(&mut self, index: usize, resolution: Option<Resolution>) {
        let hunk = self
            .segments
            .iter_mut()
            .filter_map(|segment| match segment {
                Segment::Conflict(hunk) => Some(hunk),
                Segment::Text(_) => None,
            })
            .nth(index);
        if let Some(hunk) = hunk {
            hunk.resolution = resolution;
        }
    }

    /// The text of the file: resolved conflicts as their chosen text, the others
    /// as the markers they came from (zdiff3 form with the base when present).
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Text(text) => out.push_str(text),
                Segment::Conflict(hunk) => match &hunk.resolution {
                    Some(Resolution::Left) => out.push_str(&hunk.left),
                    Some(Resolution::Right) => out.push_str(&hunk.right),
                    Some(Resolution::Both) => {
                        out.push_str(&hunk.left);
                        out.push_str(&hunk.right);
                    }
                    Some(Resolution::Custom(text)) => out.push_str(text),
                    None => render_markers(hunk, &mut out),
                },
            }
        }
        out
    }

    fn hunks(&self) -> impl Iterator<Item = &Hunk> {
        self.segments.iter().filter_map(|segment| match segment {
            Segment::Conflict(hunk) => Some(hunk),
            Segment::Text(_) => None,
        })
    }
}

/// Writes the markers of an unresolved hunk, zdiff3 form.
fn render_markers(hunk: &Hunk, out: &mut String) {
    push_marker(out, OPEN, &hunk.left_label, &hunk.eol);
    out.push_str(&hunk.left);
    if let Some(base) = &hunk.base {
        push_marker(out, BASE, &hunk.base_label, &hunk.eol);
        out.push_str(base);
    }
    push_marker(out, SEPARATOR, "", &hunk.eol);
    out.push_str(&hunk.right);
    push_marker(out, CLOSE, &hunk.right_label, &hunk.close_eol);
}

/// Writes one marker line: the marker, the label after one space when there is
/// one, then the line ending.
fn push_marker(out: &mut String, marker: &str, label: &str, eol: &str) {
    out.push_str(marker);
    if !label.is_empty() {
        out.push(' ');
        out.push_str(label);
    }
    out.push_str(eol);
}

fn new_hunk(left_label: &str, eol: &str) -> Hunk {
    Hunk {
        left: String::new(),
        base: None,
        right: String::new(),
        left_label: left_label.to_owned(),
        base_label: String::new(),
        right_label: String::new(),
        resolution: None,
        eol: eol.to_owned(),
        close_eol: String::new(),
    }
}

fn error(line: usize, kind: ParseErrorKind) -> ParseError {
    ParseError { line, kind }
}

/// The line without its `\n` or `\r\n`. A lone trailing `\r` is kept as content.
fn strip_eol(raw: &str) -> &str {
    let without_lf = raw.strip_suffix('\n').unwrap_or(raw);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
}

/// The label of a marker line, or `None` when the line is not that marker. The
/// label is what follows the marker and one space; a bare marker has `""`.
fn marker_label<'a>(content: &'a str, marker: &str) -> Option<&'a str> {
    if content == marker {
        return Some("");
    }
    content.strip_prefix(marker)?.strip_prefix(' ')
}

/// True for a marker that only a conflict can contain: the base, separator or
/// closing marker (the opening one is handled by the caller).
fn is_stray(content: &str) -> bool {
    content == SEPARATOR
        || marker_label(content, BASE).is_some()
        || marker_label(content, CLOSE).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn hunk(left: &str, right: &str) -> Hunk {
        Hunk {
            left: left.to_owned(),
            base: None,
            right: right.to_owned(),
            left_label: "HEAD".to_owned(),
            base_label: String::new(),
            right_label: "feature".to_owned(),
            resolution: None,
            eol: "\n".to_owned(),
            close_eol: "\n".to_owned(),
        }
    }

    const SIMPLE: &str = "top\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\nbottom\n";

    #[test]
    fn round_trips_a_simple_conflict_byte_exact() {
        let doc = parse(SIMPLE).unwrap();
        assert_eq!(
            doc.segments,
            vec![
                Segment::Text("top\n".to_owned()),
                Segment::Conflict(hunk("ours\n", "theirs\n")),
                Segment::Text("bottom\n".to_owned()),
            ]
        );
        assert_eq!(doc.render(), SIMPLE);
    }

    #[test]
    fn keeps_crlf_line_endings() {
        let text = "a\r\n<<<<<<< HEAD\r\nours\r\n=======\r\ntheirs\r\n>>>>>>> feature\r\nb\r\n";
        let doc = parse(text).unwrap();
        let Segment::Conflict(found) = &doc.segments[1] else {
            panic!("expected a conflict");
        };
        assert_eq!(found.left, "ours\r\n");
        assert_eq!(found.right, "theirs\r\n");
        assert_eq!(found.left_label, "HEAD");
        assert_eq!(found.eol, "\r\n");
        assert_eq!(doc.render(), text);
    }

    #[test]
    fn keeps_a_file_with_no_trailing_newline() {
        let text = "x\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature";
        let doc = parse(text).unwrap();
        let Segment::Conflict(found) = &doc.segments[1] else {
            panic!("expected a conflict");
        };
        assert_eq!(found.close_eol, "");
        assert_eq!(found.right_label, "feature");
        assert_eq!(doc.render(), text);
    }

    #[test]
    fn parses_the_base_section_of_zdiff3() {
        let text = "<<<<<<< HEAD\nours\n||||||| merged common ancestors\nbase\n=======\ntheirs\n>>>>>>> feature\n";
        let doc = parse(text).unwrap();
        let Segment::Conflict(found) = &doc.segments[0] else {
            panic!("expected a conflict");
        };
        assert_eq!(found.base.as_deref(), Some("base\n"));
        assert_eq!(found.base_label, "merged common ancestors");
        assert_eq!(doc.render(), text);
    }

    #[test]
    fn a_base_section_that_is_empty_is_kept() {
        let text = "<<<<<<< HEAD\nours\n|||||||\n=======\ntheirs\n>>>>>>> feature\n";
        let doc = parse(text).unwrap();
        let Segment::Conflict(found) = &doc.segments[0] else {
            panic!("expected a conflict");
        };
        assert_eq!(found.base.as_deref(), Some(""));
        assert_eq!(doc.render(), text);
    }

    #[test]
    fn parses_empty_sides() {
        let text = "<<<<<<< HEAD\n=======\n>>>>>>> feature\n";
        let doc = parse(text).unwrap();
        assert_eq!(doc.segments, vec![Segment::Conflict(hunk("", ""))]);
        assert_eq!(doc.render(), text);
    }

    #[test]
    fn parses_two_hunks_with_text_between() {
        let text =
            "<<<<<<< a\n1\n=======\n2\n>>>>>>> b\nmid\n<<<<<<< c\n3\n=======\n4\n>>>>>>> d\n";
        let doc = parse(text).unwrap();
        assert_eq!(doc.conflict_count(), 2);
        assert_eq!(doc.segments.len(), 3);
        assert_eq!(doc.segments[1], Segment::Text("mid\n".to_owned()));
        assert_eq!(doc.render(), text);
    }

    #[test]
    fn text_without_conflicts_is_one_segment() {
        let doc = parse("plain\ntext\n").unwrap();
        assert_eq!(
            doc.segments,
            vec![Segment::Text("plain\ntext\n".to_owned())]
        );
        assert_eq!(doc.conflict_count(), 0);
        assert!(doc.is_fully_resolved());
        assert_eq!(parse("").unwrap().segments, Vec::new());
    }

    #[test]
    fn rejects_malformed_input_with_the_line_number() {
        let unterminated = parse("a\n<<<<<<< HEAD\nours\n=======\ntheirs\n");
        assert_eq!(
            unterminated,
            Err(ParseError {
                line: 2,
                kind: ParseErrorKind::Unterminated
            })
        );

        let no_separator = parse("<<<<<<< HEAD\nours\n>>>>>>> feature\n");
        assert_eq!(
            no_separator,
            Err(ParseError {
                line: 3,
                kind: ParseErrorKind::MissingSeparator
            })
        );

        let stray = parse("ok\n=======\n");
        assert_eq!(
            stray,
            Err(ParseError {
                line: 2,
                kind: ParseErrorKind::UnexpectedMarker
            })
        );

        let nested = parse("<<<<<<< a\n<<<<<<< b\n=======\n>>>>>>> c\n");
        assert_eq!(
            nested,
            Err(ParseError {
                line: 1,
                kind: ParseErrorKind::Unterminated
            })
        );

        let twice = parse("<<<<<<< a\n|||||||\n|||||||\n=======\n>>>>>>> c\n");
        assert_eq!(
            twice,
            Err(ParseError {
                line: 3,
                kind: ParseErrorKind::UnexpectedMarker
            })
        );

        let message = unterminated.unwrap_err().to_string();
        assert!(message.starts_with("line 2: "));
    }

    #[test]
    fn a_marker_must_stand_alone_on_its_line() {
        // Text that merely starts with the marker characters is content.
        let text = "<<<<<<<x\n=======z\n";
        assert_eq!(parse(text).unwrap().render(), text);
    }

    #[test]
    fn counts_and_resolves_hunks_by_conflict_index() {
        let text =
            "<<<<<<< a\n1\n=======\n2\n>>>>>>> b\nmid\n<<<<<<< c\n3\n=======\n4\n>>>>>>> d\n";
        let mut doc = parse(text).unwrap();
        assert_eq!(doc.conflict_count(), 2);
        assert_eq!(doc.resolved_count(), 0);
        assert!(!doc.is_fully_resolved());

        doc.set_resolution(1, Some(Resolution::Right));
        assert_eq!(doc.resolved_count(), 1);
        doc.set_resolution(7, Some(Resolution::Left));
        assert_eq!(doc.resolved_count(), 1);

        doc.set_resolution(0, Some(Resolution::Left));
        assert!(doc.is_fully_resolved());
        doc.set_resolution(0, None);
        assert_eq!(doc.resolved_count(), 1);
    }

    #[test]
    fn renders_each_resolution() {
        let text = "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\nend\n";
        let render_with = |resolution| {
            let mut doc = parse(text).unwrap();
            doc.set_resolution(0, Some(resolution));
            doc.render()
        };
        assert_eq!(render_with(Resolution::Left), "ours\nend\n");
        assert_eq!(render_with(Resolution::Right), "theirs\nend\n");
        assert_eq!(render_with(Resolution::Both), "ours\ntheirs\nend\n");
        assert_eq!(
            render_with(Resolution::Custom("merged\n".to_owned())),
            "merged\nend\n"
        );
    }

    #[test]
    fn unresolved_hunks_render_as_markers_beside_resolved_ones() {
        let text =
            "<<<<<<< a\n1\n=======\n2\n>>>>>>> b\nmid\n<<<<<<< c\n3\n=======\n4\n>>>>>>> d\n";
        let mut doc = parse(text).unwrap();
        doc.set_resolution(0, Some(Resolution::Both));
        assert_eq!(
            doc.render(),
            "1\n2\nmid\n<<<<<<< c\n3\n=======\n4\n>>>>>>> d\n"
        );
    }

    #[test]
    fn a_hunk_with_a_resolution_reads_as_a_plain_hunk() {
        let mut doc = ConflictDoc {
            segments: vec![Segment::Conflict(hunk("a\n", "b\n"))],
        };
        doc.set_resolution(0, Some(Resolution::Custom("c\n".to_owned())));
        assert_eq!(doc.render(), "c\n");
        assert!(doc.is_fully_resolved());
    }

    fn line_strategy() -> impl Strategy<Value = String> {
        "[a-z ]{0,8}"
    }

    fn label_strategy() -> impl Strategy<Value = String> {
        "[A-Za-z0-9/._-]{0,10}"
    }

    proptest! {
        #[test]
        fn parse_never_panics_on_marker_like_text(text in "[<>=|a \r\n]{0,200}") {
            let _ = parse(&text);
        }

        #[test]
        fn generated_conflicts_round_trip_byte_exact(
            crlf in any::<bool>(),
            no_final_newline in any::<bool>(),
            hunks in prop::collection::vec(
                (
                    prop::collection::vec(line_strategy(), 0..4),
                    prop::option::of(prop::collection::vec(line_strategy(), 0..3)),
                    prop::collection::vec(line_strategy(), 0..4),
                    label_strategy(),
                    label_strategy(),
                    prop::collection::vec(line_strategy(), 0..3),
                ),
                0..4,
            ),
        ) {
            let eol = if crlf { "\r\n" } else { "\n" };
            let mut text = String::new();
            for (left, base, right, left_label, right_label, between) in &hunks {
                for line in between {
                    text.push_str(line);
                    text.push_str(eol);
                }
                push_marker(&mut text, OPEN, left_label, eol);
                for line in left {
                    text.push_str(line);
                    text.push_str(eol);
                }
                if let Some(base) = base {
                    push_marker(&mut text, BASE, "base", eol);
                    for line in base {
                        text.push_str(line);
                        text.push_str(eol);
                    }
                }
                push_marker(&mut text, SEPARATOR, "", eol);
                for line in right {
                    text.push_str(line);
                    text.push_str(eol);
                }
                push_marker(&mut text, CLOSE, right_label, eol);
            }
            if no_final_newline && text.ends_with(eol) {
                text.truncate(text.len() - eol.len());
            }
            let doc = parse(&text).unwrap();
            prop_assert_eq!(doc.conflict_count(), hunks.len());
            prop_assert_eq!(doc.render(), text);
        }
    }
}
