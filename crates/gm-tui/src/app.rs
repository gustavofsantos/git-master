//! The model of the terminal companion (D235, D236): what the screen shows and how keys and
//! clicks change it. Nothing here draws or touches the terminal, so every rule is tested on
//! its own. Each change the person asks of the repository leaves here as a `CommandKind`.
//!
//! Every button is an `Action`. A key maps to an action, and each draw records the area of
//! every button it drew with its action (`hits`), so a click runs the same action as the key
//! (D236 (2)).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use gm_schema::{
    Change, ChangeStats, CommandKind, Event, OperationId, OperationState, RepoStatus,
    ReviewAuthorFilter, ReviewComment, ReviewStatus, RevisionRange, StageTarget, StatusEntry,
};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use std::time::Instant;

use gm_view::compose::{self, ReviewTarget};
use gm_view::review::{self, Delete};

use crate::diff::{ColorJob, DiffView, side_of};
use crate::reviews::ReviewsView;
use crate::threads::{Composer, Folds};
use gm_view::local_review::{Filter, Version};

/// Narrower than this, the sidebar and the main pane show one at a time (D235 (9)).
pub const SPLIT_MIN_WIDTH: u16 = 100;
/// Smaller than this, only a message shows (D235 (9)).
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
/// Why the branch's diff has no staging.
const READ_ONLY: &str = "The branch's changes are committed: there is nothing to stage.";
/// Rows a turn of the mouse wheel scrolls.
const WHEEL_ROWS: usize = 3;

/// The tabs of the main pane, in the order the top bar shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Diff,
    File,
    Reviews,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Diff, Tab::File, Tab::Reviews];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Diff => "Diff",
            Tab::File => "File",
            Tab::Reviews => "Reviews",
        }
    }

    pub fn key(self) -> char {
        match self {
            Tab::Diff => '1',
            Tab::File => '2',
            Tab::Reviews => '3',
        }
    }
}

/// What the sidebar lists: the changes not staged yet, the staged ones (D236 (3)), or the
/// changes of the branch's commits against its base (D237).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    Unstaged,
    Staged,
    Branch,
}

impl Context {
    pub const ALL: [Context; 3] = [Context::Unstaged, Context::Staged, Context::Branch];

    pub fn title(self) -> &'static str {
        match self {
            Context::Unstaged => "Unstaged",
            Context::Staged => "Staged",
            Context::Branch => "Branch",
        }
    }

    /// The context `t` goes to.
    pub fn next(self) -> Context {
        match self {
            Context::Unstaged => Context::Staged,
            Context::Staged => Context::Branch,
            Context::Branch => Context::Unstaged,
        }
    }

    /// Whether the context's changes can be staged or unstaged: the branch's are committed.
    pub fn stages(self) -> bool {
        self != Context::Branch
    }

    fn index(self) -> usize {
        match self {
            Context::Unstaged => 0,
            Context::Staged => 1,
            Context::Branch => 2,
        }
    }
}

/// A changed file of one context, before the sidebar groups it by folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed {
    pub path: String,
    pub letter: char,
    pub counts: Option<(u32, u32)>,
}

/// The pane that has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Main,
}

/// A changed file as the sidebar lists it in one context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileItem {
    pub path: String,
    /// The last part of the path, as the row shows it under its folder.
    pub name: String,
    /// Git's status letter on this side: `M`, `A`, `D`, `R`, `C`, `T`, `?` for an untracked
    /// file and `U` for a conflicted one.
    pub letter: char,
    /// Lines added and removed; `None` for a binary or untracked file, or before the counts
    /// are read.
    pub counts: Option<(u32, u32)>,
    /// Threads on the file that are not resolved.
    pub threads: usize,
}

/// A line of the sidebar list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A folder with changed files directly in it; `""` is the repository's root.
    Dir {
        dir: String,
        files: usize,
        folded: bool,
    },
    File(FileItem),
}

/// What a key or a click does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Tab(Tab),
    Context(Context),
    OtherContext,
    ToggleFocus,
    Help,
    CloseHelp,
    Quit,
    /// Puts the cursor on this sidebar row; on a folder, also folds or unfolds it.
    Row(usize),
    /// Puts the diff's cursor on this row of the diff.
    DiffRow(usize),
    /// Stages this hunk of the diff under **Unstaged**, unstages it under **Staged**.
    StageHunk(usize),
    /// Stages or unstages the hunk at the diff's cursor.
    StageCursorHunk,
    /// Moves the diff's cursor to the next (`true`) or the previous hunk.
    JumpHunk(bool),
    /// Stages the selected file under **Unstaged**, unstages it under **Staged**.
    StageFile,
    /// Stages the selected lines under **Unstaged**, unstages them under **Staged**.
    StageSelection,
    ClearSelection,
    /// Opens the comment box on the selected lines, or on the line at the cursor.
    Comment,
    Reply(i64),
    Resolve(i64),
    Reopen(i64),
    /// The first press asks, a second within a few seconds deletes (`review::DeleteArm`).
    Delete(i64),
    /// Opens or folds a resolved thread.
    OpenThread(i64),
    /// Shows all of a thread's replies, or folds the earlier ones.
    ExpandThread(i64),
    ToggleOutdated,
    /// Moves the diff's cursor to the next (`true`) or the previous thread.
    JumpThread(bool),
    /// Picks the comment type at this index of `threads::TYPES`.
    CommentType(usize),
    SaveComment,
    CancelComment,
    /// Puts the **Reviews** list's cursor on this entry.
    ReviewRow(usize),
    ReviewFilter(Filter),
    /// Shows the selected thread's text as it was (`Before`) or after the agent (`After`).
    ReviewVersion(Version),
    /// Shows the thread in the **Diff** tab, the cursor on its first line.
    GoToDiff(i64),
}

/// A button the last draw put on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub area: Rect,
    pub action: Action,
}

/// Where each part of the screen is, as the last draw placed it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Areas {
    pub top: Option<Rect>,
    /// The sidebar, chips included; `None` when one pane shows and it is the main pane.
    pub sidebar: Option<Rect>,
    /// `None` when one pane shows and it is the sidebar.
    pub main: Option<Rect>,
    pub bar: Option<Rect>,
}

impl Areas {
    /// The sidebar's file list: the sidebar under its chips, without its right border.
    pub fn list(&self) -> Option<Rect> {
        self.sidebar.map(|area| Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            width: area.width.saturating_sub(1),
            ..area
        })
    }
}

/// The parts of a terminal of `size` with `focus`: a top bar, the sidebar and the main pane,
/// a button bar; one of the two panes alone below `SPLIT_MIN_WIDTH`; nothing below the
/// minimum size.
pub fn areas(size: Rect, focus: Focus) -> Areas {
    if size.width < MIN_WIDTH || size.height < MIN_HEIGHT {
        return Areas::default();
    }
    let top = Rect { height: 1, ..size };
    let bar = Rect {
        y: size.y + size.height - 1,
        height: 1,
        ..size
    };
    let body = Rect {
        y: size.y + 1,
        height: size.height - 2,
        ..size
    };
    let (sidebar, main) = if size.width < SPLIT_MIN_WIDTH {
        match focus {
            Focus::Sidebar => (Some(body), None),
            Focus::Main => (None, Some(body)),
        }
    } else {
        let width = (size.width / 3).clamp(30, 48);
        let main = Rect {
            x: body.x + width,
            width: body.width - width,
            ..body
        };
        (Some(Rect { width, ..body }), Some(main))
    };
    Areas {
        top: Some(top),
        sidebar,
        main,
        bar: Some(bar),
    }
}

/// The files of `status` in `context`, with their letters.
fn files_of(status: &RepoStatus, context: Context) -> Vec<(&StatusEntry, char)> {
    status
        .entries
        .iter()
        .filter_map(|entry| {
            let letter = match context {
                Context::Unstaged if entry.conflicted => Some('U'),
                Context::Unstaged if entry.untracked => Some('?'),
                Context::Unstaged => entry.unstaged.map(change_letter),
                Context::Staged if entry.conflicted => None,
                Context::Staged => entry.staged.map(change_letter),
                Context::Branch => None,
            }?;
            Some((entry, letter))
        })
        .collect()
}

pub fn change_letter(change: Change) -> char {
    match change {
        Change::Added => 'A',
        Change::Modified => 'M',
        Change::Deleted => 'D',
        Change::Renamed => 'R',
        Change::Copied => 'C',
        Change::TypeChanged => 'T',
    }
}

/// The folder of `path` and the name shown under it. An untracked folder, `sub/`, is one
/// entry named `sub/` in its parent.
fn split_path(path: &str) -> (&str, &str) {
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    match trimmed.rfind('/') {
        Some(at) => (&path[..at], &path[at + 1..]),
        None => ("", path),
    }
}

/// The sidebar rows: one folder row for each folder with changed files directly in it, in
/// path order with the root last, each followed by its files unless it is folded.
pub fn sidebar_rows(
    files: &[Changed],
    comments: &[ReviewComment],
    folded: &BTreeSet<String>,
) -> Vec<Row> {
    let mut dirs: BTreeMap<&str, Vec<FileItem>> = BTreeMap::new();
    for entry in files {
        let (dir, name) = split_path(&entry.path);
        let (letter, counts) = (entry.letter, entry.counts);
        let threads = comments
            .iter()
            .filter(|comment| {
                comment.path == entry.path && comment.status != ReviewStatus::Resolved
            })
            .count();
        dirs.entry(dir).or_default().push(FileItem {
            path: entry.path.clone(),
            name: name.to_owned(),
            letter,
            counts,
            threads,
        });
    }
    let root = dirs.remove("");
    let mut rows = Vec::new();
    for (dir, mut files) in dirs.into_iter().chain(root.map(|files| ("", files))) {
        files.sort_by(|a, b| a.name.cmp(&b.name));
        let is_folded = folded.contains(dir);
        rows.push(Row::Dir {
            dir: dir.to_owned(),
            files: files.len(),
            folded: is_folded,
        });
        if !is_folded {
            rows.extend(files.into_iter().map(Row::File));
        }
    }
    rows
}

/// A window drawn over the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Help,
}

/// The whole state of the companion.
#[derive(Debug)]
pub struct App {
    pub status: RepoStatus,
    pub stats: ChangeStats,
    pub comments: Vec<ReviewComment>,
    pub tab: Tab,
    pub context: Context,
    pub focus: Focus,
    pub rows: Vec<Row>,
    /// Index into `rows` of the cursor; a file or a folder.
    pub cursor: usize,
    /// The first row of the list on screen.
    pub scroll: usize,
    pub overlay: Option<Overlay>,
    /// Where the last draw put each part.
    pub areas: Areas,
    /// The buttons of the last draw. A click runs the last one that contains it.
    pub hits: Vec<Hit>,
    /// A message about the last action, for the top bar; the next action clears it.
    pub notice: Option<String>,
    /// Why the watcher is not watching, while it is not; it stays up.
    pub watch_notice: Option<String>,
    pub quit: bool,
    /// What changed since the last draw.
    pub dirty: bool,
    /// The next draw brings the cursor on screen. The wheel clears it, so the list scrolls
    /// away from the cursor until the cursor moves again.
    follow_cursor: bool,
    /// The file shown in the main pane, per context.
    selected: [Option<String>; 3],
    /// The branch's changes against its base, as last read (D237).
    pub range: Option<RevisionRange>,
    /// Why the branch's changes could not be read: on the default branch, no common
    /// ancestor, no default branch.
    pub range_error: Option<String>,
    /// The read of the branch's changes in flight.
    range_request: Option<OperationId>,
    /// The branch and commit HEAD was on when the branch's changes were last asked for.
    range_head: Option<(Option<String>, Option<String>)>,
    /// The diffs of the branch's files, read in one go for the range from `base` to `tip`,
    /// so moving between them reads nothing again.
    branch_diffs: Option<((String, String), Vec<gm_schema::FileDiff>)>,
    folded: BTreeSet<String>,
    /// The box that writes a comment or a reply, while it is open.
    pub composer: Option<Composer>,
    /// Which threads show unfolded.
    pub folds: Folds,
    /// The **Reviews** tab.
    pub reviews: ReviewsView,
    /// A line of a file to put the diff's cursor on once its diff is read.
    jump: Option<(String, u32)>,
    /// The **Diff** tab's diff of the selected file.
    pub diff: Option<DiffView>,
    /// Counts the diff views made, so an answer for an older one is known.
    generation: u64,
    color_jobs: Vec<ColorJob>,
    /// The read of the diff on screen, to show why it failed.
    diff_request: Option<OperationId>,
    /// Writes in flight, with what to call them if they fail.
    writes: HashMap<OperationId, &'static str>,
    /// The diff row a press of the left button started on; a drag selects from it.
    drag_from: Option<usize>,
    commands: Vec<CommandKind>,
}

impl App {
    /// The companion for the repository whose status is `status`, on the **Diff** tab with
    /// the first unstaged file selected. It asks for the line counts and the threads at once.
    pub fn new(status: RepoStatus) -> Self {
        let mut app = Self {
            status,
            stats: ChangeStats::default(),
            comments: Vec::new(),
            tab: Tab::Diff,
            context: Context::Unstaged,
            focus: Focus::Sidebar,
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            overlay: None,
            areas: Areas::default(),
            hits: Vec::new(),
            notice: None,
            watch_notice: None,
            quit: false,
            dirty: true,
            follow_cursor: true,
            selected: [None, None, None],
            range: None,
            range_error: None,
            range_request: None,
            range_head: None,
            branch_diffs: None,
            folded: BTreeSet::new(),
            diff: None,
            generation: 0,
            color_jobs: Vec::new(),
            diff_request: None,
            writes: HashMap::new(),
            drag_from: None,
            composer: None,
            folds: Folds::default(),
            reviews: ReviewsView::default(),
            jump: None,
            commands: Vec::new(),
        };
        app.rebuild();
        app.ask_for_reads();
        app
    }

    /// The file the main pane shows, if the context has any.
    pub fn selected(&self) -> Option<&FileItem> {
        let path = self.selected[self.context.index()].as_deref()?;
        self.rows.iter().find_map(|row| match row {
            Row::File(file) if file.path == path => Some(file),
            _ => None,
        })
    }

    /// The number of files `context` lists.
    pub fn file_count(&self, context: Context) -> usize {
        match context {
            Context::Branch => self
                .range
                .as_ref()
                .map_or(0, |range| range.files.len() + range.more_files),
            _ => files_of(&self.status, context).len(),
        }
    }

    /// The changed files `context` lists, with their letters and counts.
    pub fn changed(&self, context: Context) -> Vec<Changed> {
        if context == Context::Branch {
            let files = self
                .range
                .as_ref()
                .map_or(&[][..], |range| range.files.as_slice());
            return files
                .iter()
                .map(|file| Changed {
                    path: file.path.clone(),
                    letter: change_letter(file.change),
                    counts: (!file.binary).then_some((file.added, file.deleted)),
                })
                .collect();
        }
        let counts = match context {
            Context::Staged => &self.stats.staged,
            _ => &self.stats.unstaged,
        };
        files_of(&self.status, context)
            .into_iter()
            .map(|(entry, letter)| Changed {
                path: entry.path.clone(),
                letter,
                counts: counts
                    .iter()
                    .find(|counts| counts.path == entry.path)
                    .map(|counts| (counts.added, counts.deleted)),
            })
            .collect()
    }

    /// The threads that are not resolved.
    pub fn unresolved(&self) -> usize {
        self.comments
            .iter()
            .filter(|comment| comment.status != ReviewStatus::Resolved)
            .count()
    }

    /// The commands the model asked for since the last call, oldest first.
    pub fn take_commands(&mut self) -> Vec<CommandKind> {
        std::mem::take(&mut self.commands)
    }

    /// The code to color since the last call; the loop colors it off this thread.
    pub fn take_color_jobs(&mut self) -> Vec<ColorJob> {
        std::mem::take(&mut self.color_jobs)
    }

    /// The engine took `kind` as operation `id`: the model keeps the ids whose failure it
    /// must show.
    pub fn sent(&mut self, id: OperationId, kind: &CommandKind) {
        self.reviews.sent(id, kind);
        match kind {
            CommandKind::LoadDiff { .. } | CommandKind::LoadRevisionStack { .. } => {
                self.diff_request = Some(id);
            }
            CommandKind::LoadBranchChange { .. } => self.range_request = Some(id),
            CommandKind::Stage { .. } => {
                self.writes.insert(id, "Staging");
            }
            CommandKind::Unstage { .. } => {
                self.writes.insert(id, "Unstaging");
            }
            CommandKind::AddReviewComment { .. }
            | CommandKind::AddReviewCommentByText { .. }
            | CommandKind::ReplyToReviewComment { .. } => {
                if let Some(composer) = self.composer.as_mut() {
                    composer.pending = Some(id);
                }
            }
            CommandKind::ResolveReviewComment { .. } => {
                self.writes.insert(id, "Resolving");
            }
            CommandKind::ReopenReviewComment { .. } => {
                self.writes.insert(id, "Reopening");
            }
            CommandKind::DeleteReviewComment { .. } => {
                self.writes.insert(id, "Deleting");
            }
            _ => {}
        }
    }

    /// Text pasted into the terminal: it goes into the comment box when one is open.
    pub fn on_paste(&mut self, text: &str) {
        if let Some(composer) = self.composer.as_mut() {
            composer.type_text(text);
            self.dirty = true;
        }
    }

    /// Lays the diff out again with the threads under their lines.
    fn relayout(&mut self) {
        if let Some(view) = self.diff.as_mut() {
            view.relayout(&self.comments, self.folds.as_opened());
        }
    }

    /// Takes in the colors of a job's code.
    pub fn colored(&mut self, job: &ColorJob, spans: &[gm_schema::HighlightSpan]) {
        if let Some(view) = self.diff.as_mut().filter(|view| view.path == job.path) {
            view.take_colors(job, spans);
            self.dirty = true;
        }
    }

    /// Takes in what the engine reports. A new status asks again for the counts, the threads
    /// and the diff on screen, which may have changed with it.
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::StatusLoaded { status, .. } => {
                self.status.clone_from(status);
                self.ask_for_reads();
                let before = self.diff_key();
                self.rebuild();
                if self.diff_key() == before {
                    self.reload_diff();
                }
            }
            Event::ChangeStatsLoaded { stats, .. } => {
                self.stats.clone_from(stats);
                self.rebuild();
            }
            Event::ReviewCommentsListed { comments, .. } => {
                self.comments.clone_from(comments);
                self.rebuild();
                self.relayout();
                self.reviews.relist(&self.comments);
                self.select_review(true);
            }
            Event::ReviewChange {
                id,
                comment,
                change,
                ..
            } => {
                self.reviews
                    .page
                    .received_change(*id, *comment, change.clone());
            }
            Event::ReviewSnapshotText {
                id, comment, lines, ..
            } => {
                self.reviews
                    .page
                    .received_before(*id, *comment, lines.clone());
            }
            Event::ReviewCurrentText {
                id, comment, lines, ..
            } => {
                self.reviews
                    .page
                    .received_after(*id, *comment, lines.clone());
            }
            Event::DiffLoaded { side, diff, .. } => self.diff_loaded(Some(*side), diff),
            Event::RevisionRangeLoaded { id, range } if self.range_request == Some(*id) => {
                self.range = Some(range.clone());
                self.range_error = None;
                self.range_loaded();
            }
            Event::RevisionStackLoaded { id, diffs } if self.diff_request == Some(*id) => {
                if let Some(range) = self.range.as_ref() {
                    let ends = (range.base.clone(), range.tip.clone());
                    self.branch_diffs = Some((ends, diffs.clone()));
                }
                self.reload_diff();
            }
            Event::OperationChanged {
                id, state, detail, ..
            } => self.operation_changed(*id, *state, detail.as_deref()),
            _ => return,
        }
        self.dirty = true;
    }

    /// The branch's changes were read (again): the list, and the diff on screen when it is
    /// the branch's, since the range may have moved.
    fn range_loaded(&mut self) {
        let before = self.diff_key();
        self.rebuild();
        if self.context == Context::Branch && self.diff_key() == before {
            self.reload_diff();
        }
    }

    /// A diff arrived: `side` for the working tree's, `None` for the branch's.
    fn diff_loaded(&mut self, side: Option<gm_schema::DiffSide>, diff: &gm_schema::FileDiff) {
        let Some(view) = self.diff.as_mut() else {
            return;
        };
        let wanted = view
            .context
            .stages()
            .then(|| side_of(view.context).diff_side());
        if view.path != diff.path || wanted != side {
            return;
        }
        view.load(diff.clone());
        if let Some(job) = view.color_job() {
            self.color_jobs.push(job);
        }
        self.relayout();
        self.apply_jump();
    }

    fn operation_changed(&mut self, id: OperationId, state: OperationState, detail: Option<&str>) {
        let failed = matches!(
            state,
            OperationState::Failed | OperationState::Vetoed | OperationState::Cancelled
        );
        if !matches!(state, OperationState::Done) && !failed {
            return;
        }
        let reason = detail.unwrap_or("no reason given").to_owned();
        if self.range_request == Some(id) && failed {
            self.range_request = None;
            self.range = None;
            self.range_error = Some(reason.clone());
            self.range_loaded();
        }
        // A read on a job of its own (the branch's diff) is done before its answer arrives:
        // only a failure ends it here; a newer read replaces it.
        if self.diff_request == Some(id) && failed {
            self.diff_request = None;
            if let Some(view) = self.diff.as_mut() {
                view.error = Some(reason.clone());
            }
        }
        if let Some(what) = self.writes.remove(&id).filter(|_| failed) {
            self.notice = Some(format!("{what} failed: {reason}"));
        }
        if let Some(composer) = self.composer.as_mut().filter(|c| c.pending == Some(id)) {
            composer.pending = None;
            if failed {
                composer.error = Some(reason);
            } else {
                self.composer = None;
                if let Some(view) = self.diff.as_mut() {
                    view.anchor = None;
                }
            }
        }
    }

    /// Which file and context the **Diff** tab should show.
    fn diff_key(&self) -> Option<(String, Context)> {
        self.selected()
            .map(|file| (file.path.clone(), self.context))
    }

    /// Makes the diff view follow the selected file: a new view, and its read, when the file
    /// or the context changed.
    fn sync_diff(&mut self) {
        let Some((path, context)) = self.diff_key() else {
            self.diff = None;
            return;
        };
        if self
            .diff
            .as_ref()
            .is_some_and(|view| view.path == path && view.context == context)
        {
            return;
        }
        self.generation += 1;
        self.diff = Some(DiffView::new(path, context, self.generation));
        self.reload_diff();
    }

    /// Reads the diff on screen again.
    fn reload_diff(&mut self) {
        let Some(view) = self.diff.as_mut() else {
            return;
        };
        if view.context == Context::Branch {
            let Some(range) = self.range.as_ref() else {
                view.error = Some("Reading the branch's changes…".to_owned());
                return;
            };
            let ends = (range.base.clone(), range.tip.clone());
            let cached = self
                .branch_diffs
                .as_ref()
                .filter(|(read, _)| *read == ends)
                .and_then(|(_, diffs)| diffs.iter().find(|diff| diff.path == view.path))
                .cloned();
            match cached {
                Some(diff) => self.diff_loaded(None, &diff),
                // One patch for the range, cut in the order of its files: all of them are
                // asked for, as the app's Branch tab does.
                None => self.commands.push(CommandKind::LoadRevisionStack {
                    base: ends.0,
                    tip: ends.1,
                    files: range.files.clone(),
                }),
            }
            return;
        }
        if view.path.ends_with('/') {
            view.error = Some(
                "An untracked folder: its files are listed one by one once they are staged."
                    .to_owned(),
            );
            return;
        }
        self.commands.push(CommandKind::LoadDiff {
            path: view.path.clone(),
            side: side_of(view.context).diff_side(),
        });
    }

    /// The terminal changed size or came back into view.
    pub fn redraw(&mut self) {
        self.dirty = true;
    }

    pub fn set_watch_notice(&mut self, notice: Option<String>) {
        self.watch_notice = notice;
        self.dirty = true;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if self.composer.is_some() {
            self.composer_key(key);
            self.dirty = true;
            return;
        }
        if let Some(action) = self.action_of(key) {
            self.act(&action);
        } else if self.focus == Focus::Sidebar && self.tab == Tab::Reviews {
            self.review_list_key(key);
        } else if self.focus == Focus::Sidebar {
            self.sidebar_key(key);
        } else if self.tab == Tab::Diff {
            self.diff_key_press(key);
        } else if self.tab == Tab::Reviews {
            self.review_thread_key(key);
        }
    }

    /// Keys in the **Reviews** list.
    fn review_list_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.reviews.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.reviews.move_cursor(-1),
            KeyCode::Char('g') | KeyCode::Home => self.reviews.move_cursor(isize::MIN),
            KeyCode::Char('G') | KeyCode::End => self.reviews.move_cursor(isize::MAX),
            KeyCode::Char('F') => self.reviews.next_filter(&self.comments),
            KeyCode::Enter => self.focus = Focus::Main,
            _ => return,
        }
        self.select_review(false);
        self.dirty = true;
    }

    /// Keys on the selected thread of the **Reviews** tab.
    fn review_thread_key(&mut self, key: KeyEvent) {
        let Some(id) = self.reviews.page.selected_id() else {
            return;
        };
        let action = match key.code {
            KeyCode::Char('b') => Action::ReviewVersion(match self.reviews.page.version() {
                Version::Before => Version::After,
                Version::After => Version::Before,
            }),
            KeyCode::Char('r') | KeyCode::Enter => Action::Reply(id),
            KeyCode::Char('R') => Action::Resolve(id),
            KeyCode::Char('o') => Action::Reopen(id),
            KeyCode::Char('D') => Action::Delete(id),
            KeyCode::Char('g') => Action::GoToDiff(id),
            KeyCode::Char('F') => {
                self.reviews.next_filter(&self.comments);
                self.select_review(false);
                self.dirty = true;
                return;
            }
            _ => return,
        };
        self.act(&action);
        self.dirty = true;
    }

    /// The actions of the **Reviews** tab.
    fn review_action(&mut self, action: &Action) {
        match *action {
            Action::ReviewRow(index) => {
                self.focus = Focus::Sidebar;
                self.reviews.cursor = index;
                self.reviews.move_cursor(0);
                self.select_review(false);
            }
            Action::ReviewFilter(filter) => {
                self.reviews.set_filter(filter, &self.comments);
                self.select_review(false);
            }
            Action::ReviewVersion(version) => self.reviews.page.set_version(version),
            Action::GoToDiff(id) => self.go_to_diff(id),
            _ => {}
        }
    }

    /// Selects the thread at the list's cursor and asks for its texts, as the app's page
    /// does; `again` asks for them even when it was already selected (after a new listing).
    fn select_review(&mut self, again: bool) {
        // As the app's page: the texts are read while the tab shows, and on showing it.
        if self.tab != Tab::Reviews {
            return;
        }
        let Some(id) = self.reviews.at_cursor() else {
            return;
        };
        let Some(comment) = self.comments.iter().find(|comment| comment.id == id) else {
            return;
        };
        if self.reviews.page.select(comment) || again {
            self.commands
                .extend(self.reviews.text_reads(&self.comments));
        }
    }

    /// Shows thread `id` in the **Diff** tab: in the first chip that lists its file, with
    /// the cursor on its first line once the diff is read.
    fn go_to_diff(&mut self, id: i64) {
        let Some(comment) = self.comment(id) else {
            return;
        };
        let path = comment.path.clone();
        let line = comment
            .location
            .map_or(comment.anchor.start_line, |location| location.start_line);
        let Some(context) = Context::ALL
            .into_iter()
            .find(|context| self.changed(*context).iter().any(|file| file.path == path))
        else {
            self.notice = Some(format!(
                "{path} has no changes to show; the thread is on line {line}."
            ));
            return;
        };
        self.tab = Tab::Diff;
        self.focus = Focus::Main;
        self.selected[context.index()] = Some(path.clone());
        self.context = context;
        self.scroll = 0;
        self.jump = Some((path, line));
        self.rebuild();
        self.apply_jump();
    }

    /// Puts the diff's cursor on the line a jump waits for, once its diff is there.
    fn apply_jump(&mut self) {
        let Some((path, line)) = self.jump.clone() else {
            return;
        };
        let Some(view) = self.diff.as_mut().filter(|view| view.path == path) else {
            return;
        };
        if view.diff.is_none() {
            return;
        }
        if let Some(row) = review::row_of_line(&view.rows, view.diffs(), line) {
            view.cursor = row;
            view.follow_cursor = true;
        }
        self.jump = None;
    }

    /// Keys while the comment box is open: they type, except `Enter` (save), `Alt+Enter` (a
    /// new line), `Tab` (the next type), `Backspace` and `Esc` (close, asking first when the
    /// box holds text).
    fn composer_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let Some(composer) = self.composer.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Esc => self.act(&Action::CancelComment),
            KeyCode::Enter if alt => composer.type_text("\n"),
            KeyCode::Enter => self.act(&Action::SaveComment),
            KeyCode::Tab => composer.next_type(),
            KeyCode::Backspace => composer.backspace(),
            KeyCode::Char(letter) if !ctrl => composer.type_text(letter.encode_utf8(&mut [0; 4])),
            _ => {}
        }
    }

    /// The thread drawn first under the diff's cursor row.
    pub fn thread_at_cursor(&self) -> Option<i64> {
        let view = self.diff.as_ref()?;
        view.thread_under(view.cursor)
    }

    /// The thread keys of the diff, as actions; `None` for any other key.
    fn thread_action(&self, key: KeyEvent) -> Option<Action> {
        let selecting = self.diff.as_ref().is_some_and(|view| view.anchor.is_some());
        Some(match key.code {
            KeyCode::Char('c') => Action::Comment,
            KeyCode::Char('}') => Action::JumpThread(true),
            KeyCode::Char('{') => Action::JumpThread(false),
            KeyCode::Char('r') => Action::Reply(self.thread_at_cursor()?),
            KeyCode::Char('R') => Action::Resolve(self.thread_at_cursor()?),
            KeyCode::Char('o') if !selecting => Action::Reopen(self.thread_at_cursor()?),
            KeyCode::Char('D') => Action::Delete(self.thread_at_cursor()?),
            KeyCode::Enter => {
                let id = self.thread_at_cursor()?;
                let resolved = self
                    .comment(id)
                    .is_some_and(|comment| comment.status == ReviewStatus::Resolved);
                if resolved {
                    Action::OpenThread(id)
                } else {
                    Action::ExpandThread(id)
                }
            }
            _ => return None,
        })
    }

    pub fn comment(&self, id: i64) -> Option<&ReviewComment> {
        self.comments.iter().find(|comment| comment.id == id)
    }

    /// Opens the comment box on the selected lines, or the line at the cursor, as the app's
    /// Comment on Selection does: the same spot and the same refusals (`gm_view::compose`).
    fn open_comment(&mut self) {
        let Some(view) = self.diff.as_ref() else {
            return;
        };
        let (from, to) = view.selected_rows().unwrap_or((view.cursor, view.cursor));
        let diffs = view.diffs();
        let covered = review::covered(&view.rows, diffs, from, to);
        let picked = review::picked(&view.rows, diffs, from, to);
        let spot = compose::diff_spot(side_of(view.context), covered, &picked, &view.path);
        if let Err(reason) = compose::spot_refusal(spot) {
            self.notice = Some(reason.to_owned());
            return;
        }
        let text = || (!picked.lines.is_empty()).then(|| picked.text());
        if let Some(target) = ReviewTarget::for_spot(spot, text) {
            self.notice = None;
            self.composer = Some(Composer::new(target));
        }
    }

    fn open_reply(&mut self, id: i64) {
        let Some(comment) = self.comment(id) else {
            return;
        };
        if !review::can_reply(comment.status) {
            self.notice = Some("Reopen the thread to reply to it.".to_owned());
            return;
        }
        let (start_line, end_line) = review::shown_range(comment);
        let target = ReviewTarget::Reply {
            id,
            path: comment.path.clone(),
            start_line,
            end_line,
        };
        self.composer = Some(Composer::new(target));
    }

    /// The resolve, reopen and delete of a thread.
    fn thread_write(&mut self, action: &Action) {
        let (id, wanted) = match *action {
            Action::Resolve(id) => (id, false),
            Action::Reopen(id) => (id, true),
            Action::Delete(id) => {
                match self.folds.delete.press(id, Instant::now()) {
                    Delete::Ask => {
                        self.notice = Some(format!(
                            "Press D, or [delete], again to delete thread {id}."
                        ));
                    }
                    Delete::Confirmed => {
                        self.notice = None;
                        self.commands.push(CommandKind::DeleteReviewComment { id });
                    }
                }
                return;
            }
            _ => return,
        };
        let Some(comment) = self.comment(id) else {
            return;
        };
        let resolved = comment.status == ReviewStatus::Resolved;
        if resolved != wanted {
            self.notice = Some(if wanted {
                "The thread is not resolved.".to_owned()
            } else {
                "The thread is already resolved.".to_owned()
            });
            return;
        }
        self.commands.push(if wanted {
            CommandKind::ReopenReviewComment { id }
        } else {
            CommandKind::ResolveReviewComment { id }
        });
    }

    fn diff_key_press(&mut self, key: KeyEvent) {
        if let Some(action) = self.thread_action(key) {
            self.act(&action);
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let half = self.half_page();
        let Some(view) = self.diff.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => view.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => view.move_cursor(-1),
            KeyCode::Char('d') if ctrl => view.move_cursor(half),
            KeyCode::Char('u') if ctrl => view.move_cursor(-half),
            KeyCode::Char('g') | KeyCode::Home => view.move_cursor(isize::MIN),
            KeyCode::Char('G') | KeyCode::End => view.move_cursor(isize::MAX),
            KeyCode::Char('J') => view.jump_hunk(true),
            KeyCode::Char('K') => view.jump_hunk(false),
            KeyCode::Char('V') => view.toggle_selection(),
            KeyCode::Char('o') => view.swap_ends(),
            KeyCode::Esc => view.anchor = None,
            KeyCode::Char(letter @ ('s' | 'u')) => {
                let wanted = if letter == 's' {
                    Context::Unstaged
                } else {
                    Context::Staged
                };
                if view.context != wanted {
                    return;
                }
                if view.anchor.is_some() {
                    self.stage_selection();
                } else if let Some(hunk) = view.cursor_hunk() {
                    self.stage_hunk(hunk);
                }
            }
            _ => return,
        }
        self.dirty = true;
    }

    /// Stages the selected lines, or unstages them under **Staged**: one command for each
    /// hunk the selection touches, each naming its hunk by id. Hunk ids hash content, not line
    /// numbers, so staging one hunk leaves the next one's id as it was.
    fn stage_selection(&mut self) {
        if !self.context.stages() {
            self.notice = Some(READ_ONLY.to_owned());
            return;
        }
        let Some(view) = self.diff.as_mut() else {
            return;
        };
        let changes = view.selected_changes();
        if changes.is_empty() {
            self.notice = Some("The selection has no added or removed line.".to_owned());
            return;
        }
        view.anchor = None;
        for (hunk_index, lines) in changes {
            let Some(hunk) = view.hunk(hunk_index) else {
                continue;
            };
            let target = StageTarget::Lines {
                path: view.path.clone(),
                hunk: hunk.id.clone(),
                lines,
            };
            self.commands.push(match view.context {
                Context::Unstaged => CommandKind::Stage { target },
                Context::Staged | Context::Branch => CommandKind::Unstage { target },
            });
        }
    }

    /// Stages the selected file, or unstages it under **Staged**.
    fn stage_file(&mut self) {
        if !self.context.stages() {
            self.notice = Some(READ_ONLY.to_owned());
            return;
        }
        let Some(path) = self.selected().map(|file| file.path.clone()) else {
            return;
        };
        let target = StageTarget::File { path };
        self.commands.push(match self.context {
            Context::Unstaged => CommandKind::Stage { target },
            Context::Staged | Context::Branch => CommandKind::Unstage { target },
        });
    }

    /// Stages hunk `index` of the diff on screen, or unstages it under **Staged**. The hunk is
    /// named by its id, so a hunk that changed meanwhile is refused, never guessed.
    fn stage_hunk(&mut self, index: usize) {
        if !self.context.stages() {
            self.notice = Some(READ_ONLY.to_owned());
            return;
        }
        let Some(view) = self.diff.as_ref() else {
            return;
        };
        let Some(hunk) = view.hunk(index) else {
            return;
        };
        let target = StageTarget::Hunk {
            path: view.path.clone(),
            hunk: hunk.id.clone(),
        };
        self.commands.push(match view.context {
            Context::Unstaged => CommandKind::Stage { target },
            Context::Staged | Context::Branch => CommandKind::Unstage { target },
        });
    }

    /// The action of a key that works everywhere.
    fn action_of(&self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.overlay.is_some() {
            return Some(match key.code {
                KeyCode::Char('q') if !ctrl => Action::Quit,
                KeyCode::Char('c') if ctrl => Action::Quit,
                _ => Action::CloseHelp,
            });
        }
        Some(match key.code {
            KeyCode::Char('q') if !ctrl => Action::Quit,
            KeyCode::Char('c') if ctrl => Action::Quit,
            KeyCode::Char('?') => Action::Help,
            KeyCode::Tab | KeyCode::BackTab => Action::ToggleFocus,
            KeyCode::Char('t') => Action::OtherContext,
            KeyCode::Char(key) => Action::Tab(Tab::ALL.into_iter().find(|tab| tab.key() == key)?),
            _ => return None,
        })
    }

    fn sidebar_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char('d') if ctrl => self.move_cursor(self.half_page()),
            KeyCode::Char('u') if ctrl => self.move_cursor(-self.half_page()),
            KeyCode::Char('g') | KeyCode::Home => self.move_cursor(isize::MIN),
            KeyCode::Char('G') | KeyCode::End => self.move_cursor(isize::MAX),
            KeyCode::Enter | KeyCode::Char(' ' | 'l' | 'h') => self.open_at_cursor(key.code),
            KeyCode::Char('s') if self.context == Context::Unstaged => self.stage_file(),
            KeyCode::Char('u') if self.context == Context::Staged => self.stage_file(),
            _ => return,
        }
        self.dirty = true;
    }

    /// Runs `action`, from a key or a click.
    pub fn act(&mut self, action: &Action) {
        // A notice is about the last action; the next one replaces it. Only a delete's
        // first press keeps its question up for the second.
        if !matches!(action, Action::Delete(_)) {
            self.notice = None;
        }
        match *action {
            Action::Tab(tab) => {
                self.tab = tab;
                self.select_review(true);
            }
            Action::Context(context) => self.set_context(context),
            Action::OtherContext => self.set_context(self.context.next()),
            Action::ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Main,
                    Focus::Main => Focus::Sidebar,
                };
            }
            Action::Help => self.overlay = Some(Overlay::Help),
            Action::CloseHelp => self.overlay = None,
            Action::Quit => self.quit = true,
            Action::Row(index) => {
                self.focus = Focus::Sidebar;
                self.cursor = index.min(self.rows.len().saturating_sub(1));
                self.follow_cursor = true;
                self.open_at_cursor(KeyCode::Char(' '));
            }
            Action::DiffRow(index) => {
                self.focus = Focus::Main;
                if let Some(view) = self.diff.as_mut() {
                    view.cursor = index.min(view.rows.len().saturating_sub(1));
                    view.follow_cursor = true;
                    view.anchor = None;
                    self.drag_from = Some(view.cursor);
                }
            }
            Action::StageFile => self.stage_file(),
            Action::StageSelection => self.stage_selection(),
            Action::Comment => self.open_comment(),
            Action::Reply(id) => self.open_reply(id),
            Action::Resolve(_) | Action::Reopen(_) | Action::Delete(_) => self.thread_write(action),
            Action::OpenThread(id) => {
                self.folds.toggle_opened(id);
                self.relayout();
            }
            Action::ExpandThread(id) => {
                self.folds.toggle_expanded(id);
                self.relayout();
            }
            Action::ToggleOutdated => {
                self.folds.outdated = !self.folds.outdated;
                self.relayout();
            }
            Action::JumpThread(forward) => {
                if let Some(view) = self.diff.as_mut() {
                    view.jump_thread(forward);
                }
            }
            Action::CommentType(index) => {
                if let Some(composer) = self.composer.as_mut() {
                    composer.kind = index;
                }
            }
            Action::SaveComment => {
                if let Some(command) = self.composer.as_ref().and_then(Composer::save) {
                    self.commands.push(command);
                }
            }
            Action::CancelComment => {
                if self.composer.as_mut().is_some_and(Composer::escape) {
                    self.composer = None;
                }
            }
            Action::ReviewRow(_)
            | Action::ReviewFilter(_)
            | Action::ReviewVersion(_)
            | Action::GoToDiff(_) => self.review_action(action),
            Action::ClearSelection => {
                if let Some(view) = self.diff.as_mut() {
                    view.anchor = None;
                }
            }
            Action::StageHunk(index) => self.stage_hunk(index),
            Action::StageCursorHunk => {
                if let Some(hunk) = self.diff.as_ref().and_then(DiffView::cursor_hunk) {
                    self.stage_hunk(hunk);
                }
            }
            Action::JumpHunk(forward) => {
                if let Some(view) = self.diff.as_mut() {
                    view.jump_hunk(forward);
                }
            }
        }
        self.dirty = true;
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        let at = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = self.hits.iter().rev().find(|hit| hit.area.contains(at));
                if let Some(action) = hit.map(|hit| hit.action.clone()) {
                    self.act(&action);
                } else if self.areas.main.is_some_and(|area| area.contains(at)) {
                    self.focus = Focus::Main;
                    self.dirty = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => self.drag_to(at),
            MouseEventKind::Up(MouseButton::Left) => self.drag_from = None,
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                if let Some(main) = self.areas.main.filter(|area| area.contains(at)) {
                    let by = if mouse.kind == MouseEventKind::ScrollDown {
                        isize::try_from(WHEEL_ROWS).unwrap_or(1)
                    } else {
                        -isize::try_from(WHEEL_ROWS).unwrap_or(1)
                    };
                    if let Some(view) = self.diff.as_mut().filter(|_| self.tab == Tab::Diff) {
                        view.scroll_by(by, usize::from(main.height.saturating_sub(1)));
                        self.dirty = true;
                    }
                    return;
                }
                let Some(list) = self.areas.list().filter(|area| area.contains(at)) else {
                    return;
                };
                if self.tab == Tab::Reviews {
                    let bottom = self
                        .reviews
                        .entries
                        .len()
                        .saturating_sub(usize::from(list.height));
                    self.reviews.scroll = if mouse.kind == MouseEventKind::ScrollDown {
                        (self.reviews.scroll + WHEEL_ROWS).min(bottom)
                    } else {
                        self.reviews.scroll.saturating_sub(WHEEL_ROWS)
                    };
                    self.dirty = true;
                    return;
                }
                if mouse.kind == MouseEventKind::ScrollDown {
                    let bottom = self.rows.len().saturating_sub(usize::from(list.height));
                    self.scroll = (self.scroll + WHEEL_ROWS).min(bottom);
                } else {
                    self.scroll = self.scroll.saturating_sub(WHEEL_ROWS);
                }
                self.follow_cursor = false;
                self.dirty = true;
            }
            _ => {}
        }
    }

    /// The left button is held over `at` after a press on a diff row: the rows between are
    /// selected, the cursor at `at`'s end.
    fn drag_to(&mut self, at: Position) {
        let Some(from) = self.drag_from else {
            return;
        };
        let Some(view) = self.diff.as_mut() else {
            return;
        };
        let offset = usize::from(at.y.saturating_sub(view.body_top));
        // A card under a row counts as that row.
        let Some(row) = view.row_of_entry(view.scroll + offset) else {
            return;
        };
        if row != from || view.anchor.is_some() {
            view.anchor = Some(from);
            view.cursor = row;
            view.follow_cursor = true;
            self.dirty = true;
        }
    }

    /// Moves `scroll` so the cursor is on screen in a list `height` rows high. Called by the
    /// draw, which knows the height.
    pub fn keep_cursor_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        let last = self.rows.len().saturating_sub(height);
        self.scroll = self.scroll.min(last);
        if !self.follow_cursor {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + height {
            self.scroll = self.cursor + 1 - height;
        }
    }

    fn ask_for_reads(&mut self) {
        self.commands.push(CommandKind::LoadChangeStats);
        self.commands.push(CommandKind::ListReviewComments {
            author: ReviewAuthorFilter::Any,
        });
        // The branch's changes move only with HEAD: a commit, a checkout, a rebase.
        let head = (
            self.status.branch.head.clone(),
            self.status.branch.oid.clone(),
        );
        if self.range_head.as_ref() != Some(&head) {
            self.range_head = Some(head);
            self.commands
                .push(CommandKind::LoadBranchChange { base: None });
        }
    }

    fn set_context(&mut self, context: Context) {
        if context != self.context {
            self.context = context;
            self.scroll = 0;
            self.follow_cursor = true;
            self.rebuild();
        }
    }

    /// Builds the rows again and puts the cursor back on the selected file, or near where it
    /// was when that file is gone.
    fn rebuild(&mut self) {
        let files = self.changed(self.context);
        self.rows = sidebar_rows(&files, &self.comments, &self.folded);
        let wanted = self.selected[self.context.index()].clone();
        let found = wanted.and_then(|path| {
            self.rows
                .iter()
                .position(|row| matches!(row, Row::File(file) if file.path == path))
        });
        self.cursor = match found {
            Some(index) => index,
            None => self.nearest_file().unwrap_or(0),
        };
        self.remember_selection();
    }

    /// The file row nearest the cursor, looking down first.
    fn nearest_file(&self) -> Option<usize> {
        let is_file = |index: &usize| matches!(self.rows[*index], Row::File(_));
        let from = self.cursor.min(self.rows.len().saturating_sub(1));
        (from..self.rows.len())
            .find(is_file)
            .or_else(|| (0..from).rev().find(is_file))
    }

    fn remember_selection(&mut self) {
        if let Some(Row::File(file)) = self.rows.get(self.cursor) {
            self.selected[self.context.index()] = Some(file.path.clone());
        }
        self.sync_diff();
    }

    fn half_page(&self) -> isize {
        let height = self.areas.list().map_or(MIN_HEIGHT, |area| area.height);
        isize::from(i16::try_from(height / 2).unwrap_or(i16::MAX)).max(1)
    }

    fn move_cursor(&mut self, by: isize) {
        let last = self.rows.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(by).min(last);
        self.follow_cursor = true;
        self.remember_selection();
    }

    /// `Enter`, `Space`, `l` or `h` on the row at the cursor: a folder folds or unfolds (`l`
    /// only unfolds, `h` only folds); a file goes to the main pane on `Enter`.
    fn open_at_cursor(&mut self, code: KeyCode) {
        match self.rows.get(self.cursor) {
            Some(Row::Dir { dir, folded, .. }) => {
                let fold = match code {
                    KeyCode::Char('l') => false,
                    KeyCode::Char('h') => true,
                    _ => !folded,
                };
                let dir = dir.clone();
                if fold {
                    self.folded.insert(dir);
                } else {
                    self.folded.remove(&dir);
                }
                let at = self.cursor;
                self.rebuild();
                self.cursor = at.min(self.rows.len().saturating_sub(1));
            }
            Some(Row::File(_)) => {
                self.remember_selection();
                if code == KeyCode::Enter {
                    self.focus = Focus::Main;
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use gm_schema::{BranchInfo, LineCounts, OperationId, ReviewAnchor};
    use ratatui::crossterm::event::KeyEventState;

    fn entry(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: path.to_owned(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
            submodule: None,
        }
    }

    fn status(entries: Vec<StatusEntry>) -> RepoStatus {
        RepoStatus {
            root: "/repo".to_owned(),
            branch: BranchInfo {
                head: Some("main".to_owned()),
                ..BranchInfo::default()
            },
            entries,
        }
    }

    pub(crate) fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    pub(crate) fn comment(id: i64, path: &str, status: ReviewStatus) -> ReviewComment {
        ReviewComment {
            id,
            path: path.to_owned(),
            body: "body".to_owned(),
            status,
            anchor: ReviewAnchor {
                text: "x".to_owned(),
                before: Vec::new(),
                after: Vec::new(),
                start_line: 1,
            },
            created_at: 0,
            resolved_at: None,
            snapshot: None,
            after_snapshot: None,
            location: None,
            outdated: false,
            replies: Vec::new(),
            author: None,
            kind: None,
            severity: None,
            needs_answer: false,
            suggested_fix: None,
            label: None,
            decorations: Vec::new(),
            scorecard: None,
            workspace: None,
        }
    }

    fn sample() -> App {
        let mut untracked = entry("src/new/", None, None);
        untracked.untracked = true;
        App::new(status(vec![
            entry("src/ledger/posting.rs", None, Some(Change::Modified)),
            entry(
                "src/ledger/lock.rs",
                Some(Change::Added),
                Some(Change::Modified),
            ),
            entry("Cargo.toml", None, Some(Change::Modified)),
            entry("src/api/post.rs", Some(Change::Deleted), None),
            untracked,
        ]))
    }

    fn shown(app: &App) -> Vec<String> {
        app.rows
            .iter()
            .map(|row| match row {
                Row::Dir { dir, files, folded } => {
                    format!("{} {dir} {files}", if *folded { '>' } else { 'v' })
                }
                Row::File(file) => {
                    let counts = file
                        .counts
                        .map_or_default(|(added, deleted)| format!(" +{added} -{deleted}"));
                    format!("  {} {}{counts} t{}", file.letter, file.name, file.threads)
                }
            })
            .collect()
    }

    fn selected_path(app: &App) -> Option<&str> {
        app.selected().map(|file| file.path.as_str())
    }

    #[test]
    fn the_sidebar_groups_files_by_folder_with_the_root_last() {
        let app = sample();
        assert_eq!(
            shown(&app),
            [
                "v src 1",
                "  ? new/ t0",
                "v src/ledger 2",
                "  M lock.rs t0",
                "  M posting.rs t0",
                "v  1",
                "  M Cargo.toml t0",
            ]
        );
        assert_eq!(app.file_count(Context::Unstaged), 4);
        assert_eq!(app.file_count(Context::Staged), 2);
    }

    #[test]
    fn the_staged_context_lists_the_staged_side() {
        let mut app = sample();
        app.on_key(key(KeyCode::Char('t')));
        assert_eq!(app.context, Context::Staged);
        assert_eq!(
            shown(&app),
            [
                "v src/api 1",
                "  D post.rs t0",
                "v src/ledger 1",
                "  A lock.rs t0"
            ]
        );
        assert_eq!(selected_path(&app), Some("src/api/post.rs"));
    }

    #[test]
    fn counts_and_unresolved_threads_join_their_files() {
        let mut app = sample();
        app.apply(&Event::ChangeStatsLoaded {
            id: OperationId(3),
            stats: ChangeStats {
                unstaged: vec![LineCounts {
                    path: "src/ledger/posting.rs".to_owned(),
                    added: 48,
                    deleted: 12,
                }],
                staged: Vec::new(),
            },
        });
        app.apply(&Event::ReviewCommentsListed {
            id: OperationId(4),
            comments: vec![
                comment(1, "src/ledger/posting.rs", ReviewStatus::Open),
                comment(2, "src/ledger/posting.rs", ReviewStatus::Handled),
                comment(3, "src/ledger/posting.rs", ReviewStatus::Resolved),
            ],
        });
        assert_eq!(shown(&app)[4], "  M posting.rs +48 -12 t2");
        assert_eq!(app.unresolved(), 2);
    }

    #[test]
    fn opening_and_each_new_status_ask_for_counts_and_threads() {
        let mut app = sample();
        let reads = [
            CommandKind::LoadChangeStats,
            CommandKind::ListReviewComments {
                author: ReviewAuthorFilter::Any,
            },
        ];
        let branch = CommandKind::LoadBranchChange { base: None };
        let mut opening = reads.to_vec();
        opening.push(branch.clone());
        assert_eq!(app.take_commands(), opening);
        app.apply(&Event::StatusLoaded {
            id: OperationId(2),
            status: app.status.clone(),
        });
        // HEAD did not move: the branch's changes are as they were.
        assert_eq!(app.take_commands(), reads);
        let mut committed = app.status.clone();
        committed.branch.oid = Some("b1".to_owned());
        app.apply(&Event::StatusLoaded {
            id: OperationId(3),
            status: committed,
        });
        assert_eq!(app.take_commands(), opening);
    }

    #[test]
    fn the_branch_context_lists_the_ranges_files_read_only() {
        use gm_schema::{CommitFile, RevisionRange};
        let mut app = sample();
        app.take_commands();
        app.sent(
            OperationId(4),
            &CommandKind::LoadBranchChange { base: None },
        );
        let file = |path: &str, change, added, deleted| CommitFile {
            path: path.to_owned(),
            old_path: None,
            change,
            added,
            deleted,
            binary: false,
        };
        app.apply(&Event::RevisionRangeLoaded {
            id: OperationId(4),
            range: RevisionRange {
                base: "base0".to_owned(),
                tip: "tip0".to_owned(),
                from_name: "main".to_owned(),
                to_name: "feature".to_owned(),
                files: vec![
                    file("src/ledger/posting.rs", Change::Modified, 48, 12),
                    file("README.md", Change::Added, 5, 0),
                ],
                ..RevisionRange::default()
            },
        });
        app.on_key(key(KeyCode::Char('t')));
        app.take_commands(); // Staged's diff, on the way
        app.on_key(key(KeyCode::Char('t')));
        assert_eq!(app.context, Context::Branch);
        assert_eq!(app.file_count(Context::Branch), 2);
        assert_eq!(
            shown(&app),
            [
                "v src/ledger 1",
                "  M posting.rs +48 -12 t0",
                "v  1",
                "  A README.md +5 -0 t0"
            ]
        );
        // The diffs are read in the range, all of the branch's files in one go.
        assert_eq!(
            app.take_commands(),
            [CommandKind::LoadRevisionStack {
                base: "base0".to_owned(),
                tip: "tip0".to_owned(),
                files: vec![
                    file("src/ledger/posting.rs", Change::Modified, 48, 12),
                    file("README.md", Change::Added, 5, 0),
                ],
            }]
        );
        app.sent(
            OperationId(5),
            &CommandKind::LoadRevisionStack {
                base: String::new(),
                tip: String::new(),
                files: Vec::new(),
            },
        );
        let diff = |path: &str| gm_schema::FileDiff {
            path: path.to_owned(),
            old_path: None,
            binary: false,
            hunks: Vec::new(),
        };
        app.apply(&Event::RevisionStackLoaded {
            id: OperationId(5),
            diffs: vec![diff("src/ledger/posting.rs"), diff("README.md")],
        });
        assert_eq!(
            app.diff
                .as_ref()
                .and_then(|view| view.diff.as_ref())
                .map(|d| d.path.as_str()),
            Some("src/ledger/posting.rs")
        );
        // Another file of the branch comes from what was read: nothing is asked.
        app.on_key(key(KeyCode::Char('G')));
        assert_eq!(
            app.diff
                .as_ref()
                .and_then(|view| view.diff.as_ref())
                .map(|d| d.path.as_str()),
            Some("README.md")
        );
        assert_eq!(app.take_commands(), []);
        // Nothing stages here.
        app.on_key(key(KeyCode::Char('s')));
        app.on_key(key(KeyCode::Char('u')));
        assert_eq!(app.take_commands(), []);
    }

    #[test]
    fn a_failed_branch_read_says_why() {
        let mut app = sample();
        app.take_commands();
        app.sent(
            OperationId(4),
            &CommandKind::LoadBranchChange { base: None },
        );
        app.apply(&Event::OperationChanged {
            id: OperationId(4),
            actor: gm_schema::Actor::Human,
            state: OperationState::Failed,
            detail: Some("You are on the default branch. Choose another base.".to_owned()),
        });
        assert!(app.range.is_none());
        assert_eq!(
            app.range_error.as_deref(),
            Some("You are on the default branch. Choose another base.")
        );
        assert_eq!(app.file_count(Context::Branch), 0);
    }

    #[test]
    fn the_first_file_is_selected_and_a_folder_folds() {
        let mut app = sample();
        assert_eq!(selected_path(&app), Some("src/new/"));
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Enter)); // on "src/ledger"
        assert_eq!(shown(&app)[2], "> src/ledger 2");
        assert_eq!(app.rows.len(), 5);
        // The folded folder keeps the last file selected.
        assert_eq!(selected_path(&app), Some("src/new/"));
        app.on_key(key(KeyCode::Char('l')));
        assert_eq!(shown(&app)[2], "v src/ledger 2");
    }

    #[test]
    fn a_new_status_keeps_the_selected_file() {
        let mut app = sample();
        app.on_key(key(KeyCode::Char('G')));
        assert_eq!(selected_path(&app), Some("Cargo.toml"));
        app.apply(&Event::StatusLoaded {
            id: OperationId(9),
            status: status(vec![
                entry("a.rs", None, Some(Change::Modified)),
                entry("Cargo.toml", None, Some(Change::Modified)),
            ]),
        });
        assert_eq!(selected_path(&app), Some("Cargo.toml"));
    }

    #[test]
    fn when_the_selected_file_leaves_the_context_the_cursor_finds_a_file_nearby() {
        let mut app = sample();
        app.apply(&Event::StatusLoaded {
            id: OperationId(9),
            status: status(vec![entry("Cargo.toml", None, Some(Change::Modified))]),
        });
        assert_eq!(selected_path(&app), Some("Cargo.toml"));
    }

    #[test]
    fn keys_choose_the_tab_the_focus_help_and_quit() {
        let mut app = sample();
        app.on_key(key(KeyCode::Char('3')));
        assert_eq!(app.tab, Tab::Reviews);
        app.on_key(key(KeyCode::Char('2')));
        assert_eq!(app.tab, Tab::File);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Main);
        app.on_key(key(KeyCode::Char('?')));
        assert_eq!(app.overlay, Some(Overlay::Help));
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.overlay, None);
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.quit);
    }

    #[test]
    fn a_click_runs_the_action_of_the_button_under_it() {
        let mut app = sample();
        app.areas = areas(Rect::new(0, 0, 120, 40), Focus::Sidebar);
        app.hits = vec![
            Hit {
                area: Rect::new(10, 0, 6, 1),
                action: Action::Tab(Tab::Reviews),
            },
            Hit {
                area: Rect::new(0, 5, 30, 1),
                action: Action::Row(6),
            },
        ];
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.on_mouse(click(12, 0));
        assert_eq!(app.tab, Tab::Reviews);
        app.on_mouse(click(3, 5));
        assert_eq!(selected_path(&app), Some("Cargo.toml"));
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[test]
    fn the_screen_splits_from_100_columns_and_is_too_small_below_80_by_24() {
        let wide = areas(Rect::new(0, 0, 120, 40), Focus::Sidebar);
        assert_eq!(wide.top, Some(Rect::new(0, 0, 120, 1)));
        assert_eq!(wide.sidebar, Some(Rect::new(0, 1, 40, 38)));
        assert_eq!(wide.main, Some(Rect::new(40, 1, 80, 38)));
        assert_eq!(wide.bar, Some(Rect::new(0, 39, 120, 1)));
        assert_eq!(wide.list(), Some(Rect::new(0, 2, 39, 37)));

        let narrow = areas(Rect::new(0, 0, 90, 30), Focus::Main);
        assert_eq!(narrow.sidebar, None);
        assert_eq!(narrow.main, Some(Rect::new(0, 1, 90, 28)));

        assert_eq!(
            areas(Rect::new(0, 0, 79, 40), Focus::Sidebar),
            Areas::default()
        );
        assert_eq!(
            areas(Rect::new(0, 0, 120, 23), Focus::Sidebar),
            Areas::default()
        );
    }

    #[test]
    fn the_wheel_scrolls_away_from_the_cursor_until_the_cursor_moves() {
        let entries = (0..50)
            .map(|n| entry(&format!("f{n:02}.rs"), None, Some(Change::Modified)))
            .collect();
        let mut app = App::new(status(entries));
        app.areas = areas(Rect::new(0, 0, 120, 40), Focus::Sidebar);
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        app.on_mouse(wheel);
        app.on_mouse(wheel);
        app.keep_cursor_visible(37);
        assert_eq!(app.scroll, 6);
        app.on_key(key(KeyCode::Down));
        app.keep_cursor_visible(37);
        assert_eq!(app.scroll, 2);
    }
}
