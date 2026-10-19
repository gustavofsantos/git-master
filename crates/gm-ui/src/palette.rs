//! The command palette's model (slice 1c, item 4): what it offers for a query,
//! how the offers are ranked and where the highlight sits. The offers come from
//! the action registry and the branch listing. A choice names an `Action` and
//! its `Args`, and the window runs it through `run_action`, like a key or a menu
//! item. Nothing here touches GPUI. Ranking is the one step the window moves to
//! the background executor, through `Job` and `Ranked`.

use std::ops::Range;
use std::sync::Arc;

use gm_schema::BranchEntry;

use crate::action_registry::{
    Action, ArgKind, Args, Category, Context as ActionContext, NOT_AVAILABLE, Reason,
};
use crate::fuzzy;

const NO_BRANCH: Reason = "No branch to choose from.";
const NO_FILE: Reason = "No file in the working copy.";
const NEEDS_CHUNK: Reason = "Pick a chunk in the diff first.";
const NEEDS_COMMIT: Reason = "Pick a commit in the history first.";
const NEEDS_WORKTREE: Reason = "Run it from the worktree's menu in the sidebar.";

/// The groups the palette shows, in the order they appear.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Section {
    Actions,
    Worktrees,
    Branches,
    Files,
}

impl Section {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Actions => "Actions",
            Self::Worktrees => "Worktrees",
            Self::Branches => "Branches",
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

/// One line of the list: a section's header, or a row by its position in the hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    Header(Section),
    Row { hit: usize, highlighted: bool },
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
    /// Position among `hits` of the row Enter would choose.
    highlight: usize,
}

impl Palette {
    /// Opens the top level with an empty query.
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.stage = Stage::Actions;
        self.query.clear();
        self.highlight = 0;
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
        }
    }

    /// Opens the argument step of `action`. The query starts over: it was typed
    /// for the action, not for its argument.
    pub(crate) fn enter_pick(&mut self, action: Action) {
        self.stage = Stage::Pick(action);
        self.query.clear();
        self.highlight = 0;
    }

    /// Goes back from an argument step to the top level. Returns whether there
    /// was a step to leave.
    pub(crate) fn leave_pick(&mut self) -> bool {
        if matches!(self.stage, Stage::Pick(_)) {
            self.stage = Stage::Actions;
            self.query.clear();
            self.highlight = 0;
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
        self.highlight = self.highlight.min(self.hits.len().saturating_sub(1));
        true
    }

    /// The number of rows on screen.
    pub(crate) fn count(&self) -> usize {
        self.hits.len()
    }

    /// Up and down: moves the highlight and stops at both ends.
    pub(crate) fn move_highlight(&mut self, down: bool) {
        let count = self.hits.len();
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

    /// Makes the row at `position` the highlighted one.
    pub(crate) fn select(&mut self, position: usize) {
        if position < self.hits.len() {
            self.highlight = position;
        }
    }

    /// The candidate Enter would act on.
    pub(crate) fn highlighted(&self) -> Option<&Candidate> {
        let hit = self.hits.get(self.highlight)?;
        self.shown.get(hit.index)
    }

    /// The candidate and the hit of the row at `position`.
    pub(crate) fn row(&self, position: usize) -> Option<(&Candidate, &Hit)> {
        let hit = self.hits.get(position)?;
        Some((self.shown.get(hit.index)?, hit))
    }

    /// The list as drawn: a header before each section, then its rows.
    pub(crate) fn lines(&self) -> Vec<Line> {
        let mut lines = Vec::with_capacity(self.hits.len() + 3);
        let mut section = None;
        for (position, hit) in self.hits.iter().enumerate() {
            let Some(candidate) = self.shown.get(hit.index) else {
                continue;
            };
            if section != Some(candidate.section) {
                section = Some(candidate.section);
                lines.push(Line::Header(candidate.section));
            }
            lines.push(Line::Row {
                hit: position,
                highlighted: position == self.highlight,
            });
        }
        lines
    }

    /// The index in `lines()` of the highlighted row, for scrolling it into view.
    pub(crate) fn highlight_line(&self) -> Option<usize> {
        self.lines().iter().position(|line| {
            matches!(
                line,
                Line::Row {
                    highlighted: true,
                    ..
                }
            )
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

fn top_level(query: &str, ctx: &ActionContext<'_>, branches: &[BranchEntry]) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = Action::ALL
        .into_iter()
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

fn action_row(action: Action, ctx: &ActionContext<'_>, branches: &[BranchEntry]) -> Candidate {
    let spec = action.spec();
    let section = if spec.category == Category::Worktrees {
        Section::Worktrees
    } else {
        Section::Actions
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
        // A worktree is run from its own menu, which names it, so the palette lists it disabled.
        ArgKind::Worktree => (
            Choice::Run {
                action,
                args: Args::None,
            },
            Err(NEEDS_WORKTREE),
        ),
        // A chunk exists only in the diff pane, so the palette lists it disabled.
        ArgKind::Chunk => (
            Choice::Run {
                action,
                args: Args::None,
            },
            Err(NEEDS_CHUNK),
        ),
        // A commit is copied from the history panel, which names it, so the palette lists it disabled.
        ArgKind::Commit => (
            Choice::Run {
                action,
                args: Args::None,
            },
            Err(NEEDS_COMMIT),
        ),
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

fn pick(action: Action, ctx: &ActionContext<'_>, branches: &[BranchEntry]) -> Vec<Candidate> {
    match action.spec().args {
        ArgKind::Branch => branches
            .iter()
            .map(|branch| target_row(action, Section::Branches, &branch.name, ctx))
            .collect(),
        ArgKind::File => ctx
            .entries
            .iter()
            .map(|entry| target_row(action, Section::Files, &entry.path, ctx))
            .collect(),
        ArgKind::None
        | ArgKind::OptionalBranch
        | ArgKind::Chunk
        | ArgKind::Worktree
        | ArgKind::Commit => Vec::new(),
    }
}

fn target_row(action: Action, section: Section, name: &str, ctx: &ActionContext<'_>) -> Candidate {
    let args = match section {
        Section::Files => Args::File(name.to_owned()),
        _ => Args::Branch(name.to_owned()),
    };
    let state = action.enabled(&args, ctx);
    Candidate {
        section,
        label: name.to_owned(),
        shortcut: None,
        choice: Choice::Run { action, args },
        state,
    }
}

/// The hits for `query`, best first within each section, sections in their
/// order. Equal scores keep the order of the candidates.
#[must_use]
pub(crate) fn rank(query: &str, candidates: &[Candidate]) -> Vec<Hit> {
    let mut scored: Vec<(Section, i64, usize, Vec<Range<usize>>)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            fuzzy::fuzzy_match(query, &candidate.label)
                .map(|found| (candidate.section, found.score, index, found.ranges))
        })
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    scored
        .into_iter()
        .map(|(_, _, index, ranges)| Hit { index, ranges })
        .collect()
}

#[cfg(test)]
mod tests {
    use gm_schema::{Change, StatusEntry};

    use super::*;

    fn entry(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
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
            busy: None,
            entries,
            staged_count: entries.iter().filter(|e| e.staged.is_some()).count(),
            message,
            current_branch: Some("main"),
            root: Some("/repo"),
            hooks: crate::action_registry::HookCtx::default(),
            worktrees: &[],
            branch_scope: crate::branch_filter::Scope::All,
            user_email_known: false,
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
        assert_eq!(actions, Action::ALL.len() - worktree_actions);
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
            Err("Not available yet")
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
        assert_eq!(top.label, "Stage File");
    }

    #[test]
    fn an_empty_query_keeps_the_registry_order() {
        let entries: [StatusEntry; 0] = [];
        let candidates = build(Stage::Actions, "", &ctx_with(&entries, ""), &[]);
        let hits = rank("", &candidates);
        assert_eq!(hits.len(), candidates.len());
        assert_eq!(candidates[hits[0].index].label, "Stage File");
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
                Line::Header(Section::Worktrees)
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
        shown(&mut palette, candidates);
        assert_eq!(palette.count(), count);
        assert_eq!(
            palette.highlighted().map(|c| c.label.as_str()),
            Some("Stage File")
        );
        palette.move_highlight(false);
        assert_eq!(palette.highlight, 0);
        for _ in 0..count + 2 {
            palette.move_highlight(true);
        }
        assert_eq!(palette.highlight, count - 1);
        palette.select(0);
        assert_eq!(palette.highlight, 0);
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
        assert_eq!(
            palette.highlighted().map(|c| c.label.as_str()),
            Some("Commit")
        );
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
}
