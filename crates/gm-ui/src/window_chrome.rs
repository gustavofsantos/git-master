//! The window frame the app draws itself when the compositor does not: the
//! window controls (minimize, maximize, close) at the right end of the app's
//! header row, the drag region of that row, and the invisible resize band
//! along the edges. Where the app draws the frame it also draws the window's
//! edge: a transparent margin, rounded corners, a 1px outline and a drop
//! shadow in the margin (see `framed`). Everything here is inert when
//! `Window::window_decorations` reports `Server`, which is the case whenever
//! the platform draws the frame, and always on macOS.
//!
//! Ported from `rvw` (`crates/rvw-gui/src/window_chrome.rs`), same author,
//! MIT licensed.

use gpui::prelude::*;
use gpui::{
    AnyView, App, Bounds, BoxShadow, Corners, CursorStyle, Decorations, DispatchPhase, Div, Edges,
    HitboxBehavior, Hsla, MouseButton, MouseDownEvent, Pixels, Point, ResizeEdge, SharedString,
    Size, Stateful, Tiling, Window, WindowControlArea, canvas, div, point, px,
};

use crate::icon::{self, Icon};
use crate::theme;

/// Height of the app's header row, the top bar or the welcome header.
pub const HEADER_HEIGHT: Pixels = px(36.);
/// Width of the transparent margin around the app's own frame, on each side
/// the compositor does not tile. The shadow is drawn in it, and the resize
/// band lies in it, so the band never covers the app's content.
pub const SHADOW_MARGIN: Pixels = px(12.);
/// Corner radius of the app's own frame, where its corners are not tiled.
pub const FRAME_RADIUS: Pixels = px(10.);
/// Width of the outline around the app's own frame.
const FRAME_BORDER: Pixels = px(1.);
/// Opacity of the drop shadow behind the app's own frame.
const SHADOW_ALPHA: f32 = 0.4;
/// Side of each control button.
const CONTROL_SIZE: Pixels = px(28.);

/// The small card a control's label shows in on hover.
struct HintCard {
    text: SharedString,
}

impl Render for HintCard {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded_sm()
            .border_1()
            .border_color(theme::hairline())
            .bg(theme::panel())
            .text_size(px(12.))
            .text_color(theme::text())
            .child(self.text.clone())
    }
}

/// One of the window controls in the title bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Control {
    fn icon(self) -> Icon {
        match self {
            Self::Minimize => Icon::Minimize,
            Self::Maximize => Icon::Maximize,
            Self::Restore => Icon::Restore,
            Self::Close => Icon::Close,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Minimize => "Minimize",
            Self::Maximize => "Maximize",
            Self::Restore => "Restore",
            Self::Close => "Close",
        }
    }
}

/// Which controls the platform supports. Close is always offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Supported {
    pub minimize: bool,
    pub maximize: bool,
}

/// The controls to draw, left to right. Maximize turns into Restore while the
/// window is maximized.
pub fn visible_controls(supported: Supported, maximized: bool) -> Vec<Control> {
    let mut controls = Vec::with_capacity(3);
    if supported.minimize {
        controls.push(Control::Minimize);
    }
    if supported.maximize {
        controls.push(if maximized {
            Control::Restore
        } else {
            Control::Maximize
        });
    }
    controls.push(Control::Close);
    controls
}

/// The edge or corner to resize from at `point`, or `None` in the middle of
/// the window and on a side that is tiled against the screen.
pub fn edge_at(
    point: Point<Pixels>,
    size: Size<Pixels>,
    band: Pixels,
    corner: Pixels,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let near_left = point.x < corner && !tiling.left;
    let near_right = point.x >= size.width - corner && !tiling.right;
    let near_top = point.y < corner && !tiling.top;
    let near_bottom = point.y >= size.height - corner && !tiling.bottom;
    match (near_left, near_right, near_top, near_bottom) {
        (true, _, true, _) => return Some(ResizeEdge::TopLeft),
        (_, true, true, _) => return Some(ResizeEdge::TopRight),
        (true, _, _, true) => return Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => return Some(ResizeEdge::BottomRight),
        _ => {}
    }
    if point.y < band && !tiling.top {
        Some(ResizeEdge::Top)
    } else if point.y >= size.height - band && !tiling.bottom {
        Some(ResizeEdge::Bottom)
    } else if point.x < band && !tiling.left {
        Some(ResizeEdge::Left)
    } else if point.x >= size.width - band && !tiling.right {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

/// The rectangles of the band, edges first and corners last so the corner
/// cursor wins where they overlap. Mirrors `edge_at`.
fn band_rects(
    size: Size<Pixels>,
    band: Pixels,
    corner: Pixels,
    tiling: Tiling,
) -> Vec<(ResizeEdge, Bounds<Pixels>)> {
    let rect = |x, y, w, h| Bounds::new(point(x, y), gpui::size(w, h));
    let (w, h) = (size.width, size.height);
    let mut rects = Vec::with_capacity(8);
    if !tiling.top {
        rects.push((ResizeEdge::Top, rect(px(0.), px(0.), w, band)));
    }
    if !tiling.bottom {
        rects.push((ResizeEdge::Bottom, rect(px(0.), h - band, w, band)));
    }
    if !tiling.left {
        rects.push((ResizeEdge::Left, rect(px(0.), px(0.), band, h)));
    }
    if !tiling.right {
        rects.push((ResizeEdge::Right, rect(w - band, px(0.), band, h)));
    }
    let corners = [
        (
            ResizeEdge::TopLeft,
            px(0.),
            px(0.),
            tiling.top || tiling.left,
        ),
        (
            ResizeEdge::TopRight,
            w - corner,
            px(0.),
            tiling.top || tiling.right,
        ),
        (
            ResizeEdge::BottomLeft,
            px(0.),
            h - corner,
            tiling.bottom || tiling.left,
        ),
        (
            ResizeEdge::BottomRight,
            w - corner,
            h - corner,
            tiling.bottom || tiling.right,
        ),
    ];
    for (edge, x, y, tiled) in corners {
        if !tiled {
            rects.push((edge, rect(x, y, corner, corner)));
        }
    }
    rects
}

fn cursor_for(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

/// True when the app must draw the frame: the compositor granted client
/// decorations, which GPUI reports only after the compositor has answered.
pub fn draws_frame(window: &Window) -> bool {
    matches!(window.window_decorations(), Decorations::Client { .. })
}

/// The tiling of the frame the app draws around its content, or `None` when
/// it draws none: server decorations (macOS, or a compositor that frames the
/// window), and a window that is maximized or full screen, which has no
/// margin, no rounding and no shadow.
pub fn drawn_frame(decorations: Decorations, maximized: bool, fullscreen: bool) -> Option<Tiling> {
    match decorations {
        Decorations::Client { tiling } if !maximized && !fullscreen => Some(tiling),
        _ => None,
    }
}

/// `drawn_frame` for a window.
pub fn frame_tiling(window: &Window) -> Option<Tiling> {
    drawn_frame(
        window.window_decorations(),
        window.is_maximized(),
        window.is_fullscreen(),
    )
}

/// The width of the margin on each side of a frame with `tiling`: `inset` on
/// each side the compositor does not tile, zero on a tiled side.
pub fn margins(tiling: Tiling, inset: Pixels) -> Edges<Pixels> {
    let side = |tiled: bool| if tiled { px(0.) } else { inset };
    Edges {
        top: side(tiling.top),
        right: side(tiling.right),
        bottom: side(tiling.bottom),
        left: side(tiling.left),
    }
}

/// The corner radii of the frame: `FRAME_RADIUS` at each corner whose two
/// sides are both untiled, square otherwise.
pub fn radii(tiling: Tiling) -> Corners<Pixels> {
    let corner = |a: bool, b: bool| if a || b { px(0.) } else { FRAME_RADIUS };
    Corners {
        top_left: corner(tiling.top, tiling.left),
        top_right: corner(tiling.top, tiling.right),
        bottom_right: corner(tiling.bottom, tiling.right),
        bottom_left: corner(tiling.bottom, tiling.left),
    }
}

/// The corner radii of the header and footer rows, which reach the frame's
/// corners: the frame's radii less its outline, so they sit inside it. All
/// zero when the app draws no frame.
pub fn inner_radii(window: &Window) -> Corners<Pixels> {
    let inner = |r: Pixels| {
        if r > FRAME_BORDER {
            r - FRAME_BORDER
        } else {
            px(0.)
        }
    };
    let outer = frame_tiling(window).map_or_else(Corners::default, radii);
    Corners {
        top_left: inner(outer.top_left),
        top_right: inner(outer.top_right),
        bottom_right: inner(outer.bottom_right),
        bottom_left: inner(outer.bottom_left),
    }
}

/// Tells the platform how wide the frame's margin is, so the compositor does
/// not count it as window content: the margin when the app draws a frame, and
/// zero when it is maximized or full screen. Only with client decorations;
/// server decorations and macOS are left alone. Called on every render, as
/// GPUI only sends a change to the platform.
pub fn sync_client_inset(window: &mut Window) {
    if !matches!(window.window_decorations(), Decorations::Client { .. }) {
        return;
    }
    let inset = if frame_tiling(window).is_some() {
        SHADOW_MARGIN
    } else {
        px(0.)
    };
    window.set_client_inset(inset);
}

/// The space gpui-component's `Root` puts around the window's content, per side as `[top, right,
/// bottom, left]`: its padding, and the 1px border after it. Zero where the window is tiled, and
/// on platforms that draw the frame, since `Root` adds neither there.
#[must_use]
pub fn root_overhang(window: &Window) -> [Pixels; 4] {
    let pads = gpui_component::window_paddings(window);
    let side = |pad: Pixels| if pad > px(0.) { pad + px(1.) } else { px(0.) };
    [
        side(pads.top),
        side(pads.right),
        side(pads.bottom),
        side(pads.left),
    ]
}

/// The window's root, wrapped in the frame the app draws when it draws one:
/// a transparent margin around `app`, which gets rounded corners, a 1px
/// outline and a drop shadow that falls into the margin. The resize band goes
/// last, over the margin. Without a drawn frame, `app` is returned with the
/// resize band (none, under server decorations) as its last child, exactly as
/// before the frame existed.
pub fn framed(window: &Window, app: Div) -> Div {
    let Some(tiling) = frame_tiling(window) else {
        return app.children(resize_layer(window));
    };
    let edge = margins(tiling, SHADOW_MARGIN);
    let corners = radii(tiling);
    let overhang = root_overhang(window);
    let outer = if overhang == [px(0.); 4] {
        div().size_full().relative()
    } else {
        // Root's padding and border sit around this view; the view is moved over them, so the
        // frame is drawn at the window's edge as before.
        div()
            .absolute()
            .top(-overhang[0])
            .right(-overhang[1])
            .bottom(-overhang[2])
            .left(-overhang[3])
    };
    outer
        .pt(edge.top)
        .pr(edge.right)
        .pb(edge.bottom)
        .pl(edge.left)
        .child(
            app.rounded_tl(corners.top_left)
                .rounded_tr(corners.top_right)
                .rounded_br(corners.bottom_right)
                .rounded_bl(corners.bottom_left)
                .border_1()
                .border_color(theme::window_border())
                .shadow(vec![BoxShadow {
                    color: theme::window_shadow().opacity(SHADOW_ALPHA),
                    offset: point(px(0.), px(2.)),
                    blur_radius: px(10.),
                    spread_radius: px(0.),
                }]),
        )
        .children(resize_layer(window))
}

/// The invisible resize band, to be the last child of the root so it sits
/// above everything. `None` when the platform draws the frame, or when the
/// window is maximized or full screen. The band's hitboxes are `Normal`, so
/// they do not take hover away from the content under them.
pub fn resize_layer(window: &Window) -> Option<impl IntoElement + use<>> {
    let tiling = frame_tiling(window)?;
    // The band is the margin: a side with no margin (tiled) has no band.
    let inset = SHADOW_MARGIN;
    Some(
        canvas(
            move |bounds, window, _cx| {
                band_rects(bounds.size, inset, inset, tiling)
                    .into_iter()
                    .map(|(edge, rect)| {
                        let at = Bounds::new(bounds.origin + rect.origin, rect.size);
                        (edge, window.insert_hitbox(at, HitboxBehavior::Normal))
                    })
                    .collect::<Vec<_>>()
            },
            move |bounds, hitboxes, window, _cx| {
                for (edge, hitbox) in &hitboxes {
                    window.set_cursor_style(cursor_for(*edge), hitbox);
                }
                let window_size = bounds.size;
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    let found = edge_at(event.position, window_size, inset, inset, tiling);
                    if let Some(edge) = found {
                        window.start_window_resize(edge);
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .size_full(),
    )
}

/// Makes `bar` the window's drag handle: a press moves the window, a double
/// click toggles maximize, a right click opens the window menu. Children that
/// must keep their clicks wrap themselves in `no_drag`.
pub fn drag_region(bar: Div, window: &Window) -> Div {
    let controls = window.window_controls();
    bar.window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            if event.click_count == 2 {
                if controls.maximize {
                    window.zoom_window();
                }
            } else {
                window.start_window_move();
            }
        })
        .on_mouse_down(MouseButton::Right, move |event, window, _| {
            if controls.window_menu {
                window.show_window_menu(event.position);
            }
        })
}

/// The header row every screen starts with: the layout of the row, with the
/// drag region applied when the app draws the frame. The caller adds the
/// row's content and, when `draws_frame`, `window_controls` at its right end.
pub fn header_bar(window: &Window) -> Div {
    let corners = inner_radii(window);
    let bar = div()
        .h(HEADER_HEIGHT)
        .flex_none()
        .pl_4()
        .pr_1()
        .flex()
        .flex_row()
        .items_center()
        .gap_4()
        .rounded_tl(corners.top_left)
        .rounded_tr(corners.top_right)
        .bg(theme::panel())
        .border_b_1()
        .border_color(theme::hairline());
    if draws_frame(window) {
        drag_region(bar, window)
    } else {
        bar
    }
}

/// A tooltip that shows `text` on the hint card: a control that is disabled
/// and says why, or the full name of a truncated branch.
pub fn hint(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |_: &mut Window, cx: &mut App| -> AnyView {
        cx.new(|_| HintCard { text: text.clone() }).into()
    }
}

/// The window controls the platform supports, left to right, drawn for the
/// caller to place at the end of a header row.
pub fn window_controls(window: &Window) -> Div {
    let platform = window.window_controls();
    let supported = Supported {
        minimize: platform.minimize,
        maximize: platform.maximize,
    };
    visible_controls(supported, window.is_maximized())
        .into_iter()
        .fold(div().flex().items_center().gap(px(2.)), |row, control| {
            row.child(control_button(control))
        })
}

/// Wraps a header item that must keep its own clicks. A press on it never
/// reaches the drag region, so it neither moves the window nor opens the
/// window menu. The wrapper is the item's parent, so the item's own handlers
/// run first and this one only stops the press from going further up.
pub fn no_drag(item: impl IntoElement) -> Div {
    stop_presses(div().flex_none(), item)
}

/// `no_drag` for an item whose text must give way when the header is narrow:
/// the wrapper takes the free width and may shrink, so the item can shrink too.
pub fn no_drag_shrinking(item: impl IntoElement) -> Div {
    stop_presses(div().flex_shrink().min_w(px(0.)), item)
}

fn stop_presses(wrapper: Div, item: impl IntoElement) -> Div {
    wrapper
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(item)
}

fn control_button(control: Control) -> Stateful<Div> {
    let closing = control == Control::Close;
    let (ink, wash): (Hsla, Hsla) = if closing {
        (theme::removed_fg(), theme::removed_bg())
    } else {
        (theme::muted(), theme::selected_row())
    };
    let label = control.label();
    div()
        .id(label)
        .flex()
        .items_center()
        .justify_center()
        .size(CONTROL_SIZE)
        .rounded_sm()
        .cursor_pointer()
        .text_color(ink)
        .hover(move |style| style.bg(wash))
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |_, window, cx| match control {
            Control::Minimize => window.minimize_window(),
            Control::Maximize | Control::Restore => window.zoom_window(),
            // Same path as ⌘Q, so a running terminal can still ask first.
            Control::Close => window.dispatch_action(Box::new(crate::Quit), cx),
        })
        .tooltip(move |_: &mut Window, cx: &mut App| -> AnyView {
            cx.new(|_| HintCard { text: label.into() }).into()
        })
        .child(icon::view(control.icon(), icon::SIZE, ink))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    const BAND: Pixels = px(6.);
    const CORNER: Pixels = px(12.);

    fn edge(x: f32, y: f32, tiling: Tiling) -> Option<ResizeEdge> {
        edge_at(
            point(px(x), px(y)),
            size(px(1000.), px(800.)),
            BAND,
            CORNER,
            tiling,
        )
    }

    #[test]
    fn the_middle_of_the_window_is_not_an_edge() {
        assert_eq!(edge(500., 400., Tiling::default()), None);
        assert_eq!(edge(6., 400., Tiling::default()), None);
        assert_eq!(
            edge(500., 794.5, Tiling::default()),
            Some(ResizeEdge::Bottom)
        );
        assert_eq!(edge(500., 793.9, Tiling::default()), None);
    }

    #[test]
    fn each_side_and_corner_is_found() {
        let free = Tiling::default();
        assert_eq!(edge(500., 2., free), Some(ResizeEdge::Top));
        assert_eq!(edge(500., 799., free), Some(ResizeEdge::Bottom));
        assert_eq!(edge(2., 400., free), Some(ResizeEdge::Left));
        assert_eq!(edge(999., 400., free), Some(ResizeEdge::Right));
        assert_eq!(edge(2., 2., free), Some(ResizeEdge::TopLeft));
        assert_eq!(edge(999., 2., free), Some(ResizeEdge::TopRight));
        assert_eq!(edge(2., 799., free), Some(ResizeEdge::BottomLeft));
        assert_eq!(edge(999., 799., free), Some(ResizeEdge::BottomRight));
        assert_eq!(edge(10., 10., free), Some(ResizeEdge::TopLeft));
    }

    #[test]
    fn a_tiled_side_offers_no_edge() {
        let top_tiled = Tiling {
            top: true,
            ..Tiling::default()
        };
        assert_eq!(edge(500., 2., top_tiled), None);
        assert_eq!(edge(2., 2., top_tiled), Some(ResizeEdge::Left));
        assert_eq!(edge(500., 799., top_tiled), Some(ResizeEdge::Bottom));
        assert_eq!(edge(2., 2., Tiling::tiled()), None);
    }

    #[test]
    fn the_band_rectangles_agree_with_edge_at() {
        let tiling = Tiling {
            left: true,
            ..Tiling::default()
        };
        let window = size(px(1000.), px(800.));
        let rects = band_rects(window, BAND, CORNER, tiling);
        assert!(rects.iter().all(|(e, _)| *e != ResizeEdge::Left));
        for (found, rect) in rects.iter().rev() {
            let centre = rect.center();
            let at = edge_at(centre, window, BAND, CORNER, tiling);
            assert_eq!(at, Some(*found));
        }
    }

    #[test]
    fn controls_follow_the_platform_and_the_maximized_state() {
        let all = Supported {
            minimize: true,
            maximize: true,
        };
        assert_eq!(
            visible_controls(all, false),
            [Control::Minimize, Control::Maximize, Control::Close]
        );
        assert_eq!(
            visible_controls(all, true),
            [Control::Minimize, Control::Restore, Control::Close]
        );
        let none = Supported {
            minimize: false,
            maximize: false,
        };
        assert_eq!(visible_controls(none, true), [Control::Close]);
    }

    fn margin_of(tiling: Tiling) -> [Pixels; 4] {
        let m = margins(tiling, SHADOW_MARGIN);
        [m.top, m.right, m.bottom, m.left]
    }

    #[test]
    fn a_frame_is_drawn_only_with_client_decorations_on_a_window_not_maximized_or_full_screen() {
        let client = Decorations::Client {
            tiling: Tiling::default(),
        };
        assert_eq!(drawn_frame(client, false, false), Some(Tiling::default()));
        assert_eq!(drawn_frame(client, true, false), None);
        assert_eq!(drawn_frame(client, false, true), None);
        assert_eq!(drawn_frame(Decorations::Server, false, false), None);
    }

    #[test]
    fn a_free_window_has_a_margin_on_every_side() {
        assert_eq!(
            margin_of(Tiling::default()),
            [SHADOW_MARGIN; 4],
            "every side of a free window carries the margin"
        );
    }

    #[test]
    fn a_tiled_side_has_no_margin() {
        let top = Tiling {
            top: true,
            ..Tiling::default()
        };
        assert_eq!(
            margin_of(top),
            [px(0.), SHADOW_MARGIN, SHADOW_MARGIN, SHADOW_MARGIN]
        );
        assert_eq!(margin_of(Tiling::tiled()), [px(0.); 4]);
    }

    #[test]
    fn a_corner_is_rounded_only_where_both_sides_are_free() {
        let free = radii(Tiling::default());
        assert_eq!(
            [
                free.top_left,
                free.top_right,
                free.bottom_right,
                free.bottom_left
            ],
            [FRAME_RADIUS; 4]
        );
        let left = radii(Tiling {
            left: true,
            ..Tiling::default()
        });
        assert_eq!(left.top_left, px(0.));
        assert_eq!(left.bottom_left, px(0.));
        assert_eq!(left.top_right, FRAME_RADIUS);
        assert_eq!(left.bottom_right, FRAME_RADIUS);
        let tiled = radii(Tiling::tiled());
        assert_eq!(
            [
                tiled.top_left,
                tiled.top_right,
                tiled.bottom_right,
                tiled.bottom_left
            ],
            [px(0.); 4]
        );
    }

    #[test]
    fn the_resize_band_is_the_margin_and_never_the_content() {
        let window = size(px(1000.), px(800.));
        let free = Tiling::default();
        let m = SHADOW_MARGIN;
        assert_eq!(
            edge_at(point(px(5.), px(400.)), window, m, m, free),
            Some(ResizeEdge::Left)
        );
        assert_eq!(
            edge_at(point(px(500.), px(3.)), window, m, m, free),
            Some(ResizeEdge::Top)
        );
        assert_eq!(
            edge_at(point(px(995.), px(790.)), window, m, m, free),
            Some(ResizeEdge::BottomRight)
        );
        // Just inside the margin is content, so no resize there.
        assert_eq!(edge_at(point(px(12.5), px(400.)), window, m, m, free), None);
        assert_eq!(edge_at(point(px(500.), px(12.5)), window, m, m, free), None);
        assert_eq!(
            edge_at(point(px(987.5), px(400.)), window, m, m, free),
            None
        );
        assert_eq!(
            edge_at(point(px(500.), px(787.5)), window, m, m, free),
            None
        );
    }
}
