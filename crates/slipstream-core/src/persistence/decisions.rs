//! Photo State and checked Photo Decision mutations: compare-and-set writes
//! with batch classification, version advancement, and undo.

use super::mutation::{mutation_error_from_sqlite, mutation_transaction};
use super::owner::{
    MutationError, MutationVersions, PhotoDecisionWriteError,
    photo_decision_write_error_from_mutation,
};
use super::scan::{parse_selection_state, selection_state_value};
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

#[cfg(test)]
mod tests {
    use crate::persistence::admission::StateDirectory;
    use crate::persistence::owner::validate_photo_state_batch_mutation;
    use crate::persistence::test_support::*;
    use crate::persistence::{MutationError, Persistence, PhotoDecisionWriteError};
    use crate::{
        AlbumMutation, CheckedPhotoDecisionItem, CheckedPhotoDecisionItemResult,
        CheckedPhotoDecisionMutation, CheckedPhotoDecisionOutcome, MAXIMUM_PHOTO_RATING,
        OriginalKind, PhotoDecisionFacts, PhotoDecisionSnapshot, PhotoStateBatchApplied,
        PhotoStateBatchChangedElsewhere, PhotoStateBatchItem, PhotoStateBatchMissing,
        PhotoStateBatchMutation, PhotoStateField, PhotoStateMutation, PhotoStateValue,
        SelectionState,
    };
    use rusqlite::Connection;

    /// A batch that names no Photo, names one twice, or exceeds the bound is
    /// the request's own defect, so it is classified as invalid before any
    /// state is read or written.
    #[test]
    fn batch_photo_state_validation_classifies_malformed_requests_as_invalid() {
        let item = |photo_id: &str| crate::PhotoStateBatchItem {
            photo_id: photo_id.to_owned(),
            expected_current: SelectionState::Undecided,
        };
        let valid = PhotoStateBatchMutation {
            photos: vec![item("one"), item("two")],
            value: SelectionState::Selected,
        };
        assert_eq!(validate_photo_state_batch_mutation(&valid), Ok(()));
        let malformed = [
            PhotoStateBatchMutation {
                photos: Vec::new(),
                value: SelectionState::Rejected,
            },
            PhotoStateBatchMutation {
                photos: vec![item("one"), item("one")],
                value: SelectionState::Rejected,
            },
            PhotoStateBatchMutation {
                photos: vec![item("one")],
                value: SelectionState::Undecided,
            },
            PhotoStateBatchMutation {
                photos: (0..=crate::PHOTO_STATE_BATCH_MAX)
                    .map(|index| item(&format!("photo-{index}")))
                    .collect(),
                value: SelectionState::Selected,
            },
        ];
        for mutation in malformed {
            assert_eq!(
                validate_photo_state_batch_mutation(&mutation),
                Err(MutationError::Invalid)
            );
        }
    }

    #[tokio::test]
    async fn batch_photo_state_compares_each_photo_and_reports_a_complete_partition() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 1.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[0].clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Selected),
                expected_current: Some(PhotoStateValue::Selection(SelectionState::Undecided)),
                album_id: None,
            })
            .await
            .unwrap();

        let result = persistence
            .mutate_photo_state_batch_receiver(PhotoStateBatchMutation {
                photos: vec![
                    PhotoStateBatchItem {
                        photo_id: ids[0].clone(),
                        expected_current: SelectionState::Undecided,
                    },
                    PhotoStateBatchItem {
                        photo_id: ids[1].clone(),
                        expected_current: SelectionState::Undecided,
                    },
                    PhotoStateBatchItem {
                        photo_id: "missing-photo".to_owned(),
                        expected_current: SelectionState::Undecided,
                    },
                ],
                value: SelectionState::Rejected,
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.applied,
            vec![PhotoStateBatchApplied {
                photo_id: ids[1].clone(),
                prior_value: SelectionState::Undecided,
            }]
        );
        assert_eq!(
            result.changed_elsewhere,
            vec![PhotoStateBatchChangedElsewhere {
                photo_id: ids[0].clone(),
                current_value: SelectionState::Selected,
            }]
        );
        assert_eq!(
            result.missing,
            vec![PhotoStateBatchMissing {
                photo_id: "missing-photo".to_owned(),
            }]
        );
        let after = persistence.snapshot().await.unwrap();
        assert_eq!(
            after
                .photos
                .iter()
                .map(|photo| photo.selection_state)
                .collect::<Vec<_>>(),
            vec![SelectionState::Selected, SelectionState::Rejected]
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn batch_photo_state_rolls_back_earlier_matches_when_a_later_write_fails() {
        let (_base, library, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 1.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let escaped_id = ids[1].replace('\'', "''");
        Connection::open(&path)
            .unwrap()
            .execute_batch(&format!(
                "CREATE TRIGGER fail_batch_second BEFORE UPDATE OF selection_state ON photos\n                 WHEN OLD.id = '{escaped_id}'\n                 BEGIN SELECT RAISE(ABORT, 'forced batch failure'); END;"
            ))
            .unwrap();

        let result = persistence
            .mutate_photo_state_batch_receiver(PhotoStateBatchMutation {
                photos: ids
                    .iter()
                    .map(|photo_id| PhotoStateBatchItem {
                        photo_id: photo_id.clone(),
                        expected_current: SelectionState::Undecided,
                    })
                    .collect(),
                value: SelectionState::Selected,
            })
            .unwrap()
            .await
            .unwrap();
        assert!(result.is_err());
        let after = persistence.snapshot().await.unwrap();
        assert!(
            after
                .photos
                .iter()
                .all(|photo| photo.selection_state == SelectionState::Undecided)
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn checked_photo_decisions_classify_guard_and_invalidate_across_writers() {
        let (base, library, state, name, _path) = fixture();
        let canonical_root = library.canonical_path().to_string_lossy().into_owned();
        let persistence = Persistence::open(state, name.clone(), canonical_root.clone()).unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                    discovered("three.JPG", OriginalKind::Jpeg, 3, 3.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let read_photo = |photo_id: String| {
            let persistence = persistence.clone();
            async move {
                persistence
                    .photo_receiver(&photo_id)
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
            }
        };

        // One single-field change reports both decision fields before and
        // after, advances the version, and leaves the other field untouched.
        let initial = read_photo(ids[0].clone()).await;
        let changed = persistence
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Selected),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: initial.decision_version.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(changed.counts.changed, 1);
        assert_eq!(changed.counts.unchanged, 0);
        assert_eq!(changed.counts.conflict, 0);
        assert_eq!(changed.counts.missing, 0);
        assert_eq!(
            changed.results,
            vec![CheckedPhotoDecisionItemResult {
                photo_id: ids[0].clone(),
                outcome: CheckedPhotoDecisionOutcome::Changed {
                    prior: PhotoDecisionFacts {
                        selection_state: SelectionState::Undecided,
                        rating: 0,
                    },
                    current: PhotoDecisionSnapshot {
                        selection_state: SelectionState::Selected,
                        rating: 0,
                        decision_version: read_photo(ids[0].clone()).await.decision_version,
                    },
                },
            }]
        );
        let observed = read_photo(ids[0].clone()).await;
        assert_eq!(observed.selection_state, SelectionState::Selected);
        assert_eq!(observed.rating, 0);
        let selected_version = observed.decision_version;
        assert_ne!(selected_version, initial.decision_version);

        // An intervening Web write that changes the value away and back must
        // still conflict with the version read before those edits. Web Undo
        // replays the same single-Photo writer, so it shares this guard.
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[0].clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Undecided),
                expected_current: Some(PhotoStateValue::Selection(SelectionState::Selected)),
                album_id: None,
            })
            .await
            .unwrap();
        let undone = read_photo(ids[0].clone()).await;
        assert_eq!(undone.selection_state, SelectionState::Undecided);
        let away_and_back_version = undone.decision_version.clone();
        assert_ne!(away_and_back_version, selected_version);
        let away_and_back = persistence
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Undecided),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: selected_version.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(
            away_and_back.results,
            vec![CheckedPhotoDecisionItemResult {
                photo_id: ids[0].clone(),
                outcome: CheckedPhotoDecisionOutcome::Conflict {
                    current: PhotoDecisionSnapshot {
                        selection_state: SelectionState::Undecided,
                        rating: 0,
                        decision_version: away_and_back_version.clone(),
                    },
                },
            }]
        );

        // A mixed batch keeps request order, reports one outcome and exact
        // counts per Photo, and leaves no-op versions untouched. ids[1]
        // already holds the target Rating; ids[2] carries a stale version
        // because an intervening Web write advanced it.
        let photo_two_initial = read_photo(ids[1].clone()).await;
        persistence
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[1].clone(),
                    expected_version: photo_two_initial.decision_version.clone(),
                }],
            })
            .await
            .unwrap();
        let photo_two = read_photo(ids[1].clone()).await;
        assert_eq!(photo_two.rating, 4);
        let photo_three_stale = read_photo(ids[2].clone()).await;
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[2].clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Selected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
        let photo_three = read_photo(ids[2].clone()).await;
        assert_ne!(
            photo_three.decision_version,
            photo_three_stale.decision_version
        );
        let photo_one = read_photo(ids[0].clone()).await;
        let mixed = persistence
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                photos: vec![
                    CheckedPhotoDecisionItem {
                        photo_id: ids[0].clone(),
                        expected_version: photo_one.decision_version.clone(),
                    },
                    CheckedPhotoDecisionItem {
                        photo_id: ids[2].clone(),
                        expected_version: photo_three_stale.decision_version.clone(),
                    },
                    CheckedPhotoDecisionItem {
                        photo_id: "missing-photo".to_owned(),
                        expected_version: photo_three.decision_version.clone(),
                    },
                    CheckedPhotoDecisionItem {
                        photo_id: ids[1].clone(),
                        expected_version: photo_two.decision_version.clone(),
                    },
                ],
            })
            .await
            .unwrap();
        assert_eq!(mixed.counts.changed, 1);
        assert_eq!(mixed.counts.unchanged, 1);
        assert_eq!(mixed.counts.conflict, 1);
        assert_eq!(mixed.counts.missing, 1);
        assert_eq!(
            mixed
                .results
                .iter()
                .map(|result| result.photo_id.clone())
                .collect::<Vec<_>>(),
            vec![
                ids[0].clone(),
                ids[2].clone(),
                "missing-photo".to_owned(),
                ids[1].clone(),
            ]
        );
        assert!(matches!(
            mixed.results[0].outcome,
            CheckedPhotoDecisionOutcome::Changed { .. }
        ));
        assert_eq!(
            mixed.results[1].outcome,
            CheckedPhotoDecisionOutcome::Conflict {
                current: PhotoDecisionSnapshot {
                    selection_state: SelectionState::Selected,
                    rating: 0,
                    decision_version: photo_three.decision_version.clone(),
                },
            }
        );
        assert_eq!(
            mixed.results[2].outcome,
            CheckedPhotoDecisionOutcome::Missing
        );
        assert_eq!(
            mixed.results[3].outcome,
            CheckedPhotoDecisionOutcome::Unchanged {
                current: PhotoDecisionSnapshot {
                    selection_state: SelectionState::Undecided,
                    rating: 4,
                    decision_version: photo_two.decision_version.clone(),
                },
            }
        );
        // A no-op does not advance the version; the conflict left the stale
        // item's facts and version untouched.
        assert_eq!(
            read_photo(ids[1].clone()).await.decision_version,
            photo_two.decision_version
        );
        let after_three = read_photo(ids[2].clone()).await;
        assert_eq!(after_three.rating, 0);
        assert_eq!(after_three.selection_state, SelectionState::Selected);

        // Restart issues a fresh process epoch: the old token conflicts, and
        // a fresh read allows one explicit next write.
        persistence.shutdown().unwrap();
        let reopened_state =
            StateDirectory::open_or_create(&library, base.0.join("state")).unwrap();
        let reopened = Persistence::open(reopened_state, name, canonical_root).unwrap();
        let fresh = reopened
            .photo_receiver(&ids[0])
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_ne!(fresh.decision_version, photo_one.decision_version);
        assert_eq!(fresh.rating, 4);
        let expired = reopened
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(5),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: photo_one.decision_version.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(expired.counts.conflict, 1);
        let explicit = reopened
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(5),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: fresh.decision_version.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(explicit.counts.changed, 1);
        reopened.shutdown().unwrap();
    }

    #[tokio::test]
    async fn checked_photo_decision_refusals_write_nothing_and_change_one_field() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let read_photo = |photo_id: String| {
            let persistence = persistence.clone();
            async move {
                persistence
                    .photo_receiver(&photo_id)
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
            }
        };
        let first = read_photo(ids[0].clone()).await;
        let refusal = |mutation| persistence.mutate_photo_decision_checked(mutation);
        // Malformed batches are refused before any write.
        assert_eq!(
            refusal(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                photos: vec![],
            })
            .await,
            Err(PhotoDecisionWriteError::Invalid)
        );
        assert_eq!(
            refusal(CheckedPhotoDecisionMutation {
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Rating(4),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: first.decision_version.clone(),
                }],
            })
            .await,
            Err(PhotoDecisionWriteError::Invalid)
        );
        assert_eq!(
            refusal(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(MAXIMUM_PHOTO_RATING + 1),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: first.decision_version.clone(),
                }],
            })
            .await,
            Err(PhotoDecisionWriteError::Invalid)
        );
        assert_eq!(
            refusal(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                photos: vec![
                    CheckedPhotoDecisionItem {
                        photo_id: ids[0].clone(),
                        expected_version: first.decision_version.clone(),
                    },
                    CheckedPhotoDecisionItem {
                        photo_id: ids[0].clone(),
                        expected_version: first.decision_version.clone(),
                    },
                ],
            })
            .await,
            Err(PhotoDecisionWriteError::Invalid)
        );
        assert_eq!(
            refusal(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: String::new(),
                }],
            })
            .await,
            Err(PhotoDecisionWriteError::Invalid)
        );
        assert_eq!(
            refusal(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                photos: (0..=crate::PHOTO_STATE_BATCH_MAX)
                    .map(|index| CheckedPhotoDecisionItem {
                        photo_id: format!("photo-{index}"),
                        expected_version: first.decision_version.clone(),
                    })
                    .collect(),
            })
            .await,
            Err(PhotoDecisionWriteError::LimitExceeded {
                limit: crate::PHOTO_STATE_BATCH_MAX,
                actual: crate::PHOTO_STATE_BATCH_MAX + 1,
            })
        );
        let untouched = read_photo(ids[0].clone()).await;
        assert_eq!(untouched.selection_state, first.selection_state);
        assert_eq!(untouched.rating, first.rating);
        assert_eq!(untouched.decision_version, first.decision_version);

        // A checked decision write changes neither the other decision field
        // nor an Album's saved browsing position or version.
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Resume".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[0].clone()],
            })
            .await
            .unwrap();
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_id.clone(),
                photo_id: ids[0].clone(),
            })
            .await
            .unwrap();
        let before = persistence
            .album_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(before.has_saved_position);
        let changed = persistence
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(3),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: untouched.decision_version.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(changed.counts.changed, 1);
        let after_photo = read_photo(ids[0].clone()).await;
        assert_eq!(after_photo.rating, 3);
        assert_eq!(after_photo.selection_state, SelectionState::Undecided);
        let after_album = persistence
            .album_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(after_album.has_saved_position);
        assert_eq!(after_album.album_version, before.album_version);
        assert_eq!(after_album.photo_count, before.photo_count);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn checked_photo_decision_batch_rolls_back_siblings_and_preserves_versions() {
        let (_base, library, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let escaped_id = ids[1].replace('\'', "''");
        Connection::open(&path)
            .unwrap()
            .execute_batch(&format!(
                "CREATE TRIGGER fail_checked_second BEFORE UPDATE OF selection_state ON photos\n                 WHEN OLD.id = '{escaped_id}'\n                 BEGIN SELECT RAISE(ABORT, 'forced checked failure'); END;"
            ))
            .unwrap();
        let read_photo = |photo_id: String| {
            let persistence = persistence.clone();
            async move {
                persistence
                    .photo_receiver(&photo_id)
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
            }
        };
        let first = read_photo(ids[0].clone()).await;
        let second = read_photo(ids[1].clone()).await;
        assert_eq!(
            persistence
                .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                    field: PhotoStateField::SelectionState,
                    value: PhotoStateValue::Selection(SelectionState::Selected),
                    photos: vec![
                        CheckedPhotoDecisionItem {
                            photo_id: ids[0].clone(),
                            expected_version: first.decision_version.clone(),
                        },
                        CheckedPhotoDecisionItem {
                            photo_id: ids[1].clone(),
                            expected_version: second.decision_version.clone(),
                        },
                    ],
                })
                .await,
            Err(PhotoDecisionWriteError::Persistence)
        );
        // Both siblings are unchanged and every version survives the
        // rollback; the next explicit write still uses the observed version.
        let after_first = read_photo(ids[0].clone()).await;
        let after_second = read_photo(ids[1].clone()).await;
        assert_eq!(after_first.selection_state, SelectionState::Undecided);
        assert_eq!(after_second.selection_state, SelectionState::Undecided);
        assert_eq!(after_first.decision_version, first.decision_version);
        assert_eq!(after_second.decision_version, second.decision_version);
        let recovered = persistence
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Rejected),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: first.decision_version.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(recovered.counts.changed, 1);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn progress_state_cas_undo_and_atomic_progress_are_scoped_and_global() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let album_a = persistence
            .mutate_album(AlbumMutation::Create {
                name: "A".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        let album_b = persistence
            .mutate_album(AlbumMutation::Create {
                name: "B".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        for album_id in [&album_a, &album_b] {
            persistence
                .mutate_album(AlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: ids.clone(),
                })
                .await
                .unwrap();
        }
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_a.clone(),
                photo_id: ids[0].clone(),
            })
            .await
            .unwrap();
        let albums = persistence.list_albums().await.unwrap();
        assert_eq!(
            albums
                .iter()
                .find(|album| album.id == album_a)
                .unwrap()
                .last_reviewed_photo_id
                .as_deref(),
            Some(ids[0].as_str())
        );
        let result = persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[0].clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Selected),
                expected_current: Some(PhotoStateValue::Selection(SelectionState::Undecided)),
                album_id: Some(album_b.clone()),
            })
            .await
            .unwrap();
        assert_eq!(
            result.undo.prior_value,
            PhotoStateValue::Selection(SelectionState::Undecided)
        );
        let albums = persistence.list_albums().await.unwrap();
        let current_a = albums.iter().find(|album| album.id == album_a).unwrap();
        let current_b = albums.iter().find(|album| album.id == album_b).unwrap();
        assert_eq!(
            current_a.members[0].selection_state,
            SelectionState::Selected
        );
        assert_eq!(
            current_b.members[0].selection_state,
            SelectionState::Selected
        );
        assert_eq!(
            current_b.last_reviewed_photo_id.as_deref(),
            Some(ids[0].as_str())
        );
        assert_eq!(
            persistence
                .mutate_photo_state(PhotoStateMutation {
                    photo_id: ids[0].clone(),
                    field: PhotoStateField::SelectionState,
                    value: result.undo.prior_value,
                    expected_current: Some(PhotoStateValue::Selection(SelectionState::Undecided)),
                    album_id: None,
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[0].clone(),
                field: PhotoStateField::SelectionState,
                value: result.undo.prior_value,
                expected_current: Some(result.undo.expected_current),
                album_id: None,
            })
            .await
            .unwrap();
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[1].clone(),
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(5),
                expected_current: Some(PhotoStateValue::Rating(0)),
                album_id: Some(album_a.clone()),
            })
            .await
            .unwrap();
        let albums = persistence.list_albums().await.unwrap();
        let current_a = albums.iter().find(|album| album.id == album_a).unwrap();
        let current_b = albums.iter().find(|album| album.id == album_b).unwrap();
        assert_eq!(current_a.members[1].rating, 5);
        assert_eq!(current_b.members[1].rating, 5);
        assert_eq!(
            current_a.last_reviewed_photo_id.as_deref(),
            Some(ids[1].as_str())
        );
        persistence
            .mutate_album(AlbumMutation::RemoveMember {
                album_id: album_a.clone(),
                photo_id: ids[1].clone(),
            })
            .await
            .unwrap();
        let albums = persistence.list_albums().await.unwrap();
        assert_eq!(
            albums
                .iter()
                .find(|album| album.id == album_a)
                .unwrap()
                .last_reviewed_photo_id,
            None
        );
        persistence.shutdown().unwrap();
    }
}
