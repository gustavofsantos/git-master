//! Moving a file's change to another branch or worktree (D109). A changed file
//! is dragged onto a sidebar row, or moved from the palette or the file menu,
//! and the move ends here. The answers the rows give, the check Git makes
//! while the pointer rests on a row, the commit popover and the status notice
//! are pure and tested here; the window side is the `impl StatusView` at the
//! bottom. The engine decides everything that touches Git: these rules only
//! decide what the human sees before the engine is asked.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gm_core::branch;
use gm_core::move_changes::{commit_message, describe};
use gm_schema::{
    Actor, CommandKind, MoveBlocker, MoveOutcome, MovePreview, MoveTarget, OperationId,
    OperationState, WorktreeEntry,
};
use gpui::{
    App, Context, Div, DragMoveEvent, Entity, FontWeight, InteractiveElement, KeyDownEvent,
    MouseButton, MouseDownEvent, Render, Stateful, StatefulInteractiveElement, StyleRefinement,
    Styled, Subscription, Window, div, prelude::*, px,
};

use crate::action_registry::{Action, Args, Reason};
use crate::text_area::{TextArea, TextAreaEvent};
use crate::theme;
use crate::view::StatusView;
use crate::worktree_create::{Mode, field_row};
use crate::worktree_list;
use crate::{FocusNext, FocusPrev};

/// How long the pointer must rest on a row before Git is asked about it.
pub(crate) const HOVER_DELAY: Duration = Duration::from_millis(150);
/// The popover's line about hooks: a move makes its commit without running them.
pub(crate) const HOOKS_NOTE: &str = "Git hooks do not run for this commit.";
/// The reason a row refuses the file that is already on the branch of the window.
pub(crate) const SAME_BRANCH: &str = "This is the branch you are on";
/// The reason a row refuses the file that is already in the worktree of the window.
pub(crate) const SAME_WORKTREE: &str = "This is the worktree you are in";
/// The reason a folder is refused: the engine's sentence for `MoveBlocker::IsFolder`.
pub(crate) const FOLDER_REASON: &str =
    "a folder cannot be moved as one; move its files one at a time";
/// The reason a worktree whose folder is gone is refused: `MoveBlocker::WorktreeMissing`.
pub(crate) const MISSING_FOLDER: &str = "the worktree folder is missing";
/// The reason every target refuses while Git is older than the feature's floor.
const GIT_TOO_OLD: &str = "Requires Git 2.38 or newer";
/// The popover's error when a new branch has no name.
const NO_NAME: &str = "Name the new branch.";

/// What a sidebar row stands for, as a drop target.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DropTarget {
    /// A local branch row of the Branches section.
    Branch { name: String },
    /// A linked worktree row of the Worktrees section, by its folder.
    Worktree { path: String },
    /// The Branches header or a prefix group: a new branch, named after `prefix`
    /// (with its trailing `/`) when there is one.
    NewBranchGroup { prefix: Option<String> },
    /// The Worktrees header or its `+`: a new worktree with a new branch.
    NewWorktree,
}

impl DropTarget {
    /// The engine's target for an existing branch or worktree row. A creation
    /// has none yet: its branch is named by the popover or the dialog.
    #[must_use]
    pub(crate) fn move_target(&self) -> Option<MoveTarget> {
        match self {
            Self::Branch { name } => Some(MoveTarget::Branch {
                name: name.clone(),
                create: false,
            }),
            Self::Worktree { path } => Some(MoveTarget::Worktree { path: path.clone() }),
            Self::NewBranchGroup { .. } | Self::NewWorktree => None,
        }
    }

    /// Whether Git is asked about this row before a drop: only the rows that exist.
    fn is_asked(&self) -> bool {
        matches!(self, Self::Branch { .. } | Self::Worktree { .. })
    }
}

/// The file being dragged: its path as the Working Copy lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DragSource {
    pub(crate) path: String,
}

/// What a row says about the file under the pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DropAnswer {
    /// The row is highlighted; the hint says what the drop does.
    Accepts { hint: String },
    /// The row is dimmed; the reason is shown in the hint.
    Rejects { reason: String },
    /// Git has not answered the check for this row yet.
    Checking,
}

/// What the window knows without asking Git. Built by the window for each answer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DropContext<'a> {
    /// The checked-out branch; `None` when HEAD is detached or unborn.
    pub(crate) current_branch: Option<&'a str>,
    /// The folder of the worktree this window shows.
    pub(crate) own_worktree: Option<&'a str>,
    /// Whether the installed Git meets the floor, or nothing says otherwise yet.
    pub(crate) git_ok: bool,
    /// Why the repository is busy now, as the other actions give it; `None` when idle.
    pub(crate) busy: Option<Reason>,
    /// The worktrees listed for the repository, main checkout first.
    pub(crate) worktrees: &'a [WorktreeEntry],
}

/// The answer of one row for `source`. The rules that need no Git come first,
/// then the preview the check made, when one exists. A preview with blockers
/// refuses with the first blocker's sentence.
#[must_use]
pub(crate) fn answer(
    source: &DragSource,
    target: &DropTarget,
    ctx: &DropContext<'_>,
    preview: Option<&MovePreview>,
) -> DropAnswer {
    let refuse = |reason: String| DropAnswer::Rejects { reason };
    match target {
        DropTarget::Branch { name } if ctx.current_branch == Some(name.as_str()) => {
            return refuse(SAME_BRANCH.to_owned());
        }
        DropTarget::Worktree { path }
            if ctx.own_worktree.is_some_and(|own| same_folder(own, path)) =>
        {
            return refuse(SAME_WORKTREE.to_owned());
        }
        _ => {}
    }
    if is_folder(&source.path) {
        return refuse(FOLDER_REASON.to_owned());
    }
    if let Some(busy) = ctx.busy {
        return refuse(busy.to_owned());
    }
    if !ctx.git_ok {
        return refuse(GIT_TOO_OLD.to_owned());
    }
    match target {
        DropTarget::NewBranchGroup { .. } => DropAnswer::Accepts {
            hint: format!("Create a branch for {}", source.path),
        },
        DropTarget::NewWorktree => DropAnswer::Accepts {
            hint: format!("Create a worktree for {}", source.path),
        },
        DropTarget::Branch { name } => {
            if let Some(entry) = ctx
                .worktrees
                .iter()
                .find(|entry| entry.branch.as_deref() == Some(name.as_str()))
            {
                return refuse(describe(&MoveBlocker::BranchCheckedOutElsewhere {
                    path: entry.path.clone(),
                }));
            }
            checked(preview, || DropAnswer::Accepts {
                hint: format!("Commit to {name}"),
            })
        }
        DropTarget::Worktree { path } => {
            if ctx
                .worktrees
                .iter()
                .any(|entry| same_folder(&entry.path, path) && entry.prunable.is_some())
            {
                return refuse(MISSING_FOLDER.to_owned());
            }
            checked(preview, || DropAnswer::Accepts {
                hint: format!("Move to {}", worktree_name(path, ctx.worktrees)),
            })
        }
    }
}

/// The answer once Git has checked the move: its first blocker, or `accepts`
/// when the check found none. Without a check yet, the row is still checking.
fn checked(preview: Option<&MovePreview>, accepts: impl FnOnce() -> DropAnswer) -> DropAnswer {
    match preview {
        None => DropAnswer::Checking,
        Some(preview) => match preview.blockers.first() {
            Some(first) => DropAnswer::Rejects {
                reason: describe(first),
            },
            None => accepts(),
        },
    }
}

/// The name a worktree row shows: the same one the sidebar uses. A folder the
/// list does not have is named by its last path component.
#[must_use]
pub(crate) fn worktree_name(path: &str, worktrees: &[WorktreeEntry]) -> String {
    worktrees
        .iter()
        .find(|entry| same_folder(&entry.path, path))
        .map_or_else(|| file_name(path).to_owned(), worktree_list::display_name)
}

/// A path that names a folder of the Working Copy, as an untracked folder does.
fn is_folder(path: &str) -> bool {
    path.ends_with('/')
}

/// Paths compare without a trailing separator.
fn same_folder(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// The last component of a path, without its folder.
fn file_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

/// What the status bar says when a move ends, and the worktree it can open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Notice {
    pub(crate) text: String,
    /// The folder of the worktree the change went to, for **Open Worktree**.
    pub(crate) open_worktree: Option<String>,
}

/// The status bar line for a finished move. `worktree` is the name the sidebar
/// shows for a worktree target; the folder is what **Open Worktree** opens.
#[must_use]
pub(crate) fn moved_notice(
    outcome: &MoveOutcome,
    target: &DropTarget,
    file: &str,
    worktree: &str,
) -> Notice {
    let (sentence, open_worktree) = match target {
        DropTarget::Branch { name } => (format!("Committed {file} to {name}"), None),
        DropTarget::Worktree { path } => {
            (format!("Moved {file} to {worktree}"), Some(path.clone()))
        }
        DropTarget::NewBranchGroup { .. } => (format!("Committed {file}"), None),
        DropTarget::NewWorktree => (format!("Moved {file} to a new worktree"), None),
    };
    let text = match &outcome.kept {
        Some(reason) => format!(
            "{sentence}. The file stays in the Working Copy: {}.",
            reason.trim_end_matches('.')
        ),
        None => format!("{sentence}."),
    };
    Notice {
        text,
        open_worktree,
    }
}

/// The period of the drag's poll. It runs only while a drag runs.
pub(crate) const POLL: Duration = Duration::from_millis(50);

/// A sidebar row that takes dragged files: its target and what it answers for the drag now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DropZone {
    pub(crate) target: DropTarget,
    /// `None` when no drag runs: the row is then plain, but it still hears the drag.
    pub(crate) answer: Option<DropAnswer>,
}

/// How a row is styled while a file is dragged over it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DragStyle {
    /// The row takes the file: highlighted.
    Accept,
    /// The row refuses the file: dimmed, with the not-allowed pointer.
    Reject,
    /// The row is still checking: a subtle pending look.
    Checking,
}

/// The style of a row for `answer`. Without an answer, no drag runs and the row is plain.
#[must_use]
pub(crate) fn drag_style(answer: Option<&DropAnswer>) -> Option<DragStyle> {
    match answer? {
        DropAnswer::Accepts { .. } => Some(DragStyle::Accept),
        DropAnswer::Rejects { .. } => Some(DragStyle::Reject),
        DropAnswer::Checking => Some(DragStyle::Checking),
    }
}

/// Whether a drop on a row with `answer` is taken: an accepting row, or one still checking.
#[must_use]
pub(crate) fn takes_drop(answer: Option<&DropAnswer>) -> bool {
    matches!(
        answer,
        Some(DropAnswer::Accepts { .. } | DropAnswer::Checking)
    )
}

/// The status bar line while a file is dragged: the hint of the row under the
/// pointer, or what to do when no row is under it. `None` when no drag runs.
#[must_use]
pub(crate) fn drag_status(hovered: Option<&DropAnswer>, dragging: bool) -> Option<String> {
    if !dragging {
        return None;
    }
    Some(match hovered {
        Some(DropAnswer::Accepts { hint }) => hint.clone(),
        Some(DropAnswer::Rejects { reason }) => reason.clone(),
        Some(DropAnswer::Checking) => "Checking…".to_owned(),
        None => "Drop the file on a branch or a worktree to move it.".to_owned(),
    })
}

/// The popover's title: the commit it makes, or the branch it creates.
#[must_use]
pub(crate) fn popover_title(popover: &PopoverState) -> String {
    match popover {
        PopoverState::Existing { path, branch, .. } => {
            format!("Commit {} to {branch}", file_name(path))
        }
        PopoverState::New { .. } => "New branch".to_owned(),
    }
}

/// What one poll of a drag does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PollStep {
    /// The drag runs: ask Git about the row under the pointer, then poll again.
    Ask,
    /// gpui has ended the drag (dropped outside, released, or cancelled): end ours.
    End,
    /// This poll belongs to a drag that is over or superseded: stop, touch nothing.
    Stop,
}

/// The step of a poll started for drag `loop_generation`, given the drag now
/// (`current`, `dragging`) and whether gpui still carries a drag (`gpui_dragging`).
#[must_use]
pub(crate) fn poll_step(
    loop_generation: u64,
    current: u64,
    dragging: bool,
    gpui_dragging: bool,
) -> PollStep {
    if loop_generation != current || !dragging {
        PollStep::Stop
    } else if gpui_dragging {
        PollStep::Ask
    } else {
        PollStep::End
    }
}

/// What the pointer does on a row during a drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PointerStep {
    /// The pointer came onto the row.
    Enter,
    /// The pointer left the row, which was the one hovered.
    Leave,
    /// Nothing changes for this row.
    Stay,
}

/// The step for `row` after a drag move: `inside` says whether the pointer is over
/// it, and `hovered` is the row the window last entered.
#[must_use]
pub(crate) fn pointer_step(
    hovered: Option<&DropTarget>,
    row: &DropTarget,
    inside: bool,
) -> PointerStep {
    match (inside, hovered == Some(row)) {
        (true, false) => PointerStep::Enter,
        (false, true) => PointerStep::Leave,
        _ => PointerStep::Stay,
    }
}

/// The popover's default message: the file name, as the commit's subject.
fn default_message(path: &str) -> String {
    format!("Move {}", file_name(path))
}

/// The branch popover, opened by a drop on a branch row or on a branch group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PopoverState {
    /// An existing local branch: the commit message is the only field.
    Existing {
        path: String,
        branch: String,
        message: String,
    },
    /// A new branch: its name, which a prefix group fills in, and the message.
    New {
        path: String,
        name: String,
        message: String,
    },
}

impl PopoverState {
    /// The popover of a drop on the branch row `branch`.
    #[must_use]
    pub(crate) fn existing(path: &str, branch: &str) -> Self {
        Self::Existing {
            path: path.to_owned(),
            branch: branch.to_owned(),
            message: default_message(path),
        }
    }

    /// The popover of a drop on a branch group, or on the Branches header with
    /// no prefix. The name starts with the group's prefix and ends with its `/`.
    #[must_use]
    pub(crate) fn new(path: &str, prefix: Option<&str>) -> Self {
        let name = match prefix {
            Some(prefix) if !prefix.is_empty() && !prefix.ends_with('/') => format!("{prefix}/"),
            Some(prefix) => prefix.to_owned(),
            None => String::new(),
        };
        Self::New {
            path: path.to_owned(),
            name,
            message: default_message(path),
        }
    }

    /// The commit message as typed.
    #[must_use]
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Existing { message, .. } | Self::New { message, .. } => message,
        }
    }

    /// Replaces the commit message with the text the field holds.
    pub(crate) fn set_message(&mut self, text: String) {
        match self {
            Self::Existing { message, .. } | Self::New { message, .. } => *message = text,
        }
    }

    /// Replaces the new branch's name with the text the field holds. An
    /// existing branch has no name field, so this does nothing for it.
    pub(crate) fn set_name(&mut self, text: String) {
        if let Self::New { name, .. } = self {
            *name = text;
        }
    }

    /// The move the popover asks for, when its fields are valid. `existing` is
    /// the branch names the window lists, which a new name must not reuse.
    pub(crate) fn validate(&self, existing: &[String]) -> Result<MoveRequest, String> {
        match self {
            Self::Existing {
                path,
                branch,
                message,
            } => {
                if !existing.contains(branch) {
                    return Err(describe(&MoveBlocker::BranchNotFound));
                }
                Ok(MoveRequest {
                    path: path.clone(),
                    target: MoveTarget::Branch {
                        name: branch.clone(),
                        create: false,
                    },
                    message: commit_message(Some(message))?,
                })
            }
            Self::New {
                path,
                name,
                message,
            } => {
                let name = name.trim();
                if name.is_empty() {
                    return Err(NO_NAME.to_owned());
                }
                if let Some(problem) = branch::name_problem(name) {
                    return Err(problem.to_owned());
                }
                if existing.iter().any(|branch| branch == name) {
                    return Err(describe(&MoveBlocker::BranchExists));
                }
                Ok(MoveRequest {
                    path: path.clone(),
                    target: MoveTarget::Branch {
                        name: name.to_owned(),
                        create: true,
                    },
                    message: commit_message(Some(message))?,
                })
            }
        }
    }
}

/// A validated move from the popover: the file, the target and the trimmed message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MoveRequest {
    pub(crate) path: String,
    pub(crate) target: MoveTarget,
    pub(crate) message: String,
}

impl MoveRequest {}

/// The hover check: the row under the pointer, the preview asked for it and
/// the answers that came back. Time is passed in, so the timing is tested.
/// Checks and answers belong to one drag; a late answer is dropped by its
/// operation id, because the row it was asked for is no longer the one under
/// the pointer.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct HoverCheck {
    /// The file being dragged; every check and answer is for this path.
    path: Option<String>,
    /// The row under the pointer, and when the pointer arrived on it.
    hovering: Option<(DropTarget, Instant)>,
    /// The preview in flight, by its operation id and the row it is for.
    asked: Option<(OperationId, DropTarget)>,
    /// The row whose check failed; it is not asked again until the pointer comes back.
    failed: Option<DropTarget>,
    /// The previews this drag has had, by path and row. Cleared when the status changes.
    answers: HashMap<(String, DropTarget), MovePreview>,
}

impl HoverCheck {
    /// A drag of `path` began: its checks start from nothing.
    pub(crate) fn start(&mut self, path: &str) {
        *self = Self {
            path: Some(path.to_owned()),
            ..Self::default()
        };
    }

    /// The drag ended: nothing is checked any more.
    pub(crate) fn stop(&mut self) {
        *self = Self::default();
    }

    /// The pointer is over `target` at `now`. Entering another row supersedes
    /// the check in flight: its answer will be dropped.
    pub(crate) fn enter(&mut self, target: DropTarget, now: Instant) {
        if self
            .hovering
            .as_ref()
            .is_some_and(|(current, _)| *current == target)
        {
            return;
        }
        self.asked = None;
        self.failed = None;
        self.hovering = Some((target, now));
    }

    /// The pointer left the rows. The check in flight is superseded too.
    pub(crate) fn leave(&mut self) {
        self.hovering = None;
        self.asked = None;
        self.failed = None;
    }

    /// The row to ask Git about at `now`: one the pointer has rested on for
    /// `HOVER_DELAY`, with no check of its own yet. `None` otherwise.
    #[must_use]
    pub(crate) fn due(&self, now: Instant) -> Option<DropTarget> {
        let (target, since) = self.hovering.as_ref()?;
        if !target.is_asked() || now.saturating_duration_since(*since) < HOVER_DELAY {
            return None;
        }
        let in_flight = self
            .asked
            .as_ref()
            .is_some_and(|(_, asked)| asked == target);
        let failed = self.failed.as_ref() == Some(target);
        if in_flight || failed || self.preview(target).is_some() {
            return None;
        }
        Some(target.clone())
    }

    /// The preview request for the row under the pointer was sent as `id`.
    pub(crate) fn sent(&mut self, id: OperationId) {
        if let Some((target, _)) = &self.hovering {
            self.asked = Some((id, target.clone()));
        }
    }

    /// The preview `id` answered. True when it was the one in flight, so it is
    /// kept; a late or superseded answer returns false and changes nothing.
    pub(crate) fn answered(&mut self, id: OperationId, preview: MovePreview) -> bool {
        let Some(target) = take_asked(&mut self.asked, id) else {
            return false;
        };
        if let Some(path) = &self.path {
            self.answers.insert((path.clone(), target), preview);
        }
        true
    }

    /// Whether the preview `id` is the one in flight.
    #[must_use]
    pub(crate) fn awaits(&self, id: OperationId) -> bool {
        self.asked.as_ref().is_some_and(|(asked, _)| *asked == id)
    }

    /// The operation `id` ended without an answer. True when it was the one in
    /// flight; that row is then not asked again until the pointer comes back.
    pub(crate) fn ended(&mut self, id: OperationId, state: OperationState) -> bool {
        if !state.is_terminal() {
            return false;
        }
        let Some(target) = take_asked(&mut self.asked, id) else {
            return false;
        };
        if state != OperationState::Done {
            self.failed = Some(target);
        }
        true
    }

    /// The row under the pointer, while a drag runs.
    #[must_use]
    pub(crate) fn hovered(&self) -> Option<&DropTarget> {
        self.hovering.as_ref().map(|(target, _)| target)
    }

    /// The preview of `target` for this drag, once Git has answered it.
    #[must_use]
    pub(crate) fn preview(&self, target: &DropTarget) -> Option<&MovePreview> {
        let path = self.path.as_ref()?;
        self.answers.get(&(path.clone(), target.clone()))
    }

    /// The status changed, so every answer may be stale: they are asked again.
    pub(crate) fn status_changed(&mut self) {
        self.answers.clear();
    }

    /// The file being dragged, while a drag runs.
    #[must_use]
    pub(crate) fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

/// Takes the row of the preview in flight when it is the one `id` answers. A
/// different id leaves the slot as it is.
fn take_asked(slot: &mut Option<(OperationId, DropTarget)>, id: OperationId) -> Option<DropTarget> {
    if slot.as_ref().is_some_and(|(asked, _)| *asked == id) {
        slot.take().map(|(_, target)| target)
    } else {
        None
    }
}

/// A move sent to the engine, and what the window needs to show when it ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Moving {
    pub(crate) id: OperationId,
    /// The file being moved.
    pub(crate) file: String,
    /// The target as it was made: a created branch or the picked worktree.
    pub(crate) target: DropTarget,
    /// The worktree's name for the notice; empty for a branch.
    pub(crate) worktree: String,
    /// The popover that sent the move: it closes on success and stays open on failure.
    pub(crate) from_popover: bool,
}

/// The move state the window keeps: the drag, the hover check, the popover and the
/// move in flight. Pure; the window's methods below drive it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MoveFile {
    /// The file being dragged, while a drag runs.
    pub(crate) drag: Option<DragSource>,
    pub(crate) hover: HoverCheck,
    /// The branch popover, while it is open, with its target.
    pub(crate) popover: Option<PopoverState>,
    /// Why the popover's last Enter was refused, or why its move failed.
    pub(crate) popover_error: Option<String>,
    /// The move sent and not yet ended.
    pub(crate) moving: Option<Moving>,
    /// The file to move into the worktree a New Worktree dialog creates, once it exists.
    pub(crate) after_create: Option<String>,
    /// Set once a preview reported that the installed Git is too old.
    pub(crate) git_too_old: bool,
    /// Counts the drags begun: a poll of an older drag stops.
    pub(crate) generation: u64,
}

impl MoveFile {
    /// Whether the move in flight is `id`'s.
    #[must_use]
    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.moving.as_ref().is_some_and(|moving| moving.id == id)
    }

    /// Whether the installed Git meets the floor, as far as the window knows.
    #[must_use]
    pub(crate) fn git_ok(&self) -> bool {
        !self.git_too_old
    }

    /// A preview answered. The Git floor is noted from its blockers.
    pub(crate) fn answered(&mut self, id: OperationId, preview: MovePreview) -> bool {
        if preview
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, MoveBlocker::GitTooOld { .. }))
        {
            self.git_too_old = true;
        }
        self.hover.answered(id, preview)
    }

    /// A drag of `path` began. Returns its generation, which its poll checks.
    pub(crate) fn begin_drag(&mut self, path: &str) -> u64 {
        self.generation += 1;
        self.drag = Some(DragSource {
            path: path.to_owned(),
        });
        self.hover.start(path);
        self.generation
    }

    /// The drag ended. The popover, if one opened, keeps its own file.
    pub(crate) fn end_drag(&mut self) {
        self.drag = None;
        self.hover.stop();
    }

    /// A move was sent: the window waits for its answer.
    pub(crate) fn sent_move(&mut self, moving: Moving) {
        self.moving = Some(moving);
    }

    /// The move `id` made its change. Returns the notice for the status bar and
    /// closes the popover that sent it. `None` when `id` is not the move in flight.
    pub(crate) fn moved(&mut self, id: OperationId, outcome: &MoveOutcome) -> Option<Notice> {
        if !self.owns(id) {
            return None;
        }
        let moving = self.moving.take()?;
        if moving.from_popover {
            self.popover = None;
            self.popover_error = None;
        }
        Some(moved_notice(
            outcome,
            &moving.target,
            &moving.file,
            &moving.worktree,
        ))
    }

    /// The operation `id` ended. A failed move stays in the popover when the
    /// popover sent it; otherwise its message is returned for the status bar.
    pub(crate) fn settle(
        &mut self,
        id: OperationId,
        state: OperationState,
        detail: Option<String>,
    ) -> Option<String> {
        if !self.owns(id) || !state.is_terminal() {
            return None;
        }
        let moving = self.moving.take()?;
        if state == OperationState::Done {
            return None;
        }
        let message = detail.unwrap_or_else(|| "The change was not moved.".to_owned());
        if moving.from_popover {
            self.popover_error = Some(message);
            self.moving = None;
            None
        } else {
            Some(message)
        }
    }

    /// Opens the popover for a drop, with no error.
    pub(crate) fn open_popover(&mut self, popover: PopoverState) {
        self.popover = Some(popover);
        self.popover_error = None;
    }

    /// Closes the popover and forgets its error.
    pub(crate) fn close_popover(&mut self) {
        self.popover = None;
        self.popover_error = None;
    }
}

impl StatusView {
    /// A changed file was picked up on its row of the Working Copy list. A poll
    /// starts with the drag: it asks Git about the row under the pointer, and it
    /// ends the drag once gpui no longer carries it.
    pub(crate) fn begin_drag(&mut self, path: &str, cx: &mut Context<Self>) {
        let generation = self.model.move_file.begin_drag(path);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let kept = this
                    .update(cx, |view, cx| view.poll_drag(generation, cx))
                    .unwrap_or(false);
                if !kept {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// One poll of the drag `generation`. True when the poll goes on.
    fn poll_drag(&mut self, generation: u64, cx: &mut Context<Self>) -> bool {
        let step = poll_step(
            generation,
            self.model.move_file.generation,
            self.model.move_file.drag.is_some(),
            cx.has_active_drag(),
        );
        match step {
            PollStep::Ask => {
                self.request_due_preview(Instant::now(), cx);
                true
            }
            PollStep::End => {
                self.end_drag(cx);
                false
            }
            PollStep::Stop => false,
        }
    }

    /// The drag ended, dropped or not.
    pub(crate) fn end_drag(&mut self, cx: &mut Context<Self>) {
        self.model.move_file.end_drag();
        cx.notify();
    }

    /// The pointer is over a sidebar row. The check for it runs once the pointer has rested there.
    pub(crate) fn hover_row(&mut self, target: DropTarget, now: Instant, cx: &mut Context<Self>) {
        self.model.move_file.hover.enter(target, now);
        cx.notify();
    }

    /// The pointer left the sidebar rows.
    pub(crate) fn leave_row(&mut self, cx: &mut Context<Self>) {
        self.model.move_file.hover.leave();
        cx.notify();
    }

    /// Asks Git about the row under the pointer, when it has rested there long
    /// enough. The window calls this on a timer while a drag runs.
    pub(crate) fn request_due_preview(&mut self, now: Instant, cx: &mut Context<Self>) {
        let Some(path) = self.model.move_file.hover.path().map(str::to_owned) else {
            return;
        };
        let Some(target) = self.model.move_file.hover.due(now) else {
            return;
        };
        let Some(move_target) = target.move_target() else {
            return;
        };
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::PreviewMoveChanges {
                path,
                target: move_target,
            },
        );
        self.model.move_file.hover.sent(id);
        cx.notify();
    }

    /// What `target` says about the drag now. `None` when no drag runs.
    #[must_use]
    pub(crate) fn drop_answer(&self, target: &DropTarget) -> Option<DropAnswer> {
        let drag = self.model.move_file.drag.as_ref()?;
        let ctx = self.drop_context();
        Some(answer(
            drag,
            target,
            &ctx,
            self.model.move_file.hover.preview(target),
        ))
    }

    /// Drops the dragged file on `target`. A refused drop says why; the rest run
    /// as the design says: a worktree moves at once, a branch or a branch group
    /// opens the popover, a new worktree opens the create dialog in new-branch mode.
    pub(crate) fn drop_on(
        &mut self,
        target: DropTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.model.move_file.drag.clone() else {
            return;
        };
        let verdict = answer(
            &drag,
            &target,
            &self.drop_context(),
            self.model.move_file.hover.preview(&target),
        );
        if let DropAnswer::Rejects { reason } = verdict {
            self.notify_user(&reason, cx);
            return;
        }
        match target {
            DropTarget::Worktree { path } => {
                let request = MoveRequest {
                    path: drag.path,
                    target: MoveTarget::Worktree { path },
                    message: String::new(),
                };
                self.send_move(request, false, cx);
            }
            DropTarget::Branch { name } => {
                self.model
                    .move_file
                    .open_popover(PopoverState::existing(&drag.path, &name));
                self.show_move_popover(window, cx);
            }
            DropTarget::NewBranchGroup { prefix } => {
                self.model
                    .move_file
                    .open_popover(PopoverState::new(&drag.path, prefix.as_deref()));
                self.show_move_popover(window, cx);
            }
            DropTarget::NewWorktree => {
                self.model.move_file.after_create = Some(drag.path);
                self.open_worktree_dialog_in(Mode::New, None, window, cx);
            }
        }
        cx.notify();
    }

    /// Runs a move the engine's worktree target asked for, from the palette or
    /// the file menu: the same drop as a drag, with the file as the drag source.
    pub(crate) fn move_file_to(
        &mut self,
        path: &str,
        target: DropTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin_drag(path, cx);
        self.drop_on(target, window, cx);
        self.end_drag(cx);
    }

    /// Opens the palette on the target step of a move action, with the file
    /// selected, so Tab picks the branch or the worktree.
    pub(crate) fn pick_move_target(
        &mut self,
        action: Action,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self
            .model
            .entries()
            .iter()
            .position(|entry| entry.path == path)
        {
            self.model.select_row(row);
        }
        self.open_palette_at_pick(action, window, cx);
    }

    /// Enter on the popover: reads its fields, checks them, then sends the move.
    /// A refused check stays in the popover with its reason.
    pub(crate) fn submit_move_popover(
        &mut self,
        typed_name: Option<String>,
        typed_message: String,
        cx: &mut Context<Self>,
    ) {
        if self.model.move_file.moving.is_some() {
            return;
        }
        let Some(mut popover) = self.model.move_file.popover.clone() else {
            return;
        };
        popover.set_message(typed_message);
        if let Some(name) = typed_name {
            popover.set_name(name);
        }
        self.model.move_file.popover = Some(popover.clone());
        let existing: Vec<String> = self
            .model
            .branches
            .known()
            .iter()
            .map(|branch| branch.name.clone())
            .collect();
        match popover.validate(&existing) {
            Err(reason) => self.model.move_file.popover_error = Some(reason),
            Ok(request) => {
                self.model.move_file.popover_error = None;
                self.send_move(request, true, cx);
            }
        }
        cx.notify();
    }

    /// Escape or Cancel on the popover.
    pub(crate) fn close_move_popover(&mut self, cx: &mut Context<Self>) {
        self.model.move_file.close_popover();
        cx.notify();
    }

    /// A New Worktree dialog created the worktree at `path`: the file waiting for
    /// it moves there, at once, like a drop on a worktree row.
    pub(crate) fn move_after_create(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(file) = self.model.move_file.after_create.take() else {
            return;
        };
        let request = MoveRequest {
            path: file,
            target: MoveTarget::Worktree {
                path: path.to_owned(),
            },
            message: String::new(),
        };
        self.send_move(request, false, cx);
    }

    /// Sends a move to the engine and remembers it for its notice. A worktree
    /// move carries no message; a branch move carries the popover's.
    fn send_move(&mut self, request: MoveRequest, from_popover: bool, cx: &mut Context<Self>) {
        // One move at a time: the answer of the one in flight is still to come.
        if self.model.move_file.moving.is_some() {
            self.notify_user("Wait for the move to finish.", cx);
            return;
        }
        let (shown, worktree) = match &request.target {
            MoveTarget::Branch { name, .. } => {
                (DropTarget::Branch { name: name.clone() }, String::new())
            }
            MoveTarget::Worktree { path } => (
                DropTarget::Worktree { path: path.clone() },
                worktree_name(path, &self.model.worktrees),
            ),
        };
        let message = match &request.target {
            MoveTarget::Branch { .. } => Some(request.message.clone()),
            MoveTarget::Worktree { .. } => None,
        };
        let file = request.path.clone();
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::MoveChanges {
                path: request.path,
                target: request.target,
                message,
            },
        );
        self.model.move_file.sent_move(Moving {
            id,
            file,
            target: shown,
            worktree,
            from_popover,
        });
        cx.notify();
    }

    /// The facts the answers read, from the window's status and worktrees.
    fn drop_context(&self) -> DropContext<'_> {
        let status = self.model.status();
        DropContext {
            current_branch: status.and_then(|status| status.branch.head.as_deref()),
            own_worktree: status.map(|status| status.root.as_str()),
            git_ok: self.model.move_file.git_ok(),
            busy: self.busy_reason(),
            worktrees: &self.model.worktrees,
        }
    }
}

/// The payload of a drag from a changed file's row of the Working Copy list.
pub(crate) struct DraggedFile {
    pub(crate) path: String,
}

/// What a drag shows under the pointer: the file's name.
struct DragGhost {
    label: String,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .h(px(22.))
            .flex()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme::hairline_strong())
            .bg(theme::panel())
            .text_color(theme::text())
            .font_family(theme::code_font())
            .text_sm()
            .child(self.label.clone())
    }
}

/// Makes a changed file's row draggable. gpui starts a drag only once the pointer
/// has moved past its threshold, so a click and a double click keep working.
pub(crate) fn drag_source(
    row: Stateful<Div>,
    path: &str,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let view = cx.entity().downgrade();
    row.on_drag(
        DraggedFile {
            path: path.to_owned(),
        },
        move |dragged: &DraggedFile, _, _, cx: &mut App| {
            let path = dragged.path.clone();
            // The window learns of the drag as it starts: its state and poll follow.
            let _ = view.update(cx, |this, cx| this.begin_drag(&path, cx));
            let label = file_name(&path).to_owned();
            cx.new(|_| DragGhost { label })
        },
    )
}

/// The style a drag over a row adds, given the row's style for the answer.
fn dragged_over(style: StyleRefinement, drag: Option<DragStyle>) -> StyleRefinement {
    match drag {
        Some(DragStyle::Accept) => style
            .bg(theme::selection())
            .border_1()
            .border_color(theme::accent()),
        Some(DragStyle::Reject) => style.opacity(0.4).cursor_not_allowed(),
        Some(DragStyle::Checking) => style.opacity(0.7),
        None => style,
    }
}

/// Makes a sidebar row take dropped files, styled by what it answers for the drag.
/// A row without a zone takes nothing. A drop runs `drop_on` and ends the drag;
/// a drag over the row keeps the window's hover in step with the pointer.
pub(crate) fn accept_drops<E: InteractiveElement>(
    element: E,
    zone: Option<DropZone>,
    cx: &Context<StatusView>,
) -> E {
    let Some(zone) = zone else {
        return element;
    };
    let drag = drag_style(zone.answer.as_ref());
    let takes = takes_drop(zone.answer.as_ref());
    let dropped = zone.target.clone();
    let moved_over = zone.target;
    element
        .drag_over::<DraggedFile>(move |style, _, _, _| dragged_over(style, drag))
        // One predicate serves every payload: a dragged branch is judged by its own handler.
        .can_drop(move |dragged, _, _| takes || !dragged.is::<DraggedFile>())
        .on_drop(cx.listener(move |this, _: &DraggedFile, window, cx| {
            this.drop_on(dropped.clone(), window, cx);
            this.end_drag(cx);
        }))
        .on_drag_move(
            cx.listener(move |this, event: &DragMoveEvent<DraggedFile>, _, cx| {
                let inside = event.bounds.contains(&event.event.position);
                match pointer_step(this.model.move_file.hover.hovered(), &moved_over, inside) {
                    PointerStep::Enter => this.hover_row(moved_over.clone(), Instant::now(), cx),
                    PointerStep::Leave => this.leave_row(cx),
                    PointerStep::Stay => {}
                }
            }),
        )
}

/// The popover's inputs while it is open. The text lives in the inputs until the
/// person submits, and then it is read into the state.
pub(crate) struct MovePopover {
    name: Option<Entity<TextArea>>,
    message: Entity<TextArea>,
    _subscriptions: Vec<Subscription>,
}

/// One single-line input with its text. Esc in it closes the popover.
fn popover_input(
    placeholder: &'static str,
    text: &str,
    window: &mut Window,
    cx: &mut Context<StatusView>,
) -> (Entity<TextArea>, Subscription) {
    let area = cx.new(|cx| TextArea::new(placeholder, cx).with_lines(1, 1));
    area.update(cx, |area, cx| area.set_text(text, cx));
    let subscription = cx.subscribe_in(
        &area,
        window,
        |this, _, event: &TextAreaEvent, window, cx| {
            if *event == TextAreaEvent::Cancel {
                this.close_move_popover_view(window, cx);
            }
        },
    );
    (area, subscription)
}

/// Where the popover sits: below the top of the window, just right of the sidebar.
const POPOVER_TOP: f32 = 120.;
const POPOVER_GAP: f32 = 8.;
const POPOVER_WIDTH: f32 = 340.;

impl StatusView {
    /// The drop target and answer of a sidebar row, as the row is drawn now.
    #[must_use]
    pub(crate) fn drop_zone(&self, target: DropTarget) -> DropZone {
        let answer = self.drop_answer(&target);
        DropZone { target, answer }
    }

    /// The status bar line while a file is dragged, if one runs.
    #[must_use]
    pub(crate) fn drag_status_text(&self) -> Option<String> {
        let dragging = self.model.move_file.drag.is_some();
        let hovered = self
            .model
            .move_file
            .hover
            .hovered()
            .and_then(|target| self.drop_answer(target));
        drag_status(hovered.as_ref(), dragging)
    }

    /// **Open Worktree** on the notice of a finished move, when the move has a worktree.
    pub(crate) fn open_worktree_offer(&self, cx: &Context<Self>) -> Option<Stateful<Div>> {
        let path = self.model.notice_open.clone()?;
        Some(
            div()
                .id("notice-open-worktree")
                .flex_none()
                .px_2()
                .rounded_sm()
                .cursor_pointer()
                .text_color(theme::accent())
                .font_weight(FontWeight::MEDIUM)
                .child("Open Worktree")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.run_action(
                            Action::OpenWorktree,
                            &Args::Worktree(path.clone()),
                            window,
                            cx,
                        );
                    }),
                ),
        )
    }

    /// Opens the popover's inputs for the state the drop opened, focusing the first.
    pub(crate) fn show_move_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popover) = self.model.move_file.popover.clone() else {
            return;
        };
        let mut subscriptions = Vec::new();
        let name = match &popover {
            PopoverState::New { name, .. } => {
                let (area, subscription) = popover_input("Branch name", name, window, cx);
                subscriptions.push(subscription);
                Some(area)
            }
            PopoverState::Existing { .. } => None,
        };
        let (message, subscription) =
            popover_input("Commit message", popover.message(), window, cx);
        subscriptions.push(subscription);
        let first = name.clone().unwrap_or_else(|| message.clone());
        first.update(cx, |area, cx| area.focus(window, cx));
        self.move_popover = Some(MovePopover {
            name,
            message,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    /// Enter in the popover: reads its inputs and sends the move if they are valid.
    pub(crate) fn submit_move_popover_view(&mut self, cx: &mut Context<Self>) {
        let Some(fields) = self.move_popover.as_ref() else {
            return;
        };
        let name = fields
            .name
            .as_ref()
            .map(|area| area.read(cx).text().to_owned());
        let message = fields.message.read(cx).text().to_owned();
        self.submit_move_popover(name, message, cx);
    }

    /// Esc, Cancel and a click outside: closes the popover and gives the keys back to the list.
    pub(crate) fn close_move_popover_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_move_popover(cx);
        self.move_popover = None;
        self.list_focus.focus(window);
    }

    /// Once the state has closed the popover (a move succeeded), the view lets it go too.
    pub(crate) fn sync_move_popover(&mut self, window: &mut Window) {
        if self.move_popover.is_some() && self.model.move_file.popover.is_none() {
            self.move_popover = None;
            self.list_focus.focus(window);
        }
    }

    /// The popover's layer, or nothing while it is closed.
    pub(crate) fn move_popover_layer(&self, cx: &Context<Self>) -> Option<Div> {
        let fields = self.move_popover.as_ref()?;
        let popover = self.model.move_file.popover.as_ref()?;
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_move_popover_view(window, cx);
            }),
        );
        let mut panel = div()
            .absolute()
            .top(px(POPOVER_TOP))
            .left(px(self.sidebar_width + POPOVER_GAP))
            .w(px(POPOVER_WIDTH))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::hairline_strong())
            .rounded_md()
            .shadow_lg()
            .occlude()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.close_move_popover_view(window, cx),
                    "enter" => this.submit_move_popover_view(cx),
                    _ => {}
                }
            }))
            // Tab has nothing else to reach while the popover is open.
            .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
            .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(popover_title(popover)),
            );
        if let Some(name) = &fields.name {
            panel = panel.child(field_row("Branch name", name.clone()));
        }
        panel = panel
            .child(field_row("Commit message", fields.message.clone()))
            .child(div().text_sm().text_color(theme::muted()).child(HOOKS_NOTE))
            .children(self.model.move_file.popover_error.as_ref().map(|error| {
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(error.clone())
            }))
            .child(popover_buttons(cx));
        Some(div().absolute().inset_0().child(outside).child(panel))
    }
}

/// Cancel, and the button that sends the move (Enter does the same).
fn popover_buttons(cx: &Context<StatusView>) -> Div {
    div()
        .flex()
        .flex_row()
        .justify_end()
        .gap_2()
        .child(
            div()
                .id("move-popover-cancel")
                .px_3()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded_md()
                .bg(theme::selection())
                .text_color(theme::text())
                .cursor_pointer()
                .child("Cancel")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_move_popover_view(window, cx);
                    }),
                ),
        )
        .child(
            div()
                .id("move-popover-confirm")
                .px_3()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded_md()
                .bg(theme::accent())
                .text_color(theme::on_accent())
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .child("Move")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.submit_move_popover_view(cx);
                    }),
                ),
        )
}

#[cfg(test)]
mod tests {
    use gm_schema::{
        MoveBlocker, MoveOutcome, MovePreview, MoveTarget, OperationId, OperationState,
        WorktreeEntry,
    };
    use std::time::{Duration, Instant};

    use super::*;

    fn source(path: &str) -> DragSource {
        DragSource {
            path: path.to_owned(),
        }
    }

    fn worktree(path: &str, branch: Option<&str>) -> WorktreeEntry {
        WorktreeEntry {
            path: path.into(),
            head: Some("abc".into()),
            branch: branch.map(Into::into),
            detached: branch.is_none(),
            bare: false,
            is_main: false,
            locked: None,
            prunable: None,
        }
    }

    fn clean() -> MovePreview {
        MovePreview::default()
    }

    fn blocked(blockers: Vec<MoveBlocker>) -> MovePreview {
        MovePreview { blockers }
    }

    fn ctx(worktrees: &[WorktreeEntry]) -> DropContext<'_> {
        DropContext {
            current_branch: Some("main"),
            own_worktree: Some("/repo"),
            git_ok: true,
            busy: None,
            worktrees,
        }
    }

    fn branch(name: &str) -> DropTarget {
        DropTarget::Branch { name: name.into() }
    }

    fn at(path: &str) -> DropTarget {
        DropTarget::Worktree { path: path.into() }
    }

    fn accepts(hint: &str) -> DropAnswer {
        DropAnswer::Accepts { hint: hint.into() }
    }

    fn rejects(reason: &str) -> DropAnswer {
        DropAnswer::Rejects {
            reason: reason.into(),
        }
    }

    #[test]
    fn the_branch_of_the_window_and_its_worktree_never_accept() {
        let list = [];
        let context = ctx(&list);
        let clean = clean();
        assert_eq!(
            answer(&source("a.rs"), &branch("main"), &context, Some(&clean)),
            rejects(SAME_BRANCH)
        );
        assert_eq!(
            answer(&source("a.rs"), &at("/repo/"), &context, Some(&clean)),
            rejects(SAME_WORKTREE)
        );
    }

    #[test]
    fn a_folder_is_refused_for_every_target_with_the_folder_sentence() {
        let list = [];
        let context = ctx(&list);
        let refused = rejects(&describe(&MoveBlocker::IsFolder));
        for target in [
            branch("feature"),
            at("/w/ui"),
            DropTarget::NewBranchGroup { prefix: None },
            DropTarget::NewWorktree,
        ] {
            assert_eq!(answer(&source("new/"), &target, &context, None), refused);
        }
    }

    #[test]
    fn a_busy_repository_refuses_every_target_with_the_reason_of_the_other_actions() {
        let list = [];
        let context = DropContext {
            busy: Some("Wait for the refresh to finish."),
            ..ctx(&list)
        };
        for target in [branch("feature"), DropTarget::NewWorktree] {
            assert_eq!(
                answer(&source("a.rs"), &target, &context, None),
                rejects("Wait for the refresh to finish.")
            );
        }
    }

    #[test]
    fn an_old_git_refuses_every_target_and_the_rows_still_say_why() {
        let list = [];
        let context = DropContext {
            git_ok: false,
            ..ctx(&list)
        };
        assert_eq!(
            answer(&source("a.rs"), &at("/w/ui"), &context, None),
            rejects(GIT_TOO_OLD)
        );
        assert_eq!(
            answer(&source("a.rs"), &DropTarget::NewWorktree, &context, None),
            rejects(GIT_TOO_OLD)
        );
    }

    #[test]
    fn a_branch_or_worktree_row_checks_with_git_before_it_accepts() {
        let list = [worktree("/w/ui", Some("ui"))];
        let context = ctx(&list);
        assert_eq!(
            answer(&source("a.rs"), &branch("feature"), &context, None),
            DropAnswer::Checking
        );
        assert_eq!(
            answer(&source("a.rs"), &at("/w/ui"), &context, None),
            DropAnswer::Checking
        );
    }

    #[test]
    fn a_clean_check_accepts_with_the_hint_of_what_the_drop_does() {
        let list = [worktree("/w/ui", Some("ui")), worktree("/w/scratch", None)];
        let context = ctx(&list);
        let clean = clean();
        assert_eq!(
            answer(&source("a.rs"), &branch("feature"), &context, Some(&clean)),
            accepts("Commit to feature")
        );
        assert_eq!(
            answer(&source("a.rs"), &at("/w/ui"), &context, Some(&clean)),
            accepts("Move to ui")
        );
        assert_eq!(
            answer(&source("a.rs"), &at("/w/scratch"), &context, Some(&clean)),
            accepts("Move to scratch")
        );
    }

    #[test]
    fn a_check_with_blockers_refuses_with_the_first_blockers_sentence() {
        let list = [];
        let context = ctx(&list);
        let preview = blocked(vec![
            MoveBlocker::Conflict {
                paths: vec!["a.rs".into()],
            },
            MoveBlocker::BranchExists,
        ]);
        assert_eq!(
            answer(
                &source("a.rs"),
                &branch("feature"),
                &context,
                Some(&preview)
            ),
            rejects(&describe(&MoveBlocker::Conflict {
                paths: vec!["a.rs".into()],
            }))
        );
    }

    #[test]
    fn a_branch_checked_out_elsewhere_and_a_missing_worktree_refuse_before_the_check() {
        let mut missing = worktree("/w/gone", Some("gone"));
        missing.prunable = Some("gitdir file points to non-existent location".into());
        let list = [worktree("/w/ui", Some("feature")), missing];
        let context = ctx(&list);
        assert_eq!(
            answer(&source("a.rs"), &branch("feature"), &context, None),
            rejects(&describe(&MoveBlocker::BranchCheckedOutElsewhere {
                path: "/w/ui".into(),
            }))
        );
        assert_eq!(
            answer(&source("a.rs"), &at("/w/gone"), &context, None),
            rejects(&describe(&MoveBlocker::WorktreeMissing))
        );
    }

    #[test]
    fn a_creation_row_accepts_without_a_check_and_names_the_file() {
        let list = [];
        let context = ctx(&list);
        assert_eq!(
            answer(
                &source("src/a.rs"),
                &DropTarget::NewBranchGroup {
                    prefix: Some("feature/".into())
                },
                &context,
                None
            ),
            accepts("Create a branch for src/a.rs")
        );
        assert_eq!(
            answer(&source("a.rs"), &DropTarget::NewWorktree, &context, None),
            accepts("Create a worktree for a.rs")
        );
    }

    fn at_time(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    #[test]
    fn a_row_is_asked_about_only_after_the_pointer_rests_on_it() {
        let start = Instant::now();
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        hover.enter(branch("feature"), start);
        assert_eq!(hover.due(at_time(start, 149)), None);
        assert_eq!(hover.due(at_time(start, 150)), Some(branch("feature")));
    }

    #[test]
    fn leaving_the_row_cancels_the_wait_and_a_new_row_restarts_it() {
        let start = Instant::now();
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        hover.enter(branch("feature"), start);
        hover.leave();
        assert_eq!(hover.due(at_time(start, 1000)), None);
        hover.enter(branch("feature"), at_time(start, 1000));
        hover.enter(at("/w/ui"), at_time(start, 1100));
        assert_eq!(hover.due(at_time(start, 1200)), None);
        assert_eq!(hover.due(at_time(start, 1250)), Some(at("/w/ui")));
    }

    #[test]
    fn staying_on_a_row_keeps_its_arrival_time() {
        let start = Instant::now();
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        hover.enter(branch("feature"), start);
        hover.enter(branch("feature"), at_time(start, 100));
        assert_eq!(hover.due(at_time(start, 150)), Some(branch("feature")));
    }

    #[test]
    fn creation_rows_are_never_asked_about() {
        let start = Instant::now();
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        hover.enter(DropTarget::NewWorktree, start);
        assert_eq!(hover.due(at_time(start, 5000)), None);
    }

    #[test]
    fn an_answer_to_the_request_in_flight_is_kept_and_read_for_its_row() {
        let start = Instant::now();
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        hover.enter(branch("feature"), start);
        let id = OperationId(7);
        hover.sent(id);
        let preview = blocked(vec![MoveBlocker::AddOverExisting]);
        assert!(hover.answered(id, preview.clone()));
        assert_eq!(hover.preview(&branch("feature")), Some(&preview));
        assert_eq!(
            answer(
                &source("a.rs"),
                &branch("feature"),
                &ctx(&[]),
                hover.preview(&branch("feature"))
            ),
            rejects(&describe(&MoveBlocker::AddOverExisting))
        );
        // A cached row is not asked again.
        assert_eq!(hover.due(at_time(start, 5000)), None);
    }

    #[test]
    fn a_late_answer_for_a_row_the_pointer_left_is_dropped() {
        let start = Instant::now();
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        hover.enter(branch("feature"), start);
        let id = OperationId(3);
        hover.sent(id);
        hover.enter(at("/w/ui"), at_time(start, 10));
        assert!(!hover.answered(id, clean()));
        assert_eq!(hover.preview(&branch("feature")), None);
    }

    #[test]
    fn an_answer_after_the_pointer_left_the_rows_is_dropped() {
        let start = Instant::now();
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        hover.enter(branch("feature"), start);
        let id = OperationId(4);
        hover.sent(id);
        hover.leave();
        assert!(!hover.answered(id, clean()));
    }

    #[test]
    fn an_answer_with_another_operation_id_is_dropped_and_changes_nothing() {
        let start = Instant::now();
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        hover.enter(branch("feature"), start);
        hover.sent(OperationId(9));
        assert!(!hover.answered(OperationId(8), clean()));
        assert_eq!(hover.due(at_time(start, 500)), None);
        assert!(hover.answered(OperationId(9), clean()));
    }

    #[test]
    fn a_failed_check_is_not_asked_again_until_the_pointer_comes_back() {
        let start = Instant::now();
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        hover.enter(branch("feature"), start);
        let id = OperationId(5);
        hover.sent(id);
        assert!(hover.ended(id, OperationState::Failed));
        assert_eq!(hover.due(at_time(start, 500)), None);
        hover.enter(at("/w/ui"), at_time(start, 600));
        hover.enter(branch("feature"), at_time(start, 700));
        assert_eq!(hover.due(at_time(start, 900)), Some(branch("feature")));
    }

    #[test]
    fn a_status_change_clears_the_answers_so_they_are_asked_again() {
        let start = Instant::now();
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        hover.enter(branch("feature"), start);
        hover.sent(OperationId(1));
        assert!(hover.answered(OperationId(1), clean()));
        hover.status_changed();
        assert_eq!(hover.preview(&branch("feature")), None);
        assert_eq!(hover.due(at_time(start, 500)), Some(branch("feature")));
    }

    #[test]
    fn a_new_drag_starts_with_no_answers_of_the_last_one() {
        let start = Instant::now();
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        hover.enter(branch("feature"), start);
        hover.sent(OperationId(1));
        hover.answered(OperationId(1), clean());
        hover.start("b.rs");
        assert_eq!(hover.preview(&branch("feature")), None);
        assert_eq!(hover.path(), Some("b.rs"));
    }

    #[test]
    fn an_ended_check_that_was_not_asked_changes_nothing() {
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        assert!(!hover.ended(OperationId(2), OperationState::Failed));
    }

    #[test]
    fn the_popover_of_a_branch_is_prefilled_with_the_file_name() {
        let popover = PopoverState::existing("src/a.rs", "feature");
        assert_eq!(popover.message(), "Move a.rs");
        assert!(matches!(popover, PopoverState::Existing { .. }));
        assert_eq!(HOOKS_NOTE, "Git hooks do not run for this commit.");
    }

    #[test]
    fn a_group_popover_starts_its_name_with_the_prefix_and_its_slash() {
        let with_prefix = PopoverState::new("a.rs", Some("feature"));
        assert_eq!(
            with_prefix,
            PopoverState::New {
                path: "a.rs".into(),
                name: "feature/".into(),
                message: "Move a.rs".into(),
            }
        );
        let kept = PopoverState::new("a.rs", Some("fix/"));
        assert!(matches!(kept, PopoverState::New { ref name, .. } if name == "fix/"));
        let bare = PopoverState::new("a.rs", None);
        assert!(matches!(bare, PopoverState::New { ref name, .. } if name.is_empty()));
    }

    #[test]
    fn an_existing_branch_validates_its_message_and_sends_the_move() {
        let existing = ["main".to_owned(), "feature".to_owned()];
        let mut popover = PopoverState::existing("a.rs", "feature");
        popover.set_message("   ".into());
        assert_eq!(
            popover.validate(&existing),
            Err("a commit message is required".to_owned())
        );
        popover.set_message("  Fix the parser  ".into());
        assert_eq!(
            popover.validate(&existing),
            Ok(MoveRequest {
                path: "a.rs".into(),
                target: MoveTarget::Branch {
                    name: "feature".into(),
                    create: false,
                },
                message: "Fix the parser".into(),
            })
        );
    }

    #[test]
    fn an_existing_branch_that_is_gone_is_refused() {
        let popover = PopoverState::existing("a.rs", "gone");
        assert_eq!(
            popover.validate(&["main".to_owned()]),
            Err(describe(&MoveBlocker::BranchNotFound))
        );
    }

    #[test]
    fn a_new_branch_needs_a_valid_free_name_and_a_message() {
        let existing = ["main".to_owned()];
        let mut popover = PopoverState::new("a.rs", None);
        assert_eq!(popover.validate(&existing), Err(NO_NAME.to_owned()));
        popover.set_name("-x".into());
        assert_eq!(
            popover.validate(&existing),
            Err(branch::name_problem("-x")
                .expect("-x is refused")
                .to_owned())
        );
        popover.set_name("main".into());
        assert_eq!(
            popover.validate(&existing),
            Err(describe(&MoveBlocker::BranchExists))
        );
        popover.set_name(" ui/a ".into());
        popover.set_message("Move a.rs".into());
        assert_eq!(
            popover.validate(&existing),
            Ok(MoveRequest {
                path: "a.rs".into(),
                target: MoveTarget::Branch {
                    name: "ui/a".into(),
                    create: true,
                },
                message: "Move a.rs".into(),
            })
        );
    }

    #[test]
    fn a_new_branch_is_validated_into_the_create_with_the_message() {
        let mut popover = PopoverState::new("a.rs", Some("ui"));
        popover.set_name("ui/a".into());
        assert_eq!(
            popover.validate(&[]),
            Ok(MoveRequest {
                path: "a.rs".into(),
                target: MoveTarget::Branch {
                    name: "ui/a".into(),
                    create: true,
                },
                message: "Move a.rs".into(),
            })
        );
    }

    #[test]
    fn setting_the_name_of_an_existing_branch_changes_nothing() {
        let mut popover = PopoverState::existing("a.rs", "feature");
        let before = popover.clone();
        popover.set_name("other".into());
        assert_eq!(popover, before);
    }

    #[test]
    fn a_worktree_move_says_the_file_is_in_the_worktree_and_offers_to_open_it() {
        let outcome = MoveOutcome::default();
        let notice = moved_notice(&outcome, &at("/w/ui"), "a.rs", "ui");
        assert_eq!(
            notice,
            Notice {
                text: "Moved a.rs to ui.".into(),
                open_worktree: Some("/w/ui".into()),
            }
        );
    }

    #[test]
    fn a_branch_move_says_the_commit_went_to_the_branch() {
        let outcome = MoveOutcome {
            commit: Some("abc1234".into()),
            kept: None,
        };
        let notice = moved_notice(&outcome, &branch("feature"), "a.rs", "");
        assert_eq!(
            notice,
            Notice {
                text: "Committed a.rs to feature.".into(),
                open_worktree: None,
            }
        );
    }

    #[test]
    fn a_kept_file_says_why_it_stayed_in_the_working_copy() {
        let outcome = MoveOutcome {
            commit: None,
            kept: Some("the file changed after the move was checked".into()),
        };
        let notice = moved_notice(&outcome, &at("/w/ui"), "a.rs", "ui");
        assert_eq!(
            notice.text,
            "Moved a.rs to ui. The file stays in the Working Copy: the file changed after the move was checked."
        );
        assert_eq!(notice.open_worktree, Some("/w/ui".into()));
        let kept_branch = MoveOutcome {
            commit: Some("abc".into()),
            kept: Some("it is kept.".into()),
        };
        assert_eq!(
            moved_notice(&kept_branch, &branch("feature"), "a.rs", "").text,
            "Committed a.rs to feature. The file stays in the Working Copy: it is kept."
        );
    }

    #[test]
    fn a_new_worktree_move_names_the_worktree_it_made() {
        let notice = moved_notice(
            &MoveOutcome::default(),
            &DropTarget::NewWorktree,
            "a.rs",
            "",
        );
        assert_eq!(notice.text, "Moved a.rs to a new worktree.");
    }

    #[test]
    fn a_worktree_name_is_the_sidebar_name_or_the_folder() {
        let list = [worktree("/w/ui", Some("ui")), worktree("/w/free", None)];
        assert_eq!(worktree_name("/w/ui", &list), "ui");
        assert_eq!(worktree_name("/w/free", &list), "free");
        assert_eq!(worktree_name("/w/other/", &[]), "other");
    }

    #[test]
    fn a_move_that_succeeds_closes_its_popover_and_gives_the_notice() {
        let mut state = MoveFile::default();
        let id = OperationId(11);
        state.open_popover(PopoverState::existing("a.rs", "feature"));
        state.sent_move(Moving {
            id,
            file: "a.rs".into(),
            target: branch("feature"),
            worktree: String::new(),
            from_popover: true,
        });
        let notice = state.moved(id, &MoveOutcome::default());
        assert_eq!(
            notice.map(|notice| notice.text),
            Some("Committed a.rs to feature.".to_owned())
        );
        assert!(state.popover.is_none());
        assert!(state.moving.is_none());
    }

    #[test]
    fn a_move_answer_that_is_not_the_one_in_flight_changes_nothing() {
        let mut state = MoveFile::default();
        state.sent_move(Moving {
            id: OperationId(1),
            file: "a.rs".into(),
            target: at("/w/ui"),
            worktree: "ui".into(),
            from_popover: false,
        });
        assert_eq!(state.moved(OperationId(2), &MoveOutcome::default()), None);
        assert!(state.moving.is_some());
    }

    #[test]
    fn a_failed_branch_move_stays_in_the_popover_with_the_message() {
        let mut state = MoveFile::default();
        let id = OperationId(12);
        state.open_popover(PopoverState::existing("a.rs", "feature"));
        state.sent_move(Moving {
            id,
            file: "a.rs".into(),
            target: branch("feature"),
            worktree: String::new(),
            from_popover: true,
        });
        let status = state.settle(
            id,
            OperationState::Failed,
            Some("update-ref refused".into()),
        );
        assert_eq!(status, None);
        assert!(state.popover.is_some());
        assert_eq!(state.popover_error.as_deref(), Some("update-ref refused"));
        assert!(state.moving.is_none());
    }

    #[test]
    fn a_failed_worktree_move_gives_its_message_to_the_status_bar() {
        let mut state = MoveFile::default();
        let id = OperationId(13);
        state.sent_move(Moving {
            id,
            file: "a.rs".into(),
            target: at("/w/ui"),
            worktree: "ui".into(),
            from_popover: false,
        });
        assert_eq!(
            state.settle(id, OperationState::Failed, None),
            Some("The change was not moved.".to_owned())
        );
        assert!(state.moving.is_none());
    }

    #[test]
    fn a_move_that_is_done_without_an_answer_just_ends() {
        let mut state = MoveFile::default();
        let id = OperationId(14);
        state.sent_move(Moving {
            id,
            file: "a.rs".into(),
            target: at("/w/ui"),
            worktree: "ui".into(),
            from_popover: false,
        });
        assert_eq!(state.settle(id, OperationState::Done, None), None);
        assert!(state.moving.is_none());
    }

    #[test]
    fn an_old_git_in_a_preview_is_remembered_and_refuses_later_drops() {
        let mut state = MoveFile::default();
        assert!(state.git_ok());
        state.begin_drag("a.rs");
        state.hover.enter(branch("feature"), Instant::now());
        state.hover.sent(OperationId(1));
        assert!(state.answered(
            OperationId(1),
            blocked(vec![MoveBlocker::GitTooOld {
                found: "2.30.0".into(),
            }]),
        ));
        assert!(!state.git_ok());
    }

    #[test]
    fn a_drag_has_a_source_until_it_ends() {
        let mut state = MoveFile::default();
        state.begin_drag("a.rs");
        assert_eq!(state.drag, Some(source("a.rs")));
        state.end_drag();
        assert_eq!(state.drag, None);
        assert_eq!(state.hover.path(), None);
    }

    #[test]
    fn the_file_for_a_new_worktree_is_taken_once() {
        let mut state = MoveFile {
            after_create: Some("a.rs".into()),
            ..MoveFile::default()
        };
        assert_eq!(state.after_create.take(), Some("a.rs".into()));
        assert_eq!(state.after_create.take(), None);
    }

    #[test]
    fn the_refusal_sentences_are_the_engines_own() {
        assert_eq!(FOLDER_REASON, describe(&MoveBlocker::IsFolder));
        assert_eq!(MISSING_FOLDER, describe(&MoveBlocker::WorktreeMissing));
    }

    #[test]
    fn the_engine_target_exists_for_rows_and_not_for_creations() {
        assert_eq!(
            branch("feature").move_target(),
            Some(MoveTarget::Branch {
                name: "feature".into(),
                create: false,
            })
        );
        assert_eq!(
            at("/w/ui").move_target(),
            Some(MoveTarget::Worktree {
                path: "/w/ui".into(),
            })
        );
        assert_eq!(DropTarget::NewWorktree.move_target(), None);
        assert_eq!(
            DropTarget::NewBranchGroup { prefix: None }.move_target(),
            None
        );
    }

    #[test]
    fn a_row_is_styled_by_its_answer_and_plain_without_a_drag() {
        let accepts = DropAnswer::Accepts {
            hint: "Commit to feature".into(),
        };
        let rejects = DropAnswer::Rejects {
            reason: "no".into(),
        };
        assert_eq!(drag_style(Some(&accepts)), Some(DragStyle::Accept));
        assert_eq!(drag_style(Some(&rejects)), Some(DragStyle::Reject));
        assert_eq!(
            drag_style(Some(&DropAnswer::Checking)),
            Some(DragStyle::Checking)
        );
        assert_eq!(drag_style(None), None);
    }

    #[test]
    fn only_an_accepting_or_checking_row_takes_the_drop() {
        let accepts = DropAnswer::Accepts { hint: "x".into() };
        let rejects = DropAnswer::Rejects { reason: "x".into() };
        assert!(takes_drop(Some(&accepts)));
        assert!(takes_drop(Some(&DropAnswer::Checking)));
        assert!(!takes_drop(Some(&rejects)));
        assert!(!takes_drop(None));
    }

    #[test]
    fn the_status_line_shows_the_hovered_row_or_what_to_do() {
        let accepts = DropAnswer::Accepts {
            hint: "Move to ui".into(),
        };
        assert_eq!(drag_status(Some(&accepts), true), Some("Move to ui".into()));
        let rejects = DropAnswer::Rejects {
            reason: "Conflicts".into(),
        };
        assert_eq!(drag_status(Some(&rejects), true), Some("Conflicts".into()));
        assert_eq!(
            drag_status(Some(&DropAnswer::Checking), true),
            Some("Checking…".into())
        );
        assert_eq!(
            drag_status(None, true),
            Some("Drop the file on a branch or a worktree to move it.".into())
        );
        assert_eq!(drag_status(Some(&accepts), false), None);
    }

    #[test]
    fn the_popover_title_names_the_commit_or_the_new_branch() {
        let existing = PopoverState::existing("src/a.rs", "feature");
        assert_eq!(popover_title(&existing), "Commit a.rs to feature");
        let new = PopoverState::new("a.rs", Some("ui"));
        assert_eq!(popover_title(&new), "New branch");
    }

    #[test]
    fn a_poll_asks_while_the_drag_runs_and_stops_when_it_is_over_or_superseded() {
        assert_eq!(poll_step(1, 1, true, true), PollStep::Ask);
        assert_eq!(poll_step(1, 1, true, false), PollStep::End);
        assert_eq!(poll_step(1, 1, false, true), PollStep::Stop);
        assert_eq!(poll_step(1, 1, false, false), PollStep::Stop);
        assert_eq!(poll_step(1, 2, true, true), PollStep::Stop);
    }

    #[test]
    fn a_drag_gets_a_new_generation_so_an_older_poll_stops() {
        let mut state = MoveFile::default();
        let first = state.begin_drag("a.rs");
        state.end_drag();
        let second = state.begin_drag("b.rs");
        assert_ne!(first, second);
        assert_eq!(
            poll_step(first, state.generation, true, true),
            PollStep::Stop
        );
        assert_eq!(
            poll_step(second, state.generation, true, true),
            PollStep::Ask
        );
    }

    #[test]
    fn the_pointer_enters_and_leaves_only_the_row_it_is_on() {
        let feature = branch("feature");
        let ui = at("/w/ui");
        assert_eq!(pointer_step(None, &feature, true), PointerStep::Enter);
        assert_eq!(
            pointer_step(Some(&feature), &feature, true),
            PointerStep::Stay
        );
        assert_eq!(
            pointer_step(Some(&feature), &feature, false),
            PointerStep::Leave
        );
        assert_eq!(pointer_step(Some(&ui), &feature, false), PointerStep::Stay);
        assert_eq!(pointer_step(Some(&ui), &feature, true), PointerStep::Enter);
    }

    #[test]
    fn the_hovered_row_is_the_one_last_entered() {
        let mut hover = HoverCheck::default();
        hover.start("a.rs");
        assert_eq!(hover.hovered(), None);
        hover.enter(branch("feature"), Instant::now());
        assert_eq!(hover.hovered(), Some(&branch("feature")));
        hover.leave();
        assert_eq!(hover.hovered(), None);
    }
}
