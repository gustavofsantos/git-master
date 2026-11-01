//! The Labels page (P4, D145): the labels of the repository on the left, with a filter
//! box above them; the selected label on the right, with what is attached to it,
//! grouped by kind. The model is `labels_page.rs`; this file only draws it and sends
//! the reads it plans. A filter re-reads the list 250 ms after the last keystroke,
//! and Esc clears the filter.

use std::ops::Range;
use std::time::Duration;

use gm_schema::threads::{AttachKind, Label, LabelAttachment};
use gm_schema::{Actor, CommandKind, Event};
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    Stateful, Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use crate::icon::{self, Icon};
use crate::labels_page::{self, DetailRow, Empty};
use crate::shell_history::now_secs;
use crate::shell_threads::chip;
use crate::text_area::TextArea;
use crate::text_search::DEBOUNCE_MS;
use crate::theme;
use crate::threads;
use crate::view::{Screen, StatusView};

/// The left column's width: the filter box and the labels. Fixed, so the detail keeps the rest.
const LIST_WIDTH: f32 = 280.;
/// A label row's height in the list.
const LABEL_ROW_HEIGHT: f32 = 32.;
/// An attachment row's height: a title line and a line of its details. A group's heading
/// has the same height, since a uniform list draws rows of one height.
const DETAIL_ROW_HEIGHT: f32 = 44.;

/// The window's part of the page: the filter box and the lists' scroll. Its state is `model.labels`.
pub(crate) struct LabelsView {
    pub(crate) query: Entity<TextArea>,
    pub(crate) list: UniformListScrollHandle,
    pub(crate) detail: UniformListScrollHandle,
    /// Counts the filter's edits, so a delayed re-read knows whether it is still the latest.
    edits: u64,
}

impl LabelsView {
    pub(crate) fn new(query: Entity<TextArea>) -> Self {
        Self {
            query,
            list: UniformListScrollHandle::new(),
            detail: UniformListScrollHandle::new(),
            edits: 0,
        }
    }
}

/// The subscription that starts a delayed re-read when the filter changes. Made with the window.
pub(crate) fn subscriptions(
    query: &Entity<TextArea>,
    cx: &mut Context<StatusView>,
) -> Vec<Subscription> {
    vec![cx.observe(query, |this, _, cx| this.on_labels_edited(cx))]
}

/// Whether an engine event changes what the Labels page shows: a label write, or a
/// thread write, since a thread row shows its labels. Read before the model takes the event.
pub(crate) fn follows_change(event: &Event) -> bool {
    matches!(event, Event::LabelsChanged { .. } | Event::ThreadsChanged)
}

impl StatusView {
    /// Shows the page: the filter takes the cursor and the list is read again.
    pub(crate) fn show_labels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Labels;
        self.model.commit_stack.close();
        self.model.stash_stack.close();
        self.model.reflog_stack.close();
        self.request_labels_list();
        self.labels_view
            .query
            .update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    /// Reads the labels the filter asks for. Nothing is asked before a repository is
    /// open, or while an open is in flight.
    pub(crate) fn request_labels_list(&mut self) {
        if self.model.status().is_none() || self.model.is_opening() {
            return;
        }
        let kind = self.model.labels.plan_list();
        self.send_label_read(kind);
    }

    /// After a change the list and the selected label are read again, while the page is shown.
    pub(crate) fn follow_labels(&mut self, changed: bool) {
        if !changed || self.screen != Screen::Labels {
            return;
        }
        for kind in self.model.labels.changed() {
            self.send_label_read(kind);
        }
    }

    /// Dispatches a planned read and records which read the answer belongs to.
    fn send_label_read(&mut self, kind: CommandKind) {
        let is_list = matches!(kind, CommandKind::ListLabels { .. });
        let id = self.engine.dispatch(Actor::Human, kind);
        if is_list {
            self.model.labels.request_list(id);
        } else {
            self.model.labels.request_detail(id);
        }
    }

    /// The filter changed: the model takes the new text, and the list is read again
    /// after a pause, unless another edit came first.
    fn on_labels_edited(&mut self, cx: &mut Context<Self>) {
        if self.screen != Screen::Labels {
            return;
        }
        let text = self.labels_view.query.read(cx).text().to_owned();
        if text.contains('\n') {
            // The box is one line; Enter is not a line break.
            let first = text.lines().next().unwrap_or("").to_owned();
            self.labels_view
                .query
                .update(cx, |area, cx| area.set_text(&first, cx));
            return;
        }
        self.model.labels.set_query(&text);
        self.labels_view.edits += 1;
        let edit = self.labels_view.edits;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DEBOUNCE_MS))
                .await;
            let _ = this.update(cx, |view, _cx| {
                if view.labels_view.edits == edit {
                    view.request_labels_list();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Esc: the filter is cleared and the list shows every label again.
    fn clear_labels_filter(&mut self, cx: &mut Context<Self>) {
        self.labels_view
            .query
            .update(cx, |area, cx| area.set_text("", cx));
        cx.notify();
    }

    /// Selects label `id` (a click on its row) and reads what is attached to it.
    fn choose_label(&mut self, id: i64, cx: &mut Context<Self>) {
        let kind = self.model.labels.select(id);
        self.send_label_read(kind);
        cx.notify();
    }

    /// The page: a header with the count, then the list and the selected label.
    pub(crate) fn labels_page(&self, cx: &mut Context<Self>) -> Div {
        let page = &self.model.labels;
        let header = div()
            .h(px(44.))
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .border_b_1()
            .border_color(theme::hairline())
            .child(div().font_weight(FontWeight::MEDIUM).child("Labels"))
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(labels_page::count_label(page.labels().len())),
            );
        let body = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_row()
            .child(self.labels_list_column(cx))
            .child(self.labels_detail(cx));
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(theme::ground())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key.as_str() == "escape" && !this.model.labels.query().is_empty()
                {
                    this.clear_labels_filter(cx);
                }
            }))
            .child(header)
            .child(body)
    }

    /// The left column: the filter box, the failure of the last read, then the labels or
    /// what says there are none.
    fn labels_list_column(&self, cx: &mut Context<Self>) -> Div {
        let page = &self.model.labels;
        let filter = div()
            .h(px(40.))
            .px_3()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(theme::hairline())
            .child(icon::view(
                Icon::MagnifyingGlass,
                icon::SMALL,
                theme::muted(),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(self.labels_view.query.clone()),
            );
        let failure = page.failure().map(|text| {
            div()
                .px_3()
                .py_2()
                .text_sm()
                .text_color(theme::error())
                .child(text.to_owned())
        });
        let body: AnyElement = match page.empty() {
            _ if page.is_loading() && page.labels().is_empty() => {
                note("Loading labels…").into_any_element()
            }
            Some(Empty::NoLabels) => note("No labels yet").into_any_element(),
            Some(Empty::NoMatches) => note("No label matches").into_any_element(),
            None => label_rows(page.visible().len(), self.labels_view.list.clone(), cx)
                .into_any_element(),
        };
        div()
            .w(px(LIST_WIDTH))
            .min_w(px(LIST_WIDTH))
            .max_w(px(LIST_WIDTH))
            .flex_none()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::hairline())
            .child(filter)
            .children(failure)
            .child(body)
    }

    /// The right column: the selected label's name, description and origin, then the
    /// attachments under their group headings, or what the page is waiting for.
    fn labels_detail(&self, cx: &mut Context<Self>) -> Div {
        let page = &self.model.labels;
        let Some(detail) = page.detail() else {
            let text = if page.selected().is_some() {
                "Loading label…"
            } else {
                "Select a label to see what is attached to it."
            };
            return div()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .flex()
                .flex_col()
                .child(note(text));
        };
        let label = &detail.label;
        let (origin, agent) = labels_page::origin_text(&label.origin);
        let title = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .child(color_dot(label))
            .child(
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::MEDIUM)
                    .child(label.name.clone()),
            );
        let origin_line = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(theme::muted())
            .child("Created by")
            .children(agent.then(|| chip(origin.clone())))
            .children((!agent).then(|| div().child(origin.clone())));
        let header = div()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_1()
            .border_b_1()
            .border_color(theme::hairline())
            .child(title)
            .children((!label.description.is_empty()).then(|| {
                div()
                    .text_sm()
                    .text_color(theme::muted())
                    .child(label.description.clone())
            }))
            .child(origin_line);
        let count = page.detail_rows().len();
        let attachments: AnyElement = if count == 0 {
            note("Nothing is attached to this label yet.").into_any_element()
        } else {
            detail_rows(count, self.labels_view.detail.clone(), cx).into_any_element()
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(header)
            .child(attachments)
    }
}

/// The labels, read from the list as they are drawn. Each row selects its label.
fn label_rows(count: usize, list: UniformListScrollHandle, cx: &mut Context<StatusView>) -> Div {
    div().flex_1().min_h(px(0.)).flex().flex_col().child(
        uniform_list(
            "label-rows",
            count,
            cx.processor(
                move |this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut Window,
                      cx: &mut Context<StatusView>| {
                    let selected = this.model.labels.selected();
                    let labels = this.model.labels.visible();
                    labels
                        .get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(label, index)| {
                            let is_selected = selected == Some(label.id);
                            label_row(label, index, is_selected, cx)
                        })
                        .collect::<Vec<_>>()
                },
            ),
        )
        .track_scroll(list)
        .flex_1()
        .min_h(px(0.)),
    )
}

/// One label in the list: its colour, its name, an agent's name as a tag, and how many rows it holds.
fn label_row(
    label: &Label,
    index: usize,
    selected: bool,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let id = label.id;
    let (origin, agent) = labels_page::origin_text(&label.origin);
    let mut row = div()
        .id(("label-row", index))
        .cursor_pointer()
        .h(px(LABEL_ROW_HEIGHT))
        .px_3()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| this.choose_label(id, cx)),
        );
    if selected {
        row = row.bg(theme::selected_row());
    }
    row.child(color_dot(label))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label.name.clone()),
        )
        .children(agent.then(|| chip(origin)))
        .child(
            div()
                .flex_none()
                .px_2()
                .rounded_full()
                .bg(theme::selection())
                .text_color(theme::accent())
                .text_xs()
                .child(label.attachments.to_string()),
        )
}

/// The attachments of the selected label, read from the list as they are drawn.
fn detail_rows(count: usize, list: UniformListScrollHandle, cx: &mut Context<StatusView>) -> Div {
    div().flex_1().min_h(px(0.)).flex().flex_col().child(
        uniform_list(
            "label-detail-rows",
            count,
            cx.processor(
                move |this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut Window,
                      cx: &mut Context<StatusView>| {
                    let now = now_secs();
                    let rows = this.model.labels.detail_rows();
                    rows.get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(row, index)| match row {
                            DetailRow::Heading(kind) => heading_row(*kind).into_any_element(),
                            DetailRow::Attachment(attachment) => {
                                attachment_row(attachment, index, now, cx).into_any_element()
                            }
                        })
                        .collect::<Vec<_>>()
                },
            ),
        )
        .track_scroll(list)
        .flex_1()
        .min_h(px(0.)),
    )
}

/// The heading of one group of attachments.
fn heading_row(kind: AttachKind) -> Div {
    let title = match kind {
        AttachKind::Thread => "Threads",
        AttachKind::File => "Files",
        AttachKind::Commit => "Commits",
        AttachKind::Label => "Labels",
    };
    div()
        .h(px(DETAIL_ROW_HEIGHT))
        .px_4()
        .flex()
        .items_end()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::muted())
        .child(title)
}

/// One attachment: its title, then who made it, when, and what is worth knowing about
/// it (a commit's state, a file's old path). A thread row opens that thread.
fn attachment_row(
    attachment: &LabelAttachment,
    index: usize,
    now: i64,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let mut row = div()
        .id(("label-attachment", index))
        .h(px(DETAIL_ROW_HEIGHT))
        .px_4()
        .flex()
        .flex_col()
        .justify_center()
        .min_w(px(0.))
        .overflow_hidden();
    if attachment.kind == AttachKind::Thread {
        let uid = attachment.reference.clone();
        row = row.cursor_pointer().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.open_thread_from_commit(&uid, window, cx);
            }),
        );
    }
    let (origin, agent) = labels_page::origin_text(&attachment.origin);
    let extra = match attachment.kind {
        AttachKind::Commit => labels_page::commit_state_text(attachment.state)
            .map(|text| div().text_color(theme::warn_fg()).child(text)),
        AttachKind::File => attachment
            .current_path
            .as_deref()
            .filter(|path| *path != attachment.reference)
            .map(|_| {
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(format!("attached as {}", attachment.reference))
            }),
        AttachKind::Thread | AttachKind::Label => None,
    };
    let meta = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .text_xs()
        .text_color(theme::muted())
        .children(agent.then(|| chip(origin.clone())))
        .children((!agent).then(|| div().child(origin.clone())))
        .child(threads::relative(now, attachment.at))
        .children(extra);
    row.child(
        div()
            .min_w(px(0.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_sm()
            .child(labels_page::attachment_title(attachment)),
    )
    .child(meta)
}

/// The label's colour as a dot; a label without one shows a faint dot.
fn color_dot(label: &Label) -> Div {
    let colour = label
        .color
        .as_deref()
        .and_then(labels_page::hex_colour)
        .map_or_else(theme::faint, theme::color);
    div()
        .flex_none()
        .w(px(8.))
        .h(px(8.))
        .rounded_full()
        .bg(colour)
}

/// A line of text in the middle of a column, for the empty and loading states.
fn note(text: &str) -> Div {
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .items_center()
        .justify_center()
        .px_4()
        .text_color(theme::muted())
        .child(text.to_owned())
}

#[cfg(test)]
mod tests {
    use gm_schema::OperationId;

    use super::*;

    #[test]
    fn a_label_or_thread_change_is_followed_and_nothing_else() {
        assert!(follows_change(&Event::LabelsChanged {
            id: OperationId(1),
            label: None,
        }));
        assert!(follows_change(&Event::ThreadsChanged));
        assert!(!follows_change(&Event::LabelsListed {
            id: OperationId(1),
            labels: Vec::new(),
        }));
    }
}
