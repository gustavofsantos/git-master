//! The `GitMaster` window. Slice 0: the status of one repository, read through
//! the engine. The engine runs on its own thread; this crate only renders what
//! the engine reports.

mod action_registry;
mod app_settings;
mod assets;
mod avatar;
mod branch_delete;
mod branch_drag;
mod branch_filter;
mod branch_menu;
mod branches;
mod changes;
mod code_page;
mod command_panel;
mod commit_threads;
mod composer;
mod conflict_resolver;
mod context_menu;
mod diff_pane;
mod diff_rows;
mod editor;
mod editor_theme;
mod file_discard;
mod fuzzy;
mod gate_wait;
mod history_graph;
mod history_scope;
mod hook_convert;
mod icon;
mod label_curate;
mod labels_page;
mod local_review;
mod lua_language;
mod markdown;
mod merged_branches;
mod message_dialog;
mod model;
mod move_file;
mod name_fit;
mod new_branch;
mod new_stash;
mod open_window;
mod palette;
mod palette_view;
mod project_menu;
mod provenance_view;
mod quick_open;
mod quick_open_view;
mod recent;
mod reflog;
mod remote_op;
mod resize;
mod resize_handle;
mod restack_dialog;
mod review;
mod review_composer;
mod script_completion;
mod shell_code;
mod shell_commit;
mod shell_conflicts;
mod shell_history;
mod shell_labels;
mod shell_list;
mod shell_markdown;
mod shell_provenance;
mod shell_reflog;
mod shell_review;
mod shell_search;
mod shell_sidebar;
mod shell_stashes;
mod shell_threads;
mod shell_workflows;
mod sidebar;
mod stacks;
mod stash_apply;
mod stash_drop;
mod stashes;
mod suggestion;
mod text_area;
mod text_area_element;
mod text_area_math;
mod text_search;
mod theme;
mod thread_curate;
mod thread_dialogs;
mod thread_rows;
mod threads;
mod threads_settings;
mod title_bar;
mod transform_run;
mod view;
mod welcome;
mod window_chrome;
mod workflow_editor;
mod workflow_inputs;
mod workflow_keys;
mod workflow_review;
mod workflow_run;
mod workflows_page;
mod worktree_create;
mod worktree_list;
mod worktree_prune;
mod worktree_remove;

use gm_core::EngineHandle;
use gm_core::command_log::CommandLog;
use gpui::{
    App, AppContext, Application, Bounds, Global, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions, actions, px, size,
};
use gpui_component::Root;

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
        OpenContextMenu,
        CloseContextMenu,
        FocusNext,
        FocusPrev,
        OpenPalette,
        OpenQuickOpen,
        OpenSearch,
        CopyCurrentBranch,
        SaveWorkflow,
    ]
);

/// Runs the manual workflow `name` from its shortcut (`trigger.key`). It runs as the palette's
/// Run Workflow does. Bound by `bind_keys_with_workflows`, one binding per shortcut.
#[derive(Clone, Debug, PartialEq, Eq, gpui::Action)]
#[action(namespace = gitmaster, no_json, no_register)]
pub struct RunWorkflowKey {
    pub name: String,
}

/// The name GPUI gives `RunWorkflowKey`; the built-in bindings leave those out.
const RUN_WORKFLOW_KEY: &str = "gitmaster::RunWorkflowKey";

/// Where the window starts: the path given on the command line, or the
/// working directory when none was given.
pub struct StartPath {
    pub path: String,
    /// Whether the path came from the command line.
    pub from_argument: bool,
}

/// Opens the window on the repository at the start path, or on the welcome
/// screen when that is not a repository, and runs until it closes.
pub fn run(
    engine: EngineHandle,
    command_log: CommandLog,
    watch: (gm_core::watch::WatchHandle, gm_core::watch::Notices),
    start: StartPath,
) {
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
            // The component's theme follows the palette in use. The OS appearance is not
            // known yet, so `StatusView` applies it again once the window reports it.
            // The Lua grammar of the script editor (D151) is known before the first window.
            lua_language::register();
            editor_theme::apply(cx);
            // The component binds its keys when it is set up; keep them, since the app clears
            // the keymap when it rebinds workflow shortcuts (see `bind_keys_with_workflows`).
            let component_keys = cx
                .key_bindings()
                .borrow()
                .bindings()
                .cloned()
                .collect::<Vec<_>>();
            cx.set_global(ComponentKeys(component_keys));
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
                // app draws its own header controls (see `window_chrome`).
                // On Linux, ask for Client: GPUI then negotiates with the compositor
                // (Server is reported as granted even where nothing draws a frame,
                // as on GNOME/Wayland) and the app draws its own title bar when it
                // is not given one. macOS keeps its native bar.
                window_decorations: Some(if cfg!(target_os = "linux") {
                    WindowDecorations::Client
                } else {
                    WindowDecorations::Server
                }),
                // With client decorations the app draws a margin around its frame
                // and the shadow falls into it, so the window must be transparent
                // there (X11 needs an alpha visual; Wayland already is). macOS
                // keeps an opaque window.
                window_background: if cfg!(target_os = "linux") {
                    WindowBackgroundAppearance::Transparent
                } else {
                    WindowBackgroundAppearance::Opaque
                },
                // Matches the window class a desktop entry would use.
                app_id: Some("gitmaster".into()),
                ..WindowOptions::default()
            };
            // The component's `Input` needs the window root to be a `Root` (D152). `Root` draws
            // its own padding and border; `window_chrome` moves the view back over them.
            let opened = cx.open_window(options, move |window, cx| {
                let view =
                    cx.new(|cx| StatusView::new(engine, command_log, watch, start, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            });
            if let Err(error) = opened {
                eprintln!("gitmaster: {error}");
                std::process::exit(1);
            }
            cx.activate(true);
        });
}

/// The key bindings the component adds when it is set up (its inputs, `Root`), kept so they can
/// be bound again after the keymap is cleared.
struct ComponentKeys(Vec<KeyBinding>);

impl Global for ComponentKeys {}

/// The context of the window root `Root`. Its own Tab binding moves focus with GPUI's order,
/// which would skip this app's focus cycle, so Tab is bound here to the app's actions instead.
const ROOT_CONTEXT: &str = "Root";

/// The list keys are scoped to the list's context, so they never fire while
/// typing into the message or the diff pane. Tab and the quit key
/// work everywhere.
fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("k", SelectPrev, Some(KEY_CONTEXT)),
        KeyBinding::new("up", SelectPrev, Some(KEY_CONTEXT)),
        KeyBinding::new("space", ToggleStage, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-f10", OpenContextMenu, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", CloseContextMenu, Some(KEY_CONTEXT)),
        KeyBinding::new("r", Refresh, Some(KEY_CONTEXT)),
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrev, None),
        KeyBinding::new("tab", FocusNext, Some(ROOT_CONTEXT)),
        KeyBinding::new("shift-tab", FocusPrev, Some(ROOT_CONTEXT)),
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-k", OpenPalette, None),
        KeyBinding::new("secondary-p", OpenQuickOpen, None),
        KeyBinding::new("secondary-shift-f", OpenSearch, None),
        KeyBinding::new("alt-secondary-c", CopyCurrentBranch, None),
        KeyBinding::new("secondary-s", SaveWorkflow, None),
        KeyBinding::new("secondary-shift-m", diff_pane::CommentOnSelection, None),
    ]);
    composer::bind_keys(cx);
    diff_pane::bind_keys(cx);
    review_composer::bind_keys(cx);
}

/// The window's own bindings as the keymap holds them now, as `(shortcut, what it does)` with the
/// shortcut in the spelling the keymap gives and the action in words. The shortcuts of workflows
/// are left out, and so are chords, which a single shortcut cannot clash with.
pub(crate) fn built_in_keys(cx: &App) -> Vec<(String, String)> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    // The component's bindings are not the app's: they clash with nothing the user types.
    let component: Vec<&str> = cx
        .global::<ComponentKeys>()
        .0
        .iter()
        .map(|binding| binding.action().name())
        .collect();
    keymap
        .bindings()
        .filter(|binding| binding.action().name() != RUN_WORKFLOW_KEY)
        .filter(|binding| !component.contains(&binding.action().name()))
        .filter_map(|binding| match binding.keystrokes() {
            [only] => Some((
                only.unparse(),
                workflow_keys::action_words(binding.action().name()),
            )),
            _ => None,
        })
        .collect()
}

/// Replaces the keymap with the window's own bindings and one binding per workflow shortcut.
/// Shortcuts GPUI cannot parse are skipped; the plan already left out the ones that clash.
pub(crate) fn bind_keys_with_workflows(cx: &mut App, bound: &[workflow_keys::WorkflowKey]) {
    cx.clear_key_bindings();
    let component = cx.global::<ComponentKeys>().0.clone();
    cx.bind_keys(component);
    bind_keys(cx);
    for key in bound {
        if gpui::Keystroke::parse(&key.keystroke).is_err() {
            continue;
        }
        cx.bind_keys([KeyBinding::new(
            &key.keystroke,
            RunWorkflowKey {
                name: key.workflow.clone(),
            },
            None,
        )]);
    }
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
    fn the_workflow_key_action_has_the_name_the_built_in_list_leaves_out() {
        use gpui::Action;
        let action = RunWorkflowKey {
            name: String::new(),
        };
        assert_eq!(action.name(), RUN_WORKFLOW_KEY);
    }

    #[test]
    fn every_key_binding_string_is_valid() {
        // A malformed keystroke panics when bound at startup; these catch it in tests.
        for keys in [
            "j",
            "down",
            "k",
            "up",
            "space",
            "shift-f10",
            "escape",
            "r",
            "tab",
            "shift-tab",
            "secondary-q",
            "secondary-k",
            "secondary-p",
            "secondary-shift-f",
            "alt-secondary-c",
            "secondary-s",
        ] {
            let binding = KeyBinding::new(keys, Refresh, Some(KEY_CONTEXT));
            assert_eq!(binding.keystrokes().len(), 1, "{keys}");
        }
    }
}
