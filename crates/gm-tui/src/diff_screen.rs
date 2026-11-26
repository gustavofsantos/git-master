//! The Diff and File screens (D241, M4). Diff shows the branch against its base (D237) or
//! one commit, read-only, with the review threads under their lines as on Status. File
//! shows a whole file of the working tree, colored, with the lines not yet committed marked
//! and the review threads under their lines (D258).

use gm_schema::{FileDiff, Hunk, LineKind};
use gm_view::review::{self, Entry, Opened};

use crate::data::{Group, Repo};
use crate::nav::sections::Section;
use crate::status::{Item, LineChange, Mark, file_children, files_of};

/// What a Diff screen shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffKind {
    /// The branch against its base.
    Branch,
    /// One commit, by its full id.
    Commit(String),
}

impl DiffKind {
    pub fn group(&self) -> Group {
        match self {
            DiffKind::Branch => Group::Branch,
            DiffKind::Commit(_) => Group::Commit,
        }
    }

    pub fn address(&self) -> String {
        match self {
            DiffKind::Branch => "diff/branch".to_owned(),
            DiffKind::Commit(oid) => format!("diff/{}", &oid[..oid.len().min(7)]),
        }
    }
}

/// The Diff screen's sections: what it compares, then each file unfolded into its hunks.
pub fn diff(repo: &Repo, kind: &DiffKind, opened: Opened<'_>, split: bool) -> Vec<Section<Item>> {
    let header = |label, text: String| Section::leaf(label, Item::Header { label, text });
    let note = |id: &str, text: &str| Section::leaf(id, Item::Note(text.to_owned()));
    let mut out = Vec::new();
    match kind {
        DiffKind::Branch => match (&repo.range, &repo.range_error) {
            (Some(range), _) => out.push(header(
                "Diff:",
                format!(
                    "{} against {} · {} files +{} -{}",
                    range.to_name,
                    range.from_name,
                    range.files.len() + range.more_files,
                    range.added,
                    range.deleted
                ),
            )),
            (None, Some(reason)) => return vec![note("reason", reason)],
            (None, None) => return vec![note("reading", "Reading the branch…")],
        },
        DiffKind::Commit(oid) => match repo.commit.as_ref().filter(|c| &c.oid == oid) {
            Some(commit) => {
                out.push(header(
                    "Commit:",
                    format!("{} {}", commit.short, commit.subject),
                ));
                out.push(header(
                    "Author:",
                    format!("{} · {}", commit.author_name, commit.author_date),
                ));
                for (index, line) in commit.body.lines().enumerate() {
                    out.push(Section::leaf(
                        format!("body/{index}"),
                        Item::Note(line.to_owned()),
                    ));
                }
            }
            None => return vec![note("reading", "Reading the commit…")],
        },
    }
    let group = kind.group();
    let files = files_of(repo, group);
    if files.is_empty() {
        out.push(note("empty", "No file changed."));
    }
    for (path, letter, counts) in files {
        let file = repo.files.get(&(group, path.clone()));
        out.push(Section::with(
            format!("file/{group:?}/{path}"),
            Item::File {
                group,
                path: path.clone(),
                letter,
                counts,
                threads: 0,
            },
            file_children(repo, group, &path, file, opened, split),
        ));
    }
    out
}

/// The line of the working tree that line `n` of the index is at, through the unstaged
/// diff; `None` when the working tree removed it.
fn index_to_worktree(n: u32, unstaged: Option<&FileDiff>) -> Option<u32> {
    let mut offset: i64 = 0;
    for hunk in unstaged.map_or(&[][..], |diff| diff.hunks.as_slice()) {
        let begin = if hunk.old_lines == 0 {
            hunk.old_start + 1
        } else {
            hunk.old_start
        };
        let end = begin + hunk.old_lines;
        if n < begin {
            break;
        }
        if n < end {
            return hunk
                .lines
                .iter()
                .find(|line| line.old_no == Some(n))
                .and_then(|line| line.new_no);
        }
        offset += i64::from(hunk.new_lines) - i64::from(hunk.old_lines);
    }
    u32::try_from(i64::from(n) + offset).ok()
}

/// What line `line_index` of `hunk` is in the shared gutter (D262): in its run of removed and
/// added lines, the first added lines replace the removed ones (modified) and the rest are new
/// (added); a removed line is removed; a context line is `None`.
pub fn run_change(hunk: &Hunk, line_index: usize) -> Option<LineChange> {
    let line = hunk.lines.get(line_index)?;
    match line.kind {
        LineKind::Context => None,
        LineKind::Removed => Some(LineChange::Removed),
        LineKind::Added => {
            let start = hunk.lines[..line_index]
                .iter()
                .rposition(|l| l.kind == LineKind::Context)
                .map_or(0, |at| at + 1);
            let run = &hunk.lines[start..];
            let run = &run[..run
                .iter()
                .position(|l| l.kind == LineKind::Context)
                .unwrap_or(run.len())];
            let removed = run.iter().filter(|l| l.kind == LineKind::Removed).count();
            let before = hunk.lines[start..line_index]
                .iter()
                .filter(|l| l.kind == LineKind::Added)
                .count();
            Some(if before < removed {
                LineChange::Modified
            } else {
                LineChange::Added
            })
        }
    }
}

/// What `diff` changed, by line of its new side: in each run of removed and added lines, the
/// first added lines replace the removed ones (modified) and the rest are new (added), as an
/// editor's sign column counts them; a run that only removes marks the line before it, or the
/// line after it at the top of the file.
fn line_changes(diff: &FileDiff) -> Vec<(u32, LineChange)> {
    let mut out = Vec::new();
    for hunk in &diff.hunks {
        let mut before: Option<u32> = None;
        let mut removed = 0usize;
        let mut added: Vec<u32> = Vec::new();
        let mut flush =
            |removed: &mut usize, added: &mut Vec<u32>, before: Option<u32>, after: Option<u32>| {
                if added.is_empty() {
                    if *removed > 0 {
                        match (before, after) {
                            (Some(line), _) => out.push((line, LineChange::RemovedBelow)),
                            (None, Some(line)) => out.push((line, LineChange::RemovedAbove)),
                            // A hunk with no line left on the new side: the line before it.
                            (None, None) if hunk.new_start > 0 => {
                                out.push((hunk.new_start, LineChange::RemovedBelow));
                            }
                            (None, None) => {}
                        }
                    }
                } else {
                    for (at, &line) in added.iter().enumerate() {
                        let kind = if at < *removed {
                            LineChange::Modified
                        } else {
                            LineChange::Added
                        };
                        out.push((line, kind));
                    }
                }
                *removed = 0;
                added.clear();
            };
        for line in &hunk.lines {
            match line.kind {
                LineKind::Removed => removed += 1,
                LineKind::Added => added.extend(line.new_no),
                LineKind::Context => {
                    let after = line.new_no;
                    let last = added.last().copied();
                    flush(&mut removed, &mut added, before, after);
                    before = after.or(last.or(before));
                }
            }
        }
        flush(&mut removed, &mut added, before, None);
    }
    out
}

/// The marks of the File screen's lines: the staged changes, carried from the index through
/// the unstaged hunks, then the unstaged ones over them. A removal only marks a line nothing
/// else marks.
fn marks(repo: &Repo, path: &str, count: usize) -> Vec<Mark> {
    let mut marks = vec![Mark::Same; count];
    let diff = |group| {
        repo.files
            .get(&(group, path.to_owned()))
            .and_then(|file| file.diff.as_ref())
    };
    let unstaged = diff(Group::Unstaged);
    let mut set = |number: u32, kind: LineChange, staged: bool| {
        let Some(slot) = (number as usize)
            .checked_sub(1)
            .and_then(|i| marks.get_mut(i))
        else {
            return;
        };
        let removal = matches!(kind, LineChange::RemovedBelow | LineChange::RemovedAbove);
        let taken = matches!(
            slot,
            Mark::Changed {
                kind: LineChange::Added | LineChange::Modified,
                ..
            }
        );
        if !(removal && taken) {
            *slot = Mark::Changed { kind, staged };
        }
    };
    for (number, kind) in diff(Group::Staged).map_or_default(line_changes) {
        if let Some(at) = index_to_worktree(number, unstaged) {
            set(at, kind, true);
        }
    }
    for (number, kind) in unstaged.map_or_default(line_changes) {
        set(number, kind, false);
    }
    marks
}

/// The File screen's sections: every line of the file, none folded away, with the threads
/// ending on a line as its children and the outdated ones above (D258 (8)). The path is on
/// the mode line, so there is no header.
pub fn file(repo: &Repo, path: &str, opened: Opened) -> Vec<Section<Item>> {
    let note = |text: &str| Section::leaf("note", Item::Note(text.to_owned()));
    let Some(file) = repo.file.as_ref().filter(|file| file.path == path) else {
        return vec![note("Reading the file…")];
    };
    if file.binary && crate::images::is_image(path) {
        return vec![Section::leaf(
            "image",
            Item::Image((path.to_owned(), gm_schema::ImageAt::Worktree)),
        )];
    }
    if let Some(text) = &file.note {
        return vec![note(text)];
    }
    let marks = marks(repo, path, file.lines.len());
    let notes = review::file_notes(file.lines.len(), &repo.comments, path, opened);
    let mut cards = 0usize;
    // A file's notes hold cards and headings only; a diff row has no place here.
    let mut section = |entry: Entry| {
        cards += 1;
        match entry {
            Entry::Card { id, part } => Some(Section::leaf(
                format!("card/{id}/{cards}"),
                Item::Card { id, part },
            )),
            Entry::Heading(heading) => Some(Section::leaf(
                format!("heading/{heading:?}"),
                Item::Heading(heading),
            )),
            Entry::Diff(_) => None,
        }
    };
    let mut out: Vec<Section<Item>> = notes.top.into_iter().filter_map(&mut section).collect();
    for (index, &mark) in marks.iter().enumerate() {
        let mut row = Section::leaf(
            format!("line/{}", index + 1),
            Item::FileLine { index, mark },
        );
        // The threads ending on a line are its children, so `Tab` folds them (D258 (7)).
        if let Some(entries) = notes.after.get(&index) {
            row.children = entries.iter().cloned().filter_map(&mut section).collect();
        }
        out.push(row);
    }
    if file.truncated {
        out.push(Section::leaf(
            "truncated",
            Item::Note("The file is larger: only its first part is shown.".to_owned()),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{DiffLine, Hunk, HunkId};

    fn hunk(old: (u32, u32), new: (u32, u32), lines: Vec<DiffLine>) -> Hunk {
        Hunk {
            id: HunkId(String::new()),
            old_start: old.0,
            old_lines: old.1,
            new_start: new.0,
            new_lines: new.1,
            header: String::new(),
            lines,
        }
    }

    fn line(kind: LineKind, old_no: Option<u32>, new_no: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            old_no,
            new_no,
            text: String::new(),
            no_newline: false,
        }
    }

    #[test]
    fn a_diff_line_is_modified_added_or_removed_by_its_run() {
        use LineKind::{Added as A, Context as C, Removed as R};
        let h = hunk(
            (1, 4),
            (1, 4),
            vec![
                line(C, Some(1), Some(1)),
                line(R, Some(2), None),
                line(A, None, Some(2)),
                line(A, None, Some(3)),
                line(C, Some(3), Some(4)),
                line(A, None, Some(5)),
            ],
        );
        let kinds: Vec<Option<LineChange>> =
            (0..h.lines.len()).map(|i| run_change(&h, i)).collect();
        assert_eq!(
            kinds,
            [
                None,
                Some(LineChange::Removed),
                Some(LineChange::Modified),
                Some(LineChange::Added),
                None,
                Some(LineChange::Added)
            ]
        );
    }

    #[test]
    fn a_run_counts_its_first_added_lines_as_modified_and_a_removal_marks_its_edge() {
        use LineChange::{Added, Modified, RemovedAbove, RemovedBelow};
        use LineKind::{Added as A, Context as C, Removed as R};
        let diff = FileDiff {
            path: "a".to_owned(),
            old_path: None,
            binary: false,
            hunks: vec![
                // Line 1 removed at the top: line 1 now is the old line 2.
                hunk(
                    (1, 2),
                    (1, 1),
                    vec![line(R, Some(1), None), line(C, Some(2), Some(1))],
                ),
                // Two lines replaced by three: two modified, one added.
                hunk(
                    (5, 3),
                    (4, 4),
                    vec![
                        line(C, Some(5), Some(4)),
                        line(R, Some(6), None),
                        line(R, Some(7), None),
                        line(A, None, Some(5)),
                        line(A, None, Some(6)),
                        line(A, None, Some(7)),
                    ],
                ),
                // A line removed after line 20.
                hunk(
                    (22, 2),
                    (20, 1),
                    vec![line(C, Some(22), Some(20)), line(R, Some(23), None)],
                ),
            ],
        };
        assert_eq!(
            line_changes(&diff),
            [
                (1, RemovedAbove),
                (5, Modified),
                (6, Modified),
                (7, Added),
                (20, RemovedBelow)
            ]
        );
    }

    #[test]
    fn an_unstaged_change_wins_over_a_staged_one_and_a_removal_marks_only_a_free_line() {
        use crate::data::FileData;
        use LineKind::{Added as A, Context as C, Removed as R};
        let mut repo = Repo::default();
        let mut put = |group: Group, hunks: Vec<Hunk>| {
            let mut file = FileData::default();
            file.load(
                FileDiff {
                    path: "a".to_owned(),
                    old_path: None,
                    binary: false,
                    hunks,
                },
                group.side(),
            );
            repo.files.insert((group, "a".to_owned()), file);
        };
        // Staged: line 2 added; a line removed after line 4.
        put(
            Group::Staged,
            vec![
                hunk(
                    (1, 1),
                    (1, 2),
                    vec![line(C, Some(1), Some(1)), line(A, None, Some(2))],
                ),
                hunk(
                    (4, 2),
                    (5, 1),
                    vec![line(C, Some(4), Some(5)), line(R, Some(5), None)],
                ),
            ],
        );
        // Unstaged: line 2 changed again; a line removed after line 2.
        put(
            Group::Unstaged,
            vec![hunk(
                (2, 1),
                (2, 1),
                vec![line(R, Some(2), None), line(A, None, Some(2))],
            )],
        );
        let marks = marks(&repo, "a", 6);
        let changed = |kind, staged| Mark::Changed { kind, staged };
        assert_eq!(marks[0], Mark::Same);
        assert_eq!(marks[1], changed(LineChange::Modified, false));
        assert_eq!(marks[4], changed(LineChange::RemovedBelow, true));
        assert_eq!(marks[5], Mark::Same);
    }

    #[test]
    fn an_index_line_moves_with_the_unstaged_lines_before_it() {
        // The working tree added two lines after line 2 of the index and removed line 10.
        let diff = FileDiff {
            path: "a".to_owned(),
            old_path: None,
            binary: false,
            hunks: vec![
                hunk(
                    (2, 0),
                    (3, 2),
                    vec![
                        line(LineKind::Added, None, Some(3)),
                        line(LineKind::Added, None, Some(4)),
                    ],
                ),
                hunk(
                    (9, 2),
                    (11, 1),
                    vec![
                        line(LineKind::Context, Some(9), Some(11)),
                        line(LineKind::Removed, Some(10), None),
                    ],
                ),
            ],
        };
        assert_eq!(index_to_worktree(1, Some(&diff)), Some(1));
        assert_eq!(index_to_worktree(2, Some(&diff)), Some(2));
        assert_eq!(index_to_worktree(3, Some(&diff)), Some(5));
        assert_eq!(index_to_worktree(9, Some(&diff)), Some(11));
        assert_eq!(index_to_worktree(10, Some(&diff)), None);
        assert_eq!(index_to_worktree(12, Some(&diff)), Some(13));
        assert_eq!(index_to_worktree(7, None), Some(7));
    }
}
