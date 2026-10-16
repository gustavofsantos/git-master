//! One thread owns the engine (one writer per repository, docs/06 rule 3) and
//! talks to the front-ends over channels, so the UI never waits for it.
//! Tokio enters with the first async hook (slice 2).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_channel::{Receiver, Sender};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId};

use crate::{Engine, GitPort};

enum Message {
    Dispatch(OperationId, Command),
    Subscribe(Sender<Event>),
}

/// Cloneable front door to the engine thread. The thread stops when every
/// handle is dropped.
#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<Message>,
    next_id: Arc<AtomicU64>,
}

impl EngineHandle {
    /// Queues a command and returns its id at once. The id is allocated here,
    /// in dispatch order, so it matches the order the engine runs them in.
    pub fn dispatch(&self, actor: Actor, kind: CommandKind) -> OperationId {
        let id = OperationId(self.next_id.fetch_add(1, Ordering::SeqCst));
        // Sends are serialized by the channel; a closed channel means the engine
        // thread is gone and there is nobody to tell.
        let _ = self
            .tx
            .send_blocking(Message::Dispatch(id, Command { actor, kind }));
        id
    }

    /// A new stream of every event from now on. Each subscriber gets its own.
    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self.tx.send_blocking(Message::Subscribe(tx));
        rx
    }
}

/// # Panics
///
/// If the OS cannot start the engine thread.
pub fn spawn(git: Box<dyn GitPort>) -> EngineHandle {
    let (tx, rx) = async_channel::unbounded::<Message>();
    std::thread::Builder::new()
        .name("gm-engine".into())
        .spawn(move || run(Engine::new(git), &rx))
        .expect("spawn the engine thread");
    EngineHandle {
        tx,
        next_id: Arc::new(AtomicU64::new(1)),
    }
}

fn run(mut engine: Engine, rx: &Receiver<Message>) {
    let mut subscribers: Vec<Sender<Event>> = Vec::new();
    while let Ok(message) = rx.recv_blocking() {
        match message {
            Message::Subscribe(tx) => subscribers.push(tx),
            Message::Dispatch(id, command) => {
                for event in engine.handle(id, &command) {
                    subscribers.retain(|s| s.send_blocking(event.clone()).is_ok());
                }
            }
        }
    }
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
