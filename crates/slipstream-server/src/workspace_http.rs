use crate::http::{CLI_CONTRACT_HEADER, HttpState, json_response, require_cli_contract};
use crate::workspace::{WorkspaceDownload, WorkspaceError};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, Response, StatusCode},
};
use futures_core::Stream;
use serde_json::{Value, json};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
fn error(e: WorkspaceError) -> Response<Body> {
    json_response(
        StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        &json!({"error":{"code":e.code,"message":e.message,"effect":e.effect,"details":e.details}}),
    )
}
fn invalid() -> Response<Body> {
    json_response(
        StatusCode::BAD_REQUEST,
        &json!({"error":{"code":"invalid_input","message":"Correct the request fields or query","effect":"none","details":{}}}),
    )
}
pub(crate) fn boundary(
    request: &Request<Body>,
    query: bool,
) -> std::result::Result<(), Box<Response<Body>>> {
    if request.headers().contains_key(CLI_CONTRACT_HEADER) {
        require_cli_contract(request)?;
    }
    if !query && request.uri().query().is_some() {
        return Err(Box::new(invalid()));
    }
    Ok(())
}
async fn body(
    request: Request<Body>,
    allowed: &[&str],
) -> std::result::Result<Value, Response<Body>> {
    let bytes = axum::body::to_bytes(request.into_body(), 65536)
        .await
        .map_err(|_| invalid())?;
    let value = if bytes.is_empty() {
        json!({})
    } else {
        photo_files::read_json(bytes.as_ref(), 65536).map_err(|_| invalid())?
    };
    let object = value.as_object().ok_or_else(invalid)?;
    if object.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(invalid());
    }
    Ok(value)
}
async fn run(
    task: impl FnOnce() -> crate::workspace::Result<Value> + Send + 'static,
) -> Response<Body> {
    match tokio::task::spawn_blocking(task).await {
        Ok(Ok(value)) => json_response(StatusCode::OK, &value),
        Ok(Err(e)) => error(e),
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({"error":{"code":"local_io_failed","message":"Workspace worker failed","effect":"unknown","details":{}}}),
        ),
    }
}
fn query(
    request: &Request<Body>,
    allowed: &[&str],
) -> std::result::Result<std::collections::HashMap<String, String>, Box<Response<Body>>> {
    boundary(request, true)?;
    let mut result = std::collections::HashMap::new();
    for (k, v) in url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes()) {
        if !allowed.contains(&k.as_ref()) || result.insert(k.into_owned(), v.into_owned()).is_some()
        {
            return Err(Box::new(invalid()));
        }
    }
    Ok(result)
}
pub(crate) async fn open(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(e) = boundary(&request, false) {
        return *e;
    }
    if body(request, &[]).await.is_err() {
        return invalid();
    }
    let manager = state.application.workspaces.clone();
    run(move || manager.open_photo(&id)).await
}
pub(crate) async fn show(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(e) = boundary(&request, false) {
        return *e;
    }
    let manager = state.application.workspaces.clone();
    run(move || manager.show(&id)).await
}
pub(crate) async fn files(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let q = match query(&request, &["kind", "limit"]) {
        Ok(q) => q,
        Err(e) => return *e,
    };
    let limit = match q.get("limit") {
        None => 50,
        Some(s) => match s.parse::<usize>() {
            Ok(n) if (1..=60).contains(&n) => n,
            _ => return invalid(),
        },
    };
    let manager = state.application.workspaces.clone();
    run(move || manager.files(Some(&id), q.get("kind").map(String::as_str), limit, None)).await
}
pub(crate) async fn continuation(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let q = match query(&request, &["cursor"]) {
        Ok(q) if q.contains_key("cursor") => q,
        _ => return invalid(),
    };
    let manager = state.application.workspaces.clone();
    run(move || manager.files(None, None, 50, q.get("cursor").map(String::as_str))).await
}
pub(crate) async fn inspect(
    State(state): State<HttpState>,
    Path((workspace, id)): Path<(String, String)>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(e) = boundary(&request, false) {
        return *e;
    }
    let manager = state.application.workspaces.clone();
    run(move || manager.inspect(&workspace, &id)).await
}
pub(crate) async fn bytes(
    State(state): State<HttpState>,
    Path((workspace, id)): Path<(String, String)>,
    request: Request<Body>,
) -> Response<Body> {
    let q = match query(&request, &["revision"]) {
        Ok(q) => q,
        Err(e) => return *e,
    };
    let headers = request.headers().clone();
    let manager = state.application.workspaces.clone();
    match tokio::task::spawn_blocking(move || {
        manager.download(&workspace, &id, q.get("revision").map(String::as_str))
    })
    .await
    {
        Ok(Ok(file)) => stream_with(file, &headers),
        Ok(Err(e)) => error(e),
        Err(_) => invalid(),
    }
}
pub(crate) async fn refresh(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(e) = boundary(&request, false) {
        return *e;
    }
    let b = match body(request, &["expectedSourceRevision", "requestId"]).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(source) = b["expectedSourceRevision"].as_str().map(str::to_owned) else {
        return invalid();
    };
    if b.get("requestId").is_some_and(|v| !v.is_string()) {
        return invalid();
    };
    let manager = state.application.workspaces.clone();
    run(move || manager.refresh(&id, &source, b["requestId"].as_str())).await
}
pub(crate) async fn remove(
    State(state): State<HttpState>,
    Path((workspace, id)): Path<(String, String)>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(e) = boundary(&request, false) {
        return *e;
    }
    let b = match body(request, &["revision", "requestId"]).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    if ["revision", "requestId"]
        .iter()
        .any(|k| b.get(k).is_some_and(|v| !v.is_string()))
    {
        return invalid();
    };
    let manager = state.application.workspaces.clone();
    run(move || {
        manager.remove(
            &workspace,
            &id,
            b["revision"].as_str(),
            b["requestId"].as_str(),
        )
    })
    .await
}
pub(crate) async fn delete(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(e) = boundary(&request, false) {
        return *e;
    }
    let b = match body(request, &["ifState", "requestId", "confirm"]).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    if !b["ifState"].is_string() || !b["requestId"].is_string() || !b["confirm"].is_boolean() {
        return invalid();
    };
    let manager = state.application.workspaces.clone();
    run(move || {
        manager.delete(
            &id,
            b["ifState"].as_str().unwrap(),
            b["requestId"].as_str().unwrap(),
            b["confirm"].as_bool().unwrap(),
        )
    })
    .await
}
struct FileStream {
    receiver: tokio::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>,
    _leases: Vec<photo_files::FileLease>,
}
impl Stream for FileStream {
    type Item = std::io::Result<Vec<u8>>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx)
    }
}
pub(crate) fn stream_with(
    download: WorkspaceDownload,
    headers: &axum::http::HeaderMap,
) -> Response<Body> {
    let total = download.byte_length;
    let etag = format!("\"{}\"", download.revision);
    let mut start = 0u64;
    let mut end = total.saturating_sub(1);
    let mut status = StatusCode::OK;
    if let Some(raw) = headers
        .get("Range")
        .and_then(|v| v.to_str().ok())
        .filter(|_| {
            headers
                .get("If-Range")
                .is_none_or(|v| v.to_str().ok() == Some(etag.as_str()))
        })
    {
        let parsed = raw
            .strip_prefix("bytes=")
            .filter(|s| !s.contains(','))
            .and_then(|s| s.split_once('-'))
            .and_then(|(a, b)| {
                let a = a.parse::<u64>().ok()?;
                let b = if b.is_empty() {
                    end
                } else {
                    b.parse::<u64>().ok()?
                };
                (a <= b && b < total).then_some((a, b))
            });
        let Some((a, b)) = parsed else {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header("Content-Range", format!("bytes */{total}"))
                .body(Body::empty())
                .unwrap();
        };
        start = a;
        end = b;
        status = StatusCode::PARTIAL_CONTENT;
    }
    let length = if total == 0 {
        0
    } else {
        end.saturating_sub(start) + 1
    };
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    let file = download.file;
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let mut file = tokio::fs::File::from_std(file);
        if let Err(e) = file.seek(std::io::SeekFrom::Start(start)).await {
            let _ = sender.send(Err(e)).await;
            return;
        }
        let mut remaining = length;
        let mut buffer = vec![0u8; 128 * 1024];
        while remaining > 0 {
            let amount = remaining.min(buffer.len() as u64) as usize;
            match file.read(&mut buffer[..amount]).await {
                Ok(0) => {
                    let _ = sender
                        .send(Err(std::io::Error::other("Bound file ended early")))
                        .await;
                    break;
                }
                Ok(n) => {
                    remaining -= n as u64;
                    if sender.send(Ok(buffer[..n].to_vec())).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = sender.send(Err(e)).await;
                    break;
                }
            }
        }
    });
    let filename = download.filename.replace(['"', '\r', '\n'], "_");
    let mut response = Response::builder()
        .status(status)
        .header("Content-Type", download.content_type)
        .header("Content-Length", length)
        .header("Accept-Ranges", "bytes")
        .header("ETag", etag)
        .header(
            "Content-Disposition",
            format!("attachment; filename=\"{filename}\""),
        )
        .header("X-Sha256", download.sha256)
        .header("X-Revision", download.revision)
        .header("Cache-Control", "no-store");
    if status == StatusCode::PARTIAL_CONTENT {
        response = response.header("Content-Range", format!("bytes {start}-{end}/{total}"));
    }
    response
        .body(Body::from_stream(FileStream {
            receiver,
            _leases: download.leases,
        }))
        .expect("validated file response")
}
