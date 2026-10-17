//! The diffs of every file of one commit (H4c): one read of the whole commit's
//! patch, split into one section per file. The split and the parse are pure;
//! the adapter starts git. The read runs on a thread of its own, and its answer
//! is dropped when a newer request superseded it.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gm_schema::{CommitFile, FileDiff, OperationId};

use crate::diff::parse_unified;
use crate::port::{GitError, StackRead};

/// One whole-commit read, run on a thread of its own. `stop` is shared with the
/// engine, which sets it when the read is superseded.
pub struct StackJob {
    pub id: OperationId,
    pub read: StackRead,
    pub stop: Arc<AtomicBool>,
}

/// What a stack job answers. A failure carries git's own message.
pub struct StackReport {
    pub id: OperationId,
    pub outcome: Result<Vec<FileDiff>, String>,
}

impl StackJob {
    /// Runs the read to its end, or to the stop request.
    #[must_use]
    pub fn run(self) -> StackReport {
        let outcome = (self.read)(&self.stop).map_err(|error| error.0);
        StackReport {
            id: self.id,
            outcome,
        }
    }
}

/// Splits the output of `git show --format= --patch` into one section per file.
/// A section starts at a line that begins with `diff --git `. No other line can
/// begin that way: every line of a hunk begins with ` `, `+`, `-` or `\`. Text
/// before the first section is dropped.
#[must_use]
pub fn split_sections(raw: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut at = 0;
    while at < raw.len() {
        if raw[at..].starts_with(b"diff --git ") {
            starts.push(at);
        }
        match raw[at..].iter().position(|byte| *byte == b'\n') {
            Some(end) => at += end + 1,
            None => break,
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(raw.len());
            &raw[start..end]
        })
        .collect()
}

/// Parses the whole-commit patch into one `FileDiff` per listed file, in the
/// order of `files`. Git's patch lists the files in the same order as its
/// `--raw` listing, so the first section belongs to the first file. The patch
/// may hold more files than `files` (the detail lists the first
/// `DETAIL_FILE_LIMIT`); those are not parsed.
pub fn parse_stack(files: &[CommitFile], raw: &[u8]) -> Result<Vec<FileDiff>, GitError> {
    let sections = split_sections(raw);
    if sections.len() < files.len() {
        return Err(GitError(format!(
            "git printed {} file sections for {} files",
            sections.len(),
            files.len()
        )));
    }
    files
        .iter()
        .zip(sections)
        .map(|(file, section)| parse_unified(&file.path, section))
        .collect()
}

#[cfg(test)]
mod tests {
    use gm_schema::Change;

    use super::*;

    const TWO_FILES: &str = "\
diff --git a/a.txt b/a.txt
index 1111111..2222222 100644
--- a/a.txt
+++ b/a.txt
@@ -1,2 +1,2 @@
 keep
-old
+new
diff --git a/b.txt b/b.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/b.txt
@@ -0,0 +1 @@
+diff --git a/x b/x
";

    fn file(path: &str) -> CommitFile {
        CommitFile {
            path: path.into(),
            old_path: None,
            change: Change::Modified,
            added: 1,
            deleted: 1,
            binary: false,
        }
    }

    #[test]
    fn each_file_starts_a_section_and_a_body_line_does_not() {
        let sections = split_sections(TWO_FILES.as_bytes());
        assert_eq!(sections.len(), 2);
        assert!(sections[0].starts_with(b"diff --git a/a.txt"));
        assert!(sections[1].starts_with(b"diff --git a/b.txt"));
        assert!(sections[1].ends_with(b"+diff --git a/x b/x\n"));
    }

    #[test]
    fn text_before_the_first_file_is_dropped() {
        let raw = format!("\n\n{TWO_FILES}");
        assert_eq!(split_sections(raw.as_bytes()).len(), 2);
        assert_eq!(split_sections(b""), Vec::<&[u8]>::new());
    }

    #[test]
    fn parse_stack_gives_each_listed_file_its_own_hunks_in_order() {
        let files = [file("a.txt"), file("b.txt")];
        let diffs = parse_stack(&files, TWO_FILES.as_bytes()).unwrap();
        assert_eq!(diffs.len(), 2);
        assert_eq!(diffs[0].path, "a.txt");
        assert_eq!(diffs[0].hunks.len(), 1);
        assert_eq!(diffs[1].path, "b.txt");
        assert_eq!(diffs[1].hunks.len(), 1);
        assert_eq!(diffs[1].hunks[0].lines[0].text, "diff --git a/x b/x");
    }

    #[test]
    fn parse_stack_stops_at_the_listed_files() {
        let diffs = parse_stack(&[file("a.txt")], TWO_FILES.as_bytes()).unwrap();
        assert_eq!(diffs.len(), 1);
    }

    #[test]
    fn parse_stack_refuses_a_patch_with_fewer_files_than_listed() {
        let files = [file("a.txt"), file("b.txt"), file("c.txt")];
        let error = parse_stack(&files, TWO_FILES.as_bytes()).unwrap_err();
        assert!(
            error.0.contains("2 file sections for 3 files"),
            "{}",
            error.0
        );
    }
}
