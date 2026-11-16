use super::*;

#[test]
fn list_validation_enforces_requested_and_global_page_bounds() {
    let list = ListData {
        items: vec![(); 2],
        total: 2,
        next_cursor: None,
        evaluated_at: "2026-01-01T00:00:00Z".to_owned(),
        expires_at: None,
    };
    assert!(!list_expiry_valid(&list, 1));
    assert!(list_expiry_valid(&list, 2));
    let continuation = ListData {
        items: vec![(); MAXIMUM_LIST_PAGE + 1],
        total: (MAXIMUM_LIST_PAGE + 1) as u64,
        next_cursor: Some("cursor".to_owned()),
        evaluated_at: "2026-01-01T00:00:00Z".to_owned(),
        expires_at: Some("2026-01-01T00:15:00Z".to_owned()),
    };
    assert!(!list_expiry_valid(&continuation, MAXIMUM_LIST_PAGE));
}

#[test]
fn list_page_holds_a_page_to_its_bound_and_shapes_the_shared_envelope() {
    assert_eq!(
        list_page_limit(false, None, DEFAULT_LIST_PAGE, MAXIMUM_LIST_PAGE),
        DEFAULT_LIST_PAGE
    );
    assert_eq!(
        list_page_limit(false, Some(10), DEFAULT_LIST_PAGE, MAXIMUM_LIST_PAGE),
        10
    );
    assert_eq!(
        list_page_limit(true, None, DEFAULT_LIST_PAGE, MAXIMUM_LIST_PAGE),
        MAXIMUM_LIST_PAGE
    );
    let page = ListData {
        items: vec![1],
        total: 3,
        next_cursor: Some("cursor".to_owned()),
        evaluated_at: "2026-01-01T00:00:00Z".to_owned(),
        expires_at: Some("2026-01-01T00:15:00Z".to_owned()),
    };
    let value = list_page(Operation::Status, page, MAXIMUM_LIST_PAGE, |item| {
        Ok(json!(item))
    })
    .unwrap();
    assert_eq!(
        value,
        json!({
            "items": [1],
            "total": 3,
            "nextCursor": "cursor",
            "evaluatedAt": "2026-01-01T00:00:00Z",
            "expiresAt": "2026-01-01T00:15:00Z",
        })
    );
    // A continuation that is present but empty is not a confirmed page.
    let empty_cursor = ListData {
        items: vec![1],
        total: 3,
        next_cursor: Some(String::new()),
        evaluated_at: "2026-01-01T00:00:00Z".to_owned(),
        expires_at: Some("2026-01-01T00:15:00Z".to_owned()),
    };
    assert!(
        list_page(Operation::Status, empty_cursor, MAXIMUM_LIST_PAGE, |item| {
            Ok(json!(item))
        })
        .is_err()
    );
}
