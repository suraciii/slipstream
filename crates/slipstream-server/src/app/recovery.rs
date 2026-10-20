use super::*;

pub(crate) struct RecoveryLifecycle {
    pub closing: AtomicBool,
    pub active: AtomicUsize,
    pub idle: Notify,
}

struct RecoveryActivityGuard {
    application: Arc<Application>,
}

impl Drop for RecoveryActivityGuard {
    fn drop(&mut self) {
        self.application.recovery_finished();
    }
}

fn recovery_rejection_reason(reason: &'static str) -> &'static str {
    match reason {
        "colliding"
        | "content-mismatch"
        | "content-unconfirmed"
        | "destination-in-use"
        | "destination-removed"
        | "kind-mismatch"
        | "missing"
        | "occupied"
        | "retire-mismatch"
        | "retire-unconfirmed"
        | "reviewed-stale"
        | "stale"
        | "unreadable" => reason,
        // The HTTP boundary rejects malformed Locations before this point.
        // Keep direct callers inside the documented recovery vocabulary too.
        _ => "stale",
    }
}

fn complete_recovery_rejections(
    items: &[RecoveryApplyItem],
    partial: impl IntoIterator<Item = RecoveryRejectionWire>,
) -> Vec<RecoveryRejectionWire> {
    let mut reasons = partial
        .into_iter()
        .map(|rejection| {
            (
                rejection.original_id,
                recovery_rejection_reason(rejection.reason),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    items
        .iter()
        .map(|item| RecoveryRejectionWire {
            original_id: item.original_id.clone(),
            reason: reasons.remove(&item.original_id).unwrap_or("stale"),
        })
        .collect()
}

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
            .map_err(|error| {
                if error.code == "workspace_busy" {
                    ServerError::WorkspaceBusy
                } else {
                    ServerError::Export(error.to_string())
                }
            })?;
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

    /// Commits one reviewed manual relocation batch in an application-owned
    /// task. Dropping the HTTP waiter must not release a cross-store guard or
    /// abandon publication after the Library command has been accepted.
    pub(crate) async fn recovery_apply(
        self: &Arc<Self>,
        items: Vec<RecoveryApplyItem>,
    ) -> Result<RecoveryApplyResponseWire, RecoveryApplyError> {
        if self.recovery.closing.load(Ordering::Acquire) {
            return Err(RecoveryApplyError::Server(ServerError::Join(
                "Recovery is shutting down; try again after restart.".to_owned(),
            )));
        }
        self.recovery.active.fetch_add(1, Ordering::AcqRel);
        let activity = RecoveryActivityGuard {
            application: Arc::clone(self),
        };
        if self.recovery.closing.load(Ordering::Acquire) {
            drop(activity);
            return Err(RecoveryApplyError::Server(ServerError::Join(
                "Recovery is shutting down; try again after restart.".to_owned(),
            )));
        }
        let application = Arc::clone(self);
        let (reply, receive) = oneshot::channel();
        tokio::spawn(async move {
            let _activity = activity;
            let result = application.recovery_apply_operation(items).await;
            let _ = reply.send(result);
        });
        receive
            .await
            .map_err(|error| RecoveryApplyError::Server(ServerError::Join(error.to_string())))?
    }

    fn recovery_finished(&self) {
        if self.recovery.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.recovery.idle.notify_waiters();
        }
    }

    pub(crate) async fn close_recovery(&self) {
        self.recovery.closing.store(true, Ordering::Release);
        let mut notified = std::pin::pin!(self.recovery.idle.notified());
        loop {
            notified.as_mut().enable();
            if self.recovery.active.load(Ordering::Acquire) == 0 {
                break;
            }
            notified.as_mut().await;
            notified.set(self.recovery.idle.notified());
        }
    }

    /// The operation-owned recovery implementation. Its caller is detached
    /// from the request above, so accepted work remains settled independently.
    async fn recovery_apply_operation(
        self: &Arc<Self>,
        items: Vec<RecoveryApplyItem>,
    ) -> Result<RecoveryApplyResponseWire, RecoveryApplyError> {
        if items.is_empty() || items.len() > MAXIMUM_RECOVERY_APPLY {
            return Err(RecoveryApplyError::Invalid);
        }
        let mut submitted_originals = std::collections::HashSet::with_capacity(items.len());
        if items
            .iter()
            .any(|item| !submitted_originals.insert(item.original_id.as_str()))
        {
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
        let review_items = items.clone();
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
                rejections: complete_recovery_rejections(&review_items, rejections),
                refused_mappings: submitted_mappings,
            });
        }
        // Retire-and-Bind crosses the Library database and the workspace
        // store. Keep the root exclusion inside a blocking operation that
        // synchronously waits for the persistence owner; a dropped HTTP
        // waiter cannot release it while the queued SQLite command commits.
        let retire_photo_ids = relocations
            .iter()
            .filter_map(|relocation| relocation.retire_photo_id.clone())
            .collect::<Vec<_>>();
        let commit_review_items = review_items.clone();
        let commit_workspaces = Arc::clone(&self.workspaces);
        let commit_library = Arc::clone(&self.library);
        let commit_retire_photo_ids = retire_photo_ids;
        let commit_relocations = relocations;
        let committed = tokio::task::spawn_blocking(move || {
            let _workspace_guard = if commit_retire_photo_ids.is_empty() {
                None
            } else {
                Some(commit_workspaces.recovery_guard().map_err(|error| {
                    RecoveryApplyError::Server(if error.code == "workspace_busy" {
                        ServerError::WorkspaceBusy
                    } else {
                        ServerError::Export(error.to_string())
                    })
                })?)
            };
            if !commit_retire_photo_ids.is_empty() {
                let retained = commit_workspaces
                    .retained_photo_references_under_recovery_guard()
                    .map_err(|error| {
                        if error.code == "workspace_busy" {
                            RecoveryApplyError::Server(ServerError::WorkspaceBusy)
                        } else {
                            RecoveryApplyError::Rejected {
                                message: "Recovery batch rejected without changes",
                                rejections: complete_recovery_rejections(
                                    &commit_review_items,
                                    commit_review_items.iter().map(|item| RecoveryRejectionWire {
                                        original_id: item.original_id.clone(),
                                        reason: "unreadable",
                                    }),
                                ),
                                refused_mappings: submitted_mappings,
                            }
                        }
                    })?
                    .into_iter()
                    .collect::<std::collections::HashSet<_>>();
                if commit_retire_photo_ids
                    .iter()
                    .any(|id| retained.contains(id))
                {
                    let partial = commit_relocations
                        .iter()
                        .map(|relocation| RecoveryRejectionWire {
                            original_id: relocation.original_id.clone(),
                            reason: if relocation
                                .retire_photo_id
                                .as_ref()
                                .is_some_and(|id| retained.contains(id))
                            {
                                "destination-in-use"
                            } else {
                                "stale"
                            },
                        });
                    return Err(RecoveryApplyError::Rejected {
                        message: "Recovery batch rejected without changes",
                        rejections: complete_recovery_rejections(&commit_review_items, partial),
                        refused_mappings: submitted_mappings,
                    });
                }
            }
            commit_library
                .apply_relocations_blocking(commit_relocations)
                .map_err(|error| match error {
                    LibraryError::Persistence(
                        slipstream_core::persistence::PersistenceError::InvalidRecoveryMapping {
                            original_id,
                            reason,
                        },
                    ) => RecoveryApplyError::Rejected {
                        message: "Recovery batch conflicts with current Library state; rescan and review again",
                        rejections: complete_recovery_rejections(
                            &commit_review_items,
                            [RecoveryRejectionWire {
                                original_id,
                                reason,
                            }],
                        ),
                        refused_mappings: submitted_mappings,
                    },
                    LibraryError::Persistence(
                        slipstream_core::persistence::PersistenceError::InvalidRecovery,
                    ) => RecoveryApplyError::Rejected {
                        message: "Recovery batch conflicts with current Library state; rescan and review again",
                        rejections: complete_recovery_rejections(&commit_review_items, []),
                        refused_mappings: submitted_mappings,
                    },
                    other => RecoveryApplyError::Server(other.into()),
                })
        })
        .await
        .map_err(|error| RecoveryApplyError::Server(ServerError::Join(error.to_string())))??;
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
