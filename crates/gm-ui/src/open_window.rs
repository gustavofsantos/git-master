//! Opens a worktree, a workspace or a submodule (D234). **Switch to** moves this
//! window to its folder in place: one engine per window, so the switch dispatches
//! `OpenRepo` (see `project_menu.rs`), and the window keeps the view of each root it
//! leaves. **Open in New Window** keeps the earlier behaviour (D31, Q11, W4): the
//! window is another process of this program, started with the folder, so it runs
//! the same open as a path on the command line. The window itself does not wait
//! for it.

use std::path::Path;
use std::process::Stdio;

use gpui::Context;

use crate::view::StatusView;

/// Starts this program on `path` as a new process, which opens its own window.
/// Its errors go to this process's stderr, as a command-line start's would.
pub(crate) fn spawn_window(path: &str) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut child = std::process::Command::new(exe)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()?;
    // Reaped on a thread of its own, so the window never keeps a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

impl StatusView {
    /// Switches this window to the worktree, workspace or submodule folder at `path` (D234 (1)).
    /// The root already shown does nothing; the guards of the project menu apply otherwise.
    pub(crate) fn open_worktree(&mut self, path: &str, cx: &mut Context<Self>) {
        let shown = self
            .model
            .status()
            .is_some_and(|status| status.root.trim_end_matches('/') == path.trim_end_matches('/'));
        if shown {
            return;
        }
        self.open_repo(Path::new(path), cx);
    }

    /// Opens the folder at `path` in a window of its own (D234 (2)), or says why it cannot.
    pub(crate) fn open_in_new_window(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Err(error) = spawn_window(path) {
            self.notify_user(&format!("Could not open a new window: {error}"), cx);
        }
    }
}
