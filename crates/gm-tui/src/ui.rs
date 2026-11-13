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
use gm_view::review::{self, Opened};
use ratatui::crossterm::event::{KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::data::{ColorJob, Group, Repo, color_job};
use crate::nav::Mode;
use crate::nav::action::{Behavior, Registry};
use crate::nav::defaults;
use crate::nav::input::{Input, Outcome};
use crate::nav::keys::{Code, Key};
use crate::nav::prompt::{Answer, Guard};
use crate::nav::sections::Tree;
use crate::nav::stack::{Addressed, Stack};
use crate::status::{self, Item, line_of};
use crate::threads::Composer;

/// Smaller than this, only a message shows (D235 (9)).
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
/// Rows a turn of the mouse wheel scrolls.
const WHEEL_ROWS: isize = 3;

/// A screen of the stack. Status is the root; the other screens come with M3 to M6.
#[derive(Debug, Default)]
pub struct StatusScreen {
    pub tree: Tree<Item>,
    pub scroll: usize,
}

#[derive(Debug)]
pub enum Screen {
    Status(StatusScreen),
}

impl Addressed for Screen {
    fn address(&self) -> String {
        match self {
            Screen::Status(_) => "status".to_owned(),
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
    /// Every key of the screen (`?`).
    Keys,
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
    fn guard(&self) -> Option<&Guard> {
        match self {
            Overlay::Keys => None,
            Overlay::Text { guard, .. }
            | Overlay::Confirm { guard, .. }
            | Overlay::Search { guard, .. } => Some(guard),
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
}

/// What a read or a write in flight is for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    Diff(Group, String),
    Range,
    Stack,
    /// The branch's own commits.
    Commits,
    Write(&'static str),
    Text,
}

pub struct App {
    pub repo: Repo,
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
            stack: Stack::new(Screen::Status(StatusScreen::default())),
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

    pub fn set_watch_notice(&mut self, notice: Option<String>) {
        self.watch_notice = notice;
        self.dirty = true;
    }

    /// Whether a prompt on screen still waits before it accepts its keys.
    pub fn guard_closed(&self, now: Instant) -> bool {
        self.overlays
            .last()
            .and_then(Overlay::guard)
            .is_some_and(|guard| now < guard.opens_at())
    }

    pub fn redraw(&mut self) {
        self.dirty = true;
    }

    pub fn status(&self) -> &StatusScreen {
        match self.stack.top() {
            Screen::Status(screen) => screen,
        }
    }

    pub fn status_mut(&mut self) -> &mut StatusScreen {
        match self.stack.top_mut() {
            Screen::Status(screen) => screen,
        }
    }

    fn tree(&self) -> &Tree<Item> {
        &self.status().tree
    }

    fn tree_mut(&mut self) -> &mut Tree<Item> {
        &mut self.status_mut().tree
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

    pub fn colored(&mut self, job: &ColorJob, spans: &[gm_schema::HighlightSpan]) {
        if let Some(file) = self.repo.files.get_mut(&(job.group, job.path.clone())) {
            file.colors = crate::data::split_colors(job, spans);
            self.dirty = true;
        }
    }

    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::StatusLoaded { status, .. } => {
                self.repo.status.clone_from(status);
                self.ask_for_reads();
                // Unfolded files may have changed: read them again; their old diff shows meanwhile.
                for (group, path) in status::unfolded_files(self.tree().toggled()) {
                    if group != Group::Branch {
                        self.ask_diff(group, &path);
                    }
                }
            }
            Event::ChangeStatsLoaded { stats, .. } => self.repo.stats.clone_from(stats),
            Event::ReviewCommentsListed { comments, .. } => self.repo.comments.clone_from(comments),
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
            _ => return,
        }
        self.rebuild();
        self.dirty = true;
    }

    fn file_loaded(&mut self, group: Group, path: &str, diff: gm_schema::FileDiff) {
        if let Some(job) = color_job(group, &diff) {
            self.color_jobs.push(job);
        }
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
        // A read on a job of its own reports Done before its answer: keep it until then.
        if !failed
            && matches!(
                self.in_flight.get(&id),
                Some(Pending::Diff(..) | Pending::Range | Pending::Stack | Pending::Commits)
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
        self.tree_mut().set(sections);
        let missing: Vec<(Group, String)> = status::unfolded_files(self.tree().toggled())
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
            self.tree_mut().set(sections);
        }
        if let Some(id) = self.reveal {
            let card = self
                .tree()
                .rows()
                .iter()
                .find(|row| matches!(row.section.item, Item::Card { id: card, .. } if card == id))
                .map(|row| row.section.id.clone());
            if let Some(card) = card {
                self.tree_mut().move_to_id(&card);
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
        let guard = self.overlays.last().and_then(Overlay::guard).copied();
        let allowed = |answer| guard.is_none_or(|guard: Guard| guard.accepts(answer, now));
        match self.overlays.last_mut() {
            Some(Overlay::Keys) => {
                self.overlays.pop();
            }
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
                        .and_then(Overlay::guard)
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
        let screen = self.status_mut();
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
        let current = self.tree().current().map(|s| &s.item);
        let group = current
            .and_then(Item::file)
            .map(|(group, _)| group)
            .or(match current {
                Some(Item::Group { group, .. }) => Some(*group),
                _ => None,
            });
        let thread = current
            .and_then(Item::thread)
            .and_then(|id| self.repo.comment(id));
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
            "thread.delete" => thread.is_some(),
            "copy.reference" => current.and_then(Item::file).is_some(),
            "go.branch" => self.repo.range.is_some(),
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
            "fold.toggle" | "open"
                if !matches!(
                    self.tree().current().map(|s| &s.item),
                    Some(Item::Thread { .. })
                ) =>
            {
                self.tree_mut().toggle();
                self.rebuild();
            }
            "open" => self.reveal_thread(),
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
            "go.threads" => {
                self.tree_mut().unfold("threads");
                self.rebuild();
                self.tree_mut().move_to_id("threads");
            }
            "go.branch" => {
                self.tree_mut().unfold("group/Branch");
                self.rebuild();
                self.tree_mut().move_to_id("group/Branch");
            }
            "help.keys" => self.overlays.push(Overlay::Keys),
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

    fn current_thread(&self) -> Option<i64> {
        self.tree().current().and_then(|s| s.item.thread())
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

    /// `Enter` on a thread in the list: unfold its file and put the cursor on it.
    fn reveal_thread(&mut self) {
        let Some(comment) = self.current_thread().and_then(|id| self.repo.comment(id)) else {
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
}
