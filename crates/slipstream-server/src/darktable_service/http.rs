use super::{ServiceError, ToolService, transfer::MAX_BYTES};
use crate::{
    http::{HttpState, json_response},
    workspace_http::boundary,
};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, Response, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Command {
    operation: String,
    edit_id: Option<String>,
    arguments: Value,
}
fn response(result: super::Result<(u16, Value)>) -> Response<Body> {
    match result {
        Ok((status, value)) => json_response(StatusCode::from_u16(status).unwrap(), &value),
        Err(e) => json_response(
            StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            &e.value,
        ),
    }
}
fn invalid(message: &str) -> Response<Body> {
    response(Err(ServiceError::new("invalid_input", message, 400)))
}
async fn command(
    state: HttpState,
    photo: Option<String>,
    request: Request<Body>,
) -> Response<Body> {
    let service = state.application.tools.clone();
    if let Err(e) = boundary(&request, false) {
        return *e;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), 65536).await {
        Ok(b) => b,
        Err(_) => return invalid("Command body exceeds 64 KiB"),
    };
    let value = match photo_files::read_json(bytes.as_ref(), 65536) {
        Ok(v) => v,
        Err(_) => return invalid("Command must be bounded JSON without duplicate keys"),
    };
    let parsed: Command = match serde_json::from_value(value) {
        Ok(v) => v,
        Err(_) => return invalid("Expected operation, editId and arguments"),
    };
    if !parsed.arguments.is_object()
        || parsed.operation.len() > 128
        || parsed
            .edit_id
            .as_deref()
            .is_some_and(|e| !super::valid_id(e))
    {
        return invalid("Invalid operation/edit selector/arguments");
    }
    if let Some(photo) = photo.as_deref() {
        if matches!(
            parsed.operation.as_str(),
            "preview proxy get" | "preview proxy create" | "preview proxy remove"
        ) {
            let Some(object) = parsed.arguments.as_object() else {
                return invalid("Proxy arguments must be an object");
            };
            if object
                .keys()
                .any(|k| !matches!(k.as_str(), "input" | "ifState" | "requestId"))
            {
                return invalid("Unknown proxy argument");
            }
            let service = service.clone();
            let proxy_photo = photo.to_owned();
            let operation = parsed.operation.clone();
            let arguments = parsed.arguments.clone();
            let admission = match tokio::task::spawn_blocking(move || {
                service.admit_proxy(&proxy_photo, &operation, &arguments)
            })
            .await
            {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => return response(Err(e)),
                Err(_) => return response(Err(ServiceError::unknown("Proxy admission failed"))),
            };
            if let Some(result) = admission.replay.as_ref() {
                return response(Ok(result.clone()));
            }
            let mut builder = Request::builder().method("POST");
            builder = builder.header("Content-Type", "application/json");
            let body = if parsed.operation == "preview proxy create" {
                parsed.arguments["input"].to_string()
            } else {
                "{}".into()
            };
            let request = builder.body(Body::from(body)).unwrap();
            let selector = Path(photo.to_owned());
            let result = match parsed.operation.as_str() {
                "preview proxy get" => {
                    crate::development_proxy::get_development_proxy(
                        State(state.clone()),
                        selector,
                        request,
                    )
                    .await
                }
                "preview proxy create" => {
                    crate::development_proxy::post_development_proxy(
                        State(state.clone()),
                        selector,
                        request,
                    )
                    .await
                }
                _ => {
                    crate::development_proxy::delete_development_proxy(
                        State(state.clone()),
                        selector,
                        request,
                    )
                    .await
                }
            };
            let status = result.status().as_u16();
            let bytes = match axum::body::to_bytes(result.into_body(), 1024 * 1024).await {
                Ok(b) => b,
                Err(_) => return response(Err(ServiceError::unknown("Proxy result unavailable"))),
            };
            let mut value = match photo_files::read_json(bytes.as_ref(), 1024 * 1024) {
                Ok(v) => v,
                Err(_) => return response(Err(ServiceError::unknown("Proxy result is not JSON"))),
            };
            let workspaces = state.application.workspaces.clone();
            let proxy_photo = photo.to_owned();
            if let Ok(Ok(workspace)) =
                tokio::task::spawn_blocking(move || workspaces.resolve_photo(&proxy_photo)).await
            {
                value["workspaceStateToken"] = workspace["workspaceStateToken"].clone();
                value["guardContext"] = json!("workspace");
            }
            if parsed.operation != "preview proxy get" {
                let stored = value.clone();
                let receipt = tokio::task::spawn_blocking(move || {
                    admission.receipt.publish(
                        &json!({"payload":admission.payload,"status":status,"result":stored}),
                    )
                })
                .await;
                if !matches!(receipt, Ok(Ok(()))) {
                    return response(Err(ServiceError::unknown(
                        "Proxy result persistence failed",
                    )));
                }
            }
            return response(Ok((status, value)));
        }
        if matches!(
            parsed.operation.as_str(),
            "edit migration show" | "edit migrate"
        ) {
            let read = match state
                .application
                .library
                .composable_edit_recipe_read(photo)
                .await
            {
                Ok(value) => value,
                Err(e) => return response(Err(ServiceError::io(e))),
            };
            let steps = read
                .and_then(|r| r.recipe)
                .map(|r| {
                    r.steps
                        .into_iter()
                        .filter(|s| s.module.as_str() == "darktable")
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let selected = if let Some(edit) = parsed.edit_id.as_deref() {
                steps.iter().find(|s| s.step_id.as_str() == edit)
            } else if steps.len() == 1 {
                steps.first()
            } else {
                None
            };
            if let Some(step) = selected {
                let (input, legacy) = match crate::processing_recipe::legacy_darktable_input(
                    &state,
                    photo,
                    step.step_id.as_str(),
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => return response(Err(ServiceError::new("state_conflict", e, 409))),
                };
                let operation = parsed.operation;
                let arguments = parsed.arguments;
                let photo = photo.to_owned();
                return response(
                    match tokio::task::spawn_blocking(move || {
                        if operation == "edit migration show" {
                            if arguments
                                .as_object()
                                .unwrap()
                                .keys()
                                .any(|k| !matches!(k.as_str(), "bundleId" | "schemaVersion"))
                            {
                                return Err(ServiceError::new(
                                    "invalid_input",
                                    "Unknown migration argument",
                                    400,
                                ));
                            }
                            let review = service.review_legacy(&photo, &input, legacy)?;
                            if review["bundleId"] != arguments["bundleId"]
                                || review["schemaVersion"] != arguments["schemaVersion"]
                            {
                                return Err(ServiceError::new(
                                    "schema_unsupported",
                                    "Requested migration target is not installed",
                                    422,
                                ));
                            }
                            Ok((200, review))
                        } else {
                            if arguments.as_object().unwrap().keys().any(|k| {
                                !matches!(
                                    k.as_str(),
                                    "migrationToken" | "ifState" | "requestId" | "confirm"
                                )
                            }) {
                                return Err(ServiceError::new(
                                    "invalid_input",
                                    "Unknown migration argument",
                                    400,
                                ));
                            }
                            let record = service.legacy_review(&photo, &arguments)?;
                            if record["legacy"] != legacy || record["input"] != input {
                                return Err(ServiceError::new(
                                    "state_conflict",
                                    "Retained migration source changed since review",
                                    409,
                                ));
                            }
                            service
                                .apply_legacy_review(&photo, &arguments, &record)
                                .map(|v| (201, v))
                        }
                    })
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => Err(ServiceError::unknown("Migration worker failed")),
                    },
                );
            } else if !steps.is_empty() && parsed.edit_id.is_none() {
                return response(Err(ServiceError::new(
                    "edit_ambiguous",
                    "Select the retained darktable edit explicitly",
                    409,
                )));
            }
        }
    }
    response(
        match tokio::task::spawn_blocking(move || {
            service.command(
                photo.as_deref(),
                parsed.edit_id.as_deref(),
                &parsed.operation,
                parsed.arguments,
            )
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ServiceError::unknown(
                "Command worker failed; reconcile request identity",
            )),
        },
    )
}
pub(crate) async fn photo_commands(
    State(state): State<HttpState>,
    Path(photo): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    command(state, Some(photo), request).await
}
pub(crate) async fn global_commands(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    command(state, None, request).await
}
async fn job(
    service: Arc<ToolService>,
    photo: String,
    id: String,
    request: Request<Body>,
    cancel: bool,
) -> Response<Body> {
    if let Err(e) = boundary(&request, false) {
        return *e;
    }
    if !super::valid_id(&id) || !super::valid_id(&photo) {
        return invalid("Invalid job identity");
    }
    if cancel {
        match axum::body::to_bytes(request.into_body(), 65536).await {
            Ok(b) => {
                let value = if b.is_empty() {
                    json!({})
                } else {
                    match photo_files::read_json(b.as_ref(), 65536) {
                        Ok(v) => v,
                        Err(_) => return invalid("Invalid cancel JSON"),
                    }
                };
                let Some(o) = value.as_object() else {
                    return invalid("Cancel must be an object");
                };
                if o.keys().any(|k| k != "requestId")
                    || o.get("requestId")
                        .is_some_and(|v| !v.as_str().is_some_and(super::valid_id))
                {
                    return invalid("Invalid cancel request identity");
                }
            }
            _ => return invalid("Cancel body exceeds bound"),
        }
    }
    response(
        match tokio::task::spawn_blocking(move || {
            service.jobs.status(&photo, &id, cancel).map(|v| (200, v))
        })
        .await
        {
            Ok(r) => r,
            Err(_) => Err(ServiceError::io("Job worker failed")),
        },
    )
}
pub(crate) async fn status(
    State(state): State<HttpState>,
    Path((photo, id)): Path<(String, String)>,
    request: Request<Body>,
) -> Response<Body> {
    job(state.application.tools.clone(), photo, id, request, false).await
}
pub(crate) async fn cancel(
    State(state): State<HttpState>,
    Path((photo, id)): Path<(String, String)>,
    request: Request<Body>,
) -> Response<Body> {
    job(state.application.tools.clone(), photo, id, request, true).await
}
async fn upload(
    service: Arc<ToolService>,
    photo: Option<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(e) = boundary(&request, false) {
        return *e;
    }
    let name = match request
        .headers()
        .get("X-Filename")
        .and_then(|v| v.to_str().ok())
    {
        Some(s) => s.to_owned(),
        None => return invalid("X-Filename required"),
    };
    let sha = match request
        .headers()
        .get("X-Sha256")
        .and_then(|v| v.to_str().ok())
    {
        Some(s) if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) => s.to_lowercase(),
        _ => return invalid("X-Sha256 required"),
    };
    let length = match request
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
    {
        Some(n) if n > 0 && n <= MAX_BYTES => n,
        _ => return invalid("Finite Content-Length within 256 MiB required"),
    };
    let start_service = service.clone();
    let start =
        tokio::task::spawn_blocking(move || start_service.upload_start(photo.as_deref(), &name))
            .await;
    let (transfer, file) = match start {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return response(Err(e)),
        Err(_) => return invalid("Upload worker failed"),
    };
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(2);
    let writer = tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let mut file = file;
        while let Some(bytes) = receiver.blocking_recv() {
            file.write_all(&bytes).map_err(ServiceError::io)?;
        }
        file.sync_all().map_err(ServiceError::io)
    });
    let mut body = request.into_body().into_data_stream();
    let mut received = 0u64;
    loop {
        let frame = std::future::poll_fn(|cx| {
            futures_core::Stream::poll_next(std::pin::Pin::new(&mut body), cx)
        })
        .await;
        match frame {
            None => break,
            Some(Ok(bytes)) => {
                received += bytes.len() as u64;
                if received > length {
                    drop(sender);
                    let _ = writer.await;
                    return invalid("Upload length exceeded declared bound");
                }
                if sender.send(bytes.to_vec()).await.is_err() {
                    return invalid("Upload storage failed");
                }
            }
            Some(Err(_)) => {
                drop(sender);
                let _ = writer.await;
                return invalid("Upload interrupted");
            }
        }
    }
    drop(sender);
    match writer.await {
        Ok(Ok(())) => (),
        Ok(Err(e)) => return response(Err(e)),
        Err(_) => return invalid("Upload writer failed"),
    };
    if received != length {
        return invalid("Upload length differs from Content-Length");
    }
    response(
        match tokio::task::spawn_blocking(move || {
            service.upload_finish(&transfer, &sha).map(|v| (201, v))
        })
        .await
        {
            Ok(r) => r,
            Err(_) => Err(ServiceError::io("Upload finalization failed")),
        },
    )
}
pub(crate) async fn photo_upload(
    State(state): State<HttpState>,
    Path(photo): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    upload(state.application.tools.clone(), Some(photo), request).await
}
pub(crate) async fn global_upload(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    upload(state.application.tools.clone(), None, request).await
}
async fn download(
    service: Arc<ToolService>,
    photo: Option<String>,
    id: String,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(e) = boundary(&request, false) {
        return *e;
    }
    let headers = request.headers().clone();
    match tokio::task::spawn_blocking(move || service.transfer_download(photo.as_deref(), &id))
        .await
    {
        Ok(Ok(file)) => crate::workspace_http::stream_with(file, &headers),
        Ok(Err(e)) => response(Err(e)),
        Err(_) => invalid("Download worker failed"),
    }
}
pub(crate) async fn photo_download(
    State(state): State<HttpState>,
    Path((photo, id)): Path<(String, String)>,
    request: Request<Body>,
) -> Response<Body> {
    download(state.application.tools.clone(), Some(photo), id, request).await
}
pub(crate) async fn global_download(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    download(state.application.tools.clone(), None, id, request).await
}
pub(crate) async fn preview_status(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(e) = boundary(&request, false) {
        return *e;
    }
    response(
        state
            .application
            .tools
            .jobs
            .preview_status(&id)
            .map(|v| (200, v)),
    )
}
pub(crate) async fn rendered_bytes(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(e) = boundary(&request, true) {
        return *e;
    }
    let mut comparison = false;
    let mut seen = false;
    for (k, v) in url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes()) {
        if k != "comparison" || seen || !matches!(v.as_ref(), "true" | "false") {
            return invalid("Only comparison=true|false query is accepted");
        }
        seen = true;
        comparison = v == "true";
    }
    let headers = request.headers().clone();
    let service = state.application.tools.clone();
    match tokio::task::spawn_blocking(move || service.rendered_download(&id, comparison)).await {
        Ok(Ok((file, contract))) => {
            let mut result = crate::workspace_http::stream_with(file, &headers);
            if !contract.is_null() {
                match axum::http::HeaderValue::from_str(&contract.to_string()) {
                    Ok(value) => {
                        result.headers_mut().insert("X-Image-Contract", value);
                    }
                    Err(_) => {
                        return invalid("Image contract cannot be represented in transfer headers");
                    }
                }
            }
            result
        }
        Ok(Err(e)) => response(Err(e)),
        Err(_) => invalid("Rendered download worker failed"),
    }
}
