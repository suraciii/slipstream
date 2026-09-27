use super::*;
/// The submitted target of one Album mutation, kept for any unknown-outcome
/// report after the request may have been admitted.
#[derive(Clone, Debug)]
pub(crate) struct MutationIdentity {
    pub(crate) operation: Operation,
    pub(crate) photo_ids: Vec<String>,
    pub(crate) album_id: Option<String>,
    pub(crate) album_name: Option<String>,
}
#[derive(Debug)]
pub(crate) struct PendingTrashReview {
    pub(crate) photo_ids: Vec<String>,
    pub(crate) exclude_photo_ids: Vec<String>,
}

/// Records the point where a mutation request was handed to the transport.
/// Any later timeout, interruption, or unusable response is an unknown
/// outcome instead of a claimed refusal.
#[derive(Debug, Default)]
pub(crate) struct AdmissionState {
    pub(crate) identity: std::sync::Mutex<Option<MutationIdentity>>,
}

#[derive(Debug, Default)]
pub(crate) struct PublicationState {
    pub(crate) committed: std::sync::Mutex<Option<(Value, &'static str)>>,
}

impl PublicationState {
    /// Records the published result with the file kind that committed, so
    /// the deadline and interruption paths name the right file even though
    /// they run outside the command's own error construction.
    pub(crate) fn record(&self, noun: &'static str, data: Value) {
        *self
            .committed
            .lock()
            .expect("publication state is lockable") = Some((data, noun));
    }

    pub(crate) fn committed(&self) -> Option<Value> {
        self.committed
            .lock()
            .expect("publication state is lockable")
            .as_ref()
            .map(|(data, _)| data.clone())
    }

    pub(crate) fn committed_noun(&self) -> &'static str {
        self.committed
            .lock()
            .expect("publication state is lockable")
            .as_ref()
            .map(|(_, noun)| *noun)
            .unwrap_or("output")
    }
}

impl AdmissionState {
    pub(crate) fn admit(&self, identity: MutationIdentity) {
        *self.identity.lock().expect("admission state is lockable") = Some(identity);
    }

    pub(crate) fn admitted(&self) -> Option<MutationIdentity> {
        self.identity
            .lock()
            .expect("admission state is lockable")
            .clone()
    }
}
