use super::*;

impl Application {
    /// The survey the recovery review evaluates against, enriched with the
    /// authoritative workspace references before any mapping is judged. The
    /// Library survey covers Album, Edit Recipe, and Export references; the
    /// workspace registry and each active workspace's own database add every
    /// Photo a workspace still binds or retains. An unreadable workspace
    /// reference is an error, never an absent one.
    async fn recovery_survey_with_references(
        &self,
    ) -> Result<slipstream_core::RecoverySurvey, ServerError> {
        let mut survey = self.library.recovery_survey().await?;
        let workspaces = Arc::clone(&self.workspaces);
        let retained = tokio::task::spawn_blocking(move || workspaces.retained_photo_references())
            .await
            .map_err(|error| ServerError::Join(error.to_string()))?
            .map_err(|error| ServerError::Export(error.to_string()))?;
        survey.referenced_photo_ids.extend(retained);
        Ok(survey)
    }

    /// One consistent read of the active unavailable Photos for the bounded
    /// review entry. The review retains this membership; later pages read
    /// current facts of it instead of re-evaluating a different set.
    pub(crate) async fn recovery_unavailable_items(
        &self,
    ) -> Result<Vec<RecoveryItemWire>, ServerError> {
        let survey = self.library.recovery_survey().await?;
        Ok(survey
            .unavailable
            .iter()
            .map(|record| RecoveryItemWire::from_record(record, "unavailable"))
            .collect())
    }

    /// Current facts of retained review memberships, in the requested order.
    /// `None` means that record no longer exists.
    pub(crate) async fn recovery_identities(
        &self,
        original_ids: Vec<String>,
    ) -> Result<Vec<Option<slipstream_core::RecoveryRecord>>, ServerError> {
        Ok(self.library.recovery_records(original_ids).await?)
    }

    /// Evaluates one Folder-prefix proposal over every unavailable Original
    /// under the reviewed prefix, reading candidates through confined
    /// descriptors. A scope larger than the advertised bound is refused
    /// before any content is evaluated.
    pub(crate) async fn recovery_propose_batch(
        &self,
        old_prefix: &str,
        new_prefix: &str,
    ) -> Result<Vec<RecoveryMappingWire>, ServerError> {
        let survey = self.recovery_survey_with_references().await?;
        let evaluated = slipstream_core::count_prefix_scope(&survey, old_prefix)
            .map_err(|_| ServerError::FolderInvalid)?;
        if evaluated > MAXIMUM_RECOVERY_MAPPINGS {
            return Err(ServerError::RecoveryScope { evaluated });
        }
        let snapshot = self.library.snapshot().await?;
        let old = old_prefix.to_owned();
        let new = new_prefix.to_owned();
        let root = self.library_root.clone();
        let proposals = tokio::task::spawn_blocking(move || {
            let root =
                slipstream_core::LibraryRoot::open(root).map_err(|_| ServerError::StorageLayout)?;
            let native_work = slipstream_core::NativeWorkBudget::new();
            slipstream_core::plan_manual_relocations(
                &root,
                &native_work,
                &survey,
                &snapshot,
                &old,
                &new,
            )
            .map_err(|_| ServerError::FolderInvalid)
        })
        .await
        .map_err(|error| ServerError::Join(error.to_string()))??;
        Ok(proposals
            .iter()
            .map(RecoveryMappingWire::from_proposal)
            .collect())
    }

    /// Evaluates one mapping for a single unavailable Original, for renamed
    /// or split files a folder-prefix batch cannot express.
    pub(crate) async fn recovery_propose_single(
        &self,
        original_id: &str,
        new_location: &str,
    ) -> Result<RecoveryMappingWire, ServerError> {
        let survey = self.recovery_survey_with_references().await?;
        let snapshot = self.library.snapshot().await?;
        if !survey
            .unavailable
            .iter()
            .any(|record| record.original_id == original_id)
        {
            return Err(ServerError::PhotoNotFound);
        }
        let original_id = original_id.to_owned();
        let location = new_location.to_owned();
        let root = self.library_root.clone();
        let proposal = tokio::task::spawn_blocking(move || {
            let root =
                slipstream_core::LibraryRoot::open(root).map_err(|_| ServerError::StorageLayout)?;
            let native_work = slipstream_core::NativeWorkBudget::new();
            slipstream_core::plan_single_relocation(
                &root,
                &native_work,
                &survey,
                &snapshot,
                &original_id,
                &location,
            )
            .map_err(|_| ServerError::FolderInvalid)
        })
        .await
        .map_err(|error| ServerError::Join(error.to_string()))??;
        Ok(RecoveryMappingWire::from_proposal(&proposal))
    }

    /// Commits one reviewed manual relocation batch. Every submitted mapping
    /// is recomputed from current state and compared with the reviewed
    /// identity the Photographer confirmed; the whole batch commits atomically
    /// or is refused with one reason per mapping and no partial association.
    pub(crate) async fn recovery_apply(
        self: &Arc<Self>,
        items: Vec<RecoveryApplyItem>,
    ) -> Result<RecoveryApplyResponseWire, RecoveryApplyError> {
        if items.is_empty() || items.len() > MAXIMUM_RECOVERY_APPLY {
            return Err(RecoveryApplyError::Invalid);
        }
        let survey = self
            .recovery_survey_with_references()
            .await
            .map_err(RecoveryApplyError::Server)?;
        let snapshot = self
            .library
            .snapshot()
            .await
            .map_err(|error| RecoveryApplyError::Server(error.into()))?;
        let submitted_mappings = items.len() as u64;
        // The identities the request submitted, carried into any
        // unknown-outcome report so the caller can reconcile exactly the
        // mappings that may have committed.
        let submitted_identities = items
            .iter()
            .map(|item| RecoverySubmittedMappingWire {
                original_id: item.original_id.clone(),
                new_location: item.new_location.clone(),
                mapping_id: item.mapping_id.clone(),
            })
            .collect::<Vec<_>>();
        let root = self.library_root.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let root =
                slipstream_core::LibraryRoot::open(root).map_err(|_| ServerError::StorageLayout)?;
            let native_work = slipstream_core::NativeWorkBudget::new();
            // Every submitted Original vacates its remembered destination in
            // this batch, so it is neither an occupant nor a conflict.
            let relocating = items
                .iter()
                .map(|item| item.original_id.clone())
                .collect::<std::collections::HashSet<_>>();
            let mut claimed = std::collections::HashSet::new();
            let mut rejections = Vec::new();
            let mut relocations = Vec::new();
            let mut applied = Vec::new();
            for item in items {
                let mut reject = |reason: &'static str| {
                    rejections.push(RecoveryRejectionWire {
                        original_id: item.original_id.clone(),
                        reason,
                    });
                };
                let proposal = match slipstream_core::evaluate_relocation(
                    &root,
                    &native_work,
                    &survey,
                    &snapshot,
                    &item.original_id,
                    &item.new_location,
                    slipstream_core::RelocationSet {
                        relocating: &relocating,
                        claimed_destinations: &claimed,
                    },
                ) {
                    Ok(proposal) => proposal,
                    Err(_) => {
                        reject("stale");
                        continue;
                    }
                };
                if let Some(block) = proposal.blocked {
                    reject(block.code());
                    continue;
                }
                if proposal.mapping_id != item.mapping_id {
                    reject("reviewed-stale");
                    continue;
                }
                let retire = match &proposal.outcome {
                    slipstream_core::ManualOutcome::Occupied {
                        retire: Some(candidate),
                    } => match item.retire_photo_id.as_deref() {
                        Some(photo_id) if photo_id == candidate.photo_id => Some(candidate.clone()),
                        Some(_) => {
                            reject("retire-mismatch");
                            continue;
                        }
                        None => {
                            reject("retire-unconfirmed");
                            continue;
                        }
                    },
                    _ if item.retire_photo_id.is_some() => {
                        reject("retire-mismatch");
                        continue;
                    }
                    _ => None,
                };
                if !proposal.verified && !item.confirm_unverified_content {
                    reject("content-unconfirmed");
                    continue;
                }
                let Some(facts) = proposal.destination_facts else {
                    reject("missing");
                    continue;
                };
                claimed.insert(item.new_location.clone());
                relocations.push(slipstream_core::RequestedRelocation {
                    mapping_id: proposal.mapping_id.clone(),
                    original_id: proposal.original_id.clone(),
                    from_location: proposal.from_location.clone(),
                    to_location: proposal.to_location.clone(),
                    fingerprint: survey
                        .unavailable
                        .iter()
                        .find(|record| record.original_id == proposal.original_id)
                        .and_then(|record| record.fingerprint.clone()),
                    facts,
                    retire_photo_id: retire.as_ref().map(|candidate| candidate.photo_id.clone()),
                });
                applied.push(RecoveryAppliedWire {
                    original_id: proposal.original_id,
                    photo_id: proposal.photo_id.clone(),
                    from_location: proposal.from_location,
                    to_location: proposal.to_location,
                    web_url: photo_web_path(&proposal.photo_id),
                    retired: retire.map(|candidate| RetireCandidateWire {
                        photo_id: candidate.photo_id.to_owned(),
                        original_id: candidate.original_id.to_owned(),
                        location: candidate.location.to_owned(),
                    }),
                });
            }
            Ok::<_, ServerError>((rejections, relocations, applied))
        })
        .await
        .map_err(|error| RecoveryApplyError::Server(ServerError::Join(error.to_string())))?
        .map_err(RecoveryApplyError::Server)?;
        let (rejections, relocations, applied) = worker;
        if !rejections.is_empty() {
            return Err(RecoveryApplyError::Rejected {
                message: "Recovery batch rejected without changes",
                rejections,
                refused_mappings: submitted_mappings,
            });
        }
        // The workspace boundary is rechecked immediately before the reviewed
        // batch is committed: a workspace created, or a retained reference
        // added, while the review sat open must refuse the whole batch rather
        // than retire a Photo a workspace now holds.
        let retire_photo_ids = relocations
            .iter()
            .filter_map(|relocation| relocation.retire_photo_id.clone())
            .collect::<Vec<_>>();
        if !retire_photo_ids.is_empty() {
            let workspaces = Arc::clone(&self.workspaces);
            let retained = tokio::task::spawn_blocking(move || {
                Ok::<_, crate::workspace::WorkspaceError>(
                    workspaces
                        .retained_photo_references()?
                        .into_iter()
                        .collect::<std::collections::HashSet<_>>(),
                )
            })
            .await
            .map_err(|error| RecoveryApplyError::Server(ServerError::Join(error.to_string())))?
            .map_err(|error| RecoveryApplyError::Server(ServerError::Export(error.to_string())))?;
            if retire_photo_ids.iter().any(|id| retained.contains(id)) {
                return Err(RecoveryApplyError::Rejected {
                    message: "Recovery batch rejected without changes",
                    rejections: retire_photo_ids
                        .into_iter()
                        .filter(|id| retained.contains(id))
                        .map(|photo_id| RecoveryRejectionWire {
                            original_id: relocations
                                .iter()
                                .find(|relocation| {
                                    relocation.retire_photo_id.as_deref() == Some(photo_id.as_str())
                                })
                                .map(|relocation| relocation.original_id.clone())
                                .unwrap_or_default(),
                            reason: "destination-in-use",
                        })
                        .collect(),
                    refused_mappings: submitted_mappings,
                });
            }
        }
        let committed = self
            .library
            .apply_relocations(relocations)
            .await
            .map_err(|error| match error {
                LibraryError::Persistence(
                    slipstream_core::persistence::PersistenceError::InvalidRecoveryMapping {
                        original_id,
                        reason,
                    },
                ) => RecoveryApplyError::Rejected {
                    message: "Recovery batch conflicts with current Library state; rescan and review again",
                    rejections: vec![RecoveryRejectionWire {
                        original_id,
                        reason,
                    }],
                    refused_mappings: submitted_mappings,
                },
                LibraryError::Persistence(
                    slipstream_core::persistence::PersistenceError::InvalidRecovery,
                ) => RecoveryApplyError::Rejected {
                    message: "Recovery batch conflicts with current Library state; rescan and review again",
                    rejections: Vec::new(),
                    refused_mappings: submitted_mappings,
                },
                other => RecoveryApplyError::Server(other.into()),
            })?;
        // The committed counts become the recovery counts the scan status
        // reports, so the review notice stays truthful between scans even
        // when the publication that follows the commit fails.
        self.library
            .note_manual_recovery(committed.relocated_photos, committed.unavailable_photos);
        let warmup_requests = match self.shared.publish_fresh(&self.library).await {
            Ok(warmup_requests) => warmup_requests,
            Err(_) => {
                // The batch already committed, so a failed publication is
                // neither a refusal nor a confirmed storage failure: every
                // submitted mapping's outcome is unknown and the caller must
                // reconcile before submitting again.
                return Err(RecoveryApplyError::OutcomeUnknown {
                    mappings: submitted_identities,
                });
            }
        };
        self.schedule_review_warmup(warmup_requests);
        Ok(RecoveryApplyResponseWire {
            applied_mappings: applied.len() as u64,
            refused_mappings: 0,
            unavailable_photos: committed.unavailable_photos,
            mappings: applied,
        })
    }
}
