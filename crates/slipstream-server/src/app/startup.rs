use super::*;

impl Application {
    pub async fn open(config: &Config) -> Result<Arc<Self>, ServerError> {
        Self::open_with_gate(config, ScanLimits::default(), None, None).await
    }

    pub(crate) async fn open_with_gate(
        config: &Config,
        scan_limits: ScanLimits,
        scan_gate: Option<oneshot::Receiver<()>>,
        publish_gate: Option<oneshot::Receiver<()>>,
    ) -> Result<Arc<Self>, ServerError> {
        validate_storage_layout(config)?;
        let cache = CacheDirectory::open(&config.cache_directory, &config.library_root)?;
        let library_config = LibraryConfig {
            library_root: config.library_root.clone(),
            state_directory: config.state_directory.clone(),
            database_basename: config.database_basename.clone(),
            limits: scan_limits,
            ..LibraryConfig::default()
        };
        let library = tokio::task::spawn_blocking(move || Library::open(library_config))
            .await
            .map_err(|error| ServerError::Join(error.to_string()))??;
        let library = Arc::new(library);
        let access = match crate::access::Access::open(config) {
            Ok(access) => access,
            Err(error) => {
                let _ = library.shutdown();
                return Err(error);
            }
        };
        // Admission is complete: serve the last committed Library immediately
        // while the ordinary startup rescan runs in the background. A store
        // without a published Library stays initializing until its first scan
        // publishes one.
        let persisted = library.snapshot().await?;
        // Non-empty v2-v5 stores predate the durable marker and remain
        // published-compatible. Every completed scan now writes the marker,
        // which also preserves an intentionally empty Published Library.
        let published_initial =
            persisted.published || !persisted.photos.is_empty() || !persisted.originals.is_empty();
        let shared = Arc::new(SharedLibrary {
            snapshot: RwLock::new(published_initial.then(|| Published::new(persisted))),
            published: AtomicBool::new(published_initial),
            failed: AtomicBool::new(false),
            awaiting_scan: AtomicUsize::new(0),
            runs_started: AtomicU64::new(0),
            runs_completed: AtomicU64::new(0),
            publication: tokio::sync::Mutex::new(()),
        });
        let workspace_state = config.state_directory.clone();
        let workspace_library = config.library_root.clone();
        let workspace_shared = Arc::clone(&shared);
        let workspace_resolver = Arc::new(move |photo_id: &str| {
            let guard = workspace_shared
                .snapshot
                .read()
                .expect("published Library poisoned");
            let source = guard.as_ref()?.photo_metadata_source(photo_id)?;
            Some(crate::workspace::Source {
                relative_path: source.relative_path,
                revision: source.source_revision,
            })
        });
        let workspaces = Arc::new(
            tokio::task::spawn_blocking(move || {
                crate::workspace::WorkspaceManager::open(
                    &workspace_state,
                    workspace_library,
                    workspace_resolver,
                    crate::workspace::DEFAULT_STORAGE_BYTES,
                )
            })
            .await
            .map_err(|error| ServerError::Join(error.to_string()))?
            .map_err(|error| ServerError::Export(error.to_string()))?,
        );
        // The Export lifecycle runs only when the deployment configures the
        // processing capability and its finite retained-output allowance.
        let exports = match (&config.processing, config.export_retained_output_bytes) {
            (Some(processing), Some(allowance)) => {
                let processing = processing.clone();
                let library_for_exports = Arc::clone(&library);
                let root_for_exports = config.library_root.clone();
                let state_for_exports = config.state_directory.clone();
                let shared_for_exports = Arc::clone(&shared);
                let workspaces_for_exports = Arc::clone(&workspaces);
                let resolver = Arc::new(move |photo_id: &str| {
                    let guard = shared_for_exports
                        .snapshot
                        .read()
                        .expect("published Library poisoned");
                    let published = guard.as_ref()?;
                    published
                        .photo_metadata_source(photo_id)
                        .map(|source| source.relative_path)
                });
                let opened = tokio::task::spawn_blocking(move || {
                    crate::export_manager::ExportManager::open(
                        library_for_exports,
                        root_for_exports,
                        &state_for_exports,
                        resolver,
                        processing,
                        allowance,
                        workspaces_for_exports,
                    )
                })
                .await
                .map_err(|error| ServerError::Join(error.to_string()))?;
                match opened {
                    Ok(manager) => Some(Arc::new(manager)),
                    Err(message) => {
                        let library_for_close = Arc::clone(&library);
                        let _ =
                            tokio::task::spawn_blocking(move || library_for_close.shutdown()).await;
                        return Err(ServerError::Export(message));
                    }
                }
            }
            _ => None,
        };
        let proxies = match (&config.processing, exports.as_ref()) {
            (Some(processing), Some(exports)) => {
                let opened = crate::development_proxy::DevelopmentProxyManager::open(
                    Arc::clone(&library),
                    Arc::clone(exports),
                    processing.clone(),
                    &config.state_directory,
                );
                match opened {
                    Ok(manager) => Some(Arc::new(manager)),
                    Err(message) => {
                        let library_for_close = Arc::clone(&library);
                        let _ =
                            tokio::task::spawn_blocking(move || library_for_close.shutdown()).await;
                        return Err(ServerError::Export(message));
                    }
                }
            }
            _ => None,
        };
        let library_for_preview = Arc::clone(&library);
        let preview = match tokio::task::spawn_blocking(move || {
            PreviewService::from_cache(library_for_preview, cache)
        })
        .await
        .map_err(|error| ServerError::Join(error.to_string()))?
        {
            Ok(preview) => preview,
            Err(error) => {
                let library_for_close = Arc::clone(&library);
                let _ = tokio::task::spawn_blocking(move || library_for_close.shutdown()).await;
                return Err(ServerError::Preview(error.to_string()));
            }
        };
        let browse_namespace = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            ^ (u128::from(std::process::id()) << 64)
            ^ u128::from(NEXT_BROWSE_NAMESPACE.fetch_add(1, Ordering::Relaxed));
        let tool_state = config.state_directory.clone();
        let tool_workspaces = Arc::clone(&workspaces);
        let tools = Arc::new(
            tokio::task::spawn_blocking(move || {
                crate::darktable_service::ToolService::open(&tool_state, tool_workspaces)
            })
            .await
            .map_err(|error| ServerError::Join(error.to_string()))?
            .map_err(ServerError::Export)?,
        );
        let application = Arc::new(Self {
            access,
            library,
            library_root: config.library_root.clone(),
            workspaces,
            tools,
            exports,
            export_artifacts_directory: config.state_directory.join("exports").join("artifacts"),
            proxies,
            scan_cycle: ScanCycle::new(),
            review_warmup: Mutex::new(ReviewWarmupState::default()),
            preview,
            shared,
            retained_queries: RetainedQueryStore::production(),
            recovery_reviews: Mutex::new(crate::recovery_review::RecoveryReviews::production()),
            browse_namespace,
            instance_epoch: {
                let mut bytes = [0u8; 16];
                getrandom::fill(&mut bytes).expect("system randomness is available");
                bytes.iter().map(|byte| format!("{byte:02x}")).collect()
            },
            metadata_supervisor: config.metadata_supervisor.clone(),
            cursor_signer: CursorSigner::new(),
            shutdown: Mutex::new(false),
        });
        // Admit the startup cycle synchronously before returning the
        // Application, so an immediate explicit rescan joins this leader.
        let startup = application
            .admit_scan_cycle(scan_gate, publish_gate)
            .expect("a new Application admits its startup scan");
        drop(startup);
        // Settle interrupted work and provisional input pins before new admission.
        if let Some(manager) = application.exports.as_ref() {
            crate::processing_export::reconcile_processing_exports(&application)
                .await
                .map_err(ServerError::Export)?;
            manager
                .reconcile_export_dependencies_on_startup()
                .await
                .map_err(ServerError::Export)?;
            manager.reconcile_after_restart();
            manager.schedule_expiry_sweep();
        }
        if let Some(manager) = application.proxies.as_ref() {
            manager.reconcile_after_restart();
        }
        Ok(application)
    }
}
