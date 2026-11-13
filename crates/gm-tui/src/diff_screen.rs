//! The Diff and File screens (D241, M4). Diff shows the branch against its base (D237) or
//! one commit, read-only, with the review threads under their lines as on Status. File
//! shows a whole file of the working tree, colored, with the lines not yet committed marked
//! and long unchanged runs folded.

use std::collections::BTreeSet;

use gm_schema::{FileDiff, LineKind};
use gm_view::review::Opened;

use crate::data::{Group, Repo};
use crate::nav::sections::Section;
use crate::status::{Item, Mark, file_children, files_of};

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
pub fn diff(repo: &Repo, kind: &DiffKind, opened: Opened<'_>) -> Vec<Section<Item>> {
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
            file_children(repo, group, &path, file, opened),
        ));
    }
    out
}

/// Lines shown around a changed line before a run of unchanged ones folds.
const CONTEXT: usize = 3;
/// The shortest run of unchanged lines that folds.
const FOLD_RUN: usize = 8;

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

/// The marks of the lines of the working tree file `path`: unstaged additions, and staged
/// ones carried through the unstaged diff.
fn marks(repo: &Repo, path: &str, count: usize) -> Vec<Mark> {
    let mut marks = vec![Mark::Same; count];
    let diff = |group| {
        repo.files
            .get(&(group, path.to_owned()))
            .and_then(|file| file.diff.as_ref())
    };
    let unstaged = diff(Group::Unstaged);
    let added = |diff: Option<&FileDiff>| -> Vec<u32> {
        diff.map_or(&[][..], |d| d.hunks.as_slice())
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| line.kind == LineKind::Added)
            .filter_map(|line| line.new_no)
            .collect()
    };
    let mut set = |number: u32, mark| {
        if let Some(slot) = (number as usize)
            .checked_sub(1)
            .and_then(|i| marks.get_mut(i))
        {
            *slot = mark;
        }
    };
    for number in added(diff(Group::Staged)) {
        if let Some(at) = index_to_worktree(number, unstaged) {
            set(at, Mark::Staged);
        }
    }
    for number in added(unstaged) {
        set(number, Mark::Unstaged);
    }
    marks
}

/// The File screen's sections. `keep` is a line (from 1) that must not be folded away.
pub fn file(repo: &Repo, path: &str, keep: Option<u32>) -> Vec<Section<Item>> {
    let mut out = vec![Section::leaf(
        "head",
        Item::Header {
            label: "File:",
            text: path.to_owned(),
        },
    )];
    let note = |text: &str| Section::leaf("note", Item::Note(text.to_owned()));
    let Some(file) = repo.file.as_ref().filter(|file| file.path == path) else {
        out.push(note("Reading the file…"));
        return out;
    };
    if let Some(text) = &file.note {
        out.push(note(text));
        return out;
    }
    let marks = marks(repo, path, file.lines.len());
    // Lines near a change, or near the line to keep, stay unfolded; with no change, all do.
    let mut shown: BTreeSet<usize> = BTreeSet::new();
    let anchors = marks
        .iter()
        .enumerate()
        .filter(|(_, mark)| **mark != Mark::Same)
        .map(|(index, _)| index)
        .chain(keep.and_then(|n| (n as usize).checked_sub(1)));
    for at in anchors {
        shown.extend(at.saturating_sub(CONTEXT)..=at + CONTEXT);
    }
    let all = shown.is_empty();
    let line = |index: usize| {
        Section::leaf(
            format!("line/{}", index + 1),
            Item::FileLine {
                index,
                mark: marks[index],
            },
        )
    };
    let mut index = 0;
    while index < file.lines.len() {
        if all || shown.contains(&index) {
            out.push(line(index));
            index += 1;
            continue;
        }
        let end = (index..file.lines.len())
            .find(|i| shown.contains(i))
            .unwrap_or(file.lines.len());
        if end - index < FOLD_RUN {
            out.extend((index..end).map(line));
        } else {
            out.push(
                Section::with(
                    format!("run/{}", index + 1),
                    Item::Unchanged { count: end - index },
                    (index..end).map(line).collect(),
                )
                .folded(),
            );
        }
        index = end;
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
