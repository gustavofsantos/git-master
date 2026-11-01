//! The new-branch dialog. It names a local branch, the commit it starts from,
//! and whether the new branch is checked out. Opened by New Child Branch, it
//! names the stack parent instead of a start point: the branch starts at the
//! parent's tip and records it (D138). The rules are pure and tested
//! here: what the form needs, the command it sends, the command line it shows
//! and what an outcome means. The window side is the `impl StatusView` at the
//! bottom. The form refuses the names `gm_core::branch` refuses, so the reason
//! shows while typing; Git stays the final judge (its message shows on failure).

use gm_core::branch;
use gm_schema::{Actor, CommandKind, OperationId, OperationState};
use gpui::{
    App, Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, Stateful, Subscription,
    Window, div, prelude::*, px,
};

use crate::FocusNext;
use crate::gate_wait::{self, GateWait};
use crate::icon::{self, Icon};
use crate::text_area::{Newline, TextArea, TextAreaEvent};
use crate::theme;
use crate::view::StatusView;
use crate::worktree_create::{command_block, field_row};

const WIDTH: f32 = 520.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;
/// The message shown when the name is missing from the form.
const NO_NAME: &str = "Name the new branch.";

/// The text of the form. Untrimmed, as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Form {
    /// The name of the new branch.
    pub(crate) name: String,
    /// The commit, branch or tag the new branch starts from; empty for HEAD.
    pub(crate) start: String,
    /// Whether the new branch is checked out as well.
    pub(crate) checkout: bool,
    /// The stack parent of a new child branch; the start field is not used then.
    pub(crate) parent: Option<String>,
}

/// The reason the typed name cannot be a branch name, shown while typing. An
/// empty name shows nothing: the form says what is missing when it is sent.
#[must_use]
pub(crate) fn name_message(name: &str) -> Option<&'static str> {
    let name = name.trim();
    if name.is_empty() {
        None
    } else {
        branch::name_problem(name)
    }
}

/// The form as the command the engine runs, trimmed. Or the reason it cannot
/// run yet: the first missing or refused field.
pub(crate) fn build(form: &Form) -> Result<CommandKind, &'static str> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err(NO_NAME);
    }
    if let Some(problem) = branch::name_problem(name) {
        return Err(problem);
    }
    if let Some(parent) = &form.parent {
        return Ok(CommandKind::CreateChildBranch {
            name: name.to_owned(),
            parent: parent.clone(),
            checkout: form.checkout,
        });
    }
    let start = form.start.trim();
    let start_point = if start.is_empty() {
        None
    } else if let Some(problem) = branch::start_point_problem(start) {
        return Err(problem);
    } else {
        Some(start.to_owned())
    };
    Ok(CommandKind::CreateBranch {
        name: name.to_owned(),
        start_point,
        checkout: form.checkout,
    })
}

/// Whether Create may be pressed: the form builds and nothing is in flight.
#[must_use]
pub(crate) fn may_create(form: &Form, pending: bool) -> bool {
    !pending && build(form).is_ok()
}

/// The Git command the form will run, shown under the fields. Missing parts
/// read as the placeholders the fields use.
#[must_use]
pub(crate) fn command_line(form: &Form) -> String {
    let name = match form.name.trim() {
        "" => "<name>",
        name => name,
    };
    if let Some(parent) = &form.parent {
        let create = if form.checkout {
            format!("git checkout -b {name} <tip of {parent}>")
        } else {
            format!("git branch {name} <tip of {parent}>")
        };
        return format!(
            "{create}\ngit config --local branch.{name}.stackParent {parent}\ngit config --local branch.{name}.stackParentBase <tip of {parent}>"
        );
    }
    let start = match form.start.trim() {
        "" => String::new(),
        start => format!(" {start}"),
    };
    if form.checkout {
        format!("git checkout -b {name}{start}")
    } else {
        format!("git branch {name}{start}")
    }
}

/// The create in flight, and its outcome until the window takes it. Engine
/// events for other operations never touch it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BranchCreate {
    pending: Option<(OperationId, String)>,
    outcome: Option<Result<String, String>>,
}

impl BranchCreate {
    pub(crate) fn start(&mut self, id: OperationId, name: String) {
        self.pending = Some((id, name));
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

    /// Whether the create waits for its workflow gates (`WF3c`).
    pub(crate) fn waits_for_gates(&self, wait: &GateWait) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|(pending, _)| wait.waits(*pending))
    }

    /// Ends the create with the operation's terminal `state`. A success gives
    /// the name; anything else gives Git's message, or a plain one.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, name)) = self.pending.take() else {
            return;
        };
        // A cancelled create is not a failure: nothing is reported (WF3c).
        self.outcome = match state {
            OperationState::Done => Some(Ok(name)),
            OperationState::Cancelled => None,
            _ => Some(Err(
                detail.unwrap_or_else(|| "The branch was not created.".to_owned())
            )),
        };
    }

    pub(crate) fn take(&mut self) -> Option<Result<String, String>> {
        self.outcome.take()
    }
}

/// The dialog while it is open: its two fields, whether the new branch is
/// checked out, and the error it shows.
pub(crate) struct NewBranchDialog {
    pub(crate) name: Entity<TextArea>,
    pub(crate) start: Entity<TextArea>,
    pub(crate) checkout: bool,
    /// The stack parent of a new child branch (New Child Branch).
    pub(crate) parent: Option<String>,
    /// Git's refusal, or the reason the form cannot run.
    pub(crate) error: Option<String>,
    /// The two texts the dialog last saw. The fields notify on every caret
    /// blink, so an error is cleared only when a text really changed.
    seen: (String, String),
    _subscriptions: Vec<Subscription>,
}

impl NewBranchDialog {
    pub(crate) fn form(&self, cx: &App) -> Form {
        Form {
            name: self.name.read(cx).text().to_owned(),
            start: self.start.read(cx).text().to_owned(),
            checkout: self.checkout,
            parent: self.parent.clone(),
        }
    }
}

impl StatusView {
    /// Opens the dialog. `start` is the commit the new branch starts from; the
    /// checked-out branch (or HEAD) is the default. With `parent`, the dialog
    /// makes a child of that local branch instead.
    pub(crate) fn open_new_branch_dialog(
        &mut self,
        start: Option<&str>,
        parent: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let head = self
            .model
            .status()
            .and_then(|status| status.branch.head.clone())
            .unwrap_or_else(|| "HEAD".to_owned());
        let field = |placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| TextArea::new(placeholder, cx).with_lines(1, 1))
        };
        let name = field("New branch name", cx);
        let start_field = field("Branch, tag or commit", cx);
        let mut subscriptions = Vec::new();
        for area in [&name, &start_field] {
            subscriptions.push(cx.observe(area, |this, _, cx| this.on_new_branch_field(cx)));
            subscriptions.push(cx.subscribe_in(
                area,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    if *event == TextAreaEvent::Cancel {
                        this.close_new_branch_dialog(window, cx);
                    }
                },
            ));
        }
        start_field.update(cx, |area, cx| {
            area.set_text(start.unwrap_or(head.as_str()), cx);
        });
        let seen = (
            name.read(cx).text().to_owned(),
            start_field.read(cx).text().to_owned(),
        );
        self.new_branch_dialog = Some(NewBranchDialog {
            name: name.clone(),
            start: start_field,
            checkout: true,
            parent: parent.map(str::to_owned),
            error: None,
            seen,
            _subscriptions: subscriptions,
        });
        name.update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn close_new_branch_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.new_branch_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// A field changed: the error of the last try goes, since it may be about the old text.
    fn on_new_branch_field(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.new_branch_dialog.as_mut() else {
            return;
        };
        let texts = (
            dialog.name.read(cx).text().to_owned(),
            dialog.start.read(cx).text().to_owned(),
        );
        if dialog.seen == texts {
            return;
        }
        dialog.seen = texts;
        dialog.error = None;
        cx.notify();
    }

    /// The checkbox: ticks or unticks "Check out the new branch".
    pub(crate) fn toggle_new_branch_checkout(&mut self, cx: &mut Context<Self>) {
        if self.model.branch_create.is_pending() {
            return;
        }
        if let Some(dialog) = self.new_branch_dialog.as_mut() {
            dialog.checkout = !dialog.checkout;
            cx.notify();
        }
    }

    /// Enter and the create button: checks the form, then sends the create.
    pub(crate) fn submit_new_branch(&mut self, cx: &mut Context<Self>) {
        if self.model.branch_create.is_pending() {
            return;
        }
        let Some(dialog) = self.new_branch_dialog.as_mut() else {
            return;
        };
        let form = dialog.form(cx);
        match build(&form) {
            Err(reason) => dialog.error = Some(reason.to_owned()),
            Ok(kind) => {
                dialog.error = None;
                let name = form.name.trim().to_owned();
                let id = self.engine.dispatch(Actor::Human, kind);
                self.model.branch_create.start(id, name);
            }
        }
        cx.notify();
    }

    /// Runs when the engine settles a create: success closes the dialog and
    /// names the branch; a failure stays in the dialog with Git's message.
    pub(crate) fn on_branch_created(
        &mut self,
        outcome: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(name) => {
                self.new_branch_dialog = None;
                self.list_focus.focus(window);
                self.request_branch_list();
                self.notify_user(&format!("Created branch {name}"), cx);
            }
            Err(message) => match self.new_branch_dialog.as_mut() {
                Some(dialog) => dialog.error = Some(message),
                None => self.notify_user(&message, cx),
            },
        }
        cx.notify();
    }

    fn next_new_branch_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.new_branch_dialog.as_ref() else {
            return;
        };
        if dialog.parent.is_some() {
            // A child has one field: its name.
            return;
        }
        let fields = [dialog.name.clone(), dialog.start.clone()];
        let current = fields
            .iter()
            .position(|area| area.read(cx).is_focused(window))
            .unwrap_or(fields.len() - 1);
        fields[(current + 1) % fields.len()].update(cx, |area, cx| area.focus(window, cx));
    }

    /// The dialog's layer, or nothing while the dialog is closed.
    pub(crate) fn new_branch_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.new_branch_dialog.as_ref()?;
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_new_branch_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        let panel = self.new_branch_panel(dialog, left, cx);
        let view = cx.entity().downgrade();
        let tab = view.clone();
        let enter = view;
        Some(
            div().absolute().inset_0().child(outside).child(
                panel
                    .capture_action(move |_: &FocusNext, window, cx| {
                        let _ = tab.update(cx, |this, cx| this.next_new_branch_field(window, cx));
                        cx.stop_propagation();
                    })
                    .capture_action(move |_: &Newline, _, cx| {
                        let _ = enter.update(cx, StatusView::submit_new_branch);
                        cx.stop_propagation();
                    }),
            ),
        )
    }

    fn new_branch_panel(&self, dialog: &NewBranchDialog, left: f32, cx: &Context<Self>) -> Div {
        let pending = self.model.branch_create.is_pending();
        let waiting = self
            .model
            .branch_create
            .waits_for_gates(&self.model.gate_wait);
        let form = dialog.form(cx);
        let live = name_message(&form.name);
        let allowed = may_create(&form, pending);
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
                    .child(if dialog.parent.is_some() {
                        "New child branch"
                    } else {
                        "New branch"
                    }),
            )
            .child(field_row("NAME", dialog.name.clone()))
            .children(live.map(|message| div().text_sm().text_color(theme::error()).child(message)))
            .child(match &dialog.parent {
                Some(parent) => parent_row(parent),
                None => field_row("STARTS FROM", dialog.start.clone()),
            })
            .child(checkbox(
                "new-branch-checkout",
                "Check out the new branch",
                dialog.checkout,
                StatusView::toggle_new_branch_checkout,
                cx,
            ))
            .child(command_block(&command_line(&form)))
            .children(dialog.error.as_ref().map(|error| {
                div()
                    .flex()
                    .flex_col()
                    .text_sm()
                    .text_color(theme::error())
                    .children(error.lines().map(|line| div().child(line.to_owned())))
            }))
            .child(buttons(pending, waiting, allowed, cx))
    }
}

/// The fixed parent of a new child branch, where the start field would be.
fn parent_row(parent: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(theme::muted()).child("PARENT"))
        .child(
            div()
                .text_sm()
                .text_color(theme::text())
                .child(format!("{parent}, starting at its tip")),
        )
}

/// A checkbox row: the mark, then its label. A click runs `on_click` on the window.
pub(crate) fn checkbox(
    id: &'static str,
    label: &'static str,
    checked: bool,
    on_click: fn(&mut StatusView, &mut Context<StatusView>),
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let mark = if checked {
        icon::filled(Icon::CheckSquare, icon::SMALL, theme::accent())
    } else {
        icon::view(Icon::Square, icon::SMALL, theme::muted())
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(mark)
        .child(div().text_sm().text_color(theme::text()).child(label))
        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
            on_click(this, cx);
        }))
}

/// Cancel and the create button; the button waits while a create runs.
fn buttons(pending: bool, waiting: bool, allowed: bool, cx: &Context<StatusView>) -> Div {
    let create: Stateful<Div> = div()
        .id("new-branch-create")
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::accent())
        .text_color(theme::on_accent())
        .font_weight(FontWeight::MEDIUM)
        .when(!allowed || pending, |this| this.opacity(0.5))
        .when(allowed && !pending, gpui::Styled::cursor_pointer)
        .child(if waiting {
            gate_wait::WAITING
        } else if pending {
            "Creating…"
        } else {
            "Create branch"
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.submit_new_branch(cx);
            }),
        );
    div()
        .flex()
        .flex_row()
        .justify_end()
        .items_center()
        .gap_2()
        .child(
            div()
                .id("new-branch-cancel")
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
                        this.close_new_branch_dialog(window, cx);
                    }),
                ),
        )
        .when(waiting, |row| {
            row.child(crate::shell_commit::gate_cancel_link(cx))
        })
        .child(create)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child_form(name: &str, parent: &str, checkout: bool) -> Form {
        Form {
            parent: Some(parent.to_owned()),
            ..form(name, "ignored", checkout)
        }
    }

    #[test]
    fn a_child_form_creates_a_child_of_its_parent() {
        assert_eq!(
            build(&child_form(" api ", "base", false)),
            Ok(CommandKind::CreateChildBranch {
                name: "api".into(),
                parent: "base".into(),
                checkout: false,
            })
        );
        assert_eq!(build(&child_form("", "base", true)), Err(NO_NAME));
        let line = command_line(&child_form("api", "base", true));
        assert!(
            line.starts_with("git checkout -b api <tip of base>"),
            "{line}"
        );
        assert!(line.contains("git config --local branch.api.stackParent base"));
    }

    fn form(name: &str, start: &str, checkout: bool) -> Form {
        Form {
            parent: None,
            name: name.into(),
            start: start.into(),
            checkout,
        }
    }

    #[test]
    fn the_name_message_shows_the_rule_that_refuses_the_name() {
        assert_eq!(name_message(""), None);
        assert_eq!(name_message("   "), None);
        assert_eq!(name_message("feature/login"), None);
        assert_eq!(
            name_message("two words"),
            Some("A branch name cannot contain spaces.")
        );
        assert_eq!(
            name_message("tip.lock"),
            Some("A part of a branch name cannot end with '.lock'.")
        );
    }

    #[test]
    fn a_name_is_needed_and_must_pass_the_rules() {
        assert_eq!(build(&form("", "main", true)), Err(NO_NAME));
        assert_eq!(
            build(&form("a..b", "main", true)),
            Err("A branch name cannot contain '..'.")
        );
    }

    #[test]
    fn the_command_trims_the_name_and_starts_at_head_when_the_start_is_empty() {
        assert_eq!(
            build(&form("  topic  ", "  v1 ", false)),
            Ok(CommandKind::CreateBranch {
                name: "topic".into(),
                start_point: Some("v1".into()),
                checkout: false,
            })
        );
        assert_eq!(
            build(&form("topic", "   ", true)),
            Ok(CommandKind::CreateBranch {
                name: "topic".into(),
                start_point: None,
                checkout: true,
            })
        );
    }

    #[test]
    fn a_start_point_read_as_an_option_is_refused() {
        assert_eq!(
            build(&form("topic", "--all", true)),
            Err("Choose a branch, tag or commit to start from.")
        );
    }

    #[test]
    fn create_waits_for_a_valid_form_and_no_create_in_flight() {
        assert!(may_create(&form("topic", "main", true), false));
        assert!(!may_create(&form("topic", "main", true), true));
        assert!(!may_create(&form("", "main", true), false));
        assert!(!may_create(&form("bad name", "main", true), false));
    }

    #[test]
    fn the_command_line_shows_what_git_will_run() {
        assert_eq!(
            command_line(&form("topic", "main", true)),
            "git checkout -b topic main"
        );
        assert_eq!(
            command_line(&form("topic", "main", false)),
            "git branch topic main"
        );
        assert_eq!(command_line(&form("", "", false)), "git branch <name>");
    }

    #[test]
    fn creating_keeps_only_its_own_operation_and_names_the_branch_on_success() {
        let mut create = BranchCreate::default();
        assert!(!create.is_pending());
        create.start(OperationId(4), "topic".into());
        assert!(create.is_pending());
        assert!(!create.owns(OperationId(3)));
        assert!(create.owns(OperationId(4)));
        create.settle(OperationState::Done, None);
        assert_eq!(create.take(), Some(Ok("topic".to_owned())));
        assert_eq!(create.take(), None);
    }

    #[test]
    fn a_failed_create_keeps_gits_message_or_says_it_was_not_created() {
        let mut create = BranchCreate::default();
        create.start(OperationId(4), "main".into());
        create.settle(
            OperationState::Failed,
            Some("fatal: a branch named 'main' already exists".into()),
        );
        assert_eq!(
            create.take(),
            Some(Err("fatal: a branch named 'main' already exists".to_owned()))
        );
        create.start(OperationId(5), "x".into());
        create.settle(OperationState::Vetoed, None);
        assert_eq!(
            create.take(),
            Some(Err("The branch was not created.".to_owned()))
        );
    }
}
