use super::*;
fn store() -> (PathBuf, Store) {
    let path = std::env::temp_dir().join(id("state-test").unwrap());
    let store = Store::create(Directory::create(&path).unwrap()).unwrap();
    store.set_meta("identity", &json!("test-state")).unwrap();
    let revision = json!({"revision":"revision-initial","parent":null,"createdAt":timestamp(),"requestId":null,"snapshot":{"instances":[],"masks":[],"actions":[]},"input":{},"bundleId":"bundle","schemaVersion":"schema","dependencies":[],"changedTargets":[]});
    store.insert_revision(&revision).unwrap();
    store.set_meta("current", &revision["revision"]).unwrap();
    store.set_meta("baseline", &revision["revision"]).unwrap();
    (path, store)
}
#[test]
fn pending_render_request_identity_cannot_be_used_for_mutation() {
    let (path, store) = store();
    store
        .conn
        .execute(
            "INSERT INTO captures VALUES (?,?,?,?,?,NULL,?,?)",
            params![
                "capture-a",
                "revision-initial",
                "{}",
                "{}",
                "[]",
                "render-intent",
                "request-render"
            ],
        )
        .unwrap();
    assert_eq!(
        store
            .reject_capture_request("request-render")
            .unwrap_err()
            .code,
        "request_conflict"
    );
    store.reject_capture_request("request-fresh").unwrap();
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn new_nested_state_parent_is_created_under_shared_ancestor_leases() {
    let root = std::env::temp_dir().join(id("parents-test").unwrap());
    let state = root.join("edits/darktable/main");
    let (parent, leases) = create_state_parent(&state).unwrap();
    assert_eq!(parent.path(), state.parent().unwrap());
    assert!(parent.entries(10).unwrap().is_empty());
    assert!(!state.exists());
    assert!(parent.lease(LeaseMode::Exclusive, Duration::ZERO).is_err());
    drop(leases);
    drop(parent);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn standalone_input_without_state_holds_discoverable_ancestor_barriers() {
    let path = std::env::temp_dir().join(id("input-admission-test").unwrap());
    let workspace = Directory::create(&path).unwrap();
    let inputs = workspace.create_directory("inputs").unwrap();
    let mut file = inputs.create_file("input.raw", 0o600).unwrap();
    std::io::Write::write_all(&mut file, b"standalone input").unwrap();
    drop(file);
    let (_, admitted) = evidence(&inputs.path().join("input.raw")).unwrap();
    assert!(
        workspace
            .lease(LeaseMode::Exclusive, Duration::ZERO)
            .is_err()
    );
    assert!(inputs.lease(LeaseMode::Exclusive, Duration::ZERO).is_err());
    assert!(
        FileLease::acquire(
            inputs.open_file("input.raw").unwrap(),
            LeaseMode::Exclusive,
            Duration::ZERO
        )
        .is_err()
    );
    drop(admitted);
    let deletion = workspace
        .lease(LeaseMode::Exclusive, Duration::ZERO)
        .unwrap();
    let input = FileLease::acquire(
        inputs.open_file("input.raw").unwrap(),
        LeaseMode::Exclusive,
        Duration::ZERO,
    )
    .unwrap();
    drop(input);
    drop(deletion);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn retained_inspection_does_not_reacquire_exclusive_workspace_ancestor() {
    let (path, original) = store();
    let revision = original.revision(None).unwrap();
    drop(original);
    let root = Directory::open(&path).unwrap();
    let store = Store::create(root.create_directory("edit").unwrap()).unwrap();
    store
        .set_meta("identity", &json!("inspection-state"))
        .unwrap();
    store.insert_revision(&revision).unwrap();
    store.set_meta("current", &revision["revision"]).unwrap();
    store.set_meta("baseline", &revision["revision"]).unwrap();
    let admission = root.lease(LeaseMode::Exclusive, Duration::ZERO).unwrap();
    let result = crate::inspect_retained_directory(&store.dir, &store._lifecycle).unwrap();
    assert_eq!(result["revision"], "revision-initial");
    assert_eq!(result["retainedRevisions"].as_array().unwrap().len(), 1);
    assert!(crate::inspect_retained_directory(&store.dir, &admission).is_err());
    drop(admission);
    drop(root);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn replay_preserves_original_effects_after_current_revision_changes() {
    let (path, store) = store();
    let saved = json!({"outcome":"saved","revision":"revision-initial","generatedIds":{"new":"instance-original"},"effects":{"value":1}});
    store.receipt("request-a", "intent-a", &saved).unwrap();
    let mut current = store.revision(None).unwrap();
    current["revision"] = json!("revision-new");
    current["parent"] = json!("revision-initial");
    store.insert_revision(&current).unwrap();
    store.set_meta("current", &current["revision"]).unwrap();
    let replay = store.replay("request-a", "intent-a").unwrap().unwrap();
    assert_eq!(replay["revision"], saved["revision"]);
    assert_eq!(replay["current"]["revision"], "revision-new");
    assert_eq!(replay["generatedIds"], saved["generatedIds"]);
    assert_eq!(replay["effects"], saved["effects"]);
    assert_eq!(replay["outcome"], "replayed");
    assert_eq!(
        store
            .replay("request-a", "different-intent")
            .unwrap_err()
            .code,
        "request_conflict"
    );
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn baseline_id_remap_updates_nested_references() {
    let mut snapshot = json!({"instances":[{"instanceId":"baseline-1","blend":{"raster":{"instanceId":"baseline-1"}},"maskIds":["native-mask"]}],"masks":[{"maskId":"native-mask","referencedBy":["baseline-1"]}],"actions":[{"instanceId":"baseline-1","affectedTargets":["native-mask"]}]});
    remap_baseline(&mut snapshot).unwrap();
    let instance = &snapshot["instances"][0]["instanceId"];
    let mask = &snapshot["masks"][0]["maskId"];
    assert_ne!(instance, "baseline-1");
    assert_eq!(&snapshot["masks"][0]["referencedBy"][0], instance);
    assert_eq!(&snapshot["actions"][0]["instanceId"], instance);
    assert_eq!(&snapshot["instances"][0]["maskIds"][0], mask);
    assert_eq!(&snapshot["actions"][0]["affectedTargets"][0], mask);
}
#[test]
fn byte_identical_replacement_requires_explicit_observation_rebase() {
    use std::io::Write;
    let path = std::env::temp_dir().join(id("input-test").unwrap());
    let dir = Directory::create(&path).unwrap();
    dir.create_file("raw", 0o600)
        .unwrap()
        .write_all(b"verified original bytes")
        .unwrap();
    let (input, lease) = evidence(&path.join("raw")).unwrap();
    drop(lease);
    dir.rename("raw", &dir, "backup", false).unwrap();
    dir.create_file("raw", 0o600)
        .unwrap()
        .write_all(b"verified original bytes")
        .unwrap();
    match verify_input(&json!({"input":input})) {
        Err(error) => assert_eq!(error.code, "input_changed"),
        Ok(_) => panic!("replaced descriptor unexpectedly accepted"),
    };
    let (replaced, lease) = evidence(&path.join("raw")).unwrap();
    assert_eq!(replaced["boundRevision"], input["boundRevision"]);
    assert_ne!(replaced["observedRevision"], input["observedRevision"]);
    drop(lease);
    drop(dir);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn edit_view_preserves_local_catalog_identity_without_duplicate_state_metadata() {
    let (path, store) = store();
    let catalog = catalog::initialize(&path).unwrap();
    std::fs::write(path.join("input.raw"), b"retained input").unwrap();
    let (input, lease) = evidence(&path.join("input.raw")).unwrap();
    let mut revision = store.revision(None).unwrap();
    revision["input"] = input;
    store.set_meta("discovery", &json!({})).unwrap();
    let first = reads::view(&store, &revision).unwrap();
    assert_eq!(first["catalog"], catalog);
    drop(store);
    let reopened = Store::read(&path).unwrap();
    let second = reads::view(&reopened, &revision).unwrap();
    assert_eq!(second["catalog"], first["catalog"]);
    drop(reopened);
    drop(lease);
    std::fs::remove_dir_all(path).unwrap();
}
