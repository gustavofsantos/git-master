//! **Clone from URL…** in the repository list of the workspace dialog (D256 (1);
//! docs/guide/workspaces.md, "Add a repository from a URL"). The engine suggests the folder and runs
//! the clone off its thread; this module keeps what it answers and shows the small form under the
//! rows. The rules are pure and tested here: which answer is the one waited for, what a settled
//! clone means and when **Clone** is enabled. The window side is the `impl StatusView` below.

use std::time::Duration;

use gm_schema::{Actor, CommandKind, OperationId, OperationState};
use gpui::{
    Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, Stateful, Window, div,
    prelude::*, px,
};

use crate::text_area::{TextArea, TextAreaEvent};
use crate::text_search::DEBOUNCE_MS;
use crate::theme;
use crate::view::StatusView;

/// The folder the engine suggested for one URL, or why it cannot suggest one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloneSuggestion {
    pub(crate) url: String,
    pub(crate) path: Option<String>,
    pub(crate) problem: Option<String>,
}

/// The suggestion and the clone in flight, as the model keeps them. Answers to an older request
/// are dropped by their operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cloning {
    suggesting: Option<OperationId>,
    /// The newest suggestion, until the view takes it.
    suggestion: Option<CloneSuggestion>,
    running: Option<OperationId>,
    /// The folder the engine cloned into, before the operation ends.
    cloned: Option<String>,
    /// `Ok` with the clone's folder, or `Err` with the reason, until the view takes it.
    outcome: Option<Result<String, String>>,
}

impl Cloning {
    pub(crate) fn suggest(&mut self, id: OperationId) {
        self.suggesting = Some(id);
    }

    /// Keeps the suggestion that `id` answers, when it is the one waited for.
    pub(crate) fn suggested(&mut self, id: OperationId, suggestion: CloneSuggestion) -> bool {
        if self.suggesting != Some(id) {
            return false;
        }
        self.suggesting = None;
        self.suggestion = Some(suggestion);
        true
    }

    pub(crate) fn take_suggestion(&mut self) -> Option<CloneSuggestion> {
        self.suggestion.take()
    }

    pub(crate) fn start(&mut self, id: OperationId) {
        self.running = Some(id);
        self.cloned = None;
        self.outcome = None;
    }

    #[must_use]
    pub(crate) fn is_cloning(&self) -> bool {
        self.running.is_some()
    }

    /// Whether `id` is the suggestion or the clone this keeps.
    #[must_use]
    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.suggesting == Some(id) || self.running == Some(id)
    }

    /// The engine cloned into `path` for the clone `id`.
    pub(crate) fn cloned(&mut self, id: OperationId, path: String) -> bool {
        if self.running != Some(id) {
            return false;
        }
        self.cloned = Some(path);
        true
    }

    /// Ends the operation `id` with its terminal `state`. A suggestion that failed is dropped; a
    /// clone gives its folder, or the engine's reason.
    pub(crate) fn settle(
        &mut self,
        id: OperationId,
        state: OperationState,
        detail: Option<String>,
    ) {
        if !state.is_terminal() {
            return;
        }
        if self.suggesting == Some(id) {
            self.suggesting = None;
            return;
        }
        if self.running != Some(id) {
            return;
        }
        self.running = None;
        self.outcome = Some(match (state, self.cloned.take()) {
            (OperationState::Done, Some(path)) => Ok(path),
            _ => Err(detail.unwrap_or_else(|| "The repository was not cloned.".to_owned())),
        });
    }

    pub(crate) fn take_outcome(&mut self) -> Option<Result<String, String>> {
        self.outcome.take()
    }
}

/// The members of the workspace the window shows, as the engine last listed them for the root
/// it was asked about. A plain repository lists none. Asked again when the window changes root and
/// after a member is added or removed (D256).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Members {
    /// The root the listing is for, or is being asked for.
    root: Option<String>,
    pending: Option<OperationId>,
    list: Vec<String>,
}

impl Members {
    /// Whether the listing must be asked for `root`: it is for another root, or was dropped.
    #[must_use]
    pub(crate) fn wants(&self, root: &str) -> bool {
        self.root.as_deref() != Some(root)
    }

    /// The listing of `root` is asked by `id`; the old one no longer applies.
    pub(crate) fn request(&mut self, root: &str, id: OperationId) {
        self.root = Some(root.to_owned());
        self.pending = Some(id);
        self.list.clear();
    }

    /// Keeps the members that `id` answers, when it is the request waited for.
    pub(crate) fn listed_by(&mut self, id: OperationId, members: Vec<String>) {
        if self.pending == Some(id) {
            self.pending = None;
            self.list = members;
        }
    }

    /// Drops the listing, so it is asked again.
    pub(crate) fn forget(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub(crate) fn listed(&self) -> &[String] {
        &self.list
    }
}

impl StatusView {
    /// Asks for the members of the window's repository when the listing is not for it.
    pub(crate) fn request_members(&mut self) {
        let Some(root) = self.model.status().map(|status| status.root.clone()) else {
            return;
        };
        if !self.model.members.wants(&root) {
            return;
        }
        let id = self.engine.dispatch(Actor::Human, CommandKind::ListMembers);
        self.model.members.request(&root, id);
    }
}

/// Whether **Clone** is enabled: a URL, a folder, and no clone in flight.
#[must_use]
pub(crate) fn may_clone(url: &str, folder: &str, cloning: bool) -> bool {
    !url.trim().is_empty() && !folder.trim().is_empty() && !cloning
}

/// The clone form while it is open under the rows: the row it fills, its two fields, and what the
/// folder field held when the suggestion last filled it.
pub(crate) struct CloneForm {
    pub(crate) row: usize,
    pub(crate) url: Entity<TextArea>,
    pub(crate) folder: Entity<TextArea>,
    /// The URL the observers last saw; a field notifies on every caret blink.
    seen_url: String,
    /// The folder text the last suggestion put there. A folder the person typed is not replaced.
    suggested: String,
    pub(crate) error: Option<String>,
}

impl StatusView {
    /// **Clone from URL…** on row `index`: the repository list closes and the form opens with the
    /// focus on its URL field.
    pub(crate) fn open_clone_form(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let field = |placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| TextArea::new(placeholder, cx).with_lines(1, 1))
        };
        let url = field("https://host/team/repository.git", cx);
        let folder = field("Folder of the clone", cx);
        let mut subscriptions = vec![cx.observe(&url, |this, _, cx| this.on_clone_url(cx))];
        for area in [&url, &folder] {
            subscriptions.push(cx.subscribe_in(
                area,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    if *event == TextAreaEvent::Cancel {
                        this.close_clone_form(window, cx);
                    }
                },
            ));
        }
        let Some(dialog) = self.workspace_dialog.as_mut() else {
            return;
        };
        dialog.choosing = None;
        dialog.clone = Some(CloneForm {
            row: index,
            url: url.clone(),
            folder,
            seen_url: String::new(),
            suggested: String::new(),
            error: None,
        });
        dialog.clone_subscriptions = subscriptions;
        url.update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    /// Escape in the form, and Cancel: the form closes and the dialog stays.
    pub(crate) fn close_clone_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.workspace_clone.is_cloning() {
            return;
        }
        if let Some(dialog) = self.workspace_dialog.as_mut() {
            dialog.clone = None;
            dialog.clone_subscriptions.clear();
            dialog.focus_first(window, cx);
        }
        cx.notify();
    }

    /// The URL changed: the folder is suggested again once the typing settles.
    fn on_clone_url(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self
            .workspace_dialog
            .as_mut()
            .and_then(|dialog| dialog.clone.as_mut())
        else {
            return;
        };
        let url = form.url.read(cx).text().trim().to_owned();
        if url == form.seen_url {
            return;
        }
        form.seen_url.clone_from(&url);
        form.error = None;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DEBOUNCE_MS))
                .await;
            let _ = this.update(cx, |view, _| view.suggest_clone_folder(&url));
        })
        .detach();
        cx.notify();
    }

    /// Asks the engine where a clone of `url` goes, when `url` is still the one in the form.
    fn suggest_clone_folder(&mut self, url: &str) {
        let current = self
            .workspace_dialog
            .as_ref()
            .and_then(|dialog| dialog.clone.as_ref())
            .is_some_and(|form| form.seen_url == url);
        if !current || url.is_empty() {
            return;
        }
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::PlanClone {
                url: url.to_owned(),
            },
        );
        self.model.workspace_clone.suggest(id);
    }

    /// Puts the engine's suggestion in the folder field, unless the person typed a folder of their
    /// own; a URL that names no folder shows why.
    pub(crate) fn on_clone_suggested(
        &mut self,
        suggestion: CloneSuggestion,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = self
            .workspace_dialog
            .as_mut()
            .and_then(|dialog| dialog.clone.as_mut())
        else {
            return;
        };
        if suggestion.url != form.seen_url {
            return;
        }
        let typed = form.folder.read(cx).text().to_owned();
        if let Some(path) = suggestion.path {
            if typed.is_empty() || typed == form.suggested {
                form.folder.update(cx, |area, cx| area.set_text(&path, cx));
                form.suggested = path;
            }
            form.error = None;
        } else {
            form.error = suggestion.problem;
        }
        cx.notify();
    }

    /// **Clone** and Enter in the form: sends the clone when the rules allow it.
    pub(crate) fn submit_clone(&mut self, cx: &mut Context<Self>) {
        let cloning = self.model.workspace_clone.is_cloning();
        let Some(form) = self
            .workspace_dialog
            .as_mut()
            .and_then(|dialog| dialog.clone.as_mut())
        else {
            return;
        };
        let url = form.url.read(cx).text().trim().to_owned();
        let folder = form.folder.read(cx).text().trim().to_owned();
        if !may_clone(&url, &folder, cloning) {
            return;
        }
        form.error = None;
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::CloneRepository { url, path: folder },
        );
        self.model.workspace_clone.start(id);
        cx.notify();
    }

    /// Runs when the engine settles the clone: the row takes the clone and the form closes; a
    /// failure stays in the form with Git's reason.
    pub(crate) fn on_repository_cloned(
        &mut self,
        outcome: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self
            .workspace_dialog
            .as_ref()
            .and_then(|dialog| dialog.clone.as_ref())
            .map(|form| form.row)
        else {
            if let Ok(path) = &outcome {
                self.notify_user(&format!("Cloned into {path}"), cx);
            }
            return;
        };
        match outcome {
            Ok(path) => {
                self.close_clone_form(window, cx);
                self.choose_repo(row, &path, cx);
                self.notify_user(&format!("Cloned into {path}"), cx);
            }
            Err(message) => {
                if let Some(form) = self
                    .workspace_dialog
                    .as_mut()
                    .and_then(|dialog| dialog.clone.as_mut())
                {
                    form.error = Some(message);
                }
            }
        }
        cx.notify();
    }

    /// The form under the rows: the URL, the folder, the reason a clone failed and its buttons.
    pub(crate) fn clone_form_view(&self, form: &CloneForm, cx: &Context<Self>) -> Div {
        let cloning = self.model.workspace_clone.is_cloning();
        let allowed = may_clone(
            form.url.read(cx).text(),
            form.folder.read(cx).text(),
            cloning,
        );
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child("CLONE FROM URL"),
            )
            .child(boxed_field(form.url.clone(), cx))
            .child(boxed_field(form.folder.clone(), cx))
            .children(
                form.error
                    .clone()
                    .map(|error| div().text_sm().text_color(theme::error()).child(error)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(
                        small_button("workspace-clone-cancel", "Cancel", false, true)
                            .when(cloning, |this| this.opacity(0.5))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.close_clone_form(window, cx);
                                }),
                            ),
                    )
                    .child(
                        small_button(
                            "workspace-clone",
                            if cloning { "Cloning…" } else { "Clone" },
                            true,
                            allowed,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| this.submit_clone(cx)),
                        ),
                    ),
            )
    }
}

/// A one-line field in a box; a click anywhere in the box takes the focus.
fn boxed_field(field: Entity<TextArea>, cx: &Context<StatusView>) -> Div {
    let target = field.clone();
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(theme::hairline())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |_, _: &MouseDownEvent, window, cx| {
                target.update(cx, |area, cx| area.focus(window, cx));
            }),
        )
        .child(field)
}

/// A button of the form: the accent one does the clone, the plain one cancels.
fn small_button(
    id: &'static str,
    label: &'static str,
    accent: bool,
    allowed: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded_md()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .when(accent, |this| {
            this.bg(theme::accent()).text_color(theme::on_accent())
        })
        .when(!accent, |this| this.text_color(theme::text()))
        .when(!allowed, |this| this.opacity(0.5))
        .when(allowed, gpui::Styled::cursor_pointer)
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suggestion(url: &str, path: &str) -> CloneSuggestion {
        CloneSuggestion {
            url: url.to_owned(),
            path: Some(path.to_owned()),
            problem: None,
        }
    }

    #[test]
    fn only_the_suggestion_waited_for_is_kept() {
        let mut cloning = Cloning::default();
        cloning.suggest(OperationId(1));
        cloning.suggest(OperationId(2));
        assert!(!cloning.suggested(OperationId(1), suggestion("a", "/s/a")));
        assert!(cloning.suggested(OperationId(2), suggestion("b", "/s/b")));
        assert_eq!(cloning.take_suggestion(), Some(suggestion("b", "/s/b")));
        assert_eq!(cloning.take_suggestion(), None);
    }

    #[test]
    fn a_clone_that_ends_done_gives_its_folder() {
        let mut cloning = Cloning::default();
        cloning.start(OperationId(3));
        assert!(cloning.is_cloning());
        assert!(cloning.owns(OperationId(3)));
        assert!(!cloning.cloned(OperationId(4), "/s/x".into()));
        assert!(cloning.cloned(OperationId(3), "/s/pay".into()));
        cloning.settle(OperationId(3), OperationState::Executing, None);
        assert!(cloning.is_cloning(), "only a terminal state settles it");
        cloning.settle(OperationId(3), OperationState::Done, None);
        assert!(!cloning.is_cloning());
        assert_eq!(cloning.take_outcome(), Some(Ok("/s/pay".to_owned())));
    }

    #[test]
    fn a_clone_that_fails_gives_git_reason() {
        let mut cloning = Cloning::default();
        cloning.start(OperationId(3));
        cloning.settle(
            OperationId(3),
            OperationState::Failed,
            Some("repository not found".into()),
        );
        assert_eq!(
            cloning.take_outcome(),
            Some(Err("repository not found".to_owned()))
        );
    }

    #[test]
    fn a_failed_suggestion_is_dropped_without_an_outcome() {
        let mut cloning = Cloning::default();
        cloning.suggest(OperationId(5));
        cloning.settle(OperationId(5), OperationState::Failed, Some("x".into()));
        assert!(!cloning.owns(OperationId(5)));
        assert_eq!(cloning.take_outcome(), None);
    }

    #[test]
    fn members_are_asked_once_per_root_and_only_the_answer_waited_for_is_kept() {
        let mut members = Members::default();
        assert!(members.wants("/w/pay"));
        members.request("/w/pay", OperationId(1));
        assert!(!members.wants("/w/pay"));
        members.listed_by(OperationId(9), vec!["x".into()]);
        assert!(members.listed().is_empty());
        members.listed_by(OperationId(1), vec!["billing".into()]);
        assert_eq!(members.listed(), ["billing"]);
        assert!(members.wants("/src/billing"));
        members.forget();
        assert!(members.wants("/w/pay"));
        assert!(members.listed().is_empty());
    }

    #[test]
    fn clone_needs_a_url_and_a_folder_and_no_clone_in_flight() {
        assert!(may_clone("https://h/a.git", "/s/a", false));
        assert!(!may_clone(" ", "/s/a", false));
        assert!(!may_clone("https://h/a.git", "", false));
        assert!(!may_clone("https://h/a.git", "/s/a", true));
    }
}
