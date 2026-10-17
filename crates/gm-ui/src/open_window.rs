//! Opens a worktree in a window of its own (slice 1c, W4; D31). One repository
//! per window (Q11): the window is another process of this program, started
//! with the worktree's folder, so it runs the same open as a path on the command
//! line. The window itself does not wait for it.

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
    /// Opens the worktree at `path` in its own window, or says why it cannot.
    pub(crate) fn open_worktree(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Err(error) = spawn_window(path) {
            self.notify_user(&format!("Could not open the worktree: {error}"), cx);
        }
    }
}
