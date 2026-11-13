//! The Health tab of the Code page (docs/guide/code-health.md, "The Health tab"): a filter box
//! above the hotspots, the dormancy breaks, the list of files by heat and the details of the
//! selected file. The state and every computation are in `health_page.rs`; this file only draws
//! it and sends the requests it plans. Rows are drawn by a `uniform_list`, so only the rows on
//! screen are built.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gm_schema::{ChurnSummary, FileHealth, Partner, SPARK_BARS, Trend};
use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, FocusHandle, FontWeight, Hsla, KeyDownEvent,
    MouseButton, MouseDownEvent, Pixels, ScrollStrategy, Stateful, Subscription,
    UniformListScrollHandle, Window, canvas, div, prelude::*, px, uniform_list,
};

use crate::health_page::{
    self, DORMANT_SHOWN, HealthState, HeatBand, HotspotView, heat_share, human_size, spark_heights,
};
use crate::icon::{self, Icon};
use crate::text_area::TextArea;
use crate::theme;
use crate::treemap::{self, MapNode, Rect};
use crate::view::{Screen, StatusView};
use crate::window_chrome;

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
/// The map: the header strip of a folder's name, the shortest side a box may have to be drawn,
/// the size of the map before its first paint, and the smallest box that shows a file's name.
const MAP_HEADER: f64 = 18.;
const MAP_MIN_SIDE: f64 = 4.;
const MAP_DEFAULT_SIZE: (f64, f64) = (720., 420.);
const MAP_NAME_MIN_WIDTH: f64 = 56.;
const MAP_NAME_MIN_HEIGHT: f64 = 16.;

/// The map's boxes, made for one size of the map and one revision of the health state.
struct MapLayout {
    size: (f64, f64),
    rev: u64,
    nodes: Vec<MapNode>,
}

/// The window's part of the Health tab: the filter box, the hotspot list's scroll and its focus,
/// and the map's size and layout. Its state is `model.health`.
pub(crate) struct HealthView {
    pub(crate) query: Entity<TextArea>,
    pub(crate) list: UniformListScrollHandle,
    /// The hotspot list takes the arrow and page keys while it has focus.
    pub(crate) focus: FocusHandle,
    /// The map's size in pixels, as its canvas last painted it; the default before that.
    map_size: Rc<Cell<(f64, f64)>>,
    /// The map's boxes. Laid out again only when the size or the health state changed.
    map_layout: RefCell<Option<MapLayout>>,
}

impl HealthView {
    pub(crate) fn new(query: Entity<TextArea>, focus: FocusHandle) -> Self {
        Self {
            query,
            list: UniformListScrollHandle::new(),
            focus,
            map_size: Rc::new(Cell::new(MAP_DEFAULT_SIZE)),
            map_layout: RefCell::new(None),
        }
    }

    /// Lays the map out again when its size or the health state changed since the last layout.
    fn ensure_layout(&self, health: &HealthState) {
        let size = self.map_size.get();
        let mut slot = self.map_layout.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|layout| same_size(layout.size, size) && layout.rev == health.rev())
        {
            return;
        }
        let files: Vec<(usize, &str, u64)> = health
            .map_files()
            .iter()
            .map(|(index, path, bytes)| (*index, path.as_str(), *bytes))
            .collect();
        let area = Rect {
            x: 0.,
            y: 0.,
            w: size.0,
            h: size.1,
        };
        let scope = health_page::scope_of(health.filter());
        let nodes = treemap::layout(&files, scope, area, MAP_HEADER, MAP_MIN_SIDE);
        *slot = Some(MapLayout {
            size,
            rev: health.rev(),
            nodes,
        });
    }
}

/// Whether two map sizes are the same. Compared by bits: the size only changes by a layout pass.
fn same_size(a: (f64, f64), b: (f64, f64)) -> bool {
    a.0.to_bits() == b.0.to_bits() && a.1.to_bits() == b.1.to_bits()
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

    /// Shows the hotspots as `view`: the list or the map.
    fn set_hotspot_view(&mut self, view: HotspotView, cx: &mut Context<Self>) {
        self.model.health.set_view(view);
        cx.notify();
    }

    /// Puts `folder` in the filter box, as typing it would, so the map and the list look inside it.
    fn open_health_folder(&mut self, folder: &str, cx: &mut Context<Self>) {
        let text = folder.to_owned();
        self.health_view
            .query
            .update(cx, |area, cx| area.set_text(&text, cx));
        if self.model.health.set_filter(&text) {
            self.health_view.list.scroll_to_item(0, ScrollStrategy::Top);
        }
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

    /// The hotspots: a heading with the choice of list or map, then the rows under the filter or
    /// the map of them.
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
        } else if health.view() == HotspotView::Map {
            self.hotspot_map(cx).into_any_element()
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
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(section_title("Hotspots"))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_4()
                            .child(self.view_button(
                                "List",
                                "health-view-list",
                                HotspotView::List,
                                cx,
                            ))
                            .child(self.view_button(
                                "Map",
                                "health-view-map",
                                HotspotView::Map,
                                cx,
                            )),
                    ),
            )
            .child(body)
            .child(churn_section(health))
    }

    /// One of the List and Map choices above the hotspots, underlined while it is shown, as the
    /// tabs of the Code page are.
    fn view_button(
        &self,
        label: &'static str,
        id: &'static str,
        view: HotspotView,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let active = self.model.health.view() == view;
        div()
            .id(id)
            .cursor_pointer()
            .h(px(24.))
            .flex()
            .flex_row()
            .items_center()
            .border_b_2()
            .border_color(if active {
                theme::accent()
            } else {
                theme::panel()
            })
            .text_xs()
            .font_weight(if active {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            })
            .text_color(if active {
                theme::text()
            } else {
                theme::muted()
            })
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.set_hotspot_view(view, cx);
                }),
            )
    }

    /// The map of the hotspots: a box per folder under the filter, with its files as boxes
    /// inside, sized by their bytes and colored by their heat. The boxes are laid out for the
    /// size the map was last painted at; a canvas measures it and asks for a redraw on a change.
    fn hotspot_map(&self, cx: &mut Context<Self>) -> Div {
        let health = &self.model.health;
        self.health_view.ensure_layout(health);
        let layout = self.health_view.map_layout.borrow();
        let nodes = layout
            .as_ref()
            .map_or(&[][..], |layout| layout.nodes.as_slice());
        let selected = health.selected();

        let size = Rc::clone(&self.health_view.map_size);
        let measure = canvas(
            move |bounds: Bounds<Pixels>, window: &mut Window, _: &mut App| {
                let now = (
                    f64::from(f32::from(bounds.size.width)),
                    f64::from(f32::from(bounds.size.height)),
                );
                if !same_size(size.get(), now) {
                    size.set(now);
                    // A refresh asked for while painting is lost; the next frame lays the map
                    // out again at the size just measured.
                    window.on_next_frame(|window, _| window.refresh());
                }
            },
            |_, (), _, _| {},
        )
        .absolute()
        .size_full();

        let mut boxes: Vec<AnyElement> = Vec::new();
        for node in nodes {
            boxes.push(map_folder(node, cx).into_any_element());
            for (index, rect) in &node.files {
                if let Some(file) = health.file(*index) {
                    let chosen = selected == Some(file.path.as_str());
                    boxes
                        .push(map_file(*index, file, *rect, health, chosen, cx).into_any_element());
                }
            }
        }
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .relative()
            .overflow_hidden()
            .child(measure)
            .children(boxes)
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

/// The box of a folder: a hairline border, its name in a muted strip across the top. Clicking the
/// name puts the folder in the filter box. The files of the scope are named "files here".
fn map_folder(node: &MapNode, cx: &Context<StatusView>) -> Div {
    let rect = node.rect;
    let name = if node.folder.is_empty() {
        "files here".to_owned()
    } else {
        node.folder
            .rsplit('/')
            .next()
            .unwrap_or(&node.folder)
            .to_owned()
    };
    let strip = div()
        .h(Pixels::from(MAP_HEADER))
        .px_1()
        .flex()
        .flex_row()
        .items_center()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_xs()
        .text_color(theme::muted())
        .child(name);
    let strip = if node.folder.is_empty() {
        strip
    } else {
        let folder = node.folder.clone();
        strip.cursor_pointer().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.open_health_folder(&folder, cx);
            }),
        )
    };
    div()
        .absolute()
        .left(Pixels::from(rect.x))
        .top(Pixels::from(rect.y))
        .w(Pixels::from(rect.w))
        .h(Pixels::from(rect.h))
        .border_1()
        .border_color(theme::hairline())
        .overflow_hidden()
        .child(strip)
}

/// The box of a file, at `rect` in the map: its heat as the fill (the hottest tenth) or a muted
/// fill, its name when the box is large enough, and the path, rank and size on hover. A click
/// selects the file; the selected box has a 2 px outline. Boxes are inset by half a pixel, so
/// neighbours are 1 px apart.
fn map_file(
    index: usize,
    file: &FileHealth,
    rect: Rect,
    health: &HealthState,
    chosen: bool,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let band = health.file_band(&file.path).map(|(band, _)| band);
    let color = band.map_or_else(theme::hairline, theme::heat);
    // White on the hottest band, the text color on the others: each keeps its contrast.
    let name_color = if band == Some(HeatBand::Top1) {
        theme::on_accent()
    } else {
        theme::text()
    };
    let (w, h) = ((rect.w - 1.).max(0.), (rect.h - 1.).max(0.));
    let name = file
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&file.path)
        .to_owned();
    let label = (w >= MAP_NAME_MIN_WIDTH && h >= MAP_NAME_MIN_HEIGHT).then(|| {
        div()
            .px_1()
            .text_xs()
            .text_color(name_color)
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(name)
    });
    let tip = format!(
        "{} · rank {} of {} · {}",
        file.path,
        file.rank,
        health.of(),
        human_size(file.bytes)
    );
    let path = file.path.clone();
    div()
        .id(("health-map-file", index))
        .absolute()
        .left(Pixels::from(rect.x + 0.5))
        .top(Pixels::from(rect.y + 0.5))
        .w(Pixels::from(w))
        .h(Pixels::from(h))
        .bg(color)
        .overflow_hidden()
        .cursor_pointer()
        .when(chosen, |boxed| boxed.border_2().border_color(theme::text()))
        .tooltip(window_chrome::hint(tip))
        .children(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.select_health_file(&path, cx);
            }),
        )
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
        .map_or_default(health_page::dormancy_text);
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
                .flex_none()
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
