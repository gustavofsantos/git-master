//! The diff of the selected file: unstaged hunks, then staged hunks, with
//! stage and unstage by hunk and by line (D24). Stub: the API is fixed, the
//! body is filled in by the slice 1 diff-pane worker.

use gm_schema::{FileDiff, StageTarget};
use gpui::{Context, EventEmitter, FocusHandle, IntoElement, Render, Window, div};

/// What the pane asks the shell to do; the shell turns it into a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffPaneEvent {
    Stage(StageTarget),
    Unstage(StageTarget),
}

pub struct DiffPane {
    focus: FocusHandle,
}

impl EventEmitter<DiffPaneEvent> for DiffPane {}

impl DiffPane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self { focus: cx.focus_handle() }
    }

    /// Replaces what is shown. `None` for a side means it has no diff (or is
    /// still loading). Selection state resets when the path changes and is kept
    /// when the same path is reloaded and a hunk id is still present.
    pub fn set_diffs(
        &mut self,
        _unstaged: Option<FileDiff>,
        _staged: Option<FileDiff>,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }
}

/// Binds the pane's keys; called once at startup.
pub fn bind_keys(_cx: &mut gpui::App) {}

impl Render for DiffPane {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
