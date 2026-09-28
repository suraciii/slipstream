use super::*;

fn substitute_protocol_captures(
    value: &serde_json::Value,
    album_id: &str,
    publication: &str,
) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => serde_json::Value::String(
            text.replace("$albumId", album_id)
                .replace("$publication", publication),
        ),
        serde_json::Value::Array(values) => serde_json::Value::Array(
            values
                .iter()
                .map(|item| substitute_protocol_captures(item, album_id, publication))
                .collect(),
        ),
        serde_json::Value::Object(entries) => serde_json::Value::Object(
            entries
                .iter()
                .map(|(name, item)| {
                    (
                        name.clone(),
                        substitute_protocol_captures(item, album_id, publication),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Masks the wall-clock `updatedAt` progress timestamp so the exact contract
/// comparison pins its presence and shape without pinning a clock reading.
fn mask_updated_at(body: &mut serde_json::Value, name: &str) {
    fn mask_field(object: &mut serde_json::Value, name: &str) {
        let Some(updated_at) = object.get("updatedAt").and_then(|value| value.as_u64()) else {
            return;
        };
        assert!(
            updated_at > 0,
            "{name} updatedAt must be a real epoch timestamp"
        );
        object["updatedAt"] = serde_json::Value::String("$updatedAt".to_owned());
    }
    mask_field(body, name);
    if let Some(scan) = body.get_mut("scan") {
        mask_field(scan, name);
    }
}

#[tokio::test]
async fn shared_protocol_vectors_execute_all_requests_with_exact_results() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let vectors: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility/protocol/vectors.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let mut captured_album_id = String::new();
    let mut captured_publication = String::new();
    for vector in vectors {
        let request_definition = &vector["request"];
        let method = request_definition["method"].as_str().unwrap();
        let path = request_definition["path"].as_str().unwrap();
        let mut builder = authenticated_request()
            .method(method)
            .uri(format!("https://camera.local{path}"));
        if let Some(headers) = request_definition["headers"].as_object() {
            for (name, value) in headers {
                builder = builder.header(name, value.as_str().unwrap());
            }
        }
        let body = request_definition
            .get("body")
            .map(|body| Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap_or_else(Body::empty);
        let request = builder.body(body).unwrap();
        let response = tower::ServiceExt::oneshot(router.clone(), request)
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            vector["expected"]["status"].as_u64().unwrap() as u16,
            "{}",
            vector["name"]
        );
        if let Some(expected_headers) = vector["expected"]["headers"].as_object() {
            for (name, expected) in expected_headers {
                assert_eq!(
                    response
                        .headers()
                        .get(name)
                        .and_then(|value| value.to_str().ok()),
                    expected.as_str(),
                    "{} header {name}",
                    vector["name"]
                );
            }
        }
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        if let Some(expected) = vector["expected"]["body"].as_object() {
            let mut actual: serde_json::Value = serde_json::from_slice(&body).unwrap();
            mask_updated_at(&mut actual, vector["name"].as_str().unwrap());
            if captured_album_id.is_empty()
                && let Some(id) = actual["albums"][0]["id"].as_str()
            {
                captured_album_id = id.to_owned();
            }
            if captured_publication.is_empty()
                && let Some(publication) = actual
                    .get("publication")
                    .or_else(|| actual.get("scan")?.get("publication"))
                    .and_then(|value| value.as_str())
            {
                captured_publication = publication.to_owned();
            }
            let expected = substitute_protocol_captures(
                &serde_json::to_value(expected).unwrap(),
                &captured_album_id,
                &captured_publication,
            );
            assert_eq!(
                actual.as_object().unwrap(),
                expected.as_object().unwrap(),
                "{}",
                vector["name"]
            );
        }
        if let Some(expected) = vector["expected"]["bodyText"].as_str() {
            assert_eq!(
                std::str::from_utf8(&body).unwrap(),
                expected,
                "{}",
                vector["name"]
            );
        }
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

fn reverse_substitute(
    value: &serde_json::Value,
    captures: &HashMap<String, String>,
) -> serde_json::Value {
    // Replace the longest capture values first so URLs collapse before the
    // photo IDs they contain.
    let mut by_value: Vec<(&String, &String)> = captures.iter().collect();
    by_value.sort_by_key(|(_, value)| std::cmp::Reverse(value.len()));
    fn walk(value: &serde_json::Value, by_value: &[(&String, &String)]) -> serde_json::Value {
        match value {
            serde_json::Value::String(text) => {
                let mut result = text.clone();
                for (placeholder, value) in by_value {
                    result = result.replace(value.as_str(), &format!("${placeholder}"));
                }
                serde_json::Value::String(result)
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(|item| walk(item, by_value)).collect())
            }
            serde_json::Value::Object(entries) => serde_json::Value::Object(
                entries
                    .iter()
                    .map(|(name, item)| (name.clone(), walk(item, by_value)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    walk(value, &by_value)
}

#[tokio::test]
async fn browse_protocol_fixtures_execute_with_captured_token() {
    let vectors: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compatibility/protocol/browse-vectors.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(vectors.len(), 53);
    fn substitute(
        value: &serde_json::Value,
        captures: &HashMap<String, String>,
    ) -> serde_json::Value {
        match value {
            serde_json::Value::String(text) => {
                let mut result = text.clone();
                for (placeholder, replacement) in captures {
                    result = result.replace(&format!("${placeholder}"), replacement);
                }
                serde_json::Value::String(result)
            }
            serde_json::Value::Array(values) => serde_json::Value::Array(
                values
                    .iter()
                    .map(|item| substitute(item, captures))
                    .collect(),
            ),
            serde_json::Value::Object(entries) => serde_json::Value::Object(
                entries
                    .iter()
                    .map(|(name, item)| (name.clone(), substitute(item, captures)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    let (base, config) = prepare_populated_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(
        photo_ids.len(),
        3,
        "populated protocol fixture must have three Photos"
    );
    let album = application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Compat Album".to_owned(),
        })
        .await
        .unwrap();
    let album_id = album.albums[0].id.clone();
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album_id.clone(),
            photo_ids: vec![photo_ids[1].clone(), photo_ids[0].clone()],
        })
        .await
        .unwrap();
    // The two JPEG Photos preview from their own bytes; the RAW fixture is
    // arbitrary non-RAW bytes and stays terminally unavailable.
    for (index, photo_id) in photo_ids.iter().enumerate() {
        let preview = application.preview(photo_id).await.unwrap();
        if index == 2 {
            assert_ne!(preview.state, "ready", "RAW fixture cannot preview");
            continue;
        }
        assert_eq!(preview.state, "ready", "fixture Preview must be ready");
        let thumbnail = application.thumbnail(photo_id).await.unwrap();
        assert_eq!(thumbnail.state, "ready", "fixture Thumbnail must be ready");
    }
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let vectors: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compatibility/protocol/browse-vectors.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(vectors.len(), 53);
    let mut captures = HashMap::from([
        ("albumId".to_owned(), album_id),
        ("photoId".to_owned(), photo_ids[0].clone()),
        ("secondPhotoId".to_owned(), photo_ids[1].clone()),
        ("thirdPhotoId".to_owned(), photo_ids[2].clone()),
    ]);
    let regenerate = std::env::var("SLIPSTREAM_REGENERATE_PROTOCOL").is_ok();
    let mut regenerated: Vec<serde_json::Value> = Vec::new();
    let mut token = String::new();
    let mut publication = String::new();
    for vector in vectors {
        let name = vector["name"].as_str().unwrap().to_owned();
        let request_definition = &vector["request"];
        let method = request_definition["method"].as_str().unwrap();
        let path = substitute(
            &serde_json::Value::String(request_definition["path"].as_str().unwrap().to_owned()),
            &captures,
        )
        .as_str()
        .unwrap()
        .to_owned();
        let mut builder = authenticated_request()
            .method(method)
            .uri(format!("https://camera.local{path}"));
        if let Some(headers) = request_definition["headers"].as_object() {
            for (header_name, value) in headers {
                builder = builder.header(header_name, value.as_str().unwrap());
            }
        }
        let body = request_definition
            .get("body")
            .map(|body| Body::from(serde_json::to_vec(&substitute(body, &captures)).unwrap()))
            .unwrap_or_else(Body::empty);
        let request = builder.body(body).unwrap();
        let response = tower::ServiceExt::oneshot(router.clone(), request)
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            vector["expected"]["status"].as_u64().unwrap() as u16,
            "{name}"
        );
        let body = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let mut actual: Option<serde_json::Value> = if vector["expected"]["body"].is_object() {
            Some(serde_json::from_slice(&body).unwrap())
        } else {
            None
        };
        if let Some(actual_value) = actual.as_mut() {
            mask_updated_at(actual_value, &name);
        }
        if let Some(actual_value) = actual.as_ref() {
            if publication.is_empty()
                && let Some(captured) = actual_value
                    .get("publication")
                    .or_else(|| {
                        actual_value
                            .get("scan")
                            .and_then(|scan| scan.get("publication"))
                    })
                    .and_then(|value| value.as_str())
            {
                publication = captured.to_owned();
                captures.insert("publication".to_owned(), publication.clone());
            }
            if method == "POST"
                && path == "/api/browse"
                && let Some(new_token) = actual_value.get("token").and_then(|value| value.as_str())
            {
                token = new_token.to_owned();
                captures.insert("token".to_owned(), token.clone());
                assert!(token.len() >= 36, "{name} token is not opaque");
            }
            if let Some(photos) = actual_value
                .get("photos")
                .and_then(|value| value.as_array())
            {
                for (index, photo) in photos.iter().take(3).enumerate() {
                    let fields = match index {
                        0 => [
                            ("photoId", "id"),
                            ("reviewUrl", "preview.url"),
                            ("thumbnailUrl", "preview.thumbnailUrl"),
                        ],
                        1 => [
                            ("secondPhotoId", "id"),
                            ("secondReviewUrl", "preview.url"),
                            ("secondThumbnailUrl", "preview.thumbnailUrl"),
                        ],
                        _ => [
                            ("thirdPhotoId", "id"),
                            ("thirdReviewUrl", "preview.url"),
                            ("thirdThumbnailUrl", "preview.thumbnailUrl"),
                        ],
                    };
                    for (placeholder, field) in fields {
                        let value = field
                            .split('.')
                            .try_fold(photo, |value, key| value.get(key))
                            .and_then(|value| value.as_str());
                        if let Some(value) = value {
                            captures
                                .entry(placeholder.to_owned())
                                .or_insert_with(|| value.to_owned());
                        }
                    }
                }
            }
            if let Some(url) = actual_value.get("url").and_then(|value| value.as_str()) {
                let placeholder = if url.contains("/thumbnail/") {
                    "thumbnailUrl"
                } else if url.contains("/review/") {
                    "reviewUrl"
                } else {
                    ""
                };
                if !placeholder.is_empty() {
                    captures
                        .entry(placeholder.to_owned())
                        .or_insert_with(|| url.to_owned());
                }
            }
        }
        if regenerate {
            let mut updated = vector.clone();
            if let Some(actual_value) = actual.as_ref() {
                updated["expected"]["body"] = reverse_substitute(actual_value, &captures);
            }
            regenerated.push(updated);
            continue;
        }
        if let (Some(actual_value), Some(expected)) =
            (actual.as_ref(), vector["expected"]["body"].as_object())
        {
            let expected = substitute(&serde_json::Value::Object(expected.clone()), &captures);
            assert_eq!(
                actual_value.as_object().unwrap(),
                expected.as_object().unwrap(),
                "{name}"
            );
        }
    }
    if regenerate {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../compatibility/protocol/browse-vectors.json");
        fs::write(&path, serde_json::to_vec_pretty(&regenerated).unwrap()).unwrap();
        application.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(base);
        return;
    }
    assert!(!token.is_empty(), "fixtures must exercise a captured token");
    assert!(captures.contains_key("albumId"));
    assert!(captures.contains_key("photoId"));
    assert!(captures.contains_key("reviewUrl"));
    assert!(captures.contains_key("thumbnailUrl"));
    assert!(
        !publication.is_empty(),
        "fixtures must exercise a captured publication"
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn response_goldens_match_real_serialized_routes() {
    let goldens: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compatibility/protocol/responses.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(goldens.len(), 9);

    fn substitute(
        value: &serde_json::Value,
        captures: &HashMap<String, String>,
    ) -> serde_json::Value {
        match value {
            serde_json::Value::String(text) => {
                let mut result = text.clone();
                for (placeholder, replacement) in captures {
                    result = result.replace(&format!("${placeholder}"), replacement);
                }
                serde_json::Value::String(result)
            }
            serde_json::Value::Array(values) => serde_json::Value::Array(
                values
                    .iter()
                    .map(|item| substitute(item, captures))
                    .collect(),
            ),
            serde_json::Value::Object(entries) => serde_json::Value::Object(
                entries
                    .iter()
                    .map(|(name, item)| (name.clone(), substitute(item, captures)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    fn assert_golden(
        goldens: &[serde_json::Value],
        index: usize,
        actual: &serde_json::Value,
        captures: &HashMap<String, String>,
    ) {
        if std::env::var("SLIPSTREAM_REGENERATE_PROTOCOL").is_ok() {
            let updated = reverse_substitute(actual, captures);
            let slot = goldens
                .last()
                .and_then(|last| last.as_object())
                .and_then(|object| object.get("__regenerated"))
                .and_then(|value| value.as_array())
                .map(|values| values.len())
                .unwrap_or(0);
            let _ = slot;
            REGOLDED.with(|cell| {
                let mut map = cell.borrow_mut();
                map.insert(index, updated);
            });
            return;
        }
        assert_eq!(
            actual,
            &substitute(&goldens[index], captures),
            "response golden {index}"
        );
    }

    thread_local! {
        static REGOLDED: std::cell::RefCell<HashMap<usize, serde_json::Value>> =
            std::cell::RefCell::new(HashMap::new());
    }

    let (base, config) = prepare_populated_fixture();
    let root = &config.library_root;
    oversized_jpeg_fixture(&root.join("failed.JPG"));
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let summaries = browse_summaries(&application, BrowseSourceRequest::Library).await;
    let photo_id_for = |filename: &str| {
        summaries
            .iter()
            .find(|photo| photo.original_filename.as_deref() == Some(filename))
            .unwrap_or_else(|| panic!("fixture is missing {filename}"))
            .id
            .clone()
    };
    let photo_id = photo_id_for("pair.JPG");
    let later_id = photo_id_for("later.JPG");
    let failed_id = photo_id_for("failed.JPG");

    let created = response_json(
        post_json(
            &router,
            "/api/albums",
            serde_json::json!({"name":"Review"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let album_id = created["albums"][0]["id"].as_str().unwrap().to_owned();
    let added = response_json(
        post_json(
            &router,
            &format!("/api/albums/{album_id}/members"),
            serde_json::json!({"photoIds":[photo_id.clone()]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let mut captures = HashMap::from([
        ("albumId".to_owned(), album_id),
        ("photoId".to_owned(), photo_id.clone()),
    ]);
    assert_golden(&goldens, 2, &added, &captures);

    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"album","albumId":captures["albumId"]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap().to_owned();
    let pending = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=1"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 0, &pending, &captures);

    let pair_current = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(pair_current["state"], "ready");
    captures.insert(
        "reviewUrl".to_owned(),
        pair_current["url"].as_str().unwrap().to_owned(),
    );

    let current = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{later_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(current["state"], "ready");
    assert_eq!(current["width"], 120);
    assert_eq!(current["height"], 60);
    captures.insert("secondPhotoId".to_owned(), later_id.clone());
    captures.insert(
        "secondReviewUrl".to_owned(),
        current["url"].as_str().unwrap().to_owned(),
    );
    let thumbnail = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/thumbnail"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(thumbnail["state"], "ready");
    captures.insert(
        "thumbnailUrl".to_owned(),
        thumbnail["url"].as_str().unwrap().to_owned(),
    );
    let ready = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=1"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 1, &ready, &captures);
    assert_golden(&goldens, 3, &current, &captures);

    let set_two = response_json(
        post_json(
            &router,
            &format!("/api/photos/{photo_id}/state"),
            serde_json::json!({"field":"rating","value":2}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(set_two["kind"], "applied");
    let set_four = response_json(
        post_json(
            &router,
            &format!("/api/photos/{photo_id}/state"),
            serde_json::json!({"field":"rating","value":4}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 7, &set_four, &captures);
    let batch = response_json(
        post_json(
            &router,
            "/api/photos/state",
            serde_json::json!({
                "photos":[
                    {"photoId":photo_id.clone(),"expectedCurrent":"undecided"},
                    {"photoId":"00000000-0000-4000-8000-000000000000","expectedCurrent":"undecided"}
                ],
                "selectionState":"selected"
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 8, &batch, &captures);

    jpeg_fixture(&root.join("later.JPG"), 140, 70, [32, 192, 64]);
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("https://camera.local/api/scan")
                .header(header::ORIGIN, "https://camera.local")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let changed = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{later_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(changed["state"], "ready");
    assert_eq!(changed["width"], 140);
    assert_eq!(changed["height"], 70);
    captures.insert(
        "secondReviewUrl".to_owned(),
        changed["url"].as_str().unwrap().to_owned(),
    );
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local{}",
                    changed["url"].as_str().unwrap()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::OK
    );

    fs::write(root.join("later.JPG"), b"malformed replacement").unwrap();
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("https://camera.local/api/scan")
                .header(header::ORIGIN, "https://camera.local")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let stale = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{later_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 4, &stale, &captures);

    let failed = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{failed_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 5, &failed, &captures);

    fs::remove_file(root.join("later.JPG")).unwrap();
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("https://camera.local/api/scan")
                .header(header::ORIGIN, "https://camera.local")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let unavailable = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{later_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 6, &unavailable, &captures);

    if std::env::var("SLIPSTREAM_REGENERATE_PROTOCOL").is_ok() {
        REGOLDED.with(|cell| {
            let map = cell.borrow();
            let mut updated: Vec<serde_json::Value> = goldens.clone();
            for (index, value) in map.iter() {
                updated[*index] = value.clone();
            }
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compatibility/protocol/responses.json");
            fs::write(&path, serde_json::to_vec_pretty(&updated).unwrap()).unwrap();
        });
        application.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(base);
        return;
    }

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn cache_protocol_fixtures_execute_with_declared_headers() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(
        &config.library_root.join("photo.jpg"),
        90,
        45,
        [192, 64, 32],
    );
    let web_root = config.web_root();
    fs::create_dir_all(web_root.join("assets")).unwrap();
    fs::write(web_root.join("assets/app.js"), b"console.log(1)").unwrap();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let vectors: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compatibility/protocol/cache-vectors.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(vectors.len(), 2);
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();
    let preview = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(preview["state"], "ready");
    assert_eq!(preview["source"], "jpeg-original");
    let preview_url = preview["url"].as_str().unwrap().to_owned();
    for vector in vectors {
        let name = vector["name"].as_str().unwrap();
        let request_definition = &vector["request"];
        let method = request_definition["method"].as_str().unwrap();
        let target = match vector["setup"].as_str().unwrap() {
            "jpeg-original" => {
                assert_eq!(request_definition["target"], "generated-derivative");
                format!("https://camera.local{preview_url}")
            }
            "web-asset" => {
                assert_eq!(request_definition["path"], "/assets/app.js");
                "https://camera.local/assets/app.js".to_owned()
            }
            other => panic!("unknown cache fixture setup {other}"),
        };
        let expected = &vector["expected"];
        let declared_headers = expected["headers"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            declared_headers,
            BTreeSet::from(["cache-control", "content-type", "x-content-type-options"]),
            "{name} must declare the complete cache header contract"
        );
        let response = send(
            &router,
            authenticated_request()
                .method(method)
                .uri(target.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(
            response.status().as_u16(),
            expected["status"].as_u64().unwrap() as u16,
            "{name}"
        );
        for (header_name, value) in expected["headers"].as_object().unwrap() {
            assert_eq!(
                response.headers()[header_name.as_str()],
                value.as_str().unwrap(),
                "{name} {header_name}"
            );
        }
        if let Some(pattern) = expected["etagPattern"].as_str() {
            assert_etag_pattern(
                pattern,
                response.headers()[header::ETAG].to_str().unwrap(),
                name,
            );
        }
        if vector["setup"] == "jpeg-original" {
            let cache_key = preview_url
                .rsplit('/')
                .next()
                .and_then(|filename| filename.strip_suffix(".jpg"))
                .unwrap();
            assert_eq!(
                response.headers()[header::ETAG],
                format!("\"{cache_key}\""),
                "{name} ETag must identify the requested derivative cache key"
            );
        }
        let etag = response
            .headers()
            .get(header::ETAG)
            .map(|value| value.to_str().unwrap().to_owned());
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap();
        if let Some(minimum) = expected["minimumBodyBytes"].as_u64() {
            assert!(body.len() >= minimum as usize, "{name}");
        }
        if let Some(revalidation) = vector.get("revalidation") {
            assert_eq!(revalidation["header"], "if-none-match");
            let revalidated = send(
                &router,
                authenticated_request()
                    .method(method)
                    .uri(target)
                    .header(header::IF_NONE_MATCH, etag.unwrap())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(
                revalidated.status().as_u16(),
                revalidation["expectedStatus"].as_u64().unwrap() as u16,
                "{name} revalidation"
            );
            assert_eq!(
                axum::body::to_bytes(revalidated.into_body(), 1024)
                    .await
                    .unwrap()
                    .len(),
                0,
                "{name} revalidation body"
            );
        }
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

fn assert_etag_pattern(pattern: &str, etag: &str, name: &str) {
    assert_eq!(pattern, "^\"[a-f0-9]{64}\"$", "{name} etag pattern");
    let key = etag
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{name} etag must be quoted"));
    assert_eq!(key.len(), 64, "{name} etag key length");
    assert!(
        key.bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{name} etag key must be lowercase hex"
    );
}

#[tokio::test]
async fn photo_json_omits_optional_values_and_preserves_original_order() {
    let contract: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../compatibility/protocol/capture-order-omission.json"
    ))
    .unwrap();
    let ordered_paths = contract["orderedPaths"].as_array().unwrap();
    let allowed_keys = contract["allowedKeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let optional_keys = contract["optionalKeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("z.JPG"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 10:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let opened = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    let window = application
        .browse_window(&opened.token, 0, 60)
        .await
        .unwrap();
    application.browse_close(&opened.token);
    let photos = serde_json::to_value(window).unwrap();
    let list = photos["photos"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    let snapshot = application.library.snapshot().await.unwrap();
    assert_eq!(
        snapshot
            .photos
            .iter()
            .map(|photo| photo.sort_path.as_str())
            .collect::<Vec<_>>(),
        ordered_paths
            .iter()
            .map(|path| path.as_str().unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        list.iter()
            .map(|photo| photo["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        snapshot
            .photos
            .iter()
            .map(|photo| photo.id.as_str())
            .collect::<Vec<_>>()
    );
    for photo in list {
        assert_eq!(photo["original"]["kind"], "jpeg");
        assert_eq!(
            photo["preview"],
            serde_json::json!({"state": "inspection-pending"})
        );
        assert!(!photo.to_string().contains(":null"));
        let keys = photo
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        for key in &keys {
            assert!(allowed_keys.contains(key), "{key} leaked into protocol");
        }
        for key in allowed_keys.difference(&optional_keys) {
            assert!(keys.contains(key), "{key} missing from protocol");
        }
        assert!(
            !photo
                .to_string()
                .contains(config.library_root.to_str().unwrap())
        );
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A sparse JPEG-suffixed file beyond the preview input budget. The scanner
/// can retain its identity, while preview inspection returns the real request
/// failure state without allocating 128 MiB of fixture data.
fn oversized_jpeg_fixture(path: &Path) {
    let file = fs::File::create(path).unwrap();
    file.set_len(128 * 1024 * 1024 + 1).unwrap();
}

#[tokio::test]
async fn album_and_state_protocol_persists_across_reopen() {
    let (base, mut config) = prepare_fixture();
    jpeg_fixture(&config.library_root.join("a.jpg"), 8, 4, [192, 64, 32]);
    jpeg_fixture(&config.library_root.join("b.jpg"), 8, 4, [32, 192, 64]);
    jpeg_fixture(&config.library_root.join("c.jpg"), 8, 4, [32, 64, 192]);
    config.port = 0;
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(ids.len(), 3);

    let created = response_json(
        post_json(
            &router,
            "/api/albums",
            serde_json::json!({"name": " Picks "}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(created["albums"][0]["name"], "Picks");
    // Mutation responses expose bounded summaries only, never members.
    assert_eq!(created["albums"][0]["photoCount"], 0);
    assert_eq!(created["albums"][0]["hasSavedPosition"], false);
    assert!(created["albums"][0]["members"].is_null());
    assert!(created["albums"][0]["lastReviewedPhotoId"].is_null());
    let album_a = created["albums"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri(format!("https://camera.local/api/albums/{album_a}/members"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, "https://camera.local")
                .body(Body::from(serde_json::json!({"photoIds": ids}).to_string()))
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_a}/order"),
            serde_json::json!({"photoIds": [&ids[2], &ids[0], &ids[1]]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_a}/progress"),
            serde_json::json!({"photoId": ids[0]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let created_b = response_json(
        post_json(
            &router,
            "https://camera.local/api/albums",
            serde_json::json!({"name": "Other"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let album_b = created_b["albums"]
        .as_array()
        .unwrap()
        .iter()
        .find(|album| album["name"] == "Other")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let first_b_add = response_json(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members"),
            serde_json::json!({"photoIds": [&ids[0]]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(first_b_add["albumId"], album_b);
    assert_eq!(first_b_add["addedPhotoIds"], serde_json::json!([ids[0]]));
    assert_eq!(first_b_add["alreadyMemberPhotoIds"], serde_json::json!([]));

    let mixed_b_add = response_json(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members"),
            serde_json::json!({"photoIds": [&ids[0], &ids[1]]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(mixed_b_add["addedPhotoIds"], serde_json::json!([ids[1]]));
    assert_eq!(
        mixed_b_add["alreadyMemberPhotoIds"],
        serde_json::json!([ids[0]])
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/progress"),
            serde_json::json!({"photoId": ids[1]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let removed_b = response_json(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members/batch-remove"),
            serde_json::json!({"photoIds": [ids[1]]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(removed_b["removedPhotoIds"], serde_json::json!([ids[1]]));
    assert_eq!(removed_b["alreadyAbsentPhotoIds"], serde_json::json!([]));
    assert_eq!(
        removed_b["albums"]
            .as_array()
            .unwrap()
            .iter()
            .find(|album| album["id"] == album_b)
            .unwrap()["hasSavedPosition"],
        false
    );
    let repeated_removed_b = response_json(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members/batch-remove"),
            serde_json::json!({"photoIds": [ids[1]]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(repeated_removed_b["removedPhotoIds"], serde_json::json!([]));
    assert_eq!(
        repeated_removed_b["alreadyAbsentPhotoIds"],
        serde_json::json!([ids[1]])
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members"),
            serde_json::json!({"photoIds": [&ids[2], &ids[2]]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members/batch-remove"),
            serde_json::json!({"photoIds": [&ids[0], &ids[0]]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members/batch-remove"),
            serde_json::json!({"photoIds": []}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let over_limit = (0..=100)
        .map(|index| format!("00000000-0000-4000-8000-{index:012}"))
        .collect::<Vec<_>>();
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members/batch-remove"),
            serde_json::json!({"photoIds": over_limit}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members/batch-remove"),
            serde_json::json!({"photoIds": ["00000000-0000-4000-8000-00000000dead"]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let selected = response_json(
        post_json(
            &router,
            &format!("https://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "selectionState", "value": "selected", "albumId": album_a}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let undo = selected["undo"].clone();
    assert_eq!(selected["kind"], "applied");
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "rating", "value": 4}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    // Membership order is observable only through a fresh Album Browse
    // Snapshot; the bounded membership response carries identities, not
    // member lists.
    let ordered = browse_photo_ids(&application, BrowseSourceRequest::Album(album_a.clone())).await;
    assert_eq!(
        ordered,
        vec![ids[2].clone(), ids[0].clone(), ids[1].clone()]
    );
    let album_b_photos =
        browse_summaries(&application, BrowseSourceRequest::Album(album_b.clone())).await;
    let shared = album_b_photos
        .iter()
        .find(|photo| photo.id == ids[0])
        .unwrap();
    assert_eq!(shared.selection_state, "selected");
    assert_eq!(shared.rating, 4);
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({
                "field": undo["field"],
                "value": undo["priorValue"],
                "expectedCurrent": undo["expectedCurrent"]
            }),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "selectionState", "value": "rejected"}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let conflict = response_json(
            post_json(
                &router,
                &format!("https://camera.local/api/photos/{}/state", ids[0]),
                serde_json::json!({"field": "selectionState", "value": "selected", "expectedCurrent": "undecided"}),
                Some("https://camera.local"),
            )
            .await,
        )
        .await;
    assert_eq!(
        conflict,
        serde_json::json!({"error": "Mutation conflicts with current state"})
    );

    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_a}/members/remove"),
            serde_json::json!({"photoId": ids[0]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    // Removing the saved-position Photo clears the persisted progress;
    // the summary-only mutation response proves the cleared flag.
    let album_a_summary = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.id == album_a)
        .unwrap();
    assert!(!album_a_summary.has_saved_position);
    let before_original = fs::read(config.library_root.join("b.jpg")).unwrap();
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_a}/delete"),
            serde_json::json!({}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        fs::read(config.library_root.join("b.jpg")).unwrap(),
        before_original
    );
    application.shutdown().await.unwrap();

    let reopened = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&reopened).await;
    assert_eq!(
        browse_photo_ids(&reopened, BrowseSourceRequest::Library)
            .await
            .len(),
        3
    );
    let persisted = published_photo_summary(&reopened, &ids[0]).await;
    assert_eq!(persisted.selection_state, "rejected");
    assert_eq!(persisted.rating, 4);
    assert!(
        reopened
            .albums()
            .await
            .unwrap()
            .albums
            .iter()
            .all(|album| album.id != album_a)
    );
    reopened.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
