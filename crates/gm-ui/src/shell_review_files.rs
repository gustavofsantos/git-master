//! The Review page's left cards and its continuous diff (UI redesign, slice 4): the Changes
//! of the branch, the Scorecards table of the branch (D219, S5), and every changed file's diff
//! one after another with a **Viewed** box each. The conversations list and the selected
//! conversation's detail stay in `shell_review`.

use gm_schema::{Actor, CommandKind, CommentLabel, DiffLine, LineKind};
use gpui::{
    Context, Div, FontWeight, Hsla, MouseButton, MouseDownEvent, SharedString, Stateful, div,
    prelude::*, px,
};

use crate::icon::{self, Icon};
use crate::review_files::Sides;
use crate::review_scorecards::{self, Decoration, Dot, ScorecardRow, StatusTone, Weight};
use crate::shell_memories::tag;
use crate::theme;
use crate::view::{Screen, StatusView};

/// The width of the column of cards and conversations.
pub(crate) const SIDE_WIDTH: f32 = 320.;
/// The most lines drawn of one file in the continuous diff; the rest is counted.
const LINES_SHOWN: usize = 400;

impl StatusView {
    /// Reads every changed file's diff again for the continuous diff, while the Review page
    /// is shown. Called when the page opens and after each status.
    pub(crate) fn request_review_files(&mut self) {
        if self.screen != Screen::LocalReview {
            return;
        }
        let entries = self.model.entries().to_vec();
        let plan = self.model.review_files.plan(&entries);
        for (path, side) in plan {
            let id = self.engine.dispatch(
                Actor::Human,
                CommandKind::LoadDiff {
                    path: path.clone(),
                    side,
                },
            );
            self.model.review_files.request(id, path, side);
        }
    }

    /// Reads the recent scorecards of the checked-out branch (D219 (11)): the table and its trend
    /// dots. Called when the page opens and after each listing of the review comments, which a
    /// publish always follows, so the table is as fresh as a publish.
    pub(crate) fn request_scorecards(&mut self) {
        let Some(branch) = self.current_branch() else {
            return;
        };
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::ListRecentScorecards {
                branch: Some(branch),
                member: None,
            },
        );
        self.model.scorecards.request(id);
    }

    /// The checked-out branch, or `None` when HEAD is detached or the status is not read yet.
    pub(crate) fn current_branch(&self) -> Option<String> {
        self.model
            .status()
            .and_then(|status| status.branch.head.clone())
    }

    /// The left column's cards: Changes, then the Scorecards table.
    pub(crate) fn review_cards(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(theme::hairline())
            .child(self.changes_card())
            .child(self.scorecards_card(cx))
    }

    fn changes_card(&self) -> Div {
        let branch = self.model.status().map(|status| &status.branch);
        let head = branch
            .and_then(|branch| branch.head.clone())
            .unwrap_or_else(|| "detached HEAD".to_owned());
        let base = branch
            .and_then(|branch| branch.upstream.clone())
            .unwrap_or_else(|| "no upstream".to_owned());
        let (ahead, behind) = branch.map_or((0, 0), |branch| (branch.ahead, branch.behind));
        let (added, removed) = self
            .model
            .review_files
            .files()
            .map(|(_, sides)| sides.counts())
            .fold((0, 0), |(a, r), (x, y)| (a + x, r + y));
        let (viewed, read) = self.model.review_files.viewed_count();
        card("Changes")
            .child(
                div()
                    .font_family(theme::code_font())
                    .child(format!("{head} → {base}")),
            )
            .child(muted_line(&format!(
                "↑{ahead} ↓{behind} · +{added} −{removed} in the working tree"
            )))
            .child(muted_line(&format!("{viewed} of {read} files viewed")))
    }

    /// The Scorecards table: one row per scorecard name on the branch, the newest of each, with
    /// its status, label, trend and blocking count. A row opens to its body and origin.
    fn scorecards_card(&self, cx: &mut Context<Self>) -> Div {
        let mut body = card("Scorecards");
        match (self.model.scorecards.rows(), self.current_branch()) {
            (_, None) => {
                body = body.child(muted_line("HEAD is detached, so no branch has scorecards."));
            }
            (None, Some(_)) => body = body.child(muted_line(review_scorecards::READING)),
            (Some(rows), Some(branch)) => {
                let table = review_scorecards::table_of(rows, &branch);
                if table.is_empty() {
                    body = body
                        .child(muted_line(review_scorecards::NO_RUN_TITLE))
                        .child(muted_line(review_scorecards::NO_RUN_HINT));
                }
                for (index, row) in table.iter().enumerate() {
                    let open = self.model.scorecards.is_expanded(&row.name);
                    body = body.child(Self::scorecard_row(index, row, open, cx));
                }
            }
        }
        body
    }

    /// One row of the table: its name, status and Stale mark, its label, decorations and subject,
    /// the trend and the blocking count. Expanded, its body and where it came from.
    fn scorecard_row(
        index: usize,
        row: &ScorecardRow,
        open: bool,
        cx: &mut Context<StatusView>,
    ) -> Stateful<Div> {
        let name = row.name.clone();
        let (colour, background) = status_colours(row.status.tone);
        let head = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(theme::code_font())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(row.name.clone()),
            )
            .child(tag(row.status.word, background).text_color(colour))
            .children(row.stale.then(|| {
                tag(review_scorecards::STALE, theme::warn_bg()).text_color(theme::warn_fg())
            }));
        let mut summary = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .text_xs()
            .min_w(px(0.))
            .child(label_chip(row.label));
        for decoration in review_scorecards::decoration_chips(&row.decorations) {
            summary = summary.child(decoration_chip(&decoration));
        }
        summary = summary.child(
            div()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(theme::text())
                .child(row.subject.clone()),
        );
        let blocking = if row.blocking_open > 0 {
            theme::removed_fg()
        } else {
            theme::muted()
        };
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(trend_dots(&row.trend))
            .child(
                div()
                    .text_xs()
                    .text_color(blocking)
                    .child(format!("{} blocking", row.blocking_open)),
            );
        let mut item = div()
            .id(("scorecard-row", index))
            .flex()
            .flex_col()
            .gap_1()
            .px_1()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.bg(theme::selected_row()))
            .child(head)
            .child(summary)
            .child(footer);
        if open {
            item = item
                .child(
                    div()
                        .pt_1()
                        .text_sm()
                        .text_color(theme::text())
                        .child(row.body.clone()),
                )
                .child(muted_line(&review_scorecards::origin_line(row)));
        }
        item.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.model.scorecards.toggle(&name);
                cx.notify();
            }),
        )
    }

    /// The continuous diff: every changed file's diff, with its Viewed box.
    pub(crate) fn review_files_pane(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let pane = div()
            .id("review-files")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .min_h(px(0.))
            .overflow_y_scroll()
            .border_l_1()
            .border_color(theme::hairline())
            .flex()
            .flex_col();
        let files: Vec<(&str, &Sides)> = self.model.review_files.files().collect();
        if files.is_empty() {
            return pane.child(
                div()
                    .p_4()
                    .text_color(theme::muted())
                    .child("No changed file to review."),
            );
        }
        pane.children(
            files
                .into_iter()
                .enumerate()
                .map(|(index, (path, sides))| self.review_file(index, path, sides, cx)),
        )
    }

    fn review_file(&self, index: usize, path: &str, sides: &Sides, cx: &mut Context<Self>) -> Div {
        let viewed = self.model.review_files.is_viewed(path);
        let (added, removed) = sides.counts();
        let target = path.to_owned();
        let toggle = div()
            .id(SharedString::from(format!("review-viewed-{index}")))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .text_xs()
            .text_color(if viewed {
                theme::accent()
            } else {
                theme::muted()
            })
            .child(if viewed {
                icon::filled(Icon::CheckSquare, icon::SMALL, theme::accent())
            } else {
                icon::view(Icon::Square, icon::SMALL, theme::muted())
            })
            .child("Viewed")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.model.review_files.toggle_viewed(&target);
                    cx.notify();
                }),
            );
        let header = div()
            .px_3()
            .py_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(theme::code_font())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(path.to_owned()),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(theme::muted())
                    .child(format!("+{added} −{removed}")),
            )
            .child(toggle);
        let lines: Vec<&DiffLine> = [&sides.staged, &sides.unstaged]
            .into_iter()
            .flatten()
            .flat_map(|diff| diff.hunks.iter())
            .flat_map(|hunk| hunk.lines.iter())
            .collect();
        let hidden = lines.len().saturating_sub(LINES_SHOWN);
        // A viewed file folds to its header, so what is left to read stays in front.
        let body = (!viewed).then(|| {
            div()
                .flex()
                .flex_col()
                .font_family(theme::code_font())
                .text_xs()
                .children(lines.into_iter().take(LINES_SHOWN).map(diff_line))
                .children((hidden > 0).then(|| {
                    div()
                        .px_3()
                        .py_1()
                        .text_color(theme::muted())
                        .child(format!("{hidden} more lines; open the file on Changes."))
                }))
        });
        div()
            .flex_none()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(theme::hairline())
            .child(header)
            .children(body)
    }
}

/// The text colour and tint of a status chip, which the trend dots share the text colour of.
fn status_colours(tone: StatusTone) -> (Hsla, Hsla) {
    match tone {
        StatusTone::Pass => (theme::added_fg(), theme::added_bg()),
        StatusTone::NeedsWork => (theme::warn_fg(), theme::warn_bg()),
        StatusTone::Fail => (theme::removed_fg(), theme::removed_bg()),
    }
}

/// A label's chip, drawn by its weight: praise, note and thought muted, nitpick, suggestion,
/// todo and chore plain, issue stronger, question in the accent colour.
pub(crate) fn label_chip(label: CommentLabel) -> Div {
    let (background, colour) = match review_scorecards::label_weight(label) {
        Weight::Muted => (theme::selected_row(), theme::muted()),
        Weight::Normal => (theme::selected_row(), theme::text()),
        Weight::Strong => (theme::warn_bg(), theme::warn_fg()),
        Weight::Accent => (theme::selection(), theme::accent()),
    };
    tag(label.as_str(), background).text_color(colour)
}

/// A decoration's chip: `blocking` is a danger chip, the others are plain.
pub(crate) fn decoration_chip(decoration: &Decoration) -> Div {
    let (background, colour) = if decoration.danger {
        (theme::removed_bg(), theme::removed_fg())
    } else {
        (theme::selected_row(), theme::muted())
    };
    tag(&decoration.word, background).text_color(colour)
}

/// The five trend dots, oldest to newest: a filled dot per result, an empty circle where the
/// name has none.
fn trend_dots(dots: &[Dot]) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .children(dots.iter().map(|dot| {
            let base = div().w(px(8.)).h(px(8.)).rounded_full();
            match dot {
                Dot::Result(tone) => base.bg(status_colours(*tone).0),
                Dot::Empty => base.border_1().border_color(theme::muted()),
            }
        }))
}

fn diff_line(line: &DiffLine) -> Div {
    let (mark, background, colour) = match line.kind {
        LineKind::Added => ("+", theme::added_bg(), theme::added_fg()),
        LineKind::Removed => ("−", theme::removed_bg(), theme::removed_fg()),
        LineKind::Context => (" ", theme::ground(), theme::text()),
    };
    div()
        .px_3()
        .flex()
        .flex_row()
        .gap_2()
        .bg(background)
        .child(div().flex_none().w(px(10.)).text_color(colour).child(mark))
        .child(
            div()
                .min_w(px(0.))
                .whitespace_nowrap()
                .overflow_hidden()
                .child(line.text.clone()),
        )
}

fn card(title: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(theme::hairline())
        .bg(theme::panel())
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::muted())
                .child(title.to_uppercase()),
        )
}

fn muted_line(text: &str) -> Div {
    div()
        .text_xs()
        .text_color(theme::muted())
        .child(text.to_owned())
}
