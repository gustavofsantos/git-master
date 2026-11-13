//! The terminal companion, `gitmaster tui [PATH]` (D235): `GitMaster` in a terminal pane next
//! to a coding agent. A third front end of the engine: it dispatches the same commands as the
//! app, as the person, and reads the same store. It opens no window and starts no GPUI code.

mod blame_screens;
mod color;
mod data;
mod diff_screen;
mod draw;
mod help;
mod icons;
mod nav;
mod review_screens;
mod reviews;
mod status;
mod terminal;
mod threads;
mod ui;

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

use crate::color::Colorer;
use crate::terminal::Screen;
use crate::ui::{App, Request};

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
    let mut colorer = Colorer::start()?;
    let mut screen = Screen::take()?;
    let mut app = App::new(status);
    loop {
        // A prompt's keys light up when its guard opens (D242 (5)).
        if app.guard_closed(std::time::Instant::now()) {
            app.redraw();
        }
        if app.dirty {
            screen.terminal.draw(|frame| draw::draw(frame, &mut app))?;
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
                    Input::Paste(text) => app.on_paste(&text),
                    Input::FocusLost => {}
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
                    app.set_watch_notice(Some(format!("Not watching: {reason}")));
                }
                Notice::Watching { .. } => app.set_watch_notice(None),
                Notice::Refreshing(_) => {}
            }
        }
        while let Ok((job, spans)) = colorer.done.try_recv() {
            app.colored(&job, &spans);
        }
        for kind in app.take_commands() {
            let id = engine.dispatch(Actor::Human, kind.clone());
            app.sent(id, &kind);
        }
        for job in app.take_color_jobs() {
            colorer.color(job);
        }
        for request in app.take_requests() {
            match request {
                Request::Suspend => {
                    // Give the terminal back, stop as a shell job does, and take it again on `fg`.
                    drop(screen);
                    let _ = std::process::Command::new("kill")
                        .args(["-TSTP", &std::process::id().to_string()])
                        .status();
                    screen = Screen::take()?;
                    screen.terminal.clear()?;
                    app.redraw();
                }
                Request::Redraw => {
                    screen.terminal.clear()?;
                    app.redraw();
                }
                Request::Copy(text) => copy(&text)?,
                Request::Edit { path, line } => {
                    // The editor takes the terminal; the companion takes it back after.
                    drop(screen);
                    if let Err(error) = edit(&path, line) {
                        app.notify(format!("The editor did not start: {error}"));
                    }
                    screen = Screen::take()?;
                    screen.terminal.clear()?;
                    app.redraw();
                }
            }
        }
        if app.quit {
            return Ok(());
        }
    }
}

/// Runs `$VISUAL`, else `$EDITOR`, else `vi` on `path` at `line`, and waits for it. The
/// variable may hold arguments (`code -w`), split on spaces as a shell would for words.
fn edit(path: &str, line: u32) -> io::Result<()> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".to_owned());
    let mut words = editor.split_whitespace();
    let program = words.next().unwrap_or("vi");
    std::process::Command::new(program)
        .args(words)
        .arg(format!("+{line}"))
        .arg(path)
        .status()
        .map(|_| ())
}

/// Puts `text` on the clipboard with OSC 52, which terminals and tmux pass on.
fn copy(text: &str) -> io::Result<()> {
    use std::io::Write;
    let mut out = io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()
}

/// Standard base64, for OSC 52.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (at, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if at <= chunk.len() {
                out.push(char::from(
                    TABLE[usize::try_from((n >> shift) & 63).unwrap_or(0)],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod copy_tests {
    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(super::base64(b"@src/a.rs:4-8"), "QHNyYy9hLnJzOjQtOA==");
        assert_eq!(super::base64(b"ab"), "YWI=");
        assert_eq!(super::base64(b"abc"), "YWJj");
    }
}
