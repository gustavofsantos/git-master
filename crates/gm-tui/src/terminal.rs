//! Takes the terminal and gives it back: raw mode, the alternate screen, mouse capture and
//! focus reports while the companion runs; the terminal as it was on quit, on error and on
//! panic.

use std::io::{self, Stdout, Write};
use std::time::{Duration, Instant};

use ratatui::Terminal;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::palette::{self, Palette, Tints};

/// The terminal while the companion has it. Dropping it gives the terminal back.
pub struct Screen {
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Screen {
    /// Takes the terminal. A panic from here on gives it back before the panic message is
    /// printed, so the message is readable and the shell is usable.
    pub fn take() -> io::Result<Self> {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
        enable_raw_mode()?;
        if !palette::asked() {
            palette::set(ask_tints());
        }
        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableFocusChange,
            EnableBracketedPaste
        ) {
            restore();
            return Err(error);
        }
        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                restore();
                Err(error)
            }
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
        let _ = self.terminal.show_cursor();
    }
}

/// Undoes what `Screen::take` did. Safe to call more than once; errors are ignored, since
/// there is nothing left to do about them.
fn restore() {
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        DisableFocusChange,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

/// How long the terminal has to answer the palette query.
const ANSWER_DEADLINE: Duration = Duration::from_millis(150);

/// Asks the terminal for its colors and blends the tints from them (D250); `None` when colors
/// are off, the terminal cannot show 24-bit color, or it did not answer in time. Runs in raw
/// mode before the first event read, so the answers never reach the key reader.
fn ask_tints() -> Option<Tints> {
    let off = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    let colorterm = std::env::var("COLORTERM").ok();
    if off || !palette::truecolor(colorterm.as_deref()) {
        return None;
    }
    let mut stdout = io::stdout();
    write!(stdout, "\x1b]10;?\x1b\\\x1b]11;?\x1b\\").ok()?;
    for index in 0..16 {
        write!(stdout, "\x1b]4;{index};?\x1b\\").ok()?;
    }
    // Every terminal answers DA1, so its answer marks the end of the others.
    write!(stdout, "\x1b[c").ok()?;
    stdout.flush().ok()?;
    Tints::from_palette(&Palette::parse(&read_answer()))
}

/// Reads the terminal's answers until the answer to DA1 or the deadline.
#[cfg(unix)]
fn read_answer() -> Vec<u8> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let stdin = rustix::stdio::stdin();
    let deadline = Instant::now() + ANSWER_DEADLINE;
    let mut reply = Vec::new();
    let mut chunk = [0u8; 1024];
    while !answered(&reply) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        let timeout = Timespec::try_from(left).ok();
        match poll(&mut fds, timeout.as_ref()) {
            Ok(0) | Err(_) => break,
            Ok(_) => match rustix::io::read(stdin, &mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => reply.extend_from_slice(&chunk[..read]),
            },
        }
    }
    reply
}

#[cfg(not(unix))]
fn read_answer() -> Vec<u8> {
    Vec::new()
}

/// Whether `reply` ends with the answer to DA1, `ESC [ ? … c`.
fn answered(reply: &[u8]) -> bool {
    reply.ends_with(b"c")
        && reply
            .windows(3)
            .rposition(|w| w == b"\x1b[?")
            .is_some_and(|at| {
                reply[at + 3..reply.len() - 1]
                    .iter()
                    .all(|b| b.is_ascii_digit() || *b == b';')
            })
}

#[cfg(test)]
mod tests {
    use super::answered;

    #[test]
    fn the_answer_to_da1_ends_the_reply() {
        assert!(answered(b"\x1b]11;rgb:1/1/1\x1b\\\x1b[?1;2c"));
        assert!(!answered(b"\x1b]11;rgb:1/1/1\x1b\\"));
        assert!(!answered(b"\x1b[?1;2"));
        assert!(!answered(b"\x1b]10;abc"));
    }
}
