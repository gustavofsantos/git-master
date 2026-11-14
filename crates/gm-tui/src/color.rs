//! Colors code off the input loop. The highlighter may take up to its budget on a large
//! file, so it runs on a thread of its own. Jobs are kept by what they color (D251 (2)): a
//! newer job for the same target stops the one waiting or in progress, and jobs for other
//! targets all run, in the order they came.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use gm_core::syntax::{highlight, language_for_path};
use gm_schema::HighlightSpan;

use crate::data::{ColorJob, ColorTarget};

/// The colors of a job's text.
pub type Colored = (ColorJob, Vec<HighlightSpan>);

/// A job and its flag: set to stop it, and set by the thread once it ran.
type Queued = (ColorJob, Arc<AtomicBool>);

pub struct Colorer {
    jobs: Sender<Queued>,
    /// The flag of the newest job sent for each target, until it ran.
    newest: HashMap<ColorTarget, Arc<AtomicBool>>,
    pub done: Receiver<Colored>,
}

impl Colorer {
    /// Starts the coloring thread. It ends when the `Colorer` is dropped.
    pub fn start() -> std::io::Result<Self> {
        let (jobs, inbox) = mpsc::channel::<Queued>();
        let (out, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("gm-tui-color".into())
            .spawn(move || {
                let mut waiting: VecDeque<Queued> = VecDeque::new();
                loop {
                    if waiting.is_empty() {
                        let Ok(next) = inbox.recv() else { break };
                        waiting.push_back(next);
                    }
                    waiting.extend(inbox.try_iter());
                    let Some((job, stop)) = waiting.pop_front() else {
                        continue;
                    };
                    if stop.load(Ordering::SeqCst) {
                        continue;
                    }
                    let spans = language_for_path(&job.path)
                        .and_then(|language| highlight(language, &job.text, &stop));
                    stop.store(true, Ordering::SeqCst);
                    if let Some(spans) = spans
                        && out.send((job, spans)).is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Self {
            jobs,
            newest: HashMap::new(),
            done,
        })
    }

    /// Colors `job`, stopping the job of the same target that has not run yet.
    pub fn color(&mut self, job: ColorJob) {
        self.newest.retain(|_, stop| !stop.load(Ordering::SeqCst));
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(older) = self.newest.insert(job.target.clone(), Arc::clone(&stop)) {
            older.store(true, Ordering::SeqCst);
        }
        let _ = self.jobs.send((job, stop));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::data::Group;

    fn job(path: &str, text: &str) -> ColorJob {
        let target = ColorTarget::Diff {
            group: Group::Unstaged,
            path: path.to_owned(),
            old: false,
        };
        let mut job = ColorJob::new(target, path);
        job.push(Some((0, 0)), text);
        job
    }

    #[test]
    fn jobs_for_different_files_are_all_colored() {
        let mut colorer = Colorer::start().unwrap();
        colorer.color(job("a.rs", "fn a() {}"));
        colorer.color(job("b.rs", "fn b() {}"));
        let mut paths: Vec<String> = (0..2)
            .map(|_| {
                colorer
                    .done
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap()
                    .0
                    .path
            })
            .collect();
        paths.sort();
        assert_eq!(paths, ["a.rs", "b.rs"]);
    }

    #[test]
    fn a_newer_job_for_the_same_target_replaces_the_older_one() {
        let mut colorer = Colorer::start().unwrap();
        // Keep the thread busy so the next two usually wait together.
        colorer.color(job("busy.rs", &"fn f() {}\n".repeat(5_000)));
        colorer.color(job("a.rs", "fn old() {}"));
        let older = Arc::clone(&colorer.newest[&job("a.rs", "").target]);
        colorer.color(job("a.rs", "fn new() {}"));
        assert!(
            older.load(Ordering::SeqCst),
            "the older job is told to stop"
        );
        let mut texts = Vec::new();
        while let Ok((job, _)) = colorer.done.recv_timeout(Duration::from_secs(10)) {
            let last = job.text == "fn new() {}\n";
            texts.push(job.text);
            if last {
                break;
            }
        }
        // The older job is stopped unless it ran before the newer one was sent; it never
        // comes after the newer one.
        assert_eq!(texts.last().map(String::as_str), Some("fn new() {}\n"));
        assert!(
            colorer
                .done
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
    }
}
