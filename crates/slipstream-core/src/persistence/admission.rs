use crate::confinement::LibraryRoot;
use std::{
    ffi::{CString, OsStr, OsString},
    fmt, fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::ffi::OsStrExt,
    },
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseName(OsString);

impl DatabaseName {
    pub fn parse(value: impl Into<OsString>) -> Result<Self, StateError> {
        let value = value.into();
        let path = Path::new(&value);
        let mut components = path.components();
        if !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
            || value.as_bytes().contains(&0)
        {
            return Err(StateError::InvalidDatabaseName);
        }
        Ok(Self(value))
    }

    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StateFileIdentity {
    device: u64,
    inode: u64,
    uid: u32,
    mode: u32,
    link_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StateError {
    InvalidStatePath,
    InvalidDatabaseName,
    UnderLibraryRoot,
    UnsafeStateDirectory,
    UnsafeDatabase,
    ChangedDatabase,
    UnsafeSidecar,
    SidecarPresent,
    Io(&'static str),
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidStatePath => "SQLite state path is invalid",
            Self::InvalidDatabaseName => "SQLite database name is invalid",
            Self::UnderLibraryRoot => {
                "SQLite database must be outside the read-only Photo Library root"
            }
            Self::UnsafeStateDirectory => "SQLite state directory is not safely owned",
            Self::UnsafeDatabase => "SQLite database inode is not safely owned",
            Self::ChangedDatabase => "SQLite database inode changed before startup",
            Self::UnsafeSidecar => "SQLite sidecar is not safely owned",
            Self::SidecarPresent => "SQLite sidecar requires operator recovery",
            Self::Io(message) => message,
        })
    }
}

impl std::error::Error for StateError {}

pub struct StateDirectory {
    descriptor: OwnedFd,
    canonical_path: PathBuf,
}

impl StateDirectory {
    pub fn open_or_create(
        library: &LibraryRoot,
        path: impl AsRef<Path>,
    ) -> Result<Self, StateError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(StateError::InvalidStatePath);
        }
        reject_under_library(library.canonical_path(), path)?;
        let descriptor = open_or_create_directory(library, path)?;
        let canonical_path = canonical_path_for_descriptor(descriptor.as_raw_fd())?;
        reject_under_library(library.canonical_path(), &canonical_path)?;
        let facts =
            sys::fstat(descriptor.as_raw_fd()).map_err(|_| StateError::UnsafeStateDirectory)?;
        if facts.st_mode & libc::S_IFMT != libc::S_IFDIR
            || facts.st_uid != effective_uid()
            || facts.st_mode & 0o022 != 0
        {
            return Err(StateError::UnsafeStateDirectory);
        }
        Ok(Self {
            descriptor,
            canonical_path,
        })
    }

    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub fn prepare_database(&self, name: &DatabaseName) -> Result<StateFileIdentity, StateError> {
        self.prepare_database_with_creation(name)
            .map(|(identity, _)| identity)
    }

    pub(crate) fn prepare_database_with_creation(
        &self,
        name: &DatabaseName,
    ) -> Result<(StateFileIdentity, bool), StateError> {
        // Recovery sidecars must be admitted before O_CREAT can materialize a
        // new database. Callers still re-check after opening for races.
        self.admit_sidecars(name)?;
        let existed = self.database_exists(name)?;
        let descriptor =
            self.open_database(name, libc::O_RDWR | libc::O_CREAT | libc::O_NONBLOCK, 0o600)?;
        let facts = sys::fstat(descriptor.as_raw_fd()).map_err(|_| StateError::UnsafeDatabase)?;
        let identity = safe_identity(&facts).ok_or(StateError::UnsafeDatabase)?;
        Ok((identity, !existed))
    }

    pub(crate) fn remove_created_empty_database(
        &self,
        name: &DatabaseName,
        expected: StateFileIdentity,
    ) -> Result<(), StateError> {
        let descriptor = self.open_database(name, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        let facts = sys::fstat(descriptor.as_raw_fd()).map_err(|_| StateError::ChangedDatabase)?;
        if safe_identity(&facts) != Some(expected) || facts.st_size != 0 {
            return Err(StateError::ChangedDatabase);
        }
        let name = CString::new(name.as_os_str().as_bytes())
            .map_err(|_| StateError::InvalidDatabaseName)?;
        sys::unlink_at(self.descriptor.as_raw_fd(), &name).map_err(|_| StateError::ChangedDatabase)
    }

    fn database_exists(&self, name: &DatabaseName) -> Result<bool, StateError> {
        let name = CString::new(name.as_os_str().as_bytes())
            .map_err(|_| StateError::InvalidDatabaseName)?;
        match sys::open_at(
            self.descriptor.as_raw_fd(),
            &name,
            libc::O_RDONLY | libc::O_NONBLOCK,
            0,
        ) {
            Ok(_) => Ok(true),
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(false),
            Err(_) => Err(StateError::UnsafeDatabase),
        }
    }

    pub fn verify_database(
        &self,
        name: &DatabaseName,
        expected: StateFileIdentity,
    ) -> Result<(), StateError> {
        let descriptor = self.open_database(name, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        let facts = sys::fstat(descriptor.as_raw_fd()).map_err(|_| StateError::ChangedDatabase)?;
        if safe_identity(&facts) != Some(expected) {
            return Err(StateError::ChangedDatabase);
        }
        Ok(())
    }

    pub fn admit_sidecars(&self, name: &DatabaseName) -> Result<(), StateError> {
        if self.inspect_sidecars(name)? {
            return Err(StateError::SidecarPresent);
        }
        Ok(())
    }

    pub(crate) fn startup_sidecars_present(&self, name: &DatabaseName) -> Result<bool, StateError> {
        self.inspect_sidecars(name)
    }

    fn inspect_sidecars(&self, name: &DatabaseName) -> Result<bool, StateError> {
        let mut present = false;
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut sidecar = name.as_os_str().as_bytes().to_vec();
            sidecar.extend_from_slice(suffix.as_bytes());
            let sidecar = CString::new(sidecar).map_err(|_| StateError::InvalidDatabaseName)?;
            match sys::open_at(
                self.descriptor.as_raw_fd(),
                &sidecar,
                libc::O_RDONLY | libc::O_NONBLOCK,
                0,
            ) {
                Ok(descriptor) => {
                    let facts = sys::fstat(descriptor.as_raw_fd())
                        .map_err(|_| StateError::UnsafeSidecar)?;
                    if safe_identity(&facts).is_none() {
                        return Err(StateError::UnsafeSidecar);
                    }
                    present = true;
                }
                Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {}
                Err(_) => return Err(StateError::UnsafeSidecar),
            }
        }
        Ok(present)
    }

    pub fn sqlite_path(&self, name: &DatabaseName) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.descriptor.as_raw_fd()))
            .join(name.as_os_str())
    }

    pub fn sqlite_immutable_uri(&self, name: &DatabaseName) -> String {
        let path = self.sqlite_path(name);
        let mut uri = String::from("file:");
        for byte in path.as_os_str().as_bytes() {
            if *byte == b'/'
                || byte.is_ascii_alphanumeric()
                || matches!(*byte, b'-' | b'_' | b'.' | b'~')
            {
                uri.push(*byte as char);
            } else {
                uri.push('%');
                uri.push(char::from(b"0123456789ABCDEF"[(*byte >> 4) as usize]));
                uri.push(char::from(b"0123456789ABCDEF"[(*byte & 0x0f) as usize]));
            }
        }
        uri.push_str("?mode=ro&immutable=1");
        uri
    }

    fn open_database(
        &self,
        name: &DatabaseName,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> Result<OwnedFd, StateError> {
        let name = CString::new(name.as_os_str().as_bytes())
            .map_err(|_| StateError::InvalidDatabaseName)?;
        sys::open_at(self.descriptor.as_raw_fd(), &name, flags, mode)
            .map_err(|_| StateError::UnsafeDatabase)
    }
}

fn reject_under_library(library: &Path, candidate: &Path) -> Result<(), StateError> {
    if candidate == library || candidate.starts_with(library) {
        Err(StateError::UnderLibraryRoot)
    } else {
        Ok(())
    }
}

fn effective_uid() -> libc::uid_t {
    // SAFETY: `geteuid` has no preconditions.
    unsafe { libc::geteuid() }
}

fn safe_identity(facts: &libc::stat) -> Option<StateFileIdentity> {
    (facts.st_mode & libc::S_IFMT == libc::S_IFREG
        && facts.st_uid == effective_uid()
        && facts.st_mode & 0o022 == 0
        && facts.st_nlink == 1)
        .then_some(StateFileIdentity {
            device: facts.st_dev,
            inode: facts.st_ino,
            uid: facts.st_uid,
            mode: facts.st_mode,
            link_count: facts.st_nlink,
        })
}

fn open_or_create_directory(library: &LibraryRoot, path: &Path) -> Result<OwnedFd, StateError> {
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::RootDir => None,
            Component::Normal(value) => Some(value.to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if components.is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(StateError::InvalidStatePath);
    }

    let mut current =
        open_directory(Path::new("/")).map_err(|_| StateError::UnsafeStateDirectory)?;
    for component in components {
        let name = CString::new(component.as_bytes()).map_err(|_| StateError::InvalidStatePath)?;
        let next = match sys::open_at(
            current.as_raw_fd(),
            &name,
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        ) {
            Ok(next) => next,
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                let parent = canonical_path_for_descriptor(current.as_raw_fd())
                    .map_err(|_| StateError::UnsafeStateDirectory)?;
                reject_under_library(library.canonical_path(), &parent)?;
                sys::mkdir_at(current.as_raw_fd(), &name, 0o700)
                    .map_err(|_| StateError::Io("SQLite state directory could not be created"))?;
                sys::open_at(
                    current.as_raw_fd(),
                    &name,
                    libc::O_RDONLY | libc::O_DIRECTORY,
                    0,
                )
                .map_err(|_| StateError::UnsafeStateDirectory)?
            }
            Err(_) => return Err(StateError::UnsafeStateDirectory),
        };
        let child_path = canonical_path_for_descriptor(next.as_raw_fd())
            .map_err(|_| StateError::UnsafeStateDirectory)?;
        reject_under_library(library.canonical_path(), &child_path)?;
        current = next;
    }
    Ok(current)
}

fn canonical_path_for_descriptor(fd: RawFd) -> Result<PathBuf, StateError> {
    fs::canonicalize(format!("/proc/self/fd/{fd}")).map_err(|_| StateError::UnsafeStateDirectory)
}

fn open_directory(path: &Path) -> io::Result<OwnedFd> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: `path` is NUL-terminated and successful open returns unique ownership.
    let descriptor = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    sys::owned(descriptor)
}

mod sys {
    use super::*;
    use std::mem::MaybeUninit;

    pub fn open_at(
        root: RawFd,
        name: &CString,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> io::Result<OwnedFd> {
        // SAFETY: `name` is NUL-terminated and successful open returns unique ownership.
        owned(unsafe {
            libc::openat(
                root,
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                mode,
            )
        })
    }

    pub fn fstat(fd: RawFd) -> io::Result<libc::stat> {
        let mut value = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `value` is writable and initialized on success.
        if unsafe { libc::fstat(fd, value.as_mut_ptr()) } == 0 {
            // SAFETY: successful `fstat` initialized the value.
            Ok(unsafe { value.assume_init() })
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn mkdir_at(root: RawFd, name: &CString, mode: libc::mode_t) -> io::Result<()> {
        // SAFETY: `name` is NUL-terminated and relative to the retained directory descriptor.
        if unsafe { libc::mkdirat(root, name.as_ptr(), mode) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn unlink_at(root: RawFd, name: &CString) -> io::Result<()> {
        // SAFETY: `name` is NUL-terminated and relative to the retained directory descriptor.
        if unsafe { libc::unlinkat(root, name.as_ptr(), 0) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn owned(descriptor: libc::c_int) -> io::Result<OwnedFd> {
        if descriptor < 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: the descriptor is newly opened and uniquely owned.
            Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::{
        fs::{self, File},
        os::unix::{ffi::OsStrExt, fs::PermissionsExt},
        path::PathBuf,
        sync::{
            atomic::{AtomicU64, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    static NEXT_TEMP_TREE: AtomicU64 = AtomicU64::new(0);

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            loop {
                let nonce = NEXT_TEMP_TREE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("slipstream-state-{}-{nonce}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("temporary state fixture could not be created: {error}"),
                }
            }
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rejects_state_under_library_before_creating_it() {
        let base = TempTree::new();
        let library_path = base.0.join("originals");
        fs::create_dir(&library_path).unwrap();
        let library = LibraryRoot::open(&library_path).unwrap();
        let state = library_path.join("missing/state");
        assert!(matches!(
            StateDirectory::open_or_create(&library, &state),
            Err(StateError::UnderLibraryRoot)
        ));
        assert!(!library_path.join("missing").exists());
    }

    #[test]
    fn rejects_an_intermediate_symlink_without_creating_inside_the_library() {
        let base = TempTree::new();
        let library_path = base.0.join("originals");
        let outside = base.0.join("outside");
        fs::create_dir(&library_path).unwrap();
        fs::create_dir(&outside).unwrap();
        let link = base.0.join("state-parent");
        symlink(&library_path, &link).unwrap();
        let library = LibraryRoot::open(&library_path).unwrap();
        let state = link.join("created").join("state");
        assert!(matches!(
            StateDirectory::open_or_create(&library, &state),
            Err(StateError::UnsafeStateDirectory)
        ));
        assert!(!library_path.join("created").exists());
        assert!(!outside.join("created").exists());
    }

    #[test]
    fn creates_missing_components_from_retained_ancestors_after_path_replacement() {
        let base = TempTree::new();
        let library_path = base.0.join("originals");
        let state_parent = base.0.join("state-parent");
        let replacement = base.0.join("replacement");
        fs::create_dir(&library_path).unwrap();
        fs::create_dir(&state_parent).unwrap();
        fs::create_dir(&replacement).unwrap();
        let library = LibraryRoot::open(&library_path).unwrap();
        let state = state_parent.join("nested").join("state");
        let retained_parent = open_directory(&state_parent).unwrap();
        fs::rename(&state_parent, replacement.join("retained")).unwrap();
        symlink(&replacement, &state_parent).unwrap();
        let name = CString::new("nested").unwrap();
        sys::mkdir_at(retained_parent.as_raw_fd(), &name, 0o700).unwrap();
        assert!(replacement.join("retained/nested").is_dir());
        assert!(!state_parent.join("nested").exists());
        drop(retained_parent);
        assert!(StateDirectory::open_or_create(&library, &state).is_err());
    }

    #[test]
    fn prepares_reverifies_and_admits_safe_files() {
        let base = TempTree::new();
        let library_path = base.0.join("originals");
        let state_path = base.0.join("state");
        fs::create_dir(&library_path).unwrap();
        fs::create_dir(&state_path).unwrap();
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o700)).unwrap();
        let library = LibraryRoot::open(&library_path).unwrap();
        let state = StateDirectory::open_or_create(&library, &state_path).unwrap();
        let name = DatabaseName::parse("library.sqlite").unwrap();
        let identity = state.prepare_database(&name).unwrap();
        state.verify_database(&name, identity).unwrap();
        File::create(state_path.join("library.sqlite-journal")).unwrap();
        fs::set_permissions(
            state_path.join("library.sqlite-journal"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert_eq!(state.admit_sidecars(&name), Err(StateError::SidecarPresent));
    }

    #[test]
    fn rejects_present_sidecar_before_creating_missing_database() {
        let base = TempTree::new();
        let library_path = base.0.join("originals");
        let state_path = base.0.join("state");
        fs::create_dir(&library_path).unwrap();
        fs::create_dir(&state_path).unwrap();
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o700)).unwrap();
        let library = LibraryRoot::open(&library_path).unwrap();
        let state = StateDirectory::open_or_create(&library, &state_path).unwrap();
        let name = DatabaseName::parse("library.sqlite").unwrap();
        let sidecar = state_path.join("library.sqlite-journal");
        fs::write(&sidecar, b"operator recovery data").unwrap();
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(state.startup_sidecars_present(&name).unwrap());
        assert_eq!(state.admit_sidecars(&name), Err(StateError::SidecarPresent));
        assert_eq!(
            state.prepare_database(&name),
            Err(StateError::SidecarPresent)
        );
        assert!(!state_path.join("library.sqlite").exists());
        assert_eq!(fs::read(sidecar).unwrap(), b"operator recovery data");
    }

    #[test]
    fn rejects_database_and_sidecar_links_and_unsafe_state_mode() {
        let base = TempTree::new();
        let library_path = base.0.join("originals");
        let state_path = base.0.join("state");
        fs::create_dir(&library_path).unwrap();
        fs::create_dir(&state_path).unwrap();
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o700)).unwrap();
        let target = base.0.join("target");
        fs::write(&target, b"unchanged").unwrap();
        let library = LibraryRoot::open(&library_path).unwrap();
        let state = StateDirectory::open_or_create(&library, &state_path).unwrap();
        let name = DatabaseName::parse("library.sqlite").unwrap();

        fs::hard_link(&target, state_path.join("library.sqlite")).unwrap();
        assert!(state.prepare_database(&name).is_err());
        fs::remove_file(state_path.join("library.sqlite")).unwrap();
        symlink(&target, state_path.join("library.sqlite")).unwrap();
        assert!(state.prepare_database(&name).is_err());
        fs::remove_file(state_path.join("library.sqlite")).unwrap();

        state.prepare_database(&name).unwrap();
        symlink(&target, state_path.join("library.sqlite-wal")).unwrap();
        assert!(matches!(
            state.admit_sidecars(&name),
            Err(StateError::UnsafeSidecar)
        ));
        assert_eq!(fs::read(&target).unwrap(), b"unchanged");

        let unsafe_path = base.0.join("unsafe");
        fs::create_dir(&unsafe_path).unwrap();
        fs::set_permissions(&unsafe_path, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(matches!(
            StateDirectory::open_or_create(&library, &unsafe_path),
            Err(StateError::UnsafeStateDirectory)
        ));
    }

    #[test]
    fn rejects_fifo_database_and_sidecar_without_blocking() {
        let base = TempTree::new();
        let library_path = base.0.join("originals");
        let state_path = base.0.join("state");
        fs::create_dir(&library_path).unwrap();
        fs::create_dir(&state_path).unwrap();
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o700)).unwrap();
        let library = LibraryRoot::open(&library_path).unwrap();
        let state = StateDirectory::open_or_create(&library, &state_path).unwrap();
        let name = DatabaseName::parse("library.sqlite").unwrap();

        let database =
            CString::new(state_path.join("library.sqlite").as_os_str().as_bytes()).unwrap();
        // SAFETY: `database` is NUL-terminated and the path does not exist.
        assert_eq!(unsafe { libc::mkfifo(database.as_ptr(), 0o600) }, 0);
        let (send, receive) = mpsc::channel();
        thread::spawn(move || send.send(state.prepare_database(&name)).unwrap());
        assert!(
            receive
                .recv_timeout(Duration::from_secs(1))
                .expect("FIFO database admission must not block")
                .is_err()
        );

        fs::remove_file(state_path.join("library.sqlite")).unwrap();
        let library = LibraryRoot::open(&library_path).unwrap();
        let state = StateDirectory::open_or_create(&library, &state_path).unwrap();
        let name = DatabaseName::parse("library.sqlite").unwrap();
        state.prepare_database(&name).unwrap();
        let sidecar = CString::new(
            state_path
                .join("library.sqlite-journal")
                .as_os_str()
                .as_bytes(),
        )
        .unwrap();
        // SAFETY: `sidecar` is NUL-terminated and the path does not exist.
        assert_eq!(unsafe { libc::mkfifo(sidecar.as_ptr(), 0o600) }, 0);
        let (send, receive) = mpsc::channel();
        thread::spawn(move || send.send(state.admit_sidecars(&name)).unwrap());
        assert!(matches!(
            receive
                .recv_timeout(Duration::from_secs(1))
                .expect("FIFO sidecar admission must not block"),
            Err(StateError::UnsafeSidecar)
        ));
    }

    #[test]
    fn retained_state_descriptor_survives_path_replacement_and_reverify_detects_inode_swap() {
        let base = TempTree::new();
        let library_path = base.0.join("originals");
        let state_path = base.0.join("state");
        let moved = base.0.join("state-moved");
        fs::create_dir(&library_path).unwrap();
        fs::create_dir(&state_path).unwrap();
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o700)).unwrap();
        let library = LibraryRoot::open(&library_path).unwrap();
        let state = StateDirectory::open_or_create(&library, &state_path).unwrap();
        let name = DatabaseName::parse("library.sqlite").unwrap();
        let identity = state.prepare_database(&name).unwrap();
        fs::rename(&state_path, &moved).unwrap();
        symlink(&library_path, &state_path).unwrap();
        state.verify_database(&name, identity).unwrap();
        assert!(moved.join("library.sqlite").is_file());
        assert!(!library_path.join("library.sqlite").exists());

        // Keep the admitted inode linked so the filesystem cannot reuse its
        // number for the replacement and make this identity test nondeterministic.
        fs::rename(
            moved.join("library.sqlite"),
            moved.join("library.sqlite-admitted"),
        )
        .unwrap();
        fs::write(moved.join("library.sqlite"), b"replacement").unwrap();
        fs::set_permissions(
            moved.join("library.sqlite"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(matches!(
            state.verify_database(&name, identity),
            Err(StateError::ChangedDatabase)
        ));
    }
}
