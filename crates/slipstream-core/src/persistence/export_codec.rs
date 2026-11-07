//! Stored Export row decoding and integrity validation.
//!
//! The export owner keeps transactions, receipts, and lifecycle policy. This
//! module owns the persisted row shape and the all-or-nothing conversion to a
//! domain ExportRecord.

use super::PersistenceError;
use crate::{
    EXPORT_DEVELOPMENT_TIFF_WORKLOAD, EXPORT_FILM_JPEG_WORKLOAD, EditRecipeSettings,
    ExportArtifactFacts, ExportAttempt, ExportExposureRange, ExportRecipePayload, ExportRecord,
    ExportSnapshot, ExportSourceEvidence, ExportState, OriginalKind, WhiteBalanceIntent,
};

pub(super) struct ExportRow {
    id: String,
    photo_id: String,
    state: ExportState,
    outcome: Option<String>,
    recipe_revision: String,
    exposure_ev: f64,
    source_revision: String,
    source_profile_id: String,
    source_size: Option<u64>,
    source_sha256: Option<String>,
    recipe_digest: String,
    policy_id: String,
    bundle_id: String,
    workload: String,
    attempt_incarnation: Option<String>,
    attempt_sequence: Option<u64>,
    artifact_size: Option<u64>,
    artifact_sha256: Option<String>,
    artifact_expires_at: Option<u64>,
    artifact_width: Option<u32>,
    artifact_height: Option<u32>,
    artifact_profile_identity: Option<String>,
    created_at: u64,
    settled_at: Option<u64>,
    retain_until: Option<u64>,
}

pub(super) const EXPORT_ROW_COLUMNS: &str = "id,photo_id,state,outcome,recipe_revision,exposure_ev,
    source_revision,source_profile_id,source_size,source_sha256,recipe_digest,policy_id,
    bundle_id,attempt_incarnation,attempt_sequence,artifact_size,artifact_sha256,
    artifact_expires_at,artifact_width,artifact_height,artifact_profile_identity,
    created_at,settled_at,retain_until,workload";

pub(super) fn export_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExportRow> {
    let state_name: String = row.get(2)?;
    let state = ExportState::parse_name(&state_name).ok_or(rusqlite::Error::InvalidQuery)?;
    let source_size = match row.get::<_, Option<i64>>(8)? {
        None => None,
        Some(value) => Some(u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)?),
    };
    let attempt_sequence: Option<u64> = match row.get::<_, Option<i64>>(14)? {
        None => None,
        Some(value) => Some(u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)?),
    };
    let artifact_size = match row.get::<_, Option<i64>>(15)? {
        None => None,
        Some(value) => Some(u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)?),
    };
    let artifact_width = match row.get::<_, Option<i64>>(18)? {
        None => None,
        Some(value) => u32::try_from(value)
            .map(Some)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
    };
    let artifact_height = match row.get::<_, Option<i64>>(19)? {
        None => None,
        Some(value) => u32::try_from(value)
            .map(Some)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
    };
    let created_at =
        u64::try_from(row.get::<_, i64>(21)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let settled_at = match row.get::<_, Option<i64>>(22)? {
        None => None,
        Some(value) => Some(u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)?),
    };
    let retain_until = match row.get::<_, Option<i64>>(23)? {
        None => None,
        Some(value) => Some(u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)?),
    };
    Ok(ExportRow {
        id: row.get(0)?,
        photo_id: row.get(1)?,
        state,
        outcome: row.get(3)?,
        recipe_revision: row.get(4)?,
        exposure_ev: row.get(5)?,
        source_revision: row.get(6)?,
        source_profile_id: row.get(7)?,
        source_size,
        source_sha256: row.get(9)?,
        recipe_digest: row.get(10)?,
        policy_id: row.get(11)?,
        bundle_id: row.get(12)?,
        workload: row.get(24)?,
        attempt_incarnation: row.get(13)?,
        attempt_sequence,
        artifact_size,
        artifact_sha256: row.get(16)?,
        artifact_expires_at: row
            .get::<_, Option<i64>>(17)?
            .map(|value| u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        artifact_width,
        artifact_height,
        artifact_profile_identity: row.get(20)?,
        created_at,
        settled_at,
        retain_until,
    })
}

pub(super) fn export_record_from_row(row: ExportRow) -> Result<ExportRecord, PersistenceError> {
    let settings = EditRecipeSettings {
        exposure_ev: row.exposure_ev,
        white_balance: WhiteBalanceIntent::AsShot,
    };
    let attempt = match (&row.attempt_incarnation, row.attempt_sequence) {
        (Some(incarnation), Some(sequence)) => Some(ExportAttempt {
            incarnation: incarnation.clone(),
            sequence,
        }),
        (None, None) => None,
        _ => return Err(PersistenceError::Storage),
    };
    let artifact = match (
        row.artifact_size,
        row.artifact_sha256,
        row.artifact_expires_at,
        row.artifact_width,
        row.artifact_height,
        &row.artifact_profile_identity,
    ) {
        (
            Some(size),
            Some(sha256),
            Some(expires_at),
            Some(width),
            Some(height),
            Some(profile_identity),
        ) => Some(ExportArtifactFacts {
            size,
            sha256,
            expires_at,
            width,
            height,
            profile_identity: profile_identity.clone(),
        }),
        (None, None, None, None, None, None) => None,
        // A partial artifact row can never be served as validated metadata.
        _ => return Err(PersistenceError::Storage),
    };
    let source = match (row.source_size, row.source_sha256) {
        (Some(size), Some(sha256)) => Some(ExportSourceEvidence { size, sha256 }),
        (None, None) => None,
        _ => return Err(PersistenceError::Storage),
    };
    let payload = ExportRecipePayload::capture(
        &settings,
        ExportExposureRange {
            minimum_milli_ev: i64::MIN,
            maximum_milli_ev: i64::MAX,
        },
    )
    .map_err(|_| PersistenceError::Storage)?;
    if payload.digest() != row.recipe_digest
        || !matches!(
            row.workload.as_str(),
            EXPORT_DEVELOPMENT_TIFF_WORKLOAD | EXPORT_FILM_JPEG_WORKLOAD
        )
    {
        return Err(PersistenceError::Storage);
    }
    Ok(ExportRecord {
        id: row.id,
        snapshot: ExportSnapshot {
            photo_id: row.photo_id,
            recipe_revision: row.recipe_revision,
            settings,
            source_revision: row.source_revision,
            source_kind: OriginalKind::Raw,
            source_profile_id: row.source_profile_id,
            policy_id: row.policy_id,
            bundle_id: row.bundle_id,
            workload: row.workload,
            recipe_digest: row.recipe_digest,
        },
        source,
        state: row.state,
        outcome: row.outcome,
        attempt,
        artifact,
        created_at: row.created_at,
        settled_at: row.settled_at,
        retain_until: row.retain_until,
    })
}
