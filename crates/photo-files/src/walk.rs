//! Bounded directory traversal shared by workspace tools.
//!
//! Traversal mechanics live here and nothing else: the depth bound, the
//! per-directory entry bound, the relative path reported for every entry, and
//! the order in which entries are entered, descended, and left. Dot-file,
//! symlink, special-file, counting, hashing, error, and acceptance policies
//! stay with each caller.

use crate::{Directory, Entry};
use std::{io, path::Path};

/// Deepest directory nesting a traversal accepts before it refuses.
pub const MAXIMUM_DEPTH: usize = 64;

/// Bounds applied before any caller policy runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WalkLimits {
    maximum_depth: usize,
    maximum_entries: usize,
}

impl WalkLimits {
    /// `maximum_entries` bounds how many entries one directory may list.
    pub const fn new(maximum_entries: usize) -> Self {
        Self {
            maximum_depth: MAXIMUM_DEPTH,
            maximum_entries,
        }
    }
}

/// One visited entry, addressed relative to the traversal root.
#[derive(Clone, Copy, Debug)]
pub struct Reported<'a> {
    /// Directory that listed the entry. Descriptor-relative work starts here.
    pub directory: &'a Directory,
    /// Recursion depth of `directory`; the traversal root is zero.
    pub depth: usize,
    /// `prefix` joined with the entry name, for display and caller policy.
    pub path: &'a Path,
    pub entry: &'a Entry,
}

/// Walk `directory` and its descendants in listing order.
///
/// `prefix` only labels reported paths; it never authorizes a filesystem
/// operation. `exceeded_depth` builds the caller's refusal for nesting past
/// [`MAXIMUM_DEPTH`]. `enter` decides every entry: returning `Some(child)`
/// descends into `child` and then calls `leave`, while `None` skips the entry.
/// Callers open the child themselves, so they keep the descriptor they need
/// and never open an entry they chose to skip.
pub fn walk<E>(
    directory: &Directory,
    prefix: &Path,
    limits: WalkLimits,
    exceeded_depth: impl Fn() -> E,
    mut enter: impl FnMut(Reported<'_>) -> Result<Option<Directory>, E>,
    mut leave: impl FnMut(Reported<'_>) -> Result<(), E>,
) -> Result<(), E>
where
    E: From<io::Error>,
{
    descend(
        directory,
        prefix,
        0,
        limits,
        &exceeded_depth,
        &mut enter,
        &mut leave,
    )
}

fn descend<E>(
    directory: &Directory,
    prefix: &Path,
    depth: usize,
    limits: WalkLimits,
    exceeded_depth: &impl Fn() -> E,
    enter: &mut impl FnMut(Reported<'_>) -> Result<Option<Directory>, E>,
    leave: &mut impl FnMut(Reported<'_>) -> Result<(), E>,
) -> Result<(), E>
where
    E: From<io::Error>,
{
    if depth > limits.maximum_depth {
        return Err(exceeded_depth());
    }
    for entry in directory.entries(limits.maximum_entries)? {
        let path = prefix.join(&entry.name);
        let reported = Reported {
            directory,
            depth,
            path: &path,
            entry: &entry,
        };
        if let Some(child) = enter(reported)? {
            descend(
                &child,
                &path,
                depth + 1,
                limits,
                exceeded_depth,
                enter,
                leave,
            )?;
            leave(reported)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntryKind;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "slipstream-photo-walk-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn chain(root: &Directory, names: &[&str]) {
        let mut current = root.try_clone().unwrap();
        for name in names {
            current = current.create_directory(name).unwrap();
        }
    }

    fn directories(reported: Reported<'_>) -> Result<Option<Directory>, io::Error> {
        Ok((reported.entry.kind == EntryKind::Directory)
            .then(|| reported.directory.child(&reported.entry.name))
            .transpose()?)
    }

    #[test]
    fn refuses_nesting_past_the_depth_bound_with_the_callers_error() {
        let path = temp_dir("depth");
        let root = Directory::open(&path).unwrap();
        chain(&root, &["a", "b", "c"]);
        let mut visited = Vec::new();
        let error = walk(
            &root,
            Path::new(""),
            WalkLimits {
                maximum_depth: 2,
                maximum_entries: 64,
            },
            || io::Error::other("nesting exceeds the traversal bound"),
            |reported| {
                visited.push(reported.path.display().to_string());
                directories(reported)
            },
            |_| Ok(()),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "nesting exceeds the traversal bound");
        assert_eq!(visited, ["a", "a/b", "a/b/c"]);
        std::fs::remove_dir_all(&path).unwrap();
    }

    #[test]
    fn refuses_a_directory_that_lists_past_the_entry_bound() {
        let path = temp_dir("entries");
        let root = Directory::open(&path).unwrap();
        for name in ["a", "b", "c"] {
            root.create_file(name, 0o600).unwrap();
        }
        let error = walk(
            &root,
            Path::new(""),
            WalkLimits::new(2),
            || io::Error::other("unreachable"),
            |reported| {
                assert_eq!(reported.depth, 0);
                directories(reported)
            },
            |_| Ok(()),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "directory exceeds entry limit");
        std::fs::remove_dir_all(&path).unwrap();
    }

    #[test]
    fn enters_before_a_descent_and_leaves_after_it() {
        let path = temp_dir("order");
        let root = Directory::open(&path).unwrap();
        chain(&root, &["a"]);
        root.child("a").unwrap().create_file("b", 0o600).unwrap();
        let mut entered = Vec::new();
        let mut left = Vec::new();
        walk(
            &root,
            Path::new(""),
            WalkLimits::new(64),
            || io::Error::other("unreachable"),
            |reported| {
                entered.push(format!("enter {}", reported.path.display()));
                directories(reported)
            },
            |reported| {
                left.push(format!("leave {}", reported.path.display()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(entered, ["enter a", "enter a/b"]);
        assert_eq!(left, ["leave a"]);
        std::fs::remove_dir_all(&path).unwrap();
    }

    #[test]
    fn reports_paths_under_the_given_prefix() {
        let path = temp_dir("prefix");
        let root = Directory::open(&path).unwrap();
        chain(&root, &["d"]);
        root.child("d").unwrap().create_file("b", 0o600).unwrap();
        root.create_file("a", 0o600).unwrap();
        let mut reported = Vec::new();
        walk(
            &root,
            Path::new("root-1"),
            WalkLimits::new(64),
            || io::Error::other("unreachable"),
            |entry| {
                reported.push(entry.path.display().to_string());
                directories(entry)
            },
            |_| Ok(()),
        )
        .unwrap();
        reported.sort();
        assert_eq!(reported, ["root-1/a", "root-1/d", "root-1/d/b"]);
        std::fs::remove_dir_all(&path).unwrap();
    }

    #[test]
    fn propagates_caller_errors_and_stops_the_walk() {
        let path = temp_dir("errors");
        let root = Directory::open(&path).unwrap();
        chain(&root, &["a"]);
        root.child("a").unwrap().create_file("b", 0o600).unwrap();
        let mut left = Vec::new();
        let enter = walk(
            &root,
            Path::new(""),
            WalkLimits::new(64),
            || io::Error::other("unreachable"),
            |_| Err(io::Error::other("enter refused")),
            |reported| {
                left.push(reported.path.display().to_string());
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(enter.to_string(), "enter refused");
        assert!(left.is_empty());
        let leave = walk(
            &root,
            Path::new(""),
            WalkLimits::new(64),
            || io::Error::other("unreachable"),
            directories,
            |reported| {
                assert_eq!(reported.path, Path::new("a"));
                Err(io::Error::other("leave refused"))
            },
        )
        .unwrap_err();
        assert_eq!(leave.to_string(), "leave refused");
        std::fs::remove_dir_all(&path).unwrap();
    }

    #[test]
    fn never_descends_into_an_entry_the_caller_skips() {
        let path = temp_dir("skip");
        let root = Directory::open(&path).unwrap();
        chain(&root, &[".hidden"]);
        root.child(".hidden")
            .unwrap()
            .create_file("secret", 0o600)
            .unwrap();
        root.create_file("visible", 0o600).unwrap();
        let mut visited = Vec::new();
        walk(
            &root,
            Path::new(""),
            WalkLimits::new(64),
            || io::Error::other("unreachable"),
            |reported| {
                visited.push(reported.path.display().to_string());
                if reported.entry.name.starts_with('.') {
                    return Ok(None);
                }
                directories(reported)
            },
            |_| Ok(()),
        )
        .unwrap();
        visited.sort();
        assert_eq!(visited, [".hidden", "visible"]);
        std::fs::remove_dir_all(&path).unwrap();
    }
}
