//! The command log panel: a button on the status bar opens a read-only list of
//! the git commands that changed the repository, each with its result and
//! git's own output. Failures are red; a hook's report shows in full.

use gm_core::command_log::CommandRecord;
use gpui::{Context, Div, FontWeight, MouseButton, MouseDownEvent, Stateful, div, prelude::*, px};

use crate::icon::{self, Icon};
use crate::theme;
use crate::view::StatusView;

/// The newest records drawn; the log keeps more.
const SHOWN: usize = 200;
const PANEL_WIDTH: f32 = 680.;
const PANEL_HEIGHT: f32 = 320.;
const STATUS_BAR_HEIGHT: f32 = 26.;

/// How a record ended, then git's own words.
fn detail(record: &CommandRecord) -> Vec<String> {
    let ended = match record.code {
        Some(0) => "ok".to_owned(),
        Some(code) => format!("exit {code}"),
        None => "killed by a signal".to_owned(),
    };
    let mut lines = vec![format!(
        "{ended}, {} ms, {} bytes out",
        record.took.as_millis(),
        record.stdout_bytes
    )];
    lines.extend(
        record
            .stdout
            .lines()
            .chain(record.stderr.lines())
            .map(str::to_owned),
    );
    lines
}

impl StatusView {
    pub(crate) fn toggle_command_log(&mut self, cx: &mut Context<Self>) {
        self.log_open = !self.log_open;
        self.scroll_log_to_end();
        cx.notify();
    }

    pub(crate) fn on_command_logged(&mut self, cx: &mut Context<Self>) {
        if self.log_open {
            self.scroll_log_to_end();
        }
        cx.notify();
    }

    fn scroll_log_to_end(&self) {
        let shown = self.command_log.entries().len().min(SHOWN);
        self.log_scroll.scroll_to_item(shown.saturating_sub(1));
    }

    /// The status bar's button; red while the newest command failed.
    pub(crate) fn command_log_button(&self, cx: &Context<Self>) -> Stateful<Div> {
        let failed = self
            .command_log
            .entries()
            .last()
            .is_some_and(|record| !record.succeeded());
        let colour = if failed {
            theme::removed_fg()
        } else {
            theme::muted()
        };
        let icon = if self.log_open {
            icon::filled(Icon::Terminal, icon::SMALL, colour)
        } else {
            icon::view(Icon::Terminal, icon::SMALL, colour)
        };
        div()
            .id("command-log-button")
            .px_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .rounded_sm()
            .cursor_pointer()
            .text_color(colour)
            .hover(|style| style.bg(theme::selected_row()))
            .child(icon)
            .child("Git log")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.toggle_command_log(cx)),
            )
    }

    /// The floating panel above the status bar, oldest command first.
    pub(crate) fn command_log_panel(&self, cx: &Context<Self>) -> Div {
        let entries = self.command_log.entries();
        let skipped = entries.len().saturating_sub(SHOWN);
        let rows = entries.into_iter().skip(skipped).map(|record| {
            let colour = if record.succeeded() {
                theme::muted()
            } else {
                theme::removed_fg()
            };
            let lines = detail(&record).into_iter().map(move |line| {
                div()
                    .pl(px(16.))
                    .text_color(colour)
                    .child(if line.is_empty() {
                        " ".to_owned()
                    } else {
                        line
                    })
            });
            div()
                .flex()
                .flex_col()
                .pb(px(6.))
                .child(
                    div()
                        .text_color(if record.succeeded() {
                            theme::text()
                        } else {
                            theme::removed_fg()
                        })
                        .child(format!("$ {}", record.line())),
                )
                .children(lines)
        });
        let body = if skipped == 0 && self.command_log.entries().is_empty() {
            div()
                .p_3()
                .text_color(theme::muted())
                .child("No commands yet. Staging, unstaging and committing show up here.")
        } else {
            div().size_full().min_h(px(0.)).child(
                div()
                    .id("command-log")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.log_scroll)
                    .font_family(theme::code_font())
                    .text_xs()
                    .p_3()
                    .children(rows),
            )
        };
        let close = div()
            .id("command-log-close")
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.bg(theme::selected_row()))
            .child(icon::view(Icon::Close, icon::SMALL, theme::muted()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.toggle_command_log(cx)),
            );
        div()
            .absolute()
            .bottom(px(STATUS_BAR_HEIGHT))
            .right(px(8.))
            .w(px(PANEL_WIDTH))
            .h(px(PANEL_HEIGHT))
            .occlude()
            .flex()
            .flex_col()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::hairline_strong())
            .rounded_md()
            .shadow_lg()
            .child(
                div()
                    .h(px(30.))
                    .px_3()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Git commands"),
                    )
                    .child(close),
            )
            .child(body)
    }
}
