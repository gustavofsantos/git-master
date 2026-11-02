//! One thread owns the engine (one writer per repository, design/architecture.md rule 3) and
//! talks to the front-ends over channels, so the UI never waits for it. Slow
//! reads and the workflow runs have their own threads; they report back over
//! the same channel and the engine thread records the result.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_channel::{Receiver, Sender, WeakSender};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId};

use crate::commit_detail::DetailReport;
use crate::commit_diff::StackReport;
use crate::files::{
    HighlightJob, HighlightReport, IndexJob, IndexReport, ReadJob, ReadReport, SearchJob,
    SearchReport, TextJob, TextReport,
};
use crate::history::HistoryReport;
use crate::remote::RemoteReport;
use crate::script::run::ScriptEvent;
use crate::script_session::{EffectRequest, ScriptLink, ScriptReport};
use crate::threads_session::{ProvenanceJob, ProvenanceReport};
use crate::{Engine, GitPort};

enum Message {
    /// The sender lets the engine start a thread that reports back. It is
    /// dropped with the message, so the engine thread stops once every handle
    /// is gone and no run is in progress.
    Dispatch(OperationId, Command, Sender<Message>),
    Subscribe(Sender<Event>),
    History(HistoryReport),
    Detail(DetailReport),
    Stack(StackReport),
    Remote(RemoteReport),
    /// The file list was read. The sender is for the search that waited for it.
    Index(IndexReport, Sender<Message>),
    Search(SearchReport),
    /// A file was read. The sender is for the coloring that follows it.
    Read(ReadReport, Sender<Message>),
    Highlight(HighlightReport),
    Text(TextReport),
    /// The blame of a provenance read was read.
    Provenance(ProvenanceReport),
    /// A script run asks the engine to carry out an effect. The run waits for the answer.
    ScriptEffect(EffectRequest),
    /// A Git command a script run waits for has ended (`WF4a`).
    ScriptRemote(ScriptReport),
    /// A script run reports; the front ends hear it as events.
    ScriptEvent(ScriptEvent),
    /// The thread database changed on disk; the subscribers hear `ThreadsChanged`.
    ThreadsChanged,
    /// Refs of the open repository may have moved; the engine checks the branch tips (`WF3a`).
    RefsChanged(OperationId),
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

    /// Tells the subscribers that the thread database changed on disk (the watcher calls it).
    pub fn threads_changed(&self) {
        let _ = self.tx.send_blocking(Message::ThreadsChanged);
    }

    /// Tells the engine that refs of the open repository may have moved. The watcher calls it after
    /// its quiet period; the engine fires `refs.changed` for each branch whose tip moved.
    pub fn refs_changed(&self) {
        let id = OperationId(self.next_id.fetch_add(1, Ordering::SeqCst));
        let _ = self.tx.send_blocking(Message::RefsChanged(id));
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

/// Starts the engine. No workflow runs on an event: this is the engine of the MCP server and of
/// tests.
///
/// # Panics
///
/// If the OS cannot start the engine thread.
pub fn spawn(git: Box<dyn GitPort>) -> EngineHandle {
    spawn_engine(git, false)
}

/// The engine of the app, which runs the workflows on their triggers (D155(4)).
///
/// # Panics
///
/// If the OS cannot start the engine thread.
pub fn spawn_app(git: Box<dyn GitPort>) -> EngineHandle {
    spawn_engine(git, true)
}

/// Like `spawn`, with the workflow triggers on or off as `triggers` says: tests use it to
/// turn the triggers on.
///
/// # Panics
///
/// If the OS cannot start the engine thread.
pub fn spawn_with(git: Box<dyn GitPort>, triggers: bool) -> EngineHandle {
    spawn_engine(git, triggers)
}

fn spawn_engine(git: Box<dyn GitPort>, triggers: bool) -> EngineHandle {
    let (tx, rx) = async_channel::unbounded::<Message>();
    let busy = Arc::new(AtomicBool::new(false));
    let thread_busy = Arc::clone(&busy);
    // Script runs reach the engine thread through weak handles, so they do not keep it alive.
    let effects_to = tx.downgrade();
    let events_to = tx.downgrade();
    let reports_to = tx.downgrade();
    let link = ScriptLink {
        effects: Arc::new(move |request| {
            if let Some(tx) = effects_to.upgrade() {
                let _ = tx.send_blocking(Message::ScriptEffect(request));
            }
        }),
        events: Arc::new(move |event| {
            if let Some(tx) = events_to.upgrade() {
                let _ = tx.send_blocking(Message::ScriptEvent(event));
            }
        }),
    };
    std::thread::Builder::new()
        .name("gm-engine".into())
        .spawn(move || {
            let mut engine = Engine::new(git);
            engine.set_script_link(link);
            engine.set_triggers(triggers);
            run(engine, &rx, &thread_busy, &reports_to);
        })
        .expect("spawn the engine thread");
    EngineHandle {
        tx,
        next_id: Arc::new(AtomicU64::new(1)),
        busy,
    }
}

fn run(
    mut engine: Engine,
    rx: &Receiver<Message>,
    busy: &AtomicBool,
    reports_to: &WeakSender<Message>,
) {
    let mut subscribers: Vec<Sender<Event>> = Vec::new();
    let broadcast = |events: Vec<Event>, subscribers: &mut Vec<Sender<Event>>| {
        for event in events {
            subscribers.retain(|s| s.send_blocking(event.clone()).is_ok());
        }
    };
    while let Ok(message) = rx.recv_blocking() {
        match message {
            Message::Subscribe(tx) => subscribers.push(tx),
            Message::ScriptEffect(request) => {
                let events = engine.script_effect(request);
                broadcast(events, &mut subscribers);
                spawn_script_jobs(&mut engine, reports_to);
            }
            Message::ScriptRemote(report) => {
                let events = engine.script_remote_report(report);
                broadcast(events, &mut subscribers);
            }
            Message::ScriptEvent(event) => {
                let events = engine.script_event(event);
                broadcast(events, &mut subscribers);
            }
            Message::ThreadsChanged => broadcast(vec![Event::ThreadsChanged], &mut subscribers),
            Message::RefsChanged(id) => {
                let events = engine.refs_changed(id);
                broadcast(events, &mut subscribers);
            }
            Message::Dispatch(id, command, reports) => {
                busy.store(true, Ordering::SeqCst);
                let events = engine.handle(id, &command);
                broadcast(events, &mut subscribers);
                busy.store(false, Ordering::SeqCst);
                spawn_read_jobs(&mut engine, &reports);
                spawn_file_jobs(&mut engine, &reports);
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
            Message::Read(report, reports) => {
                let events = engine.read_report(report);
                broadcast(events, &mut subscribers);
                spawn_file_jobs(&mut engine, &reports);
            }
            Message::Highlight(report) => {
                let events = engine.highlight_report(report);
                broadcast(events, &mut subscribers);
            }
            Message::Text(report) => {
                let events = engine.text_report(report);
                broadcast(events, &mut subscribers);
            }
            Message::Provenance(report) => {
                let events = engine.provenance_report(report);
                broadcast(events, &mut subscribers);
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

/// Runs the Git commands script runs wait for, each on a thread of its own (`WF4a`). The report
/// goes back to the engine through `reports_to`, which does not keep the engine thread alive.
fn spawn_script_jobs(engine: &mut Engine, reports_to: &WeakSender<Message>) {
    for job in engine.take_script_jobs() {
        let reports_to = reports_to.clone();
        std::thread::Builder::new()
            .name("gm-script".into())
            .spawn(move || {
                let report = job.run();
                if let Some(tx) = reports_to.upgrade() {
                    let _ = tx.send_blocking(Message::ScriptRemote(report));
                }
            })
            .expect("spawn a script thread");
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
    for job in engine.take_highlight_jobs() {
        spawn_highlight(job, reports.clone());
    }
    for job in engine.take_text_jobs() {
        spawn_text(job, reports.clone());
    }
    for job in engine.take_provenance_jobs() {
        spawn_provenance(job, reports.clone());
    }
}

fn spawn_highlight(job: HighlightJob, reports: Sender<Message>) {
    std::thread::Builder::new()
        .name("gm-highlight".into())
        .spawn(move || {
            let report = job.run();
            let _ = reports.send_blocking(Message::Highlight(report));
        })
        .expect("spawn a highlight thread");
}

fn spawn_text(job: TextJob, reports: Sender<Message>) {
    std::thread::Builder::new()
        .name("gm-grep".into())
        .spawn(move || {
            let report = job.run();
            let _ = reports.send_blocking(Message::Text(report));
        })
        .expect("spawn a text search thread");
}

fn spawn_provenance(job: ProvenanceJob, reports: Sender<Message>) {
    std::thread::Builder::new()
        .name("gm-blame".into())
        .spawn(move || {
            let report = job.run();
            let _ = reports.send_blocking(Message::Provenance(report));
        })
        .expect("spawn a blame thread");
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
            let next = reports.clone();
            let _ = reports.send_blocking(Message::Read(report, next));
        })
        .expect("spawn a file read thread");
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
