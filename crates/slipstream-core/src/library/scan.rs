//! Library scan admission with one verification scope.

use super::*;

impl Library {
    pub async fn scan(&self) -> Result<ScanSnapshot, LibraryError> {
        self.scan_with_options(crate::recovery::RecoveryOptions::full())
            .await
    }

    /// Runs one scan with an explicit verification scope and read budget.
    /// A request for different options while a scan is in flight reports
    /// `ScanBusy`; matching options join the active cycle.
    pub async fn scan_with_options(
        &self,
        options: crate::recovery::RecoveryOptions,
    ) -> Result<ScanSnapshot, LibraryError> {
        let (reply, receive) = tokio::sync::oneshot::channel();
        {
            let _admission = self.admit()?;
            let (lock, _) = &*self.scanner.state;
            let mut state = lock.lock().unwrap();
            if !state.open {
                return Err(LibraryError::Closed);
            }
            if state.in_flight.is_some() && state.options.as_ref() != Some(&options) {
                return Err(LibraryError::ScanBusy);
            }
            let first = state.in_flight.is_none();
            let waiters = state.in_flight.get_or_insert_with(Vec::new);
            // Receiver cancellation releases waiter capacity without
            // cancelling the physical scanner operation.
            waiters.retain(|waiter| !waiter.is_closed());
            if waiters.len() >= MAX_SCAN_WAITERS {
                return Err(LibraryError::ScanBusy);
            }
            waiters.push(reply);
            #[cfg(test)]
            scanner_admitted(&self.root);
            if first {
                state.options = Some(options.clone());
                if self
                    .scanner
                    .sender
                    .try_send(ScanCommand::Scan(options))
                    .is_err()
                {
                    state.in_flight.take();
                    state.options = None;
                    return Err(LibraryError::ScanBusy);
                }
            }
        }
        receive
            .await
            .unwrap_or(Err(LibraryError::ScannerStopped))
            .map(|snapshot| (*snapshot).clone())
    }

    /// The verification scope of the active scan, if it is a Folder Check.
    pub fn scan_scope(&self) -> Option<String> {
        let (lock, _) = &*self.scanner.state;
        let state = lock.lock().unwrap();
        state
            .options
            .as_ref()
            .and_then(|options| options.verification_scope.clone())
    }
}
