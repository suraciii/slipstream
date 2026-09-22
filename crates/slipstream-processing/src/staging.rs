//! The only Film descriptor transfer. Public launcher IPC never accepts descriptors.
use crate::{
    film::{self, EngineGrant, FileIdentity, StageOffer},
    journal::Record,
    protocol::{ErrorCode, digest},
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    mem,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
    time::Instant,
};
pub(crate) type Result<T> = std::result::Result<T, ErrorCode>;
fn fail(_: io::Error) -> ErrorCode {
    ErrorCode::Uncertain
}
pub fn identity(file: &File) -> io::Result<FileIdentity> {
    let m = file.metadata()?;
    Ok(FileIdentity {
        device: m.dev(),
        inode: m.ino(),
    })
}
pub fn safe_open(directory: &File, name: &str, flags: i32) -> io::Result<File> {
    let name = std::ffi::CString::new(name)?;
    #[repr(C)]
    struct How {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let how = How {
        flags: (flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK) as u64,
        mode: 0,
        resolve: 0x08 | 0x04 | 0x02 | 0x01,
    };
    // SAFETY: openat2 reads the initialized ABI struct and NUL-terminated confined relative name.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            directory.as_raw_fd(),
            name.as_ptr(),
            &how,
            mem::size_of::<How>(),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat2 returned a new descriptor.
    Ok(unsafe { File::from_raw_fd(fd as i32) })
}
pub fn access(fd: RawFd) -> io::Result<i32> {
    // SAFETY: F_GETFL reads flags for the borrowed descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(flags & libc::O_ACCMODE)
    }
}
fn address(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    // SAFETY: sockaddr_un is plain integer storage and a zero address is valid for initialization.
    let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return Err(io::Error::other("stage path"));
    }
    address.sun_family = libc::AF_UNIX as _;
    for (dst, src) in address.sun_path.iter_mut().zip(bytes) {
        *dst = *src as _;
    }
    Ok((address, mem::size_of::<libc::sockaddr_un>() as _))
}
pub fn socket() -> io::Result<OwnedFd> {
    // SAFETY: socket creates a fresh nonblocking Unix seqpacket endpoint.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful socket returned a fresh descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
pub fn connect(path: &Path) -> io::Result<OwnedFd> {
    let fd = socket()?;
    let (address, len) = address(path)?;
    // SAFETY: address is a live sockaddr_un and len matches its size.
    if unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            len,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}
pub fn send<T: Serialize>(fd: RawFd, value: &T, rights: &[RawFd]) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > film::FRAME || rights.len() > 3 {
        return Err(io::Error::other("stage packet limit"));
    }
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut _,
        iov_len: bytes.len(),
    };
    let mut ancillary = [0usize; 8];
    // SAFETY: zeroed msghdr is initialized with pointers to live stack buffers below.
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if !rights.is_empty() {
        msg.msg_control = ancillary.as_mut_ptr().cast();
        // SAFETY: ancillary storage is aligned for cmsghdr and sufficiently large for three descriptors.
        unsafe {
            msg.msg_controllen = libc::CMSG_SPACE(mem::size_of_val(rights) as _) as usize;
            let header = libc::CMSG_FIRSTHDR(&msg);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(mem::size_of_val(rights) as _) as usize;
            std::ptr::copy_nonoverlapping(
                rights.as_ptr(),
                libc::CMSG_DATA(header).cast::<RawFd>(),
                rights.len(),
            );
        }
    }
    // SAFETY: every msghdr pointer remains live through this nonblocking sendmsg.
    let sent = unsafe { libc::sendmsg(fd, &msg, libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT) };
    if sent < 0 {
        Err(io::Error::last_os_error())
    } else if sent as usize != bytes.len() {
        Err(io::Error::other("partial packet"))
    } else {
        Ok(())
    }
}
pub fn receive<T: DeserializeOwned>(fd: RawFd) -> io::Result<Option<(T, Vec<OwnedFd>)>> {
    let mut bytes = [0u8; film::FRAME];
    let mut control = [0usize; 16];
    let mut iov = libc::iovec {
        iov_base: bytes.as_mut_ptr().cast(),
        iov_len: bytes.len(),
    };
    // SAFETY: msghdr is filled with live aligned buffers before recvmsg.
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = mem::size_of_val(&control);
    // SAFETY: buffers are writable and the kernel sets CLOEXEC atomically on received rights.
    let count = unsafe { libc::recvmsg(fd, &mut msg, libc::MSG_CMSG_CLOEXEC | libc::MSG_DONTWAIT) };
    if count < 0 {
        let e = io::Error::last_os_error();
        return if e.kind() == io::ErrorKind::WouldBlock {
            Ok(None)
        } else {
            Err(e)
        };
    }
    let mut rights = Vec::new();
    let mut invalid = false;
    // SAFETY: CMSG traversal is bounded by the kernel-returned control length. Every received FD immediately receives an owner.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&msg);
        while !header.is_null() {
            if (*header).cmsg_level != libc::SOL_SOCKET || (*header).cmsg_type != libc::SCM_RIGHTS {
                invalid = true;
            } else {
                let base = libc::CMSG_LEN(0) as usize;
                if (*header).cmsg_len < base {
                    invalid = true;
                    break;
                }
                let len = (*header).cmsg_len - base;
                if !len.is_multiple_of(mem::size_of::<RawFd>()) {
                    invalid = true;
                }
                for i in 0..len / mem::size_of::<RawFd>() {
                    rights.push(OwnedFd::from_raw_fd(
                        *libc::CMSG_DATA(header).cast::<RawFd>().add(i),
                    ));
                }
            }
            header = libc::CMSG_NXTHDR(&msg, header);
        }
    }
    if count == 0
        && !invalid
        && rights.is_empty()
        && msg.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) == 0
    {
        let mut state = libc::pollfd {
            fd,
            events: libc::POLLRDHUP,
            revents: 0,
        };
        // SAFETY: poll only inspects this borrowed socket; zero timeout cannot block.
        if unsafe { libc::poll(&mut state, 1, 0) } > 0
            && state.revents & (libc::POLLRDHUP | libc::POLLHUP) != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stage peer closed",
            ));
        }
    }
    if count == 0
        || invalid
        || rights.len() > 3
        || msg.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0
    {
        return Err(io::Error::other("invalid stage packet"));
    }
    let value = serde_json::from_slice(&bytes[..count as usize])
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(Some((value, rights)))
}

#[derive(Clone, PartialEq, Eq)]
struct SourceStamp {
    device: u64,
    inode: u64,
    size: u64,
    mtime: i64,
    mtime_ns: i64,
    ctime: i64,
    ctime_ns: i64,
}
impl SourceStamp {
    fn read(file: &File) -> io::Result<Self> {
        let m = file.metadata()?;
        Ok(Self {
            device: m.dev(),
            inode: m.ino(),
            size: m.len(),
            mtime: m.mtime(),
            mtime_ns: m.mtime_nsec(),
            ctime: m.ctime(),
            ctime_ns: m.ctime_nsec(),
        })
    }
}

struct Endpoint {
    directory: File,
    identity: FileIdentity,
    uid: u32,
}
impl Endpoint {
    fn bind(directory: File) -> io::Result<(Self, OwnedFd)> {
        let alias = PathBuf::from(format!(
            "/proc/self/fd/{}/stage.sock",
            directory.as_raw_fd()
        ));
        let listener = socket()?;
        let (addr, len) = address(&alias)?;
        // SAFETY: the retained directory descriptor resolves the short alias into the owned control directory.
        if unsafe {
            libc::bind(
                listener.as_raw_fd(),
                (&addr as *const libc::sockaddr_un).cast(),
                len,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        fs::set_permissions(&alias, fs::Permissions::from_mode(0o666))?;
        // SAFETY: this is a bound seqpacket socket; only the authenticated bootstrap is admitted.
        if unsafe { libc::listen(listener.as_raw_fd(), 1) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let meta = fs::symlink_metadata(alias)?;
        Ok((
            Self {
                directory,
                identity: FileIdentity {
                    device: meta.dev(),
                    inode: meta.ino(),
                },
                uid: meta.uid(),
            },
            listener,
        ))
    }
    fn unlink(&self) -> io::Result<()> {
        let alias = PathBuf::from(format!(
            "/proc/self/fd/{}/stage.sock",
            self.directory.as_raw_fd()
        ));
        let meta = fs::symlink_metadata(alias)?;
        if !meta.file_type().is_socket()
            || meta.uid() != self.uid
            || meta.dev() != self.identity.device
            || meta.ino() != self.identity.inode
        {
            return Err(io::Error::other("stage socket identity changed"));
        }
        // SAFETY: unlinkat resolves the fixed child name only beneath our held directory; it does not follow the socket.
        if unsafe { libc::unlinkat(self.directory.as_raw_fd(), c"stage.sock".as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

pub(crate) struct Session {
    listener: OwnedFd,
    pub connection: Option<OwnedFd>,
    endpoint: Endpoint,
    source: Option<File>,
    source_stamp: Option<SourceStamp>,
    snapshot_writer: Option<File>,
    retained_snapshot_writer: Option<File>,
    result_writer: Option<File>,
    pub result: File,
    pub offer: StageOffer,
    pub pid: u32,
    pidfd: Option<OwnedFd>,
    process: Option<File>,
    pub started: Instant,
}
impl Session {
    pub fn prepare(root: &Path, workspace: &Path, record: &mut Record) -> Result<Self> {
        let captured = record.film.as_mut().ok_or(ErrorCode::Uncertain)?;
        let storage = workspace.join("work");
        for (name, uid, mode) in [
            ("input", 0, 0o755),
            ("work", 1000, 0o700),
            ("output", 1000, 0o700),
            ("native", 0, 0o700),
        ] {
            let path = storage.join(name);
            fs::create_dir(&path).map_err(fail)?;
            let file = File::open(&path).map_err(fail)?;
            // SAFETY: descriptor identifies a newly created directory on the owned capped tmpfs.
            if unsafe { libc::fchown(file.as_raw_fd(), uid, uid) } != 0 {
                return Err(ErrorCode::Uncertain);
            }
            file.set_permissions(fs::Permissions::from_mode(mode))
                .map_err(fail)?;
        }
        let create = |path: &Path, bytes: &[u8]| -> Result<File> {
            let mut f = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)
                .map_err(fail)?;
            f.write_all(bytes)
                .and_then(|_| f.sync_all())
                .map_err(fail)?;
            f.set_permissions(fs::Permissions::from_mode(0o444))
                .map_err(fail)?;
            Ok(f)
        };
        let grant_bytes = film::canonical(&captured.grant)?;
        if grant_bytes.len() > film::FRAME {
            return Err(ErrorCode::InvalidRequest);
        }
        let grant = create(&storage.join("input/grant.json"), &grant_bytes)?;
        captured.grant_file = Some(identity(&grant).map_err(fail)?);
        drop(grant);
        let result_writer = create(&storage.join("native/result"), &[0u8; film::FRAME])?;
        captured.result_file = Some(identity(&result_writer).map_err(fail)?);
        let result = File::open(storage.join("native/result")).map_err(fail)?;
        let (source, stamp, snapshot_writer) =
            if let film::Source::DevelopmentTiff { bytes, .. } = &captured.grant.fixture.source {
                let rootfd = File::open(root).map_err(fail)?;
                let file = safe_open(
                    &rootfd,
                    &format!("fixtures/{}.tif", captured.grant.fixture.id),
                    libc::O_RDONLY,
                )
                .map_err(|_| {
                    captured.detail = Some(film::Detail::SourceMismatch);
                    ErrorCode::Unavailable
                })?;
                let m = file.metadata().map_err(fail)?;
                if !m.is_file()
                    || m.uid() != 0
                    || m.nlink() != 1
                    || m.mode() & 0o022 != 0
                    || m.len() != *bytes
                {
                    captured.detail = Some(film::Detail::SourceMismatch);
                    return Err(ErrorCode::Unavailable);
                }
                let stamp = SourceStamp::read(&file).map_err(fail)?;
                let writer = create(&storage.join("input/input.tif"), &[])?;
                captured.snapshot = Some(identity(&writer).map_err(fail)?);
                (Some(file), Some(stamp), Some(writer))
            } else {
                (None, None, None)
            };
        fs::set_permissions(storage.join("input"), fs::Permissions::from_mode(0o555))
            .map_err(fail)?;
        fs::set_permissions(&storage, fs::Permissions::from_mode(0o555)).map_err(fail)?;
        let control = File::open(workspace.join("control")).map_err(fail)?;
        let (endpoint, listener) = Endpoint::bind(control).map_err(fail)?;
        let source_sha256 = match &captured.grant.fixture.source {
            film::Source::DevelopmentTiff { sha256, .. } => Some(sha256.clone()),
            _ => None,
        };
        let result_identity = captured.result_file.as_ref().ok_or(ErrorCode::Uncertain)?;
        let offer = StageOffer {
            version: 2,
            kind: "stage-offer".into(),
            launch_id: record.launch_id.clone(),
            source: if source.is_some() {
                "development-tiff"
            } else {
                "synthetic-rgb"
            }
            .into(),
            source_bytes: captured.grant.fixture.source_bytes(),
            source_sha256,
            destination_device: captured.snapshot.as_ref().map(|s| s.device),
            destination_inode: captured.snapshot.as_ref().map(|s| s.inode),
            result_device: result_identity.device,
            result_inode: result_identity.inode,
            grant_sha256: digest(&grant_bytes),
        };
        Ok(Self {
            listener,
            connection: None,
            endpoint,
            source,
            source_stamp: stamp,
            snapshot_writer,
            retained_snapshot_writer: None,
            result_writer: Some(result_writer),
            result,
            offer,
            pid: 0,
            pidfd: None,
            process: None,
            started: Instant::now(),
        })
    }
    pub fn accept(&mut self, pid: u32) -> Result<bool> {
        // SAFETY: accept4 returns an owned CLOEXEC descriptor from the private listening endpoint.
        let fd = unsafe {
            libc::accept4(
                self.listener.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            )
        };
        if fd < 0 {
            return if io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
                Ok(false)
            } else {
                Err(ErrorCode::Uncertain)
            };
        }
        // SAFETY: fd is fresh from accept4.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: ucred is integer storage and getsockopt receives its exact size.
        let mut peer: libc::ucred = unsafe { mem::zeroed() };
        let mut len = mem::size_of_val(&peer) as libc::socklen_t;
        // SAFETY: pointers refer to writable initialized ucred and length.
        if unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut peer as *mut libc::ucred).cast(),
                &mut len,
            )
        } != 0
            || peer.uid != 1000
            || peer.pid as u32 != pid
        {
            return Err(ErrorCode::Unauthorized);
        }
        // SAFETY: pidfd_open targets the already authenticated expected bootstrap; cgroup identity is rechecked by the caller.
        let pfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if pfd < 0 {
            return Err(ErrorCode::Uncertain);
        }
        // SAFETY: successful pidfd_open returned a fresh descriptor.
        self.pidfd = Some(unsafe { OwnedFd::from_raw_fd(pfd as i32) });
        self.process = Some(File::open(format!("/proc/{pid}")).map_err(fail)?);
        self.pid = pid;
        self.connection = Some(fd);
        Ok(true)
    }
    pub fn unlink_endpoint(&self) -> Result<()> {
        self.endpoint.unlink().map_err(fail)
    }
    pub fn offer(&mut self) -> Result<()> {
        let mut rights = Vec::new();
        if let Some(f) = &self.source {
            rights.push(f.as_raw_fd());
            rights.push(
                self.snapshot_writer
                    .as_ref()
                    .ok_or(ErrorCode::Uncertain)?
                    .as_raw_fd(),
            );
        }
        rights.push(
            self.result_writer
                .as_ref()
                .ok_or(ErrorCode::Uncertain)?
                .as_raw_fd(),
        );
        send(
            self.connection
                .as_ref()
                .ok_or(ErrorCode::Uncertain)?
                .as_raw_fd(),
            &self.offer,
            &rights,
        )
        .map_err(fail)?;
        drop(self.snapshot_writer.take());
        drop(self.result_writer.take());
        Ok(())
    }
    pub fn retain_snapshot_writer(&mut self) -> Result<()> {
        if self.retained_snapshot_writer.is_some() {
            return Err(ErrorCode::Uncertain);
        }
        self.retained_snapshot_writer = Some(
            self.snapshot_writer
                .as_ref()
                .ok_or(ErrorCode::Uncertain)?
                .try_clone()
                .map_err(fail)?,
        );
        Ok(())
    }
    pub fn audit(&mut self, record: &Record, leaf: &Path) -> Result<()> {
        let pfd = self.pidfd.as_ref().ok_or(ErrorCode::Uncertain)?;
        let mut poll = libc::pollfd {
            fd: pfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll reads a single owned pidfd without waiting.
        if unsafe { libc::poll(&mut poll, 1, 0) } != 0
            || fs::read_to_string(leaf.join("cgroup.procs"))
                .map_err(fail)?
                .trim()
                != self.pid.to_string()
        {
            return Err(ErrorCode::Uncertain);
        }
        if let (Some(source), Some(stamp)) = (&self.source, &self.source_stamp)
            && SourceStamp::read(source).map_err(fail)? != *stamp
        {
            return Err(ErrorCode::InvalidRequest);
        }
        let captured = record.film.as_ref().ok_or(ErrorCode::Uncertain)?;
        if let Some(snapshot) = &captured.snapshot {
            for (base, may_close) in [
                (PathBuf::from("/proc/self"), true),
                (
                    PathBuf::from(format!(
                        "/proc/self/fd/{}",
                        self.process
                            .as_ref()
                            .ok_or(ErrorCode::Uncertain)?
                            .as_raw_fd()
                    )),
                    false,
                ),
            ] {
                audit_snapshot_writers(&base, snapshot, may_close)?;
            }
        }
        drop(self.source.take());
        Ok(())
    }
}
fn process_text(path: &Path, limit: u64) -> Result<String> {
    let mut text = String::new();
    File::open(path)
        .map_err(fail)?
        .take(limit + 1)
        .read_to_string(&mut text)
        .map_err(fail)?;
    if text.len() as u64 > limit {
        return Err(ErrorCode::Uncertain);
    }
    Ok(text)
}
fn audit_snapshot_writers(base: &Path, snapshot: &FileIdentity, may_close: bool) -> Result<()> {
    for (count, entry) in fs::read_dir(base.join("fd")).map_err(fail)?.enumerate() {
        if count >= 4096 {
            return Err(ErrorCode::Uncertain);
        }
        let entry = entry.map_err(fail)?;
        let meta = match fs::metadata(entry.path()) {
            Ok(meta) => meta,
            Err(error)
                if may_close
                    && error.kind() == io::ErrorKind::NotFound
                    && fs::symlink_metadata(entry.path())
                        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(_) => return Err(ErrorCode::Uncertain),
        };
        if meta.dev() == snapshot.device && meta.ino() == snapshot.inode {
            let info = process_text(&base.join("fdinfo").join(entry.file_name()), 16 * 1024)?;
            let flags = info
                .lines()
                .find_map(|s| s.strip_prefix("flags:\t"))
                .and_then(|s| u32::from_str_radix(s, 8).ok())
                .ok_or(ErrorCode::Uncertain)?;
            if flags & libc::O_ACCMODE as u32 != libc::O_RDONLY as u32 {
                return Err(ErrorCode::InvalidRequest);
            }
        }
    }
    let maps = process_text(&base.join("maps"), 1024 * 1024)?;
    for line in maps.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 5 {
            return Err(ErrorCode::Uncertain);
        }
        let (major, minor) = fields[3].split_once(':').ok_or(ErrorCode::Uncertain)?;
        let major = u32::from_str_radix(major, 16).map_err(|_| ErrorCode::Uncertain)?;
        let minor = u32::from_str_radix(minor, 16).map_err(|_| ErrorCode::Uncertain)?;
        let inode = fields[4].parse::<u64>().map_err(|_| ErrorCode::Uncertain)?;
        if fields[1].contains('w')
            && fields[1].ends_with('s')
            && inode == snapshot.inode
            && major == libc::major(snapshot.device)
            && minor == libc::minor(snapshot.device)
        {
            return Err(ErrorCode::InvalidRequest);
        }
    }
    Ok(())
}
pub(crate) fn read_result(file: &File, grant: &EngineGrant) -> Result<Option<film::WorkerResult>> {
    use std::os::unix::fs::FileExt;
    let mut bytes = [0u8; film::FRAME];
    file.read_exact_at(&mut bytes, 0).map_err(fail)?;
    let size =
        u32::from_be_bytes(bytes[..4].try_into().map_err(|_| ErrorCode::Uncertain)?) as usize;
    if size == 0 {
        return Ok(None);
    }
    if size > film::FRAME - 4 || bytes[4 + size..].iter().any(|b| *b != 0) {
        return Err(ErrorCode::InvalidRequest);
    }
    let result: film::WorkerResult = film::parse(&bytes[4..4 + size], film::FRAME - 4)?;
    result.validate(grant)?;
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> (OwnedFd, OwnedFd) {
        let mut sockets = [0; 2];
        // SAFETY: socketpair initializes exactly two fresh descriptors.
        assert_eq!(
            unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                    0,
                    sockets.as_mut_ptr(),
                )
            },
            0
        );
        // SAFETY: each fresh descriptor receives one owner.
        unsafe {
            (
                OwnedFd::from_raw_fd(sockets[0]),
                OwnedFd::from_raw_fd(sockets[1]),
            )
        }
    }
    #[test]
    fn sealing_audit_rejects_live_duplicate_writers_and_shared_writable_mappings() {
        let path =
            std::env::temp_dir().join(format!("slipstream-stage-writer-{}", std::process::id()));
        let writer = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        writer.set_len(4096).unwrap();
        let snapshot = identity(&writer).unwrap();
        let duplicate = writer.try_clone().unwrap();
        drop(writer);
        assert_eq!(
            audit_snapshot_writers(Path::new("/proc/self"), &snapshot, true),
            Err(ErrorCode::InvalidRequest)
        );
        // SAFETY: the live writable file contains one page; the resulting mapping is unmapped below.
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                duplicate.as_raw_fd(),
                0,
            )
        };
        assert_ne!(mapping, libc::MAP_FAILED);
        drop(duplicate);
        assert_eq!(
            audit_snapshot_writers(Path::new("/proc/self"), &snapshot, true),
            Err(ErrorCode::InvalidRequest)
        );
        // SAFETY: mapping and length match the successful mmap above.
        assert_eq!(unsafe { libc::munmap(mapping, 4096) }, 0);
        let reader = File::open(&path).unwrap();
        assert!(audit_snapshot_writers(Path::new("/proc/self"), &snapshot, true).is_ok());
        drop(reader);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn unavailable_descriptor_inventory_never_proves_a_definite_sealing_violation() {
        let path = std::env::temp_dir().join(format!(
            "slipstream-stage-observation-{}",
            std::process::id()
        ));
        fs::create_dir_all(path.join("fd")).unwrap();
        fs::write(path.join("maps"), "").unwrap();
        std::os::unix::fs::symlink("missing", path.join("fd/3")).unwrap();
        let snapshot = FileIdentity {
            device: 1,
            inode: 1,
        };
        for may_close in [true, false] {
            assert_eq!(
                audit_snapshot_writers(&path, &snapshot, may_close),
                Err(ErrorCode::Uncertain)
            );
        }
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn peer_eof_is_distinct_from_an_empty_or_malformed_connected_packet() {
        let (sender, receiver) = pair();
        // SAFETY: send permits a null buffer for a zero-length message.
        assert_eq!(
            unsafe { libc::send(sender.as_raw_fd(), std::ptr::null(), 0, 0) },
            0
        );
        assert_ne!(
            receive::<film::Permit>(receiver.as_raw_fd())
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        let invalid = b"{";
        // SAFETY: the borrowed socket and single-byte buffer are live.
        assert_eq!(
            unsafe {
                libc::send(
                    sender.as_raw_fd(),
                    invalid.as_ptr().cast(),
                    invalid.len(),
                    0,
                )
            },
            1
        );
        assert_ne!(
            receive::<film::Permit>(receiver.as_raw_fd())
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        drop(sender);
        assert_eq!(
            receive::<film::Permit>(receiver.as_raw_fd())
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
    #[test]
    fn stage_rights_are_received_close_on_exec_and_packet_boundaries_are_exact() {
        let (a, b) = pair();
        let source = File::open("/dev/null").unwrap();
        let packet = film::Permit {
            version: 2,
            kind: "engine-permit".into(),
            launch_id: "a".repeat(32),
            grant_sha256: "b".repeat(64),
        };
        send(a.as_raw_fd(), &packet, &[source.as_raw_fd()]).unwrap();
        let (received, rights) = receive::<film::Permit>(b.as_raw_fd()).unwrap().unwrap();
        assert_eq!(received, packet);
        assert_eq!(rights.len(), 1);
        // SAFETY: F_GETFD only inspects the received owned descriptor.
        assert_ne!(
            unsafe { libc::fcntl(rights[0].as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        assert!(send(a.as_raw_fd(), &packet, &[source.as_raw_fd(); 4]).is_err());
        let large = vec![b'x'; film::FRAME + 1];
        // SAFETY: socket and byte slice remain valid for this bounded oversized test packet.
        assert_eq!(
            unsafe { libc::send(a.as_raw_fd(), large.as_ptr().cast(), large.len(), 0) },
            large.len() as isize
        );
        assert!(receive::<film::Permit>(b.as_raw_fd()).is_err());
    }
    #[test]
    fn confined_open_refuses_symlinks_and_parent_traversal() {
        let root =
            std::env::temp_dir().join(format!("slipstream-film-open-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("link")).unwrap();
        let directory = File::open(&root).unwrap();
        assert!(safe_open(&directory, "link", libc::O_RDONLY).is_err());
        assert!(safe_open(&directory, "../passwd", libc::O_RDONLY).is_err());
        fs::remove_file(root.join("link")).unwrap();
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn long_control_paths_bind_through_a_held_directory_and_unlink_only_owned_socket() {
        let root =
            std::env::temp_dir().join(format!("slipstream-stage-long-{}", std::process::id()));
        for length in [61usize, 180] {
            let control = root
                .join("x".repeat(length))
                .join("attempts")
                .join("f".repeat(32))
                .join("control");
            fs::create_dir_all(&control).unwrap();
            assert!(control.join("stage.sock").as_os_str().len() > 108);
            let directory = File::open(&control).unwrap();
            let (endpoint, listener) = Endpoint::bind(directory).unwrap();
            assert_eq!(
                fs::symlink_metadata(control.join("stage.sock"))
                    .unwrap()
                    .ino(),
                endpoint.identity.inode
            );
            let alias = PathBuf::from(format!(
                "/proc/self/fd/{}/stage.sock",
                endpoint.directory.as_raw_fd()
            ));
            let client = connect(&alias).unwrap();
            drop(client);
            endpoint.unlink().unwrap();
            assert!(!control.join("stage.sock").exists());
            drop(listener);
            let (first, one) = Endpoint::bind(File::open(&control).unwrap()).unwrap();
            fs::rename(control.join("stage.sock"), control.join("old.sock")).unwrap();
            let (second, two) = Endpoint::bind(File::open(&control).unwrap()).unwrap();
            assert!(first.unlink().is_err());
            assert!(control.join("stage.sock").exists());
            second.unlink().unwrap();
            drop(one);
            drop(two);
            fs::remove_file(control.join("old.sock")).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }
}
