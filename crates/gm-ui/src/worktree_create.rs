//! The create-worktree dialog (slice 1c, W3; D31). The rules are pure and
//! tested on their own here: what the form needs, the folder it suggests, the
//! command it runs and what the outcome of a run means. The window side is the
//! `impl StatusView` at the bottom: the layer, the fields and the submit. The
//! create runs on the engine thread like every other command.

use std::path::Path;

use gm_schema::{Actor, BranchEntry, CommandKind, OperationId, OperationState, WorktreeMode};
use gpui::{
    App, Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, Stateful, Subscription,
    Window, div, prelude::*, px,
};

use crate::FocusNext;
use crate::fuzzy;
use crate::text_area::{Down, Newline, TextArea, TextAreaEvent, Up};
use crate::theme;
use crate::view::StatusView;
use crate::worktree_list;

const WIDTH: f32 = 560.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;
/// The message shown when a folder is missing from the form.
const NO_FOLDER: &str = "Choose a folder for the worktree.";
/// The most branches the branch field lists under it. Typing narrows the list.
pub(crate) const SUGGESTIONS_SHOWN: usize = 8;

/// The three ways a new worktree can start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    /// An existing local branch, checked out in the new folder.
    #[default]
    Existing,
    /// A new branch, created at a commit and checked out in the new folder.
    New,
    /// A detached HEAD at a commit.
    Detached,
}

impl Mode {
    pub(crate) const ALL: [Mode; 3] = [Mode::Existing, Mode::New, Mode::Detached];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Mode::Existing => "Existing branch",
            Mode::New => "New branch",
            Mode::Detached => "Detached HEAD",
        }
    }
}

/// The text of the form. Untrimmed, as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Form {
    pub(crate) mode: Mode,
    /// The existing branch to check out.
    pub(crate) branch: String,
    /// The name of the new branch.
    pub(crate) name: String,
    /// The commit the new branch or the detached HEAD starts from.
    pub(crate) start: String,
    /// The folder the worktree is created in.
    pub(crate) path: String,
}

impl Form {
    /// What the folder is named after: the branch, the new name, or "detached".
    fn label(&self) -> &str {
        match self.mode {
            Mode::Existing => &self.branch,
            Mode::New => &self.name,
            Mode::Detached => "detached",
        }
    }

    /// The folder the form suggests for its current name and mode. Nothing is
    /// suggested until there is a name to put in it.
    pub(crate) fn suggested_path(&self, root: &str) -> String {
        if self.label().trim().is_empty() {
            return String::new();
        }
        default_path(root, self.label())
    }
}

/// The local branches the branch field lists for `query`, best match first and
/// at most `SUGGESTIONS_SHOWN`. An empty query lists them in the order given.
/// Remote branches are left out: the dialog checks out a local one.
pub(crate) fn suggest_branches(query: &str, branches: &[BranchEntry]) -> Vec<String> {
    let mut scored: Vec<(i64, &str)> = branches
        .iter()
        .filter(|branch| !branch.is_remote)
        .filter_map(|branch| {
            fuzzy::fuzzy_match(query, &branch.name).map(|found| (found.score, branch.name.as_str()))
        })
        .collect();
    // A stable sort: equal scores keep the order the branches were listed in.
    scored.sort_by_key(|a| std::cmp::Reverse(a.0));
    scored
        .into_iter()
        .take(SUGGESTIONS_SHOWN)
        .map(|(_, name)| name.to_owned())
        .collect()
}

/// The suggestion Up or Down moves to, among `count` rows. Nothing is
/// highlighted until the first move, so Enter still submits what was typed.
/// The moves stop at both ends.
pub(crate) fn move_pick(current: Option<usize>, count: usize, down: bool) -> Option<usize> {
    let last = count.checked_sub(1)?;
    Some(match (current.map(|at| at.min(last)), down) {
        (None, true) => 0,
        (None, false) => last,
        (Some(at), true) => (at + 1).min(last),
        (Some(at), false) => at.saturating_sub(1),
    })
}

/// The part of the form that the observers compare.
fn seen_of(form: &Form) -> (Mode, String, String, String) {
    (
        form.mode,
        form.branch.clone(),
        form.name.clone(),
        form.start.clone(),
    )
}

/// The folder a worktree of `label` gets by default: a sibling of the
/// repository, named `<repo>-<label>`, with `/` in the label made `-`. A label
/// with no usable character gives `<repo>-worktree`, never the repository itself.
pub(crate) fn default_path(root: &str, label: &str) -> String {
    let root = Path::new(root.trim_end_matches(['/', '\\']));
    let repo = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("worktree");
    let folder = match folder_label(label) {
        tail if tail.is_empty() => format!("{repo}-worktree"),
        tail => format!("{repo}-{tail}"),
    };
    match root.parent() {
        Some(parent) => parent.join(folder).to_string_lossy().into_owned(),
        None => folder,
    }
}

/// A label made safe for one folder name.
fn folder_label(label: &str) -> String {
    label
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

/// The path the field shows after a change of the name or the mode. A folder
/// the user typed is kept: only the suggestion the field held is replaced.
pub(crate) fn refresh_path(current: &str, previous: &str, next: &str) -> String {
    if current == previous {
        next.to_owned()
    } else {
        current.to_owned()
    }
}

/// The form as a command: its folder and mode, trimmed. Or the reason it cannot
/// run yet, for the first missing field.
pub(crate) fn build(form: &Form) -> Result<(String, WorktreeMode), &'static str> {
    let mode = match form.mode {
        Mode::Existing => {
            let branch = form.branch.trim();
            if branch.is_empty() {
                return Err("Choose the branch to check out.");
            }
            WorktreeMode::Existing {
                branch: branch.to_owned(),
            }
        }
        Mode::New => {
            let name = form.name.trim();
            if name.is_empty() {
                return Err("Name the new branch.");
            }
            let base = form.start.trim();
            if base.is_empty() {
                return Err("Choose the commit the new branch starts from.");
            }
            WorktreeMode::New {
                branch: name.to_owned(),
                base: base.to_owned(),
            }
        }
        Mode::Detached => {
            let commit = form.start.trim();
            if commit.is_empty() {
                return Err("Choose the commit to check out.");
            }
            WorktreeMode::Detached {
                commit: commit.to_owned(),
            }
        }
    };
    // The branch, name or commit comes first: it is the field the dialog opens on.
    let path = form.path.trim();
    if path.is_empty() {
        return Err(NO_FOLDER);
    }
    Ok((path.to_owned(), mode))
}

/// The Git command the form will run, shown under the fields. Missing parts
/// read as the placeholders the fields use.
pub(crate) fn command_line(form: &Form) -> String {
    let path = match form.path.trim() {
        "" => "<folder>",
        path => path,
    };
    let or = |text: &str, placeholder: &'static str| match text.trim() {
        "" => placeholder.to_owned(),
        text => text.to_owned(),
    };
    match form.mode {
        Mode::Existing => format!("git worktree add {path} {}", or(&form.branch, "<branch>")),
        Mode::New => format!(
            "git worktree add -b {} {path} {}",
            or(&form.name, "<branch>"),
            or(&form.start, "<commit>")
        ),
        Mode::Detached => format!(
            "git worktree add --detach {path} {}",
            or(&form.start, "<commit>")
        ),
    }
}

/// The create in flight, and its outcome until the window takes it. Engine
/// events for other operations never touch it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Creating {
    pending: Option<(OperationId, String)>,
    outcome: Option<Result<String, String>>,
}

impl Creating {
    pub(crate) fn start(&mut self, id: OperationId, path: String) {
        self.pending = Some((id, path));
        self.outcome = None;
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|(pending, _)| *pending == id)
    }

    /// Ends the create with the operation's terminal `state`. A success gives
    /// the path; anything else gives Git's first reason, or a plain one.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, path)) = self.pending.take() else {
            return;
        };
        self.outcome = Some(if state == OperationState::Done {
            Ok(path)
        } else {
            Err(detail.unwrap_or_else(|| "the worktree was not created".to_owned()))
        });
    }

    pub(crate) fn take(&mut self) -> Option<Result<String, String>> {
        self.outcome.take()
    }
}

/// The dialog while it is open: its fields, its mode and the error it shows.
pub(crate) struct WorktreeDialog {
    pub(crate) mode: Mode,
    pub(crate) branch: Entity<TextArea>,
    pub(crate) name: Entity<TextArea>,
    pub(crate) start: Entity<TextArea>,
    pub(crate) path: Entity<TextArea>,
    /// The repository root the suggestions are made from.
    pub(crate) root: String,
    /// The folder the form suggested last; a path that still equals it follows
    /// the form, one the user typed does not.
    suggested: String,
    /// The mode and the three texts the form last showed. The fields notify on
    /// every caret blink, so a change counts only when these differ.
    seen: (Mode, String, String, String),
    pub(crate) error: Option<String>,
    /// The branch suggestion Up and Down highlight; `None` until one is moved to.
    pub(crate) pick: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl WorktreeDialog {
    pub(crate) fn form(&self, cx: &gpui::App) -> Form {
        Form {
            mode: self.mode,
            branch: self.branch.read(cx).text().to_owned(),
            name: self.name.read(cx).text().to_owned(),
            start: self.start.read(cx).text().to_owned(),
            path: self.path.read(cx).text().to_owned(),
        }
    }

    /// The fields in the order Tab visits them, for the current mode.
    fn fields(&self) -> Vec<&Entity<TextArea>> {
        match self.mode {
            Mode::Existing => vec![&self.branch, &self.path],
            Mode::New => vec![&self.name, &self.start, &self.path],
            Mode::Detached => vec![&self.start, &self.path],
        }
    }
}

impl StatusView {
    /// Opens the dialog in the existing-branch mode. `branch` is the branch the
    /// caller chose; without one the branch field starts empty, to be typed or
    /// picked from the suggestions, and the folder follows the branch.
    pub(crate) fn open_worktree_dialog(
        &mut self,
        branch: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_worktree_dialog_in(Mode::Existing, branch, window, cx);
    }

    /// Opens the dialog in `mode`, as `open_worktree_dialog` does for the
    /// existing branch. A drop on the Worktrees header opens the new-branch mode.
    pub(crate) fn open_worktree_dialog_in(
        &mut self,
        mode: Mode,
        branch: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self
            .model
            .status()
            .map(|status| status.root.clone())
            .unwrap_or_default();
        let head = self
            .model
            .status()
            .and_then(|status| status.branch.head.clone())
            .unwrap_or_else(|| "HEAD".to_owned());
        let field = |placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| TextArea::new(placeholder, cx).with_lines(1, 1))
        };
        let branch_field = field("Branch to check out", cx);
        let name_field = field("New branch name", cx);
        let start_field = field("Commit or branch", cx);
        let path_field = field("Folder", cx);
        let mut subscriptions = Vec::new();
        for area in [&branch_field, &name_field, &start_field] {
            subscriptions.push(cx.observe(area, |this, _, cx| this.on_worktree_field(cx)));
        }
        for area in [&branch_field, &name_field, &start_field, &path_field] {
            subscriptions.push(cx.subscribe_in(
                area,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    if *event == TextAreaEvent::Cancel {
                        this.close_worktree_dialog(window, cx);
                    }
                },
            ));
        }
        if let Some(branch) = branch {
            branch_field.update(cx, |area, cx| area.set_text(branch, cx));
        }
        start_field.update(cx, |area, cx| area.set_text(&head, cx));
        let mut dialog = WorktreeDialog {
            mode,
            branch: branch_field,
            name: name_field,
            start: start_field,
            path: path_field,
            root,
            suggested: String::new(),
            seen: (mode, String::new(), String::new(), String::new()),
            error: None,
            pick: None,
            _subscriptions: subscriptions,
        };
        let form = dialog.form(cx);
        let suggested = form.suggested_path(&dialog.root);
        dialog
            .path
            .update(cx, |area, cx| area.set_text(&suggested, cx));
        dialog.suggested = suggested;
        dialog.seen = seen_of(&form);
        let focus = dialog.fields()[0].clone();
        self.worktree_dialog = Some(dialog);
        focus.update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn close_worktree_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A file waiting for the worktree goes with the dialog, unless a create is still running.
        if !self.model.creating.is_pending() {
            self.model.move_file.after_create = None;
        }
        if self.worktree_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// A field changed: the suggested folder follows it, unless the user typed one.
    fn on_worktree_field(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.worktree_dialog.as_mut() else {
            return;
        };
        let form = dialog.form(cx);
        if dialog.seen == seen_of(&form) {
            return;
        }
        dialog.seen = seen_of(&form);
        dialog.pick = None;
        let next = form.suggested_path(&dialog.root);
        let shown = refresh_path(&form.path, &dialog.suggested, &next);
        dialog.suggested = next;
        if shown != form.path {
            dialog.path.update(cx, |area, cx| area.set_text(&shown, cx));
        }
        dialog.error = None;
        cx.notify();
    }

    /// The local branches the branch field lists now, in the existing-branch mode.
    fn branch_suggestions(&self, dialog: &WorktreeDialog, cx: &App) -> Vec<String> {
        if dialog.mode != Mode::Existing {
            return Vec::new();
        }
        let typed = dialog.branch.read(cx).text();
        suggest_branches(typed, self.model.branches.known())
    }

    /// Up or Down with suggestions on screen: moves the highlight. Returns
    /// whether it did, so the keys go on to the field when it did not.
    pub(crate) fn move_worktree_pick(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some(dialog) = self.worktree_dialog.as_ref() else {
            return false;
        };
        let count = self.branch_suggestions(dialog, cx).len();
        if count == 0 {
            return false;
        }
        let pick = move_pick(dialog.pick, count, down);
        if let Some(dialog) = self.worktree_dialog.as_mut() {
            dialog.pick = pick;
        }
        cx.notify();
        true
    }

    /// Enter: takes the highlighted suggestion, or else sends the create.
    fn worktree_enter(&mut self, cx: &mut Context<Self>) {
        let chosen = self.worktree_dialog.as_ref().and_then(|dialog| {
            let at = dialog.pick?;
            self.branch_suggestions(dialog, cx).into_iter().nth(at)
        });
        match chosen {
            Some(name) => self.choose_worktree_branch(&name, cx),
            None => self.submit_worktree(cx),
        }
    }

    /// Puts `name` in the branch field. The field's observer then moves the
    /// suggested folder to match.
    fn choose_worktree_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = self.worktree_dialog.as_mut() {
            dialog.pick = None;
            dialog.branch.update(cx, |area, cx| area.set_text(name, cx));
        }
        cx.notify();
    }

    pub(crate) fn set_worktree_mode(
        &mut self,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.worktree_dialog.as_mut() else {
            return;
        };
        dialog.mode = mode;
        dialog.error = None;
        dialog.pick = None;
        let focus = dialog.fields()[0].clone();
        focus.update(cx, |area, cx| area.focus(window, cx));
        self.on_worktree_field(cx);
    }

    /// Enter and the create button: checks the form, then sends the create.
    pub(crate) fn submit_worktree(&mut self, cx: &mut Context<Self>) {
        if self.model.creating.is_pending() {
            return;
        }
        let Some(dialog) = self.worktree_dialog.as_mut() else {
            return;
        };
        let form = dialog.form(cx);
        match build(&form) {
            Err(reason) => dialog.error = Some(reason.to_owned()),
            Ok((path, mode)) => {
                dialog.error = None;
                let id = self.engine.dispatch(
                    Actor::Human,
                    CommandKind::CreateWorktree {
                        path: path.clone(),
                        mode,
                    },
                );
                self.model.creating.start(id, path);
            }
        }
        cx.notify();
    }

    /// Runs when the engine settles a create: success closes the dialog and
    /// names the new folder; a failure stays in the dialog with Git's message.
    pub(crate) fn on_worktree_created(
        &mut self,
        outcome: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(path) => {
                // The dialog's field held the focus; the list takes it back.
                self.worktree_dialog = None;
                self.list_focus.focus(window);
                self.notify_user(&format!("Created worktree at {path}"), cx);
                self.move_after_create(&path, cx);
            }
            // Closed while the create ran: the reason goes to the status bar instead.
            Err(message) => {
                if let Some(dialog) = self.worktree_dialog.as_mut() {
                    dialog.error = Some(message);
                } else {
                    self.model.move_file.after_create = None;
                    self.notify_user(&message, cx);
                }
            }
        }
        cx.notify();
    }

    fn next_worktree_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.worktree_dialog.as_ref() else {
            return;
        };
        let fields = dialog.fields();
        let current = fields
            .iter()
            .position(|area| area.read(cx).is_focused(window))
            .unwrap_or(fields.len() - 1);
        let next = fields[(current + 1) % fields.len()].clone();
        next.update(cx, |area, cx| area.focus(window, cx));
    }

    pub(crate) fn worktree_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let Some(dialog) = self.worktree_dialog.as_ref() else {
            return div();
        };
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_worktree_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        let panel = self.worktree_panel(dialog, left, cx);
        let view = cx.entity().downgrade();
        let tab = view.clone();
        let up = view.clone();
        let down = view.clone();
        let enter = view;
        div().absolute().inset_0().child(outside).child(
            panel
                .capture_action(move |_: &FocusNext, window, cx| {
                    let _ = tab.update(cx, |this, cx| this.next_worktree_field(window, cx));
                    cx.stop_propagation();
                })
                // Up and Down are kept only while suggestions are on screen; the
                // field keeps them otherwise.
                .capture_action(move |_: &Up, _, cx| {
                    if matches!(
                        up.update(cx, |this, cx| this.move_worktree_pick(false, cx)),
                        Ok(true)
                    ) {
                        cx.stop_propagation();
                    }
                })
                .capture_action(move |_: &Down, _, cx| {
                    if matches!(
                        down.update(cx, |this, cx| this.move_worktree_pick(true, cx)),
                        Ok(true)
                    ) {
                        cx.stop_propagation();
                    }
                })
                .capture_action(move |_: &Newline, _, cx| {
                    let _ = enter.update(cx, StatusView::worktree_enter);
                    cx.stop_propagation();
                }),
        )
    }

    fn worktree_panel(&self, dialog: &WorktreeDialog, left: f32, cx: &Context<Self>) -> Div {
        let pending = self.model.creating.is_pending();
        let form = dialog.form(cx);
        let elsewhere = match (dialog.error.is_some(), dialog.mode) {
            (true, Mode::Existing) => self.checked_out_elsewhere(form.branch.trim()),
            _ => None,
        };
        let suggestions = self.branch_suggestions(dialog, cx);
        let list =
            (!suggestions.is_empty()).then(|| suggestion_list(&suggestions, dialog.pick, cx));
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
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("New worktree"),
            )
            .child(mode_switch(dialog.mode, cx))
            .child(match dialog.mode {
                Mode::Existing => field_row("BRANCH", dialog.branch.clone()),
                Mode::New => field_row("NEW BRANCH", dialog.name.clone()),
                Mode::Detached => field_row("COMMIT", dialog.start.clone()),
            })
            .children(list)
            .children(
                (dialog.mode == Mode::New).then(|| field_row("STARTS FROM", dialog.start.clone())),
            )
            .child(field_row("FOLDER", dialog.path.clone()))
            .child(command_block(&command_line(&form)))
            .children(dialog.error.as_ref().map(|error| {
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(error.clone())
            }))
            .children(elsewhere.map(|path| {
                // A row keeps the button at its own width, not the panel's.
                div().flex().flex_row().child(open_button(path, cx))
            }))
            .child(buttons(pending, cx))
    }

    /// The folder of the worktree that has `branch`, when it is not this
    /// window's own: Git refused the branch because that worktree has it.
    fn checked_out_elsewhere(&self, branch: &str) -> Option<String> {
        let root = self.model.status().map(|status| status.root.clone());
        worktree_list::rows(&self.model.worktrees, root.as_deref(), theme::LANE_COUNT)
            .into_iter()
            .find(|row| !row.here && row.entry.branch.as_deref() == Some(branch))
            .map(|row| row.entry.path.clone())
    }

    /// Closes the dialog and opens `path` in its own window.
    fn open_from_dialog(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.close_worktree_dialog(window, cx);
        self.open_worktree(path, cx);
    }
}

/// The local branches the branch field suggests, under the field. The row
/// Up and Down highlight is `pick`; a click takes any row.
fn suggestion_list(names: &[String], pick: Option<usize>, cx: &Context<StatusView>) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded_md()
        .border_1()
        .border_color(theme::hairline())
        .children(names.iter().enumerate().map(|(at, name)| {
            let chosen = name.clone();
            div()
                .id(("worktree-suggestion", at))
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .text_sm()
                .cursor_pointer()
                .text_color(theme::text())
                .when(pick == Some(at), |this| {
                    this.bg(theme::selection()).text_color(theme::accent())
                })
                .child(name.clone())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.choose_worktree_branch(&chosen, cx);
                    }),
                )
        }))
}

/// The button that goes to the worktree which has the branch.
fn open_button(path: String, cx: &Context<StatusView>) -> Stateful<Div> {
    div()
        .id("worktree-open-elsewhere")
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .cursor_pointer()
        .border_1()
        .border_color(theme::hairline_strong())
        .text_color(theme::text())
        .child("Open that worktree")
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.open_from_dialog(&path, window, cx);
            }),
        )
}

/// The three modes as a segmented control.
fn mode_switch(selected_mode: Mode, cx: &Context<StatusView>) -> Div {
    let modes = Mode::ALL.map(|mode| {
        let selected = mode == selected_mode;
        div()
            .id(mode.label())
            .px_3()
            .h(px(24.))
            .flex()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .when(selected, |this| {
                this.bg(theme::accent()).text_color(theme::on_accent())
            })
            .when(!selected, |this| this.text_color(theme::text()))
            .child(mode.label())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.set_worktree_mode(mode, window, cx);
                }),
            )
    });
    div()
        .flex()
        .flex_row()
        .gap_1()
        .p_1()
        .rounded_md()
        .bg(theme::ground())
        .children(modes)
}

/// The Git command the form will run, in the code face.
pub(crate) fn command_block(plan: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(theme::muted()).child("COMMAND"))
        .child(
            div()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(theme::ground())
                .font_family(theme::code_font())
                .text_xs()
                .text_color(theme::text())
                .child(plan.to_owned()),
        )
}

/// Cancel and the create button; the button waits while a create runs.
fn buttons(pending: bool, cx: &Context<StatusView>) -> Div {
    div()
        .flex()
        .flex_row()
        .justify_end()
        .items_center()
        .gap_2()
        .child(
            div()
                .id("worktree-cancel")
                .px_3()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .text_color(theme::text())
                .child("Cancel")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_worktree_dialog(window, cx);
                    }),
                ),
        )
        .child(
            div()
                .id("worktree-create")
                .px_3()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded_md()
                .bg(theme::accent())
                .text_color(theme::on_accent())
                .font_weight(FontWeight::MEDIUM)
                .when(pending, |this| this.opacity(0.5))
                .when(!pending, gpui::Styled::cursor_pointer)
                .child(if pending {
                    "Creating…"
                } else {
                    "Create worktree"
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.submit_worktree(cx);
                    }),
                ),
        )
}

pub(crate) fn field_row(label: &'static str, field: Entity<TextArea>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(theme::muted()).child(label))
        .child(
            div()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(theme::hairline())
                .child(field),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(mode: Mode) -> Form {
        Form {
            mode,
            ..Form::default()
        }
    }

    #[test]
    fn the_default_folder_is_a_sibling_named_after_the_repo_and_branch() {
        assert_eq!(
            default_path("/home/me/code/ledger", "ledger-pay"),
            "/home/me/code/ledger-ledger-pay"
        );
        assert_eq!(
            default_path("/home/me/code/ledger/", "feature/x"),
            "/home/me/code/ledger-feature-x"
        );
    }

    #[test]
    fn a_label_with_no_safe_characters_gets_a_worktree_suffix_never_the_repo_itself() {
        assert_eq!(default_path("/a/ledger", "///"), "/a/ledger-worktree");
        assert_eq!(default_path("/a/ledger", ""), "/a/ledger-worktree");
    }

    #[test]
    fn nothing_is_suggested_until_the_branch_has_a_name() {
        let mut form = form(Mode::Existing);
        assert_eq!(form.suggested_path("/a/ledger"), "");
        form.branch = "  ".into();
        assert_eq!(form.suggested_path("/a/ledger"), "");
        form.branch = "feature".into();
        assert_eq!(form.suggested_path("/a/ledger"), "/a/ledger-feature");
    }

    fn branch(name: &str, is_remote: bool) -> BranchEntry {
        BranchEntry {
            name: name.into(),
            is_current: false,
            is_remote,
            upstream: None,
            author_email: None,
        }
    }

    #[test]
    fn an_empty_branch_field_lists_the_local_branches_in_order_without_remotes() {
        let branches = [
            branch("main", false),
            branch("origin/main", true),
            branch("feature/x", false),
        ];
        assert_eq!(suggest_branches("", &branches), ["main", "feature/x"]);
    }

    #[test]
    fn typing_filters_the_branches_fuzzily_and_the_best_match_comes_first() {
        let branches = [
            branch("feature/login", false),
            branch("main", false),
            branch("fix/login-timeout", false),
        ];
        // The word that starts earlier in the name ranks first.
        assert_eq!(
            suggest_branches("login", &branches),
            ["fix/login-timeout", "feature/login"]
        );
        assert_eq!(suggest_branches("mn", &branches), ["main"]);
        assert!(suggest_branches("zzz", &branches).is_empty());
    }

    #[test]
    fn a_remote_branch_is_never_suggested_even_when_it_matches() {
        let branches = [branch("origin/feature", true)];
        assert!(suggest_branches("feature", &branches).is_empty());
    }

    #[test]
    fn the_suggestions_are_capped_so_the_list_stays_short() {
        let branches: Vec<BranchEntry> = (0..SUGGESTIONS_SHOWN + 5)
            .map(|at| branch(&format!("topic-{at}"), false))
            .collect();
        assert_eq!(suggest_branches("", &branches).len(), SUGGESTIONS_SHOWN);
    }

    #[test]
    fn up_and_down_move_the_pick_and_stop_at_both_ends() {
        assert_eq!(move_pick(None, 0, true), None);
        assert_eq!(move_pick(None, 3, true), Some(0));
        assert_eq!(move_pick(None, 3, false), Some(2));
        assert_eq!(move_pick(Some(0), 3, true), Some(1));
        assert_eq!(move_pick(Some(2), 3, true), Some(2));
        assert_eq!(move_pick(Some(0), 3, false), Some(0));
        // A list that got shorter since the pick was made still lands on a row.
        assert_eq!(move_pick(Some(9), 3, false), Some(1));
        assert_eq!(move_pick(Some(9), 3, true), Some(2));
    }

    #[test]
    fn the_new_worktree_dialog_starts_on_the_existing_branch_mode() {
        assert_eq!(Mode::default(), Mode::Existing);
    }

    #[test]
    fn a_root_without_a_parent_still_gets_a_name() {
        assert_eq!(default_path("/", "x"), "worktree-x");
    }

    #[test]
    fn a_suggested_folder_follows_the_name_until_the_user_types_one() {
        let first = default_path("/a/ledger", "one");
        let second = default_path("/a/ledger", "two");
        assert_eq!(refresh_path(&first, &first, &second), second);
        assert_eq!(refresh_path("/elsewhere", &first, &second), "/elsewhere");
    }

    #[test]
    fn an_existing_branch_needs_a_branch_and_a_folder() {
        let mut form = form(Mode::Existing);
        assert_eq!(build(&form), Err("Choose the branch to check out."));
        form.path = "/a/wt".into();
        assert_eq!(build(&form), Err("Choose the branch to check out."));
        form.path = String::new();
        form.branch = "feature".into();
        assert_eq!(build(&form), Err(NO_FOLDER));
        form.path = "/a/wt".into();
        form.branch = "  feature  ".into();
        assert_eq!(
            build(&form),
            Ok((
                "/a/wt".to_owned(),
                WorktreeMode::Existing {
                    branch: "feature".into()
                }
            ))
        );
    }

    #[test]
    fn a_new_branch_needs_a_name_and_a_start_point() {
        let mut form = form(Mode::New);
        form.path = "/a/wt".into();
        assert_eq!(build(&form), Err("Name the new branch."));
        form.name = "topic".into();
        assert_eq!(
            build(&form),
            Err("Choose the commit the new branch starts from.")
        );
        form.start = "main".into();
        assert_eq!(
            build(&form),
            Ok((
                "/a/wt".to_owned(),
                WorktreeMode::New {
                    branch: "topic".into(),
                    base: "main".into()
                }
            ))
        );
    }

    #[test]
    fn a_detached_worktree_needs_a_commit() {
        let mut form = form(Mode::Detached);
        form.path = "/a/wt".into();
        assert_eq!(build(&form), Err("Choose the commit to check out."));
        form.start = "HEAD~1".into();
        assert_eq!(
            build(&form),
            Ok((
                "/a/wt".to_owned(),
                WorktreeMode::Detached {
                    commit: "HEAD~1".into()
                }
            ))
        );
    }

    #[test]
    fn the_command_line_shows_what_git_will_run() {
        let mut form = form(Mode::New);
        form.name = "topic".into();
        form.start = "main".into();
        form.path = "/a/wt".into();
        assert_eq!(command_line(&form), "git worktree add -b topic /a/wt main");
        form.mode = Mode::Existing;
        form.branch = "feature".into();
        assert_eq!(command_line(&form), "git worktree add /a/wt feature");
        form.mode = Mode::Detached;
        form.start = String::new();
        assert_eq!(
            command_line(&form),
            "git worktree add --detach /a/wt <commit>"
        );
    }

    #[test]
    fn creating_keeps_only_its_own_operation() {
        let mut creating = Creating::default();
        assert!(!creating.is_pending());
        creating.start(OperationId(4), "/a/wt".into());
        assert!(creating.is_pending());
        assert!(!creating.owns(OperationId(3)));
        assert!(creating.owns(OperationId(4)));
        creating.settle(OperationState::Done, None);
        assert_eq!(creating.take(), Some(Ok("/a/wt".to_owned())));
        assert_eq!(creating.take(), None);
    }

    #[test]
    fn a_failed_create_keeps_gits_reason() {
        let mut creating = Creating::default();
        creating.start(OperationId(4), "/a/wt".into());
        creating.settle(
            OperationState::Failed,
            Some("'main' is already checked out at '/r'".into()),
        );
        assert_eq!(
            creating.take(),
            Some(Err("'main' is already checked out at '/r'".to_owned()))
        );
    }

    #[test]
    fn a_failed_create_without_a_reason_says_so() {
        let mut creating = Creating::default();
        creating.start(OperationId(4), "/a/wt".into());
        creating.settle(OperationState::Vetoed, None);
        assert_eq!(
            creating.take(),
            Some(Err("the worktree was not created".to_owned()))
        );
    }
}
