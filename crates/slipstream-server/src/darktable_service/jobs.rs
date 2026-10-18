use super::processing_admission as admission;
use super::{Result, ServiceError, process, transfer::Transfer};
use parking_lot::{Condvar, Mutex};
use photo_files::{Directory, atomic_json, read_json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
const CAPACITY: usize = 2;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Job {
    #[serde(default)]
    render_id: String,
    photo_id: String,
    workspace_id: String,
    edit_id: String,
    request_id: String,
    state_directory: PathBuf,
    operation: String,
    arguments: Value,
    capture: Value,
    status: String,
    result: Option<Value>,
    transfer: Option<Transfer>,
    created_at: u64,
}
struct Inner {
    directory: Directory,
    jobs: Mutex<HashMap<String, Job>>,
    active: Mutex<usize>,
    ready: Condvar,
    cancel: Mutex<HashMap<String, Arc<AtomicBool>>>,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
    stopping: AtomicBool,
}
pub(crate) struct Jobs(Arc<Inner>);
pub(crate) struct Admission<'a> {
    pub state: &'a Path,
    pub photo: &'a str,
    pub workspace: &'a str,
    pub edit: &'a str,
    pub operation: &'a str,
    pub arguments: Value,
    pub transfer: Option<Transfer>,
}
fn key(photo: &str, id: &str) -> String {
    format!("{photo}--{id}")
}
impl Jobs {
    pub(crate) fn open(root: &Directory) -> Result<Self> {
        let directory = root.create_directory("jobs").map_err(ServiceError::io)?;
        let mut jobs = HashMap::new();
        for entry in directory.entries(10000).map_err(ServiceError::io)? {
            if !entry.name.ends_with(".json") {
                continue;
            }
            let value = read_json(
                directory.open_file(&entry.name).map_err(ServiceError::io)?,
                1024 * 1024,
            )
            .map_err(ServiceError::io)?;
            let mut job: Job = serde_json::from_value(value).map_err(ServiceError::io)?;
            if matches!(job.status.as_str(), "queued" | "running" | "unknown") {
                match process::invoke(
                    Some(&job.state_directory),
                    "__receipt",
                    json!({"requestId":job.request_id}),
                    &AtomicBool::new(false),
                    Duration::from_secs(30),
                ) {
                    Ok(receipt) if receipt["state"] == "settled" => {
                        settle(&mut job, receipt["result"].clone());
                    }
                    Ok(receipt)
                        if receipt["state"] == "pending"
                            && receipt["captureId"] == job.capture["captureId"] =>
                    {
                        job.status = "queued".into();
                        job.result = None;
                    }
                    _ => {
                        job.status = "unknown".into();
                        job.result = Some(
                            json!({"error":{"code":"unknown_result","effect":"unknown","message":"Reconcile retained tool receipt before retrying","details":{}}}),
                        );
                    }
                }
                atomic_json(&directory, &entry.name, &job).map_err(ServiceError::io)?;
            }
            jobs.insert(key(&job.photo_id, &job.request_id), job);
        }
        let service = Self(Arc::new(Inner {
            directory,
            jobs: Mutex::new(jobs),
            active: Mutex::new(0),
            ready: Condvar::new(),
            cancel: Mutex::new(HashMap::new()),
            threads: Mutex::new(Vec::new()),
            stopping: AtomicBool::new(false),
        }));
        let queued = service
            .0
            .jobs
            .lock()
            .iter()
            .filter(|(_, j)| j.status == "queued")
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>();
        for key in queued {
            service.start(key);
        }
        Ok(service)
    }
    pub(crate) fn admit(&self, request: Admission<'_>) -> Result<Value> {
        let Admission {
            state,
            photo,
            workspace,
            edit,
            operation,
            mut arguments,
            transfer,
        } = request;
        let id = arguments["requestId"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or(photo_files::identity("request").map_err(ServiceError::io)?);
        if !super::valid_id(photo) || !super::valid_id(&id) {
            return Err(ServiceError::new(
                "invalid_input",
                "Invalid request identity",
                400,
            ));
        }
        arguments["requestId"] = json!(id);
        let key = key(photo, &id);
        let mut jobs = self.0.jobs.lock();
        if let Some(existing) = jobs.get(&key) {
            if existing.operation != operation
                || existing.arguments != arguments
                || existing.edit_id != edit
            {
                return Err(ServiceError::new(
                    "request_conflict",
                    "Request identity has different intent",
                    409,
                ));
            }
            return Ok(view(existing));
        }
        if self.0.stopping.load(Ordering::Acquire) {
            return Err(ServiceError::new(
                "service_unavailable",
                "Service is shutting down",
                503,
            ));
        }
        let mut capture_args = arguments.clone();
        capture_args["operation"] = json!(operation);
        // Canonical state/dependency retention precedes queue admission.
        let capture = process::invoke(
            Some(state),
            "__capture-render",
            capture_args,
            &AtomicBool::new(false),
            Duration::from_secs(60),
        )?;
        if jobs
            .values()
            .filter(|j| matches!(j.status.as_str(), "queued" | "running"))
            .count()
            >= CAPACITY
        {
            process::invoke(
                Some(state),
                "__release-capture",
                json!({"captureId":capture["captureId"],"reason":"failed"}),
                &AtomicBool::new(false),
                Duration::from_secs(30),
            )?;
            return Err(ServiceError::new(
                "queue_full",
                "Heavy-work queue is full",
                429,
            ));
        }
        let job = Job {
            render_id: photo_files::identity("render").map_err(ServiceError::io)?,
            photo_id: photo.into(),
            workspace_id: workspace.into(),
            edit_id: edit.into(),
            request_id: id,
            state_directory: state.into(),
            operation: operation.into(),
            arguments,
            capture,
            status: "queued".into(),
            result: None,
            transfer,
            created_at: photo_files::timestamp().map_err(ServiceError::io)?,
        };
        atomic_json(&self.0.directory, format!("{key}.json"), &job).map_err(ServiceError::io)?;
        let result = view(&job);
        jobs.insert(key.clone(), job);
        drop(jobs);
        self.start(key);
        Ok(result)
    }
    fn start(&self, key: String) {
        let inner = self.0.clone();
        let requested = inner
            .jobs
            .lock()
            .get(&key)
            .is_some_and(|j| j.capture["cancelRequested"] == true);
        let cancel = Arc::new(AtomicBool::new(requested));
        self.0.cancel.lock().insert(key.clone(), cancel.clone());
        let thread = std::thread::spawn(move || {
            let mut active = inner.active.lock();
            while *active >= 1
                && !cancel.load(Ordering::Acquire)
                && !inner.stopping.load(Ordering::Acquire)
            {
                inner.ready.wait(&mut active);
            }
            *active += 1;
            drop(active);
            let mut job = inner.jobs.lock()[&key].clone();
            if !cancel.load(Ordering::Acquire) && !inner.stopping.load(Ordering::Acquire) {
                job.status = "running".into();
                if atomic_json(&inner.directory, format!("{key}.json"), &job).is_ok() {
                    inner.jobs.lock().insert(key.clone(), job.clone());
                    let args =
                        json!({"captureId":job.capture["captureId"],"requestId":job.request_id});
                    let host_state = inner
                        .directory
                        .path()
                        .parent()
                        .and_then(Path::parent)
                        .unwrap();
                    let result =
                        admission::acquire_cancellable(host_state, &cancel, &inner.stopping)
                            .and_then(|_lease| {
                                process::invoke(
                                    Some(&job.state_directory),
                                    &job.operation,
                                    args,
                                    &cancel,
                                    Duration::from_secs(15 * 60),
                                )
                            });
                    match result {
                        Ok(value) => settle(&mut job, value),
                        Err(error) => {
                            job.status = if error.value["error"]["effect"] == "unknown" {
                                "unknown"
                            } else {
                                "failed"
                            }
                            .into();
                            job.result = Some(error.value);
                        }
                    }
                } else {
                    job.status = "unknown".into();
                }
            } else {
                job.status = "cancelled".into();
            }
            if matches!(job.status.as_str(), "cancelled" | "failed") {
                match process::invoke(
                    Some(&job.state_directory),
                    "__release-capture",
                    json!({"captureId":job.capture["captureId"],"reason":if job.status=="cancelled"{"cancelled"}else{"failed"}}),
                    &AtomicBool::new(false),
                    Duration::from_secs(30),
                ) {
                    Ok(value) => settle(&mut job, value),
                    Err(error) if error.value["error"]["code"] == "file_committed" => {
                        let reconcile = AtomicBool::new(false);
                        let host_state = inner
                            .directory
                            .path()
                            .parent()
                            .and_then(Path::parent)
                            .unwrap();
                        let result = admission::acquire_cancellable(
                            host_state,
                            &reconcile,
                            &inner.stopping,
                        )
                        .and_then(|_lease| {
                            process::invoke(
                                Some(&job.state_directory),
                                &job.operation,
                                json!({"captureId":job.capture["captureId"],"requestId":job.request_id}),
                                &reconcile,
                                Duration::from_secs(15 * 60),
                            )
                        });
                        match result {
                            Ok(value) => settle(&mut job, value),
                            Err(error) => {
                                job.status = "unknown".into();
                                job.result = Some(error.value);
                            }
                        }
                    }
                    Err(_) => job.status = "unknown".into(),
                }
            }
            if job.status == "unknown"
                && let Ok(receipt) = process::invoke(
                    Some(&job.state_directory),
                    "__receipt",
                    json!({"requestId":job.request_id}),
                    &AtomicBool::new(false),
                    Duration::from_secs(30),
                )
                && receipt["state"] == "settled"
            {
                settle(&mut job, receipt["result"].clone());
            }
            if atomic_json(&inner.directory, format!("{key}.json"), &job).is_ok() {
                inner.jobs.lock().insert(key, job);
            }
            let mut active = inner.active.lock();
            *active -= 1;
            inner.ready.notify_all();
        });
        self.0.threads.lock().push(thread);
    }
    pub(crate) fn status(&self, photo: &str, id: &str, cancel: bool) -> Result<Value> {
        let key = key(photo, id);
        let mut jobs = self.0.jobs.lock();
        let job = jobs
            .get_mut(&key)
            .ok_or_else(|| ServiceError::new("not_found", "Request not found", 404))?;
        if cancel && matches!(job.status.as_str(), "queued" | "running") {
            // Durable intent precedes killing the owner; terminal receipt wins reconciliation.
            job.capture["cancelRequested"] = json!(true);
            atomic_json(&self.0.directory, format!("{key}.json"), job).map_err(ServiceError::io)?;
            if let Some(flag) = self.0.cancel.lock().get(&key) {
                flag.store(true, Ordering::Release);
                self.0.ready.notify_all();
            }
        }
        Ok(view(job))
    }
    pub(crate) fn list(&self, photo: &str, edit: Option<&str>) -> Value {
        json!({"jobs":self.0.jobs.lock().values().filter(|j|j.photo_id==photo&&edit.is_none_or(|id|j.edit_id==id)).map(view).collect::<Vec<_>>()})
    }
    pub(crate) fn preview_status(&self, id: &str) -> Result<Value> {
        let jobs = self.0.jobs.lock();
        let j = jobs
            .values()
            .find(|j| j.render_id == id && j.operation == "preview render")
            .ok_or_else(|| ServiceError::new("not_found", "Preview not found", 404))?;
        Ok(view(j))
    }
    pub(crate) fn completed(&self, id: &str) -> Result<(String, Value)> {
        let jobs = self.0.jobs.lock();
        let job = jobs
            .values()
            .find(|j| j.render_id == id)
            .ok_or_else(|| ServiceError::new("not_found", "Completed artifact not found", 404))?;
        if !matches!(job.status.as_str(), "succeeded" | "settled") {
            return Err(ServiceError::new(
                "state_conflict",
                "Render has not succeeded",
                409,
            ));
        }
        let mut result = job
            .result
            .clone()
            .ok_or_else(|| ServiceError::unknown("Job result missing"))?;
        result["photoId"] = json!(job.photo_id);
        Ok((job.workspace_id.clone(), result))
    }
    pub(crate) fn retry(&self, photo: &str, id: &str, new_id: Option<&str>) -> Result<Value> {
        if self.0.stopping.load(Ordering::Acquire) {
            return Err(ServiceError::new(
                "service_unavailable",
                "Service is shutting down",
                503,
            ));
        }
        let original = self
            .0
            .jobs
            .lock()
            .get(&key(photo, id))
            .cloned()
            .ok_or_else(|| ServiceError::new("not_found", "Retry source not found", 404))?;
        if !matches!(original.status.as_str(), "failed" | "cancelled") {
            return Err(ServiceError::new(
                "state_conflict",
                "Retry requires failed or cancelled work",
                409,
            ));
        }
        let new_id = new_id
            .map(str::to_owned)
            .unwrap_or(photo_files::identity("request").map_err(ServiceError::io)?);
        if !super::valid_id(&new_id) {
            return Err(ServiceError::new(
                "invalid_input",
                "Invalid retry identity",
                400,
            ));
        }
        let new_key = key(photo, &new_id);
        let mut jobs = self.0.jobs.lock();
        if let Some(job) = jobs.get(&new_key) {
            if job.capture["retryOf"] != original.capture["captureId"] {
                return Err(ServiceError::new(
                    "request_conflict",
                    "Retry identity has different source",
                    409,
                ));
            }
            return Ok(view(job));
        }
        if jobs
            .values()
            .filter(|j| matches!(j.status.as_str(), "queued" | "running"))
            .count()
            >= CAPACITY
        {
            return Err(ServiceError::new(
                "queue_full",
                "Heavy-work queue is full",
                429,
            ));
        }
        let capture = process::invoke(
            Some(&original.state_directory),
            "__retry-capture",
            json!({"captureId":original.capture["captureId"],"requestId":new_id}),
            &AtomicBool::new(false),
            Duration::from_secs(60),
        )?;
        let mut job = original;
        job.render_id = photo_files::identity("render").map_err(ServiceError::io)?;
        job.request_id = new_id;
        job.capture = capture;
        job.arguments["requestId"] = json!(job.request_id);
        job.status = "queued".into();
        job.result = None;
        job.created_at = photo_files::timestamp().map_err(ServiceError::io)?;
        atomic_json(&self.0.directory, format!("{new_key}.json"), &job)
            .map_err(ServiceError::io)?;
        let result = view(&job);
        jobs.insert(new_key.clone(), job);
        drop(jobs);
        self.start(new_key);
        Ok(result)
    }
    pub(crate) fn begin_shutdown(&self) {
        self.0.stopping.store(true, Ordering::Release);
        for flag in self.0.cancel.lock().values() {
            flag.store(true, Ordering::Release);
        }
        self.0.ready.notify_all();
    }
    pub(crate) async fn shutdown(&self) -> Result<()> {
        self.begin_shutdown();
        let threads = std::mem::take(&mut *self.0.threads.lock());
        tokio::task::spawn_blocking(move || {
            for thread in threads {
                let _ = thread.join();
            }
        })
        .await
        .map_err(ServiceError::io)?;
        Ok(())
    }
}
fn view(j: &Job) -> Value {
    let state = match j.status.as_str() {
        "queued" => "accepted",
        "running" => "executing",
        other => other,
    };
    let mut result = j.result.clone();
    if let Some(value) = result.as_mut() {
        value["artifactId"] = json!(j.render_id);
        value["stateFileId"] = json!(j.render_id);
    }
    json!({"photoId":j.photo_id,"workspaceId":j.workspace_id,"editId":j.edit_id,"requestId":j.request_id,"previewId":if j.operation=="preview render"{Some(j.render_id.as_str())}else{None},"status":state,"state":state,"revision":j.capture["revision"],"capture":j.capture,"result":result,"createdAt":j.created_at})
}
fn settled_status(value: &Value) -> &'static str {
    if value["fileCommitted"] == true {
        "succeeded"
    } else if value["outcome"] == "cancelled" {
        "cancelled"
    } else {
        "failed"
    }
}
fn settle(job: &mut Job, mut value: Value) {
    job.status = settled_status(&value).into();
    if job.status == "succeeded"
        && let Some(transfer) = &job.transfer
    {
        let result = Directory::open(transfer.path.parent().unwrap())
            .map_err(ServiceError::io)
            .and_then(|directory| super::transfer::finish(&directory, transfer, &mut value));
        if let Err(error) = result {
            job.status = "unknown".into();
            job.result = Some(error.value);
            return;
        }
    }
    job.result = Some(value);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settled_state_file_publishes_verified_transfer_receipt() {
        use std::io::Write;
        let path = std::env::temp_dir().join(photo_files::identity("settle-test").unwrap());
        let directory = Directory::create(&path).unwrap();
        directory
            .create_file("state.xmp", 0o600)
            .unwrap()
            .write_all(b"retained state file")
            .unwrap();
        let transfer = Transfer {
            id: "transfer-test".into(),
            scope: Some("photo-test".into()),
            filename: "state.xmp".into(),
            path: path.join("state.xmp"),
        };
        let mut job = Job {
            render_id: "render-test".into(),
            photo_id: "photo-test".into(),
            workspace_id: "workspace-test".into(),
            edit_id: "edit-test".into(),
            request_id: "request-test".into(),
            state_directory: path.clone(),
            operation: "export state-file".into(),
            arguments: json!({}),
            capture: json!({}),
            status: "unknown".into(),
            result: None,
            transfer: Some(transfer),
            created_at: 0,
        };
        settle(
            &mut job,
            json!({"fileCommitted":true,"requestId":"request-test"}),
        );
        assert_eq!(job.status, "succeeded");
        assert_eq!(job.result.as_ref().unwrap()["transferId"], "transfer-test");
        let receipt = read_json(directory.open_file("receipt.json").unwrap(), 16384).unwrap();
        assert_eq!(receipt["transfer"]["id"], "transfer-test");
        assert_eq!(receipt["evidence"]["observation"]["byteLength"], 19);
        std::fs::remove_dir_all(path).unwrap();
    }
}
