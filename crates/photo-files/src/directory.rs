use crate::{FileLease, LeaseMode};
use std::{
    ffi::{CStr, CString},
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
}

/// A retained directory descriptor. All descendant operations use openat2 with
/// no symlink or mount-escape traversal; returned absolute paths are for display
/// and explicit tool invocation, never authority to bypass the descriptor.
#[derive(Debug)]
pub struct Directory {
    file: File,
    path: PathBuf,
}

fn cpath(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

fn relative(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a contained relative path",
        ));
    }
    Ok(())
}

fn owned(fd: RawFd) -> io::Result<File> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn openat(directory: RawFd, path: &CStr, flags: i32, mode: u32) -> io::Result<File> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let how = OpenHow {
        flags: (flags | libc::O_CLOEXEC | libc::O_NOFOLLOW) as u64,
        mode: mode as u64,
        resolve: 0x08 | 0x04 | 0x02,
    };
    // SAFETY: valid retained dirfd, NUL-terminated relative name and Linux ABI.
    owned(unsafe {
        libc::syscall(
            libc::SYS_openat2,
            directory,
            path.as_ptr(),
            &how,
            size_of::<OpenHow>(),
        ) as i32
    })
}

impl Directory {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "directory must be absolute without parent traversal",
            ));
        }
        let root = owned(unsafe {
            libc::open(
                c"/".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        })?;
        let child = path.strip_prefix("/").map_err(io::Error::other)?;
        let file = if child.as_os_str().is_empty() {
            root
        } else {
            openat(
                root.as_raw_fd(),
                &cpath(child)?,
                libc::O_RDONLY | libc::O_DIRECTORY,
                0,
            )?
        };
        Ok(Self {
            file,
            path: path.to_owned(),
        })
    }

    pub fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "directory must be absolute",
            ));
        }
        let mut current = Self::open("/")?;
        for component in path.components() {
            match component {
                Component::RootDir => (),
                Component::Normal(name) => current = current.create_directory(Path::new(name))?,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid directory component",
                    ));
                }
            }
        }
        Ok(current)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn descriptor(&self) -> &File {
        &self.file
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        // An independent open description avoids flock conversion through dup.
        Ok(Self {
            file: openat(
                self.file.as_raw_fd(),
                c".",
                libc::O_RDONLY | libc::O_DIRECTORY,
                0,
            )?,
            path: self.path.clone(),
        })
    }

    pub fn child(&self, path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        relative(path)?;
        Ok(Self {
            file: openat(
                self.file.as_raw_fd(),
                &cpath(path)?,
                libc::O_RDONLY | libc::O_DIRECTORY,
                0,
            )?,
            path: self.path.join(path),
        })
    }

    pub fn create_directory(&self, name: impl AsRef<Path>) -> io::Result<Self> {
        let name = name.as_ref();
        relative(name)?;
        if name.components().count() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "create one directory at a time",
            ));
        }
        let cname = cpath(name)?;
        if unsafe { libc::mkdirat(self.file.as_raw_fd(), cname.as_ptr(), 0o700) } != 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        } else {
            self.sync()?;
        }
        self.child(name)
    }

    pub fn open_file(&self, path: impl AsRef<Path>) -> io::Result<File> {
        let path = path.as_ref();
        relative(path)?;
        let file = openat(
            self.file.as_raw_fd(),
            &cpath(path)?,
            libc::O_RDONLY | libc::O_NONBLOCK,
            0,
        )?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(file)
    }

    pub fn create_file(&self, name: impl AsRef<Path>, mode: u32) -> io::Result<File> {
        let name = name.as_ref();
        relative(name)?;
        openat(
            self.file.as_raw_fd(),
            &cpath(name)?,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
            mode,
        )
    }

    pub fn lease(&self, mode: LeaseMode, timeout: Duration) -> io::Result<FileLease> {
        FileLease::acquire(self.try_clone()?.file, mode, timeout)
    }

    pub fn sync(&self) -> io::Result<()> {
        self.file.sync_all()
    }

    pub fn remove_file(&self, name: impl AsRef<Path>) -> io::Result<()> {
        let name = name.as_ref();
        relative(name)?;
        let (parent, name) = self.parent_and_name(name)?;
        if unsafe { libc::unlinkat(parent.file.as_raw_fd(), cpath(&name)?.as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        parent.sync()
    }

    pub fn remove_directory(&self, name: impl AsRef<Path>) -> io::Result<()> {
        let name = name.as_ref();
        relative(name)?;
        let (parent, name) = self.parent_and_name(name)?;
        if unsafe {
            libc::unlinkat(
                parent.file.as_raw_fd(),
                cpath(&name)?.as_ptr(),
                libc::AT_REMOVEDIR,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        parent.sync()
    }

    pub fn rename(
        &self,
        from: impl AsRef<Path>,
        destination: &Self,
        to: impl AsRef<Path>,
        replace: bool,
    ) -> io::Result<()> {
        let from = from.as_ref();
        let to = to.as_ref();
        relative(from)?;
        relative(to)?;
        let (source, source_name) = self.parent_and_name(from)?;
        let (target, target_name) = destination.parent_and_name(to)?;
        if unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                source.file.as_raw_fd(),
                cpath(&source_name)?.as_ptr(),
                target.file.as_raw_fd(),
                cpath(&target_name)?.as_ptr(),
                if replace { 0 } else { libc::RENAME_NOREPLACE },
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        source.sync()?;
        target.sync()
    }

    fn parent_and_name(&self, path: &Path) -> io::Result<(Self, PathBuf)> {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file name missing"))?;
        let parent = match path.parent().filter(|p| !p.as_os_str().is_empty()) {
            Some(parent) => self.child(parent)?,
            None => self.try_clone()?,
        };
        Ok((parent, PathBuf::from(name)))
    }

    pub fn entries(&self, maximum: usize) -> io::Result<Vec<Entry>> {
        use std::os::fd::IntoRawFd;
        let fd = self.try_clone()?.file.into_raw_fd();
        let dir = unsafe { libc::fdopendir(fd) };
        if dir.is_null() {
            unsafe {
                libc::close(fd);
            }
            return Err(io::Error::last_os_error());
        }
        struct Dir(*mut libc::DIR);
        impl Drop for Dir {
            fn drop(&mut self) {
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let dir = Dir(dir);
        let mut entries = Vec::new();
        loop {
            unsafe {
                *libc::__errno_location() = 0;
            }
            let item = unsafe { libc::readdir(dir.0) };
            if item.is_null() {
                if unsafe { *libc::__errno_location() } != 0 {
                    return Err(io::Error::last_os_error());
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*item).d_name.as_ptr()) };
            if name == c"." || name == c".." {
                continue;
            }
            if entries.len() == maximum {
                return Err(io::Error::other("directory exceeds entry limit"));
            }
            let name = name
                .to_str()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file name is not UTF-8"))?
                .to_owned();
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe {
                libc::fstatat(
                    self.file.as_raw_fd(),
                    cpath(Path::new(&name))?.as_ptr(),
                    stat.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            let mode = unsafe { stat.assume_init() }.st_mode & libc::S_IFMT;
            let kind = match mode {
                libc::S_IFREG => EntryKind::File,
                libc::S_IFDIR => EntryKind::Directory,
                libc::S_IFLNK => EntryKind::Symlink,
                _ => EntryKind::Other,
            };
            entries.push(Entry { name, kind });
        }
        entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    /// Detect ancestor replacement before exposing a pathname to a child process.
    pub fn verify_path(&self) -> io::Result<()> {
        let current = Self::open(&self.path)?;
        let retained = self.file.metadata()?;
        let observed = current.file.metadata()?;
        if retained.dev() != observed.dev() || retained.ino() != observed.ino() {
            return Err(io::Error::other("directory path was replaced"));
        }
        Ok(())
    }
}

/// Shared ancestor leases let a backup/removal owner exclude all cooperating
/// independent writers without requiring those writers to recognize a service.
pub fn ancestor_leases(path: &Path, timeout: Duration) -> io::Result<Vec<FileLease>> {
    let mut directories = Vec::new();
    for ancestor in path.ancestors().skip(1) {
        directories.push(Directory::open(ancestor)?);
    }
    directories.reverse();
    directories
        .into_iter()
        .map(|dir| dir.lease(LeaseMode::Shared, timeout))
        .collect()
}
