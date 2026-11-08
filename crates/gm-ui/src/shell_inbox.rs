//! The Inbox page (UI redesign, slice 6): what needs the person in the current repository, in
//! four sections. The items come from `inbox::items` on every frame, so one leaves by itself
//! once its cause is gone. Selecting an item fills the top bar's context actions; Enter or a
//! double-click runs its primary action.

use gm_schema::Feature;
use gpui::{
    AnyElement, Context, Div, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, SharedString,
    Stateful, div, prelude::*, px,
};

use crate::inbox::{self, Awaiting, Inputs, Item, Section};
use crate::shell_history::relative_time;
use crate::theme;
use crate::view::StatusView;

impl StatusView {
    /// The items of the Inbox now.
    pub(crate) fn inbox_items(&self) -> Vec<Item> {
        let awaiting: Vec<Awaiting<'_>> = self
            .model
            .workflows
            .runs
            .iter()
            .filter_map(|(run, view)| {
                view.approval.as_deref().map(|summary| Awaiting {
                    run: *run,
                    workflow: &view.workflow,
                    summary,
                })
            })
            .collect();
        let branch = self.model.status().map(|status| &status.branch);
        inbox::items(&Inputs {
            entries: self.model.entries(),
            comments: self.model.review.comments(),
            awaiting: &awaiting,
            proposed_memories: self.model.memories.proposed_count(),
            stacks: if self.features.is_enabled(Feature::Stacks) {
                self.model.stacks.snapshot()
            } else {
                None
            },
            branch: branch.and_then(|branch| branch.head.as_deref()),
            upstream: branch.and_then(|branch| branch.upstream.as_deref()),
            behind: branch.map_or(0, |branch| branch.behind),
            resolver: self.features.is_enabled(Feature::ConflictResolver),
        })
    }

    /// The selected item, if it is still in the Inbox.
    pub(crate) fn inbox_selected_item(&self) -> Option<Item> {
        let key = self.inbox_selected.as_deref()?;
        self.inbox_items().into_iter().find(|item| item.key == key)
    }

    /// Runs the primary action of the selected item (Enter).
    fn run_inbox_primary(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if let Some((action, args)) = self
            .inbox_selected_item()
            .and_then(|item| item.actions.into_iter().next())
        {
            self.run_action(action, &args, window, cx);
        }
    }

    /// Moves the selection one item up or down.
    fn step_inbox(&mut self, down: bool, cx: &mut Context<Self>) {
        let items = self.inbox_items();
        let at = self
            .inbox_selected
            .as_deref()
            .and_then(|key| items.iter().position(|item| item.key == key));
        let next = match (at, down) {
            (None, _) => 0,
            (Some(at), true) => (at + 1).min(items.len().saturating_sub(1)),
            (Some(at), false) => at.saturating_sub(1),
        };
        self.inbox_selected = items.get(next).map(|item| item.key.clone());
        cx.notify();
    }

    pub(crate) fn inbox_page(&self, cx: &mut Context<Self>) -> Div {
        let items = self.inbox_items();
        let header = div()
            .h(px(44.))
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(theme::hairline())
            .child(div().font_weight(FontWeight::MEDIUM).child("Inbox"))
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child("This repository"),
            );
        let body: AnyElement = if items.is_empty() {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_color(theme::text()).child("Nothing needs you"))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::muted())
                        .child("Conflicts, approvals and work returned by agents show here."),
                )
                .into_any_element()
        } else {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
            let mut list = div().flex().flex_col();
            for section in Section::ALL {
                let in_section: Vec<&Item> = items
                    .iter()
                    .filter(|item| item.section == section)
                    .collect();
                if in_section.is_empty() {
                    continue;
                }
                list = list.child(
                    div()
                        .px_4()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::muted())
                        .child(format!(
                            "{} · {}",
                            section.title().to_uppercase(),
                            in_section.len()
                        )),
                );
                for item in in_section {
                    list = list.child(self.inbox_row(item, now, cx));
                }
            }
            div()
                .id("inbox-list")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(list)
                .into_any_element()
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(theme::ground())
            .track_focus(&self.list_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "enter" => this.run_inbox_primary(window, cx),
                    "down" => this.step_inbox(true, cx),
                    "up" => this.step_inbox(false, cx),
                    _ => {}
                }
            }))
            .child(header)
            .child(body)
    }

    fn inbox_row(&self, item: &Item, now: i64, cx: &mut Context<Self>) -> Stateful<Div> {
        let selected = self.inbox_selected.as_deref() == Some(item.key.as_str());
        let key = item.key.clone();
        let when = item.when.map(|ms| relative_time(now, ms.div_euclid(1000)));
        div()
            .id(SharedString::from(format!("inbox-{}", item.key)))
            .px_4()
            .py_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .border_b_1()
            .border_color(theme::hairline())
            .when(selected, |row| row.bg(theme::selected_row()))
            .hover(|style| style.bg(theme::selected_row()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(item.what.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .font_family(theme::code_font())
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(item.place.clone()),
                    ),
            )
            .children(when.map(|when| {
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(when)
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.inbox_selected = Some(key.clone());
                    this.list_focus.focus(window);
                    if event.click_count == 2 {
                        this.run_inbox_primary(window, cx);
                    }
                    cx.notify();
                }),
            )
    }
}
