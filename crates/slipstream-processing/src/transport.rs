use crate::{Executor, protocol::*};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub fn request(socket: impl AsRef<Path>, request: &Request) -> Result<Response, ErrorCode> {
    let mut stream = UnixStream::connect(socket).map_err(|_| ErrorCode::Unavailable)?;
    timeouts(&stream)?;
    let bytes = serde_json::to_vec(request).map_err(|_| ErrorCode::InvalidRequest)?;
    if bytes.len() > REQUEST_BYTES {
        return Err(ErrorCode::InvalidRequest);
    }
    send(&mut stream, &bytes)?;
    let bytes = frame(&mut stream, RESPONSE_BYTES)?;
    serde_json::from_slice(&bytes).map_err(|_| ErrorCode::Unavailable)
}

pub fn serve(executor: Arc<Executor>) -> Result<(), ErrorCode> {
    let config = executor.config();
    let path = Path::new(&config.socket);
    let mut socket_claim = claim_socket(path, config)?;
    let listener = UnixListener::bind(path).map_err(|_| ErrorCode::Unavailable)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| ErrorCode::Unavailable)?;
    let claim = SocketClaim {
        instance: config.instance.clone(),
        root: config.root.clone(),
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    let bytes = serde_json::to_vec(&claim).map_err(|_| ErrorCode::Unavailable)?;
    socket_claim
        .set_len(0)
        .and_then(|_| socket_claim.write_all(&bytes))
        .and_then(|_| socket_claim.sync_all())
        .map_err(|_| ErrorCode::Unavailable)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| ErrorCode::Unavailable)?;
    allow_peer(path, config.peer_uid)?;
    // SAFETY: listener owns a valid bound listening socket; backlog is explicitly bounded.
    if unsafe { libc::listen(listener.as_raw_fd(), 4) } != 0 {
        return Err(ErrorCode::Unavailable);
    }
    executor.recover_async();
    let count = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        if count.fetch_add(1, Ordering::AcqRel) >= 4 {
            count.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let count = count.clone();
        let executor = executor.clone();
        thread::spawn(move || {
            let result = (|| {
                timeouts(&stream)?;
                let (uid, pid) = peer(&stream)?;
                if uid != executor.config().peer_uid {
                    return Err(ErrorCode::Unauthorized);
                }
                let bytes = frame(&mut stream, REQUEST_BYTES)?;
                if executor.is_film() {
                    executor.handle_film(crate::film::Request::parse(&bytes)?, pid)
                } else {
                    executor.cancel(Request::parse(&bytes)?, pid)
                }
            })();
            let mut response = Response::from(result);
            if executor.is_film() {
                match &mut response {
                    Response::Result { version, .. } | Response::Error { version, .. } => {
                        *version = 2
                    }
                }
            }
            if let Ok(bytes) = serde_json::to_vec(&response)
                && bytes.len() <= RESPONSE_BYTES
            {
                let _ = send(&mut stream, &bytes);
            }
            count.fetch_sub(1, Ordering::AcqRel);
        });
    }
    Err(ErrorCode::Unavailable)
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SocketClaim {
    instance: String,
    root: String,
    device: u64,
    inode: u64,
}

fn claim_socket(path: &Path, config: &Config) -> Result<File, ErrorCode> {
    let mut claim = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path.with_extension("sock-owner"))
        .map_err(|_| ErrorCode::Unavailable)?;
    let metadata = claim.metadata().map_err(|_| ErrorCode::Unavailable)?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 4096
    {
        return Err(ErrorCode::Uncertain);
    }
    // SAFETY: the claim is a live owned file. This also fences distinct instance roots sharing a socket path.
    if unsafe { libc::flock(claim.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(ErrorCode::Busy);
    }
    let mut bytes = Vec::new();
    (&mut claim)
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::Uncertain)?;
    let previous = if bytes.is_empty() {
        None
    } else {
        Some(serde_json::from_slice::<SocketClaim>(&bytes).map_err(|_| ErrorCode::Uncertain)?)
    };
    if previous
        .as_ref()
        .is_some_and(|value| value.instance != config.instance || value.root != config.root)
    {
        return Err(ErrorCode::Uncertain);
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket()
                || metadata.uid() != 0
                || previous.as_ref().is_none_or(|value| {
                    value.device != metadata.dev() || value.inode != metadata.ino()
                })
            {
                return Err(ErrorCode::Uncertain);
            }
            fs::remove_file(path).map_err(|_| ErrorCode::Uncertain)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(ErrorCode::Uncertain),
    }
    std::io::Seek::rewind(&mut claim).map_err(|_| ErrorCode::Uncertain)?;
    Ok(claim)
}

fn timeouts(stream: &UnixStream) -> Result<(), ErrorCode> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .and_then(|_| stream.set_write_timeout(Some(Duration::from_secs(2))))
        .map_err(|_| ErrorCode::Unavailable)
}

fn send(stream: &mut UnixStream, bytes: &[u8]) -> Result<(), ErrorCode> {
    let length = u32::try_from(bytes.len()).map_err(|_| ErrorCode::InvalidRequest)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut framed = Vec::with_capacity(bytes.len() + 4);
    framed.extend(length.to_be_bytes());
    framed.extend(bytes);
    let mut remaining = framed.as_slice();
    while !remaining.is_empty() {
        let time = deadline
            .checked_duration_since(Instant::now())
            .filter(|value| !value.is_zero())
            .ok_or(ErrorCode::Unavailable)?;
        stream
            .set_write_timeout(Some(time))
            .map_err(|_| ErrorCode::Unavailable)?;
        let written = stream
            .write(remaining)
            .map_err(|_| ErrorCode::Unavailable)?;
        if written == 0 {
            return Err(ErrorCode::Unavailable);
        }
        remaining = &remaining[written..];
    }
    Ok(())
}

fn frame(stream: &mut UnixStream, maximum: usize) -> Result<Vec<u8>, ErrorCode> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut header = [0u8; 4];
    receive_exact(stream, &mut header, deadline)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > maximum {
        return Err(ErrorCode::InvalidRequest);
    }
    let mut bytes = vec![0; length];
    receive_exact(stream, &mut bytes, deadline)?;
    Ok(bytes)
}

fn receive_exact(
    stream: &UnixStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), ErrorCode> {
    while !bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|value| !value.is_zero())
            .ok_or(ErrorCode::InvalidRequest)?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|_| ErrorCode::Unavailable)?;
        let mut control = [0usize; 8];
        let mut vector = libc::iovec {
            iov_base: bytes.as_mut_ptr().cast(),
            iov_len: bytes.len(),
        };
        // SAFETY: zero is a valid initial msghdr; all supplied buffers live through recvmsg.
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut vector;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = std::mem::size_of_val(&control);
        // SAFETY: stream and buffer descriptors are valid; received descriptors are marked close-on-exec.
        let received =
            unsafe { libc::recvmsg(stream.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
        if received <= 0 {
            return Err(ErrorCode::InvalidRequest);
        }
        let mut ancillary = false;
        // SAFETY: ancillary headers are kernel-generated within the supplied aligned control buffer.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&message);
            while !header.is_null() {
                ancillary = true;
                if (*header).cmsg_level == libc::SOL_SOCKET
                    && (*header).cmsg_type == libc::SCM_RIGHTS
                {
                    let count = ((*header).cmsg_len - libc::CMSG_LEN(0) as usize)
                        / std::mem::size_of::<i32>();
                    let descriptors = libc::CMSG_DATA(header).cast::<i32>();
                    for index in 0..count {
                        libc::close(*descriptors.add(index));
                    }
                }
                header = libc::CMSG_NXTHDR(&message, header);
            }
        }
        if ancillary || message.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(ErrorCode::InvalidRequest);
        }
        bytes = &mut bytes[received as usize..];
    }
    Ok(())
}

fn peer(stream: &UnixStream) -> Result<(u32, u32), ErrorCode> {
    // SAFETY: ucred is a C POD struct initialized by getsockopt.
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: credentials is an appropriately sized writable buffer.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    } != 0
        || credentials.pid <= 0
    {
        return Err(ErrorCode::Unauthorized);
    }
    Ok((credentials.uid, credentials.pid as u32))
}

fn allow_peer(path: &Path, uid: u32) -> Result<(), ErrorCode> {
    if uid == 0 {
        return Ok(());
    }
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| ErrorCode::Unavailable)?;
    let mut acl = Vec::from(2u32.to_le_bytes());
    for (tag, permissions, identity) in [
        (1u16, 6u16, u32::MAX),
        (2, 6, uid),
        (4, 0, u32::MAX),
        (16, 6, u32::MAX),
        (32, 0, u32::MAX),
    ] {
        acl.extend(tag.to_le_bytes());
        acl.extend(permissions.to_le_bytes());
        acl.extend(identity.to_le_bytes());
    }
    // SAFETY: both C strings and the ACL byte buffer remain valid for the syscall.
    if unsafe {
        libc::setxattr(
            path.as_ptr(),
            c"system.posix_acl_access".as_ptr(),
            acl.as_ptr().cast(),
            acl.len(),
            0,
        )
    } != 0
    {
        return Err(ErrorCode::Unavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_rejects_length_before_allocating_body() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        sender.write_all(&u32::MAX.to_be_bytes()).unwrap();
        assert_eq!(
            frame(&mut receiver, REQUEST_BYTES),
            Err(ErrorCode::InvalidRequest)
        );
    }

    #[test]
    fn framing_rejects_ancillary_descriptors() {
        let (sender, mut receiver) = UnixStream::pair().unwrap();
        let file = File::open("/dev/null").unwrap();
        let mut bytes = [0u8, 0, 0, 1];
        let mut vector = libc::iovec {
            iov_base: bytes.as_mut_ptr().cast(),
            iov_len: bytes.len(),
        };
        let mut control = [0usize; 8];
        // SAFETY: msghdr and cmsghdr refer to aligned buffers alive across sendmsg.
        unsafe {
            let mut message: libc::msghdr = std::mem::zeroed();
            message.msg_iov = &mut vector;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as usize;
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as usize;
            *libc::CMSG_DATA(header).cast::<i32>() = file.as_raw_fd();
            assert_eq!(libc::sendmsg(sender.as_raw_fd(), &message, 0), 4);
        }
        assert_eq!(
            frame(&mut receiver, REQUEST_BYTES),
            Err(ErrorCode::InvalidRequest)
        );
        assert!(file.metadata().is_ok());
    }

    #[test]
    fn expired_frame_deadline_refuses_even_a_ready_byte() {
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        sender.write_all(b"x").unwrap();
        assert_eq!(
            receive_exact(&receiver, &mut [0u8; 1], Instant::now()),
            Err(ErrorCode::InvalidRequest)
        );
    }

    #[test]
    fn framing_reconstructs_exact_payload() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        send(&mut sender, b"test").unwrap();
        assert_eq!(frame(&mut receiver, REQUEST_BYTES).unwrap(), b"test");
    }
}
