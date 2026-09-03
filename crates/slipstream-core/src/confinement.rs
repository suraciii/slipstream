use crate::{
    domain::{
        DiscoveredOriginal, OriginalErrorCategory, OriginalFacts, OriginalKind, OriginalScanError,
        RelativeOriginalPath, ScanResult,
    },
    identity::{classify_extension, classify_name},
};
use std::{
    ffi::{CStr, CString, OsStr},
    fmt,
    fs::{File, OpenOptions},
    io,
    mem::MaybeUninit,
    os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[cfg(test)]
use std::sync::{Condvar, Mutex, OnceLock};

const MAXIMUM_RANGE_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_WHOLE_BYTES: u64 = 128 * 1024 * 1024;
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;

#[derive(Clone, Copy, Debug)]
pub struct ScanLimits {
    pub maximum_files: usize,
    pub maximum_entries: usize,
    pub maximum_entries_per_directory: u32,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            maximum_files: 100_000,
            maximum_entries: 250_000,
            maximum_entries_per_directory: 25_000,
        }
    }
}

impl ScanLimits {
    pub fn new(
        maximum_files: usize,
        maximum_entries: usize,
        maximum_entries_per_directory: u64,
    ) -> Result<Self, ConfinementError> {
        if maximum_files == 0 || maximum_entries == 0 {
            return Err(ConfinementError::ResourceLimit("scan limit is invalid"));
        }
        let maximum_entries_per_directory =
            u32::try_from(maximum_entries_per_directory).map_err(|_| {
                ConfinementError::ResourceLimit("directory entry limit exceeds 32 bits")
            })?;
        if maximum_entries_per_directory == 0 {
            return Err(ConfinementError::ResourceLimit(
                "directory entry limit is invalid",
            ));
        }
        Ok(Self {
            maximum_files,
            maximum_entries,
            maximum_entries_per_directory,
        })
    }
}

#[derive(Clone, Debug)]
pub enum ConfinementError {
    InvalidRoot,
    InvalidPath,
    Closed,
    UnsafeOpen,
    NotRegular,
    Changed,
    UnsupportedFilenameEncoding,
    ResourceLimit(&'static str),
    Io(&'static str),
}

impl fmt::Display for ConfinementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRoot => "Photo Library root is invalid",
            Self::InvalidPath => "Original path escapes the Photo Library",
            Self::Closed => "Photo Library is closed",
            Self::UnsafeOpen => "Original File could not be opened safely",
            Self::NotRegular => "Original File is not a regular file",
            Self::Changed => "Original File changed during read",
            Self::UnsupportedFilenameEncoding => "filename encoding is unsupported",
            Self::ResourceLimit(message) | Self::Io(message) => message,
        })
    }
}

impl std::error::Error for ConfinementError {}

struct RootInner {
    descriptor: OwnedFd,
    canonical_path: PathBuf,
    closed: AtomicBool,
}

#[derive(Clone)]
pub struct LibraryRoot(Arc<RootInner>);

impl LibraryRoot {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ConfinementError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(ConfinementError::InvalidRoot);
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| ConfinementError::InvalidRoot)?;
        let descriptor: OwnedFd = file.into();
        let canonical_path = canonical_path_for_descriptor(descriptor.as_raw_fd())?;
        Ok(Self(Arc::new(RootInner {
            descriptor,
            canonical_path,
            closed: AtomicBool::new(false),
        })))
    }

    pub fn canonical_path(&self) -> &Path {
        &self.0.canonical_path
    }

    pub fn original(
        &self,
        path: RelativeOriginalPath,
    ) -> Result<OriginalCapability, ConfinementError> {
        self.ensure_open()?;
        Ok(OriginalCapability {
            root: self.clone(),
            path,
        })
    }

    pub(crate) fn descendant(&self, path: RelativeOriginalPath) -> Result<Self, ConfinementError> {
        let file = self.open_confined(&path, true)?;
        let descriptor: OwnedFd = file.into();
        let canonical_path = canonical_path_for_descriptor(descriptor.as_raw_fd())?;
        Ok(Self(Arc::new(RootInner {
            descriptor,
            canonical_path,
            closed: AtomicBool::new(false),
        })))
    }

    pub(crate) fn identifies_same_directory(&self, other: &Self) -> Result<bool, ConfinementError> {
        self.ensure_open()?;
        other.ensure_open()?;
        let left =
            sys::fstat(self.0.descriptor.as_raw_fd()).map_err(|_| ConfinementError::UnsafeOpen)?;
        let right =
            sys::fstat(other.0.descriptor.as_raw_fd()).map_err(|_| ConfinementError::UnsafeOpen)?;
        Ok(left.st_mode & libc::S_IFMT == libc::S_IFDIR
            && right.st_mode & libc::S_IFMT == libc::S_IFDIR
            && left.st_dev == right.st_dev
            && left.st_ino == right.st_ino)
    }

    pub fn close(&self) {
        self.0.closed.store(true, Ordering::Release);
    }

    pub fn scan(&self, limits: ScanLimits) -> Result<ScanResult, ConfinementError> {
        self.scan_with_progress(limits, &AtomicU64::new(0))
    }

    /// Walks the Library and reports each recognized supported file through
    /// `discovered` as it is accepted, before inspection or publication.
    pub fn scan_with_progress(
        &self,
        limits: ScanLimits,
        discovered: &AtomicU64,
    ) -> Result<ScanResult, ConfinementError> {
        self.ensure_open()?;
        let mut scan = Scanner {
            root: self,
            limits,
            total_entries: 0,
            recognized_files: 0,
            discovered: Some(discovered),
            originals: Vec::new(),
            errors: Vec::new(),
        };
        scan.directory("")?;
        scan.originals.sort_by(|left, right| {
            left.path
                .as_str()
                .as_bytes()
                .cmp(right.path.as_str().as_bytes())
        });
        scan.errors.sort_by(|left, right| {
            left.path
                .as_str()
                .as_bytes()
                .cmp(right.path.as_str().as_bytes())
        });
        Ok(ScanResult {
            originals: scan.originals,
            errors: scan.errors,
        })
    }

    fn ensure_open(&self) -> Result<(), ConfinementError> {
        if self.0.closed.load(Ordering::Acquire) {
            Err(ConfinementError::Closed)
        } else {
            Ok(())
        }
    }

    fn open_confined(
        &self,
        path: &RelativeOriginalPath,
        directory: bool,
    ) -> Result<File, ConfinementError> {
        self.ensure_open()?;
        let path = CString::new(path.as_str()).map_err(|_| ConfinementError::InvalidPath)?;
        let descriptor = sys::open_confined(self.0.descriptor.as_raw_fd(), &path, directory)
            .map_err(|_| ConfinementError::UnsafeOpen)?;
        Ok(File::from(descriptor))
    }

    fn open_directory(&self, path: &str) -> Result<OwnedFd, ConfinementError> {
        self.ensure_open()?;
        if path.is_empty() {
            let dot = CString::new(".").unwrap();
            return sys::open_at_directory(self.0.descriptor.as_raw_fd(), &dot)
                .map_err(|_| ConfinementError::UnsafeOpen);
        }
        let relative = RelativeOriginalPath::parse(path.to_owned())
            .map_err(|_| ConfinementError::InvalidPath)?;
        let file = self.open_confined(&relative, true)?;
        Ok(file.into())
    }
}

#[derive(Clone)]
pub struct OriginalCapability {
    root: LibraryRoot,
    path: RelativeOriginalPath,
}

impl OriginalCapability {
    pub fn path(&self) -> &RelativeOriginalPath {
        &self.path
    }

    pub fn facts(&self) -> Result<OriginalFacts, ConfinementError> {
        let file = self.root.open_confined(&self.path, false)?;
        facts(file.as_raw_fd())
    }

    pub fn read_range(&self, offset: u64, length: usize) -> Result<Vec<u8>, ConfinementError> {
        if length > MAXIMUM_RANGE_BYTES || offset > i64::MAX as u64 {
            return Err(ConfinementError::ResourceLimit(
                "Confined read exceeds limits",
            ));
        }
        let file = self.root.open_confined(&self.path, false)?;
        let before = stat_regular(file.as_raw_fd())?;
        #[cfg(test)]
        test_hook(TestHookPoint::RangeAfterInitialStat);
        let mut bytes = vec![0; length];
        let count = sys::pread(file.as_raw_fd(), &mut bytes, offset)
            .map_err(|_| ConfinementError::Io("Original File could not be read safely"))?;
        bytes.truncate(count);
        let after = stat_regular(file.as_raw_fd())?;
        if !same_revision(&before, &after) {
            return Err(ConfinementError::Changed);
        }
        Ok(bytes)
    }

    pub fn read_whole(&self, maximum_bytes: u64) -> Result<RevisionCheckedBytes, ConfinementError> {
        if maximum_bytes > MAXIMUM_WHOLE_BYTES {
            return Err(ConfinementError::ResourceLimit(
                "Confined whole-file read exceeds limits",
            ));
        }
        let file = self.root.open_confined(&self.path, false)?;
        let before = stat_regular(file.as_raw_fd())?;
        #[cfg(test)]
        test_hook(TestHookPoint::WholeAfterInitialStat);
        let size = validated_size(&before)?;
        if size > maximum_bytes {
            return Err(ConfinementError::ResourceLimit(
                "Original File exceeds whole-file read limit",
            ));
        }
        let mut bytes = vec![
            0;
            usize::try_from(size).map_err(|_| {
                ConfinementError::ResourceLimit("Original File exceeds whole-file read limit")
            })?
        ];
        let mut consumed = 0;
        while consumed < bytes.len() {
            let count = match sys::pread(file.as_raw_fd(), &mut bytes[consumed..], consumed as u64)
            {
                Ok(count) => count,
                Err(_) => {
                    return Err(read_failure_after_revision_check(
                        file.as_raw_fd(),
                        &before,
                        "Original File could not be read completely",
                    ));
                }
            };
            if count == 0 {
                return Err(read_failure_after_revision_check(
                    file.as_raw_fd(),
                    &before,
                    "Original File could not be read completely",
                ));
            }
            consumed += count;
        }
        let after = stat_regular(file.as_raw_fd())?;
        if !same_revision(&before, &after) {
            return Err(ConfinementError::Changed);
        }
        Ok(RevisionCheckedBytes {
            bytes,
            facts: facts_from_stat(&before)?,
        })
    }

    pub(crate) fn open_revision_checked(&self) -> Result<OpenedOriginal, ConfinementError> {
        let file = self.root.open_confined(&self.path, false)?;
        let revision = stat_regular(file.as_raw_fd())?;
        Ok(OpenedOriginal { file, revision })
    }
}

pub(crate) struct OpenedOriginal {
    file: File,
    revision: libc::stat,
}

impl OpenedOriginal {
    /// Reads one bounded range through the descriptor retained for this
    /// inspection. A short result is meaningful to format parsers (for
    /// example, a truncated EXIF segment); callers verify the same descriptor
    /// once their complete inspection is finished.
    pub(crate) fn pread_range(
        &self,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, ConfinementError> {
        if length > MAXIMUM_RANGE_BYTES || offset > i64::MAX as u64 {
            return Err(ConfinementError::ResourceLimit(
                "Confined read exceeds limits",
            ));
        }
        let mut bytes = vec![0; length];
        let count = sys::pread(self.file.as_raw_fd(), &mut bytes, offset).map_err(|_| {
            read_failure_after_revision_check(
                self.file.as_raw_fd(),
                &self.revision,
                "Original File could not be read safely",
            )
        })?;
        bytes.truncate(count);
        Ok(bytes)
    }

    pub(crate) fn size(&self) -> Result<u64, ConfinementError> {
        validated_size(&self.revision)
    }

    pub(crate) fn descriptor(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    pub(crate) fn verify_unchanged(&self) -> Result<OriginalFacts, ConfinementError> {
        #[cfg(test)]
        test_hook(TestHookPoint::DescriptorBeforeVerification);
        let after = stat_regular(self.file.as_raw_fd())?;
        if !same_revision(&self.revision, &after) {
            return Err(ConfinementError::Changed);
        }
        facts_from_stat(&self.revision)
    }
}

#[derive(Debug)]
pub struct RevisionCheckedBytes {
    pub bytes: Vec<u8>,
    pub facts: OriginalFacts,
}

struct Scanner<'a> {
    root: &'a LibraryRoot,
    limits: ScanLimits,
    total_entries: usize,
    recognized_files: usize,
    discovered: Option<&'a AtomicU64>,
    originals: Vec<DiscoveredOriginal>,
    errors: Vec<OriginalScanError>,
}

impl Scanner<'_> {
    fn directory(&mut self, path: &str) -> Result<(), ConfinementError> {
        let descriptor = self.root.open_directory(path)?;
        let entries = sys::list_directory(descriptor, self.limits.maximum_entries_per_directory)?;
        self.total_entries = self.total_entries.checked_add(entries.len()).ok_or(
            ConfinementError::ResourceLimit("Photo Library exceeds total entry limit"),
        )?;
        if self.total_entries > self.limits.maximum_entries {
            return Err(ConfinementError::ResourceLimit(
                "Photo Library exceeds total entry limit",
            ));
        }
        for entry in entries {
            let name = entry.name.to_str();
            if entry.kind == EntryKind::Directory {
                let name = name.ok_or(ConfinementError::UnsupportedFilenameEncoding)?;
                self.directory(&join(path, name))?;
                continue;
            }
            let Some(kind) = classify_entry(&entry.name, entry.kind)? else {
                continue;
            };
            let name = name.ok_or(ConfinementError::UnsupportedFilenameEncoding)?;
            let relative = RelativeOriginalPath::parse(join(path, name))
                .map_err(|_| ConfinementError::InvalidPath)?;
            if entry.kind != EntryKind::File {
                self.errors.push(OriginalScanError {
                    path: relative,
                    kind,
                    category: OriginalErrorCategory::Unreadable,
                    message: "Original File could not be inspected".to_owned(),
                });
                continue;
            }
            self.recognized_files += 1;
            if let Some(discovered) = self.discovered {
                discovered.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            if self.recognized_files > self.limits.maximum_files {
                return Err(ConfinementError::ResourceLimit(
                    "Photo Library exceeds recognized file limit",
                ));
            }
            match self.root.original(relative.clone())?.facts() {
                Ok(file_facts) => self.originals.push(DiscoveredOriginal {
                    path: relative,
                    kind,
                    facts: file_facts,
                    error_category: None,
                    error_message: None,
                    capture: crate::CaptureFact::pending(),
                }),
                Err(_) => {
                    let error_category = OriginalErrorCategory::Unreadable;
                    let error_message = "Original File could not be inspected".to_owned();
                    self.originals.push(DiscoveredOriginal {
                        path: relative.clone(),
                        kind,
                        facts: OriginalFacts::UNREADABLE,
                        error_category: Some(error_category),
                        error_message: Some(error_message.clone()),
                        capture: crate::CaptureFact::pending(),
                    });
                    self.errors.push(OriginalScanError {
                        path: relative,
                        kind,
                        category: error_category,
                        message: error_message,
                    });
                }
            }
        }
        Ok(())
    }
}

fn classify_entry(name: &OsStr, kind: EntryKind) -> Result<Option<OriginalKind>, ConfinementError> {
    if let Some(name) = name.to_str() {
        return Ok(classify_name(name).filter(|_| kind != EntryKind::Directory));
    }
    let bytes = name.as_bytes();
    let supported_suffix = bytes
        .rsplit(|byte| *byte == b'.')
        .next()
        .filter(|suffix| suffix.len() < bytes.len())
        .and_then(classify_extension)
        .is_some();
    if supported_suffix && kind == EntryKind::File {
        Err(ConfinementError::UnsupportedFilenameEncoding)
    } else {
        Ok(None)
    }
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn stat_regular(fd: RawFd) -> Result<libc::stat, ConfinementError> {
    let value =
        sys::fstat(fd).map_err(|_| ConfinementError::Io("Original File could not be inspected"))?;
    if value.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(ConfinementError::NotRegular);
    }
    Ok(value)
}

fn facts(fd: RawFd) -> Result<OriginalFacts, ConfinementError> {
    let value = stat_regular(fd)?;
    facts_from_stat(&value)
}

fn validated_size(value: &libc::stat) -> Result<u64, ConfinementError> {
    u64::try_from(value.st_size)
        .map_err(|_| ConfinementError::Io("Original File facts are invalid"))
}

fn facts_from_stat(value: &libc::stat) -> Result<OriginalFacts, ConfinementError> {
    let size = validated_size(value)?;
    let mtime_ms = value.st_mtime as f64 * 1000.0 + value.st_mtime_nsec as f64 / 1_000_000.0;
    if !mtime_ms.is_finite() || mtime_ms < 0.0 {
        return Err(ConfinementError::Io("Original File facts are invalid"));
    }
    Ok(OriginalFacts {
        size,
        mtime_ms,
        device: value.st_dev,
        inode: value.st_ino,
    })
}

fn read_failure_after_revision_check(
    fd: RawFd,
    before: &libc::stat,
    message: &'static str,
) -> ConfinementError {
    match stat_regular(fd) {
        Ok(after) if !same_revision(before, &after) => ConfinementError::Changed,
        _ => ConfinementError::Io(message),
    }
}

fn canonical_path_for_descriptor(fd: RawFd) -> Result<PathBuf, ConfinementError> {
    let descriptor_path = PathBuf::from(format!("/proc/self/fd/{fd}"));
    let canonical_path = descriptor_path
        .canonicalize()
        .map_err(|_| ConfinementError::InvalidRoot)?;
    if !canonical_path.is_absolute() {
        return Err(ConfinementError::InvalidRoot);
    }
    Ok(canonical_path)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestHookPoint {
    RangeAfterInitialStat,
    WholeAfterInitialStat,
    DescriptorBeforeVerification,
}

#[cfg(test)]
struct TestHook {
    point: TestHookPoint,
    target_thread: Mutex<Option<std::thread::ThreadId>>,
    reached: Mutex<bool>,
    reached_signal: Condvar,
    resume: Mutex<bool>,
    resume_signal: Condvar,
}

#[cfg(test)]
static TEST_HOOK: OnceLock<Mutex<Option<Arc<TestHook>>>> = OnceLock::new();

#[cfg(test)]
fn test_hook(point: TestHookPoint) {
    let selected = TEST_HOOK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .clone();
    let Some(hook) = selected.filter(|hook| {
        hook.point == point
            && *hook.target_thread.lock().unwrap() == Some(std::thread::current().id())
    }) else {
        return;
    };
    *hook.reached.lock().unwrap() = true;
    hook.reached_signal.notify_one();
    let mut resume = hook.resume.lock().unwrap();
    while !*resume {
        resume = hook.resume_signal.wait(resume).unwrap();
    }
}

fn same_revision(left: &libc::stat, right: &libc::stat) -> bool {
    left.st_dev == right.st_dev
        && left.st_ino == right.st_ino
        && left.st_size == right.st_size
        && left.st_mtime == right.st_mtime
        && left.st_mtime_nsec == right.st_mtime_nsec
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

struct DirectoryEntry {
    name: std::ffi::OsString,
    kind: EntryKind,
}

mod sys {
    use super::*;

    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }

    pub fn open_confined(root: RawFd, path: &CStr, directory: bool) -> io::Result<OwnedFd> {
        let how = OpenHow {
            flags: (libc::O_RDONLY
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | if directory {
                    libc::O_DIRECTORY
                } else {
                    libc::O_NONBLOCK
                }) as u64,
            mode: 0,
            resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
        };
        // SAFETY: `path` is NUL-terminated and `how` matches Linux `open_how`.
        let descriptor = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                root,
                path.as_ptr(),
                &how,
                size_of::<OpenHow>(),
            ) as libc::c_int
        };
        owned(descriptor)
    }

    pub fn open_at_directory(root: RawFd, path: &CStr) -> io::Result<OwnedFd> {
        // SAFETY: `path` is NUL-terminated and the returned descriptor is uniquely owned.
        owned(unsafe {
            libc::openat(
                root,
                path.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            )
        })
    }

    pub fn list_directory(
        descriptor: OwnedFd,
        maximum: u32,
    ) -> Result<Vec<DirectoryEntry>, ConfinementError> {
        let raw = descriptor.as_raw_fd();
        let owned_raw = descriptor.into_raw_fd();
        // SAFETY: ownership transfers to `DIR`; `closedir` closes the descriptor.
        let directory = unsafe { libc::fdopendir(owned_raw) };
        if directory.is_null() {
            // SAFETY: fdopendir did not take ownership on failure.
            unsafe { libc::close(owned_raw) };
            return Err(ConfinementError::Io(
                "Directory could not be enumerated safely",
            ));
        }
        let mut entries = Vec::new();
        loop {
            errno_clear();
            // SAFETY: `directory` remains valid until `closedir` below.
            let item = unsafe { libc::readdir(directory) };
            if item.is_null() {
                if errno() != 0 {
                    // SAFETY: this is the unique live DIR pointer.
                    unsafe { libc::closedir(directory) };
                    return Err(ConfinementError::Io("Directory enumeration failed safely"));
                }
                break;
            }
            // SAFETY: `d_name` is NUL-terminated for the returned dirent.
            let name = unsafe { CStr::from_ptr((*item).d_name.as_ptr()) };
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            if entries.len() >= maximum as usize {
                // SAFETY: this is the unique live DIR pointer.
                unsafe { libc::closedir(directory) };
                return Err(ConfinementError::ResourceLimit(
                    "Directory exceeds entry limit",
                ));
            }
            let facts = fstat_at(raw, name);
            let kind = facts.map_or(EntryKind::Other, |facts| {
                match facts.st_mode & libc::S_IFMT {
                    libc::S_IFREG => EntryKind::File,
                    libc::S_IFDIR => EntryKind::Directory,
                    libc::S_IFLNK => EntryKind::Symlink,
                    _ => EntryKind::Other,
                }
            });
            entries.push(DirectoryEntry {
                name: OsStr::from_bytes(name.to_bytes()).to_owned(),
                kind,
            });
        }
        // SAFETY: this is the unique live DIR pointer.
        unsafe { libc::closedir(directory) };
        entries.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
        Ok(entries)
    }

    pub fn fstat(fd: RawFd) -> io::Result<libc::stat> {
        let mut value = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `value` points to writable storage and is initialized on success.
        if unsafe { libc::fstat(fd, value.as_mut_ptr()) } == 0 {
            // SAFETY: successful `fstat` initialized the value.
            Ok(unsafe { value.assume_init() })
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn pread(fd: RawFd, bytes: &mut [u8], offset: u64) -> io::Result<usize> {
        // SAFETY: the slice is writable for `bytes.len()` and the descriptor is open.
        let count = unsafe {
            libc::pread(
                fd,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                offset as libc::off_t,
            )
        };
        if count < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(count as usize)
        }
    }

    fn fstat_at(fd: RawFd, name: &CStr) -> io::Result<libc::stat> {
        let mut value = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: arguments are valid and `value` is initialized on success.
        if unsafe {
            libc::fstatat(
                fd,
                name.as_ptr(),
                value.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
        {
            // SAFETY: successful `fstatat` initialized the value.
            Ok(unsafe { value.assume_init() })
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn owned(descriptor: libc::c_int) -> io::Result<OwnedFd> {
        if descriptor < 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: a successful open returned a new uniquely-owned descriptor.
            Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
        }
    }

    fn errno_clear() {
        // SAFETY: libc exposes thread-local errno storage.
        unsafe { *libc::__errno_location() = 0 };
    }

    fn errno() -> libc::c_int {
        // SAFETY: libc exposes thread-local errno storage.
        unsafe { *libc::__errno_location() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{original_snapshot, raw_sample};
    use std::{
        fs,
        os::unix::{
            ffi::OsStringExt,
            fs::{PermissionsExt, symlink},
        },
        sync::{MutexGuard, OnceLock, mpsc},
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    static TEST_HOOK_LEASE: OnceLock<Mutex<()>> = OnceLock::new();

    struct HookHarness {
        hook: Arc<TestHook>,
        _lease: MutexGuard<'static, ()>,
    }

    impl HookHarness {
        fn install(point: TestHookPoint) -> Self {
            let lease = TEST_HOOK_LEASE
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap();
            let hook = Arc::new(TestHook {
                point,
                target_thread: Mutex::new(None),
                reached: Mutex::new(false),
                reached_signal: Condvar::new(),
                resume: Mutex::new(false),
                resume_signal: Condvar::new(),
            });
            *TEST_HOOK.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(hook.clone());
            Self {
                hook,
                _lease: lease,
            }
        }

        fn target(&self, thread: &thread::Thread) {
            *self.hook.target_thread.lock().unwrap() = Some(thread.id());
            thread.unpark();
        }

        fn wait_until_reached(&self) {
            let mut reached = self.hook.reached.lock().unwrap();
            while !*reached {
                reached = self.hook.reached_signal.wait(reached).unwrap();
            }
        }

        fn resume(&self) {
            *self.hook.resume.lock().unwrap() = true;
            self.hook.resume_signal.notify_one();
        }
    }

    impl Drop for HookHarness {
        fn drop(&mut self) {
            self.resume();
            *TEST_HOOK.get_or_init(|| Mutex::new(None)).lock().unwrap() = None;
        }
    }

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("slipstream-core-{}-{unique}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, relative: &str, bytes: &[u8]) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn validates_paths_and_scan_limits() {
        for path in [
            "",
            "/absolute.JPG",
            "../escape.JPG",
            "a/./b.JPG",
            "a//b.JPG",
            "a/b/",
        ] {
            assert!(RelativeOriginalPath::parse(path).is_err());
        }
        assert!(RelativeOriginalPath::parse("a\\b.JPG").is_ok());
        assert!(RelativeOriginalPath::parse("photo..final.JPG").is_ok());
        assert!(ScanLimits::new(1, 1, u32::MAX as u64).is_ok());
        assert!(ScanLimits::new(1, 1, u32::MAX as u64 + 1).is_err());
    }

    #[test]
    fn traverses_deterministically_and_enforces_all_limits() {
        let tree = TempTree::new();
        tree.write("z/two.nef", b"two");
        tree.write("a/one.ARW", b"raw");
        tree.write("a/one.jpg", b"jpeg");
        tree.write("notes.txt", b"ignored");
        let root = LibraryRoot::open(tree.path()).unwrap();
        let paths: Vec<_> = root
            .scan(ScanLimits::default())
            .unwrap()
            .originals
            .into_iter()
            .map(|original| original.path.to_string())
            .collect();
        assert_eq!(paths, ["a/one.ARW", "a/one.jpg", "z/two.nef"]);
        assert!(root.scan(ScanLimits::new(2, 100, 100).unwrap()).is_err());
        assert!(matches!(
            root.scan(ScanLimits::new(100, 2, 100).unwrap()),
            Err(ConfinementError::ResourceLimit(
                "Photo Library exceeds total entry limit"
            ))
        ));
        assert!(root.scan(ScanLimits::new(100, 100, 1).unwrap()).is_err());
    }

    #[test]
    fn counts_the_current_directory_before_recursing() {
        let tree = TempTree::new();
        fs::create_dir(tree.path().join("a")).unwrap();
        tree.write("z.JPG", b"photo");
        let root = LibraryRoot::open(tree.path()).unwrap();
        assert!(matches!(
            root.scan(ScanLimits::new(100, 1, 100).unwrap()),
            Err(ConfinementError::ResourceLimit(
                "Photo Library exceeds total entry limit"
            ))
        ));
    }

    #[test]
    fn treats_supported_symlinks_as_unreadable_without_following_them() {
        let tree = TempTree::new();
        let outside = TempTree::new();
        outside.write("outside.JPG", b"outside");
        symlink(
            outside.path().join("outside.JPG"),
            tree.path().join("link.JPG"),
        )
        .unwrap();
        tree.write("real.JPG", b"real");
        let result = LibraryRoot::open(tree.path())
            .unwrap()
            .scan(ScanLimits::new(1, 100, 100).unwrap())
            .unwrap();
        assert_eq!(result.originals.len(), 1);
        assert_eq!(result.originals[0].path.as_str(), "real.JPG");
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].path.as_str(), "link.JPG");
        assert_eq!(
            result.errors[0].message,
            "Original File could not be inspected"
        );
    }

    #[test]
    fn reports_pre_epoch_files_unreadable_without_aborting_siblings() {
        let tree = TempTree::new();
        tree.write("old.JPG", b"old");
        tree.write("valid.JPG", b"valid");
        let path = CString::new(tree.path().join("old.JPG").as_os_str().as_bytes()).unwrap();
        let times = [
            libc::timespec {
                tv_sec: -1,
                tv_nsec: 0,
            },
            libc::timespec {
                tv_sec: -1,
                tv_nsec: 0,
            },
        ];
        // SAFETY: the path is NUL-terminated and `times` contains two valid timespec values.
        assert_eq!(
            unsafe { libc::utimensat(libc::AT_FDCWD, path.as_ptr(), times.as_ptr(), 0) },
            0
        );
        let result = LibraryRoot::open(tree.path())
            .unwrap()
            .scan(ScanLimits::default())
            .unwrap();
        assert_eq!(result.originals.len(), 2);
        assert_eq!(result.originals[0].path.as_str(), "old.JPG");
        assert_eq!(
            result.originals[0].error_category,
            Some(OriginalErrorCategory::Unreadable)
        );
        assert_eq!(result.originals[1].path.as_str(), "valid.JPG");
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].path.as_str(), "old.JPG");
    }

    #[test]
    fn rejects_supported_non_utf8_names_without_lossy_conversion() {
        let tree = TempTree::new();
        let name = std::ffi::OsString::from_vec(b"photo-\xff.JPG".to_vec());
        fs::write(tree.path().join(name), b"bytes").unwrap();
        assert!(matches!(
            LibraryRoot::open(tree.path())
                .unwrap()
                .scan(ScanLimits::default()),
            Err(ConfinementError::UnsupportedFilenameEncoding)
        ));
    }

    #[test]
    fn fifo_original_replacement_is_rejected_without_blocking() {
        let tree = TempTree::new();
        tree.write("photo.JPG", b"bytes");
        let root = LibraryRoot::open(tree.path()).unwrap();
        let original = root
            .original(RelativeOriginalPath::parse("photo.JPG").unwrap())
            .unwrap();
        fs::remove_file(tree.path().join("photo.JPG")).unwrap();
        let fifo = CString::new(tree.path().join("photo.JPG").as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo` is NUL-terminated and the path does not exist.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let (send, receive) = mpsc::channel();
        thread::spawn(move || send.send(original.facts()).unwrap());
        assert!(
            receive
                .recv_timeout(Duration::from_secs(1))
                .expect("FIFO Original open must not block")
                .is_err()
        );
    }

    #[test]
    fn reads_only_regular_files_with_bounded_revision_checked_operations() {
        let tree = TempTree::new();
        tree.write("dir/photo.JPG", b"inside");
        let root = LibraryRoot::open(tree.path()).unwrap();
        let original = root
            .original(RelativeOriginalPath::parse("dir/photo.JPG").unwrap())
            .unwrap();
        assert_eq!(original.facts().unwrap().size, 6);
        assert_eq!(original.read_range(2, 20).unwrap(), b"side");
        let whole = original.read_whole(6).unwrap();
        assert_eq!(whole.bytes, b"inside");
        assert_eq!(whole.facts.size, 6);
        assert!(original.read_range(0, MAXIMUM_RANGE_BYTES + 1).is_err());
        assert!(original.read_whole(5).is_err());
        assert!(original.read_whole(MAXIMUM_WHOLE_BYTES + 1).is_err());
        let opened = original.open_revision_checked().unwrap();
        assert!(opened.descriptor() >= 0);
        assert_eq!(opened.verify_unchanged().unwrap().size, 6);
        assert!(
            root.original(RelativeOriginalPath::parse("dir").unwrap())
                .unwrap()
                .facts()
                .is_err()
        );
    }

    #[test]
    fn rejects_a_symlink_library_root() {
        let container = TempTree::new();
        let actual = container.path().join("actual");
        let alias = container.path().join("alias");
        fs::create_dir(&actual).unwrap();
        symlink(&actual, &alias).unwrap();
        assert!(matches!(
            LibraryRoot::open(&alias),
            Err(ConfinementError::InvalidRoot)
        ));
    }

    #[test]
    fn descendant_descriptor_identity_distinguishes_different_directories() {
        let base = TempTree::new();
        fs::create_dir(base.path().join("ancestor")).unwrap();
        fs::create_dir(base.path().join("ancestor/old")).unwrap();
        fs::create_dir(base.path().join("ancestor/other")).unwrap();
        let ancestor = LibraryRoot::open(base.path().join("ancestor")).unwrap();
        let confined = ancestor
            .descendant(RelativeOriginalPath::parse("old").unwrap())
            .unwrap();
        let same = LibraryRoot::open(base.path().join("ancestor/old")).unwrap();
        let other = LibraryRoot::open(base.path().join("ancestor/other")).unwrap();
        assert!(confined.identifies_same_directory(&same).unwrap());
        assert!(!confined.identifies_same_directory(&other).unwrap());
    }

    #[test]
    fn canonical_path_is_derived_from_the_retained_descriptor() {
        let container = TempTree::new();
        let original = container.path().join("original");
        let moved = container.path().join("moved");
        fs::create_dir(&original).unwrap();
        let descriptor = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&original)
            .unwrap();
        fs::rename(&original, &moved).unwrap();
        fs::create_dir(&original).unwrap();
        assert_eq!(
            canonical_path_for_descriptor(descriptor.as_raw_fd()).unwrap(),
            moved
        );
    }

    #[test]
    fn descriptor_canonicalization_distinguishes_literal_deleted_suffix_from_unlink() {
        let container = TempTree::new();
        let live = container.path().join("photos (deleted)");
        fs::create_dir(&live).unwrap();
        let root = LibraryRoot::open(&live).unwrap();
        assert_eq!(root.canonical_path(), live);

        let unlinked = container.path().join("unlinked");
        fs::create_dir(&unlinked).unwrap();
        let descriptor = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&unlinked)
            .unwrap();
        fs::remove_dir(&unlinked).unwrap();
        assert!(matches!(
            canonical_path_for_descriptor(descriptor.as_raw_fd()),
            Err(ConfinementError::InvalidRoot)
        ));
    }

    #[test]
    fn rejects_symlink_and_directory_escape_after_replacement() {
        let tree = TempTree::new();
        let outside = TempTree::new();
        tree.write("dir/inside.JPG", b"inside");
        outside.write("outside.JPG", b"outside");
        let root = LibraryRoot::open(tree.path()).unwrap();
        fs::rename(tree.path().join("dir"), tree.path().join("old")).unwrap();
        symlink(outside.path(), tree.path().join("dir")).unwrap();
        let escaped = root
            .original(RelativeOriginalPath::parse("dir/outside.JPG").unwrap())
            .unwrap();
        assert!(matches!(escaped.facts(), Err(ConfinementError::UnsafeOpen)));
        symlink(
            outside.path().join("outside.JPG"),
            tree.path().join("leaf.JPG"),
        )
        .unwrap();
        let leaf = root
            .original(RelativeOriginalPath::parse("leaf.JPG").unwrap())
            .unwrap();
        assert!(matches!(
            leaf.read_range(0, 10),
            Err(ConfinementError::UnsafeOpen)
        ));
    }

    #[test]
    fn detects_in_place_revision_changes_and_drains_admitted_reads() {
        let tree = TempTree::new();
        tree.write("photo.JPG", b"abcdef");
        let root = LibraryRoot::open(tree.path()).unwrap();
        let path = RelativeOriginalPath::parse("photo.JPG").unwrap();

        let range_hook = HookHarness::install(TestHookPoint::RangeAfterInitialStat);
        let range_original = root.original(path.clone()).unwrap();
        let range_thread = thread::spawn(move || {
            thread::park();
            range_original.read_range(0, 6)
        });
        range_hook.target(range_thread.thread());
        range_hook.wait_until_reached();
        OpenOptions::new()
            .write(true)
            .open(tree.path().join("photo.JPG"))
            .unwrap()
            .set_len(3)
            .unwrap();
        root.close();
        assert!(matches!(
            root.original(path.clone()),
            Err(ConfinementError::Closed)
        ));
        range_hook.resume();
        assert!(matches!(
            range_thread.join().unwrap(),
            Err(ConfinementError::Changed)
        ));
        drop(range_hook);

        let reopened = LibraryRoot::open(tree.path()).unwrap();
        fs::write(tree.path().join("photo.JPG"), b"abcdef").unwrap();
        let whole_hook = HookHarness::install(TestHookPoint::WholeAfterInitialStat);
        let whole_original = reopened.original(path.clone()).unwrap();
        let whole_thread = thread::spawn(move || {
            thread::park();
            whole_original.read_whole(6)
        });
        whole_hook.target(whole_thread.thread());
        whole_hook.wait_until_reached();
        OpenOptions::new()
            .write(true)
            .open(tree.path().join("photo.JPG"))
            .unwrap()
            .set_len(3)
            .unwrap();
        whole_hook.resume();
        assert!(matches!(
            whole_thread.join().unwrap(),
            Err(ConfinementError::Changed)
        ));
        drop(whole_hook);

        fs::write(tree.path().join("photo.JPG"), b"abcdef").unwrap();
        let descriptor_original = reopened.original(path).unwrap();
        let opened = descriptor_original.open_revision_checked().unwrap();
        let descriptor_hook = HookHarness::install(TestHookPoint::DescriptorBeforeVerification);
        let verify_thread = thread::spawn(move || {
            thread::park();
            opened.verify_unchanged()
        });
        descriptor_hook.target(verify_thread.thread());
        descriptor_hook.wait_until_reached();
        OpenOptions::new()
            .write(true)
            .open(tree.path().join("photo.JPG"))
            .unwrap()
            .set_len(2)
            .unwrap();
        descriptor_hook.resume();
        assert!(matches!(
            verify_thread.join().unwrap(),
            Err(ConfinementError::Changed)
        ));
    }

    #[test]
    fn scan_and_reads_never_mutate_originals_and_close_rejects_new_operations() {
        let tree = TempTree::new();
        tree.write("a/one.JPG", b"one");
        tree.write("b/two.ARW", b"two");
        fs::set_permissions(
            tree.path().join("b/two.ARW"),
            fs::Permissions::from_mode(0o400),
        )
        .unwrap();
        let before = snapshot(tree.path());
        let root = LibraryRoot::open(tree.path()).unwrap();
        root.scan(ScanLimits::default()).unwrap();
        root.scan(ScanLimits::default()).unwrap();
        root.original(RelativeOriginalPath::parse("a/one.JPG").unwrap())
            .unwrap()
            .read_whole(16)
            .unwrap();
        assert_eq!(snapshot(tree.path()), before);
        let captured = root
            .original(RelativeOriginalPath::parse("a/one.JPG").unwrap())
            .unwrap();
        root.close();
        assert!(matches!(captured.facts(), Err(ConfinementError::Closed)));
    }

    fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>, u32)> {
        fn walk(root: &Path, current: &Path, rows: &mut Vec<(PathBuf, Vec<u8>, u32)>) {
            let mut entries: Vec<_> = fs::read_dir(current).unwrap().map(Result::unwrap).collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).unwrap();
                if metadata.is_dir() {
                    walk(root, &path, rows);
                } else if metadata.is_file() {
                    rows.push((
                        path.strip_prefix(root).unwrap().to_owned(),
                        fs::read(&path).unwrap(),
                        metadata.permissions().mode(),
                    ));
                }
            }
        }
        let mut rows = Vec::new();
        walk(root, root, &mut rows);
        rows
    }

    #[test]
    #[ignore = "requires SLIPSTREAM_RAW_SAMPLE"]
    fn sony_original_remains_unchanged() {
        let (path, before) = raw_sample();
        let root = LibraryRoot::open(path.parent().unwrap()).unwrap();
        let relative = RelativeOriginalPath::parse(
            path.file_name()
                .unwrap()
                .to_str()
                .expect("sample filename must be UTF-8"),
        )
        .unwrap();
        let original = root.original(relative).unwrap();
        assert_eq!(original.facts().unwrap().size, before.length);
        let _ = original.read_range(0, 1024).unwrap();
        assert_eq!(original_snapshot(&path), before);
    }
}
