//! File Location window wire shapes.

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileLocationsResponse {
    pub publication: String,
    pub parent: String,
    pub start: usize,
    pub limit: usize,
    pub total: usize,
    pub children: Vec<FolderChildWire>,
    /// Recursive Pending Candidate count for this Folder subtree.
    pub pending_files: usize,
    /// Whether at least one strict descendant Folder is Pending.
    pub has_pending_descendants: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderChildWire {
    pub location: String,
    pub name: String,
    pub photo_count: usize,
    pub has_descendant_folders: bool,
    /// Recursive Pending Candidate count for this child subtree.
    pub pending_files: usize,
    /// Whether at least one strict descendant Folder is Pending.
    pub has_pending_descendants: bool,
}
