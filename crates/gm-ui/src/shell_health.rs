//! The Health tab of the Code page (docs/guide/code-health.md, "The Health tab"): a filter box
//! above the hotspots, the dormancy breaks, the list of files by heat and the details of the
//! selected file. The state and every computation are in `health_page.rs`; this file only draws
//! it and sends the requests it plans. Rows are drawn by a `uniform_list`, so only the rows on
//! screen are built.

use std::ops::Range;

use gm_schema::{ChurnSummary, FileHealth, Partner, SPARK_BARS, Trend};
use gpui::{
    Context, Div, Entity, FocusHandle, FontWeight, Hsla, KeyDownEvent, MouseButton, MouseDownEvent,
    Pixels, ScrollStrategy, Stateful, Subscription, UniformListScrollHandle, Window, div,
    prelude::*, px, uniform_list,
};

use crate::health_page::{self, DORMANT_SHOWN, HealthState, heat_share, spark_heights};
use crate::icon::{self, Icon};
use crate::text_area::TextArea;
use crate::theme;
use crate::view::{Screen, StatusView};

/// A row of the hotspots list, as tall as a row of the file tree.
const ROW_HEIGHT: f32 = 24.;
/// The width of a heat bar's track.
const BAR_TRACK: f32 = 80.;
/// The width of the details panel.
const DETAILS_WIDTH: f32 = 320.;
/// The activity chart of a row: 40 px wide, 16 px tall.
const ROW_SPARK_WIDTH: f32 = 40.;
const ROW_SPARK_HEIGHT: u16 = 16;
/// The activity chart of the details: the panel's content width, 48 px tall.
const DETAIL_SPARK_WIDTH: f32 = DETAILS_WIDTH - 32.;
const DETAIL_SPARK_HEIGHT: u16 = 48;
/// The Churn section under the hotspots: a fixed height, and the survival chart's track, columns
/// and bar width. The track is as tall as the chart's bars can grow.
const CHURN_SECTION_HEIGHT: f32 = 120.;
const CHURN_BAR_MAX: f32 = 48.;
const CHURN_BAR_WIDTH: f32 = 14.;
const CHURN_COLUMN_WIDTH: f32 = 40.;

/// The window's part of the Health tab: the filter box, the hotspot list's scroll and its focus.
/// Its state is `model.health`.
pub(crate) struct HealthView {
    pub(crate) query: Entity<TextArea>,
    pub(crate) list: UniformListScrollHandle,
    /// The hotspot list takes the arrow and page keys while it has focus.
    pub(crate) focus: FocusHandle,
}

impl HealthView {
    pub(crate) fn new(query: Entity<TextArea>, focus: FocusHandle) -> Self {
        Self {
            query,
            list: UniformListScrollHandle::new(),
            focus,
        }
    }
}

/// The subscription that keeps the model's filter in step with the box. Made with the window.
pub(crate) fn subscriptions(
    query: &Entity<TextArea>,
    cx: &mut Context<StatusView>,
) -> Vec<Subscription> {
    vec![cx.observe(query, |this, _, cx| this.on_health_edited(cx))]
}

impl StatusView {
    /// Opens the Health tab and asks for the code health of HEAD.
    pub(crate) fn show_code_health(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::Health;
        self.request_health();
        cx.notify();
    }

    /// The filter box changed: the model takes the new folder and the list starts at its top.
    fn on_health_edited(&mut self, cx: &mut Context<Self>) {
        let text = self.health_view.query.read(cx).text().to_owned();
        if text.contains('\n') {
            // The box is one line; Enter is not a line break.
            let first = text.lines().next().unwrap_or("").to_owned();
            self.health_view
                .query
                .update(cx, |area, cx| area.set_text(&first, cx));
            return;
        }
        if self.model.health.set_filter(&text) {
            self.health_view.list.scroll_to_item(0, ScrollStrategy::Top);
            cx.notify();
        }
    }

    /// Shows the details of the file at `path`.
    fn select_health_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.model.health.select(path);
        cx.notify();
    }

    /// A key in the hotspot list: the arrows, Home, End and the page keys move the selection, and
    /// Enter opens the selected file.
    fn on_health_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if key == "enter" {
            if let Some(path) = self
                .model
                .health
                .selected_file()
                .map(|file| file.path.clone())
            {
                self.view_file(&path, None, cx);
            }
            return;
        }
        let Some(step) = health_page::step_of_key(key) else {
            return;
        };
        let visible = self
            .health_view
            .list
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .height;
        let page = health_page::page_rows(f32::from(visible), ROW_HEIGHT);
        let health = &self.model.health;
        let Some(row) =
            health_page::move_selection(health.selected_row(), health.rows().len(), step, page)
        else {
            return;
        };
        let Some(path) = health.path_at_row(row).map(str::to_owned) else {
            return;
        };
        self.model.health.select(&path);
        self.health_view
            .list
            .scroll_to_item(row, ScrollStrategy::Top);
        cx.notify();
    }

    /// The page for the main area: the tabs and the filter, the status, the dormancy breaks,
    /// then the hotspots beside the details of the selected file.
    pub(crate) fn health_page(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(theme::editor())
            .child(self.health_header(cx))
            .children(self.health_status())
            .children(self.dormancy_section(cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_row()
                    .child(self.hotspots(cx))
                    .children(self.health_details(cx)),
            )
    }

    /// The tab strip and the filter box.
    fn health_header(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .px_4()
            .py_3()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(theme::hairline())
            .bg(theme::panel())
            .child(self.code_tabs(cx))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_1()
                    .border_color(theme::hairline_strong())
                    .rounded_sm()
                    .child(icon::view(
                        Icon::MagnifyingGlass,
                        icon::SIZE,
                        theme::muted(),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(self.health_view.query.clone()),
                    ),
            )
    }

    /// The line about the read, or that there is no code health yet. Nothing while a read runs
    /// without progress to show.
    fn health_status(&self) -> Option<Div> {
        let health = &self.model.health;
        let text = health.status_text().or_else(|| {
            (!health.has_report() && !health.pending()).then(|| "No code health yet.".to_owned())
        })?;
        Some(
            div()
                .flex_none()
                .px_4()
                .py_2()
                .text_xs()
                .text_color(theme::muted())
                .child(text),
        )
    }

    /// The dormancy breaks: the first few files, in rank order. Nothing when there are none.
    fn dormancy_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        let health = &self.model.health;
        let dormant = health.dormant();
        if dormant.is_empty() {
            return None;
        }
        let rows = dormant
            .iter()
            .take(DORMANT_SHOWN)
            .enumerate()
            .filter_map(|(slot, &index)| Some(dormant_row(slot, health.file(index)?, cx)));
        let more = dormant.len().saturating_sub(DORMANT_SHOWN);
        Some(
            div()
                .flex_none()
                .px_4()
                .py_3()
                .flex()
                .flex_col()
                .gap_1()
                .border_b_1()
                .border_color(theme::hairline())
                .child(section_title("Dormancy breaks"))
                .children(rows)
                .children((more > 0).then(|| {
                    div()
                        .text_xs()
                        .text_color(theme::muted())
                        .child(format!("and {more} more"))
                })),
        )
    }

    /// The hotspots: a heading, then the list of the rows under the filter.
    fn hotspots(&self, cx: &mut Context<Self>) -> Div {
        let health = &self.model.health;
        let count = health.rows().len();
        let body = if count == 0 {
            div()
                .flex_1()
                .px_4()
                .py_2()
                .text_xs()
                .text_color(theme::muted())
                .child(empty_text(health))
                .into_any_element()
        } else {
            hotspot_list(
                count,
                self.health_view.list.clone(),
                &self.health_view.focus,
                cx,
            )
            .into_any_element()
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .pt_3()
                    .pb_1()
                    .child(section_title("Hotspots")),
            )
            .child(body)
            .child(churn_section(health))
    }

    /// The details of the selected file, or nothing when no file is selected.
    fn health_details(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let health = &self.model.health;
        let file = health.selected_file()?;
        let path = file.path.clone();
        Some(
            div()
                .id("health-details")
                .flex_none()
                .w(px(DETAILS_WIDTH))
                .h_full()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_4()
                .p_4()
                .border_l_1()
                .border_color(theme::hairline())
                .bg(theme::panel())
                .child(
                    div()
                        .font_family(theme::code_font())
                        .font_weight(FontWeight::MEDIUM)
                        .child(file.path.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::muted())
                        .child(health_page::rank_line(file, health.of())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(spark_chart(
                            &file.spark,
                            DETAIL_SPARK_HEIGHT,
                            DETAIL_SPARK_WIDTH,
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::muted())
                                .child("Last 180 active days of the repository"),
                        ),
                )
                .children(file.dormancy_break.as_ref().map(|dormancy| {
                    div()
                        .text_sm()
                        .text_color(theme::warn_fg())
                        .child(health_page::dormancy_text(dormancy))
                }))
                .child(partners_section(file, cx))
                .child(ownership_section(file))
                .children(complexity_block(file))
                .children(churn_block(file))
                .child(constraints_section(file))
                .child(
                    div()
                        .id("health-open")
                        .flex_none()
                        .h(px(26.))
                        .px_3()
                        .flex()
                        .items_center()
                        .rounded_md()
                        .border_1()
                        .border_color(theme::button_border())
                        .text_xs()
                        .cursor_pointer()
                        .child("Open File")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.view_file(&path, None, cx);
                            }),
                        ),
                ),
        )
    }
}

/// A small heading of a section.
fn section_title(text: &'static str) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::muted())
        .child(text)
}

/// A section of the details: its heading, then what it holds.
fn detail_section(title: &'static str) -> Div {
    div().flex().flex_col().gap_1().child(section_title(title))
}

/// The line that says the empty list is empty, and why.
fn empty_text(health: &HealthState) -> String {
    if !health.has_report() {
        String::new()
    } else if health.of() == 0 {
        "No file changed in the analysis window.".to_owned()
    } else {
        format!("No file under {}.", health.filter().trim())
    }
}

/// The list of the hotspots: one row per file under the filter, drawn as they scroll into view.
/// It takes focus on a click and then the keys of `on_health_key`.
fn hotspot_list(
    count: usize,
    list: UniformListScrollHandle,
    focus: &FocusHandle,
    cx: &mut Context<StatusView>,
) -> Div {
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .track_focus(focus)
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
            this.on_health_key(event, cx);
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, _| {
                this.health_view.focus.focus(window);
            }),
        )
        .child(
            uniform_list(
                "health-rows",
                count,
                cx.processor(
                    |this: &mut StatusView,
                     range: Range<usize>,
                     _window: &mut Window,
                     cx: &mut Context<StatusView>| {
                        let health = &this.model.health;
                        let max_heat = health.max_heat();
                        let selected = health.selected();
                        health
                            .rows()
                            .get(range.clone())
                            .unwrap_or_default()
                            .iter()
                            .zip(range)
                            .filter_map(|(&index, row)| {
                                let file = health.file(index)?;
                                let chosen = selected == Some(file.path.as_str());
                                Some(hotspot_row(file, row, health, max_heat, chosen, cx))
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

/// One hotspot: the rank, the path, the heat bar, the activity chart and the trend. A click
/// shows the file's details.
fn hotspot_row(
    file: &FileHealth,
    row: usize,
    health: &HealthState,
    max_heat: f64,
    chosen: bool,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let path = file.path.clone();
    let band = health.file_band(&file.path).map(|(band, _)| band);
    let bar = band.map_or_else(theme::muted, theme::heat);
    let bar_fill = Pixels::from(f64::from(BAR_TRACK) * heat_share(file.heat, max_heat));
    let trend_color = if file.trend == Trend::Rising {
        theme::heat_top1()
    } else {
        theme::muted()
    };
    div()
        .id(("health-row", row))
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_3()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .when(chosen, |r| r.bg(theme::selected_row()))
        .hover(|style| style.bg(theme::selected_row()))
        .child(
            div()
                .flex_none()
                .w(px(40.))
                .flex()
                .justify_end()
                .font_family(theme::code_font())
                .text_xs()
                .text_color(theme::muted())
                .child(file.rank.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(file.path.clone()),
        )
        .child(heat_bar(bar_fill, bar))
        .child(spark_chart(&file.spark, ROW_SPARK_HEIGHT, ROW_SPARK_WIDTH))
        .child(
            div()
                .flex_none()
                .w(px(56.))
                .text_xs()
                .text_color(trend_color)
                .child(health_page::trend_word(file.trend)),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.select_health_file(&path, cx);
            }),
        )
}

/// A heat bar: a fixed track, filled to `fill` in `color`.
fn heat_bar(fill: Pixels, color: Hsla) -> Div {
    div()
        .flex_none()
        .w(px(BAR_TRACK))
        .h(px(6.))
        .rounded_sm()
        .bg(theme::hairline())
        .child(div().h_full().w(fill).rounded_sm().bg(color))
}

/// An activity chart: one bar per period, each as tall as its share of the busiest one.
fn spark_chart(spark: &[u32; SPARK_BARS], height: u16, width: f32) -> Div {
    let heights = spark_heights(spark, u32::from(height));
    div()
        .flex_none()
        .w(px(width))
        .h(px(f32::from(height)))
        .flex()
        .flex_row()
        .items_end()
        .gap(px(1.5))
        .children(heights.into_iter().map(|bar| {
            div()
                .flex_1()
                .h(Pixels::from(f64::from(bar)))
                .bg(theme::muted())
        }))
}

/// One dormancy break of the list: the path and what it says. A click shows the file.
fn dormant_row(slot: usize, file: &FileHealth, cx: &Context<StatusView>) -> Stateful<Div> {
    let path = file.path.clone();
    let sentence = file
        .dormancy_break
        .as_ref()
        .map(health_page::dormancy_text)
        .unwrap_or_default();
    div()
        .id(("health-dormant", slot))
        .flex()
        .flex_col()
        .cursor_pointer()
        .hover(|style| style.bg(theme::selected_row()))
        .child(
            div()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(theme::code_font())
                .child(file.path.clone()),
        )
        .child(div().text_xs().text_color(theme::muted()).child(sentence))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.select_health_file(&path, cx);
            }),
        )
}

/// The files the selected file changes with, strongest first, or what says there are none.
fn partners_section(file: &FileHealth, cx: &Context<StatusView>) -> Div {
    let section = detail_section("Changes together with");
    if file.changes_together.is_empty() {
        return section.child(note("No file changes with it often enough."));
    }
    section.children(
        file.changes_together
            .iter()
            .enumerate()
            .map(|(slot, partner)| partner_row(slot, partner, cx)),
    )
}

/// One partner: its path and how strongly it changes with the file. A click shows the partner.
fn partner_row(slot: usize, partner: &Partner, cx: &Context<StatusView>) -> Stateful<Div> {
    let path = partner.path.clone();
    div()
        .id(("health-partner", slot))
        .flex()
        .flex_col()
        .cursor_pointer()
        .hover(|style| style.bg(theme::selected_row()))
        .child(
            div()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(theme::code_font())
                .child(partner.path.clone()),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::muted())
                .child(health_page::partner_text(partner)),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.select_health_file(&path, cx);
            }),
        )
}

/// The ownership of the file: counts and shares, never a name.
fn ownership_section(file: &FileHealth) -> Div {
    health_page::ownership_lines(&file.ownership)
        .into_iter()
        .fold(detail_section("Ownership"), |section, line| {
            section.child(note(&line))
        })
}

/// The complexity of the selected file: its deepest and mean nesting, and where it stands among
/// the measured files. Nothing when the file has none (D229).
fn complexity_block(file: &FileHealth) -> Option<Div> {
    let [depth, standing] = health_page::complexity_lines(file.complexity.as_ref()?);
    Some(
        detail_section("Complexity")
            .child(note(&depth))
            .child(note(&standing)),
    )
}

/// The churn of the selected file: its line of churn and the share alive after 30 active days.
/// Nothing when the file has no churn.
fn churn_block(file: &FileHealth) -> Option<Div> {
    let churn = ChurnSummary::of(file.churn.as_ref())?;
    Some(
        detail_section("Churn")
            .child(note(&health_page::churn_line(&churn)))
            .child(note(&health_page::alive_line(&churn))),
    )
}

/// The churn of the folder the list is limited to, under the hotspots: its line and its survival
/// chart, or why there is none yet. A fixed height.
fn churn_section(health: &HealthState) -> Div {
    let folder = health_page::scope_of(health.filter());
    let folder = if folder.is_empty() {
        "whole repository"
    } else {
        folder
    };
    let body = match health.churn() {
        Some(churn) => churn_body(&churn),
        None if health.reading() => note("Churn appears once the history of the window is read."),
        None => note("No lines to follow here."),
    };
    div()
        .flex_none()
        .h(px(CHURN_SECTION_HEIGHT))
        .px_4()
        .py_2()
        .flex()
        .flex_col()
        .gap_1()
        .overflow_hidden()
        .border_t_1()
        .border_color(theme::hairline())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(section_title("Churn"))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::muted())
                        .child(format!("· {folder}")),
                ),
        )
        .child(body)
}

/// The line of a folder's churn beside its survival chart.
fn churn_body(churn: &ChurnSummary) -> Div {
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .flex_row()
        .gap_4()
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_xs()
                .text_color(theme::muted())
                .child(health_page::churn_line(churn)),
        )
        .child(survival_chart(churn))
}

/// The share of lines alive at each age: one column per age, a bar on a muted track, the
/// percentage under it and the age at the foot. The bars start from `survival_floor`, so the
/// differences between the shares show (`health_page::survival_fills`).
fn survival_chart(churn: &ChurnSummary) -> Div {
    let points = health_page::survival_points(churn);
    let fills = health_page::survival_fills(&churn.survival);
    div().flex_none().flex().flex_row().gap_2().children(
        fills
            .into_iter()
            .zip(points.iter())
            .map(|(fill, (age, label))| survival_column(*age, fill, label.as_str())),
    )
}

/// One column of the survival chart: a bar filled to `fill` of its track.
fn survival_column(age: u32, fill: f64, label: &str) -> Div {
    let height = Pixels::from(fill.clamp(0.0, 1.0) * f64::from(CHURN_BAR_MAX));
    div()
        .w(px(CHURN_COLUMN_WIDTH))
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .w(px(CHURN_BAR_WIDTH))
                .h(px(CHURN_BAR_MAX))
                .flex()
                .flex_col()
                .justify_end()
                .rounded_sm()
                .bg(theme::hairline())
                .child(div().w_full().h(height).rounded_sm().bg(theme::muted())),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::muted())
                .child(label.to_owned()),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::muted())
                .child(format!("{age}d")),
        )
}

/// The constraints an agent is told before it changes the file.
fn constraints_section(file: &FileHealth) -> Div {
    if file.constraints.is_empty() {
        return detail_section("What an agent is told").child(note("No constraints."));
    }
    file.constraints
        .iter()
        .fold(detail_section("What an agent is told"), |section, line| {
            section.child(note(line))
        })
}

/// A line of text in the details, wrapped to the panel.
fn note(text: &str) -> Div {
    div()
        .text_xs()
        .text_color(theme::muted())
        .child(text.to_owned())
}
