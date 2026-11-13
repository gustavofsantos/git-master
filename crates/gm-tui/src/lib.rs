//! The terminal companion, `gitmaster tui [PATH]` (D235): `GitMaster` in a terminal pane next
//! to a coding agent. A third front end of the engine: it dispatches the same commands as the
//! app, as the person, and reads the same store. It opens no window and starts no GPUI code.

mod app;
mod render;
mod terminal;

use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use async_channel::Receiver;
use gm_core::EngineHandle;
use gm_core::git::SubprocessGit;
use gm_core::watch::{self, Notice, WatchHandle};
use gm_schema::{Actor, CommandKind, Event, OperationState, RepoStatus};
use ratatui::crossterm::event::{self, Event as Input};

use crate::app::App;
use crate::terminal::Screen;

/// How long the loop waits for a key or a click before it looks at the engine again.
const TICK: Duration = Duration::from_millis(50);

/// Runs `gitmaster tui` with the words after `tui` and returns the exit status: 0 after the
/// person quits, 1 when the repository cannot be opened or the terminal cannot be used, 2 for
/// a wrong command line.
pub fn command(words: &[String]) -> i32 {
    let path = match words {
        [] => std::env::current_dir(),
        [path] => std::path::absolute(path),
        _ => {
            eprintln!("usage: gitmaster tui [PATH]");
            return 2;
        }
    };
    let path = match path {
        Ok(path) => path,
        Err(error) => {
            eprintln!("gitmaster tui: {error}");
            return 1;
        }
    };
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("gitmaster tui: standard input and output must be a terminal");
        return 1;
    }
    let opened = match open(&path) {
        Ok(opened) => opened,
        Err(error) => {
            eprintln!("gitmaster tui: {error}");
            return 1;
        }
    };
    match run(opened) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("gitmaster tui: {error}");
            1
        }
    }
}

/// The repository open in an engine of its own, with the watcher started.
pub struct Opened {
    pub engine: EngineHandle,
    /// Every event of the engine since before the open.
    pub events: Receiver<Event>,
    pub status: RepoStatus,
    pub watch: WatchHandle,
    /// The watcher's notices, forwarded so the loop can read them without waiting.
    pub notices: mpsc::Receiver<Notice>,
}

/// Opens the repository that contains `path` and waits for its first status. The workflow
/// triggers stay off: the companion has no place to approve a run (D235).
pub fn open(path: &Path) -> Result<Opened, String> {
    let git = SubprocessGit::default();
    let login_path = git.login_path.clone();
    let engine = gm_core::spawn(Box::new(git));
    let events = engine.subscribe();
    // Started before the open, so the watcher sees the repository open.
    let (watch, notices) = watch::spawn(engine.clone(), login_path);
    let id = engine.dispatch(
        Actor::Human,
        CommandKind::OpenRepo {
            path: path.to_string_lossy().into_owned(),
        },
    );
    let status = loop {
        match events.recv_blocking() {
            Ok(Event::StatusLoaded { id: of, status }) if of == id => break status,
            Ok(Event::OperationChanged {
                id: of,
                state: OperationState::Failed | OperationState::Vetoed | OperationState::Cancelled,
                detail,
                ..
            }) if of == id => {
                return Err(detail.unwrap_or_else(|| "the repository could not be opened".into()));
            }
            Ok(_) => {}
            Err(_) => return Err("the engine stopped".to_owned()),
        }
    };
    let (forward, notices_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("gm-tui-notices".into())
        .spawn(move || {
            while let Some(notice) = notices.next_blocking() {
                if forward.send(notice).is_err() {
                    break;
                }
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(Opened {
        engine,
        events,
        status,
        watch,
        notices: notices_rx,
    })
}

fn run(opened: Opened) -> io::Result<()> {
    let Opened {
        engine,
        events,
        status,
        watch,
        notices,
    } = opened;
    let mut screen = Screen::take()?;
    let mut app = App::new(status);
    loop {
        if app.dirty {
            screen
                .terminal
                .draw(|frame| render::draw(frame, &mut app))?;
            app.dirty = false;
        }
        if event::poll(TICK)? {
            loop {
                match event::read()? {
                    Input::Key(key) => app.on_key(key),
                    Input::Mouse(mouse) => app.on_mouse(mouse),
                    Input::FocusGained => {
                        // As the app does on focus: covers a repository the watcher cannot watch.
                        watch.request();
                        app.redraw();
                    }
                    Input::Resize(..) => app.redraw(),
                    Input::FocusLost | Input::Paste(_) => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        while let Ok(event) = events.try_recv() {
            app.apply(&event);
        }
        while let Ok(notice) = notices.try_recv() {
            match notice {
                Notice::Degraded { reason } => {
                    app.set_notice(Some(format!("Not watching: {reason}")));
                }
                Notice::Watching { .. } => app.set_notice(None),
                Notice::Refreshing(_) => {}
            }
        }
        for kind in app.take_commands() {
            engine.dispatch(Actor::Human, kind);
        }
        if app.quit {
            return Ok(());
        }
    }
}
