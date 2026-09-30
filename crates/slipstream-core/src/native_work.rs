//! Shared admission budget for native parsing, LibRaw, and libvips work.

use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicU64, Ordering},
};

const NATIVE_WORK_CAPACITY: usize = 2;

/// A Library creates one budget; standalone schedulers create their own.
#[derive(Clone)]
pub struct NativeWorkBudget(Arc<NativeWorkSemaphore>);

struct NativeWorkSemaphore {
    state: Mutex<NativeWorkState>,
    signal: Condvar,
    active: AtomicU64,
    peak: AtomicU64,
}

struct NativeWorkState {
    available: usize,
}

/// One opaque admission to a Library's shared native-work capacity.
/// Dropping the permit releases capacity only after the native work ends.
#[must_use = "dropping the permit releases native-work admission"]
pub struct NativeWorkPermit {
    semaphore: Arc<NativeWorkSemaphore>,
}

impl NativeWorkBudget {
    pub fn new() -> Self {
        Self(NativeWorkSemaphore::new(NATIVE_WORK_CAPACITY))
    }

    pub(crate) fn acquire(&self) -> NativeWorkPermit {
        self.0.acquire()
    }

    pub(crate) fn try_acquire(&self) -> Option<NativeWorkPermit> {
        self.0.try_acquire()
    }

    #[cfg(test)]
    pub(crate) fn peak(&self) -> u64 {
        self.0.peak.load(Ordering::Acquire)
    }
}

impl Default for NativeWorkBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeWorkSemaphore {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(NativeWorkState {
                available: capacity,
            }),
            signal: Condvar::new(),
            active: AtomicU64::new(0),
            peak: AtomicU64::new(0),
        })
    }

    fn acquire(self: &Arc<Self>) -> NativeWorkPermit {
        let mut state = self.state.lock().expect("native work semaphore poisoned");
        while state.available == 0 {
            state = self
                .signal
                .wait(state)
                .expect("native work semaphore poisoned");
        }
        self.admit(&mut state)
    }

    fn try_acquire(self: &Arc<Self>) -> Option<NativeWorkPermit> {
        let mut state = self.state.lock().expect("native work semaphore poisoned");
        (state.available != 0).then(|| self.admit(&mut state))
    }

    fn admit(self: &Arc<Self>, state: &mut NativeWorkState) -> NativeWorkPermit {
        state.available -= 1;
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.peak.fetch_max(active, Ordering::Relaxed);
        NativeWorkPermit {
            semaphore: Arc::clone(self),
        }
    }
}

impl Drop for NativeWorkPermit {
    fn drop(&mut self) {
        self.semaphore.active.fetch_sub(1, Ordering::AcqRel);
        let mut state = self
            .semaphore
            .state
            .lock()
            .expect("native work semaphore poisoned");
        state.available += 1;
        drop(state);
        self.semaphore.signal.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonblocking_admission_releases_capacity() {
        let budget = NativeWorkBudget::new();
        let first = budget.try_acquire().expect("first slot");
        let second = budget.try_acquire().expect("second slot");
        assert!(budget.try_acquire().is_none());
        drop(first);
        let replacement = budget.try_acquire().expect("released slot");
        assert!(budget.try_acquire().is_none());
        drop((second, replacement));
        assert!(budget.try_acquire().is_some());
        assert_eq!(budget.peak(), 2);
    }
}
