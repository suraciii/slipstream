//! Development Proxy task lifecycle: pending-build claims, tracked task
//! spawning, and the shutdown cancel-and-drain ordering.

use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio::sync::Mutex as AsyncMutex;

use crate::preview_render::PreviewCancellation;

/// A pending build's identity, claim generation, and cancellation token.
///
/// Claims compare by identity and generation: the cancellation token is
/// coordination state, not identity.
#[derive(Clone)]
pub(super) struct BuildClaim {
    pub(super) identity: String,
    pub(super) generation: u64,
    pub(super) cancellation: PreviewCancellation,
}

impl PartialEq for BuildClaim {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity && self.generation == other.generation
    }
}

impl Eq for BuildClaim {}

/// Tracked Development Proxy tasks.
///
/// Admission closes once shutdown begins. Draining cancels every outstanding
/// build claim, then awaits the tracked handles in registration order.
#[derive(Clone)]
pub(super) struct ProxyTasks {
    state: Arc<Mutex<ProxyTaskState>>,
    next_generation: Arc<AtomicU64>,
}

struct ProxyTaskState {
    closing: bool,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl ProxyTasks {
    pub(super) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ProxyTaskState {
                closing: false,
                handles: Vec::new(),
            })),
            next_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Whether shutdown has begun and task admission is closed.
    pub(super) fn is_closing(&self) -> bool {
        self.lock().closing
    }

    /// Issues the next claim for `identity` with a fresh cancellation token.
    pub(super) fn claim(&self, identity: String) -> BuildClaim {
        BuildClaim {
            identity,
            generation: self.next_generation.fetch_add(1, Ordering::Relaxed),
            cancellation: PreviewCancellation::new(),
        }
    }

    pub(super) fn spawn<F>(&self, future: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut tasks = self.lock();
        if tasks.closing {
            return false;
        }
        tasks.handles.retain(|handle| !handle.is_finished());
        tasks.handles.push(tokio::spawn(future));
        true
    }

    pub(super) fn begin_shutdown(&self) {
        self.lock().closing = true;
    }

    pub(super) async fn shutdown(&self, builds: &AsyncMutex<HashMap<String, BuildClaim>>) {
        self.begin_shutdown();
        let handles = {
            let mut tasks = self.lock();
            std::mem::take(&mut tasks.handles)
        };
        let cancellations = {
            let builds = builds.lock().await;
            builds
                .values()
                .map(|claim| claim.cancellation.clone())
                .collect::<Vec<_>>()
        };
        for cancellation in cancellations {
            cancellation.cancel();
        }
        for handle in handles {
            let _ = handle.await;
        }
    }

    fn lock(&self) -> MutexGuard<'_, ProxyTaskState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}
