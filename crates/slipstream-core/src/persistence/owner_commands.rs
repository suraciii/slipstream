//! Command dispatch for the persistence owner loop.
//!
//! One responsibility: execute a single queued [`Command`] against the
//! owner-thread state, in loop order, so reply ordering, version
//! advancement, and transaction semantics stay exactly as serialized by
//! `owner_main`.

#[cfg(test)]
use super:suraciii::PersistenceError;
use super:suraciii::{
    Command, MutationError, MutationVersions, permanently_deleted_original_ids, write_transaction,
};
use super::{
    DatabaseName, StateDirectory, albums, decisions, development_proxy, edit_recipe, export,
    queries, removal, scan, xmp,
};
use crate::AlbumMembershipMutation;
use rusqlite::Connection;

/// Executes one owner-loop `command` in loop order. The caller owns the
/// connection and versions across commands; `sequence` is the loop counter
/// at the moment this command was accepted.
pub(super) fn dispatch(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    versions: &mut MutationVersions,
    sequence: u64,
    command: Command,
) {
    match command {
        Command::Metadata(work) => work(connection),
        Command::Probe(reply) => {
            let _ = reply.send(Ok(sequence));
        }
        Command::Snapshot(reply) => {
            let result = scan::snapshot(connection);
            let _ = reply.send(result);
        }
        Command::ApplyScan {
            discovered,
            errors,
            recovery,
            failure_after_first,
            reply,
        } => {
            let result = scan::apply_scan(
                state,
                database_name,
                connection,
                &discovered,
                &errors,
                &recovery,
                failure_after_first,
            );
            let _ = reply.send(result);
        }
        Command::RecoveryFacts {
            original_ids,
            reply,
        } => {
            let result = scan::recovery_facts(connection, &original_ids);
            let _ = reply.send(result);
        }
        Command::NextFingerprintTarget(reply) => {
            let result = scan::next_fingerprint_target(connection);
            let _ = reply.send(result);
        }
        Command::StoreFingerprint(fingerprint, reply) => {
            let result = scan::store_fingerprint(state, database_name, connection, fingerprint);
            let _ = reply.send(result);
        }
        Command::FingerprintCounts(reply) => {
            let result = scan::fingerprint_counts(connection);
            let _ = reply.send(result);
        }
        Command::RecoverySurvey(reply) => {
            let result = scan::recovery_survey(connection);
            let _ = reply.send(result);
        }
        Command::RecoveryRecords {
            original_ids,
            reply,
        } => {
            let result = scan::recovery_records(connection, &original_ids);
            let _ = reply.send(result);
        }
        Command::ApplyRelocations {
            root,
            relocations,
            reply,
        } => {
            let result = scan::apply_manual_relocations(
                state,
                database_name,
                connection,
                &root,
                &relocations,
            );
            let _ = reply.send(result);
        }
        Command::Preview(preview, reply) => {
            let result = scan::seed_preview(state, database_name, connection, preview);
            let _ = reply.send(result);
        }
        Command::ListAlbums(reply) => {
            let _ = reply.send(albums::list_albums(connection));
        }
        Command::ListAlbumSummaries(reply) => {
            let _ = reply.send(albums::list_album_summaries(connection, versions));
        }
        Command::ReadAlbum { album_id, reply } => {
            let _ = reply.send(albums::read_album(connection, versions, &album_id));
        }
        Command::ReadAlbums { album_ids, reply } => {
            let result = album_ids
                .iter()
                .map(|album_id| albums::read_album(connection, versions, album_id))
                .collect();
            let _ = reply.send(result);
        }
        Command::CreateAlbumQuery {
            filter,
            maximum_results,
            reply,
        } => {
            let _ = reply.send(albums::create_album_query(
                connection,
                filter,
                maximum_results,
            ));
        }
        Command::ReadPhoto { photo_id, reply } => {
            let _ = reply.send(queries::read_photo(connection, versions, &photo_id));
        }
        Command::ReadEditRecipeSurface { photo_id, reply } => {
            // One serialized owner operation: the Photo facts and the
            // recipe read below cannot straddle a scan publication, so
            // every guard derives from a single published state.
            let _ = reply.send(
                queries::read_photo(connection, versions, &photo_id).and_then(|photo| {
                    photo
                        .map(|photo| {
                            edit_recipe::read_edit_recipe(connection, &photo_id)
                                .map(|read| read.map(|read| (photo, read)))
                        })
                        .transpose()
                        .map(|surface| surface.flatten())
                }),
            );
        }
        Command::ReadEditRecipe { photo_id, reply } => {
            let _ = reply.send(edit_recipe::read_edit_recipe(connection, &photo_id));
        }
        Command::ReadDevelopmentProxy { photo_id, reply } => {
            let _ = reply.send(development_proxy::read_development_proxy(
                connection, &photo_id,
            ));
        }
        Command::RecordDevelopmentProxy(record, reply) => {
            let result = development_proxy::record_development_proxy(
                state,
                database_name,
                connection,
                record,
            );
            let _ = reply.send(result);
        }
        Command::RemoveDevelopmentProxy { photo_id, reply } => {
            let _ = reply.send(development_proxy::remove_development_proxy(
                state,
                database_name,
                connection,
                &photo_id,
            ));
        }
        Command::AllDevelopmentProxies(reply) => {
            let _ = reply.send(development_proxy::all_development_proxies(connection));
        }
        Command::SaveEditRecipe(mutation, reply) => {
            let result = edit_recipe::save_edit_recipe(state, database_name, connection, mutation);
            let _ = reply.send(result);
        }
        Command::RebindEditRecipe(mutation, reply) => {
            let result =
                edit_recipe::rebind_edit_recipe(state, database_name, connection, mutation);
            let _ = reply.send(result);
        }
        Command::ReadPhotos {
            photo_ids,
            projection,
            reply,
        } => {
            let result = photo_ids
                .iter()
                .map(|photo_id| {
                    queries::read_projected_photo(connection, versions, &projection, photo_id)
                })
                .collect();
            let _ = reply.send(result);
        }
        Command::CreatePhotoQuery {
            query,
            projection,
            maximum_results,
            reply,
        } => {
            let _ = reply.send(queries::create_photo_query(
                connection,
                query,
                &projection,
                maximum_results,
            ));
        }
        Command::PhotoAlbums { photo_id, reply } => {
            let _ = reply.send(albums::photo_albums(connection, &photo_id));
        }
        Command::AlbumBrowseTarget { album_id, reply } => {
            let _ = reply.send(albums::album_browse_target(connection, &album_id));
        }
        Command::MutateAlbum(mutation, reply) => {
            let plan = albums::album_version_plan(connection, &mutation);
            let result = match plan {
                Ok(plan) if !plan.advance || versions.can_advance_album(&plan.album_id) => {
                    let result = albums::mutate_album(state, database_name, connection, mutation);
                    if result.is_ok() {
                        if plan.deleted {
                            versions.album.remove(&plan.album_id);
                        } else if plan.advance {
                            let _ = versions.advance_album(&plan.album_id);
                        }
                    }
                    result
                }
                Ok(_) => Err(MutationError::Persistence),
                Err(error) => Err(error),
            };
            let _ = reply.send(result);
        }
        Command::MutateAlbumMembership(mutation, reply) => {
            let album_id = match &mutation {
                AlbumMembershipMutation::Add { album_id, .. }
                | AlbumMembershipMutation::RemoveAdded { album_id, .. } => album_id,
            };
            let result = if versions.can_advance_album(album_id) {
                albums::mutate_album_membership(state, database_name, connection, mutation)
            } else {
                Err(MutationError::Persistence)
            };
            if let Ok(result) = &result
                && (!result.added_photo_ids.is_empty() || !result.removed_photo_ids.is_empty())
            {
                let _ = versions.advance_album(&result.album_id);
            }
            let _ = reply.send(result);
        }
        Command::CreateAlbum(name, reply) => {
            let result =
                albums::create_album_checked(state, database_name, connection, versions, name);
            let _ = reply.send(result);
        }
        Command::MutateAlbumChecked(mutation, reply) => {
            let result =
                albums::mutate_album_checked(state, database_name, connection, versions, mutation);
            let _ = reply.send(result);
        }
        Command::MutatePhotoDecisionChecked(mutation, reply) => {
            let result = decisions::mutate_photo_decision_checked(
                state,
                database_name,
                connection,
                versions,
                mutation,
            );
            let _ = reply.send(result);
        }
        Command::MutatePhotoState(mutation, reply) => {
            let photo_id = mutation.photo_id.clone();
            let value = mutation.value;
            let result = if versions.can_advance_photo(&photo_id) {
                decisions::mutate_photo_state(state, database_name, connection, mutation)
            } else {
                Err(MutationError::Persistence)
            };
            if result
                .as_ref()
                .is_ok_and(|result| result.undo.prior_value != value)
            {
                let _ = versions.advance_photo(&photo_id);
            }
            let _ = reply.send(result);
        }
        Command::MutatePhotoStateBatch(mutation, reply) => {
            let value = mutation.value;
            let can_advance = mutation
                .photos
                .iter()
                .all(|photo| versions.can_advance_photo(&photo.photo_id));
            let result = if can_advance {
                decisions::mutate_photo_state_batch(state, database_name, connection, mutation)
            } else {
                Err(MutationError::Persistence)
            };
            if let Ok(result) = &result {
                for applied in &result.applied {
                    if applied.prior_value != value {
                        let _ = versions.advance_photo(&applied.photo_id);
                    }
                }
            }
            let _ = reply.send(result);
        }
        Command::RemovePhotos(request, reply) => {
            let result =
                removal::remove_photos(state, database_name, connection, versions, request);
            if let Ok(result) = &result {
                for photo_id in &result.newly_removed {
                    let _ = versions.advance_photo(photo_id);
                }
            }
            let _ = reply.send(result);
        }
        Command::ReadPhotoRemovalOperation {
            operation_id,
            reply,
        } => {
            let result = removal::read_photo_removal_operation(connection, &operation_id).and_then(
                |receipt| {
                    receipt
                        .map(|receipt| {
                            removal::photo_removal_result_from_receipt(&operation_id, receipt)
                        })
                        .transpose()
                },
            );
            let _ = reply.send(result);
        }
        Command::RestorePhotosExplicit(mutation, reply) => {
            let result =
                removal::restore_photos_explicit(state, database_name, connection, mutation);
            if let Ok(result) = &result {
                for photo_id in &result.restored {
                    let _ = versions.advance_photo(photo_id);
                }
            }
            let _ = reply.send(result);
        }
        Command::ReadPhotoRestoreOperation {
            operation_id,
            reply,
        } => {
            let result = removal::read_photo_restore_operation(connection, &operation_id).and_then(
                |receipt| {
                    receipt
                        .map(|receipt| {
                            removal::photo_restore_result_from_receipt(&operation_id, receipt)
                        })
                        .transpose()
                },
            );
            let _ = reply.send(result);
        }
        Command::RestorePhotos(restoration, reply) => {
            let result = removal::restore_photos(state, database_name, connection, restoration);
            if let Ok(result) = &result {
                for photo_id in &result.restored {
                    let _ = versions.advance_photo(photo_id);
                }
            }
            let _ = reply.send(result);
        }
        Command::RemovedPhotos {
            start,
            limit,
            reply,
        } => {
            let _ = reply.send(removal::removed_photos(connection, start, limit));
        }
        Command::WriteProbe(reply) => {
            let result = write_transaction(state, database_name, connection, |_| Ok(()));
            let _ = reply.send(result);
        }
        Command::SubmitExport(submission, reply) => {
            let result = export::submit_export(state, database_name, connection, submission);
            let _ = reply.send(result);
        }
        Command::ReadExport { export_id, reply } => {
            let _ = reply.send(export::export_record(connection, &export_id));
        }
        Command::TrashCandidates { selection, reply } => {
            let _ = reply.send(removal::trash_candidates(connection, selection));
        }
        Command::PreparePermanentDeletion {
            operation_id,
            targets,
            reply,
        } => {
            let result = removal::prepare_permanent_deletion(
                state,
                database_name,
                connection,
                operation_id,
                targets,
            );
            let _ = reply.send(result);
        }
        Command::PermanentDeletionWork {
            operation_id,
            retry_unresolved,
            reply,
        } => {
            let _ = reply.send(removal::permanent_deletion_work(
                connection,
                &operation_id,
                retry_unresolved,
            ));
        }
        Command::MarkPermanentDeletionDeleting {
            operation_id,
            photo_id,
            reply,
        } => {
            let result = removal::mark_permanent_deletion_deleting(
                state,
                database_name,
                connection,
                &operation_id,
                &photo_id,
            );
            let _ = reply.send(result);
        }
        Command::PermanentlyDeletedOriginalIds(reply) => {
            let _ = reply.send(permanently_deleted_original_ids(connection));
        }
        Command::SettlePermanentDeletion {
            operation_id,
            photo_id,
            state: item_state,
            message,
            reply,
        } => {
            let result = removal::settle_permanent_deletion(
                state,
                database_name,
                connection,
                &operation_id,
                &photo_id,
                item_state,
                message,
            );
            let _ = reply.send(result);
        }
        Command::ReadPermanentDeletion {
            operation_id,
            reply,
        } => {
            let _ = reply.send(removal::read_permanent_deletion(connection, &operation_id));
        }
        Command::ListPhotoExports { photo_id, reply } => {
            let _ = reply.send(export::list_photo_exports(connection, &photo_id));
        }
        Command::CancelExport { export_id, reply } => {
            let result = export::cancel_export(state, database_name, connection, &export_id);
            let _ = reply.send(result);
        }
        Command::SettleExport {
            export_id,
            settlement,
            reply,
        } => {
            let result =
                export::settle_export(state, database_name, connection, &export_id, settlement);
            let _ = reply.send(result);
        }
        Command::BeginExportAttempt {
            export_id,
            attempt,
            reply,
        } => {
            let result =
                export::begin_export_attempt(state, database_name, connection, &export_id, attempt);
            let _ = reply.send(result);
        }
        Command::RecordExportSource {
            export_id,
            size,
            sha256,
            reply,
        } => {
            let result = export::record_export_source(
                state,
                database_name,
                connection,
                &export_id,
                size,
                &sha256,
            );
            let _ = reply.send(result);
        }
        Command::RetryExport {
            export_id,
            request_id,
            expected_bundle_id,
            allowance,
            reply,
        } => {
            let result = export::retry_export(
                state,
                database_name,
                connection,
                &export_id,
                &request_id,
                &expected_bundle_id,
                allowance,
            );
            let _ = reply.send(result);
        }
        Command::ResolveExportSubmission {
            photo_id,
            request_id,
            payload_digest,
            reply,
        } => {
            let _ = reply.send(Ok(export::resolve_export_submission(
                connection,
                &photo_id,
                &request_id,
                &payload_digest,
            )));
        }
        Command::CreateXmp {
            photo_id,
            request_id,
            expected_recipe,
            expected_source,
            now,
            reply,
        } => {
            let _ = reply.send(xmp::create(
                connection,
                &photo_id,
                &request_id,
                &expected_recipe,
                &expected_source,
                now,
            ));
        }
        Command::ReadXmp { export_id, reply } => {
            let _ = reply.send(xmp::read(connection, &export_id));
        }
        Command::ListPhotoXmp { photo_id, reply } => {
            let _ = reply.send(xmp::list(connection, &photo_id));
        }
        Command::ClaimExportPublication {
            export_id,
            incarnation,
            sequence,
            reply,
        } => {
            let _ = reply.send(export::claim_export_publication(
                connection,
                &export_id,
                &incarnation,
                sequence,
            ));
        }
        Command::ExportPublicationClaim { export_id, reply } => {
            let _ = reply.send(Ok(export::export_publication_claim(connection, &export_id)));
        }
        Command::RenewExportLease {
            lease_id,
            now,
            reply,
        } => {
            let _ = reply.send(Ok(export::renew_export_lease(connection, &lease_id, now)));
        }
        Command::SweepExportExpiry { now, reply } => {
            let result = export::sweep_export_expiry(state, database_name, connection, now);
            let _ = reply.send(result);
        }
        Command::UnfinishedExports(reply) => {
            let _ = reply.send(export::unfinished_exports(connection));
        }
        Command::AcquireExportLease {
            export_id,
            now,
            reply,
        } => {
            let result =
                export::acquire_export_lease(state, database_name, connection, &export_id, now);
            let _ = reply.send(result);
        }
        Command::ReleaseExportLease { lease_id, reply } => {
            let result = export::release_export_lease(state, database_name, connection, &lease_id);
            let _ = reply.send(result);
        }
        #[cfg(test)]
        Command::Configuration(reply) => {
            let result = (|| {
                let journal = connection
                    .pragma_query_value(None, "journal_mode", |row| row.get(0))
                    .map_err(|_| PersistenceError::Storage)?;
                let foreign_keys = connection
                    .pragma_query_value(None, "foreign_keys", |row| row.get(0))
                    .map_err(|_| PersistenceError::Storage)?;
                Ok((journal, foreign_keys))
            })();
            let _ = reply.send(result);
        }
        #[cfg(test)]
        Command::Block {
            entered,
            release,
            reply,
        } => {
            let _ = entered.send(());
            let result = release.recv().map_err(|_| PersistenceError::OwnerStopped);
            let _ = reply.send(result);
        }
    }
}