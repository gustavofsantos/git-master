//! The model of the terminal companion (D235, D236): what the screen shows and how keys and
//! clicks change it. Nothing here draws or touches the terminal, so every rule is tested on
//! its own. Each change the person asks of the repository leaves here as a `CommandKind`.
//!
//! Every button is an `Action`. A key maps to an action, and each draw records the area of
//! every button it drew with its action (`hits`), so a click runs the same action as the key
//! (D236 (2)).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use gm_schema::{
    Change, ChangeStats, CommandKind, Event, LineCounts, OperationId, OperationState, RepoStatus,
    ReviewAuthorFilter, ReviewComment, ReviewStatus, StageTarget, StatusEntry,
};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::diff::{ColorJob, DiffView, side_of};

/// Narrower than this, the sidebar and the main pane show one at a time (D235 (9)).
pub const SPLIT_MIN_WIDTH: u16 = 100;
/// Smaller than this, only a message shows (D235 (9)).
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
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

/// What the sidebar lists: the changes not staged yet, or the staged ones (D236 (3)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    Unstaged,
    Staged,
}

impl Context {
    pub const ALL: [Context; 2] = [Context::Unstaged, Context::Staged];

    pub fn title(self) -> &'static str {
        match self {
            Context::Unstaged => "Unstaged",
            Context::Staged => "Staged",
        }
    }

    pub fn other(self) -> Context {
        match self {
            Context::Unstaged => Context::Staged,
            Context::Staged => Context::Unstaged,
        }
    }

    fn index(self) -> usize {
        match self {
            Context::Unstaged => 0,
            Context::Staged => 1,
        }
    }
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
    status: &RepoStatus,
    context: Context,
    counts: &[LineCounts],
    comments: &[ReviewComment],
    folded: &BTreeSet<String>,
) -> Vec<Row> {
    let mut dirs: BTreeMap<&str, Vec<FileItem>> = BTreeMap::new();
    for (entry, letter) in files_of(status, context) {
        let (dir, name) = split_path(&entry.path);
        let counts = counts
            .iter()
            .find(|counts| counts.path == entry.path)
            .map(|counts| (counts.added, counts.deleted));
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
    /// A message for the top bar, such as why watching is off.
    pub notice: Option<String>,
    pub quit: bool,
    /// What changed since the last draw.
    pub dirty: bool,
    /// The next draw brings the cursor on screen. The wheel clears it, so the list scrolls
    /// away from the cursor until the cursor moves again.
    follow_cursor: bool,
    /// The file shown in the main pane, per context.
    selected: [Option<String>; 2],
    folded: BTreeSet<String>,
    /// The **Diff** tab's diff of the selected file.
    pub diff: Option<DiffView>,
    /// Counts the diff views made, so an answer for an older one is known.
    generation: u64,
    color_jobs: Vec<ColorJob>,
    /// The read of the diff on screen, to show why it failed.
    diff_request: Option<OperationId>,
    /// Writes in flight, with what to call them if they fail.
    writes: HashMap<OperationId, &'static str>,
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
            quit: false,
            dirty: true,
            follow_cursor: true,
            selected: [None, None],
            folded: BTreeSet::new(),
            diff: None,
            generation: 0,
            color_jobs: Vec::new(),
            diff_request: None,
            writes: HashMap::new(),
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
        files_of(&self.status, context).len()
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
        match kind {
            CommandKind::LoadDiff { .. } => self.diff_request = Some(id),
            CommandKind::Stage { .. } => {
                self.writes.insert(id, "Staging");
            }
            CommandKind::Unstage { .. } => {
                self.writes.insert(id, "Unstaging");
            }
            _ => {}
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
            }
            Event::DiffLoaded { side, diff, .. } => self.diff_loaded(*side, diff),
            Event::OperationChanged {
                id, state, detail, ..
            } => self.operation_changed(*id, *state, detail.as_deref()),
            _ => return,
        }
        self.dirty = true;
    }

    fn diff_loaded(&mut self, side: gm_schema::DiffSide, diff: &gm_schema::FileDiff) {
        let Some(view) = self.diff.as_mut() else {
            return;
        };
        if view.path != diff.path || side_of(view.context).diff_side() != side {
            return;
        }
        view.load(diff.clone());
        if let Some(job) = view.color_job() {
            self.color_jobs.push(job);
        }
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
        if self.diff_request == Some(id) {
            self.diff_request = None;
            if let Some(view) = self.diff.as_mut().filter(|_| failed) {
                view.error = Some(reason.clone());
            }
        }
        if let Some(what) = self.writes.remove(&id).filter(|_| failed) {
            self.notice = Some(format!("{what} failed: {reason}"));
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

    pub fn set_notice(&mut self, notice: Option<String>) {
        self.notice = notice;
        self.dirty = true;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if let Some(action) = self.action_of(key) {
            self.act(&action);
        } else if self.focus == Focus::Sidebar {
            self.sidebar_key(key);
        } else if self.tab == Tab::Diff {
            self.diff_key_press(key);
        }
    }

    fn diff_key_press(&mut self, key: KeyEvent) {
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
            KeyCode::Char(letter @ ('s' | 'u')) => {
                let wanted = if letter == 's' {
                    Context::Unstaged
                } else {
                    Context::Staged
                };
                if let Some(hunk) = view.cursor_hunk().filter(|_| view.context == wanted) {
                    self.stage_hunk(hunk);
                }
            }
            _ => return,
        }
        self.dirty = true;
    }

    /// Stages hunk `index` of the diff on screen, or unstages it under **Staged**. The hunk is
    /// named by its id, so a hunk that changed meanwhile is refused, never guessed.
    fn stage_hunk(&mut self, index: usize) {
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
            Context::Staged => CommandKind::Unstage { target },
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
            _ => return,
        }
        self.dirty = true;
    }

    /// Runs `action`, from a key or a click.
    pub fn act(&mut self, action: &Action) {
        match *action {
            Action::Tab(tab) => self.tab = tab,
            Action::Context(context) => self.set_context(context),
            Action::OtherContext => self.set_context(self.context.other()),
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
        let counts = match self.context {
            Context::Unstaged => &self.stats.unstaged,
            Context::Staged => &self.stats.staged,
        };
        self.rows = sidebar_rows(
            &self.status,
            self.context,
            counts,
            &self.comments,
            &self.folded,
        );
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
mod tests {
    use super::*;
    use gm_schema::{BranchInfo, OperationId, ReviewAnchor};
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

    fn comment(id: i64, path: &str, status: ReviewStatus) -> ReviewComment {
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
        assert_eq!(app.take_commands(), reads);
        app.apply(&Event::StatusLoaded {
            id: OperationId(2),
            status: app.status.clone(),
        });
        assert_eq!(app.take_commands(), reads);
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
