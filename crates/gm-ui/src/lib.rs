//! The `GitMaster` window. Slice 0: the status of one repository, read through
//! the engine. The engine runs on its own thread; this crate only renders what
//! the engine reports.

mod assets;
mod branch_menu;
mod branches;
mod changes;
mod command_panel;
mod composer;
mod diff_pane;
mod diff_rows;
mod editor;
mod icon;
mod model;
mod project_menu;
mod recent;
mod resize;
mod resize_handle;
mod shell_commit;
mod shell_list;
mod text_area;
mod text_area_element;
mod text_area_math;
mod theme;
mod view;
mod window_chrome;

use gm_core::EngineHandle;
use gm_core::command_log::CommandLog;
use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBounds, WindowDecorations, WindowOptions, actions, px, size,
};

use crate::theme::ThemeMode;
use crate::view::{KEY_CONTEXT, StatusView};

actions!(
    gitmaster,
    [
        Quit,
        SelectNext,
        SelectPrev,
        Refresh,
        ToggleStage,
        FocusFilter,
        FocusNext,
        FocusPrev,
    ]
);

/// Opens the window on the repository at `path` and runs until it closes.
pub fn run(engine: EngineHandle, command_log: CommandLog, path: String) {
    Application::new()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            theme::choose_fonts(&cx.text_system().all_font_names());
            // `GITMASTER_THEME=light|dark|system`; anything else means system.
            let mode = std::env::var("GITMASTER_THEME")
                .ok()
                .as_deref()
                .and_then(ThemeMode::from_name)
                .unwrap_or(ThemeMode::System);
            theme::set_mode(mode);
            bind_keys(cx);
            quit_with_the_window(cx);
            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("GitMaster".into()),
                    ..TitlebarOptions::default()
                }),
                // The compositor draws the frame when it can. Where it cannot, the
                // app draws its own title bar and controls (see `window_chrome`).
                // On Linux, ask for Client: GPUI then negotiates with the compositor
                // (Server is reported as granted even where nothing draws a frame,
                // as on GNOME/Wayland) and the app draws its own title bar when it
                // is not given one. macOS keeps its native bar.
                window_decorations: Some(if cfg!(target_os = "linux") {
                    WindowDecorations::Client
                } else {
                    WindowDecorations::Server
                }),
                // Matches the window class a desktop entry would use.
                app_id: Some("gitmaster".into()),
                ..WindowOptions::default()
            };
            let opened = cx.open_window(options, move |window, cx| {
                cx.new(|cx| StatusView::new(engine, command_log, path, window, cx))
            });
            if let Err(error) = opened {
                eprintln!("gitmaster: {error}");
                std::process::exit(1);
            }
            cx.activate(true);
        });
}

/// The list keys are scoped to the list's context, so they never fire while
/// typing into the filter, the message or the diff pane. Tab and the quit key
/// work everywhere.
fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("k", SelectPrev, Some(KEY_CONTEXT)),
        KeyBinding::new("up", SelectPrev, Some(KEY_CONTEXT)),
        KeyBinding::new("space", ToggleStage, Some(KEY_CONTEXT)),
        KeyBinding::new("/", FocusFilter, Some(KEY_CONTEXT)),
        KeyBinding::new("r", Refresh, Some(KEY_CONTEXT)),
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrev, None),
        KeyBinding::new("secondary-q", Quit, None),
    ]);
    composer::bind_keys(cx);
    diff_pane::bind_keys(cx);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_binding_string_is_valid() {
        // A malformed keystroke panics when bound at startup; these catch it in tests.
        for keys in [
            "j",
            "down",
            "k",
            "up",
            "space",
            "/",
            "r",
            "tab",
            "shift-tab",
            "secondary-q",
        ] {
            let binding = KeyBinding::new(keys, Refresh, Some(KEY_CONTEXT));
            assert_eq!(binding.keystrokes().len(), 1, "{keys}");
        }
    }
}
