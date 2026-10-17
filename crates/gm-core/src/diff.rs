//! Parses unified diff output into `FileDiff`. The input is the output of
//! `git diff --no-color --no-ext-diff -U3 [--cached] -- <path>`, or of
//! `git diff --no-index -- /dev/null <path>` for an untracked file.

use gm_schema::{DiffLine, FileDiff, Hunk, HunkId, LineKind};

use crate::GitError;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// A one-hunk patch for `path`, in the form `git apply` reads back. Every line
/// keeps its kind, so reversing it removes exactly what the hunk added.
#[must_use]
pub fn hunk_patch(path: &str, hunk: &Hunk) -> String {
    let mut text = format!(
        "--- a/{path}\n+++ b/{path}\n@@ -{},{} +{},{} @@",
        hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
    );
    if !hunk.header.is_empty() {
        text.push(' ');
        text.push_str(&hunk.header);
    }
    text.push('\n');
    for line in &hunk.lines {
        let sign = match line.kind {
            LineKind::Context => ' ',
            LineKind::Added => '+',
            LineKind::Removed => '-',
        };
        text.push(sign);
        text.push_str(&line.text);
        text.push('\n');
        if line.no_newline {
            text.push_str("\\ No newline at end of file\n");
        }
    }
    text
}

/// Line counters of the hunk being read, from its header.
struct Open {
    old_left: u32,
    new_left: u32,
    old_no: u32,
    new_no: u32,
}

impl Open {
    /// `None` when the header announces no body lines at all.
    fn for_hunk(hunk: &Hunk) -> Option<Self> {
        let open = Self {
            old_left: hunk.old_lines,
            new_left: hunk.new_lines,
            old_no: hunk.old_start,
            new_no: hunk.new_start,
        };
        (!open.is_done()).then_some(open)
    }

    fn is_done(&self) -> bool {
        self.old_left == 0 && self.new_left == 0
    }
}

/// Parses one file's diff. `path` is the path of the result; the header's
/// path is not read, so paths with spaces or C-quoting need no handling.
///
/// Empty input yields a diff with no hunks. Binary diffs set `binary` and
/// carry no hunks.
pub fn parse_unified(path: &str, raw: &[u8]) -> Result<FileDiff, GitError> {
    let mut diff = FileDiff {
        path: path.to_owned(),
        old_path: None,
        binary: false,
        hunks: Vec::new(),
    };
    let mut lines: Vec<&[u8]> = raw.split(|b| *b == b'\n').collect();
    if lines.last().is_some_and(|last| last.is_empty()) {
        lines.pop();
    }

    let mut open: Option<Open> = None;
    for line in lines {
        if line.starts_with(b"\\") {
            // `\ No newline at end of file` applies to the line before it.
            if let Some(last) = diff.hunks.last_mut().and_then(|h| h.lines.last_mut()) {
                last.no_newline = true;
            }
        } else if let Some(state) = open.as_mut() {
            let hunk = diff
                .hunks
                .last_mut()
                .ok_or_else(|| GitError("hunk body without a header".into()))?;
            push_body_line(hunk, state, line)?;
            if state.is_done() {
                open = None;
            }
        } else if line.starts_with(b"@@") {
            let hunk = parse_hunk_header(line)?;
            open = Open::for_hunk(&hunk);
            diff.hunks.push(hunk);
        } else if line == b"GIT binary patch" || is_binary_files(line) {
            diff.binary = true;
            break;
        } else if let Some(old) = line.strip_prefix(b"rename from ") {
            diff.old_path = Some(unquote_c(&String::from_utf8_lossy(old)));
        }
        // Every other header line (diff, index, mode, ---/+++) is not needed.
    }
    if open.is_some() {
        return Err(GitError("diff ends inside a hunk".into()));
    }

    for hunk in &mut diff.hunks {
        hunk.id = hunk_id(&diff.path, hunk);
    }
    Ok(diff)
}

/// The id of a hunk (D21): FNV-1a over the path and the hunk's lines. Line
/// numbers and the header text are left out, so an edit above the hunk keeps
/// its id. The hash is written out by hand because `DefaultHasher` is not
/// stable across releases.
#[must_use]
pub fn hunk_id(path: &str, hunk: &Hunk) -> HunkId {
    let mut hash = FNV_OFFSET;
    fnv1a(&mut hash, path.as_bytes());
    fnv1a(&mut hash, &[0]);
    for line in &hunk.lines {
        fnv1a(&mut hash, &[kind_byte(line.kind)]);
        fnv1a(&mut hash, line.text.as_bytes());
        fnv1a(&mut hash, &[0, u8::from(line.no_newline)]);
    }
    HunkId(format!("h_{hash:016x}"))
}

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn kind_byte(kind: LineKind) -> u8 {
    match kind {
        LineKind::Context => b' ',
        LineKind::Added => b'+',
        LineKind::Removed => b'-',
    }
}

fn is_binary_files(line: &[u8]) -> bool {
    line.starts_with(b"Binary files ") && line.ends_with(b" differ")
}

/// Reads one body line of a hunk and checks it against the header counts.
fn push_body_line(hunk: &mut Hunk, open: &mut Open, line: &[u8]) -> Result<(), GitError> {
    let (kind, text) = match line.split_first() {
        Some((b'+', rest)) => (LineKind::Added, rest),
        Some((b'-', rest)) => (LineKind::Removed, rest),
        Some((b' ', rest)) => (LineKind::Context, rest),
        // Git writes an empty context line as " "; a stripped one is still context.
        None => (LineKind::Context, line),
        Some(_) => {
            return Err(GitError(format!(
                "unexpected line in hunk: {}",
                String::from_utf8_lossy(line)
            )));
        }
    };
    let fits = match kind {
        LineKind::Context => open.old_left > 0 && open.new_left > 0,
        LineKind::Removed => open.old_left > 0,
        LineKind::Added => open.new_left > 0,
    };
    if !fits {
        return Err(GitError(
            "hunk has more lines than its header counts".into(),
        ));
    }

    let takes_old = kind != LineKind::Added;
    let takes_new = kind != LineKind::Removed;
    let old_no = takes_old.then_some(open.old_no);
    let new_no = takes_new.then_some(open.new_no);
    if takes_old {
        open.old_no = open.old_no.saturating_add(1);
        open.old_left -= 1;
    }
    if takes_new {
        open.new_no = open.new_no.saturating_add(1);
        open.new_left -= 1;
    }
    hunk.lines.push(DiffLine {
        kind,
        old_no,
        new_no,
        text: String::from_utf8_lossy(text).into_owned(),
        no_newline: false,
    });
    Ok(())
}

/// Parses `@@ -a[,b] +c[,d] @@ header`. A missing count means 1.
fn parse_hunk_header(line: &[u8]) -> Result<Hunk, GitError> {
    let text = String::from_utf8_lossy(line);
    let bad = || GitError(format!("malformed hunk header: {text}"));
    let rest = text.strip_prefix("@@ -").ok_or_else(bad)?;
    let (old, rest) = rest.split_once(" +").ok_or_else(bad)?;
    let (new, rest) = rest.split_once(" @@").ok_or_else(bad)?;
    let header = match rest {
        "" => "",
        _ => rest.strip_prefix(' ').ok_or_else(bad)?,
    };
    let (old_start, old_lines) = parse_range(old).ok_or_else(bad)?;
    let (new_start, new_lines) = parse_range(new).ok_or_else(bad)?;
    Ok(Hunk {
        id: HunkId(String::new()),
        old_start,
        old_lines,
        new_start,
        new_lines,
        header: header.to_owned(),
        lines: Vec::new(),
    })
}

/// `start` or `start,count`; a missing count is 1.
fn parse_range(range: &str) -> Option<(u32, u32)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

/// Decodes a C-quoted path as git writes it for `rename from`: backslash
/// escapes and octal bytes. An unquoted value is returned as is.
fn unquote_c(value: &str) -> String {
    let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return value.to_owned();
    };
    let src = inner.as_bytes();
    let mut out = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i] != b'\\' || i + 1 >= src.len() {
            out.push(src[i]);
            i += 1;
            continue;
        }
        if let Some(digits) = src
            .get(i + 1..i + 4)
            .filter(|d| d.iter().all(|b| (b'0'..=b'7').contains(b)))
        {
            let value = digits
                .iter()
                .fold(0u32, |acc, d| acc * 8 + u32::from(d - b'0'));
            out.push(u8::try_from(value).unwrap_or(b'?'));
            i += 4;
            continue;
        }
        out.push(match src[i + 1] {
            b'a' => 0x07,
            b'b' => 0x08,
            b'f' => 0x0c,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 0x0b,
            other => other,
        });
        i += 2;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn line(kind: LineKind, old_no: Option<u32>, new_no: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_no,
            new_no,
            text: text.to_owned(),
            no_newline: false,
        }
    }

    fn ctx(old_no: u32, new_no: u32, text: &str) -> DiffLine {
        line(LineKind::Context, Some(old_no), Some(new_no), text)
    }

    fn del(old_no: u32, text: &str) -> DiffLine {
        line(LineKind::Removed, Some(old_no), None, text)
    }

    fn add(new_no: u32, text: &str) -> DiffLine {
        line(LineKind::Added, None, Some(new_no), text)
    }

    /// A hunk with its id computed the same way the parser does.
    fn hunk(
        path: &str,
        (old_start, old_lines): (u32, u32),
        (new_start, new_lines): (u32, u32),
        header: &str,
        lines: Vec<DiffLine>,
    ) -> Hunk {
        let mut hunk = Hunk {
            id: HunkId(String::new()),
            old_start,
            old_lines,
            new_start,
            new_lines,
            header: header.to_owned(),
            lines,
        };
        hunk.id = hunk_id(path, &hunk);
        hunk
    }

    fn file(path: &str, old_path: Option<&str>, hunks: Vec<Hunk>) -> FileDiff {
        FileDiff {
            path: path.to_owned(),
            old_path: old_path.map(str::to_owned),
            binary: false,
            hunks,
        }
    }

    #[test]
    fn empty_input_has_no_hunks() {
        assert_eq!(
            parse_unified("a.txt", b"").unwrap(),
            file("a.txt", None, vec![])
        );
    }

    #[test]
    fn modified_file_with_one_hunk() {
        let raw = b"diff --git a/f.txt b/f.txt\nindex 1111111..2222222 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@ fn main\n one\n-two\n+TWO\n three\n";
        let expected = file(
            "f.txt",
            None,
            vec![hunk(
                "f.txt",
                (1, 3),
                (1, 3),
                "fn main",
                vec![
                    ctx(1, 1, "one"),
                    del(2, "two"),
                    add(2, "TWO"),
                    ctx(3, 3, "three"),
                ],
            )],
        );
        assert_eq!(parse_unified("f.txt", raw).unwrap(), expected);
    }

    #[test]
    fn multi_hunk_file_is_exact() {
        let raw = b"diff --git a/m.txt b/m.txt\nindex 1..2 100644\n--- a/m.txt\n+++ b/m.txt\n@@ -1,2 +1,2 @@\n-a\n+A\n b\n@@ -10 +10,2 @@ tail\n j\n+k\n";
        let expected = file(
            "m.txt",
            None,
            vec![
                hunk(
                    "m.txt",
                    (1, 2),
                    (1, 2),
                    "",
                    vec![del(1, "a"), add(1, "A"), ctx(2, 2, "b")],
                ),
                hunk(
                    "m.txt",
                    (10, 1),
                    (10, 2),
                    "tail",
                    vec![ctx(10, 10, "j"), add(11, "k")],
                ),
            ],
        );
        assert_eq!(parse_unified("m.txt", raw).unwrap(), expected);
    }

    #[test]
    fn new_file_has_only_added_lines() {
        let raw = b"diff --git a/n.txt b/n.txt\nnew file mode 100644\nindex 0000000..3333333\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+x\n+y\n";
        let diff = parse_unified("n.txt", raw).unwrap();
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!((diff.hunks[0].old_start, diff.hunks[0].old_lines), (0, 0));
        assert_eq!(diff.hunks[0].lines, vec![add(1, "x"), add(2, "y")]);
    }

    #[test]
    fn deleted_file_has_only_removed_lines() {
        let raw = b"diff --git a/d.txt b/d.txt\ndeleted file mode 100644\nindex 4444444..0000000\n--- a/d.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-x\n-y\n";
        let diff = parse_unified("d.txt", raw).unwrap();
        assert_eq!((diff.hunks[0].new_start, diff.hunks[0].new_lines), (0, 0));
        assert_eq!(diff.hunks[0].lines, vec![del(1, "x"), del(2, "y")]);
    }

    #[test]
    fn omitted_counts_mean_one() {
        let raw = b"@@ -3 +3,0 @@\n-x\n@@ -5,0 +6 @@\n+y\n";
        let diff = parse_unified("c.txt", raw).unwrap();
        assert_eq!((diff.hunks[0].old_start, diff.hunks[0].old_lines), (3, 1));
        assert_eq!((diff.hunks[0].new_start, diff.hunks[0].new_lines), (3, 0));
        assert_eq!((diff.hunks[1].new_start, diff.hunks[1].new_lines), (6, 1));
        assert_eq!(diff.hunks[1].lines, vec![add(6, "y")]);
    }

    #[test]
    fn paths_with_spaces_and_quotes_use_the_argument() {
        let raw = b"diff --git \"a/x y\\tz.txt\" \"b/x y\\tz.txt\"\nindex 1..2 100644\n--- \"a/x y\\tz.txt\"\n+++ \"b/x y\\tz.txt\"\n@@ -1 +1 @@\n-a\n+b\n";
        let diff = parse_unified("x y\tz.txt", raw).unwrap();
        assert_eq!(diff.path, "x y\tz.txt");
        assert_eq!(diff.old_path, None);
        assert_eq!(diff.hunks[0].lines, vec![del(1, "a"), add(1, "b")]);
    }

    #[test]
    fn rename_with_edits_sets_old_path() {
        let raw = b"diff --git a/old name.txt b/new name.txt\nsimilarity index 80%\nrename from old name.txt\nrename to new name.txt\nindex 1..2 100644\n--- a/old name.txt\n+++ b/new name.txt\n@@ -1,2 +1,2 @@\n a\n-b\n+B\n";
        let diff = parse_unified("new name.txt", raw).unwrap();
        assert_eq!(diff.path, "new name.txt");
        assert_eq!(diff.old_path.as_deref(), Some("old name.txt"));
        assert_eq!(diff.hunks[0].lines.len(), 3);
    }

    #[test]
    fn quoted_rename_source_is_unquoted() {
        let raw = b"rename from \"caf\\303\\251\\tx.txt\"\nrename to new.txt\n";
        let diff = parse_unified("new.txt", raw).unwrap();
        assert_eq!(diff.old_path.as_deref(), Some("caf\u{e9}\tx.txt"));
    }

    #[test]
    fn binary_diff_has_no_hunks() {
        let raw = b"diff --git a/b.png b/b.png\nindex 1..2 100644\nBinary files a/b.png and b/b.png differ\n";
        let diff = parse_unified("b.png", raw).unwrap();
        assert!(diff.binary);
        assert!(diff.hunks.is_empty());
    }

    #[test]
    fn binary_patch_has_no_hunks() {
        let raw = b"diff --git a/b.bin b/b.bin\nindex 1..2\nGIT binary patch\nliteral 3\nKcmZQzU|?ilH~;_u1\n\n";
        let diff = parse_unified("b.bin", raw).unwrap();
        assert!(diff.binary);
        assert!(diff.hunks.is_empty());
    }

    #[test]
    fn no_newline_marks_the_removed_line() {
        let raw = b"@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n";
        let diff = parse_unified("n.txt", raw).unwrap();
        let lines = &diff.hunks[0].lines;
        assert!(lines[0].no_newline);
        assert!(!lines[1].no_newline);
    }

    #[test]
    fn no_newline_marks_the_added_line() {
        let raw = b"@@ -1 +1 @@\n-old\n+new\n\\ No newline at end of file\n";
        let diff = parse_unified("n.txt", raw).unwrap();
        let lines = &diff.hunks[0].lines;
        assert!(!lines[0].no_newline);
        assert!(lines[1].no_newline);
    }

    #[test]
    fn no_newline_on_both_sides() {
        let raw = b"@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n";
        let diff = parse_unified("n.txt", raw).unwrap();
        let lines = &diff.hunks[0].lines;
        assert!(lines[0].no_newline && lines[1].no_newline);
    }

    #[test]
    fn crlf_is_kept_in_the_text() {
        let raw = b"@@ -1 +1 @@\n-a\r\n+b\r\n";
        let diff = parse_unified("c.txt", raw).unwrap();
        assert_eq!(diff.hunks[0].lines[0].text, "a\r");
        assert_eq!(diff.hunks[0].lines[1].text, "b\r");
    }

    #[test]
    fn non_utf8_bytes_are_replaced() {
        let raw = b"@@ -1 +1 @@\n-\xff\n+b\n";
        let diff = parse_unified("c.txt", raw).unwrap();
        assert_eq!(diff.hunks[0].lines[0].text, "\u{fffd}");
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        let cases: [&[u8]; 6] = [
            b"@@ -x +1 @@\n",
            b"@@ garbage\n",
            b"@@ -1 +1\n",
            b"@@ -1,2 +1,2 @@\n-a\n",
            b"@@ -1 +1 @@\n-a\n-b\n",
            b"@@ -1 +1 @@x\n-a\n+b\n",
        ];
        for raw in cases {
            assert!(parse_unified("m.txt", raw).is_err(), "{raw:?}");
        }
    }

    #[test]
    fn hunk_id_ignores_line_numbers_and_header() {
        let near = parse_unified("f.txt", b"@@ -1 +1 @@\n-a\n+b\n").unwrap();
        let far = parse_unified("f.txt", b"@@ -50,1 +52,1 @@ ctx\n-a\n+b\n").unwrap();
        assert_eq!(near.hunks[0].id, far.hunks[0].id);
    }

    #[test]
    fn hunk_id_changes_with_one_character() {
        let a = parse_unified("f.txt", b"@@ -1 +1 @@\n-a\n+b\n").unwrap();
        let b = parse_unified("f.txt", b"@@ -1 +1 @@\n-a\n+c\n").unwrap();
        assert_ne!(a.hunks[0].id, b.hunks[0].id);
    }

    #[test]
    fn hunk_id_depends_on_path_and_no_newline() {
        let a = parse_unified("f.txt", b"@@ -1 +1 @@\n-a\n+b\n").unwrap();
        let other_path = parse_unified("g.txt", b"@@ -1 +1 @@\n-a\n+b\n").unwrap();
        let no_newline = parse_unified(
            "f.txt",
            b"@@ -1 +1 @@\n-a\n+b\n\\ No newline at end of file\n",
        )
        .unwrap();
        assert_ne!(a.hunks[0].id, other_path.hunks[0].id);
        assert_ne!(a.hunks[0].id, no_newline.hunks[0].id);
    }

    #[test]
    fn hunk_id_has_the_documented_shape() {
        let diff = parse_unified("f.txt", b"@@ -1 +1 @@\n-a\n+b\n").unwrap();
        let id = &diff.hunks[0].id.0;
        assert_eq!(id.len(), 18);
        assert!(id.starts_with("h_"));
        assert!(
            id[2..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    const FRAGMENTS: &[&str] = &[
        "diff --git a/x b/x\n",
        "index 1..2 100644\n",
        "rename from x\n",
        "rename to \"y\\303\"\n",
        "Binary files a and b differ\n",
        "GIT binary patch\n",
        "@@ -1,2 +1,2 @@ h\n",
        "@@ -4294967295,3 +4294967295 @@\n",
        "@@ -x @@\n",
        "@@\n",
        " a\n",
        "+b\n",
        "-c\n",
        "\\ No newline at end of file\n",
        "\n",
        "\t\r",
        "x",
    ];

    proptest! {
        #[test]
        fn parse_never_panics_on_arbitrary_bytes(raw in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = parse_unified("p.txt", &raw);
        }

        #[test]
        fn parse_never_panics_on_diff_like_lines(
            parts in proptest::collection::vec(proptest::sample::select(FRAGMENTS.to_vec()), 0..40)
        ) {
            let _ = parse_unified("p.txt", parts.concat().as_bytes());
        }
    }
}
