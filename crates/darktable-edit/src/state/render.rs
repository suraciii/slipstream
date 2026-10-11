use super::*;
use std::io::Write;
fn normalized(inv: &Invocation) -> Value {
    let mut args = inv.arguments.clone();
    if let Some(o) = args.as_object_mut() {
        o.remove("captureId");
    }
    json!({"operation":inv.operation,"arguments":args})
}
fn load(store: &Store, capture: &str) -> Result<Value, ToolError> {
    store
        .conn
        .query_row("SELECT snapshot FROM captures WHERE id=?", [capture], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(db)?
        .map(|s| serde_json::from_str(&s).map_err(db))
        .transpose()?
        .ok_or_else(|| err("capture_not_found", "Render capture does not exist"))
}
fn reject_preview_region(operation: &str, arguments: &Value) -> Result<(), ToolError> {
    if operation == "preview render" && !arguments["input"]["region"].is_null() {
        return Err(err(
            "schema_unsupported",
            "The qualified engine does not support preview regions",
        ));
    }
    Ok(())
}
pub(super) fn capture(store: &Store, inv: &Invocation) -> Result<Value, ToolError> {
    let _lock = store.lock()?;
    let request = inv.arguments["requestId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or(id("request")?);
    identifier(&request)?;
    let payload = digest(&normalized(inv))?;
    if let Some(result) = store.replay(&request, &payload)? {
        return Ok(result);
    }
    let existing: Option<(String, String)> = store
        .conn
        .query_row(
            "SELECT id,payload FROM captures WHERE request=?",
            [&request],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db)?;
    if let Some((capture, prior)) = existing {
        if prior != payload {
            return Err(err(
                "request_conflict",
                "Request identity has different render intent",
            ));
        }
        return load(store, &capture);
    }
    reject_preview_region(&inv.operation, &inv.arguments)?;
    let revision = store.revision(inv.arguments["revision"].as_str())?;
    let _input = verify_input(&revision)?;
    let engine = Engine::open()?;
    if engine.bundle_id() != revision["bundleId"].as_str().unwrap_or("")
        || engine.schema_version() != revision["schemaVersion"].as_str().unwrap_or("")
    {
        return Err(err(
            "schema_unsupported",
            "Captured revision requires its retained qualified engine",
        ));
    }
    let capture_id = id("render")?;
    let preview = inv.operation == "preview render";
    let state_file = inv.operation == "export state-file";
    let directory = if state_file {
        let file = absolute(Path::new(string(&inv.arguments, "file")?))?;
        file.parent().unwrap().to_owned()
    } else {
        absolute(Path::new(
            inv.arguments["directory"].as_str().unwrap_or(
                store
                    .meta(if preview {
                        "previewDirectory"
                    } else {
                        "outputDirectory"
                    })?
                    .as_str()
                    .unwrap_or(""),
            ),
        ))?
    };
    let extension = if preview {
        "png"
    } else {
        revision["snapshot"]["output"]["settings"]["format"]
            .as_str()
            .unwrap_or("tiff")
    };
    let name = if state_file {
        absolute(Path::new(string(&inv.arguments, "file")?))?
            .file_name()
            .unwrap()
            .to_str()
            .ok_or_else(|| err("invalid_input", "Output filename must be UTF-8"))?
            .to_owned()
    } else {
        inv.arguments["name"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or(format!("{capture_id}.{extension}"))
    };
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(err("invalid_input", "Output name must be one filename"));
    }
    let mut capture = revision.clone();
    capture["captureId"] = json!(capture_id);
    capture["requestId"] = json!(request);
    capture["stateDirectory"] = json!(store.dir.path());
    capture["operation"] = json!(inv.operation);
    capture["arguments"] = inv.arguments.clone();
    capture["directory"] = json!(directory);
    capture["name"] = json!(name);
    capture["path"] = json!(directory.join(&name));
    capture["temporaryName"] = json!(format!(".{capture_id}.pending"));
    capture["retainUntil"] = json!(deadline());
    capture["state"] = json!("accepted");
    if preview {
        let options = &inv.arguments["input"];
        if !options.is_null() {
            let object = options
                .as_object()
                .ok_or_else(|| err("invalid_input", "Preview options must be object"))?;
            if object.keys().any(|k| k != "comparison" && k != "region") {
                return Err(err("invalid_input", "Unknown preview option"));
            }
            if !options["comparison"].is_null() {
                let mut comparison = if options["comparison"] == "baseline" {
                    store.revision(Some(store.meta("baseline")?.as_str().unwrap_or("")))?
                } else if let Some(path) = options["comparison"]["stateDirectory"].as_str() {
                    let other = Store::read(Path::new(path))?;
                    other.revision(options["comparison"]["revision"].as_str())?
                } else {
                    store.revision(Some(string(&options["comparison"], "revision")?))?
                };
                if comparison["input"]["fingerprint"]["sha256"]
                    != revision["input"]["fingerprint"]["sha256"]
                    || comparison["input"]["fingerprint"]["byteLength"]
                        != revision["input"]["fingerprint"]["byteLength"]
                {
                    return Err(err(
                        "input_binding_conflict",
                        "Comparison input content differs from current acknowledged input",
                    ));
                }
                if comparison["bundleId"] != revision["bundleId"]
                    || comparison["schemaVersion"] != revision["schemaVersion"]
                {
                    return Err(err(
                        "schema_unsupported",
                        "Comparison requires the same retained qualified engine",
                    ));
                }
                comparison["input"] = revision["input"].clone();
                comparison["dependencies"] = catalog::pin_external_dependencies(
                    store.dir.path(),
                    &comparison["dependencies"],
                )?;
                capture["comparison"] = comparison;
            }
        }
    }
    store.conn.execute("INSERT INTO captures (id,revision,snapshot,input,dependencies,result,payload,request) VALUES (?,?,?,?,?,NULL,?,?)",params![capture_id,revision["revision"].as_str(),capture.to_string(),revision["input"].to_string(),revision["dependencies"].to_string(),payload,request]).map_err(db)?;
    store.dir.sync().map_err(io)?;
    Ok(capture)
}
pub(super) fn receipt(store: &Store, request: &str) -> Result<Value, ToolError> {
    let result: Option<String> = store
        .conn
        .query_row("SELECT result FROM receipts WHERE id=?", [request], |r| {
            r.get(0)
        })
        .optional()
        .map_err(db)?;
    if let Some(s) = result {
        return Ok(
            json!({"state":"settled","result":serde_json::from_str::<Value>(&s).map_err(db)?}),
        );
    }
    let capture: Option<String> = store
        .conn
        .query_row("SELECT id FROM captures WHERE request=?", [request], |r| {
            r.get(0)
        })
        .optional()
        .map_err(db)?;
    Ok(match capture {
        Some(id) => json!({"state":"pending","captureId":id}),
        None => json!({"state":"unknown","result":null}),
    })
}
pub(super) fn release(store: &Store, args: &Value) -> Result<Value, ToolError> {
    let capture_id = string(args, "captureId")?;
    identifier(capture_id)?;
    let locks = store.dir.child("render-locks").map_err(io)?;
    let file = match locks.create_file(capture_id, 0o600) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            locks.open_file(capture_id).map_err(io)?
        }
        Err(error) => return Err(io(error)),
    };
    let _execution = FileLease::acquire(file, LeaseMode::Exclusive, Duration::from_secs(5))
        .map_err(|e| err("render_busy", e.to_string()))?;
    let _lock = store.lock()?;
    let capture = load(store, capture_id)?;
    let reason = string(args, "reason")?;
    if reason != "failed" && reason != "cancelled" {
        return Err(err(
            "invalid_input",
            "Release reason must be failed or cancelled",
        ));
    }
    let request = string(&capture, "requestId")?;
    let payload: String = store
        .conn
        .query_row(
            "SELECT payload FROM captures WHERE id=?",
            [capture_id],
            |r| r.get(0),
        )
        .map_err(db)?;
    if let Some(result) = store.replay(request, &payload)? {
        return Ok(result);
    }
    if Directory::open(Path::new(string(&capture, "directory")?))
        .and_then(|d| d.open_file(string(&capture, "name").unwrap_or("")))
        .is_ok()
    {
        return Err(err(
            "file_committed",
            "Capture already has published bytes; settle it through exact execution",
        ));
    }
    let result = json!({"captureId":capture["captureId"],"requestId":request,"outcome":reason,"fileCommitted":false,"retainUntil":capture["retainUntil"],"effect":"none"});
    store.conn.execute_batch("BEGIN IMMEDIATE").map_err(db)?;
    store.receipt(request, &payload, &result)?;
    store
        .conn
        .execute(
            "UPDATE captures SET result=? WHERE id=?",
            params![result.to_string(), capture_id],
        )
        .map_err(db)?;
    store.conn.execute_batch("COMMIT").map_err(db)?;
    Ok(result)
}
pub(super) fn dispatch(store: &Store, inv: &Invocation) -> Result<Value, ToolError> {
    dispatch_with_destination(store, inv, |_| {})
}
fn dispatch_with_destination(
    store: &Store,
    inv: &Invocation,
    destination_ready: impl FnOnce(&Directory),
) -> Result<Value, ToolError> {
    let capture = if let Some(capture) = inv.arguments["captureId"].as_str() {
        load(store, capture)?
    } else {
        capture(store, inv)?
    };
    if capture.get("snapshot").is_none() {
        return Ok(capture);
    }
    let request = string(&capture, "requestId")?;
    let payload: String = store
        .conn
        .query_row(
            "SELECT payload FROM captures WHERE id=?",
            [string(&capture, "captureId")?],
            |r| r.get(0),
        )
        .map_err(db)?;
    if inv.arguments["requestId"]
        .as_str()
        .is_some_and(|r| r != request)
        || inv.operation != capture["operation"].as_str().unwrap_or("")
    {
        return Err(err(
            "request_conflict",
            "Capture belongs to another request or operation",
        ));
    }
    if inv.arguments["captureId"].is_string() {
        let mut supplied = normalized(inv);
        supplied["arguments"]
            .as_object_mut()
            .unwrap()
            .remove("requestId");
        let mut original =
            json!({"operation":capture["operation"],"arguments":capture["arguments"]});
        original["arguments"]
            .as_object_mut()
            .unwrap()
            .remove("requestId");
        if supplied["arguments"]
            .as_object()
            .is_some_and(|o| !o.is_empty())
            && supplied != original
        {
            return Err(err("request_conflict", "Captured request intent changed"));
        }
    }
    if let Some(result) = store.replay(request, &payload)? {
        return Ok(result);
    }
    reject_preview_region(&inv.operation, &capture["arguments"])?;
    let execution_dir = store.dir.child("render-locks").map_err(io)?;
    let lock_name = string(&capture, "captureId")?;
    let lock_file = match execution_dir.create_file(lock_name, 0o600) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            execution_dir.open_file(lock_name).map_err(io)?
        }
        Err(e) => return Err(io(e)),
    };
    let _execution = FileLease::acquire(lock_file, LeaseMode::Exclusive, Duration::from_secs(5))
        .map_err(|e| err("render_busy", e.to_string()))?;
    if let Some(result) = store.replay(request, &payload)? {
        return Ok(result);
    }
    let (directory, _destination_leases) =
        leased_directory(Path::new(string(&capture, "directory")?))?;
    destination_ready(&directory);
    let temporary = string(&capture, "temporaryName")?;
    let name = string(&capture, "name")?;
    let preview = inv.operation == "preview render";
    let state_file = inv.operation == "export state-file";
    let evidence_name = format!("{}.evidence.json", string(&capture, "captureId")?);
    let evidence_dir = store.dir.child("render-evidence").map_err(io)?;
    let ready = evidence_dir
        .open_file(&evidence_name)
        .ok()
        .map(|file| photo_files::read_json(file, 1024 * 1024).map_err(io))
        .transpose()?;
    let mut native = if let Some(ready) = ready {
        ready
    } else {
        let _input = verify_input(&capture)?;
        if directory.open_file(name).is_ok() {
            return Err(err(
                "destination_collision",
                "Destination exists without verified publication evidence",
            ));
        }
        if directory.open_file(temporary).is_ok() {
            directory.remove_file(temporary).map_err(io)?;
        }
        let engine = Engine::open()?.with_resources(&capture["dependencies"]);
        if engine.bundle_id() != capture["bundleId"].as_str().unwrap_or("")
            || engine.schema_version() != capture["schemaVersion"].as_str().unwrap_or("")
        {
            return Err(err(
                "schema_unsupported",
                "Engine differs from render capture",
            ));
        }
        let input = Path::new(string(&capture["input"], "path")?);
        let mut result = if state_file {
            let bytes = engine.state_file(input, &capture["snapshot"])?;
            if bytes.is_empty() {
                return Err(err("native_state_invalid", "Native XMP is empty"));
            }
            let mut file = directory.create_file(temporary, 0o600).map_err(io)?;
            file.write_all(&bytes).map_err(io)?;
            file.sync_all().map_err(io)?;
            json!({"mediaType":"application/rdf+xml","dependencies":capture["dependencies"]})
        } else {
            engine.render(
                input,
                &capture["snapshot"],
                &directory.path().join(temporary),
                preview,
                &json!({"region":capture["arguments"]["input"]["region"]}),
            )?
        };
        let mut file = directory.open_file(temporary).map_err(io)?;
        result["byteEvidence"] =
            serde_json::to_value(fingerprint(&mut file, u64::MAX).map_err(io)?).map_err(io)?;
        file.sync_all().map_err(io)?;
        result["byteLength"] = json!(file.metadata().map_err(io)?.len());
        photo_files::atomic_json(&evidence_dir, &evidence_name, &result).map_err(io)?;
        result
    };
    native["captureId"] = capture["captureId"].clone();
    native["requestId"] = json!(request);
    native["revision"] = capture["revision"].clone();
    native["input"] = capture["input"].clone();
    native["bundleId"] = capture["bundleId"].clone();
    native["schemaVersion"] = capture["schemaVersion"].clone();
    native["dependencies"] = capture["dependencies"].clone();
    native["path"] = capture["path"].clone();
    native["stateDirectory"] = json!(store.dir.path());
    native["executionHost"] = json!(host());
    native["retainUntil"] = capture["retainUntil"].clone();
    native["sha256"] = native["byteEvidence"]["sha256"].clone();
    native["byteLength"] = native["byteEvidence"]["byteLength"].clone();
    let mut file_committed = false;
    let settled = (|| -> Result<Value, ToolError> {
        if directory.open_file(temporary).is_ok()
            && let Err(failure) = directory.rename(temporary, &directory, name, false)
        {
            file_committed = directory
                .open_file(name)
                .ok()
                .and_then(|mut file| fingerprint(&mut file, u64::MAX).ok())
                .is_some_and(|evidence| {
                    native["byteEvidence"]["sha256"] == evidence.sha256
                        && native["byteEvidence"]["byteLength"].as_u64()
                            == Some(evidence.byte_length)
                });
            return Err(err("destination_collision", failure.to_string()));
        }
        file_committed = true;
        native["fileCommitted"] = json!(true);
        let mut file = directory.open_file(name).map_err(io)?;
        let observed =
            serde_json::to_value(fingerprint(&mut file, u64::MAX).map_err(io)?).map_err(io)?;
        if observed["sha256"] != native["byteEvidence"]["sha256"]
            || observed["byteLength"] != native["byteEvidence"]["byteLength"]
        {
            return Err(err(
                "file_changed",
                "Published bytes do not match retained render evidence",
            ));
        }
        native["sha256"] = observed["sha256"].clone();
        native["byteLength"] = observed["byteLength"].clone();
        native["byteEvidence"] = observed;
        if preview && !capture["comparison"].is_null() {
            let comparison = &capture["comparison"];
            let comparison_name =
                format!("{}-comparison.png", capture["captureId"].as_str().unwrap());
            let comparison_temp = format!(".{comparison_name}.pending");
            let record_name = format!(
                "{}-comparison.evidence.json",
                string(&capture, "captureId")?
            );
            let ready = evidence_dir
                .open_file(&record_name)
                .ok()
                .map(|file| photo_files::read_json(file, 1024 * 1024).map_err(io))
                .transpose()?;
            let mut facts = if let Some(ready) = ready {
                ready
            } else {
                if directory.open_file(&comparison_name).is_ok() {
                    return Err(err(
                        "destination_collision",
                        "Comparison destination lacks retained evidence",
                    ));
                }
                if directory.open_file(&comparison_temp).is_ok() {
                    directory.remove_file(&comparison_temp).map_err(io)?;
                }
                let engine = Engine::open()?.with_resources(&comparison["dependencies"]);
                let mut facts = engine.render(
                    Path::new(string(&comparison["input"], "path")?),
                    &comparison["snapshot"],
                    &directory.path().join(&comparison_temp),
                    true,
                    &json!({"region":capture["arguments"]["input"]["region"]}),
                )?;
                let mut f = directory.open_file(&comparison_temp).map_err(io)?;
                facts["byteEvidence"] =
                    serde_json::to_value(fingerprint(&mut f, u64::MAX).map_err(io)?).map_err(io)?;
                f.sync_all().map_err(io)?;
                photo_files::atomic_json(&evidence_dir, &record_name, &facts).map_err(io)?;
                facts
            };
            if directory.open_file(&comparison_temp).is_ok() {
                directory
                    .rename(&comparison_temp, &directory, &comparison_name, false)
                    .map_err(io)?;
            }
            let mut f = directory.open_file(&comparison_name).map_err(io)?;
            let observed =
                serde_json::to_value(fingerprint(&mut f, u64::MAX).map_err(io)?).map_err(io)?;
            if observed["sha256"] != facts["byteEvidence"]["sha256"]
                || observed["byteLength"] != facts["byteEvidence"]["byteLength"]
            {
                return Err(err(
                    "file_changed",
                    "Comparison bytes changed before settlement",
                ));
            }
            facts["byteEvidence"] = observed;
            facts["path"] = json!(directory.path().join(&comparison_name));
            facts["input"] = comparison["input"].clone();
            facts["dependencies"] = comparison["dependencies"].clone();
            facts["revision"] = comparison["revision"].clone();
            native["comparison"] = facts;
        }
        let _lock = store.lock()?;
        let current = store.revision(None)?;
        native["captureId"] = capture["captureId"].clone();
        native["requestId"] = json!(request);
        native["revision"] = capture["revision"].clone();
        native["input"] = capture["input"].clone();
        native["bundleId"] = capture["bundleId"].clone();
        native["schemaVersion"] = capture["schemaVersion"].clone();
        native["dependencies"] = capture["dependencies"].clone();
        native["path"] = capture["path"].clone();
        native["stateDirectory"] = json!(store.dir.path());
        native["executionHost"] = json!(host());
        native["kind"] = json!(if preview {
            "preview"
        } else if state_file {
            "state-file"
        } else {
            "export"
        });
        native["fileCommitted"] = json!(true);
        native["stale"] = json!(preview && capture["revision"] != current["revision"]);
        native["outcome"] = json!("saved");
        native["retainUntil"] = capture["retainUntil"].clone();
        store.conn.execute_batch("BEGIN IMMEDIATE").map_err(db)?;
        store.receipt(request, &payload, &native)?;
        store
            .conn
            .execute(
                "UPDATE captures SET result=? WHERE id=?",
                params![native.to_string(), capture["captureId"].as_str()],
            )
            .map_err(db)?;
        store
            .conn
            .execute_batch("COMMIT")
            .map_err(|e| err("outcome_unknown", e.to_string()))?;
        Ok(native.clone())
    })();
    match settled {
        Ok(result) => Ok(result),
        Err(failure) if file_committed => {
            let _ = store.conn.execute_batch("ROLLBACK");
            native["fileCommitted"] = json!(true);
            native["outcome"] = json!("unsettled");
            native["receiptState"] = json!("unconfirmed");
            native["effect"] = json!("partial");
            Err(err("outcome_unknown",format!("Output file committed; receipt settlement is unconfirmed: {}",failure.message)).effect("partial").detail(json!({"data":native,"cause":failure,"captureId":capture["captureId"],"requestId":request,"fileCommitted":true})))
        }
        Err(failure) => Err(failure),
    }
}
pub(super) fn retry(store: &Store, args: &Value) -> Result<Value, ToolError> {
    let _lock = store.lock()?;
    let source = load(store, string(args, "captureId")?)?;
    let request = string(args, "requestId")?;
    identifier(request)?;
    let payload = digest(&json!({"operation":"__retry-capture","arguments":args}))?;
    if let Some(result) = store.replay(request, &payload)? {
        return Ok(result);
    }
    let prior: Option<String> = store
        .conn
        .query_row("SELECT id FROM captures WHERE request=?", [request], |r| {
            r.get(0)
        })
        .optional()
        .map_err(db)?;
    if let Some(id) = prior {
        let existing = load(store, &id)?;
        if existing["retryOf"] != source["captureId"] {
            return Err(err(
                "request_conflict",
                "Retry request reused for another capture",
            ));
        }
        return Ok(existing);
    }
    let terminal: Option<String> = store
        .conn
        .query_row(
            "SELECT result FROM captures WHERE id=?",
            [string(&source, "captureId")?],
            |r| r.get(0),
        )
        .map_err(db)?;
    if terminal.is_none() {
        return Err(err("capture_pending", "Cannot retry a nonterminal capture"));
    }
    let terminal: Value = serde_json::from_str(&terminal.unwrap()).map_err(db)?;
    if terminal["fileCommitted"] == true {
        return Err(err("capture_succeeded", "Completed capture cannot retry"));
    }
    let _input = verify_input(&source)?;
    let mut next = source.clone();
    let capture_id = id("render")?;
    next["retryOf"] = source["captureId"].clone();
    next["captureId"] = json!(capture_id);
    next["requestId"] = json!(request);
    next["arguments"]["requestId"] = json!(request);
    next["temporaryName"] = json!(format!(".{capture_id}.pending"));
    let extension = Path::new(string(&source, "name")?)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("tiff");
    let name = format!("{capture_id}.{extension}");
    next["name"] = json!(name);
    next["path"] = json!(Path::new(string(&source, "directory")?).join(&name));
    next["retainUntil"] = json!(deadline());
    store.conn.execute("INSERT INTO captures (id,revision,snapshot,input,dependencies,result,payload,request) VALUES (?,?,?,?,?,NULL,?,?)",params![capture_id,next["revision"].as_str(),next.to_string(),next["input"].to_string(),next["dependencies"].to_string(),payload,request]).map_err(db)?;
    Ok(next)
}

#[cfg(test)]
mod lease_tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn external_render_excludes_backup_until_publication_and_settlement() {
        for fail in [false, true] {
            let root = std::env::temp_dir().join(id("render-lease-test").unwrap());
            let backup = Directory::create(root.join("backup")).unwrap();
            let destination = backup.path().join("new/previews");
            let state = root.join("state");
            let (entered_tx, entered_rx) = mpsc::channel();
            let (resume_tx, resume_rx) = mpsc::channel();
            let worker_destination = destination.clone();
            let worker_state = state.clone();
            let worker = std::thread::spawn(move || {
                let store = Store::create(Directory::create(&worker_state).unwrap()).unwrap();
                store.set_meta("identity", &json!("test-state")).unwrap();
                let revision = json!({"revision":"current","parent":null,"createdAt":timestamp(),"requestId":null,"snapshot":{},"input":{},"bundleId":"bundle","schemaVersion":"schema","dependencies":[],"changedTargets":[]});
                store.insert_revision(&revision).unwrap();
                store.set_meta("current", &json!("current")).unwrap();
                let capture = json!({"captureId":"render-a","requestId":"request-a","operation":"preview render","arguments":{},"revision":"older","snapshot":{},"input":{},"bundleId":"bundle","schemaVersion":"schema","dependencies":[],"directory":worker_destination,"name":"preview.png","temporaryName":".render-a.pending","path":worker_destination.join("preview.png"),"retainUntil":deadline()});
                store
                    .conn
                    .execute(
                        "INSERT INTO captures VALUES (?,?,?,?,?,NULL,?,?)",
                        params![
                            "render-a",
                            "older",
                            capture.to_string(),
                            "{}",
                            "[]",
                            "payload",
                            "request-a"
                        ],
                    )
                    .unwrap();
                store.dir.create_directory("render-locks").unwrap();
                let evidence = store.dir.create_directory("render-evidence").unwrap();
                let invocation = Invocation {
                    operation: "preview render".into(),
                    state_directory: Some(worker_state),
                    arguments: json!({"captureId":"render-a"}),
                };
                let result = dispatch_with_destination(&store, &invocation, |directory| {
                    if !fail {
                        let mut file = directory.create_file(".render-a.pending", 0o600).unwrap();
                        file.write_all(b"verified preview").unwrap();
                        file.sync_all().unwrap();
                        let facts = fingerprint(&mut file, u64::MAX).unwrap();
                        photo_files::atomic_json(
                            &evidence,
                            "render-a.evidence.json",
                            &json!({"byteEvidence":facts}),
                        )
                        .unwrap();
                    }
                    entered_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                });
                if fail {
                    assert!(result.is_err());
                    assert!(receipt(&store, "request-a").unwrap()["state"] == "pending");
                    let cancelled = release(
                        &store,
                        &json!({"captureId":"render-a","reason":"cancelled"}),
                    )
                    .unwrap();
                    assert_eq!(cancelled["outcome"], "cancelled");
                    assert_eq!(cancelled["fileCommitted"], false);
                    let replay = dispatch(&store, &invocation).unwrap();
                    assert_eq!(replay["outcome"], "replayed");
                    assert_eq!(replay["fileCommitted"], false);
                } else {
                    let result = result.unwrap();
                    assert_eq!(result["outcome"], "saved");
                    assert_eq!(result["stale"], true);
                    assert_eq!(result["fileCommitted"], true);
                    let replay = dispatch(&store, &invocation).unwrap();
                    assert_eq!(replay["outcome"], "replayed");
                    assert_eq!(replay["stale"], true);
                    assert_eq!(replay["sha256"], result["sha256"]);
                }
            });
            entered_rx.recv().unwrap();
            assert!(backup.lease(LeaseMode::Exclusive, Duration::ZERO).is_err());
            let directory = Directory::open(&destination).unwrap();
            assert!(
                directory
                    .lease(LeaseMode::Exclusive, Duration::ZERO)
                    .is_err()
            );
            assert!(directory.open_file("preview.png").is_err());
            let observer = Connection::open(state.join("state.sqlite3")).unwrap();
            let settled: i64 = observer
                .query_row("SELECT count(*) FROM receipts", [], |row| row.get(0))
                .unwrap();
            assert_eq!(settled, 0);
            drop(observer);
            resume_tx.send(()).unwrap();
            worker.join().unwrap();
            let _backup = backup.lease(LeaseMode::Exclusive, Duration::ZERO).unwrap();
            assert_eq!(directory.open_file("preview.png").is_ok(), !fail);
            drop(_backup);
            drop(directory);
            drop(backup);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[cfg(test)]
mod region_tests {
    use super::*;

    #[test]
    fn preview_region_refuses_before_native_capture_or_input_work() {
        let path = std::env::temp_dir().join(id("preview-region-test").unwrap());
        let store = Store::create(Directory::create(&path).unwrap()).unwrap();
        let invocation = Invocation {
            operation: "preview render".into(),
            state_directory: Some(path.clone()),
            arguments: json!({"requestId":"request-region","input":{"region":{"x":0,"y":0,"width":10,"height":10}}}),
        };
        assert_eq!(
            capture(&store, &invocation).unwrap_err().code,
            "schema_unsupported"
        );
        let count: i64 = store
            .conn
            .query_row("SELECT count(*) FROM captures", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(
            receipt(&store, "request-region").unwrap()["state"],
            "unknown"
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
}
