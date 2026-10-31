//! The text search page (design/code-navigation.md, N3): a box for the text, three switches
//! (case, whole word, regular expression), a box for the files to look in, and
//! the matching lines grouped by file. The state comes from `text_search.rs`;
//! `git grep` runs in the engine. Typing starts a search after a short pause,
//! Enter starts it at once, and a click on a line opens the file at that line on
//! the Code page.

use std::ops::Range;
use std::time::Duration;

use gm_schema::{Actor, CommandKind, GrepFile, GrepLine, GrepScope};
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, Stateful,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use crate::icon::{self, Icon};
use crate::name_fit::name_text;
use crate::quick_open::split_name;
use crate::text_area::{Newline, TextArea};
use crate::text_search::{DEBOUNCE_MS, Row, Toggle, file_count};
use crate::theme;
use crate::view::{Screen, StatusView};

const ROW_HEIGHT: f32 = 24.;
const GUTTER: f32 = 56.;

/// The window's part of the page: the two boxes and the list's scroll.
pub(crate) struct SearchView {
    pub(crate) query: Entity<TextArea>,
    pub(crate) paths: Entity<TextArea>,
    pub(crate) scroll: UniformListScrollHandle,
}

impl SearchView {
    pub(crate) fn new(query: Entity<TextArea>, paths: Entity<TextArea>) -> Self {
        Self {
            query,
            paths,
            scroll: UniformListScrollHandle::new(),
        }
    }
}

/// The subscriptions that keep the boxes and the page in step: a change in
/// either starts the delayed search. Made with the window, before the first event.
pub(crate) fn subscriptions(
    query: &Entity<TextArea>,
    paths: &Entity<TextArea>,
    cx: &mut Context<StatusView>,
) -> Vec<Subscription> {
    vec![
        cx.observe(query, |this, _, cx| this.on_search_edited(cx)),
        cx.observe(paths, |this, _, cx| this.on_search_edited(cx)),
    ]
}

impl StatusView {
    /// Opens the page with the cursor in the text box.
    pub(crate) fn show_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Search;
        self.search_view
            .query
            .update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    /// A box changed. Reads both boxes and, when the search changed, starts it
    /// after a pause in typing. The caret blinks by notifying the box, so a
    /// notification that changes nothing starts nothing.
    fn on_search_edited(&mut self, cx: &mut Context<Self>) {
        if self.screen != Screen::Search {
            return;
        }
        let query = self.search_view.query.read(cx).text().to_owned();
        let paths = self.search_view.paths.read(cx).text().to_owned();
        if query.contains('\n') || paths.contains('\n') {
            // The boxes are one line; Enter is a command, not a line break.
            for (area, text) in [
                (&self.search_view.query, query),
                (&self.search_view.paths, paths),
            ] {
                let first = text.lines().next().unwrap_or("").to_owned();
                area.update(cx, |area, cx| area.set_text(&first, cx));
            }
            return;
        }
        let changed_query = self.model.text_search.set_query(&query);
        let changed_paths = self.model.text_search.set_paths(&paths);
        if !changed_query && !changed_paths {
            return;
        }
        let generation = self.model.text_search.edited();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DEBOUNCE_MS))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.model.text_search.is_latest(generation) {
                    view.run_text_search(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Sends the search now. An empty box clears the page instead.
    pub(crate) fn run_text_search(&mut self, cx: &mut Context<Self>) {
        if self.model.text_search.is_blank() {
            self.model.text_search.clear();
            cx.notify();
            return;
        }
        let search = &self.model.text_search;
        let kind = CommandKind::SearchText {
            query: search.query().to_owned(),
            options: search.options,
            paths: search.paths().to_owned(),
        };
        let id = self.engine.dispatch(Actor::Human, kind);
        self.model.text_search.begin(id);
        self.search_view
            .scroll
            .scroll_to_item(0, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    /// A switch was clicked: flip it and search again at once.
    fn toggle_search_option(&mut self, option: Toggle, cx: &mut Context<Self>) {
        self.model.text_search.toggle(option);
        self.model.text_search.edited();
        self.run_text_search(cx);
    }

    /// The page for the main area.
    pub(crate) fn search_page(&self, cx: &mut Context<Self>) -> Div {
        let search = &self.model.text_search;
        let rows = search.rows();
        let failed = search.result().is_none() && !search.is_searching() && !search.is_blank();
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(theme::editor())
            .child(self.search_header(cx))
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_1()
                    .text_xs()
                    .text_color(if failed {
                        theme::error()
                    } else {
                        theme::muted()
                    })
                    .child(search.summary()),
            )
            .child(if rows.is_empty() {
                div().flex_1().into_any_element()
            } else {
                uniform_list(
                    "search-rows",
                    rows.len(),
                    cx.processor(
                        |this: &mut StatusView,
                         range: Range<usize>,
                         _window: &mut Window,
                         cx: &mut Context<StatusView>| {
                            let search = &this.model.text_search;
                            let rows = search.rows();
                            let Some(result) = search.result() else {
                                return Vec::new();
                            };
                            range
                                .filter_map(|index| {
                                    let row = *rows.get(index)?;
                                    Some(match row {
                                        Row::File { file } => {
                                            let entry = result.files.get(file)?;
                                            file_row(
                                                entry,
                                                search.is_folded(&entry.path),
                                                index,
                                                cx,
                                            )
                                        }
                                        Row::Line { file, line } => {
                                            let entry = result.files.get(file)?;
                                            line_row(&entry.path, entry.lines.get(line)?, index, cx)
                                        }
                                    })
                                })
                                .collect()
                        },
                    ),
                )
                .track_scroll(self.search_view.scroll.clone())
                .flex_1()
                .min_h(px(0.))
                .into_any_element()
            })
    }

    /// The two boxes and the switches. Enter in either box searches at once.
    fn search_header(&self, cx: &mut Context<Self>) -> Div {
        let options = self.model.text_search.options;
        let enter = cx.entity().downgrade();
        div()
            .flex_none()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_2()
            .border_b_1()
            .border_color(theme::hairline())
            .bg(theme::panel())
            .capture_action(move |_: &Newline, _, cx| {
                let _ = enter.update(cx, StatusView::run_text_search);
                cx.stop_propagation();
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
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
                                    .child(self.search_view.query.clone()),
                            ),
                    )
                    .child(switch(
                        "search-case",
                        "Aa",
                        "Match case",
                        options.case_sensitive,
                        Toggle::Case,
                        cx,
                    ))
                    .child(switch(
                        "search-word",
                        "ab",
                        "Whole word",
                        options.whole_word,
                        Toggle::Word,
                        cx,
                    ))
                    .child(switch(
                        "search-regex",
                        ".*",
                        "Regular expression",
                        options.regex,
                        Toggle::Regex,
                        cx,
                    ))
                    .child(switch(
                        "search-new",
                        "new",
                        "Also search new files Git does not track yet. Turn it off in a very large repository: finding them costs about as much as git status.",
                        options.scope == GrepScope::TrackedAndNew,
                        Toggle::Untracked,
                        cx,
                    )),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .border_1()
                    .border_color(theme::hairline())
                    .rounded_sm()
                    .child(self.search_view.paths.clone()),
            )
    }
}

/// A switch of the search: lit when on. A click flips it.
fn switch(
    id: &'static str,
    label: &'static str,
    hint: &'static str,
    on: bool,
    option: Toggle,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .min_w(px(32.))
        .px_2()
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .cursor_pointer()
        .font_family(theme::code_font())
        .when(on, |s| {
            s.bg(theme::selection())
                .border_color(theme::accent())
                .text_color(theme::accent())
        })
        .when(!on, |s| {
            s.border_color(theme::hairline())
                .text_color(theme::muted())
                .hover(|style| style.bg(theme::selected_row()))
        })
        .tooltip(crate::window_chrome::hint(hint.to_owned()))
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.toggle_search_option(option, cx);
            }),
        )
}

/// The header of a file: a fold marker, its name, its folder fainter and the
/// count of its matches. A click folds or unfolds it.
fn file_row(
    entry: &GrepFile,
    is_folded: bool,
    index: usize,
    cx: &Context<StatusView>,
) -> AnyElement {
    let (folder, name) = split_name(&entry.path);
    let path = entry.path.clone();
    div()
        .id(("search-file", index))
        .h(px(ROW_HEIGHT))
        .px_4()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .bg(theme::ground())
        .hover(|style| style.bg(theme::selected_row()))
        .child(
            div()
                .flex_none()
                .w(px(12.))
                .text_xs()
                .text_color(theme::muted())
                .child(icon::view(
                    if is_folded {
                        Icon::CaretRight
                    } else {
                        Icon::CaretDown
                    },
                    icon::SMALL,
                    theme::muted(),
                )),
        )
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .font_weight(FontWeight::MEDIUM)
                .child(name.to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_xs()
                .text_color(theme::muted())
                .child(folder.trim_end_matches('/').to_owned()),
        )
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(theme::muted())
                .child(file_count(entry.lines.len(), entry.more)),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.model.text_search.toggle_file(&path);
                cx.notify();
            }),
        )
        .into_any_element()
}

/// A matching line: its number, then its text with the matches in the accent
/// colour. A click opens the file at that line.
fn line_row(path: &str, line: &GrepLine, index: usize, cx: &Context<StatusView>) -> AnyElement {
    let open = path.to_owned();
    let number = line.line;
    let ranges: Vec<Range<usize>> = line.ranges.iter().map(|&(a, b)| a..b).collect();
    div()
        .id(("search-line", index))
        .h(px(ROW_HEIGHT))
        .flex()
        .flex_row()
        .items_center()
        .cursor_pointer()
        .hover(|style| style.bg(theme::selected_row()))
        .child(
            div()
                .flex_none()
                .w(px(GUTTER))
                .pr_3()
                .text_right()
                .text_xs()
                .text_color(theme::gutter())
                .child(number.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_row()
                .items_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .font_family(theme::code_font())
                .children(
                    line.cut_start
                        .then(|| div().text_color(theme::muted()).child("…")),
                )
                .child(name_text(&line.text, &ranges))
                .children(
                    line.cut_end
                        .then(|| div().text_color(theme::muted()).child("…")),
                ),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.view_file(&open, Some(number), cx);
            }),
        )
        .into_any_element()
}
