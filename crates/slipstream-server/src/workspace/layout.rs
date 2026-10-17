//! The workspace-owned managed layout: canonical directory names and safe
//! relative paths inside one Photo Workspace.
//!
//! The managed directories express roles and protection, not a protocol for
//! external tools: `input/` holds protected snapshots, `previews/` and
//! `outputs/` hold completed files, `.edits/` holds resumable tool state, and
//! `.tmp/` holds incomplete work. Every managed relative path in the index
//! takes the shape `<managed directory>/<name>` and is confined to the
//! workspace directory by the same single-component check the underlying
//! descriptor operations enforce.

use std::path::{Component, Path};

/// The managed directory holding protected input snapshots.
pub(crate) const INPUT: &str = "input";
/// The managed directory holding completed Edit Previews.
pub(crate) const PREVIEWS: &str = "previews";
/// The managed directory holding completed outputs.
pub(crate) const OUTPUTS: &str = "outputs";
/// The tool-private state directory, excluded from artifact discovery.
pub(crate) const EDITS: &str = ".edits";

/// The managed directory holding a completed file of the given kind.
pub(crate) fn completed_directory(preview: bool) -> &'static str {
    if preview { PREVIEWS } else { OUTPUTS }
}

/// The first component of an indexed relative path, for role classification.
pub(crate) fn component(path: &str) -> Option<&str> {
    Path::new(path)
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
}

/// Whether the indexed path names a completed file in a managed output
/// directory. Both directories are published by this workspace only through
/// checked no-replace publication.
pub(crate) fn in_completed_directory(path: &str) -> bool {
    matches!(component(path), Some(PREVIEWS | OUTPUTS))
}

/// A single-component filename beneath a managed directory. Managed paths
/// never nest and never leave their directory, so the index stores
/// `<managed directory>/<name>` only.
pub(crate) fn managed_file(directory: &str, name: &str) -> Option<String> {
    let name = Path::new(name);
    let mut components = name.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => {
            Some(format!("{directory}/{}", name.as_os_str().to_string_lossy()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_paths_are_one_directory_and_one_name() {
        assert_eq!(managed_file(INPUT, "DSC01234.ARW").as_deref(), Some("input/DSC01234.ARW"));
        assert_eq!(managed_file(OUTPUTS, "a.tif").as_deref(), Some("outputs/a.tif"));
        assert_eq!(managed_file(PREVIEWS, "a.png").as_deref(), Some("previews/a.png"));
        assert!(managed_file(INPUT, "nested/a.raw").is_none());
        assert!(managed_file(INPUT, "../escape").is_none());
        assert!(managed_file(INPUT, "a/b").is_none());
        assert!(managed_file(INPUT, "input/../escape").is_none());
        assert!(managed_file(INPUT, "").is_none());
        assert!(managed_file(INPUT, ".").is_none());
    }

    #[test]
    fn completed_roles_cover_only_the_published_directories() {
        assert!(in_completed_directory("outputs/a.tif"));
        assert!(in_completed_directory("previews/a.png"));
        assert!(!in_completed_directory("input/a.raw"));
        assert!(!in_completed_directory("outputs.tif"));
        assert!(!in_completed_directory("outputs../escape"));
        assert!(!in_completed_directory(""));
        assert_eq!(completed_directory(true), PREVIEWS);
        assert_eq!(completed_directory(false), OUTPUTS);
        assert_eq!(component("outputs/a.tif"), Some(OUTPUTS));
        assert_eq!(component("a.tif"), Some("a.tif"));
        assert_eq!(component(""), None);
    }
}
