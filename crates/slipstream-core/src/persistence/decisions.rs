//! Photo State and checked Photo Decision mutations: compare-and-set writes
//! with batch classification, version advancement, and undo.

use super::owner::{
    MutationError, MutationVersions, PhotoDecisionWriteError, mutation_error_from_sqlite,
    mutation_transaction, parse_selection_state, photo_decision_write_error_from_mutation,
    selection_state_value,
};
use super::{DatabaseName, StateDirectory};
use crate::{
    CheckedPhotoDecisionCounts, CheckedPhotoDecisionItemResult, CheckedPhotoDecisionMutation,
    CheckedPhotoDecisionOutcome, CheckedPhotoDecisionResult, PhotoDecisionFacts,
    PhotoDecisionSnapshot, PhotoStateBatchApplied, PhotoStateBatchChangedElsewhere,
    PhotoStateBatchMissing, PhotoStateBatchMutation, PhotoStateBatchResult, PhotoStateField,
    PhotoStateMutation, PhotoStateMutationResult, PhotoStateUndo, PhotoStateValue,
};
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn state_value(
    selection: &str,
    rating: i64,
    field: PhotoStateField,
) -> Result<PhotoStateValue, MutationError> {
    match field {
        PhotoStateField::SelectionState => parse_selection_state(selection)
            .map(PhotoStateValue::Selection)
            .map_err(|_| MutationError::Persistence),
        PhotoStateField::Rating => rating
            .try_into()
            .map(PhotoStateValue::Rating)
            .map_err(|_| MutationError::Persistence),
    }
}

pub(super) fn mutate_photo_state(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: PhotoStateMutation,
) -> Result<PhotoStateMutationResult, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        let (selection, rating): (String, i64) = transaction
            .query_row(
                "SELECT selection_state,rating FROM photos WHERE id=?",
                [&mutation.photo_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(mutation_error_from_sqlite)?
            .ok_or(MutationError::NotFound)?;
        let prior = state_value(&selection, rating, mutation.field)?;
        if mutation
            .expected_current
            .is_some_and(|expected| expected != prior)
        {
            return Err(MutationError::Conflict);
        }
        if let Some(album_id) = mutation.album_id.as_deref() {
            transaction
                .query_row(
                    "SELECT 1 FROM album_members WHERE album_id=? AND photo_id=?",
                    params![album_id, mutation.photo_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(mutation_error_from_sqlite)?
                .ok_or(MutationError::NotFound)?;
        }
        match mutation.value {
            PhotoStateValue::Selection(value) => {
                transaction
                    .execute(
                        "UPDATE photos SET selection_state=? WHERE id=?",
                        params![selection_state_value(value), mutation.photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
            }
            PhotoStateValue::Rating(value) => {
                transaction
                    .execute(
                        "UPDATE photos SET rating=? WHERE id=?",
                        params![i64::from(value), mutation.photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
            }
        }
        if let Some(album_id) = mutation.album_id.as_deref() {
            transaction
                .execute(
                    "INSERT INTO album_progress(album_id,photo_id) VALUES(?,?)
                     ON CONFLICT(album_id) DO UPDATE SET photo_id=excluded.photo_id",
                    params![album_id, mutation.photo_id],
                )
                .map_err(mutation_error_from_sqlite)?;
        }
        Ok(PhotoStateMutationResult {
            photo_id: mutation.photo_id.clone(),
            undo: PhotoStateUndo {
                photo_id: mutation.photo_id,
                field: mutation.field,
                prior_value: prior,
                expected_current: mutation.value,
            },
        })
    })
}

/// One bounded batch Selection State write. Every requested Photo is resolved
/// inside one transaction and reports exactly one outcome, so matching Photos
/// can be confirmed even when another requested Photo is missing or changed.
pub(super) fn mutate_photo_state_batch(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: PhotoStateBatchMutation,
) -> Result<PhotoStateBatchResult, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        let mut applied = Vec::with_capacity(mutation.photos.len());
        let mut changed_elsewhere = Vec::new();
        let mut missing = Vec::new();
        for item in &mutation.photos {
            let row: Option<String> = transaction
                .query_row(
                    "SELECT selection_state FROM photos WHERE id=?",
                    [&item.photo_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(mutation_error_from_sqlite)?;
            let Some(selection) = row else {
                missing.push(PhotoStateBatchMissing {
                    photo_id: item.photo_id.clone(),
                });
                continue;
            };
            let prior_value =
                parse_selection_state(&selection).map_err(|_| MutationError::Persistence)?;
            if prior_value != item.expected_current {
                changed_elsewhere.push(PhotoStateBatchChangedElsewhere {
                    photo_id: item.photo_id.clone(),
                    current_value: prior_value,
                });
                continue;
            }
            transaction
                .execute(
                    "UPDATE photos SET selection_state=? WHERE id=?",
                    params![selection_state_value(mutation.value), &item.photo_id],
                )
                .map_err(mutation_error_from_sqlite)?;
            applied.push(PhotoStateBatchApplied {
                photo_id: item.photo_id.clone(),
                prior_value,
            });
        }
        Ok(PhotoStateBatchResult {
            applied,
            changed_elsewhere,
            missing,
        })
    })
}
/// The current decision facts together with their guard, as one serialized
/// owner operation sees them.
fn photo_decision_snapshot(
    versions: &MutationVersions,
    facts: PhotoDecisionFacts,
    photo_id: &str,
) -> PhotoDecisionSnapshot {
    PhotoDecisionSnapshot {
        selection_state: facts.selection_state,
        rating: facts.rating,
        decision_version: versions.photo(photo_id),
    }
}

/// One version-checked Photo decision batch. Classification, effective
/// writes, and version advancement run in this single serialized owner
/// command: every effective change commits in one transaction, storage
/// failure rolls back all of them and preserves versions, and conflicts or
/// missing records are per-item domain results in request order.
pub(super) fn mutate_photo_decision_checked(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    versions: &mut MutationVersions,
    mutation: CheckedPhotoDecisionMutation,
) -> Result<CheckedPhotoDecisionResult, PhotoDecisionWriteError> {
    enum Classification {
        Missing,
        Conflict { current: PhotoDecisionSnapshot },
        Unchanged { current: PhotoDecisionSnapshot },
        Change { prior: PhotoDecisionFacts },
    }
    let current_value = |facts: PhotoDecisionFacts| match mutation.field {
        PhotoStateField::SelectionState => PhotoStateValue::Selection(facts.selection_state),
        PhotoStateField::Rating => PhotoStateValue::Rating(facts.rating),
    };
    let mut classified = Vec::with_capacity(mutation.photos.len());
    for item in &mutation.photos {
        let facts = connection
            .query_row(
                "SELECT selection_state,rating FROM photos WHERE id=?",
                [&item.photo_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|_| PhotoDecisionWriteError::Persistence)
            .and_then(|row| {
                row.map(|(selection_state, rating)| {
                    Ok(PhotoDecisionFacts {
                        selection_state: parse_selection_state(&selection_state)
                            .map_err(|_| PhotoDecisionWriteError::Persistence)?,
                        rating: rating
                            .try_into()
                            .map_err(|_| PhotoDecisionWriteError::Persistence)?,
                    })
                })
                .transpose()
            })?;
        // The guard is deliberately checked before classifying an otherwise
        // idempotent request. A changed-away-and-back decision still conflicts.
        let classification = match facts {
            None => Classification::Missing,
            Some(facts) => {
                let current = photo_decision_snapshot(versions, facts, &item.photo_id);
                if current.decision_version != item.expected_version {
                    Classification::Conflict { current }
                } else if current_value(facts) == mutation.value {
                    Classification::Unchanged { current }
                } else {
                    Classification::Change { prior: facts }
                }
            }
        };
        classified.push((item.photo_id.clone(), classification));
    }
    let effective = classified
        .iter()
        .filter(|(_, classification)| matches!(classification, Classification::Change { .. }))
        .map(|(photo_id, _)| photo_id.clone())
        .collect::<Vec<_>>();
    // Counter saturation fails closed before any write rather than wrapping.
    if effective
        .iter()
        .any(|photo_id| !versions.can_advance_photo(photo_id))
    {
        return Err(PhotoDecisionWriteError::Persistence);
    }
    mutation_transaction(state, database_name, connection, |transaction| {
        for photo_id in &effective {
            match mutation.value {
                PhotoStateValue::Selection(value) => {
                    transaction
                        .execute(
                            "UPDATE photos SET selection_state=? WHERE id=?",
                            params![selection_state_value(value), photo_id],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                }
                PhotoStateValue::Rating(value) => {
                    transaction
                        .execute(
                            "UPDATE photos SET rating=? WHERE id=?",
                            params![i64::from(value), photo_id],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                }
            }
        }
        Ok(())
    })
    .map_err(photo_decision_write_error_from_mutation)?;
    // Counters become visible only after the committed transaction; a
    // rollback above leaves every prior version unchanged.
    for photo_id in &effective {
        versions
            .advance_photo(photo_id)
            .map_err(photo_decision_write_error_from_mutation)?;
    }
    let mut counts = CheckedPhotoDecisionCounts::default();
    let mut results = Vec::with_capacity(classified.len());
    for (photo_id, classification) in classified {
        let outcome = match classification {
            Classification::Missing => {
                counts.missing += 1;
                CheckedPhotoDecisionOutcome::Missing
            }
            Classification::Conflict { current } => {
                counts.conflict += 1;
                CheckedPhotoDecisionOutcome::Conflict { current }
            }
            Classification::Unchanged { current } => {
                counts.unchanged += 1;
                CheckedPhotoDecisionOutcome::Unchanged { current }
            }
            Classification::Change { prior } => {
                counts.changed += 1;
                let after = PhotoDecisionFacts {
                    selection_state: match (mutation.field, mutation.value) {
                        (PhotoStateField::SelectionState, PhotoStateValue::Selection(value)) => {
                            value
                        }
                        _ => prior.selection_state,
                    },
                    rating: match (mutation.field, mutation.value) {
                        (PhotoStateField::Rating, PhotoStateValue::Rating(value)) => value,
                        _ => prior.rating,
                    },
                };
                CheckedPhotoDecisionOutcome::Changed {
                    prior,
                    current: photo_decision_snapshot(versions, after, &photo_id),
                }
            }
        };
        results.push(CheckedPhotoDecisionItemResult { photo_id, outcome });
    }
    Ok(CheckedPhotoDecisionResult { results, counts })
}
