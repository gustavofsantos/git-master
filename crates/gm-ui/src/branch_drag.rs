//! Dragging a branch onto another in the By stack view to set its parent
//! (docs/guide/stacks.md, Drag to set a parent). The drop sends the same
//! `SetStackParent` as Set Parent…. What a row answers for the dragged branch is
//! pure and tested here; the window side is the `impl StatusView` at the bottom.
//! The payload is its own type, so a branch never lands on a file's drop handler
//! and a file never lands on this one.

use gm_schema::{Actor, CommandKind, StackSnapshot};
use gpui::{
    App, Context, DragMoveEvent, InteractiveElement, Render, StatefulInteractiveElement,
    StyleRefinement, Styled, Window, div, prelude::*, px,
};

use crate::action_registry::Reason;
use crate::move_file::POLL;
use crate::theme;
use crate::view::StatusView;

/// The status bar line while a branch is dragged over no row.
const NO_ROW: &str = "Drop the branch on another branch to make that one its parent.";

/// What a branch row says about the branch dragged over it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BranchAnswer {
    /// The row can be the parent; the hint says what the drop does.
    Accepts { hint: String },
    /// The row cannot be the parent, for this reason.
    Rejects { reason: String },
}

impl BranchAnswer {
    fn text(&self) -> &str {
        match self {
            Self::Accepts { hint } => hint,
            Self::Rejects { reason } => reason,
        }
    }
}

/// What `target` answers for `dragged`, from the last stack snapshot and the
/// repository's busy reason. The engine checks shared history on the drop.
#[must_use]
pub(crate) fn answer(
    dragged: &str,
    target: &str,
    stacks: Option<&StackSnapshot>,
    busy: Option<Reason>,
) -> BranchAnswer {
    let refuse = |reason: String| BranchAnswer::Rejects { reason };
    if dragged == target {
        return refuse("A branch cannot be its own parent.".to_owned());
    }
    if let Some(reason) = busy {
        return refuse(reason.to_owned());
    }
    if let Some(snapshot) = stacks {
        if snapshot
            .branch(dragged)
            .and_then(|entry| entry.parent.as_deref())
            == Some(target)
        {
            return refuse(format!("{target} is already the parent of {dragged}."));
        }
        if is_under(snapshot, dragged, target) {
            return refuse(format!(
                "{target} is under {dragged} in its stack, so it cannot be its parent."
            ));
        }
    }
    BranchAnswer::Accepts {
        hint: format!("Make {target} the parent of {dragged}."),
    }
}

/// Whether `branch` is below `ancestor` in its stack. The snapshot lists each
/// stack parents first, so what is under a branch follows it at a greater depth.
fn is_under(snapshot: &StackSnapshot, ancestor: &str, branch: &str) -> bool {
    snapshot.stacks.iter().any(|stack| {
        let Some(start) = stack.branches.iter().position(|e| e.name == ancestor) else {
            return false;
        };
        let depth = stack.branches[start].depth;
        stack.branches[start + 1..]
            .iter()
            .take_while(|entry| entry.depth > depth)
            .any(|entry| entry.name == branch)
    })
}

/// The status bar line while a branch is dragged: the answer of the row under the
/// pointer, or what to do when no row is under it.
#[must_use]
pub(crate) fn status(hovered: Option<&BranchAnswer>) -> String {
    hovered.map_or_else(|| NO_ROW.to_owned(), |answer| answer.text().to_owned())
}

/// The branch being dragged and the row under the pointer, while a drag runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BranchDrag {
    dragged: Option<String>,
    hovered: Option<String>,
    /// Counts the drags, so the poll of an ended drag stops.
    generation: u64,
}

impl BranchDrag {
    /// A drag of `name` starts; returns its generation.
    pub(crate) fn begin(&mut self, name: &str) -> u64 {
        self.generation += 1;
        self.dragged = Some(name.to_owned());
        self.hovered = None;
        self.generation
    }

    pub(crate) fn end(&mut self) {
        self.dragged = None;
        self.hovered = None;
    }

    #[must_use]
    pub(crate) fn dragged(&self) -> Option<&str> {
        self.dragged.as_deref()
    }

    #[must_use]
    pub(crate) fn hovered(&self) -> Option<&str> {
        self.hovered.as_deref()
    }

    /// The pointer is over `row` (`inside`) or has left it. Only the row last
    /// entered is cleared on leaving, so rows that report late do not clear another.
    pub(crate) fn point(&mut self, row: &str, inside: bool) -> bool {
        match (inside, self.hovered.as_deref() == Some(row)) {
            (true, false) => {
                self.hovered = Some(row.to_owned());
                true
            }
            (false, true) => {
                self.hovered = None;
                true
            }
            _ => false,
        }
    }

    /// Whether the poll started for `generation` goes on.
    #[must_use]
    pub(crate) fn current(&self, generation: u64) -> bool {
        self.generation == generation && self.dragged.is_some()
    }
}

/// The payload of a drag from a branch row of the By stack view.
pub(crate) struct DraggedBranch {
    pub(crate) name: String,
}

/// What a branch drag shows under the pointer: the branch name.
struct BranchGhost {
    label: String,
}

impl Render for BranchGhost {
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
            .text_sm()
            .child(self.label.clone())
    }
}

/// Makes a branch row of the By stack view draggable. gpui starts the drag only
/// past its threshold, so a click and a right click keep working.
pub(crate) fn drag_source<E: StatefulInteractiveElement>(
    row: E,
    name: &str,
    cx: &Context<StatusView>,
) -> E {
    let view = cx.entity().downgrade();
    row.on_drag(
        DraggedBranch {
            name: name.to_owned(),
        },
        move |dragged: &DraggedBranch, _, _, cx: &mut App| {
            let name = dragged.name.clone();
            let _ = view.update(cx, |this, cx| this.begin_branch_drag(&name, cx));
            cx.new(|_| BranchGhost { label: name })
        },
    )
}

/// A local branch row of the By stack view: it is dragged, and it takes a dragged
/// branch. `answer` is what it says about the branch dragged now, `None` when no
/// branch drag runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BranchZone {
    pub(crate) name: String,
    pub(crate) answer: Option<BranchAnswer>,
}

/// Makes a row a branch zone: draggable, and taking a dropped branch. A row
/// without a zone is left as it is.
pub(crate) fn branch_zone<E: StatefulInteractiveElement>(
    row: E,
    zone: Option<BranchZone>,
    cx: &Context<StatusView>,
) -> E {
    match zone {
        Some(zone) => {
            let row = drag_source(row, &zone.name, cx);
            accept_branch(row, &zone.name, zone.answer.as_ref(), cx)
        }
        None => row,
    }
}

/// The style a branch drag adds to a row, by its answer.
fn dragged_over(style: StyleRefinement, accepts: Option<bool>) -> StyleRefinement {
    match accepts {
        Some(true) => style
            .bg(theme::selection())
            .border_1()
            .border_color(theme::accent()),
        Some(false) => style.opacity(0.4).cursor_not_allowed(),
        None => style,
    }
}

/// Makes the row of local branch `name` take a dropped branch. `answer` is what
/// the row says about the branch dragged now, `None` when no branch drag runs.
pub(crate) fn accept_branch<E: InteractiveElement>(
    element: E,
    name: &str,
    answer: Option<&BranchAnswer>,
    cx: &Context<StatusView>,
) -> E {
    let accepts = answer.map(|answer| matches!(answer, BranchAnswer::Accepts { .. }));
    let target = name.to_owned();
    let hovered = name.to_owned();
    // No `can_drop` here: gpui keeps one per element, the file drag's, which lets
    // a branch through. A refusing row must pass it anyway, or gpui would not draw
    // its dimmed style; the drop itself checks again and says why it refuses.
    element
        .drag_over::<DraggedBranch>(move |style, _, _, _| dragged_over(style, accepts))
        .on_drop(cx.listener(move |this, dragged: &DraggedBranch, _, cx| {
            this.drop_branch(&dragged.name, &target, cx);
        }))
        .on_drag_move(
            cx.listener(move |this, event: &DragMoveEvent<DraggedBranch>, _, cx| {
                let inside = event.bounds.contains(&event.event.position);
                if this.model.branch_drag.point(&hovered, inside) {
                    cx.notify();
                }
            }),
        )
}

impl StatusView {
    /// A branch was picked up. A poll ends the window's drag once gpui no longer
    /// carries one (a drop outside, a release, a cancel).
    pub(crate) fn begin_branch_drag(&mut self, name: &str, cx: &mut Context<Self>) {
        let generation = self.model.branch_drag.begin(name);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let kept = this
                    .update(cx, |view, cx| {
                        if !view.model.branch_drag.current(generation) {
                            return false;
                        }
                        if cx.has_active_drag() {
                            return true;
                        }
                        view.model.branch_drag.end();
                        cx.notify();
                        false
                    })
                    .unwrap_or(false);
                if !kept {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// The branch zone of the row of local branch `name`, as it is drawn now.
    #[must_use]
    pub(crate) fn branch_zone(&self, name: &str) -> BranchZone {
        BranchZone {
            name: name.to_owned(),
            answer: self.branch_drop_answer(name),
        }
    }

    /// What the row of `target` answers for the branch dragged now; `None` when
    /// no branch drag runs.
    #[must_use]
    pub(crate) fn branch_drop_answer(&self, target: &str) -> Option<BranchAnswer> {
        let dragged = self.model.branch_drag.dragged()?;
        Some(answer(
            dragged,
            target,
            self.model.stacks.snapshot(),
            self.busy_reason(),
        ))
    }

    /// The status bar line while a branch is dragged, if one is.
    #[must_use]
    pub(crate) fn branch_drag_status(&self) -> Option<String> {
        self.model.branch_drag.dragged()?;
        let hovered = self
            .model
            .branch_drag
            .hovered()
            .and_then(|row| self.branch_drop_answer(row));
        Some(status(hovered.as_ref()))
    }

    /// `dragged` was dropped on the row of `target`: a refused drop says why, an
    /// accepted one sends the parent change. The drag ends either way.
    fn drop_branch(&mut self, dragged: &str, target: &str, cx: &mut Context<Self>) {
        let verdict = answer(
            dragged,
            target,
            self.model.stacks.snapshot(),
            self.busy_reason(),
        );
        self.model.branch_drag.end();
        match verdict {
            BranchAnswer::Rejects { reason } => self.notify_user(&reason, cx),
            BranchAnswer::Accepts { .. } => {
                self.engine.dispatch(
                    Actor::Human,
                    CommandKind::SetStackParent {
                        branch: dragged.to_owned(),
                        parent: target.to_owned(),
                    },
                );
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{Stack, StackBranch, StackState};

    fn entry(name: &str, parent: Option<&str>, depth: u32) -> StackBranch {
        StackBranch {
            name: name.into(),
            parent: parent.map(str::to_owned),
            depth,
            state: StackState::Current,
            ahead: 0,
            behind: 0,
            restack: 0,
        }
    }

    /// main <- base <- (api, ui <- e2e)
    fn ledger() -> StackSnapshot {
        StackSnapshot {
            stacks: vec![Stack {
                branches: vec![
                    entry("main", None, 0),
                    entry("base", Some("main"), 1),
                    entry("api", Some("base"), 2),
                    entry("ui", Some("base"), 2),
                    entry("e2e", Some("ui"), 3),
                ],
            }],
            looped: Vec::new(),
        }
    }

    fn rejects(answer: &BranchAnswer) -> &str {
        match answer {
            BranchAnswer::Rejects { reason } => reason,
            BranchAnswer::Accepts { hint } => panic!("accepted: {hint}"),
        }
    }

    #[test]
    fn a_branch_outside_the_dragged_subtree_accepts() {
        let stacks = ledger();
        assert_eq!(
            answer("e2e", "api", Some(&stacks), None),
            BranchAnswer::Accepts {
                hint: "Make api the parent of e2e.".into()
            }
        );
        // A branch in no stack may take a parent, and be one.
        assert!(matches!(
            answer("lone", "e2e", Some(&stacks), None),
            BranchAnswer::Accepts { .. }
        ));
        assert!(matches!(
            answer("api", "lone", Some(&stacks), None),
            BranchAnswer::Accepts { .. }
        ));
        // Before the first snapshot only the branch itself is refused.
        assert!(matches!(
            answer("base", "e2e", None, None),
            BranchAnswer::Accepts { .. }
        ));
    }

    #[test]
    fn itself_its_parent_and_its_subtree_refuse() {
        let stacks = ledger();
        assert_eq!(
            rejects(&answer("ui", "ui", Some(&stacks), None)),
            "A branch cannot be its own parent."
        );
        assert_eq!(
            rejects(&answer("ui", "base", Some(&stacks), None)),
            "base is already the parent of ui."
        );
        assert!(rejects(&answer("base", "e2e", Some(&stacks), None)).contains("under base"));
        assert!(rejects(&answer("main", "api", Some(&stacks), None)).contains("under main"));
        // A sibling's subtree is not under the dragged branch.
        assert!(matches!(
            answer("api", "e2e", Some(&stacks), None),
            BranchAnswer::Accepts { .. }
        ));
    }

    #[test]
    fn a_busy_repository_refuses_with_its_reason() {
        assert_eq!(
            rejects(&answer("api", "main", None, Some("A checkout is running"))),
            "A checkout is running"
        );
    }

    #[test]
    fn the_status_line_follows_the_row_under_the_pointer() {
        assert_eq!(status(None), NO_ROW);
        let hint = BranchAnswer::Accepts {
            hint: "Make api the parent of e2e.".into(),
        };
        assert_eq!(status(Some(&hint)), "Make api the parent of e2e.");
    }

    #[test]
    fn the_drag_tracks_the_row_entered_last_and_its_generation() {
        let mut drag = BranchDrag::default();
        let first = drag.begin("api");
        assert_eq!(drag.dragged(), Some("api"));
        assert!(drag.point("main", true));
        assert!(!drag.point("main", true));
        // A late leave of another row does not clear the hovered one.
        assert!(!drag.point("base", false));
        assert_eq!(drag.hovered(), Some("main"));
        assert!(drag.point("main", false));
        assert_eq!(drag.hovered(), None);
        assert!(drag.current(first));
        let second = drag.begin("ui");
        assert!(!drag.current(first));
        drag.end();
        assert!(!drag.current(second));
        assert_eq!(drag.dragged(), None);
    }
}
