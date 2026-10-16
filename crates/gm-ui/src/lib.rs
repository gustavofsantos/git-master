//! The `GitMaster` window. Slice 0: the status of one repository, read through
//! the engine. The engine runs on its own thread; this crate only renders what
//! the engine reports.

mod changes;
mod composer;
mod diff_pane;
mod editor;
mod model;
mod text_area;
mod text_area_element;
mod text_area_math;
mod theme;
mod view;

use gm_core::EngineHandle;
use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBounds, WindowDecorations, WindowOptions, actions, px, size,
};

use crate::view::{KEY_CONTEXT, StatusView};

actions!(gitmaster, [Quit, SelectNext, SelectPrev, Refresh]);

/// Opens the window on the repository at `path` and runs until it closes.
pub fn run(engine: EngineHandle, path: String) {
    Application::new().run(move |cx: &mut App| {
        theme::choose_fonts(&cx.text_system().all_font_names());
        bind_keys(cx);
        quit_with_the_window(cx);
        let bounds = Bounds::centered(None, size(px(1100.), px(700.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("GitMaster".into()),
                ..TitlebarOptions::default()
            }),
            // Where the compositor draws no frame, the app draws its own controls.
            window_decorations: Some(WindowDecorations::Client),
            // Matches the window class a desktop entry would use.
            app_id: Some("gitmaster".into()),
            ..WindowOptions::default()
        };
        let opened = cx.open_window(options, move |window, cx| {
            cx.new(|cx| StatusView::new(engine, path, window, cx))
        });
        if let Err(error) = opened {
            eprintln!("gitmaster: {error}");
            std::process::exit(1);
        }
        cx.activate(true);
    });
}

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("k", SelectPrev, Some(KEY_CONTEXT)),
        KeyBinding::new("up", SelectPrev, Some(KEY_CONTEXT)),
        KeyBinding::new("r", Refresh, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-q", Quit, None),
    ]);
}

/// The window is the app: closing it quits, and so does ⌘Q from the app menu.
fn quit_with_the_window(cx: &mut App) {
    cx.on_window_closed(|cx| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.set_menus(vec![Menu {
        name: "GitMaster".into(),
        items: vec![MenuItem::action("Quit GitMaster", Quit)],
    }]);
}
