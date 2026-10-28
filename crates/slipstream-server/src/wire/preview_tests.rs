mod tests {
    use super::super::*;
    #[test]
    fn review_capability_is_consistent_across_ready_and_photo_wires() {
        use slipstream_core::{
            CaptureFact, OriginalFacts, OriginalKind, OriginalRecord, PhotoRead, PhotoRecord,
            RelativeOriginalPath,
        };
        use std::collections::HashMap;

        let original = OriginalRecord {
            id: "original".into(),
            relative_path: RelativeOriginalPath::parse("photo.jpg").unwrap(),
            kind: OriginalKind::Jpeg,
            facts: OriginalFacts::UNREADABLE,
            available: true,
            error_category: None,
            error_message: None,
            capture: CaptureFact::pending(),
        };
        let originals = vec![original];
        let indexes = HashMap::from([("original".to_owned(), 0)]);
        for (width, height, limited) in [(2559, 1700, true), (1700, 2560, false)] {
            let ready = slipstream_core::PreviewReady {
                cache_key: "key".into(),
                target: DerivativeTarget::Review2560,
                source: PreviewSource::JpegOriginal,
                source_size: 1,
                source_mtime_ms: 0.0,
                embedded_candidate_identity: None,
                width,
                height,
                generated: false,
            };
            let response =
                serde_json::to_value(PreviewResponse::ready("photo", &ready, false)).unwrap();
            assert_eq!(response["limitedDetail"], limited);
            assert!(response.get("detailLimited").is_none());

            let photo = PhotoRecord {
                id: "photo".into(),
                original_id: "original".into(),
                available: true,
                preview_state: PreviewState::Ready,
                preview_source_revision: Some("rev".into()),
                preview_width: Some(width),
                preview_height: Some(height),
                cache_revision: None,
                sort_path: "photo.jpg".into(),
                selection_state: SelectionState::Unflagged,
                rating: 0,
                has_saved_edits: false,
                removed: false,
            };
            let summary = photo_summary_indexed_with_url(&photo, &originals, &indexes, None, None);
            assert_eq!(
                serde_json::to_value(summary).unwrap()["preview"]["limitedDetail"],
                limited
            );

            let read = PhotoRead {
                id: "photo".into(),
                original_id: "original".into(),
                original_location: "photo.jpg".into(),
                filename: "photo.jpg".into(),
                original_kind: OriginalKind::Jpeg,
                original_available: true,
                selection_state: SelectionState::Unflagged,
                removed_at_ms: None,
                rating: 0,
                decision_version: "v1".into(),
                capture: CaptureFact::pending(),
                preview_state: PreviewState::Ready,
                preview_source: Some(PreviewSource::JpegOriginal),
                preview_source_revision: Some("rev".into()),
                preview_width: Some(width),
                preview_height: Some(height),
                has_saved_edits: false,
            };
            let cli = serde_json::to_value(CliPhotoItemWire::from(read.clone())).unwrap();
            assert_eq!(cli["preview"]["detailLimited"], limited);
            assert!(cli["preview"].get("limitedDetail").is_none());

            let mut unavailable = read;
            unavailable.preview_state = PreviewState::Unavailable;
            let cli = serde_json::to_value(CliPhotoItemWire::from(unavailable)).unwrap();
            assert!(cli["preview"]["detailLimited"].is_null());
        }
        let unavailable =
            serde_json::to_value(PreviewResponse::unavailable("unavailable")).unwrap();
        assert!(unavailable.get("limitedDetail").is_none());
    }
}
