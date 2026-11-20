//! The committed scan snapshot, pending Folder summaries, and retained changed Locations.

use crate::{OriginalRecord, OriginalScanError, PhotoRecord};

#[derive(Clone, Debug, PartialEq)]
pub struct ScanSnapshot {
    pub published: bool,
    pub originals: Vec<OriginalRecord>,
    pub photos: Vec<PhotoRecord>,
    pub errors: Vec<OriginalScanError>,
    pub pending_folders: Vec<PendingFolder>,
    /// Changed Locations whose prior facts remain protected until verified.
    pub pending_changed_paths: Vec<String>,
}

/// One Original Folder with discovered identity work that a Folder Check
/// still owns. Only pending Folders have records; an absent Folder is current.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PendingFolder {
    /// The relative Folder Location; the empty string is the Library Folder.
    pub relative_path: String,
    pub pending_files: usize,
    pub pending_bytes: u64,
}
