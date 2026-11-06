//! The commit pane of a submodule row (D175): the commits the submodule gained,
//! newest first, one line each with its author and age, read-only. It takes the
//! place of the diff pane while the row is selected, since a submodule has no
//! diff to show here. Its state is read by the engine, like the Reflog page's
//! entries (B3).

use std::ops::Range;

use gm_schema::{Actor, CommandKind};
use gpui::{Context, Div, FontWeight, Window, div, prelude::*, px, uniform_list};

use crate::shell_history::now_secs;
use crate::submodule::{self, PaneCommit};
use crate::theme;
use crate::view::StatusView;

const ROW_HEIGHT: f32 = 24.;

impl StatusView {
    /// Asks the engine for the state of each submodule of the status, after every
    /// status refresh (D175). Nothing is asked while an open is in flight: its
    /// answer would describe the repository being left.
    pub(crate) fn request_submodules(&mut self) {
        if self.model.is_opening() {
            return;
        }
        for path in self.model.submodules.take_wanted() {
            let id = self.engine.dispatch(
                Actor::Human,
                CommandKind::LoadSubmodule { path: path.clone() },
            );
            self.model.submodules.request(path, id);
        }
    }

    /// The pane of the selected submodule row: its path, then its commits, the
    /// `and more commits` line and the count of commits it is behind, or `Not
    /// checked out`. Empty while its read is in flight.
    pub(crate) fn submodule_pane(&self, cx: &mut Context<Self>) -> Div {
        let Some(entry) = self.model.selected_submodule() else {
            return div();
        };
        let path = entry.path.clone();
        let pane = self
            .model
            .submodules
            .detail(&path)
            .map(|detail| submodule::pane(detail, now_secs()));
        let header = div()
            .h(px(44.))
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .border_b_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::MEDIUM)
                    .font_family(theme::code_font())
                    .child(path.clone()),
            );
        let body = match pane {
            None => div(),
            Some(pane) if pane.not_checked_out => muted_line("Not checked out"),
            Some(pane) => {
                let count = pane.commits.len();
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .children((count > 0).then(|| commit_rows(path, count, cx)))
                    .children(pane.more.then(|| muted_line("and more commits")))
                    .children(pane.behind.as_deref().map(muted_line))
            }
        };
        // The slot around the pane is not a flex container: the pane fills it, as the diff pane
        // does, or the list under the header has no height and draws nothing.
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::ground())
            .child(header)
            .child(body)
    }
}

/// A line of secondary text under the list.
fn muted_line(text: &str) -> Div {
    div()
        .px_4()
        .py_2()
        .text_color(theme::muted())
        .child(text.to_owned())
}

/// The commit lines, read from the submodule's state as it is when they are drawn.
fn commit_rows(path: String, count: usize, cx: &mut Context<StatusView>) -> Div {
    div().flex_1().min_h(px(0.)).flex().flex_col().child(
        uniform_list(
            "submodule-commits",
            count,
            cx.processor(
                move |this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut Window,
                      _cx: &mut Context<StatusView>| {
                    let commits: Vec<PaneCommit> = this
                        .model
                        .submodules
                        .detail(&path)
                        .map(|detail| submodule::pane(detail, now_secs()).commits)
                        .unwrap_or_default();
                    commits
                        .get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .map(commit_row)
                        .collect()
                },
            ),
        )
        .flex_1()
        .min_h(px(0.)),
    )
}

/// One commit: its subject, then its author and how long ago it was written.
fn commit_row(commit: &PaneCommit) -> Div {
    div()
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_4()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(commit.subject.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_color(theme::muted())
                .child(commit.author.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_color(theme::muted())
                .child(commit.when.clone()),
        )
}
