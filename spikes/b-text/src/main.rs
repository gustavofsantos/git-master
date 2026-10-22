//! Spike B (design/roadmap.md): one multi-line text area in GPUI, ported from
//! rvw's `rvw-gui` text area. See FINDINGS.md next to Cargo.toml.

mod editor;
mod text_area;
mod text_area_element;
mod text_area_math;
mod theme;

use gpui::{
    App, Application, Bounds, Context, Entity, Render, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, size,
};

use text_area::TextArea;

const SAMPLE: &str = "Resolve conflict in parser: keep both branch handlers

The conflict was between the `feature/parser-v2` branch, which renamed the
token enum, and `main`, which added a new error variant. Both changes are
kept: the renamed variants come first, and the new error variant is mapped
to the same recovery path as before.

Notes for reviewers:
- No behaviour change for valid input.
- The fallback arm now returns `ParseError::Unexpected` instead of panicking.
- Added two regression cases for nested brackets.
- The lexer benchmark is unchanged within noise (see the attached run).

Refs: GM-42, GM-47";

struct Root {
    area: Entity<TextArea>,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(16.))
            .bg(theme::paper())
            .child(self.area.clone())
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        text_area::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(720.), px(520.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..WindowOptions::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            let area = cx.new(|cx| TextArea::new("Commit message", cx).with_lines(3, 12));
            area.update(cx, |area, cx| {
                area.set_text(SAMPLE, cx);
                area.focus(window, cx);
            });
            eprintln!("spike-b-text: window opened");
            cx.new(|_| Root { area })
        });
        if let Err(error) = opened {
            eprintln!("spike-b-text: {error}");
            std::process::exit(1);
        }
        cx.activate(true);
    });
}
