//! Derived read-only File Location navigation for one Published Library.
//!
//! Original Folders are projections of known Original Locations, never
//! persisted entities. Every derivation here is computed from one immutable
//! [`Published`](crate::app::Published) snapshot, so all windows derived from
//! the same publication are coherent by construction.

use std::collections::HashMap;

/// Maximum direct-child Folder entries returned by one File Location window.
pub(crate) const MAXIMUM_FILE_LOCATION_WINDOW: usize = 60;
/// Maximum UTF-8 byte length of one relative Folder Location.
pub(crate) const MAXIMUM_FOLDER_LOCATION_BYTES: usize = 1024;
/// Maximum component count of one relative Folder Location.
pub(crate) const MAXIMUM_FOLDER_COMPONENTS: usize = 32;

/// One direct-child Folder summary in a bounded File Location window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FolderChild {
    pub location: String,
    pub name: String,
    pub photo_count: usize,
    pub has_descendant_folders: bool,
}

/// A Folder index derived from one Published Library generation.
pub(crate) struct FolderIndex {
    /// Recursive Photo count per Folder location.
    recursive_counts: HashMap<String, usize>,
    /// Sorted direct-child locations per parent location.
    children: HashMap<String, Vec<String>>,
}

impl FolderIndex {
    /// Derives the Folder index from Published ordering Original Locations.
    ///
    /// A Photo projects through its own Original File's Location, so every
    /// Photo counts once. Remembered unavailable Originals keep their last
    /// known Location and therefore keep their Folder projection. A removed
    /// Photo keeps its Folder represented but does not count: removal hides a
    /// Photo from Library Views without moving its Original File, so a Folder
    /// whose Photos were all removed still exists and now opens empty.
    pub(crate) fn derive(
        photos: &[slipstream_core::PhotoRecord],
        originals_by_id: &HashMap<String, usize>,
        originals: &[slipstream_core::OriginalRecord],
    ) -> Self {
        let mut direct_counts: HashMap<String, usize> = HashMap::new();
        let mut located: Vec<String> = Vec::new();
        for photo in photos {
            let ordering_id = photo.original_id.as_str();
            let ordering_id: &str = ordering_id;
            let Some(&position) = originals_by_id.get(ordering_id) else {
                continue;
            };
            let Some(parent) = parent_location(originals[position].relative_path.as_str()) else {
                continue;
            };
            located.push(parent.clone());
            if !photo.removed {
                *direct_counts.entry(parent).or_insert(0) += 1;
            }
        }
        // Intermediate ancestor Folders participate without direct Photos.
        // Located Folders repeat once per Photo, so they are deduplicated
        // before they become Folder identities.
        let mut known: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for location in located {
            if seen.insert(location.clone()) {
                known.push(location);
            }
        }
        for location in known.clone() {
            let mut ancestor = parent_location(&location);
            while let Some(current) = ancestor {
                if seen.insert(current.clone()) {
                    known.push(current.clone());
                }
                ancestor = parent_location(&current);
            }
        }
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        for location in known.iter() {
            let Some(parent) = parent_location(location) else {
                continue;
            };
            children.entry(parent).or_default().push(location.clone());
        }
        for child in children.values_mut() {
            child.sort();
        }
        // The root always participates, even for an empty Library.
        children.entry(String::new()).or_default();
        let mut recursive_counts: HashMap<String, usize> = HashMap::new();
        for (location, direct) in &direct_counts {
            recursive_counts.insert(location.clone(), *direct);
        }
        // Seed every known Folder, including generated intermediate
        // ancestors without direct Photos, so counts propagate through the
        // whole chain during post-order aggregation.
        for location in known.iter() {
            recursive_counts.entry(location.clone()).or_insert(0);
        }
        // Post-order aggregation of recursive counts from deepest Folders up.
        let mut pending: Vec<String> = recursive_counts
            .keys()
            .filter(|location| !location.is_empty())
            .cloned()
            .collect();
        pending.sort_by_key(|location| std::cmp::Reverse(location.len()));
        for location in pending {
            let count = recursive_counts[&location];
            if let Some(parent) = parent_location(&location) {
                *recursive_counts.entry(parent.to_owned()).or_insert(0) += count;
            }
        }
        Self {
            recursive_counts,
            children,
        }
    }

    /// Whether one Folder location is represented in this publication.
    pub(crate) fn is_known(&self, location: &str) -> bool {
        location.is_empty()
            || self.recursive_counts.contains_key(location)
            || self.children.contains_key(location)
    }

    /// One bounded window of direct children, ordered by location bytes.
    pub(crate) fn window(
        &self,
        parent: &str,
        start: usize,
        limit: usize,
    ) -> (Vec<FolderChild>, usize) {
        let empty = Vec::new();
        let children = self.children.get(parent).unwrap_or(&empty);
        let total = children.len();
        let window = children
            .iter()
            .skip(start)
            .take(limit)
            .map(|location| FolderChild {
                name: display_name(location),
                photo_count: self.recursive_counts.get(location).copied().unwrap_or(0),
                has_descendant_folders: self
                    .children
                    .get(location)
                    .is_some_and(|descendants| !descendants.is_empty()),
                location: location.clone(),
            })
            .collect();
        (window, total)
    }

    /// Filters Published Photo IDs by component-aware Folder ancestry.
    ///
    /// A Folder contains Photos projected to itself or to any descendant;
    /// prefix text alone never matches (`a` is not an ancestor of `ab`).
    pub(crate) fn filter_photo_ids(
        &self,
        photos: &[slipstream_core::PhotoRecord],
        originals_by_id: &HashMap<String, usize>,
        originals: &[slipstream_core::OriginalRecord],
        folder: &str,
    ) -> Vec<String> {
        let prefix = if folder.is_empty() {
            String::new()
        } else {
            format!("{folder}/")
        };
        let mut ids = Vec::new();
        for photo in photos {
            if photo.removed {
                continue;
            }
            let ordering_id = photo.original_id.as_str();
            let ordering_id: &str = ordering_id;
            let Some(&position) = originals_by_id.get(ordering_id) else {
                continue;
            };
            let Some(parent) = parent_location(originals[position].relative_path.as_str()) else {
                continue;
            };
            if parent == folder || parent.starts_with(&prefix) {
                ids.push(photo.id.clone());
            }
        }
        ids
    }
}

/// Validates one client-supplied relative Folder Location.
///
/// The empty value names the Library Folder root. Every other value must be
/// normalized relative UTF-8 components without `.`, `..`, empty components,
/// NUL bytes, absolute prefixes, or excessive depth.
pub(crate) fn valid_folder_location(location: &str) -> bool {
    if location.is_empty() {
        return true;
    }
    if location.len() > MAXIMUM_FOLDER_LOCATION_BYTES || location.starts_with('/') {
        return false;
    }
    let components = location.split('/').collect::<Vec<_>>();
    components.len() <= MAXIMUM_FOLDER_COMPONENTS
        && components.iter().all(|component| {
            !component.is_empty()
                && *component != "."
                && *component != ".."
                && !component.contains('\0')
        })
}

/// Parent Folder Location of one relative Original path. Root-level paths
/// project to the explicit empty root Folder Location.
fn parent_location(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    match path.rfind('/') {
        Some(position) if position > 0 => Some(path[..position].to_owned()),
        _ => Some(String::new()),
    }
}

/// Display name of one Folder Location: its final component.
fn display_name(location: &str) -> String {
    location.rsplit('/').next().unwrap_or(location).to_owned()
}
