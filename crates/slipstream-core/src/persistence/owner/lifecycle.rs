use super::{Command, PersistenceError};
use std::sync::{
    Mutex,
    mpsc::{SyncSender, TrySendError},
};
use std::thread::JoinHandle;

const STATE_OPEN: u8 = 0;
const STATE_CLOSING: u8 = 1;
const STATE_CLOSED: u8 = 2;

struct Admission {
    state: u8,
    sender: Option<SyncSender<Command>>,
}

/// Owns the bounded command queue and the worker thread's lifecycle.
///
/// Admission and shutdown share a lock so no command can be accepted after
/// shutdown begins, while every accepted command remains queued for the
/// worker before it is joined.
pub(super) struct OwnerLifecycle {
    admission: Mutex<Admission>,
    join: Mutex<Option<JoinHandle<()>>>,
    shutdown: Mutex<Option<Result<(), PersistenceError>>>,
}

impl OwnerLifecycle {
    pub(super) fn new(sender: SyncSender<Command>, join: JoinHandle<()>) -> Self {
        Self {
            admission: Mutex::new(Admission {
                state: STATE_OPEN,
                sender: Some(sender),
            }),
            join: Mutex::new(Some(join)),
            shutdown: Mutex::new(None),
        }
    }

    pub(super) fn submit(&self, command: Command) -> Result<(), PersistenceError> {
        let admission = self.admission.lock().unwrap();
        if admission.state != STATE_OPEN {
            return Err(PersistenceError::Closed);
        }
        let sender = admission.sender.as_ref().ok_or(PersistenceError::Closed)?;
        sender.try_send(command).map_err(|error| match error {
            TrySendError::Full(_) => PersistenceError::Saturated,
            TrySendError::Disconnected(_) => PersistenceError::OwnerStopped,
        })
    }

    pub(super) fn shutdown(&self) -> Result<(), PersistenceError> {
        let mut shutdown = self.shutdown.lock().unwrap();
        if let Some(result) = shutdown.clone() {
            return result;
        }

        // Hold admission while transitioning and dropping the sender. submit()
        // holds the same lock through try_send(), so no accepted command is
        // lost and no command can be accepted after shutdown begins.
        {
            let mut admission = self.admission.lock().unwrap();
            admission.state = STATE_CLOSING;
            admission.sender.take();
        }
        let result = self
            .join
            .lock()
            .unwrap()
            .take()
            .map(|join| join.join().map_err(|_| PersistenceError::OwnerStopped))
            .unwrap_or(Ok(()));
        {
            let mut admission = self.admission.lock().unwrap();
            admission.state = STATE_CLOSED;
        }
        *shutdown = Some(result.clone());
        result
    }
}

impl Drop for OwnerLifecycle {
    fn drop(&mut self) {
        self.admission.get_mut().unwrap().sender.take();
        if let Some(join) = self.join.get_mut().unwrap().take() {
            let _ = join.join();
        }
    }
}
