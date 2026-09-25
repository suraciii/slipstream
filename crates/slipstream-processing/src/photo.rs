//! Production Photo admission transport and bounded descriptor helpers.
//!
//! This module is deliberately separate from the fixture staging protocol.
//! Fixture staging must continue to reject descriptors on its public socket.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    mem,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{ffi::OsStrExt, fs::PermissionsExt},
    },
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use crate::protocol::{
    Availability, ErrorCode, PHOTO_MODE, PHOTO_PROTOCOL_VERSION, PHOTO_WORKLOAD, REQUEST_BYTES,
    RESPONSE_BYTES,
};

/// The bundle-pinned ICC asset bytes shipped inside the worker image. The
/// output contract pins these asset bytes and the exact embedded profile
/// bytes separately (`design/development-color.md`).
pub const ICC_ASSET_SHA256: &str =
    "df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed";

/// The production transport is a datagram-like Unix socket. A packet is never
/// split into a length header and a body, so a descriptor stays attached to
/// the operation that authorizes it.
pub const FRAME_BYTES: usize = REQUEST_BYTES;
const CONTROL_BYTES: usize = 128;
// These match the current 4 GiB streaming-fingerprint and Export artifact
// limits. The launcher crate stays independent of the Library implementation.
// They also bound the sizes a production request may declare, so a packet can
// never describe more bytes than the bounded application seams accept.
const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Root-owned configuration for the separate production Photo listener.
/// This does not make a worker available; it binds the identities and finite
/// policy which the later admitted worker path must verify.
/// Production qualification has not set storage, inode, memory, CPU or task
/// ceilings. Those fields receive finite-shape checks only; parsing them cannot
/// enable admission.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u8,
    pub mode: String,
    pub instance: String,
    pub root: String,
    pub socket: String,
    pub peer_uid: u32,
    pub image: String,
    pub bundle: String,
    pub policy: String,
    pub source_bytes_max: u64,
    pub staged_storage_bytes_max: u64,
    pub staged_storage_inodes_max: u64,
    pub output_bytes_max: u64,
    pub memory_bytes: u64,
    pub cpu_quota_us: u64,
    pub tasks: u32,
    pub swap_bytes: u64,
    pub control_reserve_bytes: u64,
    pub shared_ancestor_headroom_bytes: u64,
    pub receipt_retention_seconds: u64,
}

impl Config {
    pub fn parse(bytes: &[u8]) -> Result<Self, ErrorCode> {
        if bytes.is_empty() || bytes.len() > FRAME_BYTES {
            return Err(ErrorCode::InvalidRequest);
        }
        let config: Self = serde_json::from_slice(bytes).map_err(|_| ErrorCode::InvalidRequest)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ErrorCode> {
        let root = format!("/var/lib/slipstream-processing/{}", self.instance);
        let socket = format!("/run/slipstream-processing/{}/launcher.sock", self.instance);
        let image_digest = self.image.strip_prefix("sha256:").or_else(|| {
            let (repository, digest) = self.image.split_once("@sha256:")?;
            (!repository.is_empty()
                && repository.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"/._-:".contains(&byte)
                }))
            .then_some(digest)
        });
        if self.version != PHOTO_PROTOCOL_VERSION
            || self.mode != PHOTO_MODE
            || !hex(&self.instance, 32)
            || self.root != root
            || self.socket != socket
            || self.peer_uid != 1000
            || !image_digest.is_some_and(|digest| hex(digest, 64))
            || !hex(&self.bundle, 64)
            || !hex(&self.policy, 64)
            || !finite_positive(self.source_bytes_max)
            || self.source_bytes_max > MAX_SOURCE_BYTES
            || !finite_positive(self.staged_storage_bytes_max)
            || self.staged_storage_bytes_max
                < self.source_bytes_max.saturating_add(self.output_bytes_max)
            || !finite_positive(self.staged_storage_inodes_max)
            || !finite_positive(self.output_bytes_max)
            || self.output_bytes_max > MAX_OUTPUT_BYTES
            || !finite_positive(self.memory_bytes)
            || !finite_positive(self.cpu_quota_us)
            || self.tasks == 0
            || self.tasks == u32::MAX
            || self.swap_bytes != 0
            || !finite_positive(self.control_reserve_bytes)
            || !finite_positive(self.shared_ancestor_headroom_bytes)
            || !(1..=604_800).contains(&self.receipt_retention_seconds)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

fn finite_positive(value: u64) -> bool {
    value != 0 && value != u64::MAX
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub kind: String,
    pub profile_id: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub exposure_milli_ev: i64,
    pub white_balance_mode: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    Reconcile {
        mode: String,
        version: u8,
        instance: String,
    },
    Start {
        mode: String,
        version: u8,
        instance: String,
        export_id: String,
        incarnation: String,
        sequence: u64,
        policy: String,
        bundle: String,
        workload: String,
        source: Source,
        recipe: Recipe,
        recipe_digest: String,
        manifest_sha256: String,
    },
    Output {
        mode: String,
        version: u8,
        instance: String,
        export_id: String,
        incarnation: String,
        sequence: u64,
        target: String,
    },
    ValidateOutput {
        mode: String,
        version: u8,
        instance: String,
        export_id: String,
        incarnation: String,
        sequence: u64,
        target: String,
        size: u64,
        sha256: String,
        accepted: bool,
    },
    Inspect {
        mode: String,
        version: u8,
        instance: String,
        export_id: String,
        incarnation: String,
        sequence: u64,
    },
    Cancel {
        mode: String,
        version: u8,
        instance: String,
        export_id: String,
        incarnation: String,
        sequence: u64,
    },
}

impl Request {
    pub fn reconcile(instance: impl Into<String>) -> Self {
        Self::Reconcile {
            mode: PHOTO_MODE.into(),
            version: PHOTO_PROTOCOL_VERSION,
            instance: instance.into(),
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, ErrorCode> {
        if bytes.is_empty() || bytes.len() > FRAME_BYTES {
            return Err(ErrorCode::InvalidRequest);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| ErrorCode::InvalidRequest)?;
        value.validate()?;
        Ok(value)
    }

    pub fn bytes(&self) -> Result<Vec<u8>, ErrorCode> {
        self.validate().map_err(|_| ErrorCode::InvalidRequest)?;
        let bytes = serde_json::to_vec(self).map_err(|_| ErrorCode::InvalidRequest)?;
        if bytes.len() > FRAME_BYTES {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(bytes)
    }

    pub fn expected_rights(&self) -> usize {
        match self {
            Self::Start { .. } | Self::Output { .. } => 1,
            Self::Reconcile { .. }
            | Self::ValidateOutput { .. }
            | Self::Inspect { .. }
            | Self::Cancel { .. } => 0,
        }
    }

    fn validate(&self) -> Result<(), ErrorCode> {
        let (mode, version, instance) = match self {
            Self::Reconcile {
                mode,
                version,
                instance,
            }
            | Self::Start {
                mode,
                version,
                instance,
                ..
            }
            | Self::Output {
                mode,
                version,
                instance,
                ..
            }
            | Self::ValidateOutput {
                mode,
                version,
                instance,
                ..
            }
            | Self::Inspect {
                mode,
                version,
                instance,
                ..
            }
            | Self::Cancel {
                mode,
                version,
                instance,
                ..
            } => (mode, version, instance),
        };
        if mode != PHOTO_MODE || *version != PHOTO_PROTOCOL_VERSION || !hex(instance, 32) {
            return Err(ErrorCode::InvalidRequest);
        }
        match self {
            Self::Reconcile { .. } => {}
            Self::Start {
                export_id,
                incarnation,
                sequence,
                policy,
                bundle,
                workload,
                source,
                recipe,
                recipe_digest,
                manifest_sha256,
                ..
            } => {
                if !identifier(export_id, 128)
                    || !hex(incarnation, 32)
                    || *sequence == 0
                    || !hex(policy, 64)
                    || !hex(bundle, 64)
                    || workload != PHOTO_WORKLOAD
                    || source.kind != "raw"
                    || !identifier(&source.profile_id, 64)
                    || source.size == 0
                    || source.size > MAX_SOURCE_BYTES
                    || !hex(&source.sha256, 64)
                    || recipe.white_balance_mode != "as-shot"
                    || !hex(recipe_digest, 64)
                    || !hex(manifest_sha256, 64)
                {
                    return Err(ErrorCode::InvalidRequest);
                }
            }
            Self::Output {
                export_id,
                incarnation,
                sequence,
                target,
                ..
            } => {
                if !identifier(export_id, 128)
                    || !hex(incarnation, 32)
                    || *sequence == 0
                    || target != PHOTO_WORKLOAD
                {
                    return Err(ErrorCode::InvalidRequest);
                }
            }
            Self::ValidateOutput {
                export_id,
                incarnation,
                sequence,
                target,
                size,
                sha256,
                ..
            } => {
                if !identifier(export_id, 128)
                    || !hex(incarnation, 32)
                    || *sequence == 0
                    || target != PHOTO_WORKLOAD
                    || *size == 0
                    || *size > MAX_OUTPUT_BYTES
                    || !hex(sha256, 64)
                {
                    return Err(ErrorCode::InvalidRequest);
                }
            }
            Self::Inspect {
                export_id,
                incarnation,
                sequence,
                ..
            }
            | Self::Cancel {
                export_id,
                incarnation,
                sequence,
                ..
            } => {
                if !identifier(export_id, 128) || !hex(incarnation, 32) || *sequence == 0 {
                    return Err(ErrorCode::InvalidRequest);
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PhotoReceipt {
    pub export_id: String,
    pub incarnation: String,
    pub sequence: u64,
    pub workload: String,
    pub policy: String,
    pub bundle: String,
    pub state: String,
    pub outcome: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OutputReceipt {
    pub export_id: String,
    pub incarnation: String,
    pub sequence: u64,
    pub target: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ResultBody {
    Capability {
        capability: String,
        instance: String,
        incarnation: String,
        next_sequence: u64,
        policy: String,
        bundle: String,
        availability: Availability,
        active: Option<PhotoReceipt>,
    },
    Receipt {
        receipt: PhotoReceipt,
    },
    Output {
        receipt: OutputReceipt,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    pub code: ErrorCode,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(untagged, deny_unknown_fields)]
pub enum Response {
    Result {
        mode: String,
        version: u8,
        result: Box<ResultBody>,
    },
    Error {
        mode: String,
        version: u8,
        error: ErrorResponse,
    },
}

impl Response {
    pub fn result(result: ResultBody) -> Self {
        Self::Result {
            mode: PHOTO_MODE.into(),
            version: PHOTO_PROTOCOL_VERSION,
            result: Box::new(result),
        }
    }

    pub fn error(code: ErrorCode) -> Self {
        Self::Error {
            mode: PHOTO_MODE.into(),
            version: PHOTO_PROTOCOL_VERSION,
            error: ErrorResponse { code },
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, ErrorCode> {
        if bytes.is_empty() || bytes.len() > RESPONSE_BYTES {
            return Err(ErrorCode::InvalidRequest);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| ErrorCode::InvalidRequest)?;
        let valid = match &value {
            Self::Result { mode, version, .. } | Self::Error { mode, version, .. } => {
                mode == PHOTO_MODE && *version == PHOTO_PROTOCOL_VERSION
            }
        };
        valid.then_some(value).ok_or(ErrorCode::InvalidRequest)
    }

    fn bytes(&self) -> io::Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        if bytes.len() > RESPONSE_BYTES {
            return Err(invalid("photo response exceeds frame limit"));
        }
        Ok(bytes)
    }
}

/// Reconcile without a descriptor. This is the only public convenience
/// request until the launcher admission path is wired to the Photo executor.
pub fn reconcile(
    socket: impl AsRef<Path>,
    instance: impl Into<String>,
) -> Result<Response, ErrorCode> {
    let request = Request::reconcile(instance);
    request_socket(socket.as_ref(), &request)
}

/// Send a request with the exact descriptor count required by its operation.
pub fn send_request(fd: RawFd, request: &Request, descriptor: Option<RawFd>) -> io::Result<()> {
    let rights = descriptor.into_iter().collect::<Vec<_>>();
    if rights.len() != request.expected_rights() {
        return Err(invalid("photo operation has the wrong descriptor count"));
    }
    let bytes = request
        .bytes()
        .map_err(|_| invalid("invalid photo request"))?;
    send_packet(fd, &bytes, &rights)
}

pub fn send_response(fd: RawFd, response: &Response) -> io::Result<()> {
    send_packet(fd, &response.bytes()?, &[])
}

/// Receive and validate one Photo request. Unexpected or missing rights are
/// dropped before returning an error.
pub fn receive_request(fd: RawFd) -> io::Result<(Request, Option<OwnedFd>)> {
    let (bytes, rights) = receive_packet(fd)?;
    let request = Request::parse(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid photo request"))?;
    if rights.len() != request.expected_rights() {
        drop(rights);
        return Err(invalid("photo operation has the wrong descriptor count"));
    }
    Ok((request, rights.into_iter().next()))
}

pub fn receive_response(fd: RawFd) -> io::Result<Response> {
    let (response, descriptor) = receive_response_with_descriptor(fd)?;
    drop(descriptor);
    Ok(response)
}

/// Receive one response and return a transferred descriptor with it. More
/// than one descriptor is refused; the protocol response carries at most one.
pub fn receive_response_with_descriptor(fd: RawFd) -> io::Result<(Response, Option<OwnedFd>)> {
    let (bytes, rights) = receive_packet(fd)?;
    if rights.len() > 1 {
        drop(rights);
        return Err(invalid("photo responses cannot carry multiple descriptors"));
    }
    let response = Response::parse(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid photo response"))?;
    Ok((response, rights.into_iter().next()))
}

pub fn request_socket(socket: &Path, request: &Request) -> Result<Response, ErrorCode> {
    if request.expected_rights() != 0 {
        return Err(ErrorCode::InvalidRequest);
    }
    let fd = connect(socket).map_err(|_| ErrorCode::Unavailable)?;
    send_request(fd.as_raw_fd(), request, None).map_err(|_| ErrorCode::Unavailable)?;
    receive_response(fd.as_raw_fd()).map_err(|_| ErrorCode::Unavailable)
}

/// Send one request that carries exactly one descriptor, such as Start with
/// the staged source or Output with the service output file, and receive the
/// response over a new connection.
pub fn request_with_descriptor(
    socket: impl AsRef<Path>,
    request: &Request,
    descriptor: RawFd,
) -> Result<Response, ErrorCode> {
    if request.expected_rights() != 1 {
        return Err(ErrorCode::InvalidRequest);
    }
    let fd = connect(socket.as_ref()).map_err(|_| ErrorCode::Unavailable)?;
    send_request(fd.as_raw_fd(), request, Some(descriptor)).map_err(|_| ErrorCode::Unavailable)?;
    receive_response(fd.as_raw_fd()).map_err(|_| ErrorCode::Unavailable)
}

/// Start the root-owned production Photo seqpacket listener. Reconcile
/// reports the durable capability state; work operations execute through the
/// attempt executor, which stages the received descriptor before admission.
pub fn serve(config: Config) -> Result<(), ErrorCode> {
    config.validate()?;
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        return Err(ErrorCode::Unauthorized);
    }
    let executor = crate::photo_exec::PhotoExecutor::open(config.clone())?;
    let path = Path::new(&config.socket);
    let mut claim = crate::transport::claim_socket(path, &config.instance, &config.root)?;
    let listener = bind(path).map_err(|_| ErrorCode::Unavailable)?;
    crate::transport::record_socket_claim(&mut claim, path, &config.instance, &config.root)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| ErrorCode::Unavailable)?;
    crate::transport::allow_peer(path, config.peer_uid)?;
    executor.recover_async();

    let active = Arc::new(AtomicUsize::new(0));
    loop {
        let connection = accept(listener.as_raw_fd()).map_err(|_| ErrorCode::Unavailable)?;
        if active.fetch_add(1, Ordering::AcqRel) >= 4 {
            active.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let thread_active = active.clone();
        let peer_uid = executor.config().peer_uid;
        let handler = {
            let executor = executor.clone();
            move |request: Request, descriptor: Option<File>, pid: u32| {
                executor.handle(request, descriptor, pid)
            }
        };
        if thread::Builder::new()
            .name("photo-listener-client".into())
            .spawn(move || {
                let _ = serve_connection(connection, peer_uid, &handler);
                thread_active.fetch_sub(1, Ordering::AcqRel);
            })
            .is_err()
        {
            active.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

fn serve_connection<H>(fd: OwnedFd, peer_uid: u32, handle: &H) -> io::Result<()>
where
    H: Fn(Request, Option<File>, u32) -> Result<ResultBody, ErrorCode>,
{
    set_timeouts(fd.as_raw_fd())?;
    let response = match crate::transport::peer(fd.as_raw_fd()) {
        Ok((uid, pid)) if uid == peer_uid => {
            match receive_request(fd.as_raw_fd()) {
                Ok((request, descriptor)) => {
                    // The received source or output descriptor routes into the
                    // handler; it is never dropped or re-resolved as a path.
                    let descriptor = descriptor.map(File::from);
                    match handle(request, descriptor, pid) {
                        Ok(result) => Response::result(result),
                        Err(error) => Response::error(error),
                    }
                }
                Err(_) => Response::error(ErrorCode::InvalidRequest),
            }
        }
        Ok(_) => Response::error(ErrorCode::Unauthorized),
        Err(error) => Response::error(error),
    };
    send_response(fd.as_raw_fd(), &response)
}

fn set_timeouts(fd: RawFd) -> io::Result<()> {
    let timeout = libc::timeval {
        tv_sec: 2,
        tv_usec: 0,
    };
    // SAFETY: timeout points to a valid timeval for each setsockopt call.
    for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
        if unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                (&timeout as *const libc::timeval).cast(),
                mem::size_of_val(&timeout) as libc::socklen_t,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// A source descriptor must be immutable and read-only. Output descriptors are
/// service-created empty files, so they must instead be writable and have zero
/// length before handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DescriptorKind {
    Source,
    Output,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DescriptorRequirement {
    pub kind: DescriptorKind,
    pub peer_uid: u32,
    pub declared_size: u64,
    pub max_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DescriptorMetadata {
    pub size: u64,
    pub uid: u32,
    pub nlink: u64,
    pub mode: u32,
    pub offset: u64,
}

pub fn validate_descriptor(
    fd: RawFd,
    requirement: DescriptorRequirement,
) -> io::Result<DescriptorMetadata> {
    if requirement.max_bytes == 0 || requirement.declared_size > requirement.max_bytes {
        return Err(invalid("descriptor size is outside the configured bound"));
    }
    // SAFETY: fstat writes a plain initialized stat structure for this borrowed fd.
    let mut stat: libc::stat = unsafe { mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(invalid("descriptor is not a regular file"));
    }
    let size = u64::try_from(stat.st_size).map_err(|_| invalid("negative descriptor size"))?;
    let uid = stat.st_uid;
    let nlink = stat.st_nlink;
    if uid != requirement.peer_uid || nlink != 1 {
        return Err(invalid("descriptor ownership or link count is invalid"));
    }
    // Every descriptor received by the production transport must stay closed
    // across a worker exec. MSG_CMSG_CLOEXEC supplies this bit at receive time.
    // SAFETY: fcntl reads flags from the borrowed descriptor.
    let descriptor_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if descriptor_flags < 0 || descriptor_flags & libc::FD_CLOEXEC == 0 {
        return Err(invalid("descriptor is not close-on-exec"));
    }
    // SAFETY: fcntl reads the open access mode from the borrowed descriptor.
    let status_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if status_flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let access = status_flags & libc::O_ACCMODE;
    let writable_mode = stat.st_mode & 0o222 != 0;
    match requirement.kind {
        DescriptorKind::Source => {
            if size == 0
                || size != requirement.declared_size
                || stat.st_mode & 0o222 != 0
                || access != libc::O_RDONLY
            {
                return Err(invalid(
                    "source descriptor is not immutable read-only input",
                ));
            }
        }
        DescriptorKind::Output => {
            if requirement.declared_size != 0
                || size != 0
                || !writable_mode
                || !matches!(access, libc::O_WRONLY | libc::O_RDWR)
            {
                return Err(invalid("output descriptor is not an empty writable file"));
            }
        }
    }
    // SAFETY: lseek only observes the current offset with SEEK_CUR.
    let offset = unsafe { libc::lseek(fd, 0, libc::SEEK_CUR) };
    if offset < 0 || offset != 0 {
        return Err(invalid("descriptor offset is not zero"));
    }
    Ok(DescriptorMetadata {
        size,
        uid,
        nlink,
        mode: stat.st_mode,
        offset: offset as u64,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopiedDescriptor {
    pub size: u64,
    pub sha256: String,
}

/// Copy exactly one validated source descriptor with a fixed-size buffer. The
/// helper never allocates according to the source size and refuses truncation
/// or growth around the copy.
pub fn copy_source(
    source: &mut File,
    destination: &mut File,
    peer_uid: u32,
    expected_size: u64,
    max_bytes: u64,
) -> io::Result<CopiedDescriptor> {
    validate_descriptor(
        source.as_raw_fd(),
        DescriptorRequirement {
            kind: DescriptorKind::Source,
            peer_uid,
            declared_size: expected_size,
            max_bytes,
        },
    )?;
    if destination.metadata()?.len() != 0 {
        return Err(invalid("destination is not empty"));
    }
    let mut remaining = expected_size;
    let mut copied = 0u64;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    while remaining != 0 {
        let take = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        let read = source.read(&mut buffer[..take])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "source ended before declared size",
            ));
        }
        destination.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        copied = copied
            .checked_add(read as u64)
            .ok_or_else(|| invalid("source size overflow"))?;
        remaining -= read as u64;
    }
    let mut extra = [0u8; 1];
    if source.read(&mut extra)? != 0 {
        return Err(invalid("source grew beyond declared size"));
    }
    destination.sync_all()?;
    Ok(CopiedDescriptor {
        size: copied,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn send_packet(fd: RawFd, bytes: &[u8], rights: &[RawFd]) -> io::Result<()> {
    if bytes.is_empty() || bytes.len() > FRAME_BYTES || rights.len() > 2 {
        return Err(invalid("photo packet exceeds bound"));
    }
    let mut vector = libc::iovec {
        iov_base: bytes.as_ptr() as *mut _,
        iov_len: bytes.len(),
    };
    let mut control = [0usize; CONTROL_BYTES / mem::size_of::<usize>()];
    // SAFETY: msghdr and its pointed-to buffers remain alive for sendmsg.
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    if !rights.is_empty() {
        let bytes_len = rights
            .len()
            .checked_mul(mem::size_of::<RawFd>())
            .ok_or_else(|| invalid("descriptor count overflow"))?;
        // SAFETY: control is suitably aligned and large enough for the rights payload.
        unsafe {
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = libc::CMSG_SPACE(bytes_len as _) as usize;
            let header = libc::CMSG_FIRSTHDR(&message);
            if header.is_null() {
                return Err(invalid("descriptor control buffer unavailable"));
            }
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(bytes_len as _) as usize;
            std::ptr::copy_nonoverlapping(
                rights.as_ptr(),
                libc::CMSG_DATA(header).cast::<RawFd>(),
                rights.len(),
            );
        }
    }
    // SAFETY: all pointers in message refer to live buffers and fd is borrowed.
    let sent = unsafe { libc::sendmsg(fd, &message, libc::MSG_NOSIGNAL) };
    if sent < 0 {
        Err(io::Error::last_os_error())
    } else if sent as usize != bytes.len() {
        Err(invalid("partial seqpacket send"))
    } else {
        Ok(())
    }
}

fn receive_packet(fd: RawFd) -> io::Result<(Vec<u8>, Vec<OwnedFd>)> {
    let mut bytes = [0u8; FRAME_BYTES];
    let mut control = [0usize; CONTROL_BYTES / mem::size_of::<usize>()];
    let mut vector = libc::iovec {
        iov_base: bytes.as_mut_ptr().cast(),
        iov_len: bytes.len(),
    };
    // SAFETY: msghdr and its pointed-to buffers remain alive for recvmsg.
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = mem::size_of_val(&control);
    // SAFETY: the buffers are writable and MSG_CMSG_CLOEXEC gives received fds
    // exec-safe ownership semantics.
    let received = unsafe { libc::recvmsg(fd, &mut message, libc::MSG_CMSG_CLOEXEC) };
    if received < 0 {
        return Err(io::Error::last_os_error());
    }
    if received == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "photo peer closed",
        ));
    }
    let mut rights = Vec::new();
    let mut invalid_control = false;
    // SAFETY: CMSG traversal is bounded by the kernel-provided control length;
    // each accepted raw fd is immediately wrapped in its owning type.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            let length = (*header).cmsg_len as usize;
            let base = libc::CMSG_LEN(0) as usize;
            if length < base || length > message.msg_controllen {
                invalid_control = true;
                break;
            }
            if (*header).cmsg_level != libc::SOL_SOCKET || (*header).cmsg_type != libc::SCM_RIGHTS {
                invalid_control = true;
                break;
            }
            let payload = length - base;
            if payload == 0 || !payload.is_multiple_of(mem::size_of::<RawFd>()) {
                invalid_control = true;
                break;
            }
            for index in 0..payload / mem::size_of::<RawFd>() {
                rights.push(OwnedFd::from_raw_fd(
                    *libc::CMSG_DATA(header).cast::<RawFd>().add(index),
                ));
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    if invalid_control
        || message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0
        || rights.len() > 2
    {
        drop(rights);
        return Err(invalid("invalid photo packet or descriptor control"));
    }
    Ok((bytes[..received as usize].to_vec(), rights))
}

fn socket(path_type: i32) -> io::Result<OwnedFd> {
    // SAFETY: socket returns a new descriptor owned by the caller.
    let fd = unsafe { libc::socket(libc::AF_UNIX, path_type | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: fd is freshly returned by socket.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn address(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: sockaddr_un is plain integer storage and zero is valid initialization.
    let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
    if bytes.is_empty() || bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return Err(invalid("socket path is not a bounded Unix path"));
    }
    address.sun_family = libc::AF_UNIX as _;
    for (destination, source) in address.sun_path.iter_mut().zip(bytes) {
        *destination = *source as _;
    }
    Ok((address, mem::size_of::<libc::sockaddr_un>() as _))
}

pub fn bind(path: &Path) -> io::Result<OwnedFd> {
    let listener = socket(libc::SOCK_SEQPACKET)?;
    let (address, length) = address(path)?;
    // SAFETY: address is initialized and points to the bounded path bytes.
    if unsafe {
        libc::bind(
            listener.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: listener is a bound SOCK_SEQPACKET socket.
    if unsafe { libc::listen(listener.as_raw_fd(), 4) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(listener)
}

pub fn accept(listener: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: accept4 returns a new descriptor with close-on-exec ownership.
    let fd = unsafe {
        libc::accept4(
            listener,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            libc::SOCK_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: fd is freshly returned by accept4.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn connect(path: &Path) -> io::Result<OwnedFd> {
    let fd = socket(libc::SOCK_SEQPACKET)?;
    let (address, length) = address(path)?;
    // SAFETY: address is initialized and points to the bounded path bytes.
    if unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn identifier(value: &str, maximum: usize) -> bool {
    (1..=maximum).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        fs::{self, OpenOptions},
        io::{Read, Seek},
        os::{
            fd::FromRawFd,
            unix::fs::{OpenOptionsExt, PermissionsExt},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    fn socket_pair() -> (OwnedFd, OwnedFd) {
        let mut fds = [-1; 2];
        // SAFETY: fds points to two writable integers and the socket type is valid.
        assert_eq!(
            unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                    0,
                    fds.as_mut_ptr(),
                )
            },
            0
        );
        // SAFETY: socketpair returned two fresh descriptors.
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
    }

    fn production_config() -> Config {
        Config {
            version: 1,
            mode: PHOTO_MODE.into(),
            instance: "0".repeat(32),
            root: format!("/var/lib/slipstream-processing/{}", "0".repeat(32)),
            socket: format!(
                "/run/slipstream-processing/{}/launcher.sock",
                "0".repeat(32)
            ),
            peer_uid: 1000,
            image: format!("sha256:{}", "1".repeat(64)),
            bundle: "2".repeat(64),
            policy: "3".repeat(64),
            source_bytes_max: MAX_SOURCE_BYTES,
            staged_storage_bytes_max: MAX_SOURCE_BYTES + MAX_OUTPUT_BYTES,
            staged_storage_inodes_max: 4096,
            output_bytes_max: MAX_OUTPUT_BYTES,
            memory_bytes: 8 * 1024 * 1024 * 1024,
            cpu_quota_us: 400_000,
            tasks: 256,
            swap_bytes: 0,
            control_reserve_bytes: 256 * 1024 * 1024,
            shared_ancestor_headroom_bytes: 512 * 1024 * 1024,
            receipt_retention_seconds: 86_400,
        }
    }

    fn start_request() -> Request {
        Request::Start {
            mode: PHOTO_MODE.into(),
            version: PHOTO_PROTOCOL_VERSION,
            instance: "0".repeat(32),
            export_id: "export-1".into(),
            incarnation: "d".repeat(32),
            sequence: 1,
            policy: "3".repeat(64),
            bundle: "2".repeat(64),
            workload: PHOTO_WORKLOAD.into(),
            source: Source {
                kind: "raw".into(),
                profile_id: "canon-r5".into(),
                size: 4,
                sha256: "4".repeat(64),
            },
            recipe: Recipe {
                exposure_milli_ev: 1000,
                white_balance_mode: "as-shot".into(),
            },
            recipe_digest: "5".repeat(64),
            manifest_sha256: "6".repeat(64),
        }
    }

    fn file(name: &str, bytes: &[u8], flags: i32, mode: u32) -> (std::path::PathBuf, File) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "slipstream-photo-{name}-{}-{nonce}",
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .custom_flags(flags | libc::O_CLOEXEC)
            .open(&path)
            .unwrap();
        file.set_permissions(fs::Permissions::from_mode(mode))
            .unwrap();
        if !bytes.is_empty() {
            file.write_all(bytes).unwrap();
            file.sync_all().unwrap();
        }
        drop(file);
        let reopened = OpenOptions::new()
            .read(
                (flags & libc::O_ACCMODE) == libc::O_RDONLY
                    || (flags & libc::O_ACCMODE) == libc::O_RDWR,
            )
            .write((flags & libc::O_ACCMODE) != libc::O_RDONLY)
            .custom_flags(libc::O_CLOEXEC)
            .open(&path)
            .unwrap();
        (path, reopened)
    }

    #[test]
    fn reconcile_packet_has_no_rights_and_round_trips_closed_envelope() {
        let (sender, receiver) = socket_pair();
        let request = Request::reconcile("0".repeat(32));
        send_request(sender.as_raw_fd(), &request, None).unwrap();
        let (received, descriptor) = receive_request(receiver.as_raw_fd()).unwrap();
        assert_eq!(received, request);
        assert!(descriptor.is_none());
        let response = Response::result(ResultBody::Capability {
            capability: PHOTO_MODE.into(),
            instance: "0".repeat(32),
            incarnation: "1".repeat(32),
            next_sequence: 1,
            policy: "2".repeat(64),
            bundle: "3".repeat(64),
            availability: Availability::Available,
            active: None,
        });
        send_response(receiver.as_raw_fd(), &response).unwrap();
        assert_eq!(receive_response(sender.as_raw_fd()).unwrap(), response);
    }

    #[test]
    fn reconcile_with_a_descriptor_is_refused_and_right_is_dropped() {
        let (sender, receiver) = socket_pair();
        let source = File::open("/dev/null").unwrap();
        let request = Request::reconcile("0".repeat(32));
        assert!(
            send_packet(
                sender.as_raw_fd(),
                &request.bytes().unwrap(),
                &[source.as_raw_fd()]
            )
            .is_ok()
        );
        assert!(receive_request(receiver.as_raw_fd()).is_err());
        assert!(source.metadata().is_ok());
    }

    #[test]
    fn start_requires_exactly_one_right_and_output_does_too() {
        let start = Request::Start {
            mode: PHOTO_MODE.into(),
            version: PHOTO_PROTOCOL_VERSION,
            instance: "0".repeat(32),
            export_id: "export-1".into(),
            incarnation: "1".repeat(32),
            sequence: 1,
            policy: "2".repeat(64),
            bundle: "3".repeat(64),
            workload: PHOTO_WORKLOAD.into(),
            source: Source {
                kind: "raw".into(),
                profile_id: "canon-r5".into(),
                size: 4,
                sha256: "4".repeat(64),
            },
            recipe: Recipe {
                exposure_milli_ev: 1000,
                white_balance_mode: "as-shot".into(),
            },
            recipe_digest: "5".repeat(64),
            manifest_sha256: "6".repeat(64),
        };
        assert_eq!(start.expected_rights(), 1);
        let output = Request::Output {
            mode: PHOTO_MODE.into(),
            version: PHOTO_PROTOCOL_VERSION,
            instance: "0".repeat(32),
            export_id: "export-1".into(),
            incarnation: "1".repeat(32),
            sequence: 1,
            target: PHOTO_WORKLOAD.into(),
        };
        assert_eq!(output.expected_rights(), 1);
        assert!(Request::parse(&serde_json::to_vec(&start).unwrap()).is_ok());
        assert!(Request::parse(&serde_json::to_vec(&output).unwrap()).is_ok());
    }

    #[test]
    fn source_descriptor_validation_and_bounded_copy_hash_exact_bytes() {
        let (source_path, mut source) = file("source", b"raw-bytes", libc::O_RDONLY, 0o444);
        let (destination_path, mut destination) = file("destination", b"", libc::O_RDWR, 0o600);
        let uid = unsafe { libc::getuid() };
        let copied = copy_source(&mut source, &mut destination, uid, 9, 64).unwrap();
        assert_eq!(copied.size, 9);
        assert_eq!(copied.sha256, format!("{:x}", Sha256::digest(b"raw-bytes")));
        assert_eq!(fs::read(&destination_path).unwrap(), b"raw-bytes");
        source.seek(std::io::SeekFrom::Start(0)).unwrap();
        let metadata = validate_descriptor(
            source.as_raw_fd(),
            DescriptorRequirement {
                kind: DescriptorKind::Source,
                peer_uid: uid,
                declared_size: 9,
                max_bytes: 64,
            },
        )
        .unwrap();
        assert_eq!(metadata.offset, 0);
        drop(source);
        drop(destination);
        fs::remove_file(source_path).unwrap();
        fs::remove_file(destination_path).unwrap();
    }

    #[test]
    fn descriptor_validation_rejects_offset_growth_and_writable_source() {
        let (path, mut source) = file("bad-source", b"bytes", libc::O_RDWR, 0o600);
        let uid = unsafe { libc::getuid() };
        assert!(
            validate_descriptor(
                source.as_raw_fd(),
                DescriptorRequirement {
                    kind: DescriptorKind::Source,
                    peer_uid: uid,
                    declared_size: 5,
                    max_bytes: 64,
                }
            )
            .is_err()
        );
        source.seek(std::io::SeekFrom::Start(1)).unwrap();
        assert!(
            validate_descriptor(
                source.as_raw_fd(),
                DescriptorRequirement {
                    kind: DescriptorKind::Source,
                    peer_uid: uid,
                    declared_size: 5,
                    max_bytes: 64,
                }
            )
            .is_err()
        );
        drop(source);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn production_config_is_closed_and_enforces_current_authority_bounds() {
        let config = production_config();
        assert_eq!(
            Config::parse(&serde_json::to_vec(&config).unwrap()),
            Ok(config.clone())
        );

        let mut changed = config.clone();
        changed.peer_uid = 1001;
        assert_eq!(changed.validate(), Err(ErrorCode::InvalidRequest));
        changed = config.clone();
        changed.source_bytes_max = MAX_SOURCE_BYTES + 1;
        assert_eq!(changed.validate(), Err(ErrorCode::InvalidRequest));
        changed = config.clone();
        changed.output_bytes_max = MAX_OUTPUT_BYTES + 1;
        assert_eq!(changed.validate(), Err(ErrorCode::InvalidRequest));
        changed = config.clone();
        changed.staged_storage_bytes_max -= 1;
        assert_eq!(changed.validate(), Err(ErrorCode::InvalidRequest));
        changed = config.clone();
        changed.memory_bytes = u64::MAX;
        assert_eq!(changed.validate(), Err(ErrorCode::InvalidRequest));
        changed = config.clone();
        changed.swap_bytes = 1;
        assert_eq!(changed.validate(), Err(ErrorCode::InvalidRequest));

        let mut value = serde_json::to_value(config).unwrap();
        value["unreviewed_field"] = serde_json::json!(true);
        assert_eq!(
            Config::parse(&serde_json::to_vec(&value).unwrap()),
            Err(ErrorCode::InvalidRequest)
        );
    }

    #[test]
    fn production_photo_listener_serves_reconcile_over_seqpacket() {
        let socket = env::temp_dir().join(format!(
            "slipstream-photo-listener-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o666)).unwrap();
        let config = production_config();
        // The production listener requires Web UID 1000. This transport-level
        // test authenticates its actual peer so it also runs under other CI
        // UIDs; `serve()` still validates the production identity.
        let peer_uid = unsafe { libc::geteuid() };
        let instance = config.instance.clone();
        let policy = config.policy.clone();
        let bundle = config.bundle.clone();
        let served = thread::spawn(move || {
            let fd = accept(listener.as_raw_fd()).unwrap();
            // The journal-backed handler reports the durable capability state;
            // a blocked capability stays the truthful answer without a root
            // owned attempt boundary in this transport-level test.
            let instance = instance.clone();
            serve_connection(fd, peer_uid, &move |request, descriptor, _| {
                assert!(descriptor.is_none());
                assert!(matches!(request, Request::Reconcile { .. }));
                Ok(ResultBody::Capability {
                    capability: "photo-processing".into(),
                    instance: instance.clone(),
                    incarnation: "b".repeat(32),
                    next_sequence: 1,
                    policy: policy.clone(),
                    bundle: bundle.clone(),
                    availability: Availability::Blocked,
                    active: None,
                })
            })
            .unwrap();
        });
        let response = reconcile(&socket, config.instance).unwrap();
        served.join().unwrap();
        assert!(matches!(
            response,
            Response::Result {
                result,
                ..
            } if matches!(*result, ResultBody::Capability {
                availability: Availability::Blocked,
                ..
            })
        ));
        fs::remove_file(socket).unwrap();
    }

    #[test]
    fn descriptor_carrying_request_refuses_zero_right_operations_and_round_trips() {
        let socket = env::temp_dir().join(format!(
            "slipstream-photo-descriptor-helper-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o666)).unwrap();
        // A reconcile may never claim a descriptor; the helper refuses it
        // before opening a connection.
        assert_eq!(
            request_with_descriptor(&socket, &Request::reconcile("0".repeat(32)), 0),
            Err(ErrorCode::InvalidRequest)
        );

        let (source_path, source) = file("helper-source", b"raw-bytes", libc::O_RDONLY, 0o444);
        let served = thread::spawn(move || {
            let connection = accept(listener.as_raw_fd()).unwrap();
            let (request, descriptor) = receive_request(connection.as_raw_fd()).unwrap();
            assert!(matches!(request, Request::Start { .. }));
            let descriptor = descriptor.unwrap();
            let mut copied = Vec::new();
            File::from(descriptor).read_to_end(&mut copied).unwrap();
            assert_eq!(copied, b"raw-bytes");
            send_response(
                connection.as_raw_fd(),
                &Response::result(ResultBody::Output {
                    receipt: OutputReceipt {
                        export_id: "export-1".into(),
                        incarnation: "d".repeat(32),
                        sequence: 1,
                        target: PHOTO_WORKLOAD.into(),
                        size: 4,
                        sha256: "4".repeat(64),
                    },
                }),
            )
            .unwrap();
        });
        let response =
            request_with_descriptor(&socket, &start_request(), source.as_raw_fd()).unwrap();
        served.join().unwrap();
        assert!(matches!(
            response,
            Response::Result {
                result,
                ..
            } if matches!(*result, ResultBody::Output { .. })
        ));
        drop(source);
        fs::remove_file(source_path).unwrap();
        fs::remove_file(socket).unwrap();
    }

    /// A packet may not declare more bytes than the bounded application seams
    /// accept. The application limit stays representable; a smaller configured
    /// deployment maximum is enforced during admission, not by this envelope
    /// check.
    #[test]
    fn declared_source_and_output_sizes_stay_within_the_application_limit() {
        let mut start = start_request();
        match &mut start {
            Request::Start { source, .. } => source.size = MAX_SOURCE_BYTES,
            other => panic!("unexpected request: {other:?}"),
        }
        let bytes = start.bytes().unwrap();
        assert_eq!(Request::parse(&bytes).unwrap(), start);

        let mut oversized = start.clone();
        match &mut oversized {
            Request::Start { source, .. } => source.size = MAX_SOURCE_BYTES + 1,
            other => panic!("unexpected request: {other:?}"),
        }
        assert_eq!(oversized.bytes(), Err(ErrorCode::InvalidRequest));
        assert_eq!(
            Request::parse(&serde_json::to_vec(&oversized).unwrap()),
            Err(ErrorCode::InvalidRequest)
        );

        // The listener accepts the in-limit packet and refuses the oversized one
        // of the same shape, dropping the source descriptor attached to it.
        let (sender, receiver) = socket_pair();
        let (source_path, source) = file("declared-source", b"raw bytes", libc::O_RDONLY, 0o444);
        assert!(send_packet(sender.as_raw_fd(), &bytes, &[source.as_raw_fd()]).is_ok());
        let (received, descriptor) = receive_request(receiver.as_raw_fd()).unwrap();
        assert_eq!(received, start);
        assert!(descriptor.is_some());
        drop(descriptor);

        let (sender, receiver) = socket_pair();
        assert!(
            send_packet(
                sender.as_raw_fd(),
                &serde_json::to_vec(&oversized).unwrap(),
                &[source.as_raw_fd()]
            )
            .is_ok()
        );
        assert!(receive_request(receiver.as_raw_fd()).is_err());
        assert!(source.metadata().is_ok());
        fs::remove_file(source_path).unwrap();

        let mut validation = Request::ValidateOutput {
            mode: PHOTO_MODE.into(),
            version: PHOTO_PROTOCOL_VERSION,
            instance: "0".repeat(32),
            export_id: "export-1".into(),
            incarnation: "d".repeat(32),
            sequence: 1,
            target: PHOTO_WORKLOAD.into(),
            size: MAX_OUTPUT_BYTES,
            sha256: "4".repeat(64),
            accepted: true,
        };
        assert!(Request::parse(&validation.bytes().unwrap()).is_ok());
        match &mut validation {
            Request::ValidateOutput { size, .. } => *size = MAX_OUTPUT_BYTES + 1,
            other => panic!("unexpected request: {other:?}"),
        }
        assert_eq!(validation.bytes(), Err(ErrorCode::InvalidRequest));
        assert_eq!(
            Request::parse(&serde_json::to_vec(&validation).unwrap()),
            Err(ErrorCode::InvalidRequest)
        );
    }
}
