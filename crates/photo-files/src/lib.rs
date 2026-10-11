//! Linux file operations shared by independent photo tools and their hosts.
//! Directory descriptors, stable byte evidence, and no-replace publication are
//! independent of any photo catalog or application identity.
mod directory;
mod identity;
pub mod image;
mod json;
mod locking;
mod publication;

pub use directory::{Directory, Entry, EntryKind, ancestor_leases};
pub use identity::{Evidence, Observation, fingerprint, identity, timestamp};
pub use json::{canonical_json, read_json};
pub use locking::{FileLease, LeaseMode};
pub use publication::{PendingFile, atomic_json};
