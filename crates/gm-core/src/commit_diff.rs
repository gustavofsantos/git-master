//! The diff of one file in one commit (H4): a read on a thread of its own, the
//! same shape as the commit detail read. Its answer is dropped when a newer
//! request superseded it.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gm_schema::{FileDiff, OperationId};

use crate::port::FileDiffRead;

/// One file diff read, run on a thread of its own. `stop` is shared with the
/// engine, which sets it when the read is superseded.
pub struct FileDiffJob {
    pub id: OperationId,
    pub read: FileDiffRead,
    pub stop: Arc<AtomicBool>,
}

/// What a file diff job answers. A failure carries git's own message.
pub struct FileDiffReport {
    pub id: OperationId,
    pub outcome: Result<FileDiff, String>,
}

impl FileDiffJob {
    /// Runs the read to its end, or to the stop request.
    #[must_use]
    pub fn run(self) -> FileDiffReport {
        let outcome = (self.read)(&self.stop).map_err(|error| error.0);
        FileDiffReport {
            id: self.id,
            outcome,
        }
    }
}
