//! Provenance (D144): the pure rules that tie lines to edits and commits. Nothing here reads
//! Git or the store. The caller reads blame, the blob of each commit and the edits, and passes
//! the facts in; the rules say what they prove.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};

use gm_schema::threads::{Evidence, LineRun, Unverifiable};

use crate::blame::BlameLine;

/// The shortest run of significant lines that is a fact (D144). Measured on real threads (Q34,
/// `tests/provenance_measure.rs`): 3 keeps 46 of the 53 window line matches that 2 gives and
/// 12 of the 14 whole-file ones; 4 would keep 42 and 7.
pub const LINE_MATCH_MIN: usize = 3;
/// Commits one query shows (D144).
pub const COMMIT_LIMIT: usize = 10;
/// Threads one commit shows (D144).
pub const THREAD_LIMIT: usize = 5;
/// Edits of the queried path compared for one commit, newest first.
pub const EDIT_LIMIT: usize = 200;
/// Lines one query may ask for.
pub const RANGE_LIMIT: u32 = 1000;
/// Entries shown before and after an edit's entry.
pub const CONTEXT: u32 = 2;
/// Labels one thread of a provenance result shows: the second edge out (D144, D145).
pub const LABEL_LIMIT: usize = 10;
/// Linked threads one thread of a provenance result shows: the second edge out (D144, D143).
pub const LINK_LIMIT: usize = 10;

/// Keeps the first `limit` of `items` and says whether any were cut (D144). The caller fetches
/// one more than the limit, so the cut is known without a count.
pub fn cut<T>(items: &mut Vec<T>, limit: usize) -> bool {
    let cut = items.len() > limit;
    items.truncate(limit);
    cut
}

/// Characters that make a line brace-only, with the blank lines (D144): they do not count
/// toward a run.
const TRIVIAL: &str = "{}()[];,";
/// What redaction writes in place of a secret (D121).
const REDACTED: &str = "[REDACTED:";
/// What the size limit writes in place of the dropped middle of a patch.
const TRUNCATED: &str = "[... truncated ";

/// Whether a line does not count toward a run: blank, or brace-only. Whitespace is not
/// normalized anywhere else.
#[must_use]
pub fn is_trivial(text: &str) -> bool {
    text.chars()
        .all(|character| character.is_whitespace() || TRIVIAL.contains(character))
}

/// Checks a queried line range: from 1, not backwards, and at most `RANGE_LIMIT` lines.
pub fn check_range(from: u32, to: u32) -> Result<(), String> {
    if from == 0 || to < from {
        return Err(format!("not a line range: {from} to {to}"));
    }
    if to - from >= RANGE_LIMIT {
        return Err(format!("the range is longer than {RANGE_LIMIT} lines"));
    }
    Ok(())
}

/// A repository-relative path: plain components only, no `..`, not absolute.
pub fn check_path(path: &str) -> Result<(), String> {
    let plain = !path.is_empty()
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if plain {
        Ok(())
    } else {
        Err(format!("not a repository path: {path:?}"))
    }
}

/// Consecutive lines one commit owns under one path, with the lines that count toward a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub from: u32,
    pub to: u32,
    /// The lines of the block that are not trivial, in order.
    pub significant: Vec<String>,
}

/// One commit, under one path, and the queried lines it owns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub commit: String,
    pub path: String,
    pub author: String,
    pub author_at: i64,
    pub subject: String,
    /// The lines it owns, merged into runs, in order.
    pub runs: Vec<LineRun>,
    pub blocks: Vec<Block>,
}

/// The owners of the lines, in the order of their first line. Consecutive lines of one commit
/// and path form one block; a line of another owner, or a gap, ends it.
#[must_use]
pub fn owners(lines: &[BlameLine]) -> Vec<Owner> {
    let mut owners: Vec<Owner> = Vec::new();
    let mut index: HashMap<(&str, &str), usize> = HashMap::new();
    let mut previous: Option<&BlameLine> = None;
    for line in lines {
        let slot = *index
            .entry((line.commit.as_str(), line.path.as_str()))
            .or_insert_with(|| {
                owners.push(Owner {
                    commit: line.commit.clone(),
                    path: line.path.clone(),
                    author: line.author.clone(),
                    author_at: line.author_at,
                    subject: line.subject.clone(),
                    runs: Vec::new(),
                    blocks: Vec::new(),
                });
                owners.len() - 1
            });
        let owner = &mut owners[slot];
        match owner.runs.last_mut() {
            Some(run) if run.to + 1 == line.line => run.to = line.line,
            _ => owner.runs.push(LineRun {
                from: line.line,
                to: line.line,
            }),
        }
        let continues = previous.is_some_and(|before| {
            before.line + 1 == line.line && before.commit == line.commit && before.path == line.path
        });
        let trivial = is_trivial(&line.text);
        match owner.blocks.last_mut() {
            Some(block) if continues => {
                block.to = line.line;
                if !trivial {
                    block.significant.push(line.text.clone());
                }
            }
            _ => owner.blocks.push(Block {
                from: line.line,
                to: line.line,
                significant: if trivial {
                    Vec::new()
                } else {
                    vec![line.text.clone()]
                },
            }),
        }
        previous = Some(line);
    }
    owners
}

/// What the store holds of one edit of a path, before the rules read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditRow {
    pub edit_id: i64,
    pub thread_uid: String,
    /// The sequence number of the entry that carried the edit.
    pub seq: u32,
    /// The path as the edit recorded it.
    pub path: String,
    pub at: i64,
    pub post_oid: Option<String>,
    pub from_input: bool,
    pub patch: String,
}

/// An edit as the rules compare it: its blob, why it cannot be compared, and its added lines,
/// split where a context line or a hunk breaks the file's lines apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditFact {
    pub edit_id: i64,
    pub thread_uid: String,
    pub seq: u32,
    pub path: String,
    pub at: i64,
    pub post_oid: Option<String>,
    /// Set when the edit cannot be compared with the lines (D144).
    pub unverifiable: Option<Unverifiable>,
    /// The added lines of each run of the patch that is contiguous in the new file.
    pub segments: Vec<Vec<String>>,
}

impl EditFact {
    #[must_use]
    pub fn new(row: EditRow) -> Self {
        let unverifiable = if row.from_input {
            Some(Unverifiable::FromInput)
        } else if row.patch.contains(REDACTED) {
            Some(Unverifiable::Redacted)
        } else if row.patch.contains(TRUNCATED) {
            Some(Unverifiable::Truncated)
        } else {
            None
        };
        Self {
            edit_id: row.edit_id,
            thread_uid: row.thread_uid,
            seq: row.seq,
            path: row.path,
            at: row.at,
            post_oid: row.post_oid,
            unverifiable,
            segments: added_segments(&row.patch),
        }
    }
}

/// The added lines of a unified diff, in runs. A context line or a hunk header ends a run;
/// a removed line does not, because it is not in the new file. A patch without a hunk, such as
/// the note of an edit recorded from the tool input, has no runs.
#[must_use]
pub fn added_segments(patch: &str) -> Vec<Vec<String>> {
    let mut segments = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut in_hunk = false;
    for row in patch.split('\n') {
        if row.starts_with("@@") {
            in_hunk = true;
            close(&mut segments, &mut current);
        } else if in_hunk {
            if let Some(text) = row.strip_prefix('+') {
                current.push(text.to_owned());
            } else if !row.starts_with('-') {
                close(&mut segments, &mut current);
            }
        }
    }
    close(&mut segments, &mut current);
    segments
}

fn close(segments: &mut Vec<Vec<String>>, current: &mut Vec<String>) {
    if !current.is_empty() {
        segments.push(std::mem::take(current));
    }
}

/// Whether the run of significant lines appears contiguously among the significant lines of one
/// segment (D144). The text is compared exactly. A run shorter than `LINE_MATCH_MIN` is never a fact.
#[must_use]
pub fn run_matches(run: &[String], segment: &[String]) -> bool {
    run_matches_min(run, segment, LINE_MATCH_MIN)
}

/// `run_matches` with the shortest run given by the caller. The engine uses `LINE_MATCH_MIN`;
/// the measurement of Q34 varies it.
#[must_use]
pub fn run_matches_min(run: &[String], segment: &[String], min_run: usize) -> bool {
    if run.len() < min_run {
        return false;
    }
    let kept: Vec<&String> = segment.iter().filter(|line| !is_trivial(line)).collect();
    kept.windows(run.len())
        .any(|window| window.iter().zip(run).all(|(line, wanted)| *line == wanted))
}

/// What the rules prove about one edit and one owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// The evidence, strongest first.
    pub evidence: Vec<Evidence>,
    /// Set when the edit could have matched but cannot be compared, so no match is claimed.
    pub unverifiable: Option<Unverifiable>,
}

impl Verdict {
    /// Whether the edit explains the lines by content: a blob or a line match (D144).
    #[must_use]
    pub fn decisive(&self) -> bool {
        self.evidence
            .iter()
            .any(|evidence| matches!(evidence, Evidence::BlobMatch | Evidence::LineMatch))
    }
}

/// The evidence one edit gives one owner (D144). `blob` is the blob of the owner's path in the
/// owner's commit. An edit made after the commit's author date is never a line match.
#[must_use]
pub fn judge(owner: &Owner, blob: Option<&str>, edit: &EditFact) -> Verdict {
    judge_with(owner, blob, edit, LINE_MATCH_MIN)
}

/// `judge` with the shortest run for a line match given by the caller (see `run_matches_min`).
#[must_use]
pub fn judge_with(owner: &Owner, blob: Option<&str>, edit: &EditFact, min_run: usize) -> Verdict {
    let mut evidence = Vec::new();
    let blob_match = blob.is_some() && blob == edit.post_oid.as_deref();
    if blob_match {
        evidence.push(Evidence::BlobMatch);
    }
    let mut unverifiable = None;
    if edit.at > owner.author_at {
        // Made after the commit: it cannot have written these lines.
    } else if let Some(reason) = edit.unverifiable {
        if !blob_match {
            unverifiable = Some(reason);
        }
    } else {
        let line_match = owner.blocks.iter().any(|block| {
            edit.segments
                .iter()
                .any(|segment| run_matches_min(&block.significant, segment, min_run))
        });
        if line_match {
            evidence.push(Evidence::LineMatch);
        }
        let added: HashSet<&str> = edit
            .segments
            .iter()
            .flatten()
            .filter(|line| !is_trivial(line))
            .map(String::as_str)
            .collect();
        let shares_a_line = owner.blocks.iter().any(|block| {
            block
                .significant
                .iter()
                .any(|line| added.contains(line.as_str()))
        });
        if shares_a_line {
            evidence.push(Evidence::HunkOverlap);
        }
    }
    evidence.sort();
    Verdict {
        evidence,
        unverifiable,
    }
}

/// Sorts evidence strongest first and drops repeats.
pub fn strongest_first(evidence: &mut Vec<Evidence>) {
    evidence.sort();
    evidence.dedup();
}

/// The order of threads in a result (D144): the strongest evidence first, then the most recent
/// thread. Threads with no evidence come last.
#[must_use]
pub fn thread_rank(evidence: &[Evidence], last_event_at: i64) -> (u8, Reverse<i64>) {
    let strength = evidence.first().map_or(u8::MAX, |first| *first as u8);
    (strength, Reverse(last_event_at))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn blame(line: u32, commit: &str, path: &str, text: &str) -> BlameLine {
        BlameLine {
            line,
            commit: commit.to_owned(),
            path: path.to_owned(),
            author: "Ana".to_owned(),
            author_at: 1_000,
            subject: "subject".to_owned(),
            text: text.to_owned(),
        }
    }

    fn row(patch: &str) -> EditRow {
        EditRow {
            edit_id: 1,
            thread_uid: "t".to_owned(),
            seq: 3,
            path: "src/a.rs".to_owned(),
            at: 500,
            post_oid: None,
            from_input: false,
            patch: patch.to_owned(),
        }
    }

    fn fact(patch: &str) -> EditFact {
        EditFact::new(row(patch))
    }

    fn lines(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|text| (*text).to_owned()).collect()
    }

    /// Three owned lines of `c1` in `a.rs`, numbered from 1.
    fn owned_by_c1(texts: &[&str]) -> Vec<BlameLine> {
        texts
            .iter()
            .enumerate()
            .map(|(index, text)| blame(u32::try_from(index + 1).unwrap(), "c1", "a.rs", text))
            .collect()
    }

    #[test]
    fn cut_keeps_the_first_items_and_says_when_it_cut() {
        let mut short = vec![1, 2];
        assert!(!cut(&mut short, 2));
        assert_eq!(short, vec![1, 2]);

        let mut long = vec![1, 2, 3];
        assert!(cut(&mut long, 2));
        assert_eq!(long, vec![1, 2]);

        let mut none: Vec<u8> = Vec::new();
        assert!(!cut(&mut none, 0));
    }

    const BODY: [&str; 3] = ["let a = 1;", "let b = 2;", "let c = 3;"];

    #[test]
    fn trivial_lines_are_blank_or_brace_only() {
        assert!(is_trivial(""));
        assert!(is_trivial("   \t"));
        assert!(is_trivial("    }"));
        assert!(is_trivial("});"));
        assert!(!is_trivial("    }  // end"));
        assert!(!is_trivial("let x = 1;"));
    }

    #[test]
    fn ranges_and_paths_are_checked() {
        assert!(check_range(1, 1).is_ok());
        assert!(check_range(1, 1000).is_ok());
        assert!(check_range(1, 1001).is_err());
        assert!(check_range(0, 3).is_err());
        assert!(check_range(5, 4).is_err());
        assert!(check_path("src/a.rs").is_ok());
        assert!(check_path("").is_err());
        assert!(check_path("/abs/a.rs").is_err());
        assert!(check_path("../a.rs").is_err());
        assert!(check_path("./a.rs").is_err());
    }

    #[test]
    fn owners_split_on_commit_path_and_gaps() {
        let mut input = owned_by_c1(&BODY);
        input.push(blame(4, "c2", "a.rs", "fn other() {}"));
        input.push(blame(5, "c1", "a.rs", "let d = 4;"));
        input.push(blame(6, "c1", "moved.rs", "let e = 5;"));
        let found = owners(&input);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].commit, "c1");
        assert_eq!(
            found[0].runs,
            vec![LineRun { from: 1, to: 3 }, LineRun { from: 5, to: 5 }]
        );
        assert_eq!(found[0].blocks.len(), 2);
        assert_eq!(found[0].blocks[0].significant.len(), 3);
        assert_eq!(found[1].commit, "c2");
        assert_eq!(found[2].path, "moved.rs");
    }

    #[test]
    fn a_blank_line_inside_a_block_does_not_count() {
        let input = owned_by_c1(&["let a = 1;", "", "}", "let b = 2;"]);
        let found = owners(&input);
        assert_eq!(found[0].blocks.len(), 1);
        assert_eq!(
            found[0].blocks[0].significant,
            lines(&["let a = 1;", "let b = 2;"])
        );
    }

    #[test]
    fn a_contiguous_run_of_three_lines_in_one_segment_is_a_line_match() {
        let patch = "@@ -0,0 +1,3 @@\n+let a = 1;\n+let b = 2;\n+let c = 3;\n";
        let found = owners(&owned_by_c1(&BODY));
        let edit = fact(patch);
        let verdict = judge(&found[0], None, &edit);
        assert_eq!(
            verdict.evidence,
            vec![Evidence::LineMatch, Evidence::HunkOverlap]
        );
        assert!(verdict.decisive());
    }

    #[test]
    fn a_run_shorter_than_the_minimum_is_not_a_line_match() {
        let patch = "@@ -0,0 +1,2 @@\n+let a = 1;\n+let b = 2;\n";
        let found = owners(&owned_by_c1(&["let a = 1;", "let b = 2;"]));
        let verdict = judge(&found[0], None, &fact(patch));
        assert_eq!(verdict.evidence, vec![Evidence::HunkOverlap]);
        assert!(!verdict.decisive());
    }

    #[test]
    fn a_context_line_breaks_the_run_in_the_edit() {
        let patch = "@@ -1,5 +1,5 @@\n+let a = 1;\n+let b = 2;\n let kept = 0;\n+let c = 3;\n";
        let found = owners(&owned_by_c1(&BODY));
        let verdict = judge(&found[0], None, &fact(patch));
        assert!(!verdict.evidence.contains(&Evidence::LineMatch));
    }

    #[test]
    fn a_formatter_that_changed_a_space_breaks_the_match() {
        let patch = "@@ -0,0 +1,3 @@\n+let a = 1;\n+let b = 2;\n+let c = 3;\n";
        let formatted = ["let a = 1;", "let b  = 2;", "let c = 3;"];
        let found = owners(&owned_by_c1(&formatted));
        let verdict = judge(&found[0], None, &fact(patch));
        assert!(!verdict.evidence.contains(&Evidence::LineMatch));
    }

    #[test]
    fn a_blob_match_is_the_strongest_evidence() {
        let patch = "@@ -0,0 +1,3 @@\n+let a = 1;\n+let b = 2;\n+let c = 3;\n";
        let found = owners(&owned_by_c1(&BODY));
        let mut edit = fact(patch);
        edit.post_oid = Some("blob-1".to_owned());
        let verdict = judge(&found[0], Some("blob-1"), &edit);
        assert_eq!(verdict.evidence[0], Evidence::BlobMatch);
        let other = judge(&found[0], Some("blob-2"), &edit);
        assert!(!other.evidence.contains(&Evidence::BlobMatch));
    }

    #[test]
    fn an_edit_after_the_author_date_never_line_matches() {
        let patch = "@@ -0,0 +1,3 @@\n+let a = 1;\n+let b = 2;\n+let c = 3;\n";
        let found = owners(&owned_by_c1(&BODY));
        let mut late = row(patch);
        late.at = 2_000;
        let verdict = judge(&found[0], None, &EditFact::new(late));
        assert!(verdict.evidence.is_empty());
    }

    #[test]
    fn a_redacted_edit_is_unverifiable_and_not_no_match() {
        let patch = "@@ -0,0 +1,3 @@\n+let a = 1;\n+[REDACTED:api-key]\n+let c = 3;\n";
        let found = owners(&owned_by_c1(&BODY));
        let verdict = judge(&found[0], None, &fact(patch));
        assert!(verdict.evidence.is_empty());
        assert_eq!(verdict.unverifiable, Some(Unverifiable::Redacted));
    }

    #[test]
    fn a_truncated_edit_is_unverifiable() {
        let patch = "@@ -0,0 +1,3 @@\n+let a = 1;\n[... truncated 900 bytes ...]\n+let c = 3;\n";
        let found = owners(&owned_by_c1(&BODY));
        let verdict = judge(&found[0], None, &fact(patch));
        assert_eq!(verdict.unverifiable, Some(Unverifiable::Truncated));
    }

    #[test]
    fn an_edit_from_the_tool_input_is_unverifiable() {
        let mut input = row("+let a = 1;\n+let b = 2;\n+let c = 3;\n");
        input.from_input = true;
        let found = owners(&owned_by_c1(&BODY));
        let verdict = judge(&found[0], None, &EditFact::new(input));
        assert_eq!(verdict.unverifiable, Some(Unverifiable::FromInput));
        assert!(verdict.evidence.is_empty());
    }

    #[test]
    fn a_verified_blob_match_hides_the_unverifiable_reason() {
        let patch = "@@ -0,0 +1 @@\n+[REDACTED:x]\n";
        let mut edit = fact(patch);
        edit.post_oid = Some("blob-1".to_owned());
        let found = owners(&owned_by_c1(&BODY));
        let verdict = judge(&found[0], Some("blob-1"), &edit);
        assert_eq!(verdict.unverifiable, None);
        assert_eq!(verdict.evidence, vec![Evidence::BlobMatch]);
    }

    #[test]
    fn evidence_sorts_strongest_first() {
        let mut evidence = vec![
            Evidence::Manual,
            Evidence::LineMatch,
            Evidence::BlobMatch,
            Evidence::LineMatch,
        ];
        strongest_first(&mut evidence);
        assert_eq!(
            evidence,
            vec![Evidence::BlobMatch, Evidence::LineMatch, Evidence::Manual]
        );
    }

    #[test]
    fn threads_rank_by_evidence_then_recency() {
        let line = thread_rank(&[Evidence::LineMatch], 1);
        let observed_newer = thread_rank(&[Evidence::ObservedCommit], 9);
        let none = thread_rank(&[], 9);
        assert!(line < observed_newer);
        assert!(observed_newer < none);
        assert!(thread_rank(&[Evidence::LineMatch], 9) < thread_rank(&[Evidence::LineMatch], 1));
    }

    proptest! {
        #[test]
        fn a_run_taken_from_the_edit_always_matches(
            body in proptest::collection::vec("[a-z]{1,8}", LINE_MATCH_MIN..12),
            gap in 0_usize..4,
            start in 0_usize..3,
        ) {
            let mut segment = vec![String::new(); start];
            for text in &body {
                segment.push(text.clone());
                segment.extend(std::iter::repeat_n("}".to_owned(), gap));
            }
            prop_assert!(run_matches(&body, &segment));
        }

        #[test]
        fn a_changed_character_breaks_every_match(
            body in proptest::collection::vec("[a-z]{1,8}", LINE_MATCH_MIN..8),
            index in 0_usize..8,
        ) {
            let mut segment = body.clone();
            let target = index % segment.len();
            segment[target].push('x');
            prop_assert!(!run_matches(&body, &segment));
        }

        #[test]
        fn blank_and_brace_lines_are_never_significant(text in "[ \t{}();,]{0,12}") {
            prop_assert!(is_trivial(&text));
        }

        #[test]
        fn the_rank_puts_stronger_evidence_first_whatever_the_recency(
            a in 0_i64..1_000,
            b in 0_i64..1_000,
        ) {
            prop_assert!(thread_rank(&[Evidence::BlobMatch], a) < thread_rank(&[Evidence::Manual], b));
        }
    }
}
