//! The command log: every `git` process that changes the repository, kept in
//! memory for the window's log panel. Reads (status, diff, history) are not
//! recorded. The log is an instance handed to the adapter, not a global, so
//! two engines in one process (or two tests) never share entries.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_channel::{Receiver, Sender};

/// The most records kept: the oldest go first.
pub const KEPT: usize = 1000;

/// The most bytes of each stream kept in a record.
pub const MAX_TEXT: usize = 16 * 1024;

/// One finished git process. The stdin payload (a commit message, a blob) is
/// never part of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRecord {
    pub program: String,
    pub dir: PathBuf,
    pub args: Vec<String>,
    /// `None` when git was killed by a signal.
    pub code: Option<i32>,
    pub stdout_bytes: usize,
    /// Cut at `MAX_TEXT` bytes.
    pub stdout: String,
    pub stderr: String,
    pub took: Duration,
}

impl CommandRecord {
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }

    /// The command as one shell-like line.
    #[must_use]
    pub fn line(&self) -> String {
        let mut line = format!("{} -C {}", self.program, self.dir.display());
        for arg in &self.args {
            line.push(' ');
            line.push_str(arg);
        }
        line
    }
}

/// Keeps at most `KEPT` records. Cloning shares the same log.
#[derive(Clone)]
pub struct CommandLog {
    entries: Arc<Mutex<VecDeque<CommandRecord>>>,
    wake: Sender<()>,
    woken: Receiver<()>,
}

impl Default for CommandLog {
    fn default() -> Self {
        let (wake, woken) = async_channel::bounded(1);
        Self {
            entries: Arc::default(),
            wake,
            woken,
        }
    }
}

impl CommandLog {
    pub fn record(&self, record: CommandRecord) {
        if let Ok(mut entries) = self.entries.lock() {
            if entries.len() == KEPT {
                entries.pop_front();
            }
            entries.push_back(record);
        }
        // A full channel already holds a pending wake-up.
        let _ = self.wake.try_send(());
    }

    /// The records, oldest first.
    #[must_use]
    pub fn entries(&self) -> Vec<CommandRecord> {
        self.entries
            .lock()
            .map(|entries| entries.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Yields once whenever records were added since the last wait.
    #[must_use]
    pub fn changes(&self) -> Receiver<()> {
        self.woken.clone()
    }
}

/// `bytes` as text, cut at `MAX_TEXT` on a character boundary.
#[must_use]
pub fn bounded_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= MAX_TEXT {
        return text.into_owned();
    }
    let mut end = MAX_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… (cut)", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(arg: &str) -> CommandRecord {
        CommandRecord {
            program: "git".into(),
            dir: PathBuf::from("/w"),
            args: vec![arg.into()],
            code: Some(0),
            stdout_bytes: 0,
            stdout: String::new(),
            stderr: String::new(),
            took: Duration::from_millis(1),
        }
    }

    #[test]
    fn keeps_the_newest_records_in_order() {
        let log = CommandLog::default();
        for i in 0..KEPT + 5 {
            log.record(record(&i.to_string()));
        }
        let entries = log.entries();
        assert_eq!(entries.len(), KEPT);
        assert_eq!(entries[0].args, ["5"]);
        assert_eq!(entries[KEPT - 1].args, [(KEPT + 4).to_string()]);
    }

    #[test]
    fn logs_do_not_share_entries() {
        let (a, b) = (CommandLog::default(), CommandLog::default());
        a.record(record("add"));
        assert_eq!(a.entries().len(), 1);
        assert!(b.entries().is_empty());
    }

    #[test]
    fn a_burst_leaves_one_pending_wake_up() {
        let log = CommandLog::default();
        let changes = log.changes();
        log.record(record("a"));
        log.record(record("b"));
        assert!(changes.try_recv().is_ok());
        assert!(changes.try_recv().is_err());
    }

    #[test]
    fn the_command_reads_as_one_line() {
        let mut r = record("commit");
        r.args.push("-F".into());
        assert_eq!(r.line(), "git -C /w commit -F");
    }

    #[test]
    fn long_output_is_cut_on_a_character_boundary() {
        let text = "é".repeat(MAX_TEXT);
        let cut = bounded_text(text.as_bytes());
        assert!(cut.ends_with("… (cut)"));
        assert!(cut.len() <= MAX_TEXT + "… (cut)".len());
    }
}
