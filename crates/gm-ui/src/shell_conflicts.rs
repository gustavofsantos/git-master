//! The conflict resolver page (C3b, D22, D114, D115): a header with the file, the
//! operation, the counter and the buttons; then the conflicts of the left side,
//! the editable result in the middle and the conflicts of the right side. The
//! state is `conflict_resolver::ResolverState` in the model. This file draws what
//! it reports and turns clicks into its calls; Apply sends `ResolveConflict` to
//! the engine, the way the other commands are sent.

use gm_core::conflict::Resolution;
use gm_schema::{Actor, CommandKind, ConflictSite};
use gpui::{
    ClickEvent, Context, Div, ElementId, FontWeight, MouseButton, MouseDownEvent, Stateful, div,
    prelude::*, px,
};

use crate::conflict_resolver::{HunkView, ResolverState, RestackConflicts, Side, SuggestionView};
use crate::gate_wait;
use crate::theme;
use crate::view::{Screen, StatusView};

const HEADER_HEIGHT: f32 = 44.;
const LINE_HEIGHT: f32 = 18.;
const NO_LINES: &str = "(no lines)";
const NO_CONFLICTS: &str = "No conflicts in this file.";
const UNREADABLE: &str =
    "The result has conflict markers that cannot be read. Fix them in the result.";
const NO_CONFLICT_OPEN: &str = "No conflict is open.";

impl StatusView {
    /// Shows the Conflicts page for the file the model's resolver holds. Not gated by the
    /// preview flag: flags hide entry points, never a flow in progress (D169).
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

    /// Chooses the workflow's suggestion for the conflict `index` (D168).
    pub(crate) fn choose_suggestion(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(conflict) = self.model.conflict.as_mut() {
            conflict.select(index);
            conflict.choose_suggestion(index);
        }
        self.sync_conflict_result(cx);
        cx.notify();
    }

    /// Shows the whole of the long suggestion of the conflict `index`, or its preview again.
    pub(crate) fn toggle_suggestion(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(conflict) = self.model.conflict.as_mut() {
            conflict.toggle_suggestion(index);
        }
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

    /// Apply: sends the resolution, with who resolved each hunk, when every conflict
    /// is resolved. Otherwise the resolver's note says what remains, and nothing is sent.
    pub(crate) fn apply_conflict(&mut self, cx: &mut Context<Self>) {
        if let Some(conflict) = self.model.conflict.as_mut()
            && let Some((path, text, origins)) = conflict.apply()
        {
            let site = conflict.site();
            let id = self.engine.dispatch(
                Actor::Human,
                CommandKind::ResolveConflict {
                    path,
                    text,
                    origins,
                    site,
                },
            );
            self.model.conflict_sent = Some(id);
        }
        cx.notify();
    }

    /// Reads the conflicted file `path` of the open restack conflict, in its resolution
    /// worktree (D154). The resolver shows it when the engine answers.
    pub(crate) fn read_restack_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.engine.dispatch(
            Actor::Human,
            CommandKind::ReadConflict {
                path: path.to_owned(),
                site: ConflictSite::Restack,
            },
        );
        cx.notify();
    }

    /// Next file: reads the next conflicted file still to resolve after the open one.
    pub(crate) fn next_restack_file(&mut self, cx: &mut Context<Self>) {
        let next = self
            .model
            .conflict
            .as_ref()
            .zip(self.model.restacking.resolving())
            .and_then(|(conflict, restack)| {
                restack
                    .next_unresolved_after(conflict.path())
                    .map(str::to_owned)
            });
        if let Some(path) = next {
            self.read_restack_file(&path, cx);
        }
    }

    /// Review: loads the recorded resolution into the Result pane. The text stays
    /// editable, and the pane shows it at once.
    pub(crate) fn review_recorded(&mut self, cx: &mut Context<Self>) {
        if let Some(conflict) = self.model.conflict.as_mut() {
            conflict.use_recorded();
        }
        self.sync_conflict_result(cx);
        cx.notify();
    }

    /// Reads again the file whose recorded resolution was forgotten, once. The
    /// engine has restored its markers, so the read shows them.
    pub(crate) fn reread_forgotten_conflict(&mut self) {
        if let Some(path) = self.model.take_conflict_reread() {
            let site = self
                .model
                .conflict
                .as_ref()
                .map_or(ConflictSite::WorkingCopy, ResolverState::site);
            self.engine
                .dispatch(Actor::Human, CommandKind::ReadConflict { path, site });
        }
    }

    /// Forget: asks the engine to drop the recorded resolution. The file is read
    /// again when the engine confirms it.
    pub(crate) fn forget_recorded(&mut self, cx: &mut Context<Self>) {
        let request = self.model.conflict.as_ref().and_then(|conflict| {
            conflict
                .forget_request()
                .map(|path| (path, conflict.site()))
        });
        if let Some((path, site)) = request {
            self.engine.dispatch(
                Actor::Human,
                CommandKind::ForgetRecordedResolution { path, site },
            );
        }
        cx.notify();
    }

    /// The whole page: in a restack, its header with the files; then the header, the note,
    /// then the three panes.
    pub(crate) fn conflicts_page(&self, cx: &Context<Self>) -> Div {
        let restack = self.model.restacking.resolving();
        let page = div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(theme::ground());
        let Some(conflict) = self.model.conflict.as_ref() else {
            let message = restack.map_or(NO_CONFLICT_OPEN, RestackConflicts::empty_message);
            return page
                .children(restack.map(|restack| restack_header(restack, None, cx)))
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(theme::muted())
                        .child(message),
                );
        };
        let hunks = conflict.hunk_views();
        let selected = conflict.selected();
        page.children(restack.map(|restack| restack_header(restack, Some(conflict.path()), cx)))
            .child(Self::conflict_header(conflict, restack, cx))
            .children(
                conflict
                    .recorded_banner()
                    .map(|text| recorded_banner_row(text, cx)),
            )
            .children(conflict_note(
                conflict,
                self.model.conflict_waits_for_gates(),
                cx,
            ))
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
    fn conflict_header(
        conflict: &ResolverState,
        restack: Option<&RestackConflicts>,
        cx: &Context<Self>,
    ) -> Div {
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
                    .children(after_apply_button(conflict, restack, cx))
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

/// The button after a file is applied: Back to the working copy, or in a restack the next
/// file still to resolve. Once every file is applied there is none, and Continue restack
/// goes on from the restack header.
fn after_apply_button(
    conflict: &ResolverState,
    restack: Option<&RestackConflicts>,
    cx: &Context<StatusView>,
) -> Option<Stateful<Div>> {
    if !conflict.is_staged() {
        return None;
    }
    match restack {
        None => Some(header_button("conflict-back", "Back", cx, |this, cx| {
            this.show_working_copy(cx);
        })),
        Some(restack) => (restack.unresolved() > 0).then(|| {
            header_button("conflict-next-file", "Next file", cx, |this, cx| {
                this.next_restack_file(cx);
            })
        }),
    }
}

/// The restack header (D154): the branch being resolved, the commit being replayed and the new
/// parent, the conflicted files (the open one is highlighted, an applied one says Resolved),
/// Cancel and Continue restack, and the line that says why Continue did not run.
fn restack_header(
    restack: &RestackConflicts,
    current: Option<&str>,
    cx: &Context<StatusView>,
) -> Div {
    let files = restack.files().iter().enumerate().map(|(index, file)| {
        let path = file.path.clone();
        let open = current == Some(file.path.as_str());
        div()
            .id(("restack-file", index))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .h(px(24.))
            .rounded_sm()
            .cursor_pointer()
            .when(open, |row| row.bg(theme::selected_row()))
            .child(
                div()
                    .font_family(theme::code_font())
                    .text_xs()
                    .text_color(theme::text())
                    .child(file.path.clone()),
            )
            .children(file.resolved.then(|| {
                div()
                    .text_xs()
                    .text_color(theme::added_fg())
                    .child("Resolved")
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.read_restack_file(&path, cx);
            }))
    });
    let title = div()
        .flex()
        .flex_col()
        .min_w(px(0.))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .font_family(theme::code_font())
                .child(restack.resolution().conflicted.clone()),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::muted())
                .child(restack.header()),
        );
    let actions = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .flex_none()
        .child(header_button(
            "restack-cancel",
            "Cancel",
            cx,
            StatusView::cancel_restack_resolution,
        ))
        .child(header_button(
            "restack-continue",
            "Continue restack",
            cx,
            StatusView::continue_restack_resolution,
        ));
    div()
        .px_4()
        .py_2()
        .flex()
        .flex_col()
        .gap_2()
        .border_b_1()
        .border_color(theme::hairline())
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_3()
                .child(title)
                .child(actions),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_1p5()
                .children(files),
        )
        .children(restack.note().map(|note| {
            div()
                .text_sm()
                .text_color(theme::warn_fg())
                .child(note.to_owned())
        }))
}

/// The line under the header: what the last Apply or choice said, or that the file
/// was staged. Nothing when there is nothing to say.
fn conflict_note(conflict: &ResolverState, waiting: bool, cx: &Context<StatusView>) -> Option<Div> {
    let note = if waiting {
        gate_wait::WAITING
    } else {
        conflict.applied_note()?
    };
    let colour = if !waiting && conflict.is_staged() {
        theme::added_fg()
    } else {
        theme::warn_fg()
    };
    let text = div().text_sm().text_color(colour).child(note.to_owned());
    Some(if waiting {
        div()
            .px_4()
            .py_1p5()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .child(text)
            .child(crate::shell_commit::gate_cancel_link(cx))
    } else {
        div().px_4().py_1p5().child(text)
    })
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
    let [left_id, right_id, both_id, clear_id, suggest_id, show_id] = side_ids(side);
    let lines = match side {
        Side::Left => hunk.left.as_str(),
        Side::Right => hunk.right.as_str(),
    };
    // The buttons wrap under the heading when the card is narrow, so the suggestion's label is never cut.
    let mut buttons = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_1()
        .min_w(px(0.))
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
    if let Some(suggestion) = &hunk.suggestion {
        buttons = buttons.child(hunk_button(
            (suggest_id, index),
            suggestion.label.clone(),
            cx,
            move |this, cx| this.choose_suggestion(index, cx),
        ));
    }
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
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .pb_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::muted())
                        .child(hunk_status(hunk)),
                )
                .child(buttons),
        )
        .children(
            hunk.suggestion
                .as_ref()
                .map(|suggestion| suggestion_text(suggestion, (show_id, index), cx)),
        )
        .children(hunk.base.as_deref().map(|base| {
            div()
                .pb_1()
                .child(div().text_xs().text_color(theme::muted()).child("Base"))
                .child(code_lines(base))
        }))
        .child(code_lines(lines))
}

/// The text a workflow suggested for a hunk, under the card's header, with Show or Hide when it is
/// long (D168). The text is for reading: it is applied only by the Script suggestion option.
fn suggestion_text(
    suggestion: &SuggestionView,
    toggle: (&'static str, usize),
    cx: &Context<StatusView>,
) -> Div {
    let toggle_label = if suggestion.expanded { "Hide" } else { "Show" };
    div()
        .pb_1()
        .flex()
        .flex_col()
        .gap_1()
        .child(code_lines(&suggestion.text))
        .children(suggestion.long.then(|| {
            hunk_button(toggle, toggle_label, cx, move |this, cx| {
                this.toggle_suggestion(toggle.1, cx);
            })
        }))
}

/// The heading of a conflict's card: its number, its choice and who made it, for
/// example "Conflict 1 · Left · human". The origin is left out while unresolved.
fn hunk_status(hunk: &HunkView) -> String {
    let heading = format!("Conflict {} · {}", hunk.index + 1, hunk.resolution_label);
    if hunk.origin_label.is_empty() {
        heading
    } else {
        format!("{heading} · {}", hunk.origin_label)
    }
}

/// The banner under the header while Git has a recorded resolution for the file:
/// Review loads it into the Result pane, Forget removes it.
fn recorded_banner_row(text: &'static str, cx: &Context<StatusView>) -> Div {
    div()
        .px_4()
        .py_1p5()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_2()
        .border_b_1()
        .border_color(theme::hairline())
        .child(div().text_sm().child(text))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_none()
                .child(header_button(
                    "conflict-recorded-review",
                    "Review",
                    cx,
                    StatusView::review_recorded,
                ))
                .child(header_button(
                    "conflict-recorded-forget",
                    "Forget",
                    cx,
                    StatusView::forget_recorded,
                )),
        )
}

/// The element ids of one side's buttons: Left, Right, Both, Clear, the Script suggestion and its
/// Show toggle. Ids must be unique among the siblings, and the two panes hold the same conflict indexes.
fn side_ids(side: Side) -> [&'static str; 6] {
    match side {
        Side::Left => [
            "conflict-left-pick-left",
            "conflict-left-pick-right",
            "conflict-left-pick-both",
            "conflict-left-clear",
            "conflict-left-pick-suggestion",
            "conflict-left-suggestion-show",
        ],
        Side::Right => [
            "conflict-right-pick-left",
            "conflict-right-pick-right",
            "conflict-right-pick-both",
            "conflict-right-clear",
            "conflict-right-pick-suggestion",
            "conflict-right-suggestion-show",
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
    label: impl Into<String>,
    cx: &Context<StatusView>,
    on_click: impl Fn(&mut StatusView, &mut Context<StatusView>) + 'static,
) -> Stateful<Div> {
    button(id.into(), label, cx, on_click)
}

/// A button on one hunk card.
fn hunk_button(
    id: (&'static str, usize),
    label: impl Into<String>,
    cx: &Context<StatusView>,
    on_click: impl Fn(&mut StatusView, &mut Context<StatusView>) + 'static,
) -> Stateful<Div> {
    button(id.into(), label, cx, on_click)
}

fn button(
    id: ElementId,
    label: impl Into<String>,
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
        .child(label.into())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| on_click(this, cx)),
        )
}
