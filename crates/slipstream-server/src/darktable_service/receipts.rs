//! File-backed request receipts for admitted work.
//!
//! One admitted request identity owns a single JSON document inside a
//! collection directory scoped by its photo. The document records the admitted
//! intent under `payload`; callers write their own progress fields beside it.
//! An exclusive directory lease serializes use of the identity, and the atomic
//! no-replace publication keeps a concurrent reader from observing a partial
//! document. Record shapes, byte bounds, and lease durations stay with the
//! caller that owns them.
use super::{Result, ServiceError};
use photo_files::{Directory, FileLease, LeaseMode, atomic_json, read_json};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

/// Bounded retained document for one request identity.
const RECORD_BYTES_MAX: usize = 1024 * 1024;
/// How long admission waits for the identity's lease.
const LEASE_TIMEOUT: Duration = Duration::from_secs(120);

/// Resolves the caller's request identity, generating one when it is omitted.
pub(crate) fn request_identity(arguments: &Value) -> Result<String> {
    Ok(arguments["requestId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or(photo_files::identity("request").map_err(ServiceError::io)?))
}

/// One request identity's receipt, held under its exclusive directory lease.
pub(crate) struct Receipt {
    directory: Directory,
    name: String,
    /// Retains the admission lease for this receipt's lifetime.
    _lease: FileLease,
}

impl Receipt {
    /// Opens the identity's photo-scoped collection directory and takes the
    /// exclusive admission lease that serializes use of the identity.
    pub(crate) fn open(
        root: &Directory,
        collection: &str,
        photo: &str,
        request: &str,
    ) -> Result<Self> {
        let directory = root
            .create_directory(collection)
            .and_then(|directory| directory.create_directory(photo))
            .map_err(ServiceError::io)?;
        let lease = directory
            .lease(LeaseMode::Exclusive, LEASE_TIMEOUT)
            .map_err(ServiceError::io)?;
        Ok(Self {
            directory,
            name: format!("{request}.json"),
            _lease: lease,
        })
    }

    /// Reads the retained record, refusing one whose admitted intent differs
    /// from `payload` with `conflict` as its message. `None` means the identity
    /// was never admitted.
    pub(crate) fn retained(&self, payload: &Value, conflict: &str) -> Result<Option<Value>> {
        let prior = match self.directory.open_file(&self.name) {
            Ok(file) => Some(read_json(file, RECORD_BYTES_MAX).map_err(ServiceError::io)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(ServiceError::io(error)),
        };
        if let Some(record) = &prior
            && record["payload"] != *payload
        {
            return Err(ServiceError::new("request_conflict", conflict, 409));
        }
        Ok(prior)
    }

    /// Replaces the identity's retained document with `record`.
    pub(crate) fn publish(&self, record: &impl Serialize) -> Result<()> {
        atomic_json(&self.directory, &self.name, record).map_err(ServiceError::io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn host(name: &str) -> Directory {
        Directory::create(std::env::temp_dir().join(photo_files::identity(name).unwrap())).unwrap()
    }

    #[test]
    fn an_unadmitted_identity_publishes_and_rereads_its_record() {
        let root = host("receipt-admit");
        let receipt = Receipt::open(&root, "collection", "photo-test", "request-1").unwrap();
        assert_eq!(
            receipt
                .retained(&json!({"intent": 1}), "Create request has different intent")
                .unwrap(),
            None
        );
        receipt
            .publish(&json!({"payload": {"intent": 1}, "result": {"outcome": "ok"}}))
            .unwrap();
        let retained = receipt
            .retained(&json!({"intent": 1}), "Create request has different intent")
            .unwrap()
            .expect("published record");
        assert_eq!(retained["result"], json!({"outcome": "ok"}));
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn a_changed_intent_under_one_identity_refuses_with_a_request_conflict() {
        let root = host("receipt-conflict");
        let receipt = Receipt::open(&root, "collection", "photo-test", "request-1").unwrap();
        receipt.publish(&json!({"payload": {"intent": 1}})).unwrap();
        let refusal = receipt
            .retained(&json!({"intent": 2}), "Create request has different intent")
            .unwrap_err();
        assert_eq!(
            refusal.value["error"]["code"].as_str(),
            Some("request_conflict")
        );
        assert_eq!(
            refusal.value["error"]["message"].as_str(),
            Some("Create request has different intent")
        );
        assert_eq!(refusal.status, 409);
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn an_omitted_request_identity_is_generated_once() {
        assert_eq!(
            request_identity(&json!({"requestId": "caller"})).unwrap(),
            "caller"
        );
        assert!(
            request_identity(&json!({}))
                .unwrap()
                .starts_with("request-")
        );
    }
}
