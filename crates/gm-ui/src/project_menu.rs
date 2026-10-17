//! The project button at the left of the top bar and the floating list it
//! opens: the recent repositories and an "Open folder…" row. Picking one
//! dispatches `OpenRepo`; the view changes only when the engine says it
//! succeeded (see `Model::apply`). A folder that is not a repository leaves
//! the current one on screen and git's reason on the status line.

use std::path::{Path, PathBuf};

use gm_schema::{Actor, CommandKind};
use gpui::{
    Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    PathPromptOptions, Stateful, Window, div, prelude::*, px,
};

use crate::icon::{self, Icon};
use crate::theme;
use crate::view::StatusView;

const TOP_BAR_HEIGHT: f32 = 36.;
const MENU_WIDTH: f32 = 360.;

pub(crate) struct ProjectMenu {
    pub(crate) open: bool,
    pub(crate) focus: FocusHandle,
    /// Recent paths that are no longer directories, found when the list opened.
    pub(crate) unavailable: Vec<PathBuf>,
}

impl ProjectMenu {
    pub(crate) fn new(focus: FocusHandle) -> Self {
        Self {
            open: false,
            focus,
            unavailable: Vec::new(),
        }
    }
}

/// The last component of a repository root, or the root itself.
pub(crate) fn folder_name(root: &Path) -> String {
    root.file_name().map_or_else(
        || root.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl StatusView {
    /// The button at the left of the top bar, labelled with the folder name.
    pub(crate) fn project_button(&self, cx: &Context<Self>) -> Stateful<Div> {
        let muted = theme::muted();
        let label = self.model.status().map_or_else(
            || "Open repository".to_owned(),
            |status| folder_name(Path::new(&status.root)),
        );
        div()
            .id("project-button")
            .h(px(26.))
            .px_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .rounded_sm()
            .cursor_pointer()
            .font_weight(FontWeight::SEMIBOLD)
            .hover(|style| style.bg(theme::selected_row()))
            .child(icon::view(Icon::Folder, icon::SMALL, muted))
            .child(label)
            .child(icon::view(Icon::CaretDown, icon::SMALL, muted))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.toggle_project_menu(window, cx);
                }),
            )
    }

    pub(crate) fn toggle_project_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_menu.open {
            self.close_project_menu(window, cx);
            return;
        }
        self.branch_menu.open = false;
        self.project_menu.open = true;
        self.project_menu.focus.focus(window);
        // The directory checks run off the UI thread; the rows say "missing"
        // once they are known.
        let paths = self.recents.paths().to_vec();
        cx.spawn(async move |this, cx| {
            let unavailable = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .filter(|path| !path.is_dir())
                        .collect::<Vec<PathBuf>>()
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.project_menu.unavailable = unavailable;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn close_project_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project_menu.open = false;
        self.list_focus.focus(window);
        cx.notify();
    }

    /// Dispatches the open. The current repository stays until it succeeds.
    pub(crate) fn open_repo(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.project_menu.open = false;
        if self.model.is_opening() {
            self.notify_user("Already opening a repository.", cx);
            return;
        }
        if self.model.branches.checkout_in_flight() {
            self.notify_user("Wait for the branch checkout to finish.", cx);
            return;
        }
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::OpenRepo {
                path: path.to_string_lossy().into_owned(),
            },
        );
        self.model.request_open(id);
        cx.notify();
    }

    /// Shows the native folder prompt. Its outcome is handled in a task, so
    /// the UI never waits for the dialog.
    pub(crate) fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_project_menu(window, cx);
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| match picked.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |view, cx| view.open_repo(&path, cx));
                }
            }
            // Cancelled: nothing to do.
            Ok(Ok(None)) | Err(_) => {}
            Ok(Err(error)) => {
                let text = format!("Could not open the folder prompt: {error}");
                let _ = this.update(cx, |view, cx| view.notify_user(&text, cx));
            }
        })
        .detach();
    }

    /// The list and the layer under it that closes it on a click outside.
    /// Drawn last, so it sits above the whole window.
    pub(crate) fn project_menu_layer(&self, cx: &Context<Self>) -> Div {
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_project_menu(window, cx);
            }),
        );
        div()
            .absolute()
            .inset_0()
            .child(outside)
            .child(self.project_list(px(TOP_BAR_HEIGHT + 4.), cx))
    }

    /// One recent repository: name, then its path. A missing one is greyed and
    /// does not respond to clicks; the current one closes the list on click.
    fn recent_row(
        &self,
        index: usize,
        path: &Path,
        is_current: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let muted = theme::muted();
        let missing = self
            .project_menu
            .unavailable
            .iter()
            .any(|gone| gone == path);
        let name_colour = if missing { muted } else { theme::text() };
        let mut row = div()
            .id(("recent", index))
            .px_3()
            .py_1()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(name_colour)
                            .child(folder_name(path)),
                    )
                    .child(
                        div()
                            .font_family(theme::code_font())
                            .text_xs()
                            .text_color(muted)
                            .overflow_hidden()
                            .child(path.to_string_lossy().into_owned()),
                    ),
            );
        if missing {
            return row.child(div().text_xs().text_color(muted).child("Missing"));
        }
        let owned = path.to_path_buf();
        row = row
            .cursor_pointer()
            .hover(|style| style.bg(theme::selected_row()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    if is_current {
                        this.close_project_menu(window, cx);
                    } else {
                        this.open_repo(&owned, cx);
                    }
                }),
            );
        if is_current {
            row = row.child(icon::view(Icon::CheckCircle, icon::SMALL, muted));
        }
        row
    }

    fn project_list(&self, top: gpui::Pixels, cx: &Context<Self>) -> Div {
        let muted = theme::muted();
        let current = self
            .model
            .status()
            .map(|status| PathBuf::from(&status.root));
        let recents = self.recents.paths();
        let rows = recents.iter().enumerate().map(|(index, path)| {
            let is_current = current.as_deref() == Some(path.as_path());
            self.recent_row(index, path, is_current, cx)
        });
        let empty = recents.is_empty().then(|| {
            div()
                .px_3()
                .py_2()
                .text_color(muted)
                .child("No recent repositories yet.")
        });
        let open_folder = div()
            .id("open-folder")
            .px_3()
            .py_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(theme::hairline())
            .cursor_pointer()
            .hover(|style| style.bg(theme::selected_row()))
            .child(icon::view(Icon::Folder, icon::SMALL, muted))
            .child("Open folder…")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.pick_folder(window, cx);
                }),
            );
        div()
            .absolute()
            .top(top)
            .left(px(12.))
            .w(px(MENU_WIDTH))
            .flex()
            .flex_col()
            .py_1()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::hairline_strong())
            .rounded_md()
            .shadow_lg()
            .occlude()
            .track_focus(&self.project_menu.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_project_menu(window, cx);
                }
            }))
            .children(rows)
            .children(empty)
            .child(open_folder)
    }
}
