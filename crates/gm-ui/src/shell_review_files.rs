//! The Review page's left cards and its continuous diff (UI redesign, slice 4): the Changes
//! of the branch, the gates that exist today, Reviewers (planned, layout only), and every
//! changed file's diff one after another with a **Viewed** box each. The conversations list
//! and the selected conversation's detail stay in `shell_review`.

use gm_schema::{Actor, CommandKind, DiffLine, LineKind, WorkflowInfo};
use gpui::{
    Context, Div, FontWeight, MouseButton, MouseDownEvent, SharedString, Stateful, div, prelude::*,
    px,
};

use crate::icon::{self, Icon};
use crate::review_files::Sides;
use crate::theme;
use crate::view::{Screen, StatusView};

/// The width of the column of cards and conversations.
pub(crate) const SIDE_WIDTH: f32 = 320.;
/// The most lines drawn of one file in the continuous diff; the rest is counted.
const LINES_SHOWN: usize = 400;

/// A gate that exists today: a `before.<operation>` workflow switched on in the repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Gate {
    pub(crate) operation: String,
    pub(crate) name: String,
    /// A gate not reviewed in this version does not run and stops its operation.
    pub(crate) reviewed: bool,
}

/// The gates among `workflows`, by operation then name.
#[must_use]
pub(crate) fn gates(workflows: &[WorkflowInfo]) -> Vec<Gate> {
    let mut gates: Vec<Gate> = workflows
        .iter()
        .filter(|workflow| workflow.enabled && workflow.error.is_none())
        .filter_map(|workflow| {
            let operation = workflow.trigger.strip_prefix("before.")?;
            Some(Gate {
                operation: operation.to_owned(),
                name: workflow.name.clone(),
                reviewed: workflow.reviewed,
            })
        })
        .collect();
    gates.sort_by(|a, b| a.operation.cmp(&b.operation).then(a.name.cmp(&b.name)));
    gates
}

/// What the Gates card says about Push with lease: no operation `push` has gates today.
#[must_use]
pub(crate) fn push_line(gates: &[Gate]) -> String {
    match gates.iter().filter(|gate| gate.operation == "push").count() {
        0 => "No gate runs before Push with lease.".to_owned(),
        1 => "1 gate runs before Push with lease.".to_owned(),
        n => format!("{n} gates run before Push with lease."),
    }
}

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

    /// The left column's cards: Changes, Gates and Reviewers.
    pub(crate) fn review_cards(&self) -> Div {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(theme::hairline())
            .child(self.changes_card())
            .child(self.gates_card())
            .child(card("Reviewers").child(muted_line(
                "Reviewers and their verdicts are not available yet.",
            )))
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

    fn gates_card(&self) -> Div {
        let gates = gates(self.model.workflows.listed.as_deref().unwrap_or_default());
        let mut body = card("Gates");
        if gates.is_empty() {
            body = body.child(muted_line("No gate is switched on."));
        }
        for gate in &gates {
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        div()
                            .text_color(theme::muted())
                            .child(format!("before {}", gate.operation)),
                    )
                    .child(gate.name.clone())
                    .children(
                        (!gate.reviewed)
                            .then(|| div().text_color(theme::warn_fg()).child("needs review")),
                    ),
            );
        }
        body.child(muted_line(&push_line(&gates)))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow(name: &str, trigger: &str, enabled: bool, reviewed: bool) -> WorkflowInfo {
        WorkflowInfo {
            name: name.into(),
            path: format!("/w/{name}.lua"),
            trigger: trigger.into(),
            key: None,
            inputs: Vec::new(),
            hash: "h".into(),
            error: None,
            warnings: Vec::new(),
            source: String::new(),
            reviewed,
            auto_approve: false,
            enabled,
            title: None,
        }
    }

    #[test]
    fn the_gates_are_the_before_workflows_switched_on() {
        let gates = gates(&[
            workflow("b", "before.restack", true, true),
            workflow("a", "before.commit", true, false),
            workflow("off", "before.commit", false, true),
            workflow("m", "manual", true, true),
        ]);
        assert_eq!(
            gates,
            vec![
                Gate {
                    operation: "commit".into(),
                    name: "a".into(),
                    reviewed: false
                },
                Gate {
                    operation: "restack".into(),
                    name: "b".into(),
                    reviewed: true
                },
            ]
        );
        assert_eq!(push_line(&gates), "No gate runs before Push with lease.");
    }
}
