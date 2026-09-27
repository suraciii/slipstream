// Static Web delivery.
use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode, header},
};
use std::{
    ffi::CString,
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::OpenOptionsExt,
    },
    path::{Path, PathBuf},
};

use super::{HttpState, WebRoot, api_error};
pub(crate) async fn static_web(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let path = request.uri().path();
    if path == "/api" || path.starts_with("/api/") {
        return api_error(StatusCode::NOT_FOUND, "Not found");
    }
    let requested = path.strip_prefix('/').unwrap_or(path);
    let requested = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };
    let root = state.web_root;
    let requested_file = safe_web_path(&root.path, requested);
    let actual = match requested_file {
        Some(path) => path,
        None => root.path.join("__invalid__"),
    };
    let (bytes, is_index) = match read_web_file(&root, &actual).await {
        Ok(value) => value,
        Err(_) if requested == "manifest.webmanifest" || requested.starts_with("icons/") => {
            let mut response = plain_error(StatusCode::NOT_FOUND, "Not found");
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-cache"),
            );
            if request.method() == axum::http::Method::HEAD {
                *response.body_mut() = Body::empty();
            }
            return response;
        }
        Err(_) => match read_web_file(&root, &root.path.join("index.html")).await {
            Ok(value) => value,
            Err(_) => {
                return plain_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Web application is not built",
                );
            }
        },
    };
    let cache_control = if !is_index && requested.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let content_length = bytes.len().to_string();
    let body = if request.method().as_str() == "HEAD" {
        Body::empty()
    } else {
        Body::from(bytes)
    };
    let served_path = if is_index {
        root.path.join("index.html")
    } else {
        actual
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type(&served_path))
        .header(header::CONTENT_LENGTH, content_length)
        .header(header::CACHE_CONTROL, cache_control)
        .header("x-content-type-options", "nosniff")
        .body(body)
        .expect("valid static response")
}

pub(crate) fn safe_web_path(root: &Path, requested: &str) -> Option<PathBuf> {
    if requested.contains('\0') || requested.contains('\\') {
        return None;
    }
    let mut path = root.to_path_buf();
    for component in requested.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return None;
        }
        path.push(component);
    }
    Some(path)
}

pub(crate) async fn read_web_file(root: &WebRoot, candidate: &Path) -> io::Result<(Vec<u8>, bool)> {
    let Some(descriptor) = &root.descriptor else {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    };
    let relative = candidate
        .strip_prefix(&root.path)
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let relative = CString::new(relative.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let file = open_confined_file(descriptor.as_raw_fd(), &relative)?;
    let mut bytes = Vec::new();
    let mut file = fs::File::from(file);
    io::Read::read_to_end(&mut file, &mut bytes)?;
    let is_index = relative.as_bytes() == b"index.html";
    Ok((bytes, is_index))
}

pub(crate) fn open_directory_descriptor(path: &Path) -> io::Result<OwnedFd> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let descriptor: OwnedFd = file.into();
    let facts = fstat(descriptor.as_raw_fd())?;
    if facts.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(io::Error::from(io::ErrorKind::NotADirectory));
    }
    Ok(descriptor)
}

pub(crate) fn open_confined_file(root: i32, relative: &CString) -> io::Result<OwnedFd> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let how = OpenHow {
        flags: (libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW) as u64,
        mode: 0,
        resolve: 0x08 | 0x04 | 0x02,
    };
    // SAFETY: `relative` is NUL-terminated and `how` matches Linux open_how.
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root,
            relative.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        ) as libc::c_int
    };
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: the descriptor is newly opened and uniquely owned.
        Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
    }
}

pub(crate) fn fstat(fd: i32) -> io::Result<libc::stat> {
    let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `facts` is valid writable storage and initialized on success.
    if unsafe { libc::fstat(fd, facts.as_mut_ptr()) } == 0 {
        // SAFETY: successful fstat initialized `facts`.
        Ok(unsafe { facts.assume_init() })
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

pub(crate) fn plain_error(status: StatusCode, message: &'static str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(message))
        .expect("valid plain response")
}
