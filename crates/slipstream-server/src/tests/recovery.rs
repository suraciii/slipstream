//! Reviewed Location Recovery routes: bounded paging over a retained review,
//! reviewed-identity refusals, and the explicit confirmations one apply
//! requires.

use super::*;
use std::os::unix::fs::PermissionsExt;

/// One seeded unavailable Photo. Every seeded Original is absent from disk,
/// so the settled scan leaves the Photo unavailable exactly as an operator
/// whose file moved would see it.
struct UnavailableFixture {
    photo_id: &'static str,
    original_id: &'static str,
    location: &'static str,
}

/// Opens the state database of one recovery fixture so a test can simulate a
/// Library change between two review pages.
fn open_state(config: &Config) -> rusqlite::Connection {
    rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap()
}

fn recovery_fixture(photos: &[UnavailableFixture]) -> (PathBuf, Config) {
    let (base, config) = prepare_fixture();
    fs::create_dir_all(&config.state_directory).unwrap();
    fs::set_permissions(
        config.state_directory.clone(),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let database = open_state(&config);
    database
        .execute_batch(include_str!(
            "../../../../compatibility/sqlite/schema-v6.sql"
        ))
        .unwrap();
    database
        .execute(
            "INSERT INTO library_metadata VALUES('canonical_root',?)",
            [config.library_root.to_str().unwrap()],
        )
        .unwrap();
    for photo in photos {
        database
            .execute(
                "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,\
                 capture_metadata_state,capture_source_revision) \
                 VALUES(?,?,'jpeg',11,1.0,0,'missing','remembered-revision')",
                rusqlite::params![photo.original_id, photo.location],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO photos(id,original_id,available,preview_state,sort_path,\
                 selection_state,rating) VALUES(?,?,0,'unavailable',?,'undecided',0)",
                rusqlite::params![photo.photo_id, photo.original_id, photo.location],
            )
            .unwrap();
    }
    drop(database);
    (base, config)
}

/// Writes the destination bytes an operator moved into place after the scan,
/// so the proposal for that Location really evaluates the file.
fn write_destination(config: &Config, location: &str, bytes: &[u8]) {
    let path = config.library_root.join(location);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

async fn open_review(router: &Router, body: serde_json::Value) -> serde_json::Value {
    let response = post_json(
        router,
        "/api/recovery/unavailable",
        body,
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn continuation(router: &Router, cursor: &str) -> (StatusCode, serde_json::Value) {
    let response = send(
        router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/recovery/unavailable/{cursor}"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let status = response.status();
    (status, response_json(response).await)
}

#[tokio::test]
async fn recovery_http_requires_the_reviewed_identity_and_confirmations() {
    let (base, config) = recovery_fixture(&[UnavailableFixture {
        photo_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        original_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        location: "shoot/a.JPG",
    }]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    write_destination(&config, "moved/a.JPG", b"jpeg-bytes-a");

    let review = open_review(&router, serde_json::json!({})).await;
    assert_eq!(review["total"], 1);
    assert_eq!(review["items"][0]["state"], "unavailable");
    assert_eq!(review["items"][0]["nextCursor"], serde_json::Value::Null);

    let proposal = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({
                "originalId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                "newLocation": "moved/a.JPG",
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(proposal["total"], 1);
    assert_eq!(proposal["nextCursor"], serde_json::Value::Null);
    let mapping = &proposal["items"][0];
    assert_eq!(mapping["outcome"], "matched");
    assert_eq!(mapping["verified"], false);
    assert_eq!(mapping["blockedReason"], serde_json::Value::Null);
    let reviewed_id = mapping["mappingId"].as_str().unwrap();

    // An unreviewed identity is refused before any association changes.
    let refused = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"mappings":[{
            "originalId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "newLocation": "moved/a.JPG",
            "mappingId": "0000000000000000000000000000000000000000000000000000000000000000",
            "confirmUnverifiedContent": true,
        }]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let refused = response_json(refused).await;
    assert_eq!(refused["appliedMappings"], 0);
    assert_eq!(refused["refusedMappings"], 1);
    assert_eq!(refused["rejections"][0]["reason"], "reviewed-stale");
    assert_eq!(
        open_review(&router, serde_json::json!({})).await["items"][0]["state"],
        "unavailable"
    );

    // Without a persisted fingerprint the content cannot be verified, so the
    // reviewed identity alone is not a confirmation.
    let refused = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"mappings":[{
            "originalId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "newLocation": "moved/a.JPG",
            "mappingId": reviewed_id,
        }]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let refused = response_json(refused).await;
    assert_eq!(refused["rejections"][0]["reason"], "content-unconfirmed");

    let applied = response_json(
        post_json(
            &router,
            "/api/recovery/apply",
            serde_json::json!({"mappings":[{
                "originalId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                "newLocation": "moved/a.JPG",
                "mappingId": reviewed_id,
                "confirmUnverifiedContent": true,
            }]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(applied["appliedMappings"], 1);
    assert_eq!(applied["refusedMappings"], 0);
    assert_eq!(applied["unavailablePhotos"], 0);
    assert_eq!(applied["mappings"][0]["fromLocation"], "shoot/a.JPG");
    assert_eq!(applied["mappings"][0]["toLocation"], "moved/a.JPG");
    assert_eq!(applied["mappings"][0]["retired"], serde_json::Value::Null);

    // The recovered identity leaves the unavailable set, so a fresh review
    // reports no remaining work rather than an empty placeholder.
    let review = open_review(&router, serde_json::json!({})).await;
    assert_eq!(review["total"], 0);
    assert_eq!(review["items"].as_array().unwrap().len(), 0);
    assert_eq!(review["nextCursor"], serde_json::Value::Null);
    let recovered = cli_photo_read(&router, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").await;
    assert_eq!(recovered["originalAvailable"], true);
    assert_eq!(recovered["location"], "moved/a.JPG");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_pages_a_retained_review_without_shifting_positions() {
    let (base, config) = recovery_fixture(&[
        UnavailableFixture {
            photo_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
            original_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            location: "shoot/a.JPG",
        },
        UnavailableFixture {
            photo_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
            original_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
            location: "shoot/b.JPG",
        },
        UnavailableFixture {
            photo_id: "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee",
            original_id: "ffffffff-ffff-4fff-8fff-ffffffffffff",
            location: "shoot/c.JPG",
        },
    ]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let first = open_review(&router, serde_json::json!({"limit": 1})).await;
    assert_eq!(first["total"], 3);
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["location"], "shoot/a.JPG");
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    assert!(first["expiresAt"].is_string());

    // The Library changes between two pages of one review: one reviewed Photo
    // becomes available elsewhere and another record disappears. The retained
    // membership keeps its positions and reports the current state of each
    // identity instead of shifting or inventing items.
    let database = open_state(&config);
    database
        .execute(
            "UPDATE photos SET available=1 WHERE id='cccccccc-cccc-4ccc-8ccc-cccccccccccc'",
            [],
        )
        .unwrap();
    database
        .execute(
            "DELETE FROM photos WHERE id='eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee'",
            [],
        )
        .unwrap();
    database
        .execute(
            "DELETE FROM original_files WHERE id='ffffffff-ffff-4fff-8fff-ffffffffffff'",
            [],
        )
        .unwrap();
    drop(database);

    // One page keeps the bound the opener was asked for.
    let (status, second) = continuation(&router, &cursor).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["total"], 3);
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"][0]["location"], "shoot/b.JPG");
    assert_eq!(second["items"][0]["state"], "available");
    let cursor = second["nextCursor"].as_str().unwrap().to_owned();
    assert!(second["expiresAt"].is_string());

    let (status, third) = continuation(&router, &cursor).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(third["total"], 3);
    assert_eq!(third["items"].as_array().unwrap().len(), 1);
    assert_eq!(third["items"][0]["location"], "shoot/c.JPG");
    assert_eq!(third["items"][0]["state"], "missing");
    assert_eq!(
        third["items"][0]["photoId"],
        "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee"
    );
    assert_eq!(third["nextCursor"], serde_json::Value::Null);
    assert_eq!(third["expiresAt"], serde_json::Value::Null);

    // A fresh review evaluates the current unavailable set only: the Photo
    // that became available and the vanished record are both gone from it.
    let reopened = open_review(&router, serde_json::json!({})).await;
    assert_eq!(reopened["total"], 1);
    assert_eq!(reopened["items"][0]["location"], "shoot/a.JPG");
    assert_eq!(reopened["items"][0]["state"], "unavailable");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_expires_an_evicted_review_continuation() {
    let (base, config) = recovery_fixture(&[
        UnavailableFixture {
            photo_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
            original_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            location: "shoot/a.JPG",
        },
        UnavailableFixture {
            photo_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
            original_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
            location: "shoot/b.JPG",
        },
    ]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let first = open_review(&router, serde_json::json!({"limit": 1})).await;
    let evicted = first["nextCursor"].as_str().unwrap().to_owned();
    // The review store keeps a bounded number of reviews, so opening reviews
    // past that bound evicts the oldest one.
    for _ in 0..8 {
        let review = open_review(&router, serde_json::json!({"limit": 1})).await;
        assert!(review["nextCursor"].is_string());
    }

    let response = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/recovery/unavailable/{evicted}"
            ))
            .header("Slipstream-CLI-Contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::GONE);
    let refused = response_json(response).await;
    assert_eq!(refused["error"]["code"], "cursor_expired");
    assert_eq!(refused["error"]["effect"], "none");
    assert_eq!(refused["error"]["details"]["cursorKind"], "unavailable");
    assert_eq!(refused["error"]["details"]["reason"], "idle_or_evicted");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_blocks_an_occupied_destination_with_user_state() {
    let (base, config) = recovery_fixture(&[UnavailableFixture {
        photo_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        original_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        location: "shoot/a.JPG",
    }]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    write_destination(&config, "moved/a.JPG", b"jpeg-bytes-a");
    // A second Photo keeps a deliberate Rating at that Location, so an
    // explicit retire must never replace it.
    open_state(&config)
        .execute(
            "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,\
             capture_metadata_state,capture_source_revision) \
             VALUES('11111111-1111-4111-8111-111111111111','moved/a.JPG','jpeg',11,1.0,1,\
             'missing','remembered-revision')",
            [],
        )
        .unwrap();
    open_state(&config)
        .execute(
            "INSERT INTO photos(id,original_id,available,preview_state,sort_path,\
             selection_state,rating) \
             VALUES('22222222-2222-4222-8222-222222222222',\
             '11111111-1111-4111-8111-111111111111',1,'unavailable','moved/a.JPG',\
             'unflagged',3)",
            [],
        )
        .unwrap();

    let proposal = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({
                "originalId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                "newLocation": "moved/a.JPG",
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let mapping = &proposal["items"][0];
    assert_eq!(mapping["outcome"], "occupied");
    assert_eq!(mapping["retire"], serde_json::Value::Null);
    assert_eq!(mapping["blockedReason"], "destination-in-use");

    let refused = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"mappings":[{
            "originalId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "newLocation": "moved/a.JPG",
            "mappingId": mapping["mappingId"],
            "confirmUnverifiedContent": true,
        }]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let refused = response_json(refused).await;
    assert_eq!(refused["rejections"][0]["reason"], "destination-in-use");

    // The occupying Photo keeps its Rating, both Photos keep their state, and
    // a fresh evaluation still reports the same blocked mapping.
    let database = open_state(&config);
    assert_eq!(
        database
            .query_row(
                "SELECT rating FROM photos WHERE id='22222222-2222-4222-8222-222222222222'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        3
    );
    assert_eq!(
        database
            .query_row(
                "SELECT available FROM photos WHERE id='bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    drop(database);
    let reproposal = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({
                "originalId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                "newLocation": "moved/a.JPG",
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(
        reproposal["items"][0]["blockedReason"],
        "destination-in-use"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A persisted fingerprint does not turn an absent destination unreadable:
/// with the operator's file nowhere at the candidate Location, the proposal
/// reports the mapping as missing instead of unreadable.
#[tokio::test]
async fn recovery_http_reports_a_fingerprinted_absent_destination_as_missing() {
    let (base, config) = recovery_fixture(&[UnavailableFixture {
        photo_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        original_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        location: "shoot/a.JPG",
    }]);
    // The remembered fingerprint of the moved bytes; no file exists at the
    // candidate Location, so nothing can be digested or verified.
    open_state(&config)
        .execute(
            "INSERT INTO original_fingerprints(original_id,digest,size,mtime_ms) \
             VALUES('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',?,11,1.0)",
            [slipstream_core::digest_bytes(b"jpeg-bytes-a")],
        )
        .unwrap();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let review = open_review(&router, serde_json::json!({})).await;
    assert_eq!(review["items"][0]["fingerprintEnrolled"], true);

    let proposal = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({
                "originalId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                "newLocation": "moved/a.JPG",
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let mapping = &proposal["items"][0];
    assert_eq!(mapping["outcome"], "missing");
    assert_eq!(mapping["blockedReason"], "missing");
    assert_eq!(mapping["verified"], false);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
