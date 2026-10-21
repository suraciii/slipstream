use super::store::{evidence, set_setting, setting};
use super::*;
use photo_files::{PendingFile, fingerprint, identity};
use rusqlite::{OptionalExtension, params};
use std::{ffi::CString, os::fd::AsRawFd, path::Path};

impl WorkspaceManager {
    pub(crate) fn open_photo(&self, photo_id: &str) -> Result<Value> {
        require_id(photo_id)?;
        let workspace = match self.lookup_photo(photo_id)? {
            Some(workspace) => workspace,
            None => self.create_workspace(photo_id)?,
        };
        {
            let mut session = self.locked(&workspace.id, LeaseMode::Shared)?;
            if setting(&session, "initialized")? != Value::Bool(true) {
                let source = (self.resolve)(photo_id)?.ok_or_else(|| {
                    WorkspaceError::new(
                        "source_unavailable",
                        409,
                        "Recover the Original before preparing its first input.",
                    )
                })?;
                let revision = source.revision.as_deref().ok_or_else(|| {
                    WorkspaceError::new(
                        "source_unavailable",
                        409,
                        "Recover the Original before preparing its first input.",
                    )
                })?;
                use sha2::{Digest, Sha256};
                let request = format!("open-{:x}", Sha256::digest(revision.as_bytes()));
                self.prepare(&mut session, revision, &request)?;
            }
        }
        self.show(&workspace.id)
    }

    pub(crate) fn refresh(
        &self,
        workspace_id: &str,
        expected_source: &str,
        request_id: Option<&str>,
    ) -> Result<Value> {
        if expected_source.is_empty() || expected_source.len() > 16384 {
            return Err(WorkspaceError::new(
                "invalid_input",
                400,
                "Use the observed source revision from workspace show.",
            ));
        }
        let request = match request_id {
            Some(id) => {
                require_id(id)?;
                id.to_owned()
            }
            None => identity("refresh")?,
        };
        let mut session = self.locked(workspace_id, LeaseMode::Shared)?;
        self.prepare(&mut session, expected_source, &request)
    }

    fn prepare(
        &self,
        session: &mut WorkspaceSession,
        expected_source: &str,
        request: &str,
    ) -> Result<Value> {
        let intent = json!({"operation":"refresh-input","expectedSourceRevision":expected_source});
        if let Some(mut result) = replay(session, request, &intent)? {
            result["outcome"] = json!("replayed");
            result["currentAvailability"] = self.prepared_availability(session, &result)?;
            return Ok(result);
        }
        let prior: Option<(String, String, String, Option<String>)> = session
            .db
            .query_row(
                "SELECT intent,path,artifact_id,evidence FROM preparations WHERE request_id=?1",
                [request],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if prior
            .as_ref()
            .is_some_and(|(old, _, _, _)| old != &encode(&intent).unwrap_or_default())
        {
            return Err(WorkspaceError::new(
                "request_conflict",
                409,
                "The request identity belongs to different input preparation.",
            ));
        }
        // A published file with journaled byte evidence settles without rereading
        // an Original that may have gone away after the successful copy.
        if let Some((_, path, id, Some(expected))) = &prior
            && let Ok(mut file) = session.directory.open_file(path)
        {
            let expected: photo_files::Evidence = serde_json::from_str(expected)?;
            let actual = fingerprint(&mut file, FILE_BYTES_MAX)?;
            if expected.sha256 != actual.sha256 || expected.byte_length != actual.byte_length {
                return Err(WorkspaceError::new(
                    "outcome_unknown",
                    409,
                    "Prepared bytes changed before their outcome could be reconciled.",
                )
                .detail(json!({"requestId":request,"artifactId":id})));
            }
            return self.settle_prepared(
                session,
                request,
                &intent,
                id,
                path,
                expected_source,
                &actual,
                "created",
            );
        }
        let source = (self.resolve)(&session.workspace.photo_id)?.ok_or_else(|| {
            WorkspaceError::new(
                "source_unavailable",
                409,
                "Recover the Original before preparing a new input.",
            )
        })?;
        if source.revision.as_deref() != Some(expected_source) {
            return Err(WorkspaceError::new("conflict",409,"The Library source changed; inspect it before refreshing.").detail(json!({"expectedSourceRevision":expected_source,"currentSourceRevision":source.revision})));
        }
        let root = slipstream_core::LibraryRoot::open(&self.library_root).map_err(source_error)?;
        let original = root
            .original(source.relative_path.clone())
            .map_err(source_error)?;
        let facts = original.facts().map_err(source_error)?;
        let observed =
            slipstream_core::capture_source_revision(source.relative_path.as_str(), facts)
                .map_err(|e| WorkspaceError::new("source_unavailable", 409, e.to_string()))?;
        if observed != expected_source {
            return Err(WorkspaceError::new(
                "conflict",
                409,
                "The Original changed; check the Library and inspect its new revision.",
            ));
        }
        if prior.is_none() {
            let reusable: Vec<(String, String, String)> = {
                let mut statement=session.db.prepare("SELECT id,path,evidence FROM files WHERE removed=0 AND kind='input' AND managed=1 AND state='ready' AND source_revision=?1 ORDER BY id")?;
                statement
                    .query_map([expected_source], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
            for (id, path, encoded) in reusable {
                let expected: photo_files::Evidence = serde_json::from_str(&encoded)?;
                if let Ok(mut file) = session.directory.open_file(&path) {
                    let actual = fingerprint(&mut file, FILE_BYTES_MAX)?;
                    if actual == expected {
                        return self.settle_prepared(
                            session,
                            request,
                            &intent,
                            &id,
                            &path,
                            expected_source,
                            &actual,
                            "unchanged",
                        );
                    }
                }
            }
        }
        let input = session.directory.create_directory(layout::INPUT)?;
        let (id, path) = match prior {
            Some((_, path, id, _)) => (id, path),
            None => {
                let id = identity("input")?;
                let original_name = Path::new(source.relative_path.as_str())
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| {
                        WorkspaceError::new(
                            "source_unavailable",
                            409,
                            "The Original filename cannot be represented.",
                        )
                    })?;
                let filename = if input
                    .entries(ENTRY_MAX)?
                    .iter()
                    .all(|e| e.name != original_name)
                {
                    original_name.to_owned()
                } else {
                    versioned_name(original_name, &id)
                };
                let path = layout::managed_file(layout::INPUT, &filename).ok_or_else(|| {
                    WorkspaceError::new(
                        "source_unavailable",
                        409,
                        "The Original filename cannot be represented.",
                    )
                })?;
                session.db.execute("INSERT INTO preparations(request_id,intent,path,artifact_id,source_revision) VALUES(?1,?2,?3,?4,?5)",params![request,encode(&intent)?,path,id,expected_source])?;
                (id, path)
            }
        };
        let mut pending = PendingFile::create(&input)?;
        let original_size = original.facts().map_err(source_error)?.size;
        self.reserve_storage(original_size)?;
        // fallocate reserves real filesystem space before any source copy.
        let allocated = unsafe {
            libc::posix_fallocate(
                pending.file().as_raw_fd(),
                0,
                i64::try_from(original_size).map_err(|_| {
                    WorkspaceError::new(
                        "resource_limit",
                        413,
                        "The input exceeds the admitted file size.",
                    )
                })?,
            )
        };
        if allocated != 0 && allocated != libc::EOPNOTSUPP && allocated != libc::ENOSYS {
            return Err(io::Error::from_raw_os_error(allocated).into());
        }
        let copied = original
            .copy_revision_checked(pending.file_mut())
            .map_err(source_error)?;
        let copied_revision =
            slipstream_core::capture_source_revision(source.relative_path.as_str(), copied.facts)
                .map_err(|e| WorkspaceError::new("source_unavailable", 409, e.to_string()))?;
        if copied_revision != expected_source {
            return Err(WorkspaceError::new(
                "conflict",
                409,
                "The Original changed during preparation; the incomplete copy was discarded.",
            ));
        }
        let bytes = fingerprint(pending.file_mut(), FILE_BYTES_MAX)?;
        if bytes.sha256 != copied.digest {
            return Err(WorkspaceError::new(
                "artifact_changed",
                409,
                "The protected copy did not match the Original.",
            ));
        }
        session.db.execute(
            "UPDATE preparations SET evidence=?2 WHERE request_id=?1",
            params![request, serde_json::to_string(&bytes)?],
        )?;
        let filename = Path::new(&path).file_name().ok_or_else(|| {
            WorkspaceError::new(
                "workspace_unavailable",
                503,
                "Prepared filename is invalid.",
            )
        })?;
        let published = pending.publish(filename, FILE_BYTES_MAX)?;
        self.settle_prepared(
            session,
            request,
            &intent,
            &id,
            &path,
            expected_source,
            &published,
            "created",
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn settle_prepared(
        &self,
        session: &mut WorkspaceSession,
        request: &str,
        intent: &Value,
        id: &str,
        path: &str,
        source_revision: &str,
        bytes: &photo_files::Evidence,
        outcome: &str,
    ) -> Result<Value> {
        session.db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            session.db.execute("INSERT INTO files(id,path,kind,state,evidence,revision,source_revision,managed,removed) VALUES(?1,?2,'input','ready',?3,?4,?5,1,0) ON CONFLICT(id) DO NOTHING",params![id,path,serde_json::to_string(bytes)?,bytes.revision(),source_revision])?;
            if setting(session, "defaultInput")?.is_null() {
                set_setting(session, "defaultInput", &json!(id))?;
            }
            set_setting(session, "initialized", &json!(true))?;
            let inventory = dependencies::inventory(self, session)?;
            let result = json!({"workspaceId":session.workspace.id,"requestId":request,"outcome":outcome,"inputArtifactId":id,"inputPath":session.directory.path().join(path),"revision":bytes.revision(),"workspaceStateToken":token(session,&inventory)?,"currentAvailability":"ready"});
            record_receipt(session, request, intent, &result)?;
            session.db.execute(
                "UPDATE preparations SET result=?2 WHERE request_id=?1",
                params![request, encode(&result)?],
            )?;
            Ok(result)
        })();
        match result {
            Ok(result) => {
                session.db.execute_batch("COMMIT")?;
                Ok(result)
            }
            Err(error) => {
                let _ = session.db.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }
    fn prepared_availability(&self, session: &WorkspaceSession, result: &Value) -> Result<Value> {
        let id = result["inputArtifactId"].as_str().unwrap_or("");
        let row: Option<(String, bool)> = session
            .db
            .query_row("SELECT path,removed FROM files WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let Some((path, removed)) = row else {
            return Ok(json!("missing"));
        };
        if removed {
            return Ok(json!("removed"));
        }
        let Ok(mut file) = session.directory.open_file(path) else {
            return Ok(json!("missing"));
        };
        Ok(json!(
            if fingerprint(&mut file, FILE_BYTES_MAX)? == evidence(session, id)? {
                "ready"
            } else {
                "changed"
            }
        ))
    }
    pub(super) fn reserve_storage(&self, bytes: u64) -> Result<()> {
        if bytes == 0 || bytes > FILE_BYTES_MAX || bytes > self.allowance {
            return Err(WorkspaceError::new(
                "resource_limit",
                413,
                "The requested file exceeds workspace storage limits.",
            ));
        }
        let mut used = 0_u64;
        disk_usage(&self.root, &mut used, &mut 0)?;
        if used.checked_add(bytes).is_none_or(|n| n > self.allowance) {
            return Err(WorkspaceError::new(
                "resource_limit",
                507,
                "Remove unused workspace files or increase storage before preparing this input.",
            ));
        }
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        let path = CString::new(self.root.path().as_os_str().as_encoded_bytes())
            .map_err(|e| WorkspaceError::new("workspace_unavailable", 503, e.to_string()))?;
        if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        let stats = unsafe { stats.assume_init() };
        if stats.f_bavail.saturating_mul(stats.f_frsize) < bytes {
            return Err(WorkspaceError::new(
                "resource_limit",
                507,
                "The workspace filesystem has insufficient free space.",
            ));
        }
        Ok(())
    }
}
fn source_error(error: slipstream_core::confinement::ConfinementError) -> WorkspaceError {
    WorkspaceError::new(
        "source_unavailable",
        409,
        format!("Input preparation could not read a stable Original: {error}"),
    )
}
fn versioned_name(name: &str, id: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem}-{id}.{extension}"),
        None => format!("{name}-{id}"),
    }
}
fn disk_usage(directory: &Directory, bytes: &mut u64, count: &mut usize) -> Result<()> {
    for entry in directory.entries(ENTRY_MAX)? {
        *count += 1;
        if *count > ENTRY_MAX {
            return Err(WorkspaceError::new(
                "resource_limit",
                503,
                "Workspace storage inventory exceeds the entry bound.",
            ));
        }
        match entry.kind {
            photo_files::EntryKind::File => {
                *bytes = bytes
                    .checked_add(directory.open_file(&entry.name)?.metadata()?.len())
                    .ok_or_else(|| {
                        WorkspaceError::new(
                            "resource_limit",
                            503,
                            "Workspace storage inventory overflows.",
                        )
                    })?;
            }
            photo_files::EntryKind::Directory => {
                disk_usage(&directory.child(&entry.name)?, bytes, count)?
            }
            _ => (),
        }
    }
    Ok(())
}
