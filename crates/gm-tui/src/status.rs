//! The Status screen's sections (D241): the branch, then the untracked, unstaged and staged
//! files, the branch's changes against its base, and the review threads. A file unfolds into
//! its hunks and lines with the review threads under the lines they are about, laid out by
//! `gm_view::review::notes`, as the app's diff lays them out.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use gm_schema::{Change, ReviewStatus, StackState, StatusEntry};
use gm_view::diff_rows::{self, Row as DiffRow};
use gm_view::local_review::{Filter, Version};
use gm_view::provenance_view::Row;
use gm_view::review::{self, Entry, Heading, Opened, Part};

use crate::data::{FileData, Group, Repo};
use crate::icons;
use crate::nav::sections::Section;
use crate::reviews::{self, Turn};

/// How a line of the File screen differs from the last commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Same,
    /// Added or changed in the working tree, not staged.
    Unstaged,
    /// Added or changed, and staged.
    Staged,
}

/// What a row of Status is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Header {
        label: &'static str,
        text: String,
    },
    Blank,
    Group {
        group: Group,
        count: usize,
    },
    File {
        group: Group,
        path: String,
        letter: char,
        counts: Option<(u32, u32)>,
        threads: usize,
    },
    /// A line of text in place of a file's diff: reading, an error, a notice.
    Note(String),
    Hunk {
        group: Group,
        path: String,
        hunk: usize,
    },
    /// A row of a file's diff, by its index in the file's `gm_view` rows.
    Line {
        group: Group,
        path: String,
        row: usize,
    },
    /// A row of a review thread drawn under its lines.
    Card {
        id: i64,
        part: Part,
    },
    /// A strip above a file's rows: its outdated threads, or those outside its hunks.
    Heading(Heading),
    Threads {
        count: usize,
    },
    Turn {
        turn: Turn,
        count: usize,
    },
    Thread {
        id: i64,
    },
    /// The branch's own commits; `more` when there are more than were read.
    Commits {
        count: usize,
        more: bool,
    },
    /// A commit of the branch, by its index in `Repo::commits`.
    Commit {
        index: usize,
    },
    Stashes {
        count: usize,
    },
    /// A stash, by its index in `Repo::stashes`.
    Stash {
        index: usize,
    },
    /// The Before or After text of a thread, on the Thread screen.
    Version {
        version: Version,
        caption: &'static str,
    },
    /// A line of a thread's text: its number, its text and whether it changed between the
    /// two texts.
    Text {
        version: Version,
        number: u32,
        text: String,
        marked: bool,
    },
    /// The comment and its replies, on the Thread screen.
    Messages {
        count: usize,
    },
    /// A line of the File screen, by its index in `Repo::file`, and how it changed.
    FileLine {
        index: usize,
        mark: Mark,
    },
    /// A run of unchanged lines of the File screen, folded.
    Unchanged {
        count: usize,
    },
    /// A run of lines of the Blame screen that one commit last changed (or none, when they
    /// are not committed): indexes into `Repo::blame`, both ends inclusive.
    BlameRun {
        first: usize,
        last: usize,
    },
    /// A line of the Blame screen, by its index in `Repo::blame`.
    BlameLine {
        index: usize,
    },
    /// A row of the Origin screen, as the app's provenance panel words it.
    Origin(Row),
    /// A group of the Keys screen.
    KeyGroup {
        title: &'static str,
        count: usize,
    },
    /// A line of the Keys screen: the keys of an action and what it does.
    Binding {
        keys: String,
        description: &'static str,
    },
}

impl Item {
    /// The text `/` searches: what the row shows.
    pub fn text(&self, repo: &Repo) -> String {
        match self {
            Item::Header { label, text } => format!("{label} {text}"),
            Item::Group { group, .. } => group.title().to_owned(),
            Item::File { path: text, .. }
            | Item::Note(text)
            | Item::Text { text, .. }
            | Item::Card {
                part: Part::Text(text) | Part::Folded(text),
                ..
            } => text.clone(),
            Item::Line { group, path, row } => {
                line_of(repo, *group, path, *row).map_or_default(|line| line.text.clone())
            }
            Item::Thread { id } => repo.comment(*id).map_or_default(|c| c.body.clone()),
            Item::Commit { index } => repo
                .commits
                .as_ref()
                .and_then(|history| history.commits.get(*index))
                .map_or_default(|commit| format!("{} {}", commit.short, commit.subject)),
            Item::KeyGroup { title, .. } => (*title).to_owned(),
            Item::BlameRun { first, .. } => repo
                .blame
                .as_ref()
                .and_then(|(_, lines)| lines.get(*first))
                .map_or_default(|l| format!("{} {}", l.author, l.subject)),
            Item::BlameLine { index } => repo
                .blame
                .as_ref()
                .and_then(|(_, lines)| lines.get(*index))
                .map_or_default(|l| l.text.clone()),
            Item::Origin(row) => origin_text(row),
            Item::FileLine { index, .. } => repo
                .file
                .as_ref()
                .and_then(|file| file.lines.get(*index))
                .cloned()
                .unwrap_or_default(),
            Item::Binding { keys, description } => format!("{keys} {description}"),
            Item::Stash { index } => repo
                .stashes
                .get(*index)
                .map_or_default(|stash| format!("stash@{{{}}} {}", stash.index, stash.message)),
            _ => String::new(),
        }
    }

    /// The comment a row belongs to, for the thread actions.
    pub fn thread(&self) -> Option<i64> {
        match self {
            Item::Card { id, .. } | Item::Thread { id } => Some(*id),
            _ => None,
        }
    }

    /// The file a row belongs to.
    pub fn file(&self) -> Option<(Group, &str)> {
        match self {
            Item::File { group, path, .. }
            | Item::Hunk { group, path, .. }
            | Item::Line { group, path, .. } => Some((*group, path.as_str())),
            _ => None,
        }
    }
}

/// The words of an Origin row, for `/`.
fn origin_text(row: &Row) -> String {
    match row {
        Row::Note(text) | Row::Failure(text) | Row::Cut(text) | Row::Commit { text, .. } => {
            text.clone()
        }
        Row::Thread { title, harness, .. } => format!("{title} {harness}"),
        Row::Label { name, .. } => name.clone(),
        Row::Link { title, .. } => title.clone(),
        Row::Edit { path, .. } => path.clone(),
        Row::Entry { text, .. } => text.clone(),
    }
}

/// The diff line a `Line` row shows.
pub fn line_of<'a>(
    repo: &'a Repo,
    group: Group,
    path: &str,
    row: usize,
) -> Option<&'a gm_schema::DiffLine> {
    let file = repo.files.get(&(group, path.to_owned()))?;
    let DiffRow::Line {
        hunk_index,
        line_index,
        ..
    } = *file.rows.get(row)?
    else {
        return None;
    };
    file.diff
        .as_ref()?
        .hunks
        .get(hunk_index)?
        .lines
        .get(line_index)
}

fn change_letter(change: Change) -> char {
    match change {
        Change::Added => 'A',
        Change::Modified => 'M',
        Change::Deleted => 'D',
        Change::Renamed => 'R',
        Change::Copied => 'C',
        Change::TypeChanged => 'T',
    }
}

/// A file of a group: its path, its status letter and its added and deleted line counts.
pub type Listed = (String, char, Option<(u32, u32)>);

/// The files of `group` in the status or the branch's range, with letter and counts.
pub fn files_of(repo: &Repo, group: Group) -> Vec<Listed> {
    let counts = |side: &[gm_schema::LineCounts], path: &str| {
        side.iter()
            .find(|c| c.path == path)
            .map(|c| (c.added, c.deleted))
    };
    let status = |filter: &dyn Fn(&StatusEntry) -> Option<char>, side: &[gm_schema::LineCounts]| {
        repo.status
            .entries
            .iter()
            .filter_map(|entry| {
                Some((
                    entry.path.clone(),
                    filter(entry)?,
                    counts(side, &entry.path),
                ))
            })
            .collect::<Vec<_>>()
    };
    match group {
        Group::Untracked => status(&|e| e.untracked.then_some('?'), &[]),
        Group::Unstaged => status(
            &|e| {
                if e.conflicted {
                    Some('U')
                } else if e.untracked {
                    None
                } else {
                    e.unstaged.map(change_letter)
                }
            },
            &repo.stats.unstaged,
        ),
        Group::Staged => status(
            &|e| e.staged.filter(|_| !e.conflicted).map(change_letter),
            &repo.stats.staged,
        ),
        Group::Branch => repo
            .range
            .as_ref()
            .map_or_default(|range| commit_files(&range.files)),
        Group::Commit => repo
            .commit
            .as_ref()
            .map_or_default(|commit| commit_files(&commit.files)),
    }
}

fn commit_files(files: &[gm_schema::CommitFile]) -> Vec<Listed> {
    files
        .iter()
        .map(|f| {
            (
                f.path.clone(),
                change_letter(f.change),
                (!f.binary).then_some((f.added, f.deleted)),
            )
        })
        .collect()
}

/// The rows under an unfolded file: its hunks and lines, threads under their lines.
pub fn file_children(
    repo: &Repo,
    group: Group,
    path: &str,
    file: Option<&FileData>,
    opened: Opened<'_>,
) -> Vec<Section<Item>> {
    let id = |kind: &str, rest: String| format!("{kind}/{group:?}/{path}/{rest}");
    let note = |text: &str| {
        vec![Section::leaf(
            id("note", String::new()),
            Item::Note(text.to_owned()),
        )]
    };
    if path.ends_with('/') {
        return note("An untracked folder: its files are listed one by one once it is staged.");
    }
    let Some(file) = file else {
        return note("Reading the diff…");
    };
    if let Some(error) = &file.error {
        return note(error);
    }
    let Some(diff) = &file.diff else {
        return note("Reading the diff…");
    };
    if diff.binary {
        return note("Binary file: its contents are not shown.");
    }
    if diff.hunks.is_empty() {
        return note("No text changes: a rename or a mode change only.");
    }
    let diffs = file.diffs(group.side());
    let split = diff_rows::split_rows(diffs, group.side());
    let comments: &[gm_schema::ReviewComment] = if group == Group::Untracked {
        &[]
    } else {
        &repo.comments
    };
    let notes = review::notes(&file.rows, diffs, &split, comments, path, opened);
    let mut top: Vec<Section<Item>> = Vec::new();
    let mut hunks: Vec<Section<Item>> = Vec::new();
    let mut cards = 0usize;
    for entry in notes.unified.entries {
        let section = match entry {
            Entry::Diff(row) => match file.rows[row] {
                DiffRow::HunkHeader { hunk_index, .. } => {
                    hunks.push(Section::with(
                        id("hunk", hunk_index.to_string()),
                        Item::Hunk {
                            group,
                            path: path.to_owned(),
                            hunk: hunk_index,
                        },
                        Vec::new(),
                    ));
                    continue;
                }
                _ => Section::leaf(
                    id("line", row.to_string()),
                    Item::Line {
                        group,
                        path: path.to_owned(),
                        row,
                    },
                ),
            },
            Entry::Card { id: comment, part } => {
                cards += 1;
                Section::leaf(
                    id("card", format!("{comment}/{cards}")),
                    Item::Card { id: comment, part },
                )
            }
            Entry::Heading(heading) => Section::leaf(
                id("heading", format!("{heading:?}")),
                Item::Heading(heading),
            ),
        };
        match hunks.last_mut() {
            Some(hunk) => hunk.children.push(section),
            None => top.push(section),
        }
    }
    top.extend(hunks);
    top
}

/// The branches from the root of HEAD's stack to HEAD, and what is due on HEAD's branch;
/// `None` when the branch is in no stack.
fn stack_line(repo: &Repo) -> Option<String> {
    let head = repo.status.branch.head.as_deref()?;
    let stack = repo
        .stacks
        .stacks
        .iter()
        .find(|stack| stack.branches.iter().any(|branch| branch.name == head))?;
    let find = |name: &str| stack.branches.iter().find(|branch| branch.name == name);
    let here = find(head)?;
    let mut chain = vec![here.name.as_str()];
    let mut at = here;
    // A recorded parent loop is not a stack (StackSnapshot), so the walk ends; the depth
    // bounds it all the same.
    while let Some(parent) = at.parent.as_deref().and_then(find) {
        if chain.len() > stack.branches.len() {
            break;
        }
        chain.push(parent.name.as_str());
        at = parent;
    }
    chain.reverse();
    let mut text = format!("{} {}", icons::BRANCH, chain.join(" › "));
    if here.parent.is_some() {
        let _ = write!(
            text,
            "  {}{} {}{}",
            icons::AHEAD,
            here.ahead,
            icons::BEHIND,
            here.behind
        );
    }
    let due = match here.state {
        StackState::Root | StackState::Current => None,
        StackState::Stale => Some("restack due"),
        StackState::DivergedBase => Some("its base moved"),
        StackState::ParentGone => Some("its parent is gone"),
        StackState::ParentMerged => Some("its parent is merged"),
    };
    if let Some(due) = due {
        let _ = write!(text, "  · {due}");
    }
    let above = stack
        .branches
        .iter()
        .filter(|branch| branch.parent.as_deref() == Some(head))
        .count();
    if above > 0 {
        let _ = write!(
            text,
            "  · {above} {} on it",
            if above == 1 { "branch" } else { "branches" }
        );
    }
    Some(text)
}

/// The lines about the branch: its head, its upstream, its stack and its base.
fn headers(repo: &Repo) -> Vec<Section<Item>> {
    let branch = &repo.status.branch;
    let mut out = vec![Section::leaf(
        "head",
        Item::Header {
            label: "Head:",
            text: format!(
                "{} {}  {}",
                icons::BRANCH,
                branch.head.as_deref().unwrap_or("(detached)"),
                branch
                    .oid
                    .as_deref()
                    .map_or("no commit yet", |oid| &oid[..oid.len().min(7)])
            ),
        },
    )];
    if let Some(upstream) = &branch.upstream {
        out.push(Section::leaf(
            "merge",
            Item::Header {
                label: "Merge:",
                text: format!(
                    "{} {upstream}  {}{} {}{}",
                    icons::BRANCH,
                    icons::AHEAD,
                    branch.ahead,
                    icons::BEHIND,
                    branch.behind
                ),
            },
        ));
    }
    if let Some(text) = stack_line(repo) {
        out.push(Section::leaf(
            "stack",
            Item::Header {
                label: "Stack:",
                text,
            },
        ));
    }
    let base = match (&repo.range, &repo.range_error) {
        (Some(range), _) => format!("{} {}", icons::BRANCH, range.from_name),
        (None, Some(reason)) => reason
            .split(". ")
            .next()
            .unwrap_or(reason)
            .trim_end_matches('.')
            .to_owned(),
        (None, None) => "reading…".to_owned(),
    };
    out.push(Section::leaf(
        "base",
        Item::Header {
            label: "Base:",
            text: base,
        },
    ));
    out.push(Section::leaf("blank", Item::Blank));

    out
}

/// The Status screen's sections for what the repository holds now.
pub fn sections(repo: &Repo, opened: Opened<'_>) -> Vec<Section<Item>> {
    let mut out = headers(repo);
    let mut any = false;
    for group in [Group::Untracked, Group::Unstaged, Group::Staged] {
        if let Some(section) = group_section(repo, group, opened) {
            any = true;
            out.push(section);
        }
    }
    if !any {
        out.push(Section::leaf(
            "clean",
            Item::Note("Nothing to commit: the working tree is clean.".to_owned()),
        ));
    }
    out.extend(threads(repo));
    out.extend(commits(repo));
    out.extend(stashes(repo));
    out
}

/// A group of files and what unfolds under them; `None` when it has no file.
fn group_section(repo: &Repo, group: Group, opened: Opened<'_>) -> Option<Section<Item>> {
    let files = files_of(repo, group);
    if files.is_empty() {
        return None;
    }
    let children = files
        .into_iter()
        .map(|(path, letter, counts)| {
            let threads = repo
                .comments
                .iter()
                .filter(|c| c.path == path && c.status != ReviewStatus::Resolved)
                .count();
            let file = repo.files.get(&(group, path.clone()));
            Section::with(
                format!("file/{group:?}/{path}"),
                Item::File {
                    group,
                    path: path.clone(),
                    letter,
                    counts,
                    threads,
                },
                file_children(repo, group, &path, file, opened),
            )
            .folded()
        })
        .collect::<Vec<_>>();
    Some(Section::with(
        format!("group/{group:?}"),
        Item::Group {
            group,
            count: children.len(),
        },
        children,
    ))
}

/// The commits on the branch that are not on its base.
fn commits(repo: &Repo) -> Option<Section<Item>> {
    let history = repo.commits.as_ref()?;
    if history.commits.is_empty() {
        return None;
    }
    let children = (0..history.commits.len())
        .map(|index| Section::leaf(format!("commit/{index}"), Item::Commit { index }))
        .collect::<Vec<_>>();
    Some(Section::with(
        "commits",
        Item::Commits {
            count: children.len(),
            more: history.more,
        },
        children,
    ))
}

/// The stashes, newest first.
fn stashes(repo: &Repo) -> Option<Section<Item>> {
    if repo.stashes.is_empty() {
        return None;
    }
    let children = (0..repo.stashes.len())
        .map(|index| Section::leaf(format!("stash/{index}"), Item::Stash { index }))
        .collect();
    Some(Section::with(
        "stashes",
        Item::Stashes {
            count: repo.stashes.len(),
        },
        children,
    ))
}

/// The open review threads, under who acts next.
fn threads(repo: &Repo) -> Option<Section<Item>> {
    let groups = thread_groups(repo, Filter::Open);
    if groups.is_empty() {
        return None;
    }
    let count = groups.iter().map(|g| g.children.len()).sum();
    Some(Section::with("threads", Item::Threads { count }, groups))
}

/// The threads `filter` keeps, under who acts next: a section per group, a row per thread.
pub fn thread_groups(repo: &Repo, filter: Filter) -> Vec<Section<Item>> {
    let entries = reviews::entries(&repo.comments, filter);
    let mut groups: Vec<Section<Item>> = Vec::new();
    for entry in entries {
        match entry {
            reviews::Entry::Group(turn, count) => groups.push(Section::with(
                format!("turn/{turn:?}"),
                Item::Turn { turn, count },
                Vec::new(),
            )),
            reviews::Entry::Thread(id) => {
                if let Some(group) = groups.last_mut() {
                    group
                        .children
                        .push(Section::leaf(format!("thread/{id}"), Item::Thread { id }));
                }
            }
        }
    }
    groups
}

/// The files Status shows unfolded, by group and path: the ones whose diff must be read.
pub fn unfolded_files(open: &BTreeSet<String>) -> Vec<(Group, String)> {
    open.iter()
        .filter_map(|id| {
            let rest = id.strip_prefix("file/")?;
            let (group, path) = rest.split_once('/')?;
            let group = match group {
                "Untracked" => Group::Untracked,
                "Unstaged" => Group::Unstaged,
                "Staged" => Group::Staged,
                "Branch" => Group::Branch,
                "Commit" => Group::Commit,
                _ => return None,
            };
            Some((group, path.to_owned()))
        })
        .collect()
}
