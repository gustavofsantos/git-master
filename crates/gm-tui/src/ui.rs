//! The companion's state and its actions (D241, D242, `design/tui-navigation.md`): the screen
//! stack, the overlays, the input through the navigator's key maps, and every action by its
//! name. A key, a click and a menu entry only name an action; the action finds its target
//! under the cursor or in the selection. Nothing here draws or touches the terminal.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::time::Instant;

use gm_schema::{
    CommandKind, Event, HistoryScope, LineKind, OperationId, OperationState, RepoStatus,
    ReviewAuthorFilter, ReviewStatus, StageTarget,
};
use gm_view::compose::{self, ReviewTarget};
use gm_view::diff_rows::Row as DiffRow;
use gm_view::local_review::Filter;
use gm_view::review::{self, Opened};
use ratatui::crossterm::event::{KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::agent_screen::AgentPage;
use crate::blame_screens;
use crate::data::{
    ColorJob, ColorTarget, FileText, Group, Repo, blame_color_job, diff_color_jobs, split_colors,
};
use crate::diff_screen::{self, DiffKind};
use crate::nav::Mode;
use crate::nav::action::{Behavior, Registry};
use crate::nav::defaults;
use crate::nav::input::{Input, Outcome};
use crate::nav::keys::{Code, Key};
use crate::nav::prompt::{Answer, Guard};
use crate::nav::sections::Tree;
use crate::nav::stack::{Addressed, Stack};
use crate::review_screens::{self, ThreadPage};
use crate::status::{self, Item, line_of};
use crate::threads::Composer;
use gm_view::provenance_view::Row as ProvenanceRow;

/// Smaller than this, only a message shows (D235 (9)).
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
/// Rows a turn of the mouse wheel scrolls.
const WHEEL_ROWS: isize = 3;

/// What a screen shows: its sections, with the cursor, folds and selection, and how far it
/// is scrolled.
#[derive(Debug, Default)]
pub struct View {
    pub tree: Tree<Item>,
    pub scroll: usize,
}

/// A screen of the stack. Status is the root; the other screens come with M3 to M6.
#[derive(Debug)]
pub enum Screen {
    Status(View),
    /// Every key (`?`), as a screen of its own so it scrolls and searches.
    Keys(View),
    /// The review threads under who acts next, narrowed by a filter (M3).
    Threads {
        view: View,
        filter: Filter,
    },
    /// The branch against its base, or a commit (M4).
    Diff {
        view: View,
        kind: DiffKind,
    },
    /// A whole file of the working tree (M4); `keep` is a line to show and put the cursor on.
    File {
        view: View,
        path: String,
        keep: Option<u32>,
    },
    /// Who last changed each line of a file (M5); `keep` is a line to put the cursor on.
    Blame {
        view: View,
        path: String,
        keep: Option<u32>,
    },
    /// How some lines got where they are (M5): `Repo::origin`.
    Origin {
        view: View,
    },
    /// A captured agent session (M6).
    Agent {
        view: View,
        page: Box<AgentPage>,
    },
    /// One review thread (M3).
    Thread {
        view: View,
        page: Box<ThreadPage>,
    },
}

impl Screen {
    pub fn view(&self) -> &View {
        match self {
            Screen::Status(view)
            | Screen::Keys(view)
            | Screen::Threads { view, .. }
            | Screen::Diff { view, .. }
            | Screen::File { view, .. }
            | Screen::Blame { view, .. }
            | Screen::Origin { view }
            | Screen::Agent { view, .. }
            | Screen::Thread { view, .. } => view,
        }
    }

    pub fn view_mut(&mut self) -> &mut View {
        match self {
            Screen::Status(view)
            | Screen::Keys(view)
            | Screen::Threads { view, .. }
            | Screen::Diff { view, .. }
            | Screen::File { view, .. }
            | Screen::Blame { view, .. }
            | Screen::Origin { view }
            | Screen::Agent { view, .. }
            | Screen::Thread { view, .. } => view,
        }
    }
}

impl Addressed for Screen {
    fn address(&self) -> String {
        match self {
            Screen::Status(_) => "status".to_owned(),
            Screen::Keys(_) => "keys".to_owned(),
            Screen::Threads { .. } => "threads".to_owned(),
            Screen::Diff { kind, .. } => kind.address(),
            Screen::File { path, .. } => format!("file/{path}"),
            Screen::Blame { path, .. } => format!("blame/{path}"),
            Screen::Origin { .. } => "origin".to_owned(),
            Screen::Agent { page, .. } => format!("agent/{}", page.uid),
            Screen::Thread { page, .. } => format!("thread/{}", page.id),
        }
    }
}

/// What a confirmed question does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Yes {
    Send(Vec<CommandKind>),
    /// Close the text box and lose what it holds.
    DiscardText,
}

/// An overlay over the top screen (D241 (1)).
#[derive(Debug)]
pub enum Overlay {
    Text {
        composer: Composer,
        guard: Guard,
    },
    Confirm {
        question: String,
        yes: Yes,
        guard: Guard,
    },
    Search {
        text: String,
        guard: Guard,
    },
}

impl Overlay {
    fn guard(&self) -> &Guard {
        match self {
            Overlay::Text { guard, .. }
            | Overlay::Confirm { guard, .. }
            | Overlay::Search { guard, .. } => guard,
        }
    }
}

/// What a click does: the same as a key or an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Click {
    /// A row of the top screen, by its index among the rows shown.
    Row(usize),
    /// A menu entry: the key that follows the pending prefix.
    Key(Key),
    /// A step of the trail: back to the screen at this depth.
    Trail(usize),
    /// An answer to the question on screen.
    Answer(Answer),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub area: Rect,
    pub click: Click,
}

/// What the loop has to do with the terminal for an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Suspend,
    Redraw,
    /// Put this text on the clipboard (OSC 52).
    Copy(String),
    /// Open the file at `path` (absolute) in `$VISUAL` or `$EDITOR`, at `line`.
    Edit {
        path: String,
        line: u32,
    },
}

/// What a read or a write in flight is for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    Diff(Group, String),
    Range,
    Stack,
    /// The branch's own commits.
    Commits,
    /// The commit the Diff screen shows, then its diffs.
    CommitDetail,
    CommitStack,
    /// The file the File screen shows.
    File,
    /// The blame the Blame screen shows.
    Blame,
    /// A page of the agent thread `uid`.
    Agent(String),
    Write(&'static str),
    Text,
}

pub struct App {
    pub repo: Repo,
    /// The read of the file the File screen shows, whose colors follow its answer.
    file_read: Option<OperationId>,
    pub stack: Stack<Screen>,
    pub input: Input,
    pub registry: Registry,
    pub overlays: Vec<Overlay>,
    /// Resolved threads opened, threads showing all replies, the outdated strips open.
    pub opened: BTreeSet<i64>,
    pub expanded: BTreeSet<i64>,
    pub outdated_open: bool,
    /// The last search.
    pub search: Option<String>,
    pub notice: Option<String>,
    pub watch_notice: Option<String>,
    /// Rows of the screen's body, as the last draw had them, for moving half a screen.
    pub body_rows: usize,
    pub hits: Vec<Hit>,
    pub requests: Vec<Request>,
    pub quit: bool,
    pub dirty: bool,
    /// The row a press of the left button started on; a drag selects from it.
    drag_from: Option<usize>,
    /// A thread to put the cursor on once its lines are read.
    reveal: Option<i64>,
    commands: Vec<CommandKind>,
    color_jobs: Vec<ColorJob>,
    /// Diffs asked for, in the order their commands go out, to match them to their ids.
    asked: VecDeque<(Group, String)>,
    in_flight: HashMap<OperationId, Pending>,
    range_head: Option<(Option<String>, Option<String>)>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("overlays", &self.overlays)
            .finish_non_exhaustive()
    }
}

impl App {
    pub fn new(status: RepoStatus) -> Self {
        let mut app = Self {
            repo: Repo {
                status,
                ..Repo::default()
            },
            file_read: None,
            stack: Stack::new(Screen::Status(View::default())),
            input: defaults::input(),
            registry: defaults::registry(),
            overlays: Vec::new(),
            opened: BTreeSet::new(),
            expanded: BTreeSet::new(),
            outdated_open: false,
            search: None,
            notice: None,
            watch_notice: None,
            body_rows: 20,
            hits: Vec::new(),
            requests: Vec::new(),
            quit: false,
            dirty: true,
            drag_from: None,
            reveal: None,
            commands: Vec::new(),
            color_jobs: Vec::new(),
            asked: VecDeque::new(),
            in_flight: HashMap::new(),
            range_head: None,
        };
        app.rebuild();
        app.ask_for_reads();
        app
    }

    pub fn take_commands(&mut self) -> Vec<CommandKind> {
        std::mem::take(&mut self.commands)
    }

    pub fn take_color_jobs(&mut self) -> Vec<ColorJob> {
        std::mem::take(&mut self.color_jobs)
    }

    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// Shows `text` on the hint line.
    pub fn notify(&mut self, text: String) {
        self.notice = Some(text);
        self.dirty = true;
    }

    pub fn set_watch_notice(&mut self, notice: Option<String>) {
        self.watch_notice = notice;
        self.dirty = true;
    }

    /// Whether a prompt on screen still waits before it accepts its keys.
    pub fn guard_closed(&self, now: Instant) -> bool {
        self.overlays
            .last()
            .map(Overlay::guard)
            .is_some_and(|guard| now < guard.opens_at())
    }

    pub fn redraw(&mut self) {
        self.dirty = true;
    }

    /// What the top screen shows.
    pub fn view(&self) -> &View {
        self.stack.top().view()
    }

    pub fn view_mut(&mut self) -> &mut View {
        self.stack.top_mut().view_mut()
    }

    fn tree(&self) -> &Tree<Item> {
        &self.view().tree
    }

    fn tree_mut(&mut self) -> &mut Tree<Item> {
        &mut self.view_mut().tree
    }

    /// Status's sections, wherever the stack is: the engine's answers land there.
    fn status_tree(&self) -> &Tree<Item> {
        &self.stack.root().view().tree
    }

    fn status_tree_mut(&mut self) -> &mut Tree<Item> {
        &mut self.stack.root_mut().view_mut().tree
    }

    /// The mode the keys are read in (D241 (2)).
    pub fn mode(&self) -> Mode {
        match self.overlays.last() {
            Some(Overlay::Text { .. } | Overlay::Search { .. }) => Mode::Insert,
            _ if self.tree().selection().is_some() => Mode::Visual,
            _ => Mode::Normal,
        }
    }

    // ---- The engine ----

    /// The engine took `kind` as operation `id`: the answers that matter are matched by it.
    pub fn sent(&mut self, id: OperationId, kind: &CommandKind) {
        let pending = match kind {
            CommandKind::LoadDiff { .. } => match self.asked.pop_front() {
                Some((group, path)) => Pending::Diff(group, path),
                None => return,
            },
            CommandKind::LoadBranchChange { .. } => Pending::Range,
            CommandKind::LoadRevisionStack { .. } => Pending::Stack,
            CommandKind::LoadHistory { .. } => Pending::Commits,
            CommandKind::LoadCommitDetail { .. } => Pending::CommitDetail,
            CommandKind::LoadBlame { .. } => Pending::Blame,
            CommandKind::ReadThread { uid, .. } => Pending::Agent(uid.clone()),
            CommandKind::ReadProvenance {
                path,
                from_line,
                to_line,
            } => {
                self.repo.origin.request(path, *from_line, *to_line, id);
                return;
            }
            CommandKind::LoadCommitStack { .. } => Pending::CommitStack,
            CommandKind::ReadFile { .. } => {
                self.file_read = Some(id);
                Pending::File
            }
            CommandKind::LoadReviewChange { .. }
            | CommandKind::LoadReviewSnapshotText { .. }
            | CommandKind::LoadReviewCurrentText { .. } => {
                for screen in self.stack.iter_mut() {
                    if let Screen::Thread { page, .. } = screen {
                        page.sent(id, kind);
                    }
                }
                return;
            }
            CommandKind::Stage { .. } => Pending::Write("Staging"),
            CommandKind::Unstage { .. } => Pending::Write("Unstaging"),
            CommandKind::ResolveReviewComment { .. } => Pending::Write("Resolving"),
            CommandKind::ReopenReviewComment { .. } => Pending::Write("Reopening"),
            CommandKind::DeleteReviewComment { .. } => Pending::Write("Deleting"),
            CommandKind::AddReviewComment { .. }
            | CommandKind::AddReviewCommentByText { .. }
            | CommandKind::ReplyToReviewComment { .. } => Pending::Text,
            _ => return,
        };
        self.in_flight.insert(id, pending);
    }

    /// Takes the colors of `job`, unless its target's text changed since it was sent: a late
    /// answer would paint the newer text with the older offsets.
    pub fn colored(&mut self, job: &ColorJob, spans: &[gm_schema::HighlightSpan]) {
        match &job.target {
            ColorTarget::Diff { group, path, .. } => {
                let Some(file) = self.repo.files.get_mut(&(*group, path.clone())) else {
                    return;
                };
                let current = file
                    .diff
                    .as_ref()
                    .is_some_and(|diff| diff_color_jobs(*group, diff).contains(job));
                if current {
                    file.colors.extend(split_colors(job, spans));
                    self.dirty = true;
                }
            }
            ColorTarget::Blame => {
                let current = self
                    .repo
                    .blame
                    .as_ref()
                    .and_then(|(path, lines)| blame_color_job(path, lines));
                if current.as_ref() == Some(job) {
                    self.repo.blame_colors = split_colors(job, spans)
                        .into_iter()
                        .map(|((index, _), colors)| (index, colors))
                        .collect();
                    self.dirty = true;
                }
            }
        }
    }

    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::StatusLoaded { status, .. } => {
                self.repo.status.clone_from(status);
                self.ask_for_reads();
                // The Blame screen's file may have changed too: its uncommitted lines move.
                let blamed = self
                    .stack
                    .trail()
                    .iter()
                    .find_map(|a| a.strip_prefix("blame/").map(str::to_owned));
                if let Some(path) = blamed {
                    self.commands.push(CommandKind::LoadBlame { path });
                }
                // The File screen's file may have changed too.
                let file = self
                    .stack
                    .trail()
                    .iter()
                    .find_map(|a| a.strip_prefix("file/").map(str::to_owned));
                if let Some(path) = file {
                    self.read_file(&path);
                }
                // Unfolded files may have changed: read them again; their old diff shows meanwhile.
                for (group, path) in status::unfolded_files(self.status_tree().toggled()) {
                    if group != Group::Branch {
                        self.ask_diff(group, &path);
                    }
                }
            }
            Event::ChangeStatsLoaded { stats, .. } => self.repo.stats.clone_from(stats),
            Event::ReviewCommentsListed { comments, .. } => {
                self.repo.comments.clone_from(comments);
                // A thread on screen reads its texts again: the agent may have handled it.
                self.ask_thread_texts();
            }
            Event::ReviewChange { .. }
            | Event::ReviewSnapshotText { .. }
            | Event::ReviewCurrentText { .. } => {
                let mut took = false;
                for screen in self.stack.iter_mut() {
                    if let Screen::Thread { page, .. } = screen {
                        took |= page.received(event);
                    }
                }
                if !took {
                    return;
                }
            }
            Event::DiffLoaded { id, diff, .. } => {
                if let Some(Pending::Diff(group, path)) = self.in_flight.remove(id) {
                    self.file_loaded(group, &path, diff.clone());
                }
            }
            Event::RevisionRangeLoaded { id, range }
                if self.in_flight.get(id) == Some(&Pending::Range) =>
            {
                self.in_flight.remove(id);
                self.repo.range = Some(range.clone());
                self.repo.range_error = None;
                self.repo
                    .files
                    .retain(|(group, _), _| *group != Group::Branch);
                // The branch's own commits: those of the range, as `git log base..tip`.
                self.commands.push(CommandKind::LoadHistory {
                    scope: HistoryScope::Ref {
                        name: format!("{}..{}", range.base, range.tip),
                    },
                });
            }
            Event::HistoryLoaded { id, history }
                if self.in_flight.get(id) == Some(&Pending::Commits) =>
            {
                self.in_flight.remove(id);
                self.repo.commits = Some(history.clone());
            }
            Event::StashesListed { stashes, .. } => self.repo.stashes.clone_from(stashes),
            Event::StacksLoaded { stacks, .. } => self.repo.stacks.clone_from(stacks),
            Event::RevisionStackLoaded { id, diffs }
                if self.in_flight.get(id) == Some(&Pending::Stack) =>
            {
                self.in_flight.remove(id);
                for diff in diffs {
                    self.file_loaded(Group::Branch, &diff.path.clone(), diff.clone());
                }
            }
            Event::OperationChanged {
                id, state, detail, ..
            } => self.ended(*id, *state, detail.as_deref()),
            _ if self.apply_screen_read(event) => {}
            _ => return,
        }
        self.rebuild();
        self.dirty = true;
    }

    /// Takes an answer to a read of the Diff or File screen; false when `event` is not one.
    fn apply_screen_read(&mut self, event: &Event) -> bool {
        match event {
            Event::CommitDetailLoaded { id, detail }
                if self.in_flight.get(id) == Some(&Pending::CommitDetail) =>
            {
                self.in_flight.remove(id);
                self.repo.commit = Some(detail.clone());
                self.repo
                    .files
                    .retain(|(group, _), _| *group != Group::Commit);
            }
            Event::CommitStackLoaded { id, diffs }
                if self.in_flight.get(id) == Some(&Pending::CommitStack) =>
            {
                self.in_flight.remove(id);
                for diff in diffs {
                    self.file_loaded(Group::Commit, &diff.path.clone(), diff.clone());
                }
            }
            Event::FileRead { id, file } if self.in_flight.get(id) == Some(&Pending::File) => {
                self.in_flight.remove(id);
                let mut read = FileText::from_view(file.clone());
                // The same text read again keeps its colors until the new ones come.
                if let Some(old) = self.repo.file.take()
                    && old.path == read.path
                    && old.lines == read.lines
                {
                    read.colors = old.colors;
                }
                self.repo.file = Some(read);
            }
            Event::BlameLoaded { id, path, lines }
                if self.in_flight.get(id) == Some(&Pending::Blame) =>
            {
                self.in_flight.remove(id);
                let same = self
                    .repo
                    .blame
                    .as_ref()
                    .is_some_and(|(of, old)| of == path && old == lines);
                if !same {
                    // A new text: its old colors would sit on the wrong lines.
                    self.repo.blame_colors.clear();
                    self.color_jobs.extend(blame_color_job(path, lines));
                }
                self.repo.blame = Some((path.clone(), lines.clone()));
            }
            Event::ThreadRead { id, detail } => {
                let Some(Pending::Agent(uid)) = self.in_flight.remove(id) else {
                    return false;
                };
                let mut next = None;
                for screen in self.stack.iter_mut() {
                    if let Screen::Agent { page, .. } = screen
                        && page.uid == uid
                    {
                        next = page.received(detail);
                    }
                }
                self.commands.extend(next);
            }
            Event::ThreadProvenance { id, provenance } => {
                return self.repo.origin.answered(*id, provenance.clone());
            }
            Event::FileHighlighted { id, spans } if self.file_read == Some(*id) => {
                if let Some(file) = self.repo.file.as_mut() {
                    file.colored(spans);
                }
            }
            _ => return false,
        }
        true
    }

    fn file_loaded(&mut self, group: Group, path: &str, diff: gm_schema::FileDiff) {
        self.color_jobs.extend(diff_color_jobs(group, &diff));
        self.repo
            .files
            .entry((group, path.to_owned()))
            .or_default()
            .load(diff, group.side());
    }

    fn ended(&mut self, id: OperationId, state: OperationState, detail: Option<&str>) {
        let failed = matches!(
            state,
            OperationState::Failed | OperationState::Vetoed | OperationState::Cancelled
        );
        if !failed && state != OperationState::Done {
            return;
        }
        // The Origin panel takes only the end of its read: it reads any other state as one.
        self.repo.origin.ended(id, state, detail);
        // A read on a job of its own reports Done before its answer: keep it until then.
        if !failed
            && matches!(
                self.in_flight.get(&id),
                Some(
                    Pending::Diff(..)
                        | Pending::Range
                        | Pending::Stack
                        | Pending::Commits
                        | Pending::CommitDetail
                        | Pending::CommitStack
                        | Pending::File
                        | Pending::Blame
                        | Pending::Agent(_)
                )
            )
        {
            return;
        }
        let reason = detail.unwrap_or("no reason given").to_owned();
        match self.in_flight.remove(&id) {
            Some(Pending::Diff(group, path)) => {
                self.repo.files.entry((group, path)).or_default().error = Some(reason);
            }
            Some(Pending::Range) => {
                self.repo.range = None;
                self.repo.range_error = Some(reason);
                self.repo.commits = None;
            }
            Some(Pending::Commits) => self.repo.commits = None,
            Some(Pending::CommitDetail | Pending::File | Pending::Blame) => {
                self.notice = Some(reason);
            }
            Some(Pending::Agent(uid)) => {
                for screen in self.stack.iter_mut() {
                    if let Screen::Agent { page, .. } = screen
                        && page.uid == uid
                        && page.detail.is_none()
                    {
                        page.failure = Some(reason.clone());
                    }
                }
            }
            Some(Pending::CommitStack) => {
                for ((group, _), file) in &mut self.repo.files {
                    if *group == Group::Commit && file.diff.is_none() {
                        file.error = Some(reason.clone());
                    }
                }
            }
            Some(Pending::Stack) => {
                for ((group, _), file) in &mut self.repo.files {
                    if *group == Group::Branch && file.diff.is_none() {
                        file.error = Some(reason.clone());
                    }
                }
            }
            Some(Pending::Write(what)) if failed => {
                self.notice = Some(format!("{what} failed: {reason}"));
            }
            Some(Pending::Text) => {
                if let Some(Overlay::Text { composer, .. }) = self.overlays.last_mut() {
                    if failed {
                        composer.pending = None;
                        composer.error = Some(reason);
                    } else {
                        self.overlays.pop();
                        self.tree_mut().clear_selection();
                    }
                }
            }
            _ => {}
        }
    }

    fn ask_for_reads(&mut self) {
        self.commands.push(CommandKind::LoadChangeStats);
        self.commands.push(CommandKind::ListReviewComments {
            author: ReviewAuthorFilter::Any,
        });
        // A stash changes the working tree, so its list moves with the status.
        self.commands.push(CommandKind::ListStashes);
        // The branch's changes move only with HEAD (D237 (3)).
        let head = (
            self.repo.status.branch.head.clone(),
            self.repo.status.branch.oid.clone(),
        );
        if self.range_head.as_ref() != Some(&head) {
            self.range_head = Some(head);
            self.commands
                .push(CommandKind::LoadBranchChange { base: None });
            self.commands.push(CommandKind::LoadStacks);
        }
    }

    fn ask_thread_texts(&mut self) {
        let mut reads = Vec::new();
        for screen in self.stack.iter_mut() {
            if let Screen::Thread { page, .. } = screen {
                reads.extend(page.reads(&self.repo));
            }
        }
        self.commands.extend(reads);
    }

    fn ask_diff(&mut self, group: Group, path: &str) {
        if path.ends_with('/') {
            return;
        }
        match group.diff_side() {
            Some(side) => {
                self.asked.push_back((group, path.to_owned()));
                self.commands.push(CommandKind::LoadDiff {
                    path: path.to_owned(),
                    side,
                });
            }
            None if group == Group::Commit => {
                if let Some(commit) = &self.repo.commit {
                    self.commands.push(CommandKind::LoadCommitStack {
                        oid: commit.oid.clone(),
                        files: commit.files.clone(),
                    });
                    for file in &commit.files {
                        self.repo
                            .files
                            .entry((Group::Commit, file.path.clone()))
                            .or_default();
                    }
                }
            }
            // The branch's diff is one read for all its files (D237 (2)).
            None => {
                if let Some(range) = &self.repo.range {
                    self.commands.push(CommandKind::LoadRevisionStack {
                        base: range.base.clone(),
                        tip: range.tip.clone(),
                        files: range.files.clone(),
                    });
                    for file in &range.files {
                        self.repo
                            .files
                            .entry((Group::Branch, file.path.clone()))
                            .or_default();
                    }
                }
            }
        }
    }

    /// Lays Status out again from what the repository holds, reads the diffs of files just
    /// unfolded, and puts the cursor on a thread waiting to be revealed.
    fn rebuild(&mut self) {
        let opened = Opened {
            outdated: self.outdated_open,
            comments: &self.opened,
            threads: &self.expanded,
        };
        let sections = status::sections(&self.repo, opened);
        self.status_tree_mut().set(sections);
        let missing: Vec<(Group, String)> = status::unfolded_files(self.status_tree().toggled())
            .into_iter()
            .filter(|key| !self.repo.files.contains_key(key))
            .collect();
        let asked = !missing.is_empty();
        for (group, path) in missing {
            self.repo.files.entry((group, path.clone())).or_default();
            self.ask_diff(group, &path);
        }
        if asked {
            // The files just asked for show that their diff is being read.
            let opened = Opened {
                outdated: self.outdated_open,
                comments: &self.opened,
                threads: &self.expanded,
            };
            let sections = status::sections(&self.repo, opened);
            self.status_tree_mut().set(sections);
        }
        let repo = &self.repo;
        let opened = Opened {
            outdated: self.outdated_open,
            comments: &self.opened,
            threads: &self.expanded,
        };
        let mut wanted: Vec<(Group, String)> = Vec::new();
        for screen in self.stack.iter_mut() {
            match screen {
                Screen::Threads { view, filter } => {
                    view.tree.set(review_screens::threads(repo, *filter));
                }
                Screen::Thread { view, page } => view.tree.set(page.sections(repo)),
                Screen::Diff { view, kind } => {
                    view.tree.set(diff_screen::diff(repo, kind, opened));
                    let group = kind.group();
                    wanted.extend(
                        status::files_of(repo, group)
                            .into_iter()
                            .filter(|(path, ..)| !repo.files.contains_key(&(group, path.clone())))
                            .map(|(path, ..)| (group, path)),
                    );
                }
                Screen::File { view, path, keep } => {
                    view.tree.set(diff_screen::file(repo, path, *keep));
                    if let Some(line) =
                        keep.filter(|_| repo.file.as_ref().is_some_and(|f| &f.path == path))
                    {
                        view.tree.move_to_id(&format!("line/{line}"));
                        *keep = None;
                    }
                }
                Screen::Blame { view, path, keep } => {
                    view.tree.set(blame_screens::blame(repo, path));
                    if let Some((_, lines)) = repo.blame.as_ref().filter(|(of, _)| of == path)
                        && let Some(line) = keep.take()
                    {
                        let at = line.min(u32::try_from(lines.len()).unwrap_or(u32::MAX));
                        view.tree.move_to_id(&format!("line/{at}"));
                    }
                }
                Screen::Origin { view } => view.tree.set(blame_screens::origin(repo)),
                Screen::Agent { view, page } => view.tree.set(page.sections()),
                Screen::Status(_) | Screen::Keys(_) => {}
            }
        }
        // A diff screen's files are read once, together.
        if let Some((group, path)) = wanted.into_iter().next() {
            self.ask_diff(group, &path);
            self.rebuild();
        }
        if let Some(id) = self.reveal {
            let card = self
                .tree()
                .rows()
                .iter()
                .find(|row| matches!(row.section.item, Item::Card { id: card, .. } if card == id))
                .map(|row| row.section.id.clone());
            if let Some(card) = card {
                self.status_tree_mut().move_to_id(&card);
                self.reveal = None;
            }
        }
    }

    // ---- Keys and clicks ----

    pub fn on_key(&mut self, event: KeyEvent) {
        self.on_key_at(event, Instant::now());
    }

    /// A key at `now` (the prompts' guard reads the time).
    pub fn on_key_at(&mut self, event: KeyEvent, now: Instant) {
        if event.kind == KeyEventKind::Release {
            return;
        }
        let Some(key) = Key::from_event(event) else {
            return;
        };
        self.dirty = true;
        if key == Key::ctrl('c') {
            self.quit = true;
            return;
        }
        if !self.overlays.is_empty() {
            self.overlay_key(key, now);
            return;
        }
        let mode = self.mode();
        let prefix: Vec<Key> = self
            .input
            .pending(mode)
            .map_or_default(|(keys, _)| keys.to_vec());
        match self.input.feed(mode, key) {
            Outcome::Action(name) => {
                self.run(name, now);
                // A menu for repeating stays open (D241 (3)).
                let repeat = self
                    .registry
                    .get(name)
                    .is_some_and(|spec| spec.behavior == Behavior::Repeat);
                if repeat && !prefix.is_empty() && self.overlays.is_empty() {
                    self.input.keep(&prefix);
                }
            }
            Outcome::Quit => self.quit = true,
            Outcome::Pending(_) | Outcome::Left | Outcome::Type(_) | Outcome::Unbound => {}
        }
    }

    fn overlay_key(&mut self, key: Key, now: Instant) {
        let guard = self.overlays.last().map(Overlay::guard).copied();
        let allowed = |answer| guard.is_none_or(|guard: Guard| guard.accepts(answer, now));
        match self.overlays.last_mut() {
            Some(Overlay::Confirm { .. }) => {
                let answer = match key.code {
                    Code::Char('y' | 'Y') | Code::Enter => Answer::Confirm,
                    Code::Char('n' | 'N' | 'q') | Code::Esc => Answer::Cancel,
                    _ => return,
                };
                if allowed(answer) {
                    self.answer(answer);
                }
            }
            Some(Overlay::Search { text, .. }) => match self.input.feed(Mode::Insert, key) {
                Outcome::Type(c) => text.push(c),
                Outcome::Action("text.backspace") => {
                    text.pop();
                }
                Outcome::Action("text.newline" | "text.send") => {
                    let text = text.clone();
                    self.overlays.pop();
                    self.search = (!text.is_empty()).then_some(text);
                    self.find(true);
                }
                Outcome::Action("text.cancel") if allowed(Answer::Cancel) => {
                    self.overlays.pop();
                }
                _ => {}
            },
            Some(Overlay::Text { composer, .. }) => match self.input.feed(Mode::Insert, key) {
                Outcome::Type(c) => composer.type_text(c.encode_utf8(&mut [0; 4])),
                Outcome::Action("text.newline") => composer.type_text("\n"),
                Outcome::Action("text.backspace") => composer.backspace(),
                Outcome::Action("text.type") => composer.next_type(),
                Outcome::Action("text.send") if allowed(Answer::Confirm) => {
                    if let Some(command) = composer.save() {
                        self.commands.push(command);
                    }
                }
                Outcome::Action("text.cancel") if allowed(Answer::Cancel) => {
                    if composer.text.trim().is_empty() {
                        self.overlays.pop();
                    } else {
                        self.overlays.push(Overlay::Confirm {
                            question: "Discard what you wrote?".to_owned(),
                            yes: Yes::DiscardText,
                            guard: Guard::new(now, true),
                        });
                    }
                }
                _ => {}
            },
            None => {}
        }
    }

    /// The answer to the question on screen.
    fn answer(&mut self, answer: Answer) {
        let Some(Overlay::Confirm { yes, .. }) = self.overlays.pop() else {
            return;
        };
        if answer == Answer::Cancel {
            return;
        }
        match yes {
            Yes::Send(commands) => self.commands.extend(commands),
            Yes::DiscardText => {
                if matches!(self.overlays.last(), Some(Overlay::Text { .. })) {
                    self.overlays.pop();
                }
            }
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        match self.overlays.last_mut() {
            Some(Overlay::Text { composer, .. }) => composer.type_text(text),
            Some(Overlay::Search { text: search, .. }) => search.push_str(text),
            _ => return,
        }
        self.dirty = true;
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        self.on_mouse_at(mouse, Instant::now());
    }

    pub fn on_mouse_at(&mut self, mouse: MouseEvent, now: Instant) {
        let at = Position::new(mouse.column, mouse.row);
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|hit| hit.area.contains(at))
            .map(|hit| hit.click.clone());
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => match hit {
                Some(Click::Row(index)) => {
                    self.tree_mut().clear_selection();
                    self.tree_mut().move_to(index);
                    self.drag_from = Some(index);
                    if self
                        .tree()
                        .current()
                        .is_some_and(|s| !s.children.is_empty())
                    {
                        self.run("fold.toggle", now);
                    }
                }
                Some(Click::Key(key)) => {
                    let mode = self.mode();
                    if let Outcome::Action(name) = self.input.feed(mode, key) {
                        self.run(name, now);
                    }
                }
                Some(Click::Trail(depth)) => self.stack.back_to(depth + 1),
                Some(Click::Answer(answer)) => {
                    let allowed = self
                        .overlays
                        .last()
                        .map(Overlay::guard)
                        .is_none_or(|guard| guard.accepts(answer, now));
                    if allowed {
                        self.answer(answer);
                    }
                }
                None => return,
            },
            MouseEventKind::Drag(MouseButton::Left) => {
                let (Some(from), Some(Click::Row(index))) = (self.drag_from, hit) else {
                    return;
                };
                if index != from && self.tree().selection().is_none() {
                    self.tree_mut().move_to(from);
                    self.tree_mut().toggle_selection();
                }
                self.tree_mut().move_to(index);
            }
            MouseEventKind::Up(MouseButton::Left) => self.drag_from = None,
            MouseEventKind::ScrollDown => self.scroll_by(WHEEL_ROWS),
            MouseEventKind::ScrollUp => self.scroll_by(-WHEEL_ROWS),
            _ => return,
        }
        self.dirty = true;
    }

    fn scroll_by(&mut self, by: isize) {
        let total = self.tree().rows().len();
        let rows = self.body_rows;
        let screen = self.view_mut();
        screen.scroll = screen
            .scroll
            .saturating_add_signed(by)
            .min(total.saturating_sub(rows));
    }

    // ---- Actions (D241 (3)) ----

    /// Whether `name` applies to what is under the cursor or selected: the menu lists only
    /// those, and a palette would.
    pub fn applies(&self, name: &str) -> bool {
        if self
            .registry
            .get(name)
            .is_some_and(|spec| !spec.modes.contains(&self.mode()))
        {
            return false;
        }
        let spec = self.registry.get(name);
        let moves = name.starts_with("cursor.")
            || matches!(name, "screen.back" | "screen.close")
            || spec.is_some_and(|s| matches!(s.group, "Fold" | "Search" | "Help"));
        let threads = name.starts_with("thread.") || matches!(name, "open" | "go.threads");
        let allowed = match self.stack.top() {
            // The Keys screen only moves, folds, searches and goes back.
            Screen::Keys(_) => moves,
            Screen::Threads { .. } => moves || threads || name.starts_with("filter."),
            Screen::Thread { .. } => moves || threads || name == "file.open",
            Screen::Diff { .. } | Screen::Status(_) => !name.starts_with("filter."),
            Screen::File { .. } | Screen::Blame { .. } => {
                moves
                    || name.starts_with("select.")
                    || matches!(
                        name,
                        "open"
                            | "go.threads"
                            | "go.branch"
                            | "file.open"
                            | "blame.open"
                            | "origin.open"
                    )
            }
            Screen::Origin { .. } => moves || matches!(name, "open" | "go.threads"),
            Screen::Agent { .. } => {
                moves || matches!(name, "open" | "go.threads" | "file.open" | "blame.open")
            }
        };
        if !allowed {
            return false;
        }
        if !matches!(self.stack.top(), Screen::Status(_)) && moves {
            return true;
        }
        let current = self.tree().current().map(|s| &s.item);
        let group = current
            .and_then(Item::file)
            .map(|(group, _)| group)
            .or(match current {
                Some(Item::Group { group, .. }) => Some(*group),
                _ => None,
            });
        let thread = self.current_thread().and_then(|id| self.repo.comment(id));
        match name {
            "stage" => group.and_then(Group::stages) == Some(true),
            "unstage" => group.and_then(Group::stages) == Some(false),
            "stage.file" => current
                .and_then(Item::file)
                .is_some_and(|(g, _)| g.stages() == Some(true)),
            "review.comment" => {
                self.tree().selection().is_some() || matches!(current, Some(Item::Line { .. }))
            }
            "thread.reply" => thread.is_some_and(|c| review::can_reply(c.status)),
            "thread.resolve" => thread.is_some_and(|c| c.status != ReviewStatus::Resolved),
            "thread.reopen" => thread.is_some_and(|c| c.status == ReviewStatus::Resolved),
            "thread.delete" | "thread.show" => thread.is_some(),
            "copy.reference" => current.and_then(Item::file).is_some(),
            "go.branch" => self.repo.range.is_some(),
            "file.open" | "blame.open" => self.current_file().is_some(),
            "origin.open" => self.selected_lines().is_some(),
            _ => true,
        }
    }

    /// Runs the action `name` on what is under the cursor or selected.
    pub fn run(&mut self, name: &str, now: Instant) {
        if self.registry.get(name).is_some() && !self.applies(name) {
            self.notice = Some(format!(
                "{} does not apply here.",
                self.registry.get(name).map_or(name, |s| s.title)
            ));
            return;
        }
        self.notice = None;
        let half = isize::try_from(self.body_rows / 2).unwrap_or(10).max(1);
        match name {
            "cursor.down" => self.tree_mut().move_by(1),
            "cursor.up" => self.tree_mut().move_by(-1),
            "cursor.page_down" => self.tree_mut().move_by(half),
            "cursor.page_up" => self.tree_mut().move_by(-half),
            "cursor.top" => self.tree_mut().move_to(0),
            "cursor.bottom" => self.tree_mut().move_to(usize::MAX),
            "hunk.next" => self
                .tree_mut()
                .jump(true, |item| matches!(item, Item::Hunk { .. })),
            "hunk.previous" => self
                .tree_mut()
                .jump(false, |item| matches!(item, Item::Hunk { .. })),
            "select.toggle" => self.tree_mut().toggle_selection(),
            "select.down" => self.tree_mut().extend(1),
            "select.up" => self.tree_mut().extend(-1),
            _ if self.run_screen_action(name) => {}
            "fold.toggle" | "open"
                if name == "fold.toggle"
                    || self
                        .tree()
                        .current()
                        .and_then(|s| s.item.thread())
                        .is_none() =>
            {
                self.tree_mut().toggle();
                self.rebuild();
            }
            "open" => {
                if let Some(id) = self.current_thread() {
                    self.open_thread(id);
                }
            }
            "fold.open" | "fold.close" => {
                self.tree_mut().set_fold(name == "fold.close");
                self.rebuild();
            }
            "fold.open_all" | "fold.close_all" => {
                self.tree_mut().set_all(name == "fold.close_all");
                self.rebuild();
            }
            "screen.back" | "screen.close" => {
                if self.tree().selection().is_some() {
                    self.tree_mut().clear_selection();
                } else if !self.stack.back() && name == "screen.close" {
                    self.quit = true;
                }
            }
            "search.start" => self.overlays.push(Overlay::Search {
                text: String::new(),
                guard: Guard::new(now, false),
            }),
            "search.next" => self.find(true),
            "search.previous" => self.find(false),
            "stage" => self.stage(true),
            "unstage" => self.stage(false),
            "stage.file" => {
                if let Some((_, path)) = self.tree().current().and_then(|s| s.item.file()) {
                    let target = StageTarget::File {
                        path: path.to_owned(),
                    };
                    self.commands.push(CommandKind::Stage { target });
                }
            }
            "review.comment" => self.open_comment(now),
            "thread.reply" => self.open_reply(now),
            "thread.resolve" | "thread.reopen" | "thread.delete" => self.thread_write(name, now),
            "copy.reference" => self.copy_reference(),
            "terminal.suspend" => self.requests.push(Request::Suspend),
            "terminal.redraw" => self.requests.push(Request::Redraw),
            "quit" => self.quit = true,
            _ => {}
        }
    }

    /// The diff rows a selection, or the line at the cursor, covers in one file: the file and
    /// the first and last of its `gm_view` rows. `None` over several files or on no line.
    fn picked_rows(&self) -> Option<(Group, String, usize, usize)> {
        let rows = self.tree().rows();
        let (first, last) = self
            .tree()
            .selection()
            .or_else(|| self.tree().cursor_index().map(|at| (at, at)))?;
        let mut file: Option<(Group, String)> = None;
        let mut span: Option<(usize, usize)> = None;
        for row in &rows[first..=last.min(rows.len().saturating_sub(1))] {
            if let Item::Line { group, path, row } = &row.section.item {
                if file.as_ref().is_some_and(|(g, p)| g != group || p != path) {
                    return None;
                }
                file = Some((*group, path.clone()));
                span = Some(span.map_or((*row, *row), |(a, b)| (a.min(*row), b.max(*row))));
            }
        }
        let (group, path) = file?;
        let (from, to) = span?;
        Some((group, path, from, to))
    }

    fn stage(&mut self, stage: bool) {
        let make = |target| {
            if stage {
                CommandKind::Stage { target }
            } else {
                CommandKind::Unstage { target }
            }
        };
        if self.tree().selection().is_some() {
            let Some((group, path, from, to)) = self.picked_rows() else {
                self.notice = Some("Select lines of one file to stage them.".to_owned());
                return;
            };
            let Some(file) = self.repo.files.get(&(group, path.clone())) else {
                return;
            };
            let Some(diff) = &file.diff else { return };
            let mut per_hunk: Vec<(usize, Vec<u32>)> = Vec::new();
            for row in &file.rows[from..=to] {
                let DiffRow::Line {
                    hunk_index,
                    line_index,
                    ..
                } = *row
                else {
                    continue;
                };
                let changed = diff.hunks[hunk_index].lines[line_index].kind != LineKind::Context;
                let Ok(index) = u32::try_from(line_index) else {
                    continue;
                };
                if !changed {
                    continue;
                }
                match per_hunk.last_mut() {
                    Some((hunk, lines)) if *hunk == hunk_index => lines.push(index),
                    _ => per_hunk.push((hunk_index, vec![index])),
                }
            }
            if per_hunk.is_empty() {
                self.notice = Some("The selection has no added or removed line.".to_owned());
                return;
            }
            for (hunk, lines) in per_hunk {
                let target = StageTarget::Lines {
                    path: path.clone(),
                    hunk: diff.hunks[hunk].id.clone(),
                    lines,
                };
                self.commands.push(make(target));
            }
            self.tree_mut().clear_selection();
            return;
        }
        let Some(item) = self.tree().current().map(|s| s.item.clone()) else {
            return;
        };
        let on_line = matches!(item, Item::Line { .. });
        let targets: Vec<StageTarget> = match item {
            Item::Group { group, .. } => status_files(&self.repo, group)
                .into_iter()
                .map(|path| StageTarget::File { path })
                .collect(),
            Item::File { path, .. } => vec![StageTarget::File { path }],
            Item::Hunk { group, path, hunk }
            | Item::Line {
                group,
                path,
                row: hunk,
            } => {
                let file = self.repo.files.get(&(group, path.clone()));
                let hunk = if on_line {
                    match file.and_then(|f| f.rows.get(hunk)) {
                        Some(DiffRow::Line { hunk_index, .. }) => *hunk_index,
                        _ => return,
                    }
                } else {
                    hunk
                };
                match file
                    .and_then(|f| f.diff.as_ref())
                    .and_then(|d| d.hunks.get(hunk))
                {
                    Some(found) => vec![StageTarget::Hunk {
                        path,
                        hunk: found.id.clone(),
                    }],
                    None => return,
                }
            }
            _ => return,
        };
        self.commands.extend(targets.into_iter().map(make));
    }

    fn open_comment(&mut self, now: Instant) {
        let Some((group, path, from, to)) = self.picked_rows() else {
            self.notice = Some("Select lines of one file to comment on.".to_owned());
            return;
        };
        let Some(file) = self.repo.files.get(&(group, path.clone())) else {
            return;
        };
        let diffs = file.diffs(group.side());
        let covered = review::covered(&file.rows, diffs, from, to);
        let picked = review::picked(&file.rows, diffs, from, to);
        let spot = compose::diff_spot(group.side(), covered, &picked, &path);
        if let Err(reason) = compose::spot_refusal(spot) {
            self.notice = Some(reason.to_owned());
            return;
        }
        let text = || (!picked.lines.is_empty()).then(|| picked.text());
        if let Some(target) = ReviewTarget::for_spot(spot, text) {
            self.overlays.push(Overlay::Text {
                composer: Composer::new(target),
                guard: Guard::new(now, true),
            });
        }
    }

    /// Runs an action that opens or changes a screen; false when `name` is not one.
    fn run_screen_action(&mut self, name: &str) -> bool {
        match name {
            "open" if self.on_thread_screen() => self.show_thread(),
            "open" if self.edit_target().is_some() => {
                if let Some((path, line)) = self.edit_target() {
                    let path = std::path::Path::new(&self.repo.status.root).join(path);
                    self.requests.push(Request::Edit {
                        path: path.to_string_lossy().into_owned(),
                        line,
                    });
                }
            }
            "open" if self.current_commit().is_some() => {
                if let Some(oid) = self.current_commit() {
                    self.open_diff(DiffKind::Commit(oid));
                }
            }
            "open" if self.current_agent_thread().is_some() => {
                if let Some(uid) = self.current_agent_thread() {
                    self.open_agent(uid);
                }
            }
            "go.branch" => self.open_diff(DiffKind::Branch),
            "blame.open" => {
                if let Some((path, line)) = self.current_file() {
                    self.open_blame(path, line);
                }
            }
            "origin.open" => {
                if let Some((path, from, to)) = self.selected_lines() {
                    self.open_origin(path, from, to);
                }
            }
            "file.open" => {
                if let Some((path, line)) = self.current_file() {
                    self.open_file(path, line);
                }
            }
            "thread.show" => self.show_thread(),
            "go.threads" => {
                self.stack.open(Screen::Threads {
                    view: View::default(),
                    filter: Filter::Open,
                });
                self.rebuild();
            }
            _ if name.starts_with("filter.") => {
                if let (Some(chosen), Screen::Threads { filter, .. }) =
                    (review_screens::filter_of(name), self.stack.top_mut())
                {
                    *filter = chosen;
                }
                self.rebuild();
            }
            "help.keys" => {
                let mut view = View::default();
                view.tree
                    .set(crate::help::sections(&self.registry, &self.input));
                self.stack.open(Screen::Keys(view));
            }
            _ => return false,
        }
        true
    }

    /// The file and line `Enter` opens in the editor: a line of a diff or of the File screen.
    fn edit_target(&self) -> Option<(String, u32)> {
        match &self.tree().current()?.item {
            // A removed line opens at its old number, near where it was.
            Item::Line { group, path, row } => {
                let line = line_of(&self.repo, *group, path, *row)?;
                Some((path.clone(), line.new_no.or(line.old_no)?))
            }
            item => self.numbered(item),
        }
    }

    /// The file and the line number in the working tree of a line row: a diff line's new
    /// number, a File line, a Blame line.
    fn numbered(&self, item: &Item) -> Option<(String, u32)> {
        match item {
            Item::Line { group, path, row } => line_of(&self.repo, *group, path, *row)
                .and_then(|line| line.new_no)
                .map(|n| (path.clone(), n)),
            Item::FileLine { index, .. } => {
                let file = self.repo.file.as_ref()?;
                Some((file.path.clone(), u32::try_from(index + 1).ok()?))
            }
            Item::BlameLine { index } => {
                let (path, lines) = self.repo.blame.as_ref()?;
                Some((path.clone(), lines.get(*index)?.line))
            }
            _ => None,
        }
    }

    /// The file and the lines a selection covers, or the line at the cursor: of a diff, the
    /// File screen or the Blame screen. `None` over several files or on no line.
    fn selected_lines(&self) -> Option<(String, u32, u32)> {
        let rows = self.tree().rows();
        let (first, last) = self
            .tree()
            .selection()
            .or_else(|| self.tree().cursor_index().map(|at| (at, at)))?;
        let mut path: Option<String> = None;
        let mut span: Option<(u32, u32)> = None;
        for row in &rows[first..=last.min(rows.len().saturating_sub(1))] {
            let found = self.numbered(&row.section.item);
            let Some((at_path, number)) = found else {
                continue;
            };
            if path.as_ref().is_some_and(|p| p != &at_path) {
                return None;
            }
            path = Some(at_path);
            span = Some(span.map_or((number, number), |(a, b)| (a.min(number), b.max(number))));
        }
        let (from, to) = span?;
        Some((path?, from, to))
    }

    /// The full id of the commit at the cursor: of the branch's commits, a Blame run or an
    /// Origin commit.
    fn current_commit(&self) -> Option<String> {
        match &self.tree().current()?.item {
            Item::Commit { index } => {
                let history = self.repo.commits.as_ref()?;
                Some(history.commits.get(*index)?.oid.clone())
            }
            Item::BlameRun { first, .. } => {
                let (_, lines) = self.repo.blame.as_ref()?;
                lines.get(*first)?.commit.clone()
            }
            Item::Origin(ProvenanceRow::Commit { oid, .. }) => Some(oid.clone()),
            _ => None,
        }
    }

    /// The file `g f` opens, and the line to show: of the line, the file or the thread at the
    /// cursor, or of the Thread screen's thread.
    fn current_file(&self) -> Option<(String, Option<u32>)> {
        if let Some(comment) = self.current_thread().and_then(|id| self.repo.comment(id)) {
            return Some((comment.path.clone(), Some(review::shown_range(comment).0)));
        }
        let item = &self.tree().current()?.item;
        if let Item::Agent(row) = item {
            return row.path().map(|path| (path.to_owned(), None));
        }
        if matches!(item, Item::FileLine { .. } | Item::BlameLine { .. }) {
            let (path, line) = self.edit_target()?;
            return Some((path, Some(line)));
        }
        if let Some((path, ..)) = self
            .repo
            .blame
            .as_ref()
            .filter(|_| matches!(item, Item::BlameRun { .. }))
        {
            return Some((path.clone(), None));
        }
        let (_, path) = item.file()?;
        let line = match item {
            Item::Line { group, path, row } => {
                line_of(&self.repo, *group, path, *row).and_then(|l| l.new_no)
            }
            _ => None,
        };
        Some((path.to_owned(), line))
    }

    /// Closes the screen at `address` (and those above it) when it is on the stack.
    fn close_screens(&mut self, prefix: &str) {
        if let Some(at) = self
            .stack
            .trail()
            .iter()
            .position(|address| address.starts_with(prefix))
        {
            self.stack.back_to(at);
        }
    }

    /// Opens a Diff screen; it replaces the one on the stack, whose reads it would supersede.
    fn open_diff(&mut self, kind: DiffKind) {
        self.close_screens("diff/");
        if let DiffKind::Commit(oid) = &kind
            && self.repo.commit.as_ref().is_none_or(|c| &c.oid != oid)
        {
            self.repo.commit = None;
            self.repo
                .files
                .retain(|(group, _), _| *group != Group::Commit);
            self.commands
                .push(CommandKind::LoadCommitDetail { oid: oid.clone() });
        }
        self.stack.open(Screen::Diff {
            view: View::default(),
            kind,
        });
        self.rebuild();
    }

    /// Opens the File screen of `path`, at `line`; it replaces the one on the stack.
    fn open_file(&mut self, path: String, line: Option<u32>) {
        self.close_screens("file/");
        self.read_file(&path);
        self.stack.open(Screen::File {
            view: View::default(),
            path,
            keep: line,
        });
        self.rebuild();
    }

    /// Opens the Blame screen of `path`, at `line`; it replaces the one on the stack.
    fn open_blame(&mut self, path: String, line: Option<u32>) {
        self.close_screens("blame/");
        if self.repo.blame.as_ref().is_none_or(|(of, _)| of != &path) {
            self.repo.blame = None;
        }
        self.commands
            .push(CommandKind::LoadBlame { path: path.clone() });
        self.stack.open(Screen::Blame {
            view: View::default(),
            path,
            keep: line,
        });
        self.rebuild();
    }

    /// The agent thread an Origin row names: a thread, a link, or an edit's thread.
    fn current_agent_thread(&self) -> Option<String> {
        match &self.tree().current()?.item {
            Item::Origin(
                ProvenanceRow::Thread { uid, .. }
                | ProvenanceRow::Link { uid, .. }
                | ProvenanceRow::Edit { uid, .. },
            ) => Some(uid.clone()),
            _ => None,
        }
    }

    /// Opens the Agent thread screen of `uid` and reads its first page.
    fn open_agent(&mut self, uid: String) {
        let page = Box::new(AgentPage::new(uid));
        self.commands.push(page.first_read());
        self.stack.open(Screen::Agent {
            view: View::default(),
            page,
        });
        self.rebuild();
    }

    /// Opens the Origin screen for lines `from` to `to` of `path`; it replaces the one on the
    /// stack, as the engine supersedes the read.
    fn open_origin(&mut self, path: String, from: u32, to: u32) {
        self.close_screens("origin");
        self.commands.push(CommandKind::ReadProvenance {
            path,
            from_line: from,
            to_line: to,
        });
        self.tree_mut().clear_selection();
        self.stack.open(Screen::Origin {
            view: View::default(),
        });
        self.rebuild();
    }

    /// Reads `path` and its two diffs, for the File screen's lines and marks.
    fn read_file(&mut self, path: &str) {
        self.commands.push(CommandKind::ReadFile {
            path: path.to_owned(),
        });
        self.ask_diff(Group::Unstaged, path);
        self.ask_diff(Group::Staged, path);
    }

    /// The thread actions act on: the Thread screen's, else the one at the cursor.
    fn current_thread(&self) -> Option<i64> {
        match self.stack.top() {
            Screen::Thread { page, .. } => Some(page.id),
            _ => self.tree().current().and_then(|s| s.item.thread()),
        }
    }

    fn on_thread_screen(&self) -> bool {
        matches!(self.stack.top(), Screen::Thread { .. })
    }

    /// Opens the Thread screen of `id` and reads its texts.
    fn open_thread(&mut self, id: i64) {
        let mut page = Box::new(ThreadPage::new(id));
        self.commands.extend(page.reads(&self.repo));
        self.stack.open(Screen::Thread {
            view: View::default(),
            page,
        });
        self.rebuild();
        self.tree_mut().move_to_id("messages");
    }

    /// Back to Status with the thread under its lines.
    fn show_thread(&mut self) {
        let Some(id) = self.current_thread() else {
            return;
        };
        self.stack.back_to(0);
        self.reveal_thread(id);
    }

    fn open_reply(&mut self, now: Instant) {
        let Some(comment) = self.current_thread().and_then(|id| self.repo.comment(id)) else {
            return;
        };
        let (start_line, end_line) = review::shown_range(comment);
        let target = ReviewTarget::Reply {
            id: comment.id,
            path: comment.path.clone(),
            start_line,
            end_line,
        };
        self.overlays.push(Overlay::Text {
            composer: Composer::new(target),
            guard: Guard::new(now, true),
        });
    }

    fn thread_write(&mut self, name: &str, now: Instant) {
        let Some(id) = self.current_thread() else {
            return;
        };
        match name {
            "thread.resolve" => self.commands.push(CommandKind::ResolveReviewComment { id }),
            "thread.reopen" => self.commands.push(CommandKind::ReopenReviewComment { id }),
            _ => self.overlays.push(Overlay::Confirm {
                question: format!("Delete thread {id} and its replies?"),
                yes: Yes::Send(vec![CommandKind::DeleteReviewComment { id }]),
                guard: Guard::new(now, true),
            }),
        }
    }

    /// Puts `@path:lines` of the selection, the line or the file at the cursor on the clipboard.
    fn copy_reference(&mut self) {
        let reference = match self.picked_rows() {
            Some((group, path, from, to)) => {
                let numbers: Vec<u32> = (from..=to)
                    .filter_map(|row| line_of(&self.repo, group, &path, row))
                    .filter_map(|line| line.new_no)
                    .collect();
                match (numbers.iter().min(), numbers.iter().max()) {
                    (Some(a), Some(b)) if a != b => format!("@{path}:{a}-{b}"),
                    (Some(a), _) => format!("@{path}:{a}"),
                    _ => format!("@{path}"),
                }
            }
            None => match self.tree().current().and_then(|s| s.item.file()) {
                Some((_, path)) => format!("@{path}"),
                None => return,
            },
        };
        self.notice = Some(format!("Copied {reference}"));
        self.requests.push(Request::Copy(reference));
    }

    /// Unfolds the file of thread `id` on Status and puts the cursor on the thread.
    fn reveal_thread(&mut self, id: i64) {
        let Some(comment) = self.repo.comment(id) else {
            return;
        };
        let (id, path) = (comment.id, comment.path.clone());
        let Some(group) = [Group::Unstaged, Group::Staged, Group::Branch]
            .into_iter()
            .find(|group| status_files(&self.repo, *group).contains(&path))
        else {
            self.notice = Some(format!("{path} has no changes to show the thread in."));
            return;
        };
        self.tree_mut().unfold(&format!("group/{group:?}"));
        self.tree_mut().unfold(&format!("file/{group:?}/{path}"));
        self.reveal = Some(id);
        self.rebuild();
    }

    /// The next (`forward`) or previous row holding the search, from the cursor.
    fn find(&mut self, forward: bool) {
        let Some(search) = self.search.as_ref().map(|s| s.to_lowercase()) else {
            return;
        };
        let rows = self.tree().rows();
        let texts: Vec<String> = rows
            .iter()
            .map(|row| row.section.item.text(&self.repo).to_lowercase())
            .collect();
        let at = self.tree().cursor_index().unwrap_or(0);
        let count = texts.len();
        let found = (1..=count)
            .map(|step| {
                if forward {
                    (at + step) % count
                } else {
                    (at + count - step % count) % count
                }
            })
            .find(|&index| texts[index].contains(&search));
        match found {
            Some(index) => self.tree_mut().move_to(index),
            None => self.notice = Some(format!("Not found: {search}")),
        }
    }

    // ---- What the drawing asks ----

    /// What the menu's title says it acts on.
    pub fn target_title(&self) -> String {
        if let Some((first, last)) = self.tree().selection() {
            return format!("{} rows selected", last - first + 1);
        }
        match self.tree().current().map(|s| &s.item) {
            Some(Item::Group { group, count }) => format!("{} ({count})", group.title()),
            Some(Item::File { path, .. }) => path.clone(),
            Some(Item::Hunk { path, hunk, .. }) => format!("Hunk {} · {path}", hunk + 1),
            Some(Item::Line { path, .. }) => format!("Line · {path}"),
            Some(Item::Card { id, .. } | Item::Thread { id }) => format!("Thread {id}"),
            _ => "Status".to_owned(),
        }
    }
}

/// The paths of the files `group` lists.
fn status_files(repo: &Repo, group: Group) -> Vec<String> {
    let rows = status::sections(
        repo,
        Opened {
            outdated: false,
            comments: &BTreeSet::new(),
            threads: &BTreeSet::new(),
        },
    );
    rows.into_iter()
        .find(|s| s.id == format!("group/{group:?}"))
        .map_or_default(|s| {
            s.children
                .into_iter()
                .filter_map(|c| match c.item {
                    Item::File { path, .. } => Some(path),
                    _ => None,
                })
                .collect()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use gm_schema::{
        BranchInfo, Change, DiffLine, FileDiff, Hunk, HunkId, ReviewAnchor, ReviewComment,
        ReviewLocation, StatusEntry,
    };
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

    fn app() -> App {
        let mut app = App::new(RepoStatus {
            root: "/repo".to_owned(),
            branch: BranchInfo {
                head: Some("feature".to_owned()),
                oid: Some("a41f9c2aaaa".to_owned()),
                ..BranchInfo::default()
            },
            entries: vec![
                entry("src/lib.rs", None, Some(Change::Modified)),
                entry("README.md", Some(Change::Modified), None),
            ],
        });
        app.take_commands();
        app
    }

    fn diff() -> FileDiff {
        let line = |kind, text: &str, new_no| DiffLine {
            kind,
            old_no: None,
            new_no,
            text: text.to_owned(),
            no_newline: false,
        };
        FileDiff {
            path: "src/lib.rs".to_owned(),
            old_path: None,
            binary: false,
            hunks: vec![Hunk {
                id: HunkId("h1".to_owned()),
                old_start: 3,
                old_lines: 2,
                new_start: 3,
                new_lines: 2,
                header: "fn main() {".to_owned(),
                lines: vec![
                    line(LineKind::Context, "    let a = 1;", Some(3)),
                    line(LineKind::Removed, "    old();", None),
                    line(LineKind::Added, "    new();", Some(4)),
                ],
            }],
        }
    }

    fn key(app: &mut App, keys: &str, now: Instant) {
        for key in crate::nav::keys::parse_sequence(keys).unwrap() {
            let code = match key.code {
                Code::Char(c) => KeyCode::Char(c),
                Code::Enter => KeyCode::Enter,
                Code::Esc => KeyCode::Esc,
                Code::Tab => KeyCode::Tab,
                Code::Down => KeyCode::Down,
                Code::Backspace => KeyCode::Backspace,
                other => panic!("{other:?}"),
            };
            let mut modifiers = KeyModifiers::NONE;
            if key.ctrl {
                modifiers |= KeyModifiers::CONTROL;
            }
            if key.shift {
                modifiers |= KeyModifiers::SHIFT;
            }
            app.on_key_at(KeyEvent::new(code, modifiers), now);
        }
    }

    fn shown(app: &App) -> Vec<String> {
        app.tree()
            .rows()
            .iter()
            .map(|row| format!("{}{:?}", "  ".repeat(row.depth), row.section.item))
            .map(|text| text.chars().take(60).collect())
            .collect()
    }

    /// Colors every line of `job` as a keyword.
    fn all_keyword(job: &ColorJob) -> Vec<gm_schema::HighlightSpan> {
        vec![gm_schema::HighlightSpan {
            start: 0,
            end: job.text.len(),
            role: gm_schema::SyntaxRole::Keyword,
        }]
    }

    /// Colors `job` with the real highlighter, as the coloring thread does.
    fn highlighted(job: &ColorJob) -> Vec<gm_schema::HighlightSpan> {
        let language = gm_core::syntax::language_for_path(&job.path).unwrap();
        let stop = std::sync::atomic::AtomicBool::new(false);
        gm_core::syntax::highlight(language, &job.text, &stop).unwrap()
    }

    /// The foreground of the first character of `needle` on the drawn screen.
    fn drawn_fg(app: &mut App, needle: &str) -> ratatui::style::Color {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| crate::draw::draw(frame, app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let (width, height) = (buffer.area.width, buffer.area.height);
        for y in 0..height {
            for x in 0..width {
                let rest: String = (x..width).map(|x| buffer[(x, y)].symbol()).collect();
                if rest.starts_with(needle) {
                    return buffer[(x, y)].fg;
                }
            }
        }
        panic!("{needle} is not on the screen");
    }

    fn keyword_fg() -> ratatui::style::Color {
        crate::draw::role_style(gm_schema::SyntaxRole::Keyword)
            .fg
            .unwrap_or(ratatui::style::Color::Reset)
    }

    #[test]
    fn every_file_of_a_diff_is_colored_on_both_sides_and_a_late_answer_is_dropped() {
        let mut app = app();
        app.file_loaded(Group::Commit, "src/lib.rs", diff());
        let mut other = diff();
        other.path = "src/main.rs".to_owned();
        app.file_loaded(Group::Commit, "src/main.rs", other);
        let jobs = app.take_color_jobs();
        assert_eq!(jobs.len(), 4, "two files, each with its new and old side");
        for job in &jobs {
            app.colored(job, &all_keyword(job));
        }
        for path in ["src/lib.rs", "src/main.rs"] {
            let file = &app.repo.files[&(Group::Commit, path.to_owned())];
            let mut keys: Vec<_> = file.colors.keys().copied().collect();
            keys.sort_unstable();
            assert_eq!(keys, [(0, 0), (0, 1), (0, 2)], "{path}");
        }

        // The file changed after its jobs were sent: their answers are dropped.
        let mut newer = diff();
        newer.hunks[0].lines[2].text = "    newer();".to_owned();
        app.file_loaded(Group::Commit, "src/lib.rs", newer);
        for job in jobs.iter().filter(|job| job.path == "src/lib.rs") {
            app.colored(job, &all_keyword(job));
        }
        let file = &app.repo.files[&(Group::Commit, "src/lib.rs".to_owned())];
        assert!(!file.colors.contains_key(&(0, 2)));
    }

    /// Unfolds src/lib.rs and gives it its diff.
    fn unfolded(app: &mut App, now: Instant) {
        app.tree_mut().move_to_id("file/Unstaged/src/lib.rs");
        key(app, "<tab>", now);
        let asked = app.take_commands();
        assert_eq!(
            asked,
            [CommandKind::LoadDiff {
                path: "src/lib.rs".to_owned(),
                side: gm_schema::DiffSide::Unstaged
            }]
        );
        app.sent(OperationId(7), &asked[0]);
        app.apply(&Event::DiffLoaded {
            id: OperationId(7),
            side: gm_schema::DiffSide::Unstaged,
            diff: diff(),
        });
        app.take_color_jobs();
    }

    #[test]
    fn status_lists_the_branch_and_the_groups_and_a_file_unfolds_into_its_hunks() {
        let now = Instant::now();
        let mut app = app();
        let rows = shown(&app);
        assert!(rows[0].starts_with("Header { label: \"Head:\""), "{rows:?}");
        assert!(
            rows.iter()
                .any(|r| r.starts_with("Group { group: Unstaged, count: 1")),
            "{rows:?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.starts_with("Group { group: Staged, count: 1")),
            "{rows:?}"
        );
        unfolded(&mut app, now);
        let rows = shown(&app);
        assert!(
            rows.iter().any(|r| r.contains("Hunk { group: Unstaged")),
            "{rows:?}"
        );
        assert_eq!(
            rows.iter().filter(|r| r.contains("Line {")).count(),
            3,
            "{rows:?}"
        );
    }

    #[test]
    fn space_s_stages_the_hunk_and_the_menu_stays_for_the_next() {
        let now = Instant::now();
        let mut app = app();
        unfolded(&mut app, now);
        key(&mut app, "<down>", now); // the hunk
        key(&mut app, "<space>", now);
        assert!(app.input.pending(Mode::Normal).is_some(), "the menu shows");
        assert!(app.applies("stage") && !app.applies("unstage"));
        key(&mut app, "s", now);
        assert_eq!(
            app.take_commands(),
            [CommandKind::Stage {
                target: StageTarget::Hunk {
                    path: "src/lib.rs".to_owned(),
                    hunk: HunkId("h1".to_owned())
                }
            }]
        );
        assert!(
            app.input.pending(Mode::Normal).is_some(),
            "stage repeats: the menu stays"
        );
        key(&mut app, "<esc>", now);
        assert!(app.input.pending(Mode::Normal).is_none());
    }

    #[test]
    fn a_selection_stages_its_changed_lines() {
        let now = Instant::now();
        let mut app = app();
        unfolded(&mut app, now);
        key(&mut app, "<down> <down> V j j", now); // context, removed, added
        assert_eq!(app.mode(), Mode::Visual);
        key(&mut app, "<space> s", now);
        assert_eq!(
            app.take_commands(),
            [CommandKind::Stage {
                target: StageTarget::Lines {
                    path: "src/lib.rs".to_owned(),
                    hunk: HunkId("h1".to_owned()),
                    lines: vec![1, 2]
                }
            }]
        );
        assert_eq!(app.mode(), Mode::Normal);
    }

    #[test]
    fn a_comment_waits_a_second_before_it_can_be_cancelled_or_sent() {
        let now = Instant::now();
        let mut app = app();
        unfolded(&mut app, now);
        key(&mut app, "<down> <down> V j j <space> c", now);
        assert_eq!(app.mode(), Mode::Insert);
        // Within the first second, Esc is dropped.
        key(&mut app, "<esc>", now + Duration::from_millis(300));
        assert!(matches!(app.overlays.last(), Some(Overlay::Text { .. })));
        key(
            &mut app,
            "U s e <space> ? <c-s>",
            now + Duration::from_millis(500),
        );
        assert_eq!(app.take_commands(), [], "sending waits a second too");
        key(&mut app, "<c-s>", now + Duration::from_millis(1100));
        assert_eq!(
            app.take_commands(),
            [CommandKind::AddReviewComment {
                path: "src/lib.rs".to_owned(),
                start_line: 3,
                end_line: 4,
                body: "Use ?".to_owned(),
                label: Some(gm_schema::CommentLabel::Issue),
            }]
        );
    }

    #[test]
    fn delete_asks_and_waits_before_it_takes_a_yes() {
        let now = Instant::now();
        let mut app = app();
        let comment = ReviewComment {
            id: 9,
            path: "src/lib.rs".to_owned(),
            body: "body".to_owned(),
            status: ReviewStatus::Open,
            anchor: ReviewAnchor {
                text: "    new();".to_owned(),
                before: Vec::new(),
                after: Vec::new(),
                start_line: 4,
            },
            created_at: 0,
            resolved_at: None,
            snapshot: None,
            after_snapshot: None,
            location: Some(ReviewLocation {
                start_line: 4,
                end_line: 4,
            }),
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
        };
        app.apply(&Event::ReviewCommentsListed {
            id: OperationId(3),
            comments: vec![comment],
        });
        app.take_commands();
        assert!(app.tree_mut().move_to_id("thread/9"), "{:?}", shown(&app));
        key(&mut app, "<space> D", now);
        assert!(matches!(app.overlays.last(), Some(Overlay::Confirm { .. })));
        key(&mut app, "y", now + Duration::from_millis(400));
        assert_eq!(app.take_commands(), [], "too soon: dropped");
        assert!(matches!(app.overlays.last(), Some(Overlay::Confirm { .. })));
        key(&mut app, "y", now + Duration::from_millis(1000));
        assert_eq!(
            app.take_commands(),
            [CommandKind::DeleteReviewComment { id: 9 }]
        );
        assert!(app.overlays.is_empty());
    }

    #[test]
    fn q_quits_from_status_and_esc_first_clears_a_selection() {
        let now = Instant::now();
        let mut app = app();
        key(&mut app, "V j <esc>", now);
        assert_eq!(app.mode(), Mode::Normal);
        assert!(!app.quit);
        key(&mut app, "q", now);
        assert!(app.quit);
    }

    #[test]
    fn search_finds_a_line_and_n_goes_on() {
        let now = Instant::now();
        let mut app = app();
        unfolded(&mut app, now);
        app.tree_mut().move_to(0);
        key(&mut app, "/ n e w <cr>", now);
        assert!(matches!(
            app.tree().current().map(|s| &s.item),
            Some(Item::Line { row: 3, .. })
        ));
        key(&mut app, "/ R E A D <cr>", now);
        assert!(
            matches!(app.tree().current().map(|s| &s.item), Some(Item::File { path, .. }) if path == "README.md")
        );
    }

    /// A stack main › api › feature, with feature due a restack.
    fn stacks_event() -> Event {
        use gm_schema::{Stack, StackBranch, StackSnapshot, StackState};
        let branch = |name: &str, parent: Option<&str>, depth, state| StackBranch {
            name: name.to_owned(),
            parent: parent.map(str::to_owned),
            depth,
            state,
            ahead: 2,
            behind: 1,
            restack: 0,
            commits: Vec::new(),
            base: None,
            parent_tip: None,
        };
        Event::StacksLoaded {
            id: OperationId(4),
            stacks: StackSnapshot {
                stacks: vec![Stack {
                    branches: vec![
                        branch("main", None, 0, StackState::Root),
                        branch("api", Some("main"), 1, StackState::Current),
                        branch("feature", Some("api"), 2, StackState::Stale),
                    ],
                }],
                looped: Vec::new(),
            },
        }
    }

    #[test]
    fn status_shows_the_stack_the_branchs_commits_and_the_stashes() {
        use gm_schema::{CommitSummary, History, RevisionRange, StashEntry};
        let mut app = App::new(app().repo.status.clone());
        let asked = app.take_commands();
        assert!(asked.contains(&CommandKind::ListStashes), "{asked:?}");
        assert!(asked.contains(&CommandKind::LoadStacks), "{asked:?}");
        app.sent(
            OperationId(1),
            &CommandKind::LoadBranchChange { base: None },
        );
        app.apply(&Event::RevisionRangeLoaded {
            id: OperationId(1),
            range: RevisionRange {
                from_name: "api".to_owned(),
                base: "b0".to_owned(),
                tip: "t9".to_owned(),
                ..RevisionRange::default()
            },
        });
        let asked = app.take_commands();
        assert_eq!(
            asked,
            [CommandKind::LoadHistory {
                scope: HistoryScope::Ref {
                    name: "b0..t9".to_owned()
                }
            }]
        );
        app.sent(OperationId(2), &asked[0]);
        // The read reports Done before its answer: the answer still lands.
        app.apply(&Event::OperationChanged {
            id: OperationId(2),
            state: OperationState::Done,
            detail: None,
            actor: gm_schema::Actor::Human,
        });
        let commit = |short: &str, subject: &str| CommitSummary {
            oid: short.to_owned(),
            parents: Vec::new(),
            short: short.to_owned(),
            subject: subject.to_owned(),
            refs: Vec::new(),
            author_time: 0,
        };
        app.apply(&Event::HistoryLoaded {
            id: OperationId(2),
            history: History {
                commits: vec![commit("t9", "Add the ui"), commit("t8", "Start")],
                unborn: false,
                more: false,
            },
        });
        app.apply(&Event::StashesListed {
            id: OperationId(3),
            stashes: vec![StashEntry {
                index: 0,
                oid: "s0".to_owned(),
                message: "On feature: try".to_owned(),
                branch: Some("feature".to_owned()),
                time: 0,
            }],
        });
        app.apply(&stacks_event());
        let rows = shown(&app);
        let texts: Vec<String> = app
            .tree()
            .rows()
            .iter()
            .map(|row| row.section.item.text(&app.repo))
            .collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("main › api › feature") && t.contains("restack due")),
            "{texts:?}"
        );
        assert!(
            rows.iter().any(|r| r.starts_with("Commits { count: 2")),
            "{rows:?}"
        );
        assert!(texts.contains(&"t9 Add the ui".to_owned()), "{texts:?}");
        assert!(
            rows.iter().any(|r| r.starts_with("Stashes { count: 1")),
            "{rows:?}"
        );
        assert!(
            texts.contains(&"stash@{0} On feature: try".to_owned()),
            "{texts:?}"
        );
        // `/` finds a commit.
        app.tree_mut().move_to(0);
        key(&mut app, "/ S t a r t <cr>", Instant::now());
        assert!(matches!(
            app.tree().current().map(|s| &s.item),
            Some(Item::Commit { index: 1 })
        ));
    }

    #[test]
    fn the_keys_screen_stacks_on_status_scrolls_and_goes_back() {
        let now = Instant::now();
        let mut app = app();
        app.tree_mut().move_to_id("file/Unstaged/src/lib.rs");
        let on_status = app.tree().cursor_index();
        key(&mut app, "?", now);
        assert_eq!(app.stack.trail(), ["status", "keys"]);
        assert!(matches!(
            app.tree().rows().first().map(|r| &r.section.item),
            Some(Item::KeyGroup { title: "Move", .. })
        ));
        // It moves and searches like any screen; Status's actions do not apply on it.
        key(&mut app, "G", now);
        assert_eq!(app.tree().cursor_index(), Some(app.tree().rows().len() - 1));
        key(&mut app, "g g / u n s t a g e <space> w h a t <cr>", now);
        assert!(matches!(
            app.tree().current().map(|s| &s.item),
            Some(Item::Binding { description, .. }) if description.starts_with("Unstage what")
        ));
        assert!(!app.applies("stage") && !app.applies("go.threads"));
        // An answer from the engine still lands on Status underneath.
        app.apply(&Event::ChangeStatsLoaded {
            id: OperationId(5),
            stats: gm_schema::ChangeStats::default(),
        });
        assert_eq!(app.stack.trail(), ["status", "keys"]);
        let groups: Vec<&str> = app
            .tree()
            .rows()
            .iter()
            .filter_map(|r| match r.section.item {
                Item::KeyGroup { title, .. } => Some(title),
                _ => None,
            })
            .collect();
        let mut once = groups.clone();
        once.sort_unstable();
        once.dedup();
        assert_eq!(groups.len(), once.len(), "each group once: {groups:?}");
        key(&mut app, "q", now);
        assert_eq!(app.stack.trail(), ["status"]);
        assert!(!app.quit);
        assert_eq!(
            app.tree().cursor_index(),
            on_status,
            "Status as it was left"
        );
        key(&mut app, "? <esc>", now);
        assert_eq!(app.stack.trail(), ["status"]);
    }

    fn thread(id: i64, status: ReviewStatus, created_at: i64) -> gm_schema::ReviewComment {
        gm_schema::ReviewComment {
            id,
            path: "src/lib.rs".to_owned(),
            body: format!("body {id}"),
            status,
            anchor: gm_schema::ReviewAnchor {
                text: "    new();".to_owned(),
                before: Vec::new(),
                after: Vec::new(),
                start_line: 4,
            },
            created_at,
            resolved_at: None,
            snapshot: Some("snap".to_owned()),
            after_snapshot: None,
            location: Some(gm_schema::ReviewLocation {
                start_line: 4,
                end_line: 4,
            }),
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

    fn texts(app: &App) -> Vec<String> {
        app.tree()
            .rows()
            .iter()
            .map(|row| row.section.item.text(&app.repo))
            .collect()
    }

    #[test]
    fn threads_lists_by_who_acts_next_and_filters_from_the_menu() {
        let now = Instant::now();
        let mut app = app();
        app.apply(&Event::ReviewCommentsListed {
            id: OperationId(3),
            comments: vec![
                thread(1, ReviewStatus::Open, 10),
                thread(2, ReviewStatus::Resolved, 20),
            ],
        });
        app.take_commands();
        key(&mut app, "<space> t", now);
        assert_eq!(app.stack.trail(), ["status", "threads"]);
        let rows = shown(&app);
        assert!(
            rows.iter().any(|r| r.contains("Thread { id: 1 }")),
            "{rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains("Thread { id: 2 }")),
            "{rows:?}"
        );
        // The filter is in the menu, on this screen only.
        key(&mut app, "<space> f r", now);
        let rows = shown(&app);
        assert!(
            rows.iter().any(|r| r.contains("Thread { id: 2 }")),
            "{rows:?}"
        );
        assert!(
            texts(&app)[0].starts_with("Filter: Resolved · 1 of 2"),
            "{:?}",
            texts(&app)
        );
        key(&mut app, "q", now);
        assert!(!app.applies("filter.all"), "no filter on Status");
    }

    #[test]
    fn a_thread_opens_on_its_screen_reads_its_texts_and_marks_the_change() {
        let now = Instant::now();
        let mut app = app();
        let mut handled = thread(1, ReviewStatus::Handled, 10);
        handled.after_snapshot = Some("after".to_owned());
        app.apply(&Event::ReviewCommentsListed {
            id: OperationId(3),
            comments: vec![handled],
        });
        app.take_commands();
        assert!(app.tree_mut().move_to_id("thread/1"));
        key(&mut app, "<cr>", now);
        assert_eq!(app.stack.trail(), ["status", "thread/1"]);
        let asked = app.take_commands();
        assert_eq!(asked, [CommandKind::LoadReviewChange { id: 1 }]);
        app.sent(OperationId(8), &asked[0]);
        app.apply(&Event::ReviewChange {
            id: OperationId(8),
            comment: 1,
            path: "src/lib.rs".to_owned(),
            change: Some(gm_schema::ReviewChange {
                before: vec!["    old();".to_owned()],
                after: vec!["    new();".to_owned(), "    more();".to_owned()],
                changed: true,
                before_start_line: 4,
                after_start_line: 4,
            }),
        });
        let marked: Vec<(u32, bool)> = app
            .tree()
            .rows()
            .iter()
            .filter_map(|r| match &r.section.item {
                Item::Text {
                    version: gm_view::local_review::Version::After,
                    number,
                    marked,
                    ..
                } => Some((*number, *marked)),
                _ => None,
            })
            .collect();
        assert_eq!(marked, [(4, true), (5, true)]);
        let all = texts(&app);
        assert!(all.iter().any(|t| t.contains("changed +2 -1")), "{all:?}");
        // Thread actions act on the screen's thread wherever the cursor is.
        key(&mut app, "g g <space> R", now);
        assert_eq!(
            app.take_commands(),
            [CommandKind::ResolveReviewComment { id: 1 }]
        );
        // Show in diff goes back to Status.
        key(&mut app, "<space> g", now);
        assert_eq!(app.stack.trail(), ["status"]);
    }

    #[test]
    fn the_branch_and_a_commit_open_on_the_diff_screen() {
        let now = Instant::now();
        let mut app = app();
        app.sent(
            OperationId(1),
            &CommandKind::LoadBranchChange { base: None },
        );
        let file = gm_schema::CommitFile {
            path: "src/lib.rs".to_owned(),
            old_path: None,
            change: gm_schema::Change::Modified,
            added: 1,
            deleted: 1,
            binary: false,
        };
        app.apply(&Event::RevisionRangeLoaded {
            id: OperationId(1),
            range: gm_schema::RevisionRange {
                from_name: "main".to_owned(),
                to_name: "feature".to_owned(),
                base: "b0".to_owned(),
                tip: "t9".to_owned(),
                files: vec![file.clone()],
                ..gm_schema::RevisionRange::default()
            },
        });
        app.take_commands();
        key(&mut app, "<space> d b", now);
        assert_eq!(app.stack.trail(), ["status", "diff/branch"]);
        let asked = app.take_commands();
        assert!(
            matches!(asked.as_slice(), [CommandKind::LoadRevisionStack { files, .. }] if files.len() == 1),
            "{asked:?}"
        );
        app.sent(OperationId(2), &asked[0]);
        app.apply(&Event::RevisionStackLoaded {
            id: OperationId(2),
            diffs: vec![diff()],
        });
        let rows = shown(&app);
        assert!(rows[0].contains("feature against main"), "{rows:?}");
        assert!(
            rows.iter().any(|r| r.contains("Hunk { group: Branch")),
            "{rows:?}"
        );
        assert!(!app.applies("stage"), "read-only");
        // A commit replaces the branch's diff on the stack.
        let commit = gm_schema::CommitSummary {
            oid: "c0ffee1234".to_owned(),
            parents: Vec::new(),
            short: "c0ffee1".to_owned(),
            subject: "Tidy".to_owned(),
            refs: Vec::new(),
            author_time: 0,
        };
        app.sent(
            OperationId(3),
            &CommandKind::LoadHistory {
                scope: gm_schema::HistoryScope::CurrentBranch,
            },
        );
        app.apply(&Event::HistoryLoaded {
            id: OperationId(3),
            history: gm_schema::History {
                commits: vec![commit],
                unborn: false,
                more: false,
            },
        });
        key(&mut app, "q", now);
        assert!(app.tree_mut().move_to_id("commit/0"));
        key(&mut app, "<cr>", now);
        assert_eq!(app.stack.trail(), ["status", "diff/c0ffee1"]);
        let asked = app.take_commands();
        assert_eq!(
            asked,
            [CommandKind::LoadCommitDetail {
                oid: "c0ffee1234".to_owned()
            }]
        );
        app.sent(OperationId(4), &asked[0]);
        app.apply(&Event::CommitDetailLoaded {
            id: OperationId(4),
            detail: gm_schema::CommitDetail {
                oid: "c0ffee1234".to_owned(),
                short: "c0ffee1".to_owned(),
                subject: "Tidy".to_owned(),
                files: vec![file],
                ..gm_schema::CommitDetail::default()
            },
        });
        let asked = app.take_commands();
        assert!(
            matches!(asked.as_slice(), [CommandKind::LoadCommitStack { oid, .. }] if oid == "c0ffee1234"),
            "{asked:?}"
        );
        assert!(texts(&app)[0].contains("c0ffee1 Tidy"), "{:?}", texts(&app));
    }

    #[test]
    fn g_f_opens_the_file_at_the_line_and_enter_opens_the_editor_there() {
        let now = Instant::now();
        let mut app = app();
        unfolded(&mut app, now);
        key(&mut app, "<down> <down> <down> <down>", now); // the added line 4
        key(&mut app, "g f", now);
        assert_eq!(app.stack.trail(), ["status", "file/src/lib.rs"]);
        let asked = app.take_commands();
        assert!(
            asked.contains(&CommandKind::ReadFile {
                path: "src/lib.rs".to_owned()
            }),
            "{asked:?}"
        );
        let read = asked
            .iter()
            .position(|k| matches!(k, CommandKind::ReadFile { .. }))
            .unwrap();
        app.sent(OperationId(9), &asked[read]);
        let text = (1..=30)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.apply(&Event::FileRead {
            id: OperationId(9),
            file: gm_schema::FileView {
                path: "src/lib.rs".to_owned(),
                size: 0,
                body: gm_schema::FileBody::Text { text },
                truncated: false,
            },
        });
        // The cursor is on line 4; the unstaged diff marks it; the far lines are folded.
        assert!(matches!(
            app.tree().current().map(|s| &s.item),
            Some(Item::FileLine {
                index: 3,
                mark: crate::status::Mark::Unstaged
            })
        ));
        assert!(
            shown(&app)
                .iter()
                .any(|r| r.starts_with("Unchanged { count: 23")),
            "{:?}",
            shown(&app)
        );
        key(&mut app, "<cr>", now);
        assert_eq!(
            app.take_requests(),
            [Request::Edit {
                path: "/repo/src/lib.rs".to_owned(),
                line: 4
            }]
        );
    }

    fn blame_entry(line: u32, commit: Option<&str>, text: &str) -> gm_schema::BlameEntry {
        gm_schema::BlameEntry {
            line,
            commit: commit.map(str::to_owned),
            author: if commit.is_some() {
                "Ada".to_owned()
            } else {
                String::new()
            },
            author_at: 0,
            subject: if commit.is_some() {
                "Start".to_owned()
            } else {
                String::new()
            },
            text: text.to_owned(),
        }
    }

    #[test]
    fn g_b_blames_the_file_at_the_line_and_its_runs_open_their_commit() {
        let now = Instant::now();
        let mut app = app();
        unfolded(&mut app, now);
        key(&mut app, "<down> <down> <down> <down>", now); // the added line 4
        key(&mut app, "g b", now);
        assert_eq!(app.stack.trail(), ["status", "blame/src/lib.rs"]);
        let asked = app.take_commands();
        assert_eq!(
            asked,
            [CommandKind::LoadBlame {
                path: "src/lib.rs".to_owned()
            }]
        );
        app.sent(OperationId(5), &asked[0]);
        app.apply(&Event::BlameLoaded {
            id: OperationId(5),
            path: "src/lib.rs".to_owned(),
            lines: vec![
                blame_entry(1, Some("c0ffee1234"), "fn main() {"),
                blame_entry(2, Some("c0ffee1234"), "    let a = 1;"),
                blame_entry(3, Some("c0ffee1234"), "    let b = 2;"),
                blame_entry(4, None, "    new();"),
                blame_entry(5, Some("c0ffee1234"), "}"),
            ],
        });
        let runs: Vec<(usize, usize)> = app
            .tree()
            .rows()
            .iter()
            .filter_map(|r| match r.section.item {
                Item::BlameRun { first, last } => Some((first, last)),
                _ => None,
            })
            .collect();
        assert_eq!(runs, [(0, 2), (3, 3), (4, 4)]);
        // The blamed lines are colored on their own, with no File screen read (D251 (4)).
        let jobs = app.take_color_jobs();
        let [job] = jobs.as_slice() else {
            panic!("one blame job: {jobs:?}");
        };
        assert_eq!(job.target, ColorTarget::Blame);
        app.colored(job, &highlighted(job));
        assert_eq!(drawn_fg(&mut app, "fn main"), keyword_fg());
        assert!(matches!(
            app.tree().current().map(|s| &s.item),
            Some(Item::BlameLine { index: 3 })
        ));
        key(&mut app, "<cr>", now);
        assert_eq!(
            app.take_requests(),
            [Request::Edit {
                path: "/repo/src/lib.rs".to_owned(),
                line: 4
            }]
        );
        // `Space w` on that line asks why it is there.
        key(&mut app, "<space> w", now);
        assert_eq!(app.stack.trail(), ["status", "blame/src/lib.rs", "origin"]);
        assert_eq!(
            app.take_commands(),
            [CommandKind::ReadProvenance {
                path: "src/lib.rs".to_owned(),
                from_line: 4,
                to_line: 4
            }]
        );
        key(&mut app, "q", now);
        assert!(app.tree_mut().move_to_id("run/1"));
        key(&mut app, "<cr>", now);
        assert_eq!(
            app.stack.trail(),
            ["status", "blame/src/lib.rs", "diff/c0ffee1"]
        );
    }

    #[test]
    fn origin_nests_the_threads_under_their_commit_as_the_app_words_them() {
        use gm_schema::threads::{Evidence, ProvenanceCommit, ProvenanceResult, ProvenanceThread};
        let now = Instant::now();
        let mut app = app();
        unfolded(&mut app, now);
        key(&mut app, "<down> <down> V j j <space> w", now);
        let asked = app.take_commands();
        assert_eq!(
            asked,
            [CommandKind::ReadProvenance {
                path: "src/lib.rs".to_owned(),
                from_line: 3,
                to_line: 4
            }]
        );
        app.sent(OperationId(6), &asked[0]);
        // The read goes through its states before its answer; none of them ends it.
        for state in [OperationState::Executing, OperationState::Done] {
            app.apply(&Event::OperationChanged {
                id: OperationId(6),
                actor: gm_schema::Actor::Human,
                state,
                detail: None,
            });
        }
        app.apply(&Event::ThreadProvenance {
            id: OperationId(6),
            provenance: ProvenanceResult {
                path: "src/lib.rs".to_owned(),
                from_line: 3,
                to_line: 4,
                commits: vec![ProvenanceCommit {
                    id: "c0ffee1234".to_owned(),
                    subject: "Start".to_owned(),
                    author: "Ada".to_owned(),
                    author_at: 0,
                    path: "src/lib.rs".to_owned(),
                    lines: vec![gm_schema::LineRun { from: 3, to: 4 }],
                    threads: vec![ProvenanceThread {
                        uid: "t1".to_owned(),
                        title: "Add the parser".to_owned(),
                        harness: "claude-code".to_owned(),
                        branch: None,
                        evidence: vec![Evidence::LineMatch],
                        ambiguous: false,
                        edits: Vec::new(),
                        labels: Vec::new(),
                        links: Vec::new(),
                    }],
                }],
                cuts: Vec::new(),
            },
        });
        let rows = shown(&app);
        assert!(rows[0].contains("Origin:"), "{rows:?}");
        assert!(
            rows.iter().any(|r| r.starts_with("Origin(Commit")),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.starts_with("  Origin(Thread")),
            "{rows:?}"
        );
        assert!(app.tree_mut().move_to_id("row/0"));
        key(&mut app, "<cr>", now);
        assert_eq!(app.stack.trail(), ["status", "origin", "diff/c0ffee1"]);
        // `Enter` on the thread opens its session, read from its first page.
        key(&mut app, "q", now);
        assert!(app.tree_mut().move_to_id("row/1"));
        key(&mut app, "<cr>", now);
        assert_eq!(app.stack.trail(), ["status", "origin", "agent/t1"]);
        let asked = app.take_commands();
        assert!(
            matches!(asked.as_slice(), [CommandKind::LoadCommitDetail { .. }, CommandKind::ReadThread { uid, from_seq: 0, .. }] if uid == "t1"),
            "{asked:?}"
        );
        app.sent(OperationId(7), &asked[1]);
        app.apply(&Event::ThreadRead {
            id: OperationId(7),
            detail: Box::new(crate::agent_screen::tests::detail()),
        });
        assert!(
            texts(&app).iter().any(|t| t == "Request"),
            "{:?}",
            texts(&app)
        );
    }
}
