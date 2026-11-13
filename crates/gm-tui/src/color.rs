//! Colors code off the input loop. The highlighter may take up to its budget on a large
//! file, so it runs on a thread of its own; a newer job stops the one in progress, and only
//! the newest of the waiting jobs runs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use gm_core::syntax::{highlight, language_for_path};
use gm_schema::HighlightSpan;

use crate::diff::ColorJob;

/// The colors of a job's text; empty when its language has no grammar.
pub type Colored = (ColorJob, Vec<HighlightSpan>);

pub struct Colorer {
    jobs: Sender<(ColorJob, Arc<AtomicBool>)>,
    /// Set to stop the job in progress.
    current: Option<Arc<AtomicBool>>,
    pub done: Receiver<Colored>,
}

impl Colorer {
    /// Starts the coloring thread. It ends when the `Colorer` is dropped.
    pub fn start() -> std::io::Result<Self> {
        let (jobs, inbox) = mpsc::channel::<(ColorJob, Arc<AtomicBool>)>();
        let (out, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("gm-tui-color".into())
            .spawn(move || {
                while let Ok(mut next) = inbox.recv() {
                    // Only the newest waiting job is worth coloring.
                    while let Ok(newer) = inbox.try_recv() {
                        next = newer;
                    }
                    let (job, stop) = next;
                    let Some(language) = language_for_path(&job.path) else {
                        continue;
                    };
                    if let Some(spans) = highlight(language, &job.text, &stop)
                        && out.send((job, spans)).is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Self {
            jobs,
            current: None,
            done,
        })
    }

    /// Colors `job`, stopping the job in progress.
    pub fn color(&mut self, job: ColorJob) {
        if let Some(stop) = self.current.take() {
            stop.store(true, Ordering::SeqCst);
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.current = Some(Arc::clone(&stop));
        let _ = self.jobs.send((job, stop));
    }
}
