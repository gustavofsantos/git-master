//! The Inbox page (UI redesign): what needs the person, in sections. The sections and their
//! items are derived from state the window already holds; until that lands (slice 6) the page
//! shows its empty state, so the sidebar's pages are final.

use gpui::{Div, div, prelude::*, px};

use crate::theme;
use crate::view::StatusView;

impl StatusView {
    pub(crate) fn inbox_page() -> Div {
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
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
    }
}
