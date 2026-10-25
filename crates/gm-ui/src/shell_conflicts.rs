//! The conflict resolver page (C3b, D22, D114, D115): a header with the file, the
//! operation, the counter and the buttons; then the conflicts of the left side,
//! the editable result in the middle and the conflicts of the right side. The
//! state is `conflict_resolver::ResolverState` in the model. This file draws what
//! it reports and turns clicks into its calls; Apply sends `ResolveConflict` to
//! the engine, the way the other commands are sent.

use gm_core::conflict::Resolution;
use gm_schema::{Actor, CommandKind, HunkOrigin};
use gpui::{
    Context, Div, ElementId, FontWeight, MouseButton, MouseDownEvent, Stateful, div, prelude::*, px,
};

use crate::conflict_resolver::{HunkView, ResolverState, Side};
use crate::theme;
use crate::view::{Screen, StatusView};

const HEADER_HEIGHT: f32 = 44.;
const LINE_HEIGHT: f32 = 18.;
const NO_LINES: &str = "(no lines)";
const NO_CONFLICTS: &str = "No conflicts in this file.";
const UNREADABLE: &str =
    "The result has conflict markers that cannot be read. Fix them in the result.";

impl StatusView {
    /// Shows the Conflicts page for the file the model's resolver holds.
    pub(crate) fn show_conflicts(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::Conflicts;
        self.sync_conflict_result(cx);
        cx.notify();
    }

    /// Gives the Result pane the resolver's text, when the two differ. A choice
    /// changes the text this way; a typed edit is already the resolver's text.
    fn sync_conflict_result(&self, cx: &mut Context<Self>) {
        let Some(text) = self.model.conflict.as_ref().map(ResolverState::result_text) else {
            return;
        };
        self.conflict_result.update(cx, |area, cx| {
            if area.text() != text {
                area.set_text(&text, cx);
            }
        });
    }

    /// The Result pane's text changed by typing: it becomes the resolver's result,
    /// which re-parses it so the counter follows.
    pub(crate) fn on_conflict_result_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.conflict_result.read(cx).text().to_owned();
        if let Some(conflict) = self.model.conflict.as_mut()
            && conflict.result_text() != text
        {
            conflict.set_result_text(text);
            cx.notify();
        }
    }

    /// Chooses `resolution` for the conflict `index` and points the panes at it.
    pub(crate) fn choose_conflict(
        &mut self,
        index: usize,
        resolution: Resolution,
        cx: &mut Context<Self>,
    ) {
        if let Some(conflict) = self.model.conflict.as_mut() {
            conflict.select(index);
            conflict.choose(index, resolution);
        }
        self.sync_conflict_result(cx);
        cx.notify();
    }

    /// Takes the choice back from the conflict `index`, so it shows its markers again.
    pub(crate) fn clear_conflict(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(conflict) = self.model.conflict.as_mut() {
            conflict.select(index);
            conflict.clear(index);
        }
        self.sync_conflict_result(cx);
        cx.notify();
    }

    /// Points the panes at the conflict `index`.
    pub(crate) fn select_conflict(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(conflict) = self.model.conflict.as_mut() {
            conflict.select(index);
        }
        cx.notify();
    }

    /// Moves to the next or the previous unresolved conflict.
    pub(crate) fn step_conflict(&mut self, next: bool, cx: &mut Context<Self>) {
        if let Some(conflict) = self.model.conflict.as_mut() {
            if next {
                conflict.select_next();
            } else {
                conflict.select_prev();
            }
        }
        cx.notify();
    }

    pub(crate) fn toggle_conflict_base(&mut self, cx: &mut Context<Self>) {
        if let Some(conflict) = self.model.conflict.as_mut() {
            conflict.toggle_base();
        }
        cx.notify();
    }

    /// Apply: sends the resolution when every conflict is resolved. Otherwise the
    /// resolver's note says what remains, and nothing is sent.
    pub(crate) fn apply_conflict(&mut self, cx: &mut Context<Self>) {
        if let Some(conflict) = self.model.conflict.as_mut() {
            // The resolver takes every hunk from the engineer's choices, so each is Human.
            let origins = vec![HunkOrigin::Human; conflict.conflict_count()];
            if let Some((path, text)) = conflict.apply() {
                self.engine.dispatch(
                    Actor::Human,
                    CommandKind::ResolveConflict {
                        path,
                        text,
                        origins,
                    },
                );
            }
        }
        cx.notify();
    }

    /// The whole page: the header, the note, then the three panes.
    pub(crate) fn conflicts_page(&self, cx: &Context<Self>) -> Div {
        let Some(conflict) = self.model.conflict.as_ref() else {
            return div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::muted())
                .child("No conflict is open.");
        };
        let hunks = conflict.hunk_views();
        let selected = conflict.selected();
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(theme::ground())
            .child(Self::conflict_header(conflict, cx))
            .children(conflict_note(conflict))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_row()
                    .child(side_pane(conflict, Side::Left, &hunks, selected, cx))
                    .child(self.result_pane())
                    .child(side_pane(conflict, Side::Right, &hunks, selected, cx)),
            )
    }

    /// The file, its operation and counter on the left; the base toggle, the
    /// steps, Back once staged and Apply on the right. Apply is never disabled.
    fn conflict_header(conflict: &ResolverState, cx: &Context<Self>) -> Div {
        let base_label = if conflict.show_base() {
            "Hide base"
        } else {
            "Show base"
        };
        div()
            .min_h(px(HEADER_HEIGHT))
            .px_4()
            .py_2()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .border_b_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .min_w(px(0.))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .font_family(theme::code_font())
                            .child(conflict.path().to_owned()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(conflict.operation_label()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(conflict.counter_label()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_none()
                    .child(header_button(
                        "conflict-base",
                        base_label,
                        cx,
                        Self::toggle_conflict_base,
                    ))
                    .child(header_button(
                        "conflict-prev",
                        "Previous unresolved",
                        cx,
                        |this, cx| this.step_conflict(false, cx),
                    ))
                    .child(header_button(
                        "conflict-next",
                        "Next unresolved",
                        cx,
                        |this, cx| this.step_conflict(true, cx),
                    ))
                    .children(conflict.is_staged().then(|| {
                        header_button("conflict-back", "Back", cx, |this, cx| {
                            this.show_working_copy(cx);
                        })
                    }))
                    .child(header_button("conflict-apply", "Apply", cx, |this, cx| {
                        this.apply_conflict(cx);
                    })),
            )
    }

    /// The middle pane: the result, which the Result text area edits.
    fn result_pane(&self) -> Div {
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(div().font_weight(FontWeight::MEDIUM).child("Result"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child("Edit the text directly. Apply writes it to the file."),
                    ),
            )
            .child(
                div()
                    .id("conflict-result")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p_2()
                    .child(self.conflict_result.clone()),
            )
    }
}

/// The line under the header: what the last Apply or choice said, or that the file
/// was staged. Nothing when there is nothing to say.
fn conflict_note(conflict: &ResolverState) -> Option<Div> {
    let note = conflict.applied_note()?;
    let colour = if conflict.is_staged() {
        theme::added_fg()
    } else {
        theme::warn_fg()
    };
    Some(
        div()
            .px_4()
            .py_1p5()
            .text_sm()
            .text_color(colour)
            .child(note.to_owned()),
    )
}

/// One side's pane: its title and subtitle, then each conflict's text of that side.
fn side_pane(
    conflict: &ResolverState,
    side: Side,
    hunks: &[HunkView],
    selected: usize,
    cx: &Context<StatusView>,
) -> Div {
    let (tag, list_id) = match side {
        Side::Left => ("conflict-left", "conflict-left-list"),
        Side::Right => ("conflict-right", "conflict-right-list"),
    };
    let body = if hunks.is_empty() {
        let message = if conflict.is_readable() {
            NO_CONFLICTS
        } else {
            UNREADABLE
        };
        div()
            .p_3()
            .text_sm()
            .text_color(theme::muted())
            .child(message)
            .into_any_element()
    } else {
        div()
            .id(list_id)
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .p_2()
            .flex()
            .flex_col()
            .gap_2()
            .children(
                hunks
                    .iter()
                    .map(|hunk| hunk_card(hunk, side, tag, hunk.index == selected, cx)),
            )
            .into_any_element()
    };
    div()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(theme::hairline())
        .child(
            div()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(theme::hairline())
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(conflict.side_title(side)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::muted())
                        .child(conflict.side_subtitle(side)),
                ),
        )
        .child(body)
}

/// One conflict in a side's pane: its number and choice, its buttons, the base
/// when it is shown, and this side's lines. A click on the card selects it.
fn hunk_card(
    hunk: &HunkView,
    side: Side,
    tag: &'static str,
    selected: bool,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let index = hunk.index;
    let [left_id, right_id, both_id, clear_id] = side_ids(side);
    let lines = match side {
        Side::Left => hunk.left.as_str(),
        Side::Right => hunk.right.as_str(),
    };
    let mut buttons = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_none()
        .child(hunk_button(
            (left_id, index),
            "Left",
            cx,
            move |this, cx| this.choose_conflict(index, Resolution::Left, cx),
        ))
        .child(hunk_button(
            (right_id, index),
            "Right",
            cx,
            move |this, cx| this.choose_conflict(index, Resolution::Right, cx),
        ))
        .child(hunk_button(
            (both_id, index),
            "Both",
            cx,
            move |this, cx| this.choose_conflict(index, Resolution::Both, cx),
        ));
    if hunk.resolved {
        buttons = buttons.child(hunk_button(
            (clear_id, index),
            "Clear",
            cx,
            move |this, cx| this.clear_conflict(index, cx),
        ));
    }
    div()
        .id((tag, index))
        .w_full()
        .p_2()
        .rounded_sm()
        .border_1()
        .border_color(theme::hairline())
        .when(selected, |card| {
            card.border_color(theme::accent()).bg(theme::selected_row())
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.select_conflict(index, cx);
            }),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .pb_1()
                .child(div().text_xs().text_color(theme::muted()).child(format!(
                    "Conflict {} · {}",
                    index + 1,
                    hunk.resolution_label
                )))
                .child(buttons),
        )
        .children(hunk.base.as_deref().map(|base| {
            div()
                .pb_1()
                .child(div().text_xs().text_color(theme::muted()).child("Base"))
                .child(code_lines(base))
        }))
        .child(code_lines(lines))
}

/// The element ids of one side's buttons, Left, Right, Both and Clear. Ids must be
/// unique among the siblings, and the two panes hold the same conflict indexes.
fn side_ids(side: Side) -> [&'static str; 4] {
    match side {
        Side::Left => [
            "conflict-left-pick-left",
            "conflict-left-pick-right",
            "conflict-left-pick-both",
            "conflict-left-clear",
        ],
        Side::Right => [
            "conflict-right-pick-left",
            "conflict-right-pick-right",
            "conflict-right-pick-both",
            "conflict-right-clear",
        ],
    }
}

/// The text of one side as monospace lines, each line a row so that blank lines keep their height.
fn code_lines(text: &str) -> Div {
    if text.is_empty() {
        return div().text_sm().text_color(theme::muted()).child(NO_LINES);
    }
    div()
        .flex()
        .flex_col()
        .font_family(theme::code_font())
        .text_sm()
        .children(
            text.lines()
                .map(|line| div().min_h(px(LINE_HEIGHT)).child(line.to_owned())),
        )
}

/// A header button. It is never disabled: each click runs its handler.
fn header_button(
    id: &'static str,
    label: &'static str,
    cx: &Context<StatusView>,
    on_click: impl Fn(&mut StatusView, &mut Context<StatusView>) + 'static,
) -> Stateful<Div> {
    button(id.into(), label, cx, on_click)
}

/// A button on one hunk card.
fn hunk_button(
    id: (&'static str, usize),
    label: &'static str,
    cx: &Context<StatusView>,
    on_click: impl Fn(&mut StatusView, &mut Context<StatusView>) + 'static,
) -> Stateful<Div> {
    button(id.into(), label, cx, on_click)
}

fn button(
    id: ElementId,
    label: &'static str,
    cx: &Context<StatusView>,
    on_click: impl Fn(&mut StatusView, &mut Context<StatusView>) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px_2()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(theme::button_border())
        .text_xs()
        .text_color(theme::text())
        .cursor_pointer()
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| on_click(this, cx)),
        )
}
