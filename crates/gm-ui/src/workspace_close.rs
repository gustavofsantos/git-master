//! Closing a window whose repository is an ephemeral workspace (D185; docs/guide/workspaces.md,
//! "Durable and ephemeral workspaces"). Each window is its own process, so closing the window and
//! quitting are one exit, and both ask `request` first. The exit is held while the workspace is
//! inspected and removed, or while the dialog asks; the model answers `Exit` once it has decided,
//! so the close that follows never asks again. The state machine is pure and tested here; the
//! window side, the dialog and the wiring are the `impl StatusView` and `register` at the bottom.

use gm_schema::{
    Actor, CommandKind, Event, OperationId, OperationState, RemovalBlocker, WorkspaceKind,
};
use gpui::{
    App, Context, Div, Entity, FontWeight, Global, KeyDownEvent, MouseButton, MouseDownEvent,
    Stateful, WeakEntity, Window, div, prelude::*, px,
};

use crate::FocusNext;
use crate::FocusPrev;
use crate::theme;
use crate::view::StatusView;
use crate::workspace_list::folder_name;
use crate::workspace_remove::{asks_confirmation, confirmation_label, may_remove};
use crate::workspace_remove::{body_lines, confirm_row};

/// Shown while the removal runs; the dialog stays until the window closes.
const REMOVING: &str = "Removing the workspace folder…";

/// What the window knows of its repository's kind (D185 (4)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Kind {
    /// Not read yet for the repository the window shows, or the read failed.
    #[default]
    Unknown,
    /// The kind the read answered; `None` for a repository that is not a workspace.
    Known(Option<WorkspaceKind>),
}

/// The engine command the window sends for a step of the exit (D185).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Order {
    /// Reads the kind of the open repository.
    ReadKind,
    /// Asks what removing the workspace at `path` would refuse or lose, as a closing window.
    Inspect { path: String },
    /// Removes the workspace at `path`, as a closing window. `force` is the confirmation.
    Remove { path: String, force: bool },
}

impl Order {
    /// The command the order sends.
    #[must_use]
    pub(crate) fn command(&self) -> CommandKind {
        match self {
            Self::ReadKind => CommandKind::ReadWorkspaceKind,
            Self::Inspect { path } => CommandKind::InspectWorkspaceRemoval {
                path: path.clone(),
                closing: true,
            },
            Self::Remove { path, force } => CommandKind::RemoveWorkspace {
                path: path.clone(),
                force: *force,
                closing: true,
            },
        }
    }
}

/// What the window does next, as the model decides it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Nothing changes on the screen.
    Nothing,
    /// Sends the order; the model is told its operation with `dispatched`.
    Send(Order),
    /// The dialog opened: it takes the keys.
    Show,
    /// The window closes and the app quits.
    Exit,
}

/// Where the exit is. `Inspecting` and `Removing` carry the operation the model waits for, once
/// the view has sent it; `Reading` waits for the kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Reading,
    Inspecting(Option<OperationId>),
    Asking {
        blockers: Vec<RemovalBlocker>,
        confirmed: bool,
    },
    Removing(Option<OperationId>),
    /// The message the dialog shows; OK or Cancel returns to `Idle`.
    Failed(String),
    /// Decided: the window closes, and a later close asks nothing.
    Exit,
}

/// The dialog while it is open, as the view draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dialog<'a> {
    Asking {
        path: &'a str,
        blockers: &'a [RemovalBlocker],
        confirmed: bool,
    },
    Removing {
        path: &'a str,
    },
    Failed {
        path: &'a str,
        message: &'a str,
    },
}

/// The exit check of the window. Events are fed through `answer`; the steps they produce wait in
/// the queue until the view takes them with `take_steps`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceClose {
    kind: Kind,
    /// The kind read in flight, whose answer is the only one taken.
    kind_read: Option<OperationId>,
    phase: Phase,
    /// The workspace the exit is about: the window's repository. Empty while not known.
    path: String,
    /// The repository the kind was asked for, as the status last named it.
    root: Option<String>,
    steps: Vec<Step>,
}

impl WorkspaceClose {
    /// Notes the repository the window shows, as its status reports it (D185 (4)). A change makes
    /// the kind unknown and drops a dialog or an exit that waited for the old repository; a read in
    /// flight is then not taken. Returns whether the kind must be read again for the new one.
    pub(crate) fn observe_root(&mut self, root: Option<&str>) -> bool {
        if self.root.as_deref() == root {
            return false;
        }
        self.root = root.map(str::to_owned);
        self.kind = Kind::Unknown;
        self.kind_read = None;
        if matches!(
            self.phase,
            Phase::Reading | Phase::Inspecting(_) | Phase::Asking { .. } | Phase::Failed(_)
        ) {
            self.phase = Phase::Idle;
            self.path.clear();
        }
        root.is_some()
    }

    /// The window was asked to close, or the app to quit. `path` is the repository the window
    /// shows, `None` while its status is not loaded. The answer is `Exit` when the window may close
    /// now; otherwise the exit is held and the step says what to send.
    #[must_use]
    pub(crate) fn request(&mut self, path: Option<&str>) -> Step {
        match self.phase {
            Phase::Exit => Step::Exit,
            Phase::Idle => self.start(path),
            // A flow is under way: the operation or the dialog answers it, so a second request
            // changes nothing.
            _ => Step::Nothing,
        }
    }

    /// The engine accepted `order` as operation `id`. A read's id is the one its answer carries.
    pub(crate) fn dispatched(&mut self, order: &Order, id: OperationId) {
        match (order, &mut self.phase) {
            (Order::ReadKind, _) => self.kind_read = Some(id),
            (Order::Inspect { .. }, Phase::Inspecting(slot))
            | (Order::Remove { .. }, Phase::Removing(slot)) => *slot = Some(id),
            _ => {}
        }
    }

    /// The user ticked or unticked the confirmation of the dialog. It only moves when the dialog
    /// asks for one.
    pub(crate) fn toggle(&mut self) {
        if let Phase::Asking {
            blockers,
            confirmed,
        } = &mut self.phase
            && asks_confirmation(blockers)
        {
            *confirmed = !*confirmed;
        }
    }

    /// Remove, or OK when the removal failed. Remove is sent only when the rules allow it.
    #[must_use]
    pub(crate) fn confirm(&mut self) -> Step {
        match &self.phase {
            Phase::Asking {
                blockers,
                confirmed,
            } => {
                if !may_remove(blockers, *confirmed) {
                    return Step::Nothing;
                }
                let force = *confirmed;
                self.phase = Phase::Removing(None);
                Step::Send(Order::Remove {
                    path: self.path.clone(),
                    force,
                })
            }
            Phase::Failed(_) => self.cancel(),
            _ => Step::Nothing,
        }
    }

    /// Keep workspace: the window closes and the workspace stays.
    #[must_use]
    pub(crate) fn keep(&mut self) -> Step {
        if matches!(self.phase, Phase::Asking { .. }) {
            self.leave()
        } else {
            Step::Nothing
        }
    }

    /// Cancel, or OK on a failure: the window stays and the exit is no longer pending.
    pub(crate) fn cancel(&mut self) -> Step {
        if matches!(self.phase, Phase::Asking { .. } | Phase::Failed(_)) {
            self.phase = Phase::Idle;
            self.path.clear();
        }
        Step::Nothing
    }

    /// The dialog to draw, while there is one.
    #[must_use]
    pub(crate) fn dialog(&self) -> Option<Dialog<'_>> {
        let path = self.path.as_str();
        match &self.phase {
            Phase::Asking {
                blockers,
                confirmed,
            } => Some(Dialog::Asking {
                path,
                blockers: blockers.as_slice(),
                confirmed: *confirmed,
            }),
            Phase::Removing(_) => Some(Dialog::Removing { path }),
            Phase::Failed(message) => Some(Dialog::Failed { path, message }),
            _ => None,
        }
    }

    /// Takes the steps the answers produced, in order.
    pub(crate) fn take_steps(&mut self) -> Vec<Step> {
        std::mem::take(&mut self.steps)
    }

    /// Reads an event: an answer to the operation this model waits for moves the exit on. Answers
    /// to other operations, and late or repeated ones, change nothing.
    pub(crate) fn answer(&mut self, event: &Event) {
        let step = match event {
            Event::WorkspaceKindRead { id, kind } if self.kind_read == Some(*id) => {
                self.kind_read = None;
                self.kind = Kind::Known(*kind);
                if self.phase == Phase::Reading {
                    self.resume()
                } else {
                    Step::Nothing
                }
            }
            Event::WorkspaceRemovalInspected { id, blockers, .. }
                if self.phase == Phase::Inspecting(Some(*id)) =>
            {
                self.inspected(blockers.clone())
            }
            Event::WorkspaceRemoved { id, .. } if self.phase == Phase::Removing(Some(*id)) => {
                self.leave()
            }
            Event::OperationChanged {
                id, state, detail, ..
            } => self.operation(*id, *state, detail.as_deref()),
            _ => Step::Nothing,
        };
        if step != Step::Nothing {
            self.steps.push(step);
        }
    }

    /// The exit request, once the state is known.
    fn start(&mut self, path: Option<&str>) -> Step {
        path.unwrap_or_default().clone_into(&mut self.path);
        match self.kind {
            Kind::Unknown => {
                self.phase = Phase::Reading;
                if self.kind_read.is_some() {
                    Step::Nothing
                } else {
                    Step::Send(Order::ReadKind)
                }
            }
            Kind::Known(_) => self.resume(),
        }
    }

    /// Decides the exit from the known kind: an ephemeral workspace is inspected, anything else
    /// closes at once.
    fn resume(&mut self) -> Step {
        match self.kind {
            Kind::Known(Some(WorkspaceKind::Ephemeral)) if self.path.is_empty() => {
                // Nothing to inspect yet: the exit waits for a request with the path.
                self.phase = Phase::Idle;
                Step::Nothing
            }
            Kind::Known(Some(WorkspaceKind::Ephemeral)) => {
                self.phase = Phase::Inspecting(None);
                Step::Send(Order::Inspect {
                    path: self.path.clone(),
                })
            }
            Kind::Known(_) => self.leave(),
            Kind::Unknown => Step::Nothing,
        }
    }

    /// The inspection's answer: nothing would be lost, so the workspace is removed; otherwise the
    /// dialog asks.
    fn inspected(&mut self, blockers: Vec<RemovalBlocker>) -> Step {
        if blockers.is_empty() {
            self.phase = Phase::Removing(None);
            Step::Send(Order::Remove {
                path: self.path.clone(),
                force: false,
            })
        } else {
            self.phase = Phase::Asking {
                blockers,
                confirmed: false,
            };
            Step::Show
        }
    }

    /// The state of an operation the model may wait for. Only a failure ends a wait that the
    /// answer event has not ended.
    fn operation(&mut self, id: OperationId, state: OperationState, detail: Option<&str>) -> Step {
        if !state.is_terminal() {
            return Step::Nothing;
        }
        let done = state == OperationState::Done;
        match &self.phase {
            // The answer is the inspection's event, which carries the blockers.
            Phase::Inspecting(Some(owned)) if *owned == id => {
                if done {
                    Step::Nothing
                } else {
                    self.fail(detail, "The workspace could not be read.")
                }
            }
            Phase::Removing(Some(owned)) if *owned == id => {
                if done {
                    self.leave()
                } else {
                    self.fail(detail, "The workspace was not removed.")
                }
            }
            Phase::Reading if self.kind_read == Some(id) && !done => {
                self.kind_read = None;
                self.fail(detail, "The workspace could not be read.")
            }
            _ => Step::Nothing,
        }
    }

    fn fail(&mut self, detail: Option<&str>, default: &str) -> Step {
        self.phase = Phase::Failed(detail.unwrap_or(default).to_owned());
        Step::Show
    }

    fn leave(&mut self) -> Step {
        self.phase = Phase::Exit;
        Step::Exit
    }
}

/// The windows of this process, for the Quit action. Each window is a process of its own, so the
/// list holds one view in practice.
#[derive(Default)]
pub(crate) struct OpenViews(pub(crate) Vec<WeakEntity<StatusView>>);

impl Global for OpenViews {}

/// Hooks the window's close into the check: the window holds its close until the model says
/// `Exit`. The view is kept so the Quit action reaches it too.
pub(crate) fn register(view: &Entity<StatusView>, window: &mut Window, cx: &mut App) {
    let weak = view.downgrade();
    let for_close = weak.clone();
    window.on_window_should_close(cx, move |_, cx| {
        for_close
            .update(cx, StatusView::close_requested)
            .unwrap_or(true)
    });
    cx.global_mut::<OpenViews>().0.push(weak);
}

/// The Quit action: every window is asked, and the app quits only when none holds.
pub(crate) fn quit_requested(cx: &mut App) {
    let views = cx.global::<OpenViews>().0.clone();
    let mut quit = true;
    for view in views {
        quit &= view.update(cx, StatusView::close_requested).unwrap_or(true);
    }
    if quit {
        cx.quit();
    }
}

impl StatusView {
    /// Asks the kind of the open repository. Sent at each change of repository and on Refresh.
    pub(crate) fn read_workspace_kind(&mut self) {
        self.send_close_order(&Order::ReadKind);
    }

    /// Whether the window may close now. Otherwise the exit is held, and its order is sent.
    pub(crate) fn close_requested(&mut self, cx: &mut Context<Self>) -> bool {
        let root = self.model.status().map(|status| status.root.clone());
        let step = self.model.workspace_close.request(root.as_deref());
        cx.notify();
        match step {
            Step::Exit => true,
            Step::Send(order) => {
                self.send_close_order(&order);
                false
            }
            Step::Nothing | Step::Show => false,
        }
    }

    /// Runs the steps the answers of the last events produced, after the kind of a new repository
    /// is asked for (D185 (4)).
    pub(crate) fn settle_workspace_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.model.status().map(|status| status.root.clone());
        if self.model.workspace_close.observe_root(root.as_deref()) {
            self.read_workspace_kind();
        }
        let steps = self.model.workspace_close.take_steps();
        if !steps.is_empty() {
            self.apply_close_steps(steps, window, cx);
        }
    }

    fn send_close_order(&mut self, order: &Order) {
        let id = self.engine.dispatch(Actor::Human, order.command());
        self.model.workspace_close.dispatched(order, id);
    }

    fn apply_close_steps(&mut self, steps: Vec<Step>, window: &mut Window, cx: &mut Context<Self>) {
        for step in steps {
            match step {
                Step::Nothing => {}
                Step::Send(order) => self.send_close_order(&order),
                Step::Show => self.close_focus.focus(window),
                Step::Exit => {
                    window.remove_window();
                    cx.quit();
                    return;
                }
            }
        }
        cx.notify();
    }

    fn cancel_workspace_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.model.workspace_close.cancel();
        self.list_focus.focus(window);
        cx.notify();
    }

    fn keep_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let step = self.model.workspace_close.keep();
        self.apply_close_steps(vec![step], window, cx);
    }

    fn confirm_workspace_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let step = self.model.workspace_close.confirm();
        self.apply_close_steps(vec![step], window, cx);
    }

    fn toggle_workspace_close(&mut self, cx: &mut Context<Self>) {
        self.model.workspace_close.toggle();
        cx.notify();
    }

    pub(crate) fn workspace_close_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.cancel_workspace_close(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        div().absolute().inset_0().child(outside).child(
            self.workspace_close_panel(left, cx)
                // Tab has nothing else to reach while the dialog is open.
                .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
                .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation()),
        )
    }

    fn workspace_close_panel(&self, left: f32, cx: &Context<Self>) -> Div {
        let Some(dialog) = self.model.workspace_close.dialog() else {
            return div();
        };
        let (path, lines, error) = match dialog {
            Dialog::Asking { path, blockers, .. } => (path, body_lines(path, blockers), None),
            Dialog::Removing { path } => (path, vec![REMOVING.to_owned()], None),
            Dialog::Failed { path, message } => (path, Vec::new(), Some(message.to_owned())),
        };
        let name = folder_name(path);
        let checkbox = match dialog {
            Dialog::Asking {
                blockers,
                confirmed,
                ..
            } if asks_confirmation(blockers) => Some((confirmed, confirmation_label(blockers))),
            _ => None,
        };
        div()
            .absolute()
            .top(px(TOP))
            .left(px(left))
            .w(px(WIDTH))
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
            .track_focus(&self.close_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.cancel_workspace_close(window, cx),
                    "enter" => this.confirm_workspace_close(window, cx),
                    "space" => this.toggle_workspace_close(cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Close workspace {name}?")),
            )
            .children(
                lines
                    .into_iter()
                    .map(|line| div().text_sm().text_color(theme::text()).child(line)),
            )
            .children(checkbox.map(|(confirmed, label)| {
                confirm_row(
                    "workspace-close-confirm-check",
                    confirmed,
                    label,
                    Self::toggle_workspace_close,
                    cx,
                )
            }))
            .children(
                error.map(|message| div().text_sm().text_color(theme::error()).child(message)),
            )
            .child(Self::close_buttons(dialog, cx))
    }

    fn close_buttons(dialog: Dialog<'_>, cx: &Context<Self>) -> Div {
        let row = div().flex().flex_row().justify_end().items_center().gap_2();
        match dialog {
            Dialog::Asking {
                blockers,
                confirmed,
                ..
            } => row
                .child(button(
                    "workspace-close-cancel",
                    "Cancel",
                    false,
                    true,
                    Self::cancel_workspace_close,
                    cx,
                ))
                .child(button(
                    "workspace-close-keep",
                    "Keep workspace",
                    false,
                    true,
                    Self::keep_workspace,
                    cx,
                ))
                .child(button(
                    "workspace-close-remove",
                    "Remove workspace",
                    true,
                    may_remove(blockers, confirmed),
                    Self::confirm_workspace_close,
                    cx,
                )),
            Dialog::Removing { .. } => row.child(button(
                "workspace-close-removing",
                "Removing…",
                true,
                false,
                Self::cancel_workspace_close,
                cx,
            )),
            Dialog::Failed { .. } => row
                .child(button(
                    "workspace-close-cancel",
                    "Cancel",
                    false,
                    true,
                    Self::cancel_workspace_close,
                    cx,
                ))
                .child(button(
                    "workspace-close-ok",
                    "OK",
                    false,
                    true,
                    Self::cancel_workspace_close,
                    cx,
                )),
        }
    }
}

const WIDTH: f32 = 520.;
/// The dialog hangs this far below the top of the window, as the remove dialog does.
const TOP: f32 = 96.;

/// A button of the dialog. Only an enabled one reacts to a press.
fn button(
    id: &'static str,
    label: &'static str,
    danger: bool,
    enabled: bool,
    on_click: fn(&mut StatusView, &mut Window, &mut Context<StatusView>),
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let base = div()
        .id(id)
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .font_weight(FontWeight::MEDIUM);
    let styled = if danger {
        base.bg(theme::error()).text_color(theme::on_accent())
    } else {
        base.text_color(theme::text())
    };
    styled
        .when(enabled, gpui::Styled::cursor_pointer)
        .when(!enabled, |this| this.opacity(0.5))
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                if enabled {
                    on_click(this, window, cx);
                }
            }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::RemovalBlockerKind;

    const SHUT: &str = "/w/shut";

    fn blocker(kind: RemovalBlockerKind, overridable: bool) -> RemovalBlocker {
        RemovalBlocker {
            kind,
            member: Some("billing".to_owned()),
            detail: format!("{kind:?} detail"),
            overridable,
        }
    }

    fn dirty() -> Vec<RemovalBlocker> {
        vec![blocker(RemovalBlockerKind::MemberDirty, true)]
    }

    fn kind(id: u64, kind: Option<WorkspaceKind>) -> Event {
        Event::WorkspaceKindRead {
            id: OperationId(id),
            kind,
        }
    }

    fn inspected(id: u64, blockers: Vec<RemovalBlocker>) -> Event {
        Event::WorkspaceRemovalInspected {
            id: OperationId(id),
            path: SHUT.to_owned(),
            blockers,
        }
    }

    fn removed(id: u64) -> Event {
        Event::WorkspaceRemoved {
            id: OperationId(id),
            path: SHUT.to_owned(),
        }
    }

    fn operation(id: u64, state: OperationState, detail: Option<&str>) -> Event {
        Event::OperationChanged {
            id: OperationId(id),
            actor: Actor::Human,
            state,
            detail: detail.map(str::to_owned),
        }
    }

    /// A model whose repository is known to be `kind`, with no flow under way.
    fn known(kind: Option<WorkspaceKind>) -> WorkspaceClose {
        let mut close = WorkspaceClose::default();
        close.observe_root(Some(SHUT));
        close.kind = Kind::Known(kind);
        close
    }

    /// Feeds an answer and returns the steps it produced.
    fn answer(close: &mut WorkspaceClose, event: &Event) -> Vec<Step> {
        close.answer(event);
        close.take_steps()
    }

    fn inspect() -> Step {
        Step::Send(Order::Inspect {
            path: SHUT.to_owned(),
        })
    }

    fn remove(force: bool) -> Step {
        Step::Send(Order::Remove {
            path: SHUT.to_owned(),
            force,
        })
    }

    /// Asks an ephemeral workspace to close and sends its inspection under id `1`.
    fn inspecting() -> WorkspaceClose {
        let mut close = known(Some(WorkspaceKind::Ephemeral));
        assert_eq!(close.request(Some(SHUT)), inspect());
        close.dispatched(&Order::Inspect { path: SHUT.into() }, OperationId(1));
        close
    }

    /// Reaches the dialog with `blockers` under the inspection id `1`.
    fn asking(blockers: Vec<RemovalBlocker>) -> WorkspaceClose {
        let mut close = inspecting();
        assert_eq!(
            answer(&mut close, &inspected(1, blockers)),
            vec![Step::Show]
        );
        close
    }

    /// Reaches the removal under id `2` with no blocker.
    fn removing() -> WorkspaceClose {
        let mut close = inspecting();
        assert_eq!(
            answer(&mut close, &inspected(1, Vec::new())),
            vec![remove(false)]
        );
        close.dispatched(
            &Order::Remove {
                path: SHUT.into(),
                force: false,
            },
            OperationId(2),
        );
        close
    }

    #[test]
    fn a_durable_workspace_closes_at_once_without_a_read() {
        let mut close = known(Some(WorkspaceKind::Durable));
        assert_eq!(close.request(Some(SHUT)), Step::Exit);
        assert!(close.take_steps().is_empty());
        assert_eq!(close.kind_read, None);
    }

    #[test]
    fn a_plain_repository_closes_at_once_even_without_a_path() {
        let mut close = known(None);
        assert_eq!(close.request(None), Step::Exit);
    }

    #[test]
    fn an_unknown_kind_is_read_and_then_a_durable_one_closes() {
        let mut close = WorkspaceClose::default();
        assert_eq!(close.request(Some(SHUT)), Step::Send(Order::ReadKind));
        close.dispatched(&Order::ReadKind, OperationId(5));
        assert_eq!(
            answer(&mut close, &kind(5, Some(WorkspaceKind::Durable))),
            vec![Step::Exit]
        );
    }

    #[test]
    fn a_read_during_an_exit_request_sends_the_inspection_when_it_is_ephemeral() {
        let mut close = WorkspaceClose::default();
        assert_eq!(close.request(Some(SHUT)), Step::Send(Order::ReadKind));
        close.dispatched(&Order::ReadKind, OperationId(5));
        let steps = answer(&mut close, &kind(5, Some(WorkspaceKind::Ephemeral)));
        assert_eq!(steps, vec![inspect()]);
        close.dispatched(&Order::Inspect { path: SHUT.into() }, OperationId(6));
        assert_eq!(
            answer(&mut close, &inspected(6, Vec::new())),
            vec![remove(false)]
        );
    }

    #[test]
    fn a_read_that_finds_an_ephemeral_workspace_with_no_path_holds_the_exit() {
        let mut close = WorkspaceClose::default();
        assert_eq!(close.request(None), Step::Send(Order::ReadKind));
        close.dispatched(&Order::ReadKind, OperationId(5));
        assert!(answer(&mut close, &kind(5, Some(WorkspaceKind::Ephemeral))).is_empty());
        assert!(close.dialog().is_none());
        // The next request, with the status loaded, inspects it.
        assert_eq!(close.request(Some(SHUT)), inspect());
    }

    #[test]
    fn a_read_for_a_plain_repository_closes_the_window() {
        let mut close = WorkspaceClose::default();
        assert_eq!(close.request(Some(SHUT)), Step::Send(Order::ReadKind));
        close.dispatched(&Order::ReadKind, OperationId(5));
        assert_eq!(answer(&mut close, &kind(5, None)), vec![Step::Exit]);
    }

    #[test]
    fn a_request_while_the_kind_is_being_read_waits_for_the_read() {
        let mut close = WorkspaceClose::default();
        assert_eq!(close.request(Some(SHUT)), Step::Send(Order::ReadKind));
        close.dispatched(&Order::ReadKind, OperationId(5));
        assert_eq!(close.request(Some(SHUT)), Step::Nothing);
        assert_eq!(answer(&mut close, &kind(5, None)), vec![Step::Exit]);
    }

    #[test]
    fn a_request_for_an_unknown_kind_does_not_read_twice_while_a_read_is_in_flight() {
        let mut close = WorkspaceClose::default();
        close.dispatched(&Order::ReadKind, OperationId(3));
        assert_eq!(close.request(Some(SHUT)), Step::Nothing);
        assert_eq!(answer(&mut close, &kind(3, None)), vec![Step::Exit]);
    }

    #[test]
    fn an_ephemeral_workspace_with_nothing_to_lose_is_removed_then_the_window_closes() {
        let mut close = inspecting();
        assert_eq!(
            answer(&mut close, &inspected(1, Vec::new())),
            vec![remove(false)]
        );
        assert!(close.dialog().is_some_and(|dialog| matches!(
            dialog,
            Dialog::Removing { path } if path == SHUT
        )));
        close.dispatched(
            &Order::Remove {
                path: SHUT.into(),
                force: false,
            },
            OperationId(2),
        );
        assert_eq!(answer(&mut close, &removed(2)), vec![Step::Exit]);
    }

    #[test]
    fn the_removal_done_before_its_event_closes_the_window_once() {
        let mut close = removing();
        assert_eq!(
            answer(&mut close, &operation(2, OperationState::Done, None)),
            vec![Step::Exit]
        );
        assert!(answer(&mut close, &removed(2)).is_empty());
    }

    #[test]
    fn a_done_inspection_waits_for_its_answer() {
        let mut close = inspecting();
        assert!(answer(&mut close, &operation(1, OperationState::Done, None)).is_empty());
        assert_eq!(
            answer(&mut close, &inspected(1, Vec::new())),
            vec![remove(false)]
        );
    }

    #[test]
    fn blockers_ask_through_the_dialog_and_keep_exits_without_removing() {
        let mut close = asking(dirty());
        assert_eq!(
            close.dialog(),
            Some(Dialog::Asking {
                path: SHUT,
                blockers: dirty().as_slice(),
                confirmed: false,
            })
        );
        assert_eq!(close.keep(), Step::Exit);
        assert!(close.take_steps().is_empty(), "nothing was removed");
    }

    #[test]
    fn remove_needs_the_confirmation_and_sends_it_as_the_force() {
        let mut close = asking(dirty());
        assert_eq!(close.confirm(), Step::Nothing, "not ticked");
        close.toggle();
        assert_eq!(close.confirm(), remove(true));
        assert!(matches!(close.dialog(), Some(Dialog::Removing { .. })));
    }

    #[test]
    fn ticking_twice_unticks_the_confirmation() {
        let mut close = asking(dirty());
        close.toggle();
        close.toggle();
        assert_eq!(
            close.dialog(),
            Some(Dialog::Asking {
                path: SHUT,
                blockers: dirty().as_slice(),
                confirmed: false,
            })
        );
        assert_eq!(close.confirm(), Step::Nothing);
    }

    #[test]
    fn a_non_overridable_blocker_cannot_be_removed_even_when_ticked() {
        let blockers = vec![blocker(RemovalBlockerKind::MemberRepositoryMissing, false)];
        let mut close = asking(blockers.clone());
        close.toggle();
        assert_eq!(
            close.dialog(),
            Some(Dialog::Asking {
                path: SHUT,
                blockers: blockers.as_slice(),
                confirmed: false,
            }),
            "the checkbox is not offered"
        );
        assert_eq!(close.confirm(), Step::Nothing);
        assert_eq!(close.keep(), Step::Exit);
    }

    #[test]
    fn cancel_returns_to_idle_and_the_next_request_inspects_again() {
        let mut close = asking(dirty());
        assert_eq!(close.cancel(), Step::Nothing);
        assert!(close.dialog().is_none());
        assert_eq!(close.request(Some(SHUT)), inspect());
    }

    #[test]
    fn a_removal_that_fails_keeps_the_window_with_the_engine_message() {
        let mut close = removing();
        assert_eq!(
            answer(
                &mut close,
                &operation(
                    2,
                    OperationState::Failed,
                    Some("cannot remove billing: dirty")
                )
            ),
            vec![Step::Show]
        );
        assert_eq!(
            close.dialog(),
            Some(Dialog::Failed {
                path: SHUT,
                message: "cannot remove billing: dirty",
            })
        );
        assert_eq!(close.request(Some(SHUT)), Step::Nothing, "held while shown");
        assert_eq!(close.confirm(), Step::Nothing, "OK is a cancel");
        assert!(close.dialog().is_none(), "OK returns to idle");
        assert_eq!(
            close.request(Some(SHUT)),
            inspect(),
            "and a new request starts again"
        );
    }

    #[test]
    fn a_failure_without_a_reason_says_what_was_not_done() {
        let mut close = removing();
        assert_eq!(
            answer(&mut close, &operation(2, OperationState::Failed, None)),
            vec![Step::Show]
        );
        assert_eq!(
            close.dialog(),
            Some(Dialog::Failed {
                path: SHUT,
                message: "The workspace was not removed.",
            })
        );
    }

    #[test]
    fn an_inspection_that_fails_shows_the_failure_and_cancel_returns_to_idle() {
        let mut close = inspecting();
        assert_eq!(
            answer(&mut close, &operation(1, OperationState::Failed, None)),
            vec![Step::Show]
        );
        assert_eq!(
            close.dialog(),
            Some(Dialog::Failed {
                path: SHUT,
                message: "The workspace could not be read.",
            })
        );
        assert_eq!(close.cancel(), Step::Nothing);
        assert!(close.dialog().is_none());
    }

    #[test]
    fn a_failed_kind_read_shows_the_failure_instead_of_holding_the_window_forever() {
        let mut close = WorkspaceClose::default();
        assert_eq!(close.request(Some(SHUT)), Step::Send(Order::ReadKind));
        close.dispatched(&Order::ReadKind, OperationId(5));
        assert_eq!(
            answer(&mut close, &operation(5, OperationState::Failed, None)),
            vec![Step::Show]
        );
        assert!(matches!(close.dialog(), Some(Dialog::Failed { .. })));
    }

    #[test]
    fn late_and_repeated_answers_of_other_operations_are_ignored() {
        let mut close = inspecting();
        // An inspection answer under another id, and a removal answer while inspecting.
        assert!(answer(&mut close, &inspected(9, dirty())).is_empty());
        assert!(answer(&mut close, &removed(1)).is_empty());
        assert!(answer(&mut close, &operation(9, OperationState::Failed, None)).is_empty());
        assert_eq!(answer(&mut close, &inspected(1, dirty())), vec![Step::Show]);
        // A repeat of the answer once the dialog is up.
        assert!(answer(&mut close, &inspected(1, Vec::new())).is_empty());
        assert!(matches!(close.dialog(), Some(Dialog::Asking { .. })));
    }

    #[test]
    fn a_kind_read_answer_under_another_id_is_ignored() {
        let mut close = WorkspaceClose::default();
        close.dispatched(&Order::ReadKind, OperationId(7));
        assert!(answer(&mut close, &kind(3, Some(WorkspaceKind::Durable))).is_empty());
        assert_eq!(close.kind, Kind::Unknown);
        assert_eq!(answer(&mut close, &kind(7, None)), Vec::<Step>::new());
        assert_eq!(close.kind, Kind::Known(None));
    }

    #[test]
    fn a_repeated_kind_answer_does_not_run_the_exit_twice() {
        let mut close = WorkspaceClose::default();
        assert_eq!(close.request(Some(SHUT)), Step::Send(Order::ReadKind));
        close.dispatched(&Order::ReadKind, OperationId(5));
        assert_eq!(answer(&mut close, &kind(5, None)), vec![Step::Exit]);
        assert!(answer(&mut close, &kind(5, None)).is_empty());
        assert_eq!(close.request(Some(SHUT)), Step::Exit);
    }

    #[test]
    fn a_request_while_inspecting_or_removing_changes_nothing() {
        let mut close = inspecting();
        assert_eq!(close.request(Some(SHUT)), Step::Nothing);
        assert_eq!(
            answer(&mut close, &inspected(1, Vec::new())),
            vec![remove(false)]
        );
        close.dispatched(
            &Order::Remove {
                path: SHUT.into(),
                force: false,
            },
            OperationId(2),
        );
        assert_eq!(close.request(Some(SHUT)), Step::Nothing);
        assert!(close.take_steps().is_empty());
    }

    #[test]
    fn a_request_while_the_dialog_is_up_changes_nothing() {
        let mut close = asking(dirty());
        assert_eq!(close.request(Some(SHUT)), Step::Nothing);
        assert!(matches!(close.dialog(), Some(Dialog::Asking { .. })));
    }

    #[test]
    fn exit_is_final_and_later_answers_do_nothing() {
        let mut close = removing();
        assert_eq!(answer(&mut close, &removed(2)), vec![Step::Exit]);
        assert_eq!(close.request(Some(SHUT)), Step::Exit);
        assert!(answer(&mut close, &inspected(1, dirty())).is_empty());
        assert!(close.dialog().is_none());
    }

    #[test]
    fn observe_root_reports_each_change_of_repository_once() {
        let mut close = WorkspaceClose::default();
        assert!(!close.observe_root(None), "nothing is shown yet");
        assert!(close.observe_root(Some("/w/a")), "the first repository");
        assert!(!close.observe_root(Some("/w/a")), "the same again");
        assert!(
            !close.observe_root(None),
            "no repository is not a repository to read"
        );
        assert!(close.observe_root(Some("/w/a")), "back on one is a change");
    }

    #[test]
    fn a_different_repository_drops_a_dialog_and_reads_the_kind_again() {
        let mut close = asking(dirty());
        assert!(close.observe_root(Some("/w/other")));
        assert!(close.dialog().is_none());
        assert_eq!(close.request(Some("/w/other")), Step::Send(Order::ReadKind));
    }

    #[test]
    fn a_read_in_flight_of_the_old_repository_is_not_taken() {
        let mut close = known(Some(WorkspaceKind::Durable));
        close.dispatched(&Order::ReadKind, OperationId(4));
        close.observe_root(Some("/w/other"));
        close.dispatched(&Order::ReadKind, OperationId(8));
        assert!(answer(&mut close, &kind(4, Some(WorkspaceKind::Ephemeral))).is_empty());
        assert_eq!(close.kind, Kind::Unknown);
        assert_eq!(answer(&mut close, &kind(8, None)), Vec::<Step>::new());
        assert_eq!(close.kind, Kind::Known(None));
    }

    #[test]
    fn a_repository_change_during_the_read_drops_the_waiting_exit() {
        let mut close = WorkspaceClose::default();
        assert_eq!(close.request(Some(SHUT)), Step::Send(Order::ReadKind));
        close.dispatched(&Order::ReadKind, OperationId(5));
        close.observe_root(Some("/w/other"));
        assert_eq!(close.request(Some("/w/other")), Step::Send(Order::ReadKind));
    }

    #[test]
    fn the_orders_send_the_commands_as_a_closing_window() {
        assert_eq!(Order::ReadKind.command(), CommandKind::ReadWorkspaceKind);
        assert_eq!(
            Order::Inspect { path: SHUT.into() }.command(),
            CommandKind::InspectWorkspaceRemoval {
                path: SHUT.into(),
                closing: true,
            }
        );
        assert_eq!(
            Order::Remove {
                path: SHUT.into(),
                force: true,
            }
            .command(),
            CommandKind::RemoveWorkspace {
                path: SHUT.into(),
                force: true,
                closing: true,
            }
        );
    }

    #[test]
    fn the_steps_of_answers_wait_in_order_until_taken() {
        let mut close = inspecting();
        close.answer(&inspected(1, dirty()));
        close.answer(&operation(1, OperationState::Done, None));
        assert_eq!(close.take_steps(), vec![Step::Show]);
        assert!(close.take_steps().is_empty());
    }
}
