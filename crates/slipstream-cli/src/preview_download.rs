use super::{
    CLI_CONTRACT_VERSION, CONTRACT_HEADER, CommandFailure, Operation, PreviewSize,
    PublicationState, ServiceClient, access_boundary_failure, web_url,
};
use reqwest::{StatusCode, header};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::OpenOptionsExt,
    },
    path::Path,
};
use tokio::io::AsyncWriteExt;
use zune_core::bytestream::ZCursor;
use zune_jpeg::JpegDecoder;

const MAXIMUM_PREVIEW_BYTES: usize = 64 * 1024 * 1024;
const OPERATION: Operation = Operation::PhotosPreview;

pub(super) struct Destination {
    directory: File,
    name: CString,
    path: String,
}

impl Destination {
    pub(super) fn preflight(path: &Path) -> Result<Self, CommandFailure> {
        let path_text = path.to_str().ok_or_else(|| {
            CommandFailure::invalid("file", "The Preview path must be valid UTF-8.")
        })?;
        let name = path
            .file_name()
            .ok_or_else(|| CommandFailure::invalid("file", "Name one new Preview file."))?;
        let name = CString::new(name.to_str().ok_or_else(|| {
            CommandFailure::invalid("file", "The Preview path must be valid UTF-8.")
        })?)
        .map_err(|_| CommandFailure::invalid("file", "The Preview path is invalid."))?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(parent)
            .map_err(|_| local_io(path_text))?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let exists = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if exists == 0 {
            return Err(CommandFailure::invalid(
                "file",
                "The Preview path already exists.",
            ));
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT) {
            return Err(local_io(path_text));
        }
        Ok(Self {
            directory,
            name,
            path: path_text.to_owned(),
        })
    }

    fn anonymous_file(&self) -> Result<tokio::fs::File, CommandFailure> {
        let dot = c".";
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                dot.as_ptr(),
                libc::O_TMPFILE | libc::O_RDWR | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(local_io(&self.path));
        }
        // The unnamed inode has no directory entry to race or clean up on
        // cancellation. Closing this fd before publication discards it.
        let file = unsafe { File::from(std::os::fd::OwnedFd::from_raw_fd(fd)) };
        Ok(tokio::fs::File::from_std(file))
    }

    fn publish(&self, file: &tokio::fs::File) -> Result<(), CommandFailure> {
        let empty = c"";
        let result = unsafe {
            libc::linkat(
                file.as_raw_fd(),
                empty.as_ptr(),
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                libc::AT_EMPTY_PATH,
            )
        };
        if result == 0 {
            return Ok(());
        }
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST) {
            return Err(CommandFailure::invalid(
                "file",
                "The Preview path already exists.",
            ));
        }
        Err(local_io(&self.path))
    }
}

fn local_io(path: &str) -> CommandFailure {
    CommandFailure::from_payload(
        6,
        super::ErrorPayload {
            code: "local_io_failed".to_owned(),
            message: "Check the local Preview destination and try again.".to_owned(),
            effect: "none".to_owned(),
            details: json!({"operation": "write-preview", "path": path, "fileCommitted": false}),
        },
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewMetadata {
    photo_id: String,
    state: String,
    source: String,
    source_revision: String,
    width: u32,
    height: u32,
    detail_limited: bool,
    url: String,
    web_path: String,
}

impl PreviewSize {
    fn route(self) -> &'static str {
        match self {
            Self::Thumbnail => "thumbnail",
            Self::Review => "preview",
        }
    }

    fn derivative(self) -> &'static str {
        match self {
            Self::Thumbnail => "thumbnail",
            Self::Review => "review",
        }
    }
}

fn admitted_metadata(
    metadata: &PreviewMetadata,
    photo_id: &str,
    size: PreviewSize,
    client: &ServiceClient,
) -> Result<(reqwest::Url, String), CommandFailure> {
    let key = metadata
        .url
        .strip_prefix(&format!(
            "/api/private/derivatives/{photo_id}/{}/",
            size.derivative()
        ))
        .and_then(|name| name.strip_suffix(".jpg"))
        .ok_or_else(|| CommandFailure::transport(OPERATION))?;
    if key.len() != 64
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || metadata.url
            != format!(
                "/api/private/derivatives/{photo_id}/{}/{}.jpg",
                size.derivative(),
                key
            )
        || metadata.photo_id != photo_id
        || metadata.web_path != format!("/?photoId={photo_id}")
        || metadata.state != "ready"
        || !matches!(
            metadata.source.as_str(),
            "jpeg-original" | "raw-embedded-jpeg"
        )
        || metadata.source_revision.is_empty()
        || metadata.source_revision.len() > 8192
        || metadata.width == 0
        || metadata.height == 0
        || metadata.detail_limited != (metadata.width.max(metadata.height) < 2560)
    {
        return Err(CommandFailure::transport(OPERATION));
    }
    let web_url = web_url(&client.origin, &metadata.web_path)
        .map_err(|_| CommandFailure::transport(OPERATION))?;
    let url = client
        .origin
        .join(&metadata.url)
        .map_err(|_| CommandFailure::transport(OPERATION))?;
    Ok((url, web_url))
}

fn repeated_headers_match(response: &reqwest::Response, metadata: &PreviewMetadata) -> bool {
    let header_is = |name: &str, expected: &str| {
        let mut values = response.headers().get_all(name).iter();
        values.next().and_then(|value| value.to_str().ok()) == Some(expected)
            && values.next().is_none()
    };
    let revision = metadata
        .source_revision
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    header_is("slipstream-preview-photo", &metadata.photo_id)
        && header_is("slipstream-preview-source", &metadata.source)
        && header_is("slipstream-preview-revision", &revision)
        && header_is("slipstream-preview-width", &metadata.width.to_string())
        && header_is("slipstream-preview-height", &metadata.height.to_string())
}

pub(super) async fn download(
    client: &ServiceClient,
    photo_id: &str,
    size: PreviewSize,
    destination: Destination,
    publication: &PublicationState,
) -> Result<Value, CommandFailure> {
    let metadata: PreviewMetadata = client
        .json(
            OPERATION,
            reqwest::Method::GET,
            client.endpoint(&["api", "photos", photo_id, size.route()]),
            None,
        )
        .await?;
    let (url, web_url) = admitted_metadata(&metadata, photo_id, size, client)?;
    let mut response = client
        .client
        .get(url)
        .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
        .bearer_auth(&client.token)
        .send()
        .await
        .map_err(|_| CommandFailure::transport(OPERATION))?;
    let status = response.status();
    if status.is_redirection() {
        return Err(CommandFailure::transport(OPERATION));
    }
    if let Some(failure) = access_boundary_failure(status, None, &[], OPERATION) {
        return Err(failure);
    }
    if status != StatusCode::OK
        || response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some("image/jpeg")
        || response
            .content_length()
            .is_some_and(|length| length > MAXIMUM_PREVIEW_BYTES as u64)
        || !repeated_headers_match(&response, &metadata)
    {
        return Err(CommandFailure::transport(OPERATION));
    }
    let mut file = destination.anonymous_file()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CommandFailure::transport(OPERATION))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAXIMUM_PREVIEW_BYTES {
            return Err(CommandFailure::transport(OPERATION));
        }
        file.write_all(&chunk)
            .await
            .map_err(|_| local_io(&destination.path))?;
        bytes.extend_from_slice(&chunk);
    }
    let width = metadata.width;
    let height = metadata.height;
    let complete = tokio::task::spawn_blocking(move || complete_jpeg(&bytes, width, height))
        .await
        .map_err(|_| CommandFailure::transport(OPERATION))?;
    if !complete {
        return Err(CommandFailure::transport(OPERATION));
    }
    file.sync_all()
        .await
        .map_err(|_| local_io(&destination.path))?;
    let mut data = json!({
        "photoId": photo_id,
        "path": destination.path,
        "source": metadata.source,
        "sourceRevision": metadata.source_revision,
        "width": metadata.width,
        "height": metadata.height,
        "detailLimited": metadata.detail_limited,
        "webUrl": web_url,
        "fileCommitted": true,
    });
    super::redact_value(&mut data, &client.token);
    destination.publish(&file)?;
    publication.record(data.clone());
    if unsafe { libc::fsync(destination.directory.as_raw_fd()) } != 0 {
        return Err(CommandFailure::published_preview(data, false));
    }
    Ok(data)
}

// Header parsing does not consume entropy-coded scans. Walk their marker
// structure as well; this does not claim to validate individual MCU data.
fn complete_jpeg(bytes: &[u8], width: u32, height: u32) -> bool {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return false;
    }
    let mut decoder = JpegDecoder::new(ZCursor::new(bytes));
    if decoder.decode_headers().is_err()
        || !decoder
            .info()
            .is_some_and(|info| u32::from(info.width) == width && u32::from(info.height) == height)
    {
        return false;
    }
    let mut index = 2;
    let mut in_scan = false;
    let mut saw_scan = false;
    let mut saw_payload = false;
    loop {
        let marker = if in_scan {
            loop {
                let Some(&byte) = bytes.get(index) else {
                    return false;
                };
                index += 1;
                if byte != 0xff {
                    saw_payload = true;
                    continue;
                }
                while bytes.get(index) == Some(&0xff) {
                    index += 1;
                }
                let Some(&marker) = bytes.get(index) else {
                    return false;
                };
                index += 1;
                match marker {
                    0x00 => {
                        saw_payload = true;
                        continue;
                    }
                    0xd0..=0xd7 => continue,
                    _ => break marker,
                }
            }
        } else {
            if bytes.get(index) != Some(&0xff) {
                return false;
            }
            while bytes.get(index) == Some(&0xff) {
                index += 1;
            }
            let Some(&marker) = bytes.get(index) else {
                return false;
            };
            index += 1;
            marker
        };
        match marker {
            0xd9 => return saw_scan && saw_payload && index == bytes.len(),
            0xd8 | 0x00 | 0x01 | 0xd0..=0xd7 => return false,
            _ => {
                let Some(length_bytes) = bytes.get(index..index.saturating_add(2)) else {
                    return false;
                };
                let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
                if length < 2
                    || index
                        .checked_add(length)
                        .is_none_or(|end| end > bytes.len())
                {
                    return false;
                }
                index += length;
                in_scan = marker == 0xda;
                saw_scan |= in_scan;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_state_redacts_token_and_invalid_utf8_is_refused_before_io() {
        use std::os::unix::ffi::OsStringExt;
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(
            b"/tmp/invalid-\xff.jpg".to_vec(),
        ));
        assert_eq!(
            Destination::preflight(&path).err().unwrap().payload.code,
            "invalid_input"
        );

        let token = "private-access-token";
        let mut data = json!({
            "path": "preview.jpg",
            "sourceRevision": format!("rev-{token}"),
            "fileCommitted": true
        });
        super::super::redact_value(&mut data, token);
        let publication = PublicationState::default();
        publication.record(data);
        let failure = CommandFailure::published_preview(publication.committed().unwrap(), true);
        assert_eq!(failure.payload.effect, "partial");
        assert_eq!(failure.payload.details["fileCommitted"], true);
        assert!(
            !serde_json::to_string(&failure.data)
                .unwrap()
                .contains(token)
        );
    }

    #[tokio::test]
    async fn publication_never_replaces_a_name_taken_after_preflight() {
        use std::os::unix::fs::symlink;
        let base =
            std::env::temp_dir().join(format!("slipstream-cli-publication-{}", std::process::id()));
        std::fs::create_dir(&base).unwrap();
        let path = base.join("preview.jpg");
        let sentinel = base.join("sentinel");
        std::fs::write(&sentinel, b"original").unwrap();
        let destination = Destination::preflight(&path).unwrap();
        let mut staged = destination.anonymous_file().unwrap();
        staged.write_all(b"downloaded").await.unwrap();
        staged.sync_all().await.unwrap();

        symlink(&sentinel, &path).unwrap();
        assert_eq!(
            destination.publish(&staged).unwrap_err().payload.code,
            "invalid_input"
        );
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"original");
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"racing file").unwrap();
        assert_eq!(
            destination.publish(&staged).unwrap_err().payload.code,
            "invalid_input"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"racing file");
        std::fs::remove_file(&path).unwrap();

        destination.publish(&staged).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"downloaded");
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&sentinel).unwrap();
        std::fs::remove_dir(&base).unwrap();
    }

    #[test]
    fn rejects_truncated_scan_and_trailing_bytes_without_decoding_pixels() {
        let mut jpeg = Vec::new();
        let pixels = vec![40; 8 * 4 * 3];
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(&pixels, 8, 4, image::ExtendedColorType::Rgb8)
            .unwrap();
        assert!(complete_jpeg(&jpeg, 8, 4));
        assert!(!complete_jpeg(&jpeg, 4, 8));
        assert!(!complete_jpeg(&jpeg[..jpeg.len() - 2], 8, 4));
        jpeg.push(0);
        assert!(!complete_jpeg(&jpeg, 8, 4));
    }
}
