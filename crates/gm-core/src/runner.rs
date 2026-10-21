//! One thread owns the engine (one writer per repository, docs/06 rule 3) and
//! talks to the front-ends over channels, so the UI never waits for it. Hook
//! runs are slow, so each gets its own thread; it reports back over the same
//! channel and the engine thread records the result.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_channel::{Receiver, Sender};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId};

use crate::commit_detail::DetailReport;
use crate::commit_diff::StackReport;
use crate::files::{IndexJob, IndexReport, ReadJob, ReadReport, SearchJob, SearchReport};
use crate::history::HistoryReport;
use crate::hook::{ApprovalStore, Report};
use crate::remote::RemoteReport;
use crate::workflow::driver::{ExecJob, ExecReport};
use crate::{Engine, GitPort};

enum Message {
    /// The sender lets the engine start a hook thread that reports back. It is
    /// dropped with the message, so the engine thread stops once every handle
    /// is gone and no hook is running.
    Dispatch(OperationId, Command, Sender<Message>),
    Subscribe(Sender<Event>),
    Hook(Report),
    History(HistoryReport),
    Detail(DetailReport),
    Stack(StackReport),
    Remote(RemoteReport),
    /// A workflow `exec` step's report, with the sender for the next step's thread.
    Workflow(ExecReport, Sender<Message>),
    /// The file list was read. The sender is for the search that waited for it.
    Index(IndexReport, Sender<Message>),
    Search(SearchReport),
    Read(ReadReport),
}

/// Cloneable front door to the engine thread. The thread stops when every
/// handle is dropped.
#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<Message>,
    next_id: Arc<AtomicU64>,
    /// Set while the engine thread handles a command.
    busy: Arc<AtomicBool>,
}

impl EngineHandle {
    /// Queues a command and returns its id at once. The id is allocated here,
    /// in dispatch order, so it matches the order the engine runs them in.
    pub fn dispatch(&self, actor: Actor, kind: CommandKind) -> OperationId {
        let id = OperationId(self.next_id.fetch_add(1, Ordering::SeqCst));
        // Sends are serialized by the channel; a closed channel means the engine
        // thread is gone and there is nobody to tell.
        let _ = self.tx.send_blocking(Message::Dispatch(
            id,
            Command { actor, kind },
            self.tx.clone(),
        ));
        id
    }

    /// Whether the engine thread is handling a command right now. A status
    /// started meanwhile would queue behind it and see its writes twice.
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    /// A new stream of every event from now on. Each subscriber gets its own.
    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self.tx.send_blocking(Message::Subscribe(tx));
        rx
    }
}

/// Starts the engine with approvals kept in the app's data directory.
///
/// # Panics
///
/// If the OS cannot start the engine thread.
pub fn spawn(git: Box<dyn GitPort>) -> EngineHandle {
    spawn_with(git, ApprovalStore::default_location())
}

/// Like `spawn`, with the approvals file given by the caller (tests use a
/// temporary one).
///
/// # Panics
///
/// If the OS cannot start the engine thread.
pub fn spawn_with(git: Box<dyn GitPort>, approvals: ApprovalStore) -> EngineHandle {
    let (tx, rx) = async_channel::unbounded::<Message>();
    let busy = Arc::new(AtomicBool::new(false));
    let thread_busy = Arc::clone(&busy);
    std::thread::Builder::new()
        .name("gm-engine".into())
        .spawn(move || run(Engine::with_approvals(git, approvals), &rx, &thread_busy))
        .expect("spawn the engine thread");
    EngineHandle {
        tx,
        next_id: Arc::new(AtomicU64::new(1)),
        busy,
    }
}

fn run(mut engine: Engine, rx: &Receiver<Message>, busy: &AtomicBool) {
    let mut subscribers: Vec<Sender<Event>> = Vec::new();
    let broadcast = |events: Vec<Event>, subscribers: &mut Vec<Sender<Event>>| {
        for event in events {
            subscribers.retain(|s| s.send_blocking(event.clone()).is_ok());
        }
    };
    while let Ok(message) = rx.recv_blocking() {
        match message {
            Message::Subscribe(tx) => subscribers.push(tx),
            Message::Dispatch(id, command, reports) => {
                busy.store(true, Ordering::SeqCst);
                let events = engine.handle(id, &command);
                broadcast(events, &mut subscribers);
                busy.store(false, Ordering::SeqCst);
                spawn_read_jobs(&mut engine, &reports);
                spawn_file_jobs(&mut engine, &reports);
                for job in engine.take_workflow_jobs() {
                    spawn_workflow(job, reports.clone());
                }
                for plan in engine.take_jobs() {
                    let reports = reports.clone();
                    std::thread::Builder::new()
                        .name("gm-hook".into())
                        .spawn(move || {
                            plan.execute(|report| {
                                let _ = reports.send_blocking(Message::Hook(report));
                            });
                        })
                        .expect("spawn a hook thread");
                }
            }
            Message::Hook(report) => {
                let events = engine.hook_report(report);
                broadcast(events, &mut subscribers);
            }
            Message::History(report) => {
                let events = engine.history_report(report);
                broadcast(events, &mut subscribers);
            }
            Message::Detail(report) => {
                let events = engine.detail_report(report);
                broadcast(events, &mut subscribers);
            }
            Message::Stack(report) => {
                let events = engine.stack_report(report);
                broadcast(events, &mut subscribers);
            }
            Message::Remote(report) => {
                let events = engine.remote_report(report);
                broadcast(events, &mut subscribers);
            }
            Message::Index(report, reports) => {
                let events = engine.index_report(report);
                broadcast(events, &mut subscribers);
                spawn_file_jobs(&mut engine, &reports);
            }
            Message::Search(report) => {
                let events = engine.search_report(report);
                broadcast(events, &mut subscribers);
            }
            Message::Read(report) => {
                let events = engine.read_report(report);
                broadcast(events, &mut subscribers);
            }
            Message::Workflow(report, reports) => {
                let events = engine.workflow_report(report);
                broadcast(events, &mut subscribers);
                for job in engine.take_workflow_jobs() {
                    spawn_workflow(job, reports.clone());
                }
            }
        }
    }
}

/// Runs the reads the engine accepted that walk or inspect history, the remote
/// operations: each on a thread of its own, reporting back over `reports`.
fn spawn_read_jobs(engine: &mut Engine, reports: &Sender<Message>) {
    for job in engine.take_history_jobs() {
        let reports = reports.clone();
        std::thread::Builder::new()
            .name("gm-history".into())
            .spawn(move || {
                let report = job.run();
                let _ = reports.send_blocking(Message::History(report));
            })
            .expect("spawn a history thread");
    }
    for job in engine.take_detail_jobs() {
        let reports = reports.clone();
        std::thread::Builder::new()
            .name("gm-detail".into())
            .spawn(move || {
                let report = job.run();
                let _ = reports.send_blocking(Message::Detail(report));
            })
            .expect("spawn a detail thread");
    }
    for job in engine.take_stack_jobs() {
        let reports = reports.clone();
        std::thread::Builder::new()
            .name("gm-stack".into())
            .spawn(move || {
                let report = job.run();
                let _ = reports.send_blocking(Message::Stack(report));
            })
            .expect("spawn a stack thread");
    }
    for job in engine.take_remote_jobs() {
        let reports = reports.clone();
        std::thread::Builder::new()
            .name("gm-remote".into())
            .spawn(move || {
                let report = job.run();
                let _ = reports.send_blocking(Message::Remote(report));
            })
            .expect("spawn a remote thread");
    }
}

/// Runs the file jobs the engine accepted, each on a thread of its own: the
/// read of the file list, the searches of it and the reads of one file.
fn spawn_file_jobs(engine: &mut Engine, reports: &Sender<Message>) {
    for job in engine.take_index_jobs() {
        spawn_index(job, reports.clone());
    }
    for job in engine.take_search_jobs() {
        spawn_search(job, reports.clone());
    }
    for job in engine.take_read_jobs() {
        spawn_read(job, reports.clone());
    }
}

fn spawn_index(job: IndexJob, reports: Sender<Message>) {
    std::thread::Builder::new()
        .name("gm-files".into())
        .spawn(move || {
            let report = job.run();
            let next = reports.clone();
            let _ = reports.send_blocking(Message::Index(report, next));
        })
        .expect("spawn a file list thread");
}

fn spawn_search(job: SearchJob, reports: Sender<Message>) {
    std::thread::Builder::new()
        .name("gm-search".into())
        .spawn(move || {
            let report = job.run();
            let _ = reports.send_blocking(Message::Search(report));
        })
        .expect("spawn a file search thread");
}

fn spawn_read(job: ReadJob, reports: Sender<Message>) {
    std::thread::Builder::new()
        .name("gm-read".into())
        .spawn(move || {
            let report = job.run();
            let _ = reports.send_blocking(Message::Read(report));
        })
        .expect("spawn a file read thread");
}

/// Runs one workflow `exec` step on its own thread. Its report goes back with
/// the sender, which the engine drops once the report is handled.
fn spawn_workflow(job: ExecJob, reports: Sender<Message>) {
    std::thread::Builder::new()
        .name("gm-workflow".into())
        .spawn(move || {
            let report = job.run();
            let next = reports.clone();
            let _ = reports.send_blocking(Message::Workflow(report, next));
        })
        .expect("spawn a workflow thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeGit;
    use gm_schema::{OperationState, RepoStatus};

    #[test]
    fn subscriber_sees_the_operation_to_its_end() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        let handle = spawn(Box::new(git));
        let events = handle.subscribe();
        let id = handle.dispatch(Actor::Human, CommandKind::OpenRepo { path: "/r".into() });
        loop {
            let event = events.recv_blocking().unwrap();
            if let Event::OperationChanged {
                id: got,
                state: OperationState::Done,
                ..
            } = event
            {
                assert_eq!(got, id);
                break;
            }
        }
    }
}
