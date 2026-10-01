use super::{
    DatabaseName, PersistenceError, StateDirectory, edit_recipe,
    owner::{photo_processing_source, random_uuid_v4, write_transaction},
};
use crate::{
    EXPORT_DEVELOPMENT_TIFF_WORKLOAD, EXPORT_FILM_JPEG_WORKLOAD, EXPORT_RETENTION_SECONDS,
    EditRecipeSettings, ExportArtifactFacts, ExportAttempt, ExportExposureRange,
    ExportLeaseOutcome, ExportRecipePayload, ExportRecord, ExportRetryOutcome, ExportSettlement,
    ExportSnapshot, ExportSourceEvidence, ExportState, ExportSubmission,
    ExportSubmissionResolution, ExportSubmitOutcome, ExportSweepResult, OriginalKind,
    WhiteBalanceIntent,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
// Export lifecycle: durable records, request-identity receipts, exactly-once
// settlement, bounded retention, and download leases. Every write runs in the
// serialized owner so a racing cancel and completion settle exactly once.

const EXPORT_RECEIPT_PREFIX: &str = "export_receipt:";
const MAXIMUM_EXPORT_REQUEST_ID_BYTES: usize = 128;
const MAXIMUM_EXPORT_OUTCOME_BYTES: usize = 200;
/// Bounded per-Photo list returned by the retained-export listing.
const EXPORT_LIST_LIMIT: usize = 60;
/// A lease protects an artifact for the duration of one download stream. This
/// bound only reclaims leases leaked by a crashed process; ordinary downloads
/// release their lease when the stream settles.
const EXPORT_LEASE_STALE_SECONDS: u64 = 24 * 60 * 60;

#[derive(serde::Serialize, serde::Deserialize)]
struct ExportPublicationClaimRow {
    incarnation: String,
    sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ExportReceipt {
    payload_digest: String,
    export_id: String,
    created_at: u64,
    settled_at: Option<u64>,
}

pub(super) fn export_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn validate_export_request_id(request_id: &str) -> bool {
    !request_id.is_empty()
        && request_id.len() <= MAXIMUM_EXPORT_REQUEST_ID_BYTES
        && !request_id.chars().any(char::is_control)
}

/// Durably records that `attempt` is about to publish `export_id`'s
/// artifact, before the rename: a restart can then tell a file published by
/// this very attempt from a stale leftover of a superseded one.
pub(super) fn claim_export_publication(
    connection: &mut Connection,
    export_id: &str,
    incarnation: &str,
    sequence: u64,
) -> Result<bool, PersistenceError> {
    let transaction = connection
        .transaction()
        .map_err(|_| PersistenceError::Storage)?;
    let claim = serde_json::json!({
        "incarnation": incarnation,
        "sequence": sequence,
    });
    transaction
        .execute(
            "INSERT OR REPLACE INTO library_metadata(key,value) VALUES(?1,?2)",
            params![export_publication_claim_key(export_id), claim.to_string()],
        )
        .map_err(|_| PersistenceError::Storage)?;
    transaction
        .commit()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(true)
}

/// The durable publication claim of an Export: the attempt whose validated
/// artifact is (about to be) renamed into place, if any.
pub(super) fn export_publication_claim(
    connection: &Connection,
    export_id: &str,
) -> Option<(String, u64)> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [export_publication_claim_key(export_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()?;
    let claim: ExportPublicationClaimRow = serde_json::from_str(&value).ok()?;
    Some((claim.incarnation, claim.sequence))
}

fn export_publication_claim_key(export_id: &str) -> String {
    format!("export_publication:{export_id}")
}

/// Export request identities are unique per Photo; the receipt key carries
/// the Photo identity next to the caller's request identity.
fn export_receipt_key(photo_id: &str, request_id: &str) -> String {
    format!("{EXPORT_RECEIPT_PREFIX}{photo_id}\0{request_id}")
}

/// A post-retention identity marker: the Export row is gone, but its
/// identity stays expired forever.
fn export_expiry_tombstone_key(export_id: &str) -> String {
    format!("export_expired:{export_id}")
}

fn read_export_expiry_tombstone(
    transaction: &Transaction<'_>,
    export_id: &str,
) -> Result<bool, PersistenceError> {
    transaction
        .query_row(
            "SELECT 1 FROM library_metadata WHERE key=?",
            [export_expiry_tombstone_key(export_id)],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|found| found.is_some())
        .map_err(|_| PersistenceError::Storage)
}

fn read_export_receipt(
    transaction: &Transaction<'_>,
    photo_id: &str,
    request_id: &str,
) -> Result<Option<ExportReceipt>, PersistenceError> {
    let value = transaction
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [export_receipt_key(photo_id, request_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| PersistenceError::Storage))
        .transpose()
}

fn write_export_receipt(
    transaction: &Transaction<'_>,
    photo_id: &str,
    request_id: &str,
    receipt: &ExportReceipt,
) -> Result<(), PersistenceError> {
    let value = serde_json::to_string(receipt).map_err(|_| PersistenceError::Storage)?;
    transaction
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![export_receipt_key(photo_id, request_id), value],
        )
        .map_err(|_| PersistenceError::Storage)?;
    Ok(())
}

struct ExportRow {
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

const EXPORT_ROW_COLUMNS: &str = "id,photo_id,state,outcome,recipe_revision,exposure_ev,
    source_revision,source_profile_id,source_size,source_sha256,recipe_digest,policy_id,
    bundle_id,attempt_incarnation,attempt_sequence,artifact_size,artifact_sha256,
    artifact_expires_at,artifact_width,artifact_height,artifact_profile_identity,
    created_at,settled_at,retain_until,workload";

fn export_row(_connection: &Connection, row: &rusqlite::Row<'_>) -> rusqlite::Result<ExportRow> {
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

fn export_record_from_row(row: ExportRow) -> Result<ExportRecord, PersistenceError> {
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

fn read_export_row(
    transaction: &Transaction<'_>,
    export_id: &str,
) -> Result<Option<ExportRecord>, PersistenceError> {
    let row = transaction
        .query_row(
            &format!("SELECT {EXPORT_ROW_COLUMNS} FROM exports WHERE id=?"),
            [export_id],
            |row| export_row(transaction, row),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    row.map(export_record_from_row).transpose()
}

/// Bytes of the finite retained-output allowance already committed. Unsettled
/// work reserves the complete bounded artifact; a published artifact counts by
/// its actual size until its disclosed expiry passes and its leases release.
fn reserved_retained_bytes(
    transaction: &Transaction<'_>,
    now: u64,
) -> Result<u64, PersistenceError> {
    let reserved: i64 = transaction
        .query_row(
            "SELECT COALESCE(SUM(reserved),0) FROM (
               SELECT CASE
                 WHEN state IN ('queued','running') THEN ?1
                 WHEN state = 'succeeded'
                      AND EXISTS(SELECT 1 FROM export_download_leases l WHERE l.export_id = exports.id)
                   THEN COALESCE(artifact_size, 0)
                 WHEN state = 'succeeded' AND artifact_expires_at > ?2
                   THEN COALESCE(artifact_size, 0)
                 ELSE 0
               END AS reserved
               FROM exports
             )",
            params![crate::MAXIMUM_EXPORT_BYTES as i64, now as i64],
            |row| row.get(0),
        )
        .map_err(|_| PersistenceError::Storage)?;
    u64::try_from(reserved).map_err(|_| PersistenceError::Storage)
}

fn reservable(
    transaction: &Transaction<'_>,
    now: u64,
    allowance: u64,
) -> Result<bool, PersistenceError> {
    let reserved = reserved_retained_bytes(transaction, now)?;
    let requested = reserved.saturating_add(crate::MAXIMUM_EXPORT_BYTES);
    Ok(requested <= allowance && requested >= crate::MAXIMUM_EXPORT_BYTES)
}

pub(super) fn submit_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    submission: ExportSubmission,
) -> Result<ExportSubmitOutcome, PersistenceError> {
    if !validate_export_request_id(&submission.request_id)
        || submission.source_profile_id.is_empty()
        || !matches!(
            submission.workload.as_str(),
            EXPORT_DEVELOPMENT_TIFF_WORKLOAD | EXPORT_FILM_JPEG_WORKLOAD
        )
    {
        return Ok(ExportSubmitOutcome::InvalidSettings);
    }
    let payload_digest = submission.payload_digest();
    write_transaction(state, database_name, connection, |transaction| {
        if let Some(receipt) =
            read_export_receipt(transaction, &submission.photo_id, &submission.request_id)?
        {
            if receipt.payload_digest != payload_digest {
                return Ok(ExportSubmitOutcome::RequestConflict);
            }
            return Ok(match read_export_row(transaction, &receipt.export_id)? {
                Some(record) => ExportSubmitOutcome::Existing(record),
                // The export row is removed exactly when its retention window
                // passes, so a surviving receipt without a row is expired and
                // can never start new work.
                None => ExportSubmitOutcome::Expired,
            });
        }
        let Some(current) = edit_recipe::read_edit_recipe(transaction, &submission.photo_id)?
        else {
            return Ok(ExportSubmitOutcome::UnknownPhoto);
        };
        let Some((kind, available)) = photo_processing_source(transaction, &submission.photo_id)?
        else {
            return Ok(ExportSubmitOutcome::UnknownPhoto);
        };
        if kind != OriginalKind::Raw {
            return Ok(ExportSubmitOutcome::UnsupportedPhoto);
        }
        if !available || !current.source_available {
            // A proxy never backs an Export: the Original itself is required
            // for a full-resolution artifact. When a Development Proxy
            // stands in, the refusal names that actionable state; without
            // one the source facts simply cannot be read.
            if super::development_proxy::read_development_proxy(transaction, &submission.photo_id)?
                .is_none()
            {
                return Ok(ExportSubmitOutcome::Unavailable);
            }
            return Ok(ExportSubmitOutcome::OriginalRequired);
        }
        let Some(recipe) = current.recipe.as_ref() else {
            return Ok(ExportSubmitOutcome::MissingRecipe);
        };
        if current.current_source_revision.is_none() {
            return Ok(ExportSubmitOutcome::Unavailable);
        }
        if current.current_source_revision.as_deref()
            != Some(submission.expected_source_revision.as_str())
        {
            return Ok(ExportSubmitOutcome::SourceChanged(current));
        }
        if recipe.source_revision != submission.expected_source_revision {
            // The stored recipe is bound to a source other than the current
            // published revision; an Export must never execute a payload
            // captured against the old binding. A stale binding outranks a
            // stale expected recipe revision.
            return Ok(ExportSubmitOutcome::RequiresRebind);
        }
        if recipe.revision != submission.expected_recipe_revision {
            return Ok(ExportSubmitOutcome::RecipeConflict(current));
        }
        // A saved recipe outside the approved range is invalid input for the
        // Export, not a storage failure.
        let payload =
            match ExportRecipePayload::capture(&recipe.settings, submission.exposure_range) {
                Ok(payload) => payload,
                Err(_) => return Ok(ExportSubmitOutcome::InvalidSettings),
            };
        let now = export_unix_seconds();
        if !reservable(transaction, now, submission.retained_output_bytes_max)? {
            return Ok(ExportSubmitOutcome::RetainedOutputFull);
        }
        let export_id = format!("exp-{}", random_uuid_v4()?);
        transaction
            .execute(
                "INSERT INTO exports(id,photo_id,target,state,outcome,recipe_revision,
                   exposure_ev,white_balance_mode,source_revision,source_profile_id,
                   source_kind,source_size,source_sha256,recipe_digest,policy_id,bundle_id,
                   workload,created_at)
                 VALUES(?1,?2,?3,'queued',NULL,?4,?5,'as-shot',?6,?7,'raw',
                   NULL,NULL,?8,?9,?10,?3,?11)",
                params![
                    export_id,
                    submission.photo_id,
                    submission.workload,
                    recipe.revision,
                    recipe.settings.exposure_ev,
                    submission.expected_source_revision,
                    submission.source_profile_id,
                    payload.digest(),
                    submission.policy_id,
                    submission.bundle_id,
                    now as i64,
                ],
            )
            .map_err(|_| PersistenceError::Storage)?;
        write_export_receipt(
            transaction,
            &submission.photo_id,
            &submission.request_id,
            &ExportReceipt {
                payload_digest,
                export_id: export_id.clone(),
                created_at: now,
                settled_at: None,
            },
        )?;
        let record = read_export_row(transaction, &export_id)?.ok_or(PersistenceError::Storage)?;
        Ok(ExportSubmitOutcome::Created(record))
    })
}

pub(super) fn export_record(
    connection: &Connection,
    export_id: &str,
) -> Result<Option<ExportRecord>, PersistenceError> {
    let row = connection
        .query_row(
            &format!("SELECT {EXPORT_ROW_COLUMNS} FROM exports WHERE id=?"),
            [export_id],
            |row| export_row(connection, row),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    row.map(export_record_from_row).transpose()
}

/// Resolves a request identity from its receipt without any state change.
/// `None` means the identity was never recorded and submission may proceed.
pub(super) fn resolve_export_submission(
    connection: &Connection,
    photo_id: &str,
    request_id: &str,
    payload_digest: &str,
) -> Option<ExportSubmissionResolution> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [export_receipt_key(photo_id, request_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()?;
    let receipt: ExportReceipt = serde_json::from_str(&value).ok()?;
    if receipt.payload_digest != payload_digest {
        return Some(ExportSubmissionResolution::Conflict);
    }
    match export_record(connection, &receipt.export_id) {
        Ok(Some(record)) => Some(ExportSubmissionResolution::Existing(Box::new(record))),
        // The export row is removed exactly when its retention window
        // passes, so a surviving receipt without a row is expired.
        Ok(None) => Some(ExportSubmissionResolution::Expired),
        Err(_) => None,
    }
}

/// Refreshes one download lease's liveness anchor. `false` means the lease
/// is gone and the stream must stop renewing.
pub(super) fn renew_export_lease(connection: &mut Connection, lease_id: &str, now: u64) -> bool {
    connection
        .execute(
            "UPDATE export_download_leases SET created_at=?1 WHERE id=?2",
            params![now as i64, lease_id],
        )
        .map(|updated| updated > 0)
        .unwrap_or(false)
}

pub(super) fn list_photo_exports(
    connection: &Connection,
    photo_id: &str,
) -> Result<Option<Vec<ExportRecord>>, PersistenceError> {
    let known = connection
        .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |row| {
            row.get::<_, i64>(0)
        })
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if known.is_none() {
        return Ok(None);
    }
    let rows = connection
        .prepare(&format!(
            // `created_at` has one-second resolution, so the implicit rowid
            // breaks a tie by insertion order: the head of the list is the
            // Export submitted last.
            "SELECT {EXPORT_ROW_COLUMNS} FROM exports WHERE photo_id=?
             ORDER BY created_at DESC, rowid DESC LIMIT {EXPORT_LIST_LIMIT}"
        ))
        .map_err(|_| PersistenceError::Storage)?
        .query_map([photo_id], |row| export_row(connection, row))
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let records = rows
        .into_iter()
        .map(export_record_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(records))
}

/// Records the verified staged source bytes between acceptance and launch.
/// Only unfinished work accepts them, so a settled Export can never grow
/// source evidence after the fact.
pub(super) fn record_export_source(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    export_id: &str,
    size: u64,
    sha256: &str,
) -> Result<Option<ExportRecord>, PersistenceError> {
    if size == 0 || size > crate::MAXIMUM_EXPORT_BYTES || sha256.len() != 64 {
        return Err(PersistenceError::Storage);
    }
    write_transaction(state, database_name, connection, |transaction| {
        let Some(current) = read_export_row(transaction, export_id)? else {
            return Ok(None);
        };
        if current.state.is_terminal() {
            return Ok(Some(current));
        }
        transaction
            .execute(
                "UPDATE exports SET source_size=?,source_sha256=? WHERE id=?",
                params![size as i64, sha256, export_id],
            )
            .map_err(|_| PersistenceError::Storage)?;
        read_export_row(transaction, export_id)?.map(Ok).transpose()
    })
}

pub(super) fn settle_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    export_id: &str,
    settlement: ExportSettlement,
) -> Result<Option<ExportRecord>, PersistenceError> {
    let outcome_is_bounded = match &settlement {
        ExportSettlement::Succeeded { .. } => true,
        ExportSettlement::Failed { outcome, .. } => {
            !outcome.is_empty() && outcome.len() <= MAXIMUM_EXPORT_OUTCOME_BYTES
        }
    };
    if !outcome_is_bounded {
        return Err(PersistenceError::Storage);
    }
    write_transaction(state, database_name, connection, |transaction| {
        let Some(current) = read_export_row(transaction, export_id)? else {
            return Ok(None);
        };
        // Exactly-once settlement: a racing cancel or completion already
        // decided the terminal state and is never rewritten here.
        if current.state.is_terminal() {
            return Ok(Some(current));
        }
        match settlement {
            ExportSettlement::Succeeded {
                artifact_size,
                artifact_sha256,
                published_at,
                artifact_width,
                artifact_height,
                artifact_profile_identity,
            } => {
                let expiry = published_at.saturating_add(EXPORT_RETENTION_SECONDS);
                transaction
                    .execute(
                        "UPDATE exports SET state='succeeded',outcome=NULL,
                           artifact_size=?,artifact_sha256=?,artifact_expires_at=?,
                           artifact_width=?,artifact_height=?,artifact_profile_identity=?,
                           settled_at=?,retain_until=? WHERE id=?",
                        params![
                            artifact_size as i64,
                            artifact_sha256,
                            expiry as i64,
                            artifact_width as i64,
                            artifact_height as i64,
                            artifact_profile_identity,
                            published_at as i64,
                            expiry as i64,
                            export_id
                        ],
                    )
                    .map_err(|_| PersistenceError::Storage)?;
            }
            ExportSettlement::Failed {
                outcome,
                settled_at,
            } => {
                let retain = settled_at.saturating_add(EXPORT_RETENTION_SECONDS);
                transaction
                    .execute(
                        "UPDATE exports SET state='failed',outcome=?,artifact_size=NULL,
                           artifact_sha256=NULL,artifact_expires_at=NULL,settled_at=?,
                           retain_until=? WHERE id=?",
                        params![outcome, settled_at as i64, retain as i64, export_id],
                    )
                    .map_err(|_| PersistenceError::Storage)?;
            }
        }
        read_export_row(transaction, export_id)?.map(Ok).transpose()
    })
}

pub(super) fn cancel_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    export_id: &str,
) -> Result<Option<ExportRecord>, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        let Some(current) = read_export_row(transaction, export_id)? else {
            return Ok(None);
        };
        // Cancellation settles exactly once against the actual completion
        // state and never rewrites or undoes a published artifact.
        if current.state.is_terminal() {
            return Ok(Some(current));
        }
        let now = export_unix_seconds();
        transaction
            .execute(
                "UPDATE exports SET state='cancelled',artifact_size=NULL,
                   artifact_sha256=NULL,artifact_expires_at=NULL,settled_at=?,retain_until=?
                 WHERE id=?",
                params![
                    now as i64,
                    now.saturating_add(EXPORT_RETENTION_SECONDS) as i64,
                    export_id
                ],
            )
            .map_err(|_| PersistenceError::Storage)?;
        read_export_row(transaction, export_id)?.map(Ok).transpose()
    })
}

/// Persists the executor attempt identity and marks the attempt running. A
/// terminal record is returned untouched so a caller that lost a race with
/// cancellation aborts before any work.
pub(super) fn begin_export_attempt(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    export_id: &str,
    attempt: ExportAttempt,
) -> Result<Option<ExportRecord>, PersistenceError> {
    // The executor-owned incarnation is 32 lowercase hex characters.
    if attempt.sequence == 0
        || attempt.incarnation.len() != 32
        || !attempt
            .incarnation
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PersistenceError::Storage);
    }
    write_transaction(state, database_name, connection, |transaction| {
        let Some(current) = read_export_row(transaction, export_id)? else {
            return Ok(None);
        };
        if current.state.is_terminal() {
            return Ok(Some(current));
        }
        transaction
            .execute(
                "UPDATE exports SET state='running',attempt_incarnation=?,attempt_sequence=?
                 WHERE id=?",
                params![attempt.incarnation, attempt.sequence as i64, export_id],
            )
            .map_err(|_| PersistenceError::Storage)?;
        read_export_row(transaction, export_id)?.map(Ok).transpose()
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn retry_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    export_id: &str,
    request_id: &str,
    expected_bundle_id: &str,
    allowance: u64,
) -> Result<ExportRetryOutcome, PersistenceError> {
    if !validate_export_request_id(request_id) {
        return Err(PersistenceError::Storage);
    }
    let retry_digest = format!(
        "{:x}",
        Sha256::digest(format!("retry\0{export_id}").as_bytes())
    );
    write_transaction(state, database_name, connection, |transaction| {
        let Some(current) = read_export_row(transaction, export_id)? else {
            // A reclaimed record keeps its expired identity: retry reports
            // the explicit expired outcome instead of unknown.
            if read_export_expiry_tombstone(transaction, export_id)? {
                return Ok(ExportRetryOutcome::Expired);
            }
            return Ok(ExportRetryOutcome::Unknown);
        };
        // An accepted retry identity resolves to its Export and starts no
        // work; a different payload under that identity is a conflict.
        if let Some(receipt) =
            read_export_receipt(transaction, &current.snapshot.photo_id, request_id)?
        {
            if receipt.export_id == export_id && receipt.payload_digest == retry_digest {
                return Ok(ExportRetryOutcome::Replayed(Box::new(current)));
            }
            return Ok(ExportRetryOutcome::RequestConflict);
        }
        // Only a settled failure or cancellation carries a retryable
        // snapshot; a succeeded artifact is never silently re-rendered and
        // an active attempt is never replaced.
        if !matches!(current.state, ExportState::Failed | ExportState::Cancelled) {
            return Ok(ExportRetryOutcome::NotRetriable);
        }
        let now = export_unix_seconds();
        let Some(retain_until) = current.retain_until else {
            return Ok(ExportRetryOutcome::Unknown);
        };
        if retain_until <= now {
            return Ok(ExportRetryOutcome::Expired);
        }
        // Availability is validated again against the retained snapshot: a
        // swapped approved bundle or a changed/unreadable source means the
        // captured work can never execute again.
        if current.snapshot.bundle_id != expected_bundle_id {
            return Ok(ExportRetryOutcome::OutputUnavailable);
        }
        let Some(recipe) = edit_recipe::read_edit_recipe(transaction, &current.snapshot.photo_id)?
        else {
            return Ok(ExportRetryOutcome::OutputUnavailable);
        };
        if !recipe.source_available {
            return Ok(ExportRetryOutcome::ResourceUnavailable);
        }
        if recipe.current_source_revision.as_deref()
            != Some(current.snapshot.source_revision.as_str())
        {
            return Ok(ExportRetryOutcome::OutputUnavailable);
        }
        if !reservable(transaction, now, allowance)? {
            return Ok(ExportRetryOutcome::RetainedOutputFull);
        }
        transaction
            .execute(
                "UPDATE exports SET state='queued',outcome=NULL,attempt_incarnation=NULL,
                   attempt_sequence=NULL WHERE id=?",
                [export_id],
            )
            .map_err(|_| PersistenceError::Storage)?;
        write_export_receipt(
            transaction,
            &current.snapshot.photo_id,
            request_id,
            &ExportReceipt {
                payload_digest: retry_digest,
                export_id: export_id.to_owned(),
                created_at: now,
                settled_at: None,
            },
        )?;
        Ok(read_export_row(transaction, export_id)?
            .map(|record| ExportRetryOutcome::Retried(Box::new(record)))
            .unwrap_or(ExportRetryOutcome::Unknown))
    })
}

pub(super) fn sweep_export_expiry(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    now: u64,
) -> Result<ExportSweepResult, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        transaction
            .execute(
                "DELETE FROM export_download_leases WHERE created_at < ?1",
                [now.saturating_sub(EXPORT_LEASE_STALE_SECONDS) as i64],
            )
            .map_err(|_| PersistenceError::Storage)?;
        // XMP snapshots retain their provenance and request identity after
        // expiry, but the immutable document bytes are swept.
        transaction
            .execute(
                "UPDATE xmp_exports SET document=NULL
                 WHERE expires_at <= ?1 AND document IS NOT NULL",
                [now as i64],
            )
            .map_err(|_| PersistenceError::Storage)?;
        let mut result = ExportSweepResult::default();
        let expired_artifacts = transaction
            .prepare(
                "SELECT id FROM exports WHERE state='succeeded'
                 AND artifact_expires_at IS NOT NULL AND artifact_expires_at <= ?1
                 AND NOT EXISTS(SELECT 1 FROM export_download_leases l WHERE l.export_id=exports.id)",
            )
            .map_err(|_| PersistenceError::Storage)?
            .query_map([now as i64], |row| row.get::<_, String>(0))
            .map_err(|_| PersistenceError::Storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| PersistenceError::Storage)?;
        for export_id in expired_artifacts {
            transaction
                .execute(
                    "UPDATE exports SET artifact_size=NULL,artifact_sha256=NULL,
                       artifact_expires_at=NULL WHERE id=?",
                    [&export_id],
                )
                .map_err(|_| PersistenceError::Storage)?;
            result.artifact_expiry_ids.push(export_id);
        }
        let expired_records = transaction
            .prepare(
                "SELECT id FROM exports WHERE retain_until IS NOT NULL AND retain_until <= ?1
                 AND NOT EXISTS(SELECT 1 FROM export_download_leases l WHERE l.export_id=exports.id)",
            )
            .map_err(|_| PersistenceError::Storage)?
            .query_map([now as i64], |row| row.get::<_, String>(0))
            .map_err(|_| PersistenceError::Storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| PersistenceError::Storage)?;
        for export_id in expired_records {
            transaction
                .execute("DELETE FROM exports WHERE id=?", [&export_id])
                .map_err(|_| PersistenceError::Storage)?;
            // The identity stays expired forever: it can never start new
            // work and retry keeps reporting the explicit expired outcome.
            transaction
                .execute(
                    "INSERT OR REPLACE INTO library_metadata(key,value) VALUES(?1,?2)",
                    params![export_expiry_tombstone_key(&export_id), "expired"],
                )
                .map_err(|_| PersistenceError::Storage)?;
            result.record_expiry_ids.push(export_id);
        }
        Ok(result)
    })
}

pub(super) fn unfinished_exports(
    connection: &Connection,
) -> Result<Vec<ExportRecord>, PersistenceError> {
    let rows = connection
        .prepare(&format!(
            "SELECT {EXPORT_ROW_COLUMNS} FROM exports
             WHERE state IN ('queued','running') ORDER BY created_at, id"
        ))
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| export_row(connection, row))
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    rows.into_iter().map(export_record_from_row).collect()
}

pub(super) fn acquire_export_lease(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    export_id: &str,
    now: u64,
) -> Result<ExportLeaseOutcome, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        let Some(current) = read_export_row(transaction, export_id)? else {
            return Ok(ExportLeaseOutcome::Unknown);
        };
        let Some(artifact) = current.artifact.as_ref() else {
            return Ok(ExportLeaseOutcome::Unknown);
        };
        if artifact.expires_at <= now {
            return Ok(ExportLeaseOutcome::Expired);
        }
        let lease_id = format!("lease-{}", random_uuid_v4()?);
        transaction
            .execute(
                "INSERT INTO export_download_leases(id,export_id,created_at) VALUES(?,?,?)",
                params![lease_id, export_id, now as i64],
            )
            .map_err(|_| PersistenceError::Storage)?;
        Ok(ExportLeaseOutcome::Acquired {
            lease_id,
            artifact: artifact.clone(),
        })
    })
}

pub(super) fn release_export_lease(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    lease_id: &str,
) -> Result<bool, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        let changed = transaction
            .execute("DELETE FROM export_download_leases WHERE id=?", [lease_id])
            .map_err(|_| PersistenceError::Storage)?;
        Ok(changed == 1)
    })
}

#[cfg(test)]
mod tests {
    use crate::persistence::Persistence;
    use crate::persistence::export;
    use crate::persistence::test_support::*;
    use crate::{
        EXPORT_DEVELOPMENT_TIFF_WORKLOAD, EXPORT_RETENTION_SECONDS, ExportExposureRange,
        ExportLeaseOutcome, ExportRecipePayload, ExportRetryOutcome, ExportSettlement, ExportState,
        ExportSubmission, ExportSubmitOutcome, LibraryRoot,
    };
    use rusqlite::Connection;
    use rusqlite::params;
    use std::path::Path;

    fn export_test_revision(relative_path: &str, size: i64, mtime_ms: f64) -> String {
        crate::source_revision(relative_path, u64::try_from(size).unwrap(), mtime_ms).unwrap()
    }

    fn seed_current_schema(library: &LibraryRoot, path: &Path) {
        seed(
            path,
            include_str!("../../../../compatibility/sqlite/schema-v8.sql"),
        );
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
    }

    fn seed_export_photo(connection: &Connection) -> (String, String) {
        add_recipe_test_photo(
            connection,
            RecipeTestPhoto {
                original_id: "raw-original",
                photo_id: "raw-photo",
                relative_path: "shoot/one.ARW",
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
        connection
            .execute(
                "UPDATE original_files SET capture_metadata_state='missing',capture_source_revision=? WHERE id='raw-original'",
                [format!(
                    "{}\0fixture-device\0fixture-inode",
                    export_test_revision("shoot/one.ARW", 17, 1_000.0)
                )],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode)
                 VALUES('raw-photo','recipe-rev-1',?1,0.5,'as-shot')",
                params![crate::source_revision("shoot/one.ARW", 17_u64, 1_000.0).unwrap()],
            )
            .unwrap();
        add_recipe_test_photo(
            connection,
            RecipeTestPhoto {
                original_id: "jpeg-original",
                photo_id: "jpeg-photo",
                relative_path: "shoot/two.JPG",
                kind: "jpeg",
                available: true,
                size: 19,
                mtime_ms: 1_000.0,
            },
        );
        (
            "recipe-rev-1".to_owned(),
            export_test_revision("shoot/one.ARW", 17, 1_000.0),
        )
    }

    fn export_submission(
        request_id: &str,
        recipe_revision: &str,
        source_revision: &str,
        allowance: u64,
    ) -> ExportSubmission {
        ExportSubmission {
            request_id: request_id.to_owned(),
            photo_id: "raw-photo".to_owned(),
            source_profile_id: "sony-ilce-7rm5-arw".to_owned(),
            policy_id: "a".repeat(64),
            bundle_id: "b".repeat(64),
            workload: EXPORT_DEVELOPMENT_TIFF_WORKLOAD.to_owned(),
            expected_recipe_revision: recipe_revision.to_owned(),
            expected_source_revision: source_revision.to_owned(),
            exposure_range: ExportExposureRange {
                minimum_milli_ev: 0,
                maximum_milli_ev: 1000,
            },
            retained_output_bytes_max: allowance,
        }
    }

    // Export lifecycle: submit snapshot capture and guarded rejection, request
    // identity replay, exactly-once settlement and cancellation, retry against
    // a retained snapshot, capacity reservation, and retention with leases.
    #[tokio::test]
    async fn export_submit_captures_snapshot_and_replays_identity_exactly() {
        let (_base, library, state, name, path) = fixture();
        seed_current_schema(&library, &path);
        let (recipe_revision, source_revision) = {
            let connection = Connection::open(&path).unwrap();
            seed_export_photo(&connection)
        };
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();

        let created = persistence
            .submit_export_receiver(export_submission(
                "request-1",
                &recipe_revision,
                &source_revision,
                8 * 1024 * 1024 * 1024,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let ExportSubmitOutcome::Created(record) = created else {
            panic!("first submission must be created");
        };
        assert_eq!(record.state, ExportState::Queued);
        assert_eq!(record.snapshot.photo_id, "raw-photo");
        assert_eq!(record.snapshot.recipe_revision, "recipe-rev-1");
        assert_eq!(record.snapshot.settings.exposure_ev, 0.5);
        assert_eq!(record.snapshot.source_revision, source_revision);
        assert_eq!(record.snapshot.source_profile_id, "sony-ilce-7rm5-arw");
        assert_eq!(record.snapshot.workload, "development-tiff");
        assert_eq!(record.attempt, None);
        let payload = ExportRecipePayload::capture(
            &record.snapshot.settings,
            ExportExposureRange {
                minimum_milli_ev: 0,
                maximum_milli_ev: 1000,
            },
        )
        .unwrap();
        assert_eq!(record.snapshot.recipe_digest, payload.digest());
        assert_eq!(payload.exposure_milli_ev, 500);
        assert!(record.settled_at.is_none() && record.retain_until.is_none());
        assert_eq!(record.source, None);

        let replayed = persistence
            .submit_export_receiver(export_submission(
                "request-1",
                &recipe_revision,
                &source_revision,
                8 * 1024 * 1024 * 1024,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let ExportSubmitOutcome::Existing(replayed) = replayed else {
            panic!("identical replay must resolve to the existing Export");
        };
        assert_eq!(replayed.id, record.id);

        let conflicting = persistence
            .submit_export_receiver(export_submission(
                "request-1",
                "other-revision",
                &source_revision,
                8 * 1024 * 1024 * 1024,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(conflicting, ExportSubmitOutcome::RequestConflict);

        let stale = persistence
            .submit_export_receiver(export_submission(
                "request-2",
                "older-recipe-rev",
                &source_revision,
                8 * 1024 * 1024 * 1024,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let ExportSubmitOutcome::RecipeConflict(facts) = stale else {
            panic!("stale recipe revision must conflict");
        };
        assert_eq!(facts.recipe.as_ref().unwrap().revision, "recipe-rev-1");

        let changed = persistence
            .submit_export_receiver(export_submission(
                "request-3",
                &recipe_revision,
                &export_test_revision("shoot/one.ARW", 17, 2_000.0),
                8 * 1024 * 1024 * 1024,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(changed, ExportSubmitOutcome::SourceChanged(_)));

        let unsupported = persistence
            .submit_export_receiver(ExportSubmission {
                photo_id: "jpeg-photo".to_owned(),
                ..export_submission(
                    "request-4",
                    &recipe_revision,
                    &source_revision,
                    8 * 1024 * 1024 * 1024,
                )
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unsupported, ExportSubmitOutcome::UnsupportedPhoto);

        let unknown = persistence
            .submit_export_receiver(ExportSubmission {
                photo_id: "absent-photo".to_owned(),
                ..export_submission(
                    "request-5",
                    &recipe_revision,
                    &source_revision,
                    8 * 1024 * 1024 * 1024,
                )
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unknown, ExportSubmitOutcome::UnknownPhoto);

        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn export_settle_cancel_race_settles_exactly_once_and_retry_rearms() {
        let (_base, library, state, name, path) = fixture();
        seed_current_schema(&library, &path);
        let (recipe_revision, source_revision) = {
            let connection = Connection::open(&path).unwrap();
            seed_export_photo(&connection)
        };
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let allowance = 8 * 1024 * 1024 * 1024;
        macro_rules! submit_export {
            ($request_id:expr) => {{
                let outcome = persistence
                    .submit_export_receiver(export_submission(
                        $request_id,
                        &recipe_revision,
                        &source_revision,
                        allowance,
                    ))
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap();
                match outcome {
                    ExportSubmitOutcome::Created(record) => record,
                    _ => panic!("submission must be created"),
                }
            }};
        }

        let cancelled = submit_export!("request-cancel");
        let record = persistence
            .cancel_export_receiver(&cancelled.id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(record.state, ExportState::Cancelled);
        assert!(record.settled_at.is_some());
        assert!(record.retain_until.is_some());
        let again = persistence
            .cancel_export_receiver(&cancelled.id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(again.state, ExportState::Cancelled);
        assert_eq!(again.settled_at, record.settled_at);
        let late_completion = persistence
            .settle_export_receiver(
                &cancelled.id,
                ExportSettlement::Succeeded {
                    artifact_size: 10,
                    artifact_sha256: "c".repeat(64),
                    published_at: export::export_unix_seconds(),
                    artifact_width: 2,
                    artifact_height: 1,
                    artifact_profile_identity: "e".repeat(64),
                },
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(late_completion.state, ExportState::Cancelled);

        let failed = submit_export!("request-failed");
        let record = persistence
            .settle_export_receiver(
                &failed.id,
                ExportSettlement::Failed {
                    outcome: "processing attempt did not complete: engine-failed".to_owned(),
                    settled_at: export::export_unix_seconds(),
                },
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(record.state, ExportState::Failed);
        let retried = persistence
            .retry_export_receiver(&failed.id, "retry-1", &"b".repeat(64), allowance)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let ExportRetryOutcome::Retried(record) = retried else {
            panic!("failed Export must retry");
        };
        assert_eq!(record.state, ExportState::Queued);
        assert_eq!(record.outcome, None);
        assert_eq!(record.attempt, None);
        assert_eq!(record.snapshot.recipe_revision, "recipe-rev-1");
        // The accepted retry identity replays to the current record without
        // starting work.
        assert!(matches!(
            persistence
                .retry_export_receiver(&failed.id, "retry-1", &"b".repeat(64), allowance)
                .unwrap()
                .await
                .unwrap()
                .unwrap(),
            ExportRetryOutcome::Replayed(_)
        ));
        // The consumed retry identity against a different Export conflicts.
        let queued = submit_export!("request-queued");
        assert_eq!(
            persistence
                .retry_export_receiver(&queued.id, "retry-1", &"b".repeat(64), allowance)
                .unwrap()
                .await
                .unwrap()
                .unwrap(),
            ExportRetryOutcome::RequestConflict
        );
        // An unfinished Export is never retried.
        assert_eq!(
            persistence
                .retry_export_receiver(&queued.id, "retry-2", &"b".repeat(64), allowance)
                .unwrap()
                .await
                .unwrap()
                .unwrap(),
            ExportRetryOutcome::NotRetriable
        );

        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn export_capacity_leases_and_expiry_refuse_before_acceptance() {
        let (_base, library, state, name, path) = fixture();
        seed_current_schema(&library, &path);
        let (recipe_revision, source_revision) = {
            let connection = Connection::open(&path).unwrap();
            seed_export_photo(&connection)
        };
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let allowance = 8 * 1024 * 1024 * 1024;

        let refused = persistence
            .submit_export_receiver(export_submission(
                "request-full",
                &recipe_revision,
                &source_revision,
                1024,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refused, ExportSubmitOutcome::RetainedOutputFull);

        let outcome = persistence
            .submit_export_receiver(export_submission(
                "request-published",
                &recipe_revision,
                &source_revision,
                allowance,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let ExportSubmitOutcome::Created(record) = outcome else {
            panic!("submission must be created");
        };
        let published_at = export::export_unix_seconds();
        let settled = persistence
            .settle_export_receiver(
                &record.id,
                ExportSettlement::Succeeded {
                    artifact_size: 4096,
                    artifact_sha256: "d".repeat(64),
                    published_at,
                    artifact_width: 16,
                    artifact_height: 9,
                    artifact_profile_identity: "e".repeat(64),
                },
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(settled.state, ExportState::Succeeded);
        let artifact = settled.artifact.clone().unwrap();
        assert_eq!(artifact.size, 4096);
        assert_eq!(artifact.sha256, "d".repeat(64));
        assert_eq!(artifact.expires_at, published_at + EXPORT_RETENTION_SECONDS);
        assert_eq!(artifact.width, 16);
        assert_eq!(artifact.height, 9);
        assert_eq!(artifact.profile_identity, "e".repeat(64));
        assert_eq!(
            settled.retain_until,
            Some(published_at + EXPORT_RETENTION_SECONDS)
        );

        let after_retention = published_at + EXPORT_RETENTION_SECONDS + 1;
        // The lease is fresh relative to the sweep; a week-old lease would be
        // crash debris and reclaimed by the same sweep.
        let lease = persistence
            .acquire_export_lease_receiver(&record.id, after_retention - 3600)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let ExportLeaseOutcome::Acquired {
            lease_id,
            artifact: leased,
        } = lease
        else {
            panic!("a live artifact must lease");
        };
        assert_eq!(leased, artifact);
        let after_retention = published_at + EXPORT_RETENTION_SECONDS + 1;
        let sweep = persistence
            .sweep_export_expiry_receiver(after_retention)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(sweep.record_expiry_ids.is_empty());
        assert!(
            persistence
                .export_receiver(&record.id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .is_some()
        );

        assert!(
            persistence
                .release_export_lease_receiver(&lease_id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        );
        let sweep = persistence
            .sweep_export_expiry_receiver(after_retention)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sweep.record_expiry_ids, vec![record.id.clone()]);
        assert!(
            persistence
                .export_receiver(&record.id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        let expired = persistence
            .submit_export_receiver(export_submission(
                "request-published",
                &recipe_revision,
                &source_revision,
                allowance,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(expired, ExportSubmitOutcome::Expired);

        persistence.shutdown().unwrap();
    }

    /// An unavailable Original refuses every Export before acceptance. When
    /// a Development Proxy stands in, the refusal is the actionable
    /// `OriginalRequired` — a proxy never backs a full-resolution Export —
    /// and no Export row or receipt is created; when the Original returns,
    /// the same guarded submission is admitted again.
    #[tokio::test]
    async fn export_submit_refuses_original_required_only_while_a_proxy_stands_in() {
        let (_base, library, state, name, path) = fixture();
        seed_current_schema(&library, &path);
        let (recipe_revision, source_revision) = {
            let connection = Connection::open(&path).unwrap();
            seed_export_photo(&connection)
        };
        let persistence = Persistence::open(
            crate::persistence::admission::StateDirectory::open_or_create(
                &library,
                state.canonical_path(),
            )
            .unwrap(),
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let submit = || {
            let receiver = persistence
                .submit_export_receiver(export_submission(
                    "request-proxy",
                    &recipe_revision,
                    &source_revision,
                    8 * 1024 * 1024 * 1024,
                ))
                .unwrap();
            async move { receiver.await.unwrap().unwrap() }
        };

        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE original_files SET available=0 WHERE id='raw-original'",
                [],
            )
            .unwrap();
        connection
            .execute("UPDATE photos SET available=0 WHERE id='raw-photo'", [])
            .unwrap();
        assert_eq!(
            submit().await,
            ExportSubmitOutcome::Unavailable,
            "an unavailable Original without a proxy is a plain unreadable source"
        );

        assert!(
            crate::persistence::development_proxy::record_development_proxy(
                &state,
                &name,
                &mut Connection::open(&path).unwrap(),
                crate::DevelopmentProxyRecord {
                    photo_id: "raw-photo".to_owned(),
                    source_revision: source_revision.clone(),
                    source_relative_path: "shoot/one.ARW".to_owned(),
                    source_sha256: "a".repeat(64),
                    source_size: 17,
                    profile_id: "sony-ilce-7rm5-arw".to_owned(),
                    pipeline_version: crate::DEVELOPMENT_PROXY_PIPELINE_VERSION.to_owned(),
                    bundle_sha256: "b".repeat(64),
                    long_edge: crate::DEVELOPMENT_PROXY_LONG_EDGE,
                    width: 2560,
                    height: 1707,
                    artifact_sha256: "c".repeat(64),
                    artifact_bytes: 2048,
                    created_at: 1_700_000_000,
                },
            )
            .unwrap()
        );
        assert_eq!(
            submit().await,
            ExportSubmitOutcome::OriginalRequired,
            "a proxy never backs an Export, and the refusal must say so"
        );
        assert!(
            persistence
                .photo_exports_receiver("raw-photo")
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .is_none_or(|records| records.is_empty()),
            "a refused submission creates no Export row"
        );

        connection
            .execute(
                "UPDATE original_files SET available=1 WHERE id='raw-original'",
                [],
            )
            .unwrap();
        connection
            .execute("UPDATE photos SET available=1 WHERE id='raw-photo'", [])
            .unwrap();
        drop(connection);
        let admitted = submit().await;
        assert!(
            matches!(admitted, ExportSubmitOutcome::Created(_)),
            "the Original's return admits the same guarded submission: {admitted:?}"
        );
        persistence.shutdown().unwrap();
    }
}
