use super::*;
use crate::PhotoRecord;
use std::fs;

#[test]
fn large_library_selects_only_missing_and_fact_changed_evidence() {
    let facts = crate::OriginalFacts {
        size: 10,
        mtime_ms: 1_000.0,
        device: 1,
        inode: 1,
    };
    let mut discovered = Vec::with_capacity(20_000);
    let mut previous = ScanSnapshot {
        published: true,
        originals: Vec::with_capacity(20_000),
        photos: Vec::new(),
        errors: Vec::new(),
    };
    for index in 0..20_000 {
        let path = crate::RelativeOriginalPath::parse(format!("photos/{index}.ARW")).unwrap();
        previous.originals.push(OriginalRecord {
            id: index.to_string(),
            relative_path: path.clone(),
            kind: OriginalKind::Raw,
            facts,
            available: true,
            error_category: None,
            error_message: None,
            capture: crate::CaptureFact::pending(),
        });
        if index != 19_998 {
            discovered.push(DiscoveredOriginal {
                path,
                kind: OriginalKind::Raw,
                facts: if index == 19_999 {
                    crate::OriginalFacts { size: 11, ..facts }
                } else {
                    facts
                },
                error_category: None,
                error_message: None,
                capture: crate::CaptureFact::pending(),
            });
        }
    }
    assert_eq!(
        evidence_original_ids(&discovered, &previous),
        ["19998".to_owned(), "19999".to_owned()]
    );
}

fn original_record(id: &str, path: &str, available: bool) -> OriginalRecord {
    OriginalRecord {
        id: id.to_owned(),
        relative_path: crate::RelativeOriginalPath::parse(path).unwrap(),
        kind: OriginalKind::Jpeg,
        facts: crate::OriginalFacts {
            size: 12,
            mtime_ms: 1_000.0,
            device: 0,
            inode: 0,
        },
        available,
        error_category: None,
        error_message: None,
        capture: crate::CaptureFact::pending(),
    }
}

fn photo_record(id: &str, original_id: &str, available: bool, removed: bool) -> PhotoRecord {
    PhotoRecord {
        id: id.to_owned(),
        original_id: original_id.to_owned(),
        available,
        preview_state: crate::PreviewState::Unavailable,
        preview_source_revision: None,
        preview_width: None,
        preview_height: None,
        cache_revision: None,
        sort_path: "shoot/a.JPG".to_owned(),
        selection_state: crate::SelectionState::Unflagged,
        rating: 0,
        has_saved_edits: false,
        removed,
    }
}

#[test]
fn removed_photos_are_never_relocation_candidates() {
    let base = std::env::temp_dir().join(format!(
        "slipstream-recovery-removed-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(base.join("originals/moved")).unwrap();
    fs::write(base.join("originals/moved/a.JPG"), b"jpeg-bytes-a").unwrap();
    fs::write(base.join("originals/moved/b.JPG"), b"jpeg-bytes-b").unwrap();
    let root = crate::LibraryRoot::open(base.join("originals")).unwrap();
    let previous = ScanSnapshot {
        published: true,
        originals: vec![
            original_record("removed-original", "shoot/a.JPG", false),
            original_record("active-original", "shoot/b.JPG", false),
        ],
        photos: vec![
            photo_record("removed-photo", "removed-original", false, true),
            photo_record("active-photo", "active-original", false, false),
        ],
        errors: Vec::new(),
    };
    let candidates = [
        discovered("moved/a.JPG", b"jpeg-bytes-a", 2_000.0),
        discovered("moved/b.JPG", b"jpeg-bytes-b", 3_000.0),
    ];
    let fingerprints = [
        crate::OriginalFingerprint {
            original_id: "removed-original".to_owned(),
            digest: crate::recovery::digest_bytes(b"jpeg-bytes-a"),
            size: 12,
            mtime_ms: 1_000.0,
        },
        crate::OriginalFingerprint {
            original_id: "active-original".to_owned(),
            digest: crate::recovery::digest_bytes(b"jpeg-bytes-b"),
            size: 12,
            mtime_ms: 1_000.0,
        },
    ];
    let plan = plan_recovery(
        &root,
        &NativeWorkBudget::new(),
        &candidates,
        &previous,
        &fingerprints,
        &std::collections::HashSet::new(),
        &mut (),
    );
    assert_eq!(
        plan.relocations
            .values()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["active-original"]
    );
    assert_eq!(
        plan.relocations
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["moved/b.JPG"]
    );
    assert_eq!(
        evidence_original_ids(&[], &previous),
        ["active-original".to_owned()]
    );
    let _ = fs::remove_dir_all(&base);
}

fn discovered(path: &str, bytes: &[u8], mtime_ms: f64) -> DiscoveredOriginal {
    DiscoveredOriginal {
        path: crate::RelativeOriginalPath::parse(path).unwrap(),
        kind: OriginalKind::Jpeg,
        facts: crate::OriginalFacts {
            size: bytes.len() as u64,
            mtime_ms,
            device: 0,
            inode: 0,
        },
        error_category: None,
        error_message: None,
        capture: crate::CaptureFact::pending(),
    }
}
