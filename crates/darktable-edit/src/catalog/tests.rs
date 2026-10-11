use super::*;
fn directory() -> PathBuf {
    let path = std::env::temp_dir().join(photo_files::identity("catalog-test").unwrap());
    Directory::create(&path).unwrap();
    path
}
fn definition() -> Value {
    json!({"id":"preset","version":"v1","kind":"preset","name":"settings","bundleId":"native","schemaVersion":"1","createdAt":"2026-10-11T00:00:00Z","availability":available(),"initialization":"literal","maskPolicy":"omit","resources":[],"masks":null,"settings":{"moduleId":"exposure","parameterSchemaVersion":"native-exposure","parameters":[{"parameterId":"exposure","value":1.0}],"blend":null}})
}
fn discovery() -> Value {
    json!({"modules":[{"moduleId":"exposure","parameterSchemaVersion":"native-exposure","parameters":{"type":"object","properties":{"exposure":{"type":"number","minimum":-10,"maximum":10}}},"blendSchema":{"type":"object"}}]})
}
#[test]
fn representation_schema_is_distinct_from_operation_schema() {
    let record = definition();
    definitions::validate(&record, "preset", &discovery(), "native").unwrap();
    let mut wrong = record;
    wrong["schemaVersion"] = json!("native-exposure");
    assert!(definitions::validate(&wrong, "preset", &discovery(), "native").is_err());
}
#[test]
fn unknown_interchange_fields_and_out_of_range_values_refuse() {
    let mut record = definition();
    record["nativeBlob"] = json!("untyped");
    assert!(definitions::validate(&record, "preset", &discovery(), "native").is_err());
    record.as_object_mut().unwrap().remove("nativeBlob");
    record["settings"]["parameters"][0]["value"] = json!(20);
    assert!(definitions::validate(&record, "preset", &discovery(), "native").is_err());
}
#[test]
fn capture_uses_engine_bundle_identity_without_altering_discovery() {
    let discovery = discovery();
    let args = json!({"instanceId":"source","input":{"name":"preset","parameterIds":["exposure"],"includeBlend":false,"initialization":"literal","maskPolicy":"omit"}});
    let edit = json!({"snapshot":{"instances":[{"instanceId":"source","moduleId":"exposure","order":1,"parameters":{"exposure":1.0}}]}});
    let (record, _) = definitions::capture("preset", &args, &edit, &discovery, "native").unwrap();
    assert_eq!(record["bundleId"], "native");
    assert!(discovery.get("bundleId").is_none());
    definitions::validate(&record, "preset", &discovery, "native").unwrap();
    assert_eq!(
        definitions::validate(&record, "preset", &discovery, "other-bundle")
            .unwrap_err()
            .code,
        "incompatible_definition"
    );
}
#[test]
fn missing_local_record_never_searches_other_catalog() {
    let first = directory();
    let second = directory();
    let a = Catalog::open(Some(&first), "local", true).unwrap();
    let b = Catalog::open(Some(&second), "local", true).unwrap();
    a.db.execute(
        "INSERT INTO records VALUES(?,?,?,?,NULL)",
        params!["preset", "v1", "preset", definition().to_string()],
    )
    .unwrap();
    assert!(a.record("preset", "v1", "preset").is_ok());
    let failure = b.record("preset", "v1", "preset").unwrap_err();
    assert_eq!(failure.code, "catalog_record_not_found");
    drop(a);
    drop(b);
    std::fs::remove_dir_all(first).unwrap();
    std::fs::remove_dir_all(second).unwrap();
}
#[test]
fn failed_transaction_creates_neither_definition_nor_receipt() {
    let path = directory();
    let mut c = Catalog::open(Some(&path), "local", true).unwrap();
    {
        let tx = c.db.transaction().unwrap();
        tx.execute(
            "INSERT INTO records VALUES(?,?,?,?,NULL)",
            params!["preset", "v1", "preset", definition().to_string()],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO receipts VALUES(?,?,?,?)",
            params!["request", "intent", "{}", now()],
        )
        .unwrap();
    }
    assert!(c.record("preset", "v1", "preset").is_err());
    let count: i64 =
        c.db.query_row("SELECT COUNT(*) FROM receipts", [], |r| r.get(0))
            .unwrap();
    assert_eq!(count, 0);
    drop(c);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn cursor_pins_records_across_catalog_reopen() {
    let path = directory();
    let c = Catalog::open(Some(&path), "local", true).unwrap();
    for id in ["a", "b"] {
        let mut record = definition();
        record["id"] = json!(id);
        c.db.execute(
            "INSERT INTO records VALUES(?,?,?,?,NULL)",
            params![id, "v1", "preset", record.to_string()],
        )
        .unwrap();
    }
    let first = list(&c, &json!({"limit":1}), "preset").unwrap();
    let token = first["nextCursor"].as_str().unwrap();
    c.db.execute("DELETE FROM records WHERE id='b'", [])
        .unwrap();
    drop(c);
    let second = dispatch(
        &Invocation {
            operation: "presets list".into(),
            state_directory: None,
            arguments: json!({"cursor":token}),
        },
        None,
    )
    .unwrap();
    assert_eq!(second["items"][0]["id"], "b");
    assert_eq!(second["catalog"], first["catalog"]);
    assert_eq!(
        dispatch(
            &Invocation {
                operation: "presets list".into(),
                state_directory: None,
                arguments: json!({"cursor":"unknown-cursor"})
            },
            None
        )
        .unwrap_err()
        .code,
        "cursor_expired"
    );
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn export_publication_cannot_replace_existing_bytes() {
    let path = directory();
    let dir = Directory::open(&path).unwrap();
    let mut existing = dir.create_file("record.json", 0o600).unwrap();
    existing.write_all(b"original").unwrap();
    let mut pending = PendingFile::create(&dir).unwrap();
    pending.file_mut().write_all(b"new definition").unwrap();
    assert!(
        pending
            .publish("record.json", MAX_DEFINITION as u64)
            .is_err()
    );
    assert_eq!(
        read_file(&path.join("record.json"), MAX_DEFINITION).unwrap(),
        b"original"
    );
    drop(dir);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn globally_captured_bytes_remain_local_after_source_is_removed() {
    let state = directory();
    initialize(&state).unwrap();
    let source = directory();
    let file = Directory::open(&source).unwrap();
    let bytes = b"LUT_1D_SIZE 2\n0 0 0\n1 1 1\n";
    let mut raw = file.create_file("lut.cube", 0o600).unwrap();
    raw.write_all(bytes).unwrap();
    drop(raw);
    let sha = digest(bytes);
    let input = json!([{"resourceId":format!("resource-{sha}"),"version":sha,"kind":"lut","mediaType":"application/x-cube","path":source.join("lut.cube"),"sha256":sha,"byteLength":bytes.len()}]);
    let pinned = pin_external_dependencies(&state, &input).unwrap();
    std::fs::remove_file(source.join("lut.cube")).unwrap();
    let snapshot =
        json!({"parameters":{"lut":{"resourceId":format!("resource-{sha}"),"version":sha}}});
    let retained = validate_dependencies(&state, &snapshot, "local").unwrap();
    assert_eq!(retained[0]["path"], pinned[0]["path"]);
    assert_eq!(
        read_file(Path::new(pinned[0]["path"].as_str().unwrap()), MAX_RESOURCE).unwrap(),
        bytes
    );
    drop(file);
    std::fs::remove_dir_all(state).unwrap();
    std::fs::remove_dir_all(source).unwrap();
}
