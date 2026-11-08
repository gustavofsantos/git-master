//! The command palette's model (slice 1c, item 4): what it offers for a query,
//! how the offers are ranked and where the highlight sits. The offers come from
//! the action registry and the branch listing. A choice names an `Action` and
//! its `Args`, and the window runs it through `run_action`, like a key or a menu
//! item. Nothing here touches GPUI. Ranking is the one step the window moves to
//! the background executor, through `Job` and `Ranked`.

use std::ops::Range;
use std::sync::Arc;

use gm_schema::{BranchEntry, WorkspaceRow, WorktreeEntry};

use crate::action_registry::{
    Action, ArgKind, Args, Category, Context as ActionContext, NOT_AVAILABLE, OPEN_STASHES, Reason,
    label_args, memory_args, stash_args, thread_args,
};
use crate::fuzzy;
use crate::stashes;
use crate::worktree_list;

const NO_BRANCH: Reason = "No branch to choose from.";
const NO_FILE: Reason = "No file in the working copy.";
const NEEDS_CHUNK: Reason = "Pick a chunk in the diff first.";
const NEEDS_COMMENT: Reason = "Pick the comment on its card in the diff.";
const NEEDS_COMMIT: Reason = "Pick a commit in the history first.";
const NEEDS_REFLOG_COMMIT: Reason = "Pick a commit on the Reflog page first.";
const NEEDS_LOCAL_REVIEW_COMMENT: Reason = "Pick a comment on the Local review page first.";
const NO_WORKTREE: Reason = "No worktree to choose from.";
const NO_WORKSPACE: Reason = "No workspace to choose from.";
const NO_WORKFLOW: Reason = "No workflow to choose from.";
const NOT_LISTED: Reason = "Workflows are still being read.";
const NO_TEMPLATE: Reason = "No templates listed yet";
const NO_STASH: Reason = "No stash to choose from.";

/// The groups the palette shows, in the order they appear.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Section {
    Actions,
    Worktrees,
    Workspaces,
    Branches,
    Stashes,
    Workflows,
    Files,
}

impl Section {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Actions => "Actions",
            Self::Worktrees => "Worktrees",
            Self::Workspaces => "Workspaces",
            Self::Branches => "Branches",
            Self::Stashes => "Stashes",
            Self::Workflows => "Workflows",
            Self::Files => "Files",
        }
    }
}

/// What Enter does on a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Runs `action` on `args`, as a key or a menu item would.
    Run { action: Action, args: Args },
    /// Opens the argument step of an action that takes a branch or a file.
    Pick(Action),
}

/// One row the palette could show: its group, its text, what Enter does and
/// whether that runs now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) section: Section,
    pub(crate) label: String,
    pub(crate) shortcut: Option<String>,
    pub(crate) choice: Choice,
    /// `Ok` when the choice runs now (or, for `Pick`, can be opened); otherwise
    /// the reason, shown beside the row.
    pub(crate) state: Result<(), Reason>,
}

/// Where the palette is: its top level, or the argument step of one action.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Stage {
    #[default]
    Actions,
    Pick(Action),
}

/// A candidate that matched the query, with the parts of its label to highlight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hit {
    /// Index into the candidates the ranking was made from.
    pub(crate) index: usize,
    pub(crate) ranges: Vec<Range<usize>>,
}

/// One line of the list: a section's header, a row by its position in the hits,
/// or the line that stands for a section's hidden disabled rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    Header(Section),
    Row {
        hit: usize,
        highlighted: bool,
    },
    /// `count` disabled rows of `section` are hidden; Enter lists them.
    MoreDisabled {
        section: Section,
        count: usize,
        highlighted: bool,
    },
}

/// A ranking to run off the UI thread.
pub(crate) struct Job {
    generation: u64,
    query: String,
    candidates: Arc<Vec<Candidate>>,
}

impl Job {
    /// Runs the match. Pure, so it can run on any thread.
    #[must_use]
    pub(crate) fn run(self) -> Ranked {
        let hits = rank(&self.query, &self.candidates);
        Ranked {
            generation: self.generation,
            candidates: self.candidates,
            hits,
        }
    }
}

/// The result of a `Job`, with the candidates its hits index into.
pub(crate) struct Ranked {
    generation: u64,
    candidates: Arc<Vec<Candidate>>,
    hits: Vec<Hit>,
}

/// The palette's state. The window owns one; it is changed on the UI thread.
#[derive(Debug, Default)]
pub(crate) struct Palette {
    pub(crate) open: bool,
    stage: Stage,
    query: String,
    /// Counts the jobs; a ranking from any other job is stale.
    generation: u64,
    /// The candidates and hits on screen. A hit indexes into `shown`.
    shown: Arc<Vec<Candidate>>,
    hits: Vec<Hit>,
    /// Position of the line Enter would choose, among the choosable lines: the
    /// rows on screen and the `MoreDisabled` lines (see `choosable`).
    highlight: usize,
    /// Whether the disabled rows are listed. Until then each section's disabled
    /// rows stand behind one `MoreDisabled` line.
    show_disabled: bool,
    /// The actions a scoped palette offers, in order; `None` for the whole palette.
    scope: Option<Vec<(Action, Args)>>,
    /// The title a scoped palette shows as its prompt.
    scope_title: Option<String>,
}

impl Palette {
    /// Opens the top level with an empty query, the whole palette and no scope.
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.stage = Stage::Actions;
        self.query.clear();
        self.highlight = 0;
        self.show_disabled = false;
        self.scope = None;
        self.scope_title = None;
    }

    /// Opens the top level over `items` only, titled `title`.
    pub(crate) fn open_scoped(&mut self, title: String, items: Vec<(Action, Args)>) {
        self.open();
        self.scope = Some(items);
        self.scope_title = Some(title);
    }

    /// The actions of a scoped palette, if one is open.
    pub(crate) fn scope(&self) -> Option<&[(Action, Args)]> {
        self.scope.as_deref()
    }

    /// The title of a scoped palette, if one is open.
    pub(crate) fn scope_title(&self) -> Option<&str> {
        self.scope_title.as_deref()
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    pub(crate) fn stage(&self) -> Stage {
        self.stage
    }

    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    /// The query is the first line of `text`; the highlight goes back to the top.
    pub(crate) fn set_query(&mut self, text: &str) {
        let first = text.lines().next().unwrap_or("");
        if first != self.query {
            first.clone_into(&mut self.query);
            self.highlight = 0;
            // A new query collapses the disabled rows again.
            self.show_disabled = false;
        }
    }

    /// Opens the argument step of `action`. The query starts over: it was typed
    /// for the action, not for its argument.
    pub(crate) fn enter_pick(&mut self, action: Action) {
        self.stage = Stage::Pick(action);
        self.query.clear();
        self.highlight = 0;
        self.show_disabled = false;
    }

    /// Goes back from an argument step to the top level. Returns whether there
    /// was a step to leave.
    pub(crate) fn leave_pick(&mut self) -> bool {
        if matches!(self.stage, Stage::Pick(_)) {
            self.stage = Stage::Actions;
            self.query.clear();
            self.highlight = 0;
            self.show_disabled = false;
            true
        } else {
            false
        }
    }

    /// Starts a ranking of `candidates` for the current query. Any ranking
    /// started before it is stale from now on.
    pub(crate) fn begin(&mut self, candidates: Vec<Candidate>) -> Job {
        self.generation += 1;
        Job {
            generation: self.generation,
            query: self.query.clone(),
            candidates: Arc::new(candidates),
        }
    }

    /// Shows a ranking. Returns false, and changes nothing, when it is stale.
    pub(crate) fn apply(&mut self, ranked: Ranked) -> bool {
        if ranked.generation != self.generation {
            return false;
        }
        self.shown = ranked.candidates;
        self.hits = ranked.hits;
        self.highlight = self.highlight.min(self.choosable().len().saturating_sub(1));
        true
    }

    /// The number of matching results, disabled ones included, whether listed or not.
    pub(crate) fn count(&self) -> usize {
        self.hits.len()
    }

    /// Up and down: moves the highlight over the choosable lines and stops at both ends.
    pub(crate) fn move_highlight(&mut self, down: bool) {
        let count = self.choosable().len();
        if count == 0 {
            self.highlight = 0;
            return;
        }
        let current = self.highlight.min(count - 1);
        self.highlight = if down {
            (current + 1).min(count - 1)
        } else {
            current.saturating_sub(1)
        };
    }

    /// Makes the choosable line at `position` the highlighted one.
    pub(crate) fn select(&mut self, position: usize) {
        if position < self.choosable().len() {
            self.highlight = position;
        }
    }

    /// The candidate Enter would act on. `None` when the highlight is on a
    /// `MoreDisabled` line, or on nothing.
    pub(crate) fn highlighted(&self) -> Option<&Candidate> {
        match self.choosable().get(self.highlight)? {
            Line::Row { hit, .. } => self.row(*hit).map(|(candidate, _)| candidate),
            Line::Header(_) | Line::MoreDisabled { .. } => None,
        }
    }

    /// Whether Tab would open an argument step for the highlighted row.
    pub(crate) fn highlight_picks_argument(&self) -> bool {
        tab_picks_argument(self.highlighted().map(|candidate| &candidate.choice))
    }

    /// Whether the highlight is on a `MoreDisabled` line, which Enter expands.
    pub(crate) fn highlighted_more(&self) -> bool {
        matches!(
            self.choosable().get(self.highlight),
            Some(Line::MoreDisabled { .. })
        )
    }

    /// Lists the disabled rows that were hidden. The highlight stays on the first
    /// of them when it was on the `MoreDisabled` line that hid them.
    pub(crate) fn expand_disabled(&mut self) {
        let target = match self.choosable().get(self.highlight).copied() {
            Some(Line::Row { hit, .. }) => Some(hit),
            Some(Line::MoreDisabled { section, .. }) => self.first_hidden(section),
            Some(Line::Header(_)) | None => None,
        };
        self.show_disabled = true;
        if let Some(hit) = target {
            let position = self
                .choosable()
                .iter()
                .position(|line| matches!(line, Line::Row { hit: row, .. } if *row == hit));
            if let Some(position) = position {
                self.highlight = position;
            }
        }
    }

    /// The hit position of the first disabled row of `section`.
    fn first_hidden(&self, section: Section) -> Option<usize> {
        self.hits.iter().position(|hit| {
            self.shown
                .get(hit.index)
                .is_some_and(|candidate| candidate.section == section && candidate.state.is_err())
        })
    }

    /// The candidate and the hit of the row at `position` (a position among the hits).
    pub(crate) fn row(&self, position: usize) -> Option<(&Candidate, &Hit)> {
        let hit = self.hits.get(position)?;
        Some((self.shown.get(hit.index)?, hit))
    }

    /// The list as drawn: a header before each section, then its rows, then one
    /// `MoreDisabled` line for the disabled rows that are hidden.
    pub(crate) fn lines(&self) -> Vec<Line> {
        let mut choosable = 0;
        let mut lines = self.plain_lines();
        for line in &mut lines {
            match line {
                Line::Header(_) => {}
                Line::Row { highlighted, .. } | Line::MoreDisabled { highlighted, .. } => {
                    *highlighted = choosable == self.highlight;
                    choosable += 1;
                }
            }
        }
        lines
    }

    /// The lines a person can choose, in list order: the rows on screen and the
    /// `MoreDisabled` lines. The highlight is an index into these.
    fn choosable(&self) -> Vec<Line> {
        self.plain_lines()
            .into_iter()
            .filter(|line| !matches!(line, Line::Header(_)))
            .collect()
    }

    /// The list without its highlight. Hits are grouped by section, and within a
    /// section the enabled rows come first, so the disabled ones are the tail.
    fn plain_lines(&self) -> Vec<Line> {
        let mut lines = Vec::with_capacity(self.hits.len() + 3);
        let rows: Vec<(Section, bool, usize)> = self
            .hits
            .iter()
            .enumerate()
            .filter_map(|(position, hit)| {
                let candidate = self.shown.get(hit.index)?;
                Some((candidate.section, candidate.state.is_err(), position))
            })
            .collect();
        for group in rows.chunk_by(|a, b| a.0 == b.0) {
            let section = group[0].0;
            // A scoped palette's only section repeats its title, so it has no header.
            if self.scope.is_none() {
                lines.push(Line::Header(section));
            }
            let mut hidden = 0;
            for &(_, disabled, position) in group {
                if disabled && !self.show_disabled {
                    hidden += 1;
                } else {
                    lines.push(Line::Row {
                        hit: position,
                        highlighted: false,
                    });
                }
            }
            if hidden > 0 {
                lines.push(Line::MoreDisabled {
                    section,
                    count: hidden,
                    highlighted: false,
                });
            }
        }
        lines
    }

    /// The index in `lines()` of the highlighted line, for scrolling it into view.
    pub(crate) fn highlight_line(&self) -> Option<usize> {
        self.lines().iter().position(|line| match line {
            Line::Row { highlighted, .. } | Line::MoreDisabled { highlighted, .. } => *highlighted,
            Line::Header(_) => false,
        })
    }
}

/// The candidates for the top level or an argument step, built from the window's
/// state. The top level lists every action (worktree ones in their own section),
/// and the branches only once something is typed, so an empty query stays short.
#[must_use]
pub(crate) fn build(
    stage: Stage,
    query: &str,
    ctx: &ActionContext<'_>,
    branches: &[BranchEntry],
) -> Vec<Candidate> {
    match stage {
        Stage::Actions => top_level(query, ctx, branches),
        Stage::Pick(action) => pick(action, ctx, branches),
    }
}

/// The placeholder of the palette's input. A scoped palette only filters the
/// actions of its selection, so it says so; the whole palette searches all.
#[must_use]
pub(crate) fn filter_placeholder(scope_title: Option<&str>) -> &'static str {
    match scope_title {
        Some(_) => "Filter actions",
        None => "Type a command, branch or file",
    }
}

/// The candidates of a scoped palette's top level: exactly `items`, in their order,
/// each a row that runs its action on its args with the registry's state.
#[must_use]
pub(crate) fn build_scoped(items: &[(Action, Args)], ctx: &ActionContext<'_>) -> Vec<Candidate> {
    items
        .iter()
        .map(|(action, args)| Candidate {
            section: Section::Actions,
            label: action.spec().title.to_owned(),
            shortcut: action.shortcut_label(),
            choice: Choice::Run {
                action: *action,
                args: args.clone(),
            },
            state: action.enabled(args, ctx),
        })
        .collect()
}

fn top_level(query: &str, ctx: &ActionContext<'_>, branches: &[BranchEntry]) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = Action::ALL
        .into_iter()
        .filter(|action| action.listed_in_palette() && action.offered(ctx.features))
        .map(|action| action_row(action, ctx, branches))
        .collect();
    if !query.is_empty() {
        candidates.extend(branches.iter().map(|branch| {
            let args = Args::Branch(branch.name.clone());
            let state = Action::CheckoutBranch.enabled(&args, ctx);
            Candidate {
                section: Section::Branches,
                label: branch.name.clone(),
                shortcut: None,
                choice: Choice::Run {
                    action: Action::CheckoutBranch,
                    args,
                },
                state,
            }
        }));
    }
    candidates
}

/// Whether Tab picks an argument for a row: only a `Choice::Pick` row opens an
/// argument step. An action whose argument the palette fills in itself, or that
/// is listed disabled, is a `Choice::Run` row, and Tab does nothing on it.
pub(crate) fn tab_picks_argument(choice: Option<&Choice>) -> bool {
    matches!(choice, Some(Choice::Pick(_)))
}

fn action_row(action: Action, ctx: &ActionContext<'_>, branches: &[BranchEntry]) -> Candidate {
    let spec = action.spec();
    let section = match spec.category {
        Category::Worktrees => Section::Worktrees,
        Category::Workspaces => Section::Workspaces,
        _ => Section::Actions,
    };
    let (choice, state) = match spec.args {
        ArgKind::None | ArgKind::OptionalBranch => (
            Choice::Run {
                action,
                args: Args::None,
            },
            action.enabled(&Args::None, ctx),
        ),
        ArgKind::Branch => {
            let targets: Vec<Args> = branches
                .iter()
                .map(|branch| Args::Branch(branch.name.clone()))
                .collect();
            (
                Choice::Pick(action),
                pick_state(action, &targets, ctx, NO_BRANCH),
            )
        }
        ArgKind::File => {
            let targets: Vec<Args> = ctx
                .entries
                .iter()
                .map(|entry| Args::File(entry.path.clone()))
                .collect();
            (
                Choice::Pick(action),
                pick_state(action, &targets, ctx, NO_FILE),
            )
        }
        ArgKind::Worktree => {
            let targets: Vec<Args> = listed_worktrees(ctx)
                .into_iter()
                .map(|(_, path)| Args::Worktree(path))
                .collect();
            (
                Choice::Pick(action),
                pick_state(action, &targets, ctx, NO_WORKTREE),
            )
        }
        ArgKind::Workspace => workspace_pick(action, ctx),
        ArgKind::Workflow => {
            let targets = workflow_targets(ctx);
            let nothing = if ctx.workflows.listed.is_none() {
                NOT_LISTED
            } else {
                NO_WORKFLOW
            };
            (
                Choice::Pick(action),
                pick_state(action, &targets, ctx, nothing),
            )
        }
        ArgKind::Template => {
            let targets = template_targets(ctx);
            (
                Choice::Pick(action),
                pick_state(action, &targets, ctx, NO_TEMPLATE),
            )
        }
        ArgKind::Stash => (
            Choice::Pick(action),
            pick_state(action, &stash_targets(ctx), ctx, no_stash(ctx)),
        ),
        // A chunk exists only in the diff pane, so the palette lists it disabled.
        ArgKind::Chunk => disabled_run(action, NEEDS_CHUNK),
        // A review comment is named by its card in the diff, so the palette lists it disabled.
        ArgKind::ReviewComment => disabled_run(action, review_reason(action)),
        // A commit is named by the panel it is shown in (the history's or the Reflog's), so
        // the palette lists it disabled. The palette has no picker of commits to offer.
        ArgKind::Commit => disabled_run(action, commit_reason(action)),
        // A thread is the one the Threads page has selected; with none, the action says so.
        ArgKind::Thread => {
            let args = ctx.selected_thread.map_or(Args::None, thread_args);
            let state = action.enabled(&args, ctx);
            (Choice::Run { action, args }, state)
        }
        // A label is the one the Labels page has selected; with none, the action says so.
        ArgKind::Label => {
            let args = ctx.selected_label.map_or(Args::None, label_args);
            let state = action.enabled(&args, ctx);
            (Choice::Run { action, args }, state)
        }
        // A memory is the one the Memories tab has selected; with none, the action says so (D190).
        ArgKind::Memory => {
            let args = ctx.selected_memory.map_or(Args::None, memory_args);
            let state = action.enabled(&args, ctx);
            (Choice::Run { action, args }, state)
        }
    };
    // A placeholder says it is not available, whatever its target would be.
    let state = if action.available() {
        state
    } else {
        Err(NOT_AVAILABLE)
    };
    Candidate {
        section,
        label: spec.title.to_owned(),
        shortcut: action.shortcut_label(),
        choice,
        state,
    }
}

/// Why a commit action is listed disabled in the palette: the Reflog's two
/// actions name an entry of the Reflog page, the copies name one of the history.
/// Why a comment action is not run from the palette: Open Comment in Diff is
/// named by the Local review page, the others by their card in the diff.
fn review_reason(action: Action) -> Reason {
    match action {
        Action::OpenReviewComment => NEEDS_LOCAL_REVIEW_COMMENT,
        _ => NEEDS_COMMENT,
    }
}

fn commit_reason(action: Action) -> Reason {
    match action {
        Action::CheckoutCommit | Action::NewBranchFromCommit => NEEDS_REFLOG_COMMIT,
        _ => NEEDS_COMMIT,
    }
}

/// An action with a target is offered when at least one target can take it. When
/// none can, the first target's reason says why.
fn pick_state(
    action: Action,
    targets: &[Args],
    ctx: &ActionContext<'_>,
    nothing: Reason,
) -> Result<(), Reason> {
    let mut first = None;
    for args in targets {
        match action.enabled(args, ctx) {
            Ok(()) => return Ok(()),
            Err(reason) => {
                first.get_or_insert(reason);
            }
        }
    }
    Err(first.unwrap_or(nothing))
}

/// The listed workflows the palette can pick, in the order the engine gave them.
fn workflow_targets(ctx: &ActionContext<'_>) -> Vec<Args> {
    ctx.workflows
        .listed
        .unwrap_or_default()
        .iter()
        .map(|workflow| Args::Workflow(workflow.name.clone()))
        .collect()
}

/// The built-in templates the palette can pick, in the order the engine gave them.
fn template_targets(ctx: &ActionContext<'_>) -> Vec<Args> {
    ctx.workflows
        .templates
        .iter()
        .map(|template| Args::Template(template.clone()))
        .collect()
}

/// The listed stashes the palette can pick, newest first.
fn stash_targets(ctx: &ActionContext<'_>) -> Vec<Args> {
    ctx.stashes
        .unwrap_or_default()
        .iter()
        .map(stash_args)
        .collect()
}

/// Why no stash can be picked: without a listing the Stashes page must be opened first.
fn no_stash(ctx: &ActionContext<'_>) -> Reason {
    if ctx.stashes.is_none() {
        OPEN_STASHES
    } else {
        NO_STASH
    }
}

/// The worktrees the palette can pick, as the sidebar lists them: the name and
/// the folder of each, main checkout first, bare entries left out.
fn listed_worktrees(ctx: &ActionContext<'_>) -> Vec<(String, String)> {
    let mut entries: Vec<&WorktreeEntry> = ctx.worktrees.iter().filter(|e| !e.bare).collect();
    entries.sort_by_key(|entry| !entry.is_main);
    entries
        .into_iter()
        .map(|entry| (worktree_list::display_name(entry), entry.path.clone()))
        .collect()
}

/// An action that picks one of the listed workspaces (D183).
fn workspace_pick(action: Action, ctx: &ActionContext<'_>) -> (Choice, Result<(), Reason>) {
    let targets: Vec<Args> = listed_workspaces(ctx)
        .into_iter()
        .map(|(_, path)| Args::Workspace(path))
        .collect();
    (
        Choice::Pick(action),
        pick_state(action, &targets, ctx, NO_WORKSPACE),
    )
}

/// The workspaces the palette can pick, by name, as the sidebar lists them (D183).
fn listed_workspaces(ctx: &ActionContext<'_>) -> Vec<(String, String)> {
    let mut rows: Vec<&WorkspaceRow> = ctx.workspaces.iter().collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows.into_iter()
        .map(|row| (row.name.clone(), row.path.clone()))
        .collect()
}

/// An action the palette lists disabled and runs with no target: `reason` says
/// what to do instead.
fn disabled_run(action: Action, reason: Reason) -> (Choice, Result<(), Reason>) {
    (
        Choice::Run {
            action,
            args: Args::None,
        },
        Err(reason),
    )
}

fn pick(action: Action, ctx: &ActionContext<'_>, branches: &[BranchEntry]) -> Vec<Candidate> {
    match action.spec().args {
        ArgKind::Branch => branches
            .iter()
            .map(|branch| {
                let args = Args::Branch(branch.name.clone());
                target_row(action, Section::Branches, branch.name.clone(), args, ctx)
            })
            .collect(),
        ArgKind::File => ctx
            .entries
            .iter()
            .map(|entry| {
                let args = Args::File(entry.path.clone());
                target_row(action, Section::Files, entry.path.clone(), args, ctx)
            })
            .collect(),
        ArgKind::Worktree => listed_worktrees(ctx)
            .into_iter()
            .map(|(name, path)| {
                let args = Args::Worktree(path);
                target_row(action, Section::Worktrees, name, args, ctx)
            })
            .collect(),
        ArgKind::Workspace => listed_workspaces(ctx)
            .into_iter()
            .map(|(name, path)| {
                let args = Args::Workspace(path);
                target_row(action, Section::Workspaces, name, args, ctx)
            })
            .collect(),
        ArgKind::Workflow => ctx
            .workflows
            .listed
            .unwrap_or_default()
            .iter()
            .map(|workflow| {
                let args = Args::Workflow(workflow.name.clone());
                target_row(action, Section::Workflows, workflow.name.clone(), args, ctx)
            })
            .collect(),
        ArgKind::Template => ctx
            .workflows
            .templates
            .iter()
            .map(|template| {
                let args = Args::Template(template.clone());
                target_row(action, Section::Workflows, template.clone(), args, ctx)
            })
            .collect(),
        ArgKind::Stash => ctx
            .stashes
            .unwrap_or_default()
            .iter()
            .map(|entry| {
                let label = stashes::choice_label(entry);
                target_row(action, Section::Stashes, label, stash_args(entry), ctx)
            })
            .collect(),
        ArgKind::None
        | ArgKind::OptionalBranch
        | ArgKind::Chunk
        | ArgKind::Commit
        | ArgKind::Thread
        | ArgKind::Label
        | ArgKind::Memory
        | ArgKind::ReviewComment => Vec::new(),
    }
}

fn target_row(
    action: Action,
    section: Section,
    label: String,
    args: Args,
    ctx: &ActionContext<'_>,
) -> Candidate {
    let state = action.enabled(&args, ctx);
    Candidate {
        section,
        label,
        shortcut: None,
        choice: Choice::Run { action, args },
        state,
    }
}

/// A candidate that matched, as `rank` sorts it: section, disabled, score, index
/// among the candidates, and the matched parts of its label.
type Scored = (Section, bool, i64, usize, Vec<Range<usize>>);

/// The hits for `query`, sections in their order. Within a section the enabled
/// candidates come first, then the disabled ones; each group is best first. Equal
/// scores keep the order of the candidates.
#[must_use]
pub(crate) fn rank(query: &str, candidates: &[Candidate]) -> Vec<Hit> {
    let mut scored: Vec<Scored> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            fuzzy::fuzzy_match(query, &candidate.label).map(|found| {
                (
                    candidate.section,
                    candidate.state.is_err(),
                    found.score,
                    index,
                    found.ranges,
                )
            })
        })
        .collect();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(b.2.cmp(&a.2))
            .then(a.3.cmp(&b.3))
    });
    scored
        .into_iter()
        .map(|(_, _, _, index, ranges)| Hit { index, ranges })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::move_file::{SAME_BRANCH, SAME_WORKTREE};
    use gm_schema::{Change, StashEntry, StatusEntry, WorktreeEntry};

    use super::*;

    #[test]
    fn tab_is_offered_only_for_a_row_that_picks_its_argument() {
        let pick = Choice::Pick(Action::CheckoutBranch);
        // An action with no argument, and one whose argument the palette fills in or
        // lists disabled, are runs: Tab does nothing on them.
        let plain = Choice::Run {
            action: Action::Commit,
            args: Args::None,
        };
        let disabled = Choice::Run {
            action: Action::OpenReviewComment,
            args: Args::None,
        };
        assert!(tab_picks_argument(Some(&pick)));
        assert!(!tab_picks_argument(Some(&plain)));
        assert!(!tab_picks_argument(Some(&disabled)));
        assert!(!tab_picks_argument(None));
    }

    fn entry(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
            submodule: None,
        }
    }

    fn branch(name: &str, current: bool) -> BranchEntry {
        BranchEntry {
            name: name.into(),
            is_current: current,
            is_remote: false,
            upstream: None,
            author_email: None,
        }
    }

    fn ctx_with<'a>(entries: &'a [StatusEntry], message: &'a str) -> ActionContext<'a> {
        ActionContext {
            repo_open: true,
            selected_file: None,
            selected_thread: None,
            code: crate::action_registry::CodeCtx::default(),
            submodules: None,
            selected_label: None,
            selected_memory: None,
            features: gm_schema::FeatureSet::all(),
            stack: crate::action_registry::StackCtx::default(),
            busy: None,
            entries,
            staged_count: entries.iter().filter(|e| e.staged.is_some()).count(),
            message,
            current_branch: Some("main"),
            root: Some("/repo"),
            messages: crate::action_registry::MessageCtx::default(),
            workflows: crate::action_registry::WorkflowCtx::default(),
            worktrees: &[],
            workspaces: &[],
            author_pictures: crate::action_registry::AuthorPictures::Hidden,
            remote_running: false,
            stashes: None,
            review: crate::action_registry::ReviewCtx::default(),
            undoable: None,
        }
    }

    fn worktree(path: &str, branch: &str, is_main: bool) -> WorktreeEntry {
        WorktreeEntry {
            path: path.into(),
            head: Some("abc".into()),
            branch: Some(branch.into()),
            detached: false,
            bare: false,
            is_main,
            locked: None,
            prunable: None,
        }
    }

    fn ctx_with_worktrees<'a>(
        entries: &'a [StatusEntry],
        worktrees: &'a [WorktreeEntry],
    ) -> ActionContext<'a> {
        ActionContext {
            worktrees,
            ..ctx_with(entries, "")
        }
    }

    fn find(candidates: &[Candidate], label: &str) -> Candidate {
        candidates
            .iter()
            .find(|candidate| candidate.label == label)
            .unwrap_or_else(|| panic!("no candidate labelled {label}"))
            .clone()
    }

    /// Ranks `candidates` for the palette's current query, as the window does.
    fn shown(palette: &mut Palette, candidates: Vec<Candidate>) {
        let job = palette.begin(candidates);
        assert!(palette.apply(job.run()));
    }

    #[test]
    fn the_top_level_lists_every_action_and_puts_worktrees_in_their_own_section() {
        let entries: [StatusEntry; 0] = [];
        let candidates = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        let actions = candidates
            .iter()
            .filter(|c| c.section == Section::Actions)
            .count();
        let worktree_actions = Action::ALL
            .iter()
            .filter(|action| action.spec().category == Category::Worktrees)
            .count();
        let workspace_actions = Action::ALL
            .iter()
            .filter(|action| action.spec().category == Category::Workspaces)
            .count();
        // Choose Parent is only the second step of Set Parent…; the folder actions run from a
        // folder's box in the Changes list.
        let unlisted = Action::ALL
            .iter()
            .filter(|action| !action.listed_in_palette())
            .count();
        assert_eq!(unlisted, 3);
        assert_eq!(
            actions,
            Action::ALL.len() - worktree_actions - workspace_actions - unlisted
        );
        assert!(candidates.iter().all(|c| c.label != "Choose Parent"));
        let worktree = find(&candidates, "Create Worktree");
        assert_eq!(worktree.section, Section::Worktrees);
        assert_eq!(worktree.state, Err(NO_BRANCH));
        let with_branch = build(
            Stage::Actions,
            "",
            &ctx_with(&entries, ""),
            &[branch("main", true)],
        );
        assert_eq!(find(&with_branch, "Create Worktree").state, Ok(()));
        let restack = build(
            Stage::Actions,
            "",
            &ctx_with(&entries, ""),
            &[branch("main", true)],
        );
        assert_eq!(
            find(&restack, "Restack This Stack").state,
            Err("The stacks are still being read.")
        );
    }

    #[test]
    fn a_disabled_action_keeps_the_registry_reason() {
        let entries = [entry("a.txt", Some(Change::Added), None)];
        let empty_message = build(Stage::Actions, "", &ctx_with(&entries, " "), &[]);
        assert_eq!(
            find(&empty_message, "Commit").state,
            Err("Nothing to commit: the message is empty.")
        );
        let nothing_staged: [StatusEntry; 0] = [];
        let no_stage = build(Stage::Actions, "", &ctx_with(&nothing_staged, "msg"), &[]);
        assert_eq!(find(&no_stage, "Commit").state, Err("Nothing is staged."));
    }

    #[test]
    fn branches_appear_only_once_something_is_typed() {
        let entries: [StatusEntry; 0] = [];
        let branches = [branch("main", true), branch("feature/x", false)];
        let ctx = ctx_with(&entries, "");
        let empty = build(Stage::Actions, "", &ctx, &branches);
        assert!(empty.iter().all(|c| c.section != Section::Branches));

        let typed = build(Stage::Actions, "fea", &ctx, &branches);
        let listed: Vec<&Candidate> = typed
            .iter()
            .filter(|c| c.section == Section::Branches)
            .collect();
        assert_eq!(listed.len(), 2);
        assert_eq!(
            find(&typed, "main").state,
            Err("This is the current branch")
        );
        assert_eq!(find(&typed, "feature/x").state, Ok(()));
    }

    #[test]
    fn checkout_is_offered_as_a_pick_and_the_pick_lists_each_branch() {
        let entries: [StatusEntry; 0] = [];
        let branches = [branch("main", true), branch("feature/x", false)];
        let ctx = ctx_with(&entries, "");
        let top = build(Stage::Actions, "", &ctx, &branches);
        let checkout = find(&top, "Checkout Branch");
        assert_eq!(checkout.choice, Choice::Pick(Action::CheckoutBranch));
        assert_eq!(checkout.state, Ok(()));

        let pick = build(Stage::Pick(Action::CheckoutBranch), "", &ctx, &branches);
        assert_eq!(pick.len(), 2);
        assert_eq!(
            find(&pick, "feature/x").choice,
            Choice::Run {
                action: Action::CheckoutBranch,
                args: Args::Branch("feature/x".into()),
            }
        );
        assert_eq!(find(&pick, "main").state, Err("This is the current branch"));
    }

    #[test]
    fn a_pick_with_nothing_to_choose_is_disabled_with_a_reason() {
        let entries: [StatusEntry; 0] = [];
        let top = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        assert_eq!(
            find(&top, "Checkout Branch").state,
            Err("No branch to choose from.")
        );
        assert_eq!(
            find(&top, "Stage File").state,
            Err("No file in the working copy.")
        );
    }

    #[test]
    fn a_pick_whose_targets_all_refuse_says_what_the_first_one_refused() {
        let entries = [entry("a.txt", Some(Change::Added), None)];
        let top = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        assert_eq!(
            find(&top, "Stage File").state,
            Err("Nothing unstaged in this file")
        );
    }

    #[test]
    fn stage_file_picks_among_the_files_and_refuses_the_staged_ones() {
        let entries = [
            entry("a.txt", None, Some(Change::Modified)),
            entry("b.txt", Some(Change::Added), None),
        ];
        let pick = build(
            Stage::Pick(Action::StageFile),
            "",
            &ctx_with(&entries, ""),
            &[],
        );
        assert_eq!(pick.len(), 2);
        assert!(pick.iter().all(|c| c.section == Section::Files));
        assert_eq!(find(&pick, "a.txt").state, Ok(()));
        assert_eq!(
            find(&pick, "b.txt").state,
            Err("Nothing unstaged in this file")
        );
        assert_eq!(
            find(&pick, "a.txt").choice,
            Choice::Run {
                action: Action::StageFile,
                args: Args::File("a.txt".into()),
            }
        );
    }

    #[test]
    fn chunk_actions_are_listed_disabled_because_they_run_from_the_diff() {
        let entries: [StatusEntry; 0] = [];
        let top = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        assert_eq!(
            find(&top, "Discard Chunk").state,
            Err("Pick a chunk in the diff first.")
        );
    }

    #[test]
    fn commit_copies_are_listed_disabled_because_they_run_from_the_history() {
        let entries: [StatusEntry; 0] = [];
        let top = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        for title in ["Copy Commit Hash", "Copy Commit Message"] {
            assert_eq!(
                find(&top, title).state,
                Err("Pick a commit in the history first."),
                "{title}"
            );
        }
    }

    #[test]
    fn local_review_is_offered_and_open_comment_says_to_pick_it_on_its_page() {
        let entries: [StatusEntry; 0] = [];
        let top = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        assert_eq!(find(&top, "Show Local Review").state, Ok(()));
        assert_eq!(
            find(&top, "Open Comment in Diff").state,
            Err("Pick a comment on the Local review page first.")
        );
        assert_eq!(
            find(&top, "Resolve Comment").state,
            Err("Pick the comment on its card in the diff.")
        );
    }

    #[test]
    fn reflog_commit_actions_are_listed_disabled_and_show_reflog_is_offered() {
        let entries: [StatusEntry; 0] = [];
        let top = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        for title in ["Check Out Commit (Detached)", "New Branch From Commit…"] {
            assert_eq!(
                find(&top, title).state,
                Err("Pick a commit on the Reflog page first."),
                "{title}"
            );
        }
        assert_eq!(find(&top, "Show Reflog").state, Ok(()));
    }

    #[test]
    fn a_query_ranks_the_actions_first_and_the_branches_after_them() {
        let entries: [StatusEntry; 0] = [];
        let branches = [branch("stash-me", false), branch("main", true)];
        let candidates = build(Stage::Actions, "st", &ctx_with(&entries, ""), &branches);
        let hits = rank("st", &candidates);
        let sections: Vec<Section> = hits
            .iter()
            .map(|hit| candidates[hit.index].section)
            .collect();
        assert!(sections.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(sections.last(), Some(&Section::Branches));
        let top = &candidates[hits[0].index];
        assert_eq!(top.section, Section::Actions);
        assert_eq!(top.state, Ok(()), "an enabled action leads the matches");
    }

    #[test]
    fn typing_merged_finds_delete_merged_branches_as_an_action() {
        let entries: [StatusEntry; 0] = [];
        let candidates = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        for query in ["merged", "delete merged"] {
            let hits = rank(query, &candidates);
            let row = hits
                .iter()
                .map(|hit| &candidates[hit.index])
                .find(|candidate| candidate.label == "Delete Merged Branches…")
                .unwrap_or_else(|| panic!("{query:?} does not find the action"));
            assert_eq!(row.section, Section::Actions, "{query:?}");
            assert_eq!(row.state, Ok(()), "{query:?}");
        }
    }

    #[test]
    fn an_empty_query_keeps_the_registry_order() {
        let entries: [StatusEntry; 0] = [];
        let candidates = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        let hits = rank("", &candidates);
        assert_eq!(hits.len(), candidates.len());
        // Enabled rows lead their section, so the first is the first enabled action.
        let first_enabled = candidates
            .iter()
            .find(|c| c.section == Section::Actions && c.state.is_ok())
            .expect("an enabled action");
        assert_eq!(candidates[hits[0].index].label, first_enabled.label);
        assert!(hits.iter().all(|hit| hit.ranges.is_empty()));
    }

    #[test]
    fn the_list_puts_a_header_before_each_section() {
        let entries: [StatusEntry; 0] = [];
        let mut palette = Palette::default();
        palette.open();
        let candidates = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        shown(&mut palette, candidates);
        let headers: Vec<Line> = palette
            .lines()
            .into_iter()
            .filter(|line| matches!(line, Line::Header(_)))
            .collect();
        assert_eq!(
            headers,
            [
                Line::Header(Section::Actions),
                Line::Header(Section::Worktrees),
                Line::Header(Section::Workspaces)
            ]
        );
    }

    #[test]
    fn the_highlight_moves_and_stops_at_both_ends() {
        let entries: [StatusEntry; 0] = [];
        let mut palette = Palette::default();
        palette.open();
        let candidates = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        let count = candidates.len();
        let first_enabled = candidates
            .iter()
            .find(|c| c.section == Section::Actions && c.state.is_ok())
            .map(|c| c.label.clone());
        shown(&mut palette, candidates);
        // The count is every match; the highlight moves over the choosable lines.
        assert_eq!(palette.count(), count);
        let choosable = palette
            .lines()
            .iter()
            .filter(|line| !matches!(line, Line::Header(_)))
            .count();
        assert!(choosable < count, "the disabled rows are collapsed");
        assert_eq!(
            palette.highlighted().map(|c| c.label.clone()),
            first_enabled
        );
        palette.move_highlight(false);
        assert_eq!(palette.highlight, 0);
        for _ in 0..count + 2 {
            palette.move_highlight(true);
        }
        assert_eq!(palette.highlight, choosable - 1);
        palette.select(0);
        assert_eq!(palette.highlight, 0);
        palette.select(choosable);
        assert_eq!(palette.highlight, 0, "a position past the list is ignored");
    }

    #[test]
    fn a_ranking_from_an_older_query_is_dropped() {
        let entries: [StatusEntry; 0] = [];
        let mut palette = Palette::default();
        palette.open();
        let first = palette.begin(build(Stage::Actions, "", &ctx_with(&entries, ""), &[]));
        palette.set_query("com");
        let second = palette.begin(build(Stage::Actions, "com", &ctx_with(&entries, ""), &[]));
        assert!(!palette.apply(first.run()));
        assert!(palette.apply(second.run()));
        // Commit is disabled here, so it is hidden behind the section's MoreDisabled line.
        let top = palette.highlighted().expect("an enabled match");
        assert!(top.state.is_ok());
        assert_ne!(top.label, "Commit");
    }

    #[test]
    fn the_argument_step_starts_over_and_leaves_back_to_the_top() {
        let mut palette = Palette::default();
        palette.open();
        palette.set_query("checkout");
        palette.enter_pick(Action::CheckoutBranch);
        assert_eq!(palette.stage(), Stage::Pick(Action::CheckoutBranch));
        assert_eq!(palette.query(), "");
        assert!(palette.leave_pick());
        assert_eq!(palette.stage(), Stage::Actions);
        assert!(!palette.leave_pick());
    }

    #[test]
    fn only_the_first_line_of_the_query_counts() {
        let mut palette = Palette::default();
        palette.open();
        palette.set_query("stage\nmore");
        assert_eq!(palette.query(), "stage");
    }

    #[test]
    fn remove_worktree_is_picked_from_the_listed_worktrees_not_refused() {
        let entries: [StatusEntry; 0] = [];
        let worktrees = [
            worktree("/r", "main", true),
            worktree("/r-feature", "feature", false),
        ];
        let ctx = ctx_with_worktrees(&entries, &worktrees);
        let top = build(Stage::Actions, "", &ctx, &[]);
        let remove = find(&top, "Remove Worktree");
        assert_eq!(remove.section, Section::Worktrees);
        assert_eq!(remove.choice, Choice::Pick(Action::RemoveWorktree));
        assert_eq!(remove.state, Ok(()));

        let pick = build(Stage::Pick(Action::RemoveWorktree), "", &ctx, &[]);
        assert_eq!(pick.len(), 2);
        assert!(pick.iter().all(|c| c.section == Section::Worktrees));
        // The main checkout is listed first, disabled with the registry's reason.
        assert_eq!(pick[0].label, "main");
        assert_eq!(pick[0].state, Err("The main checkout cannot be removed."));
        assert_eq!(pick[1].label, "feature");
        assert_eq!(pick[1].state, Ok(()));
        assert_eq!(
            pick[1].choice,
            Choice::Run {
                action: Action::RemoveWorktree,
                args: Args::Worktree("/r-feature".into()),
            }
        );
    }

    fn stash(index: u32, oid: &str) -> StashEntry {
        StashEntry {
            index,
            oid: oid.repeat(40),
            message: format!("On main: stash {index}"),
            branch: Some("main".into()),
            time: 1_000,
        }
    }

    #[test]
    fn a_stash_action_is_a_pick_of_the_listed_stashes_newest_first() {
        let entries: [StatusEntry; 0] = [];
        let stashes = [stash(0, "a"), stash(1, "b")];
        let ctx = ActionContext {
            stashes: Some(&stashes[..]),
            review: crate::action_registry::ReviewCtx::default(),
            ..ctx_with(&entries, "")
        };
        let top = build(Stage::Actions, "", &ctx, &[]);
        let row = find(&top, "Apply Stash");
        assert_eq!(row.section, Section::Actions);
        assert_eq!(row.choice, Choice::Pick(Action::StashApply));
        assert_eq!(row.state, Ok(()));

        let picks = build(Stage::Pick(Action::StashDrop), "", &ctx, &[]);
        let labels: Vec<&str> = picks.iter().map(|pick| pick.label.as_str()).collect();
        assert_eq!(
            labels,
            ["stash@{0}  On main: stash 0", "stash@{1}  On main: stash 1"]
        );
        for (pick, entry) in picks.iter().zip(&stashes) {
            assert_eq!(pick.section, Section::Stashes);
            assert_eq!(pick.state, Ok(()));
            assert_eq!(
                pick.choice,
                Choice::Run {
                    action: Action::StashDrop,
                    args: stash_args(entry),
                }
            );
        }
    }

    #[test]
    fn a_stash_pick_says_to_open_the_page_until_the_stashes_are_listed() {
        let entries: [StatusEntry; 0] = [];
        let unlisted = ctx_with(&entries, "");
        let top = build(Stage::Actions, "", &unlisted, &[]);
        for label in ["Apply Stash", "Pop Stash", "Drop Stash"] {
            let row = find(&top, label);
            assert_eq!(row.choice, Choice::Pick(row_action(label)), "{label}");
            assert_eq!(row.state, Err("Open the Stashes page first."), "{label}");
        }
        assert!(build(Stage::Pick(Action::StashPop), "", &unlisted, &[]).is_empty());
    }

    #[test]
    fn a_listing_with_no_stash_says_there_is_none_to_choose() {
        let entries: [StatusEntry; 0] = [];
        let listed = ActionContext {
            stashes: Some(&[]),
            review: crate::action_registry::ReviewCtx::default(),
            ..ctx_with(&entries, "")
        };
        let top = build(Stage::Actions, "", &listed, &[]);
        assert_eq!(
            find(&top, "Pop Stash").state,
            Err("No stash to choose from.")
        );
        assert!(build(Stage::Pick(Action::StashPop), "", &listed, &[]).is_empty());
    }

    #[test]
    fn a_worktree_pick_with_only_the_main_checkout_says_why_it_cannot_run() {
        let entries: [StatusEntry; 0] = [];
        let worktrees = [worktree("/r", "main", true)];
        let ctx = ctx_with_worktrees(&entries, &worktrees);
        let top = build(Stage::Actions, "", &ctx, &[]);
        assert_eq!(
            find(&top, "Remove Worktree").state,
            Err("The main checkout cannot be removed.")
        );
        assert_eq!(
            find(&top, "Open Worktree").state,
            Ok(()),
            "the main checkout can be opened"
        );
    }

    #[test]
    fn a_worktree_action_with_no_worktree_to_pick_says_so() {
        let entries: [StatusEntry; 0] = [];
        let top = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        for label in [
            "Open Worktree",
            "Reveal Worktree in File Manager",
            "Copy Worktree Path",
            "Remove Worktree",
            "Lock Worktree",
            "Unlock Worktree",
        ] {
            let row = find(&top, label);
            assert_eq!(row.choice, Choice::Pick(row_action(label)), "{label}");
            assert_eq!(row.state, Err(NO_WORKTREE), "{label}");
        }
        let pick = build(
            Stage::Pick(Action::RemoveWorktree),
            "",
            &ctx_with(&entries, ""),
            &[],
        );
        assert!(pick.is_empty());
    }

    /// The action a worktree row label names, for the table above.
    fn row_action(label: &str) -> Action {
        Action::ALL
            .into_iter()
            .find(|action| action.spec().title == label)
            .unwrap_or_else(|| panic!("no action titled {label}"))
    }

    #[test]
    fn new_worktree_is_a_plain_row_that_runs_with_no_target() {
        let entries: [StatusEntry; 0] = [];
        let top = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        let row = find(&top, "New Worktree…");
        assert_eq!(row.section, Section::Worktrees);
        assert_eq!(
            row.choice,
            Choice::Run {
                action: Action::NewWorktree,
                args: Args::None,
            }
        );
        assert_eq!(row.state, Ok(()));
    }

    #[test]
    fn a_worktree_query_finds_the_worktree_pick_by_its_name() {
        let entries: [StatusEntry; 0] = [];
        let worktrees = [
            worktree("/r", "main", true),
            worktree("/r-feature", "feature", false),
        ];
        let ctx = ctx_with_worktrees(&entries, &worktrees);
        let candidates = build(Stage::Pick(Action::RemoveWorktree), "", &ctx, &[]);
        let mut palette = Palette::default();
        palette.open();
        palette.enter_pick(Action::RemoveWorktree);
        shown(&mut palette, candidates);
        assert_eq!(palette.count(), 2);
        palette.set_query("feat");
        let job = palette.begin(build(
            Stage::Pick(Action::RemoveWorktree),
            "feat",
            &ctx,
            &[],
        ));
        assert!(palette.apply(job.run()));
        assert_eq!(palette.count(), 1);
        assert_eq!(
            palette.highlighted().map(|c| c.label.as_str()),
            Some("feature")
        );
    }

    fn workflow(name: &str, trigger: &str, error: Option<&str>) -> gm_schema::WorkflowInfo {
        gm_schema::WorkflowInfo {
            name: name.into(),
            path: format!("/w/{name}.yml"),
            trigger: trigger.into(),
            key: None,
            inputs: Vec::new(),
            hash: "h".into(),
            error: error.map(str::to_owned),
            warnings: Vec::new(),
            source: String::new(),
            reviewed: false,
            auto_approve: false,
            enabled: false,
            title: None,
        }
    }

    #[test]
    fn run_workflow_is_a_pick_of_the_listed_workflows_with_their_reasons() {
        let listed = [
            workflow("suggest-commit-message", "manual", None),
            workflow("broken", "manual", Some("line 2: bad")),
        ];
        let ctx = ActionContext {
            workflows: crate::action_registry::WorkflowCtx {
                listed: Some(&listed),
                ..Default::default()
            },
            ..ctx_with(&[], "")
        };
        let top = build(Stage::Actions, "", &ctx, &[]);
        let run = find(&top, "Run Workflow…");
        assert_eq!(run.choice, Choice::Pick(Action::RunWorkflow));
        assert_eq!(run.state, Ok(()));

        let picks = build(Stage::Pick(Action::RunWorkflow), "", &ctx, &[]);
        assert_eq!(picks.len(), 2);
        assert!(picks.iter().all(|c| c.section == Section::Workflows));
        let ready = find(&picks, "suggest-commit-message");
        assert_eq!(
            ready.choice,
            Choice::Run {
                action: Action::RunWorkflow,
                args: Args::Workflow("suggest-commit-message".into()),
            }
        );
        assert_eq!(ready.state, Ok(()));
        assert_eq!(
            find(&picks, "broken").state,
            Err("This workflow does not load: fix its file first.")
        );
    }

    #[test]
    fn run_workflow_with_nothing_listed_says_so() {
        let ctx = ctx_with(&[], "");
        let top = build(Stage::Actions, "", &ctx, &[]);
        assert_eq!(
            find(&top, "Run Workflow…").state,
            Err("Workflows are still being read.")
        );
        let listed_none = ActionContext {
            workflows: crate::action_registry::WorkflowCtx {
                listed: Some(&[]),
                ..Default::default()
            },
            ..ctx_with(&[], "")
        };
        let top = build(Stage::Actions, "", &listed_none, &[]);
        assert_eq!(
            find(&top, "Run Workflow…").state,
            Err("No workflow to choose from.")
        );
    }

    #[test]
    fn new_from_template_is_a_pick_of_the_listed_templates() {
        let templates = ["commit-message".to_owned(), "branch-from-ticket".to_owned()];
        let ctx = ActionContext {
            workflows: crate::action_registry::WorkflowCtx {
                templates: &templates,
                ..Default::default()
            },
            ..ctx_with(&[], "")
        };
        let top = build(Stage::Actions, "", &ctx, &[]);
        let new = find(&top, "New Workflow from Template…");
        assert_eq!(new.choice, Choice::Pick(Action::NewWorkflowFromTemplate));
        assert_eq!(new.state, Ok(()));
        let picks = build(Stage::Pick(Action::NewWorkflowFromTemplate), "", &ctx, &[]);
        assert_eq!(picks.len(), 2);
        assert_eq!(
            find(&picks, "commit-message").choice,
            Choice::Run {
                action: Action::NewWorkflowFromTemplate,
                args: Args::Template("commit-message".into()),
            }
        );
    }

    #[test]
    fn new_from_template_with_no_listing_says_so() {
        let ctx = ctx_with(&[], "");
        let top = build(Stage::Actions, "", &ctx, &[]);
        assert_eq!(
            find(&top, "New Workflow from Template…").state,
            Err("No templates listed yet")
        );
    }

    #[test]
    fn approve_reject_and_cancel_are_listed_with_their_reasons() {
        let ctx = ctx_with(&[], "");
        let top = build(Stage::Actions, "", &ctx, &[]);
        assert_eq!(
            find(&top, "Approve Workflow Step").state,
            Err("No workflow run waits for approval.")
        );
        assert_eq!(
            find(&top, "Cancel Workflow Run").state,
            Err("No workflow run is active.")
        );
    }

    #[test]
    fn move_file_to_branch_is_a_pick_of_the_branches_for_the_selected_file() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let branches = [branch("main", true), branch("feature", false)];
        let mut ctx = ctx_with(&entries, "");
        ctx.selected_file = Some("a.txt");
        let top = build(Stage::Actions, "", &ctx, &branches);
        let row = find(&top, "Move File to Branch…");
        assert_eq!(row.choice, Choice::Pick(Action::MoveFileToBranch));
        assert_eq!(row.state, Ok(()));
        let picks = build(Stage::Pick(Action::MoveFileToBranch), "", &ctx, &branches);
        let feature = find(&picks, "feature");
        assert_eq!(
            feature.choice,
            Choice::Run {
                action: Action::MoveFileToBranch,
                args: Args::Branch("feature".into()),
            }
        );
        assert_eq!(feature.state, Ok(()));
        assert_eq!(find(&picks, "main").state, Err(SAME_BRANCH));
    }

    #[test]
    fn move_file_says_to_select_a_file_when_none_is_selected() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let branches = [branch("feature", false)];
        let ctx = ctx_with(&entries, "");
        let top = build(Stage::Actions, "", &ctx, &branches);
        assert_eq!(
            find(&top, "Move File to Branch…").state,
            Err("Select a changed file to move first.")
        );
    }

    #[test]
    fn move_file_to_worktree_is_a_pick_of_the_listed_worktrees() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let worktrees = [
            worktree("/repo", "main", true),
            worktree("/w/ui", "ui", false),
        ];
        let mut ctx = ctx_with_worktrees(&entries, &worktrees);
        ctx.selected_file = Some("a.txt");
        let top = build(Stage::Actions, "", &ctx, &[]);
        let row = find(&top, "Move File to Worktree…");
        assert_eq!(row.choice, Choice::Pick(Action::MoveFileToWorktree));
        let picks = build(Stage::Pick(Action::MoveFileToWorktree), "", &ctx, &[]);
        assert_eq!(find(&picks, "ui").state, Ok(()));
        assert_eq!(find(&picks, "main").state, Err(SAME_WORKTREE));
    }
    #[test]
    fn a_label_action_runs_on_the_selected_label_or_says_none_is_selected() {
        let open = ctx_with(&[], "");
        let none = action_row(Action::LabelRename, &open, &[]);
        assert_eq!(none.state, Err("Select a label first."));
        let selected = ActionContext {
            selected_label: Some(crate::action_registry::LabelRef {
                id: 5,
                name: "flaky",
                attachments: 2,
            }),
            ..open
        };
        let candidate = action_row(Action::LabelRename, &selected, &[]);
        assert_eq!(candidate.state, Ok(()));
        assert_eq!(
            candidate.choice,
            Choice::Run {
                action: Action::LabelRename,
                args: Args::Label {
                    id: 5,
                    name: "flaky".into(),
                    attachments: 2,
                },
            }
        );
    }

    #[test]
    fn a_thread_action_runs_on_the_selected_thread_or_says_none_is_selected() {
        let open = ctx_with(&[], "");
        let none = action_row(Action::ThreadStar, &open, &[]);
        assert_eq!(none.state, Err("Select a thread first."));
        let selected = ActionContext {
            selected_thread: Some(crate::action_registry::ThreadRef {
                uid: "u1",
                starred: true,
            }),
            ..open
        };
        let candidate = action_row(Action::ThreadStar, &selected, &[]);
        assert_eq!(candidate.state, Ok(()));
        assert_eq!(
            candidate.choice,
            Choice::Run {
                action: Action::ThreadStar,
                args: Args::Thread {
                    uid: "u1".into(),
                    starred: true,
                },
            }
        );
    }

    #[test]
    fn a_preview_action_is_listed_only_while_its_feature_is_on() {
        let entries: [StatusEntry; 0] = [];
        let off = ActionContext {
            features: gm_schema::FeatureSet::none(),
            ..ctx_with(&entries, "")
        };
        let on = ActionContext {
            features: gm_schema::FeatureSet::all(),
            ..ctx_with(&entries, "")
        };
        // A row runs its action, or picks its target first; either way it lists the action.
        let lists_preview = |candidates: &[Candidate]| {
            candidates.iter().any(|candidate| match &candidate.choice {
                Choice::Run { action, .. } | Choice::Pick(action) => action.feature().is_some(),
            })
        };
        assert!(!lists_preview(&build(Stage::Actions, "", &off, &[])));
        assert!(lists_preview(&build(Stage::Actions, "", &on, &[])));
        assert!(
            build(Stage::Actions, "", &off, &[])
                .iter()
                .any(|candidate| candidate.choice
                    == Choice::Run {
                        action: Action::Commit,
                        args: Args::None
                    })
        );
    }

    #[test]
    fn remove_workspace_is_picked_from_the_listed_workspaces_the_window_s_own_included() {
        let entries: [StatusEntry; 0] = [];
        let rows = [
            gm_schema::WorkspaceRow {
                name: "billing".into(),
                path: "/w/billing".into(),
                members: 2,
            },
            gm_schema::WorkspaceRow {
                name: "checkout".into(),
                path: "/w/checkout".into(),
                members: 2,
            },
        ];
        let context = ActionContext {
            root: Some("/w/checkout"),
            workspaces: &rows,
            ..ctx_with(&entries, "")
        };
        let top = build(Stage::Actions, "", &context, &[]);
        let remove = find(&top, "Remove Workspace");
        assert_eq!(remove.section, Section::Workspaces);
        assert_eq!(remove.choice, Choice::Pick(Action::RemoveWorkspace));
        let pick = build(Stage::Pick(Action::RemoveWorkspace), "", &context, &[]);
        assert!(pick.iter().all(|c| c.section == Section::Workspaces));
        assert_eq!(find(&pick, "billing").state, Ok(()));
        assert_eq!(find(&pick, "checkout").state, Ok(()));
    }

    #[test]
    fn a_workspace_pick_with_no_workspace_listed_says_so() {
        let entries: [StatusEntry; 0] = [];
        let top = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        assert_eq!(
            find(&top, "Remove Workspace").state,
            Err("No workspace to choose from.")
        );
    }

    /// The rows of the list as (section, label-less kind) pairs, for the shape tests.
    fn shape(palette: &Palette) -> Vec<String> {
        palette
            .lines()
            .iter()
            .map(|line| match line {
                Line::Header(section) => format!("header {}", section.title()),
                Line::Row { hit, .. } => format!("row {hit}"),
                Line::MoreDisabled { section, count, .. } => {
                    format!("more {} {count}", section.title())
                }
            })
            .collect()
    }

    #[test]
    fn enabled_rows_rank_before_disabled_rows_within_a_section() {
        let entries: [StatusEntry; 0] = [];
        let candidates = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        let hits = rank("", &candidates);
        let actions: Vec<bool> = hits
            .iter()
            .map(|hit| &candidates[hit.index])
            .filter(|c| c.section == Section::Actions)
            .map(|c| c.state.is_ok())
            .collect();
        let first_disabled = actions
            .iter()
            .position(|ok| !ok)
            .expect("some action is disabled");
        assert!(first_disabled > 0, "some action is enabled");
        assert!(actions[first_disabled..].iter().all(|ok| !ok));
    }

    #[test]
    fn disabled_rows_are_hidden_behind_one_more_line_per_section() {
        let entries: [StatusEntry; 0] = [];
        let candidates = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        let mut palette = Palette::default();
        palette.open();
        shown(&mut palette, candidates.clone());
        let lines = palette.lines();
        let rows = lines
            .iter()
            .filter(|line| matches!(line, Line::Row { .. }))
            .count();
        let hidden: usize = lines
            .iter()
            .filter_map(|line| match line {
                Line::MoreDisabled { count, .. } => Some(*count),
                _ => None,
            })
            .sum();
        let enabled = candidates.iter().filter(|c| c.state.is_ok()).count();
        assert_eq!(rows, enabled);
        assert_eq!(hidden, candidates.len() - enabled);
        let more_actions = lines.iter().any(|line| {
            matches!(
                line,
                Line::MoreDisabled {
                    section: Section::Actions,
                    count,
                    ..
                } if *count == candidates.iter().filter(|c| {
                    c.section == Section::Actions && c.state.is_err()
                }).count()
            )
        });
        assert!(more_actions, "the Actions disabled rows are counted");
        // The MoreDisabled line sits after the section's enabled rows.
        let more = lines
            .iter()
            .position(|line| matches!(line, Line::MoreDisabled { .. }))
            .expect("a MoreDisabled line");
        assert!(matches!(lines[more - 1], Line::Row { .. }));
    }

    #[test]
    fn a_section_with_only_disabled_rows_keeps_its_header_and_gets_a_more_line() {
        let entries: [StatusEntry; 0] = [];
        let worktrees = [worktree("/r", "main", true)];
        let ctx = ctx_with_worktrees(&entries, &worktrees);
        let mut palette = Palette::default();
        palette.open();
        palette.enter_pick(Action::RemoveWorktree);
        shown(
            &mut palette,
            build(Stage::Pick(Action::RemoveWorktree), "", &ctx, &[]),
        );
        assert_eq!(shape(&palette), ["header Worktrees", "more Worktrees 1"]);
        assert_eq!(
            palette.highlighted(),
            None,
            "the highlight starts on the More line"
        );
        assert!(palette.highlighted_more());
    }

    #[test]
    fn the_highlight_can_land_on_the_more_line_and_expanding_keeps_it_on_the_first_hidden_row() {
        let entries: [StatusEntry; 0] = [];
        let candidates = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        let enabled_actions = candidates
            .iter()
            .filter(|c| c.section == Section::Actions && c.state.is_ok())
            .count();
        let mut palette = Palette::default();
        palette.open();
        shown(&mut palette, candidates);
        // The Actions rows come first, so the Actions More line is the next choosable line.
        for _ in 0..enabled_actions {
            palette.move_highlight(true);
        }
        assert!(palette.highlighted_more());
        assert!(palette.highlighted().is_none());
        assert!(matches!(
            palette.lines()[palette.highlight_line().expect("highlighted")],
            Line::MoreDisabled {
                section: Section::Actions,
                highlighted: true,
                ..
            }
        ));

        palette.expand_disabled();
        assert!(!palette.highlighted_more());
        let first = palette.highlighted().expect("the first hidden row");
        assert!(first.state.is_err());
        assert!(
            palette.lines().iter().all(|line| !matches!(
                line,
                Line::MoreDisabled {
                    section: Section::Actions,
                    ..
                }
            )),
            "the section's disabled rows are all listed now"
        );
        assert!(palette.highlight_line().is_some());
    }

    #[test]
    fn a_new_query_collapses_the_disabled_rows_again() {
        let entries: [StatusEntry; 0] = [];
        let mut palette = Palette::default();
        palette.open();
        shown(
            &mut palette,
            build(Stage::Actions, "", &ctx_with(&entries, ""), &[]),
        );
        palette.expand_disabled();
        assert!(
            palette
                .lines()
                .iter()
                .all(|line| !matches!(line, Line::MoreDisabled { .. }))
        );
        palette.set_query("com");
        shown(
            &mut palette,
            build(Stage::Actions, "com", &ctx_with(&entries, ""), &[]),
        );
        assert!(
            palette
                .lines()
                .iter()
                .any(|line| matches!(line, Line::MoreDisabled { .. })),
            "the disabled rows are collapsed for the new query"
        );
    }

    #[test]
    fn a_scoped_palette_lists_only_its_items_with_their_registry_state() {
        let entries = [
            entry("a.txt", None, Some(Change::Modified)),
            entry("b.txt", Some(Change::Added), None),
        ];
        let ctx = ctx_with(&entries, "");
        let items = vec![
            (Action::StageFile, Args::File("a.txt".into())),
            (Action::StageFile, Args::File("b.txt".into())),
        ];
        let candidates = build_scoped(&items, &ctx);
        let labels: Vec<&str> = candidates.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Stage File", "Stage File"]);
        assert_eq!(candidates[0].state, Ok(()));
        assert_eq!(candidates[1].state, Err("Nothing unstaged in this file"));
        assert_eq!(
            candidates[0].choice,
            Choice::Run {
                action: Action::StageFile,
                args: Args::File("a.txt".into()),
            }
        );

        let mut palette = Palette::default();
        palette.open_scoped("Actions for a.txt".into(), items);
        assert_eq!(palette.scope_title(), Some("Actions for a.txt"));
        assert_eq!(palette.scope().map(<[_]>::len), Some(2));
        shown(&mut palette, candidates);
        assert_eq!(palette.count(), 2);
        // Opening the palette again, unscoped, clears the scope.
        palette.open();
        assert_eq!(palette.scope_title(), None);
        assert!(palette.scope().is_none());
    }

    #[test]
    fn a_scoped_palette_lists_its_rows_without_a_section_header() {
        let entries = [
            entry("a.txt", None, Some(Change::Modified)),
            entry("c.txt", None, Some(Change::Modified)),
        ];
        let ctx = ctx_with(&entries, "");
        let items = vec![
            (Action::StageFile, Args::File("a.txt".into())),
            (Action::StageFile, Args::File("c.txt".into())),
        ];
        let mut palette = Palette::default();
        palette.open_scoped("Actions for a.txt".into(), items.clone());
        shown(&mut palette, build_scoped(&items, &ctx));
        let headers = palette
            .lines()
            .iter()
            .filter(|line| matches!(line, Line::Header(_)))
            .count();
        assert_eq!(headers, 0);
        assert_eq!(shape(&palette), ["row 0", "row 1"]);
        assert_eq!(palette.highlight_line(), Some(0));
    }

    #[test]
    fn the_filter_placeholder_names_the_actions_only_when_scoped() {
        assert_eq!(
            filter_placeholder(Some("Actions for a.txt")),
            "Filter actions"
        );
        assert_eq!(filter_placeholder(None), "Type a command, branch or file");
    }
}
