use super::*;
use serde::Serialize;

const CLEANUP_REQUEST_MAX: usize = 2 * 1024 * 1024;

fn cleanup_body<T: Serialize>(operation_id: &str, items: &T) -> Result<Value, CommandFailure> {
    let body = json!({"operationId": operation_id, "items": items});
    let length = serde_json::to_vec(&body)
        .map_err(|_| CommandFailure::transport(Operation::TrashCleanup))?
        .len();
    if length > CLEANUP_REQUEST_MAX {
        return Err(CommandFailure::limit_exceeded(
            "requestBodyBytesMaximum",
            CLEANUP_REQUEST_MAX,
            length,
        ));
    }
    Ok(body)
}

pub(crate) fn unknown_receipt(operation_id: &str) -> CommandFailure {
    CommandFailure::from_payload(
        7,
        ErrorPayload {
            code: "outcome_unknown".to_owned(),
            message: "The cleanup result could not be inspected; check the same operation again."
                .to_owned(),
            effect: "unknown".to_owned(),
            details: json!({"operationId": operation_id}),
        },
    )
}

pub(crate) fn valid_trash_page(
    value: &Value,
    expected_start: Option<u64>,
    expected_limit: Option<u64>,
) -> bool {
    if value.get("namespace").and_then(Value::as_str) != Some("service")
        || value.get("libraryRoot").and_then(Value::as_str).is_none()
        || value.get("cleanupMaximum").and_then(Value::as_u64) != Some(MAXIMUM_TRASH_IDS as u64)
        || value.get("evaluatedAt").and_then(Value::as_str).is_none()
        || !["expiresAt", "nextCursor"].iter().all(|key| {
            value
                .get(key)
                .is_some_and(|field| field.is_null() || field.is_string())
        })
        || !value
            .get("operation")
            .is_some_and(|field| field.is_null() || field.is_object())
    {
        return false;
    }
    let (Some(start), Some(limit), Some(total)) = (
        value.get("start").and_then(Value::as_u64),
        value.get("limit").and_then(Value::as_u64),
        value.get("total").and_then(Value::as_u64),
    ) else {
        return false;
    };
    let Some(items) = value.get("photos").and_then(Value::as_array) else {
        return false;
    };
    let item_count = items.len() as u64;
    let expected_items = std::cmp::min(limit, total.saturating_sub(start));
    if !(1..=60).contains(&limit)
        || item_count > limit
        || item_count != expected_items
        || expected_start.is_some_and(|expected| expected != start)
        || expected_limit.is_some_and(|expected| expected != limit)
        || value
            .get("nextCursor")
            .is_some_and(|field| field.is_string())
            != (start + item_count < total)
        || value
            .get("nextCursor")
            .is_some_and(|field| field.is_string())
            != value
                .get("expiresAt")
                .is_some_and(|field| field.is_string())
    {
        return false;
    }
    items.iter().all(|item| {
        item.get("photoId")
            .and_then(Value::as_str)
            .is_some_and(valid_library_id)
            && item
                .get("originalId")
                .and_then(Value::as_str)
                .is_some_and(valid_library_id)
            && item
                .get("originalLocation")
                .and_then(Value::as_str)
                .is_some_and(valid_original_location)
            && item
                .get("removedAtMs")
                .and_then(Value::as_i64)
                .is_some_and(|marker| marker >= 0)
            && item
                .get("originalKind")
                .and_then(Value::as_str)
                .is_some_and(|kind| matches!(kind, "raw" | "jpeg"))
            && item
                .get("originalSize")
                .is_some_and(|size| size.is_null() || size.as_u64().is_some())
            && item.get("originalAvailable").is_some_and(Value::is_boolean)
            && item
                .get("state")
                .and_then(Value::as_str)
                .is_some_and(|state| matches!(state, "captured" | "changed" | "unavailable"))
            && item
                .get("photo")
                .is_some_and(|photo| photo.is_null() || photo.is_object())
    })
}

/// Check that a cleanup receipt accounts for exactly its reviewed items.
pub(crate) fn valid_cleanup_result(
    value: &Value,
    operation_id: &str,
    expected: Option<&TrashCleanupInput>,
) -> bool {
    if value.get("operationId").and_then(Value::as_str) != Some(operation_id) {
        return false;
    }
    let Some(items) = value.get("items").and_then(Value::as_array) else {
        return false;
    };
    if value.get("reviewed").and_then(Value::as_u64) != Some(items.len() as u64)
        || items.is_empty()
        || items.len() > MAXIMUM_TRASH_IDS
    {
        return false;
    }
    if expected.is_some_and(|input| input.items.len() != items.len()) {
        return false;
    }
    let Some(counts) = value.get("counts").and_then(Value::as_object) else {
        return false;
    };
    let states = [
        ("cleaned", "cleaned"),
        ("changed", "changed"),
        ("present", "present"),
        ("blocked", "blocked"),
        ("unavailable", "unavailable"),
        ("inspection-failed", "inspectionFailed"),
    ];
    let mut actual = [0_u64; 6];
    let mut ids = std::collections::HashSet::new();
    for (index, item) in items.iter().enumerate() {
        let (Some(id), Some(location), Some(kind), Some(state), Some(blockers)) = (
            item.get("photoId").and_then(Value::as_str),
            item.get("originalLocation").and_then(Value::as_str),
            item.get("originalKind").and_then(Value::as_str),
            item.get("state").and_then(Value::as_str),
            item.get("blockers").and_then(Value::as_array),
        ) else {
            return false;
        };
        let Some(position) = states.iter().position(|(name, _)| *name == state) else {
            return false;
        };
        if !valid_library_id(id)
            || !ids.insert(id)
            || !valid_original_location(location)
            || !matches!(kind, "raw" | "jpeg")
            || !item
                .get("message")
                .is_some_and(|message| message.is_null() || message.is_string())
            || !blockers.iter().all(|blocker| {
                ["kind", "name", "action"].iter().all(|key| {
                    blocker
                        .get(key)
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty())
                })
            })
            || expected.is_some_and(|input| {
                input.items[index].photo_id != id
                    || input.items[index].original_location != location
            })
        {
            return false;
        }
        actual[position] += 1;
    }
    states
        .iter()
        .enumerate()
        .all(|(index, (_, key))| counts.get(*key).and_then(Value::as_u64) == Some(actual[index]))
}

/// Map a validated cleanup receipt to its contract outcome: a mixed batch is
/// `trash_cleanup_partial`, an all-refused batch is `conflict`, and an
/// all-unavailable batch is `not_found`. A fully cleaned batch returns `None`.
pub(crate) fn cleanup_failure(data: &Value) -> Option<CommandFailure> {
    let counts = data.get("counts")?.as_object()?;
    let items = data.get("items")?.as_array()?;
    let cleaned = counts.get("cleaned")?.as_u64()?;
    if cleaned == items.len() as u64 {
        return None;
    }
    if cleaned > 0 {
        return Some(CommandFailure::from_payload(
            5,
            ErrorPayload {
                code: "trash_cleanup_partial".to_owned(),
                message: "Read every per-record outcome; only records whose Originals were confirmed absent were cleaned up.".to_owned(),
                effect: "partial".to_owned(),
                details: json!({"counts": counts, "items": items}),
            },
        ));
    }
    let all_unavailable = items
        .iter()
        .all(|item| item.get("state").and_then(Value::as_str) == Some("unavailable"));
    let (exit_code, code, message) = if all_unavailable {
        (
            3,
            "not_found",
            "No reviewed Trash record matched the submitted evidence. Reopen Trash for a fresh listing.",
        )
    } else {
        (
            4,
            "conflict",
            "No reviewed record was cleaned up. Inspect the refused records and retry only with fresh evidence.",
        )
    };
    Some(CommandFailure::from_payload(
        exit_code,
        ErrorPayload {
            code: code.to_owned(),
            message: message.to_owned(),
            effect: "none".to_owned(),
            details: json!({"counts": counts}),
        },
    ))
}

pub(crate) fn new_operation_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{nanos:x}{:08x}{counter:016x}", std::process::id())
}

pub(crate) async fn execute_cleanup(
    client: &ServiceClient,
    admission: &AdmissionState,
    operation: Operation,
    args: &TrashCleanupArgs,
    pending: &TrashCleanupInput,
) -> Result<Value, CommandFailure> {
    let identity = MutationIdentity {
        operation,
        photo_ids: pending
            .items
            .iter()
            .map(|item| item.photo_id.clone())
            .collect(),
        album_id: None,
        album_name: None,
        mappings: vec![json!({"operationId": args.operation_id})],
    };
    let data: Value = client
        .mutation(
            &identity,
            admission,
            client.endpoint(&["api", "trash", "cleanup"]),
            cleanup_body(&args.operation_id, &pending.items)?,
        )
        .await?;
    if !valid_cleanup_result(&data, &args.operation_id, Some(pending)) {
        return Err(CommandFailure::unknown(&identity));
    }
    if let Some(failure) = cleanup_failure(&data) {
        return Err(failure.with_data(data));
    }
    Ok(data)
}

/// Clean every current Trash record. The set is captured by paging the Trash
/// listing before one cleanup request; later arrivals do not join it.
pub(crate) async fn execute_empty(
    client: &ServiceClient,
    admission: &AdmissionState,
    operation: Operation,
    requested_operation_id: Option<&str>,
) -> Result<Value, CommandFailure> {
    if let Some(operation_id) = requested_operation_id {
        let receipt: Result<Value, CommandFailure> = client
            .json(
                Operation::TrashCleanupReceipt,
                Method::GET,
                client.endpoint(&["api", "trash", "cleanup-operations", operation_id]),
                None,
            )
            .await;
        match receipt {
            Ok(data) if valid_cleanup_result(&data, operation_id, None) => {
                return Ok(data);
            }
            Ok(_) => return Err(unknown_receipt(operation_id)),
            Err(failure) if failure.payload.code == "outcome_unknown" => {}
            Err(failure) => return Err(failure),
        }
    }
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut url = client.endpoint(&["api", "trash"]);
        if let Some(cursor) = &cursor {
            url.query_pairs_mut().append_pair("cursor", cursor);
        } else {
            url.query_pairs_mut()
                .append_pair("start", "0")
                .append_pair("limit", "60");
        }
        let data: Value = client.json(operation, Method::GET, url, None).await?;
        let expected = if cursor.is_some() {
            (None, None)
        } else {
            (Some(0), Some(60))
        };
        if !valid_trash_page(&data, expected.0, expected.1) {
            return Err(CommandFailure::transport(operation));
        }
        let Some(photos) = data.get("photos").and_then(Value::as_array) else {
            return Err(CommandFailure::transport(operation));
        };
        for photo in photos {
            let (Some(photo_id), Some(removed_at_ms), Some(original_id), Some(location)) = (
                photo.get("photoId"),
                photo.get("removedAtMs"),
                photo.get("originalId"),
                photo.get("originalLocation"),
            ) else {
                return Err(CommandFailure::transport(operation));
            };
            items.push(json!({
                "photoId": photo_id,
                "removedAtMs": removed_at_ms,
                "originalId": original_id,
                "originalLocation": location,
            }));
        }
        let maximum = data
            .get("cleanupMaximum")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0);
        if items.len() > maximum {
            return Err(CommandFailure::limit_exceeded(
                "trashCleanupMaximum",
                maximum,
                items.len(),
            ));
        }
        match data.get("nextCursor").and_then(Value::as_str) {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    if items.is_empty() {
        return Ok(json!({
            "operationId": null,
            "reviewed": 0,
            "counts": {"cleaned":0,"changed":0,"present":0,"blocked":0,"unavailable":0,"inspectionFailed":0},
            "items": [],
        }));
    }
    let operation_id = requested_operation_id
        .map(str::to_owned)
        .unwrap_or_else(new_operation_id);
    let identity = MutationIdentity {
        operation,
        photo_ids: items
            .iter()
            .filter_map(|item| item.get("photoId")?.as_str().map(str::to_owned))
            .collect(),
        album_id: None,
        album_name: None,
        mappings: vec![json!({"operationId": operation_id})],
    };
    let body = cleanup_body(&operation_id, &items)?;
    eprintln!("Cleanup operation ID: {operation_id}");
    let data: Value = client
        .mutation(
            &identity,
            admission,
            client.endpoint(&["api", "trash", "cleanup"]),
            body,
        )
        .await?;
    if !valid_cleanup_result(&data, &operation_id, None) {
        return Err(CommandFailure::unknown(&identity));
    }
    let requested: std::collections::HashSet<(&str, &str)> = items
        .iter()
        .filter_map(|item| {
            Some((
                item.get("photoId")?.as_str()?,
                item.get("originalLocation")?.as_str()?,
            ))
        })
        .collect();
    let matched = data
        .get("items")
        .and_then(Value::as_array)
        .is_some_and(|returned| {
            returned.len() == items.len()
                && returned.iter().all(|item| {
                    let (Some(photo_id), Some(location)) = (
                        item.get("photoId").and_then(Value::as_str),
                        item.get("originalLocation").and_then(Value::as_str),
                    ) else {
                        return false;
                    };
                    requested.contains(&(photo_id, location))
                })
        });
    if !matched {
        return Err(CommandFailure::unknown(&identity));
    }
    if let Some(failure) = cleanup_failure(&data) {
        return Err(failure.with_data(data));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(index: usize, state: &str) -> Value {
        json!({
            "photoId": format!("00000000-0000-4000-8000-{index:012}"),
            "originalLocation": format!("folder/{index}.jpg"),
            "originalKind": "jpeg",
            "state": state,
            "message": null,
            "blockers": []
        })
    }

    fn receipt(states: &[&str]) -> Value {
        let items: Vec<_> = states
            .iter()
            .enumerate()
            .map(|(index, state)| item(index, state))
            .collect();
        let count = |name: &str| items.iter().filter(|item| item["state"] == name).count();
        json!({
            "operationId": "attempt",
            "reviewed": items.len(),
            "counts": {
                "cleaned": count("cleaned"),
                "changed": count("changed"),
                "present": count("present"),
                "blocked": count("blocked"),
                "unavailable": count("unavailable"),
                "inspectionFailed": count("inspection-failed"),
            },
            "items": items,
        })
    }

    #[test]
    fn generated_operation_ids_are_valid_and_distinct() {
        let first = new_operation_id();
        let second = new_operation_id();
        assert!(valid_library_id(&first), "{first}");
        assert!(valid_library_id(&second), "{second}");
        assert_ne!(first, second);
    }

    #[test]
    fn cleanup_outcomes_partition_mixed_and_all_refused_batches() {
        assert!(cleanup_failure(&receipt(&["cleaned"])).is_none());
        let mixed = cleanup_failure(&receipt(&["cleaned", "present"])).unwrap();
        assert_eq!(mixed.exit_code, 5);
        assert_eq!(mixed.payload.code, "trash_cleanup_partial");
        assert_eq!(mixed.payload.effect, "partial");
        assert_eq!(mixed.payload.details["items"].as_array().unwrap().len(), 2);
        let refused = cleanup_failure(&receipt(&["present", "blocked"])).unwrap();
        assert_eq!(refused.exit_code, 4);
        assert_eq!(refused.payload.code, "conflict");
        let missing = cleanup_failure(&receipt(&["unavailable", "unavailable"])).unwrap();
        assert_eq!(missing.exit_code, 3);
        assert_eq!(missing.payload.code, "not_found");
    }

    #[test]
    fn trash_page_rejects_inconsistent_capture_evidence() {
        let empty = json!({
            "start": 0, "limit": 1, "total": 2, "cleanupMaximum": 5000,
            "evaluatedAt": "2026-01-01T00:00:00Z", "expiresAt": null,
            "nextCursor": "opaque", "libraryRoot": "/service/photos",
            "namespace": "service", "operation": null, "photos": []
        });
        assert!(!valid_trash_page(&empty, Some(0), Some(1)));
        let full = json!({
            "start": 0, "limit": 1, "total": 1, "cleanupMaximum": 5000,
            "evaluatedAt": "2026-01-01T00:00:00Z", "expiresAt": null,
            "nextCursor": null, "libraryRoot": "/service/photos",
            "namespace": "service", "operation": null,
            "photos": [{
                "photoId": "00000000-0000-4000-8000-000000000001",
                "removedAtMs": 42,
                "originalId": "00000000-0000-4000-8000-000000000002",
                "originalLocation": "a.jpg",
                "originalKind": "jpeg",
                "originalSize": null,
                "originalAvailable": false,
                "state": "captured",
                "photo": null
            }]
        });
        assert!(valid_trash_page(&full, Some(0), Some(1)));
        assert!(!valid_trash_page(&full, Some(1), Some(1)));
        assert!(!valid_trash_page(&full, Some(0), Some(2)));
    }

    #[test]
    fn trash_page_accepts_an_out_of_range_empty_page() {
        let empty = json!({
            "start": 5, "limit": 60, "total": 3, "cleanupMaximum": 5000,
            "evaluatedAt": "2026-01-01T00:00:00Z", "expiresAt": null,
            "nextCursor": null, "libraryRoot": "/service/photos",
            "namespace": "service", "operation": null, "photos": []
        });
        assert!(valid_trash_page(&empty, Some(5), Some(60)));
    }

    #[test]
    fn cleanup_body_refuses_a_request_over_the_service_limit() {
        let items = vec![
            json!({
                "photoId": "00000000-0000-4000-8000-000000000001",
                "removedAtMs": 42,
                "originalId": "00000000-0000-4000-8000-000000000002",
                "originalLocation": "a".repeat(8_000),
            });
            400
        ];
        let failure = cleanup_body("00000000-0000-4000-8000-000000000003", &items).unwrap_err();
        assert_eq!(failure.exit_code, 2);
        assert_eq!(failure.payload.code, "limit_exceeded");
    }
}
