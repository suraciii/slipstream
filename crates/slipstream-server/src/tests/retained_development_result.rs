use super::*;
use crate::export_manager::{
    RetainedDevelopmentIdentity, RetainedDevelopmentTiff, retained_development_tiff_of,
};
use slipstream_core::{
    EditRecipeSettings, ExportArtifactFacts, ExportRecord, ExportSnapshot, ExportState,
    OriginalKind, WhiteBalanceIntent,
};

const BUNDLE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const REVISION: &str = "recipe-revision";
const SOURCE_REVISION: &str = "source-revision";
const EXPOSURE_MILLI_EV: i64 = 500;
const ARTIFACT_SHA256: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

fn identity<'a>(exposure_milli_ev: i64) -> RetainedDevelopmentIdentity<'a> {
    RetainedDevelopmentIdentity {
        matches_baseline: false,
        recipe_revision: Some(REVISION),
        exposure_milli_ev,
        source_revision: SOURCE_REVISION,
        bundle_sha256: BUNDLE,
    }
}

fn record(
    state: ExportState,
    white_balance: WhiteBalanceIntent,
    artifact: Option<ExportArtifactFacts>,
) -> ExportRecord {
    ExportRecord {
        id: "export-1".to_owned(),
        snapshot: ExportSnapshot {
            photo_id: "photo-1".to_owned(),
            recipe_revision: REVISION.to_owned(),
            settings: EditRecipeSettings {
                exposure_ev: EXPOSURE_MILLI_EV as f64 / 1_000.0,
                white_balance,
            },
            source_revision: SOURCE_REVISION.to_owned(),
            source_kind: OriginalKind::Raw,
            source_profile_id: "profile".to_owned(),
            policy_id: "policy".to_owned(),
            bundle_id: BUNDLE.to_owned(),
            workload: "development-tiff".to_owned(),
            recipe_digest: "digest".to_owned(),
        },
        source: None,
        state,
        outcome: None,
        attempt: None,
        artifact,
        created_at: 0,
        settled_at: None,
        retain_until: None,
    }
}

fn artifact(expires_at: u64) -> ExportArtifactFacts {
    ExportArtifactFacts {
        size: 4096,
        sha256: ARTIFACT_SHA256.to_owned(),
        expires_at,
        width: 2,
        height: 1,
        profile_identity: "profile-identity".to_owned(),
    }
}

fn resolve(
    record: &ExportRecord,
    identity: &RetainedDevelopmentIdentity<'_>,
    now: u64,
) -> Option<RetainedDevelopmentTiff> {
    resolve_all(std::slice::from_ref(record), identity, now)
}

fn resolve_all(
    records: &[ExportRecord],
    identity: &RetainedDevelopmentIdentity<'_>,
    now: u64,
) -> Option<RetainedDevelopmentTiff> {
    retained_development_tiff_of(records, identity, now, |export_id| {
        Some(PathBuf::from(format!("/artifacts/{export_id}.tiff")))
    })
}

#[test]
fn a_succeeded_export_retains_the_development_tiff_of_its_identity() {
    let record = record(
        ExportState::Succeeded,
        WhiteBalanceIntent::AsShot,
        Some(artifact(2_000)),
    );
    let retained = resolve(&record, &identity(EXPOSURE_MILLI_EV), 1_000)
        .expect("the retained Development TIFF of the current identity");
    assert_eq!(
        retained.path,
        PathBuf::from("/artifacts/export-1.tiff"),
        "the retained artifact is the published Export artifact"
    );
    assert_eq!(retained.sha256, ARTIFACT_SHA256);
    assert_eq!(retained.byte_length, 4096);
    assert_eq!(retained.recipe_revision, REVISION);
    assert_eq!(retained.exposure_milli_ev, EXPOSURE_MILLI_EV);
    assert_eq!(retained.source_revision, SOURCE_REVISION);
    assert_eq!(retained.bundle_id, BUNDLE);
}

#[test]
fn a_result_of_another_identity_is_never_retained_as_current() {
    let record = record(
        ExportState::Succeeded,
        WhiteBalanceIntent::AsShot,
        Some(artifact(2_000)),
    );
    // Another recipe revision, exposure, source revision, or bundle is a
    // different identity, and a Photo without a saved recipe can never
    // match a snapshot that captured one.
    let other_exposure = resolve(&record, &identity(EXPOSURE_MILLI_EV + 1), 1_000);
    assert!(
        other_exposure.is_none(),
        "a different exposure is not current"
    );
    let no_recipe = RetainedDevelopmentIdentity {
        recipe_revision: None,
        ..identity(EXPOSURE_MILLI_EV)
    };
    assert!(
        resolve(&record, &no_recipe, 1_000).is_none(),
        "the processing baseline is not the captured recipe"
    );
    let other_source = RetainedDevelopmentIdentity {
        source_revision: "another-source",
        ..identity(EXPOSURE_MILLI_EV)
    };
    assert!(
        resolve(&record, &other_source, 1_000).is_none(),
        "a changed source is not current"
    );
    let other_bundle = RetainedDevelopmentIdentity {
        bundle_sha256: "another-bundle",
        ..identity(EXPOSURE_MILLI_EV)
    };
    assert!(
        resolve(&record, &other_bundle, 1_000).is_none(),
        "a different bundle is not current"
    );
}

#[test]
fn only_a_succeeded_export_with_a_live_artifact_is_retained() {
    let now = 1_000;
    for state in [
        ExportState::Queued,
        ExportState::Running,
        ExportState::Failed,
        ExportState::Cancelled,
    ] {
        let record = record(state, WhiteBalanceIntent::AsShot, Some(artifact(2_000)));
        assert!(
            resolve(&record, &identity(EXPOSURE_MILLI_EV), now).is_none(),
            "{state:?} retains no Development Result"
        );
    }
    let unsettled = record(ExportState::Succeeded, WhiteBalanceIntent::AsShot, None);
    assert!(
        resolve(&unsettled, &identity(EXPOSURE_MILLI_EV), now).is_none(),
        "a succeeded Export without a published artifact retains no result"
    );
    // The artifact's disclosed expiry is the retention: an expired
    // artifact is not retained, and the boundary itself is expired.
    let expired = record(
        ExportState::Succeeded,
        WhiteBalanceIntent::AsShot,
        Some(artifact(now)),
    );
    assert!(resolve(&expired, &identity(EXPOSURE_MILLI_EV), now).is_none());
    let live = record(
        ExportState::Succeeded,
        WhiteBalanceIntent::AsShot,
        Some(artifact(now + 1)),
    );
    assert!(resolve(&live, &identity(EXPOSURE_MILLI_EV), now).is_some());
}

#[test]
fn the_retained_result_is_the_first_matching_record_in_retention_order() {
    // A newer Export of another identity must not hide an older Export
    // that does match: the selection filters on the identity, not on the
    // head of the list.
    let newer_other = {
        let mut record = record(
            ExportState::Succeeded,
            WhiteBalanceIntent::AsShot,
            Some(artifact(2_000)),
        );
        record.id = "export-newer".to_owned();
        record.snapshot.recipe_revision = "another-revision".to_owned();
        record
    };
    let older_matching = record(
        ExportState::Succeeded,
        WhiteBalanceIntent::AsShot,
        Some(artifact(2_000)),
    );
    let retained = resolve_all(
        &[newer_other, older_matching],
        &identity(EXPOSURE_MILLI_EV),
        1_000,
    )
    .expect("the matching record behind a newer one is retained");
    assert_eq!(retained.path, PathBuf::from("/artifacts/export-1.tiff"));

    // Two Exports of one identity are the same Development Result; the
    // first record in retention order is the one served.
    let first = record(
        ExportState::Succeeded,
        WhiteBalanceIntent::AsShot,
        Some(artifact(2_000)),
    );
    let second = {
        let mut record = first.clone();
        record.id = "export-second".to_owned();
        record
    };
    let retained = resolve_all(&[first, second], &identity(EXPOSURE_MILLI_EV), 1_000)
        .expect("one of the matching records is retained");
    assert_eq!(retained.path, PathBuf::from("/artifacts/export-1.tiff"));
}

#[test]
fn a_baseline_identity_matches_the_captured_baseline_settings() {
    let mut record = record(
        ExportState::Succeeded,
        WhiteBalanceIntent::AsShot,
        Some(artifact(2_000)),
    );
    record.snapshot.settings.exposure_ev = 0.0;
    // The baseline selector names the processing baseline rather than a
    // saved recipe, so the captured revision is not part of its identity:
    // a result produced under exactly the baseline settings is the same
    // development whatever revision captured it.
    let baseline = RetainedDevelopmentIdentity {
        matches_baseline: true,
        recipe_revision: None,
        exposure_milli_ev: 0,
        source_revision: SOURCE_REVISION,
        bundle_sha256: BUNDLE,
    };
    assert!(resolve(&record, &baseline, 1_000).is_some());
    // The current selector still matches the captured revision exactly,
    // so a request that names no revision is not current.
    let current = RetainedDevelopmentIdentity {
        matches_baseline: false,
        exposure_milli_ev: 0,
        ..baseline
    };
    assert!(resolve(&record, &current, 1_000).is_none());
    let named = RetainedDevelopmentIdentity {
        recipe_revision: Some(REVISION),
        ..current
    };
    assert!(resolve(&record, &named, 1_000).is_some());
    // A result captured at another exposure is a different development,
    // so it is not the baseline comparison either.
    let mut other = record.clone();
    other.snapshot.settings.exposure_ev = EXPOSURE_MILLI_EV as f64 / 1_000.0;
    assert!(resolve(&other, &baseline, 1_000).is_none());
}

#[test]
fn a_snapshot_that_cannot_execute_retains_no_result() {
    // A temperature-tint snapshot can never produce an execution payload,
    // so it never ran and cannot be the retained Development Result.
    let record = record(
        ExportState::Succeeded,
        WhiteBalanceIntent::TemperatureTint {
            temperature_kelvin: 5_000,
            tint_milli: 0,
        },
        Some(artifact(2_000)),
    );
    assert!(resolve(&record, &identity(EXPOSURE_MILLI_EV), 1_000).is_none());
}
