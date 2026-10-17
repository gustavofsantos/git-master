//! The welcome screen: what the window shows when there is no repository to
//! show, so the user sees what to do instead of an empty window. It replaces
//! the whole window body, top bar and sidebar included, because every control
//! there works on a repository. Also the decision of what replaces a start
//! path that is not a repository.

use std::path::{Path, PathBuf};

use gm_schema::{Actor, CommandKind, Event, OperationId, OperationState};
use gpui::{
    Context, Div, FontWeight, MouseButton, MouseDownEvent, Window, div, img, prelude::*, px,
};

use crate::icon::{self, Icon};
use crate::project_menu::folder_name;
use crate::theme;
use crate::view::StatusView;

const MARK_DARK: &str = "brand/gitmaster-dark-256.png";
const MARK_LIGHT: &str = "brand/gitmaster-light-256.png";
const MARK_SIZE: f32 = 96.;
const COLUMN_WIDTH: f32 = 420.;
/// Most characters of a path shown on a recent row. The text is shortened here
/// because the row's ellipsis does not cut a path cleanly.
const PATH_CHARS: usize = 48;

/// The open that starts the window, while it is in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Startup {
    pub(crate) id: OperationId,
    /// Whether a failure may fall back to the newest recent repository. False
    /// for a path given on the command line, and for a recent one already tried.
    pub(crate) may_fall_back: bool,
}

/// What the welcome screen shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Welcome {
    /// The recent repositories whose folders still exist, newest first.
    pub(crate) recents: Vec<PathBuf>,
    /// Why the last open failed, shown under the action.
    pub(crate) error: Option<String>,
}

/// What replaces a start path that is not a repository.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Fallback {
    /// Open this recent repository instead.
    OpenRecent(PathBuf),
    /// Show the welcome screen.
    Welcome,
}

/// Decides what replaces a start path that is not a repository. `existing` is
/// the recent list, newest first, without the folders that are gone: the
/// newest of them opens when the fallback is allowed, otherwise the welcome
/// screen shows.
#[must_use]
pub(crate) fn start_fallback(may_fall_back: bool, existing: &[PathBuf]) -> Fallback {
    match existing.first() {
        Some(recent) if may_fall_back => Fallback::OpenRecent(recent.clone()),
        _ => Fallback::Welcome,
    }
}

/// The paths that are still folders. Touches the disk: call it off the UI thread.
#[must_use]
pub(crate) fn existing_dirs(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().filter(|path| path.is_dir()).cloned().collect()
}

impl StatusView {
    /// Takes the startup open when `event` settles it. A failure is returned so
    /// the caller decides what replaces the repository that did not open.
    pub(crate) fn take_settled_startup(&mut self, event: &Event) -> Option<Startup> {
        let startup = self.startup?;
        match event {
            Event::OperationChanged {
                id,
                state: OperationState::Failed,
                ..
            } if *id == startup.id => {
                self.startup = None;
                Some(startup)
            }
            Event::StatusLoaded { id, .. } if *id == startup.id => {
                self.startup = None;
                None
            }
            _ => None,
        }
    }

    /// Decides, with the disk checked off the UI thread, what replaces a start
    /// that failed: the newest recent repository, or the welcome screen.
    pub(crate) fn on_start_failed(&mut self, startup: Startup, cx: &mut Context<Self>) {
        let recents = self.recents.paths().to_vec();
        cx.spawn(async move |this, cx| {
            let existing = cx
                .background_spawn(async move { existing_dirs(&recents) })
                .await;
            let _ = this.update(cx, |view, cx| {
                match start_fallback(startup.may_fall_back, &existing) {
                    Fallback::OpenRecent(path) => {
                        view.model.error = None;
                        view.open_recent_at_start(&path, cx);
                    }
                    Fallback::Welcome => {
                        // The working directory was only a guess at a repository:
                        // its git error is not shown. A path the user gave is.
                        if startup.may_fall_back {
                            view.model.error = None;
                        }
                        view.show_welcome(existing, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// The fallback open. It is not allowed to fall back again: if this recent
    /// repository fails too, the welcome screen shows with its reason.
    fn open_recent_at_start(&mut self, path: &Path, cx: &mut Context<Self>) {
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::OpenRepo {
                path: path.to_string_lossy().into_owned(),
            },
        );
        self.model.request_open(id);
        self.startup = Some(Startup {
            id,
            may_fall_back: false,
        });
        cx.notify();
    }

    fn show_welcome(&mut self, recents: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.welcome = Some(Welcome {
            recents,
            error: self.model.error.take(),
        });
        cx.notify();
    }

    /// The whole window body while there is no repository. It has no list and
    /// no top bar, so no repository action is reachable from it.
    pub(crate) fn welcome_body(&self, welcome: &Welcome, cx: &Context<Self>) -> Div {
        let muted = theme::muted();
        let mark = mark_path(theme::is_dark());
        let open = div()
            .id("welcome-open")
            .h(px(32.))
            .px_3()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .rounded_md()
            .bg(theme::accent())
            .text_color(theme::on_accent())
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .child(icon::view(Icon::Folder, icon::SIZE, theme::on_accent()))
            .child("Open a repository")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window: &mut Window, cx| {
                    this.pick_folder(window, cx);
                }),
            );
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px_4()
            .py_8()
            .child(
                div()
                    .w_full()
                    .max_w(px(COLUMN_WIDTH))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_4()
                    .child(img(mark).w(px(MARK_SIZE)).h(px(MARK_SIZE)))
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("GitMaster"),
                    )
                    .child(
                        div()
                            .text_center()
                            .text_color(muted)
                            .child("Stage, commit and switch branches in a native window. Everything runs locally, in your own Git repositories."),
                    )
                    .child(open)
                    .children(
                        self.model
                            .is_opening()
                            .then(|| div().text_xs().text_color(muted).child("Opening…")),
                    )
                    .children(welcome.error.as_deref().map(error_box))
                    .children(
                        (!welcome.recents.is_empty())
                            .then(|| recent_list(&welcome.recents, cx)),
                    ),
            )
    }
}

/// The reason an open failed, under the action.
fn error_box(error: &str) -> Div {
    div()
        .w_full()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(theme::removed_bg())
        .text_color(theme::removed_fg())
        .child(error.to_owned())
}

/// The recent repositories, newest first. Each row opens its repository.
fn recent_list(recents: &[PathBuf], cx: &Context<StatusView>) -> Div {
    let muted = theme::muted();
    div()
        .w_full()
        .flex()
        .flex_col()
        .child(
            div()
                .px_3()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(muted)
                .child("Recent repositories"),
        )
        .children(recents.iter().enumerate().map(|(index, path)| {
            let owned = path.clone();
            div()
                .id(("welcome-recent", index))
                .w_full()
                .min_w(px(0.))
                .px_3()
                .py_2()
                .rounded_md()
                .flex()
                .flex_col()
                .cursor_pointer()
                .hover(|style| style.bg(theme::selected_row()))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(folder_name(path)),
                )
                .child(
                    div()
                        .w_full()
                        .min_w(px(0.))
                        .font_family(theme::code_font())
                        .text_xs()
                        .text_color(muted)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(fitted_path(path)),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.open_repo(&owned, cx);
                    }),
                )
        }))
}

/// The path as one short line, keeping its end: the folder names are there.
#[must_use]
fn fitted_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    let count = text.chars().count();
    if count <= PATH_CHARS {
        return text.into_owned();
    }
    let tail: String = text.chars().skip(count - (PATH_CHARS - 1)).collect();
    format!("…{tail}")
}

/// The mark for the theme in use, from the bundled assets.
#[must_use]
pub(crate) fn mark_path(dark: bool) -> &'static str {
    if dark { MARK_DARK } else { MARK_LIGHT }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(format!("/repos/{name}"))
    }

    #[test]
    fn a_path_given_on_the_command_line_never_falls_back() {
        assert_eq!(start_fallback(false, &[p("a")]), Fallback::Welcome);
    }

    #[test]
    fn no_existing_recent_means_the_welcome_screen() {
        assert_eq!(start_fallback(true, &[]), Fallback::Welcome);
    }

    #[test]
    fn the_newest_existing_recent_opens_when_the_fallback_is_allowed() {
        assert_eq!(
            start_fallback(true, &[p("newest"), p("older")]),
            Fallback::OpenRecent(p("newest"))
        );
    }

    #[test]
    fn a_failed_recent_open_goes_to_the_welcome_screen() {
        assert_eq!(start_fallback(false, &[p("a")]), Fallback::Welcome);
    }

    #[test]
    fn existing_dirs_keeps_folders_and_drops_the_missing_ones() {
        let here = std::env::temp_dir();
        let gone = PathBuf::from("/gm-no-such-folder/for-the-welcome-test");
        assert_eq!(
            existing_dirs(&[gone.clone(), here.clone(), gone]),
            vec![here]
        );
    }

    #[test]
    fn a_short_path_is_shown_whole() {
        assert_eq!(fitted_path(Path::new("/repos/a")), "/repos/a");
    }

    #[test]
    fn a_long_path_keeps_its_end_within_the_budget() {
        let long = PathBuf::from(format!("/home/user/{}/project", "nested/".repeat(12)));
        let shown = fitted_path(&long);
        assert_eq!(shown.chars().count(), PATH_CHARS);
        assert!(shown.starts_with('…'));
        assert!(shown.ends_with("/project"));
    }

    #[test]
    fn the_mark_follows_the_theme() {
        assert_eq!(mark_path(true), MARK_DARK);
        assert_eq!(mark_path(false), MARK_LIGHT);
    }
}
