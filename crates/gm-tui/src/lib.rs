//! The terminal companion, `gitmaster tui [PATH]` (D235): `GitMaster` in a terminal pane next
//! to a coding agent. A third front end of the engine: it dispatches the same commands as the
//! app, as the person, and reads the same store. It opens no window and starts no GPUI code.

mod agent_screen;
mod blame_screens;
mod color;
mod data;
mod diff_screen;
mod draw;
mod help;
mod icons;
mod nav;
mod palette;
mod picker;
mod review_screens;
mod reviews;
mod scrollbar;
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
    let root = std::path::PathBuf::from(&status.root);
    let mut app = App::new(status);
    // The layout the app uses, for this session only (D252 (2)).
    let settings = gm_core::settings::Settings::load(&root, &gm_core::settings::Source::default());
    if settings.diff_layout == gm_core::settings::DiffLayout::Split {
        app.layout = gm_view::diff_rows::Layout::Split;
    }
    if settings.line_wrap == Some(false) {
        app.wrap = crate::ui::LineWrap::Cut;
    }
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

/// The companion's commit and push against the real engine and real Git: the actions send
/// the commands, the engine runs them, and the answers reach the hint line.
#[cfg(test)]
mod remote_real_git {
    use std::path::Path;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::ui::App;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// Sends what the app asked for and takes the engine's answers until `done` holds.
    fn pump(app: &mut App, opened: &super::Opened, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(app) {
            assert!(Instant::now() < deadline, "timed out: {:?}", app.notice);
            for kind in app.take_commands() {
                let id = opened
                    .engine
                    .dispatch(gm_schema::Actor::Human, kind.clone());
                app.sent(id, &kind);
            }
            while let Ok(event) = opened.events.try_recv() {
                app.apply(&event);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers, now: Instant) {
        app.on_key_at(KeyEvent::new(code, modifiers), now);
    }

    /// The files the open picker lists.
    fn picked(app: &App) -> Vec<String> {
        match app.overlays.last() {
            Some(crate::ui::Overlay::Files { picker, .. }) => {
                picker.hits().iter().map(|hit| hit.path.clone()).collect()
            }
            _ => Vec::new(),
        }
    }

    #[test]
    fn finds_a_file_opens_it_at_the_line_and_comments_on_that_line() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let text = (1..=10)
            .map(|n| format!("let line_{n} = {n};"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("src/parser.rs"), text).unwrap();
        std::fs::write(dir.path().join("README.md"), "read me\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        // A file never committed is found too.
        std::fs::write(dir.path().join("src/lexer.rs"), "fn lex() {}\n").unwrap();

        let opened = super::open(dir.path()).unwrap();
        let mut app = App::new(opened.status.clone());
        let later = || Instant::now() + Duration::from_secs(2);

        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE, later());
        press(&mut app, KeyCode::Char('o'), KeyModifiers::NONE, later());
        app.on_paste("lexer");
        pump(&mut app, &opened, |app| picked(app) == ["src/lexer.rs"]);
        for _ in 0.."lexer".len() {
            press(&mut app, KeyCode::Backspace, KeyModifiers::NONE, later());
        }
        app.on_paste("parser:3");
        pump(&mut app, &opened, |app| picked(app) == ["src/parser.rs"]);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE, later());
        assert_eq!(app.stack.trail(), ["status", "file/src/parser.rs"]);
        pump(&mut app, &opened, |app| {
            app.view().tree.current().is_some_and(|section| {
                matches!(section.item, crate::status::Item::FileLine { index: 2, .. })
            })
        });

        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE, later());
        press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE, later());
        app.on_paste("Name this better.");
        press(
            &mut app,
            KeyCode::Char('s'),
            KeyModifiers::CONTROL,
            later() + Duration::from_secs(2),
        );
        pump(&mut app, &opened, |app| {
            app.overlays.is_empty()
                && app.repo.comments.iter().any(|comment| {
                    comment.path == "src/parser.rs"
                        && comment
                            .location
                            .is_some_and(|l| (l.start_line, l.end_line) == (3, 3))
                })
        });
        // The thread is drawn under line 3, on the File screen.
        let rows = app.view().tree.rows();
        let line = rows
            .iter()
            .position(|row| {
                matches!(
                    row.section.item,
                    crate::status::Item::FileLine { index: 2, .. }
                )
            })
            .unwrap();
        assert!(
            matches!(
                rows[line + 1].section.item,
                crate::status::Item::Card { .. }
            ),
            "{:?}",
            rows[line + 1].section.item
        );
    }

    /// The row of the top screen showing line `new` (new side) or `old` (removed) of the
    /// commit's `path`.
    fn commit_row(app: &App, path: &str, new: Option<u32>, old: Option<u32>) -> usize {
        use crate::data::Group;
        let rows = app.view().tree.rows();
        rows.iter()
            .position(|row| match &row.section.item {
                crate::status::Item::Line {
                    group: Group::Commit,
                    path: p,
                    row,
                } if p == path => crate::status::line_of(&app.repo, Group::Commit, p, *row)
                    .is_some_and(|line| {
                        line.new_no == new && (old.is_none() || line.old_no == old)
                    }),
                _ => false,
            })
            .unwrap_or_else(|| panic!("no row for {new:?}/{old:?}"))
    }

    /// D259: on a commit's diff, lines whose text repeats in the file now take a comment,
    /// placed by their context, and removed lines alone take one on the commit's parent.
    #[test]
    fn comments_on_a_commit_s_lines_whose_text_repeats_and_on_its_removed_lines() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("f.clj"), "(a)\n(old)\n(b)\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        let parent = git(dir.path(), &["rev-parse", "HEAD"]);
        let text = "(a)\n(fact 1\n  )\n(b)\n(fact 2\n  )\n";
        std::fs::write(dir.path().join("f.clj"), text).unwrap();
        git(dir.path(), &["commit", "-q", "-am", "second"]);
        let head = git(dir.path(), &["rev-parse", "HEAD"]);

        let opened = super::open(dir.path()).unwrap();
        let mut app = App::new(opened.status.clone());
        let later = || Instant::now() + Duration::from_secs(2);
        app.open_diff(crate::diff_screen::DiffKind::Commit(head.clone()));
        pump(&mut app, &opened, |app| {
            app.repo
                .files
                .get(&(crate::data::Group::Commit, "f.clj".to_owned()))
                .is_some_and(|file| file.diff.is_some())
        });
        app.run("fold.open_all", later());

        // Line 6, "  )": the same text is line 3 too.
        let at = commit_row(&app, "f.clj", Some(6), None);
        app.view_mut().tree.move_to(at);
        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE, later());
        press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE, later());
        app.on_paste("Why a second fact?");
        press(
            &mut app,
            KeyCode::Char('s'),
            KeyModifiers::CONTROL,
            later() + Duration::from_secs(2),
        );
        pump(&mut app, &opened, |app| {
            app.overlays.is_empty() && !app.repo.comments.is_empty()
        });
        let comment = &app.repo.comments[0];
        assert_eq!(comment.anchor.text, "  )");
        assert_eq!(comment.anchor.revision.as_deref(), Some(head.as_str()));
        assert_eq!(
            comment.location.map(|l| (l.start_line, l.end_line)),
            Some((6, 6))
        );

        // The removed "(old)" alone: on the parent, gone from the working tree.
        let at = commit_row(&app, "f.clj", None, Some(2));
        app.view_mut().tree.move_to(at);
        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE, later());
        press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE, later());
        app.on_paste("Why was this removed?");
        press(
            &mut app,
            KeyCode::Char('s'),
            KeyModifiers::CONTROL,
            later() + Duration::from_secs(2),
        );
        pump(&mut app, &opened, |app| {
            app.overlays.is_empty() && app.repo.comments.len() == 2
        });
        let removed = app
            .repo
            .comments
            .iter()
            .find(|c| c.anchor.text == "(old)")
            .expect("the comment on the removed line");
        assert_eq!(removed.anchor.revision.as_deref(), Some(parent.as_str()));
        assert_eq!(removed.anchor.start_line, 2);
        assert!(removed.location.is_none());
    }

    #[test]
    fn commits_the_staged_change_and_pushes_it_to_the_upstream() {
        let remote = tempfile::tempdir().unwrap();
        git(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        let url = remote.path().to_string_lossy().into_owned();
        git(dir.path(), &["remote", "add", "origin", &url]);
        git(dir.path(), &["push", "-q", "-u", "origin", "main"]);
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);

        let opened = super::open(dir.path()).unwrap();
        let mut app = App::new(opened.status.clone());
        let later = || Instant::now() + Duration::from_secs(2);

        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE, later());
        press(&mut app, KeyCode::Char('C'), KeyModifiers::SHIFT, later());
        app.on_paste("second");
        press(
            &mut app,
            KeyCode::Char('s'),
            KeyModifiers::CONTROL,
            later() + Duration::from_secs(2),
        );
        pump(&mut app, &opened, |app| {
            app.notice
                .as_deref()
                .is_some_and(|n| n.starts_with("Committed"))
                && app.overlays.is_empty()
        });
        assert_eq!(git(dir.path(), &["log", "-1", "--format=%s"]), "second");
        let head = git(dir.path(), &["rev-parse", "HEAD"]);
        // The status that followed the commit reaches the app: one ahead of origin/main.
        pump(&mut app, &opened, |app| app.repo.status.branch.ahead == 1);

        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE, later());
        press(&mut app, KeyCode::Char('P'), KeyModifiers::SHIFT, later());
        press(
            &mut app,
            KeyCode::Char('y'),
            KeyModifiers::NONE,
            later() + Duration::from_secs(2),
        );
        pump(&mut app, &opened, |app| {
            app.notice.as_deref() == Some("Pushed.")
        });
        assert_eq!(git(remote.path(), &["rev-parse", "main"]), head);
    }
}
