use super::*;
use photo_files::{
    Directory, EntryKind, FileLease, LeaseMode, WalkLimits, fingerprint, identity, walk,
};
use rusqlite::params;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

impl WorkspaceManager {
    pub(crate) fn remove(
        &self,
        workspace_id: &str,
        artifact_id: &str,
        revision: Option<&str>,
        request_id: Option<&str>,
    ) -> Result<Value> {
        require_id(workspace_id)?;
        require_id(artifact_id)?;
        let request = request_id.map(str::to_owned).unwrap_or(identity("remove")?);
        require_id(&request)?;
        let intent = json!({"operation":"remove","artifactId":artifact_id,"revision":revision});
        let mut session = self.locked(workspace_id, LeaseMode::Exclusive)?;
        if let Some(mut settled) = replay(&session, &request, &intent)? {
            if settled["reclamation"] == "pending" {
                let row = files::record(&session, artifact_id)?;
                match session.directory.open_file(&row.path) {
                    Ok(mut retained) => {
                        let _guard = FileLease::acquire(
                            retained.try_clone()?,
                            LeaseMode::Exclusive,
                            Duration::ZERO,
                        )?;
                        let actual = fingerprint(&mut retained, FILE_BYTES_MAX)?;
                        if row.evidence.as_ref() == Some(&actual) {
                            actual
                                .observation
                                .verify(&session.directory.open_file(&row.path)?)?;
                            session.directory.remove_file(&row.path)?;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
                settled["reclamation"] = json!("complete");
                session.db.execute(
                    "UPDATE receipts SET result=? WHERE request_id=?",
                    params![encode(&settled)?, request],
                )?;
            }
            let availability = files::file(&session, artifact_id)
                .map(|v| v["availability"].clone())
                .unwrap_or(json!("missing"));
            settled["outcome"] = json!("replayed");
            settled["currentAvailability"] = availability;
            return Ok(settled);
        }
        files::reconcile(self, &mut session)?;
        let selected = files::file(&session, artifact_id)?;
        let row = files::record(&session, artifact_id)?;
        if row.removed {
            return Err(WorkspaceError::new(
                "artifact_unavailable",
                409,
                "Artifact was already explicitly removed; replay its original request",
            ));
        }
        let evidence = row.evidence.as_ref().ok_or_else(|| {
            WorkspaceError::new(
                "artifact_unavailable",
                409,
                "Selected file has no checked revision to remove",
            )
        })?;
        files::check_revision(&session, artifact_id, revision, evidence)?;
        let inventory = dependencies::inventory(self, &session)?;
        let blockers =
            dependencies::blockers(&inventory, Some(&selected), session.directory.path());
        if !blockers.is_empty() {
            return Err(WorkspaceError::new("artifact_referenced",409,"Selected artifact remains referenced or its independent use is unverified").detail(json!({"workspaceId":workspace_id,"artifactId":artifact_id,"revision":row.revision(),"blockers":blockers})));
        }
        let mut opened = match session.directory.open_file(&row.path) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let _file_guard = if let Some(opened) = opened.as_mut() {
            let guard=FileLease::acquire(opened.try_clone()?,LeaseMode::Exclusive,Duration::ZERO).map_err(|error|WorkspaceError::new("artifact_referenced",409,"Independent process is actively using the selected file").detail(json!({"workspaceId":workspace_id,"artifactId":artifact_id,"revision":row.revision(),"blockers":[{"kind":"active-use","id":null,"name":row.path,"action":"Wait for the independent process to release the file","reason":error.to_string()}]})))?;
            let current = fingerprint(opened, FILE_BYTES_MAX)?;
            if current != *evidence {
                return Err(WorkspaceError::new("artifact_changed",409,"Selected file changed before removal").detail(json!({"workspaceId":workspace_id,"artifactId":artifact_id,"expectedRevision":row.revision(),"currentRevision":current.revision()})));
            }
            evidence
                .observation
                .verify(&session.directory.open_file(&row.path)?)?;
            Some(guard)
        } else {
            None
        };
        // Persist tombstone and replay intent before unlink. The exact retained
        // observation permits safe recovery without deleting a replacement.
        let tx = session.db.unchecked_transaction()?;
        tx.execute(
            "UPDATE files SET removed=1,state='removed' WHERE id=?",
            [artifact_id],
        )?;
        let default = store::setting(&session, "defaultInput")?;
        if default == artifact_id {
            tx.execute(
                "UPDATE settings SET value='null' WHERE key='defaultInput'",
                [],
            )?;
            tx.execute("INSERT INTO settings(key,value) VALUES('initialized','true') ON CONFLICT(key) DO UPDATE SET value='true'",[])?;
        }
        let inventory = dependencies::inventory(self, &session)?;
        let input = if default == artifact_id {
            Value::Null
        } else {
            default
        };
        let input_path = match input.as_str() {
            Some(id) => files::file(&session, id)
                .ok()
                .map(|v| v["path"].clone())
                .unwrap_or(Value::Null),
            None => Value::Null,
        };
        let mut result = json!({"workspaceId":workspace_id,"artifactId":artifact_id,"revision":row.revision(),"requestId":request,"outcome":"removed","reclamation":"pending","inputArtifactId":input,"inputPath":input_path,"workspaceStateToken":token(&session,&inventory)?});
        record_receipt(&session, &request, &intent, &result)?;
        tx.commit()?;
        if opened.is_some() {
            session.directory.remove_file(&row.path)?;
        }
        result["reclamation"] = json!("complete");
        let inventory = dependencies::inventory(self, &session)?;
        result["workspaceStateToken"] = json!(token(&session, &inventory)?);
        session.db.execute(
            "UPDATE receipts SET result=? WHERE request_id=?",
            params![encode(&result)?, request],
        )?;
        Ok(result)
    }

    pub(crate) fn delete(
        &self,
        workspace_id: &str,
        reviewed_token: &str,
        request: &str,
        confirm: bool,
    ) -> Result<Value> {
        require_id(workspace_id)?;
        require_id(request)?;
        if !confirm {
            return Err(WorkspaceError::new(
                "invalid_input",
                400,
                "Workspace deletion requires explicit confirmation of its reviewed inventory",
            ));
        }
        let intent = json!({"operation":"delete","ifState":reviewed_token,"confirm":confirm});
        if let Some(mut result) = self.deletion_receipt(workspace_id, request, &intent)? {
            if result["reclamation"] != "complete" {
                result = self.continue_reclamation(workspace_id, request, &result)?;
            }
            result["outcome"] = json!("replayed");
            return Ok(result);
        }
        let mut session = self.locked(workspace_id, LeaseMode::Exclusive)?;
        files::reconcile(self, &mut session)?;
        let inventory = dependencies::inventory(self, &session)?;
        if token(&session, &inventory)? != reviewed_token {
            return Err(WorkspaceError::new(
                "workspace_changed",
                409,
                "Workspace loss inventory changed; review it again before deletion",
            ));
        }
        let blockers = dependencies::blockers(&inventory, None, session.directory.path());
        if !blockers.is_empty() {
            return Err(WorkspaceError::new(
                "workspace_referenced",
                409,
                "Workspace has active or unverified independent use or external references",
            )
            .detail(json!({"workspaceId":workspace_id,"blockers":blockers})));
        }
        let barriers = reclamation_barriers(&session.directory, true).map_err(|error| {
            WorkspaceError::new(
                "workspace_referenced",
                409,
                "Workspace files have active or unverified independent use",
            )
            .detail(json!({"workspaceId":workspace_id,"blockers":[{"kind":"active-use","id":null,"name":workspace_id,"action":"Wait for independent file use to end and review the workspace again","reason":error.to_string()}]}))
        })?;
        let result = json!({"workspaceId":workspace_id,"requestId":request,"outcome":"deleted","deleted":true,"reclamation":"pending","originalFilesChanged":false});
        self.withdraw(workspace_id, request, &intent, &result)?;
        // Keep the original directory/admission leases until reclamation ends.
        let final_result = reclaim_result(
            &session.directory,
            &self.root,
            workspace_id,
            &result,
            &barriers,
        );
        self.finish_deletion(workspace_id, request, &final_result)?;
        Ok(final_result)
    }

    fn continue_reclamation(&self, id: &str, request: &str, prior: &Value) -> Result<Value> {
        let directory = match self.reclaim_directory(id) {
            Ok(directory) => directory,
            Err(error) if error.code == "not_found" => {
                let mut result = prior.clone();
                result["reclamation"] = json!("complete");
                self.finish_deletion(id, request, &result)?;
                return Ok(result);
            }
            Err(error) => return Err(error),
        };
        let _ancestors = photo_files::ancestor_leases(directory.path(), LOCK_WAIT)?;
        let _guard = directory.lease(LeaseMode::Exclusive, LOCK_WAIT)?;
        let result = match reclamation_barriers(&directory, false) {
            Ok(barriers) => reclaim_result(&directory, &self.root, id, prior, &barriers),
            Err(error) => {
                let mut result = prior.clone();
                result["reclamation"] = json!("pending");
                result["reclamationError"] = json!(error.to_string());
                result
            }
        };
        self.finish_deletion(id, request, &result)?;
        Ok(result)
    }
}

fn reclaim_result(
    directory: &Directory,
    root: &Directory,
    id: &str,
    prior: &Value,
    barriers: &ReclamationBarriers,
) -> Value {
    let mut result = prior.clone();
    match reclaim_checked(directory, &mut 0, Some(barriers)).and_then(|_| root.remove_directory(id))
    {
        Ok(()) => {
            result["reclamation"] = json!("complete");
            if let Some(object) = result.as_object_mut() {
                object.remove("reclamationError");
            }
        }
        Err(error) => {
            result["reclamation"] = json!("pending");
            result["reclamationError"] = json!(error.to_string());
        }
    }
    result
}

struct ReclamationBarriers {
    _leases: Vec<FileLease>,
    files: std::collections::BTreeMap<PathBuf, photo_files::Observation>,
}

fn reclamation_barriers(
    directory: &Directory,
    index_locked: bool,
) -> std::io::Result<ReclamationBarriers> {
    let mut barriers = ReclamationBarriers {
        _leases: Vec::new(),
        files: std::collections::BTreeMap::new(),
    };
    retain_reclamation_barriers(directory, &mut 0, index_locked, &mut barriers)?;
    Ok(barriers)
}

fn retain_reclamation_barriers(
    directory: &Directory,
    count: &mut usize,
    index_locked: bool,
    barriers: &mut ReclamationBarriers,
) -> std::io::Result<()> {
    let nesting = || std::io::Error::other("Workspace reclamation nesting exceeds finite limit");
    let inventory = || std::io::Error::other("Workspace reclamation exceeds finite entry limit");
    walk(
        directory,
        Path::new(""),
        WalkLimits::new(ENTRY_MAX),
        nesting,
        |reported| {
            *count += 1;
            if *count > ENTRY_MAX {
                return Err(inventory());
            }
            match reported.entry.kind {
                EntryKind::Directory => {
                    let child = reported.directory.child(&reported.entry.name)?;
                    // Root-to-leaf acquisition agrees with standalone ancestor admission.
                    // Nonblocking barriers refuse existing users instead of waiting on a child.
                    barriers
                        ._leases
                        .push(child.lease(LeaseMode::Exclusive, Duration::ZERO)?);
                    Ok(Some(child))
                }
                EntryKind::File => {
                    let file = reported.directory.open_file(&reported.entry.name)?;
                    if !(reported.depth == 0
                        && index_locked
                        && reported.entry.name == ".workspace-lock")
                    {
                        barriers._leases.push(FileLease::acquire(
                            file.try_clone()?,
                            LeaseMode::Exclusive,
                            Duration::ZERO,
                        )?);
                    }
                    barriers.files.insert(
                        reported.directory.path().join(&reported.entry.name),
                        photo_files::Observation::of(&file)?,
                    );
                    Ok(None)
                }
                // Reclamation reports unexpected entries after registry withdrawal;
                // they have no regular file descriptor to lease or unlink here.
                EntryKind::Symlink | EntryKind::Other => Ok(None),
            }
        },
        |_| Ok(()),
    )
}

#[cfg(test)]
fn reclaim(directory: &Directory, count: &mut usize) -> std::io::Result<()> {
    reclaim_checked(directory, count, None)
}

fn reclaim_checked(
    directory: &Directory,
    count: &mut usize,
    barriers: Option<&ReclamationBarriers>,
) -> std::io::Result<()> {
    let nesting = || std::io::Error::other("Workspace reclamation nesting exceeds finite limit");
    let inventory = || std::io::Error::other("Workspace reclamation exceeds finite entry limit");
    walk(
        directory,
        Path::new(""),
        WalkLimits::new(ENTRY_MAX),
        nesting,
        |reported| {
            *count += 1;
            if *count > ENTRY_MAX {
                return Err(inventory());
            }
            match reported.entry.kind {
                EntryKind::Directory => Ok(Some(reported.directory.child(&reported.entry.name)?)),
                EntryKind::File => {
                    if let Some(barriers) = barriers {
                        let path = reported.directory.path().join(&reported.entry.name);
                        let expected = barriers.files.get(&path).ok_or_else(|| {
                            std::io::Error::other("Unreviewed file appeared during reclamation")
                        })?;
                        let observed = photo_files::Observation::of(
                            &reported.directory.open_file(&reported.entry.name)?,
                        )?;
                        if observed.device != expected.device || observed.inode != expected.inode {
                            return Err(std::io::Error::other(
                                "File was replaced during reclamation",
                            ));
                        }
                    }
                    reported.directory.remove_file(&reported.entry.name)?;
                    Ok(None)
                }
                EntryKind::Symlink | EntryKind::Other => Err(std::io::Error::other(
                    "Unexpected symbolic link or special file appeared during reclamation",
                )),
            }
        },
        |reported| reported.directory.remove_directory(&reported.entry.name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destructive_lease_excludes_concurrent_bind_and_download_lifetime() {
        let path = std::env::temp_dir().join(identity("workspace-lease-test").unwrap());
        let directory = Directory::create(&path).unwrap();
        let pinned = directory.lease(LeaseMode::Shared, Duration::ZERO).unwrap();
        assert!(
            directory
                .lease(LeaseMode::Exclusive, Duration::ZERO)
                .is_err()
        );
        drop(pinned);
        let removal = directory
            .lease(LeaseMode::Exclusive, Duration::ZERO)
            .unwrap();
        assert!(directory.lease(LeaseMode::Shared, Duration::ZERO).is_err());
        drop(removal);
        std::fs::remove_dir(path).unwrap();
    }
    #[test]
    fn reclamation_barriers_hold_files_and_refuse_late_replacements() {
        let path = std::env::temp_dir().join(identity("workspace-file-barrier-test").unwrap());
        let directory = Directory::create(&path).unwrap();
        directory.create_file("input.raw", 0o600).unwrap();
        let admitted = FileLease::acquire(
            directory.open_file("input.raw").unwrap(),
            LeaseMode::Shared,
            Duration::ZERO,
        )
        .unwrap();
        assert!(reclamation_barriers(&directory, false).is_err());
        assert!(directory.open_file("input.raw").is_ok());
        drop(admitted);
        let barriers = reclamation_barriers(&directory, false).unwrap();
        assert!(
            FileLease::acquire(
                directory.open_file("input.raw").unwrap(),
                LeaseMode::Shared,
                Duration::ZERO
            )
            .is_err()
        );
        directory
            .rename("input.raw", &directory, "retained.raw", false)
            .unwrap();
        directory.create_file("input.raw", 0o600).unwrap();
        assert!(reclaim_checked(&directory, &mut 0, Some(&barriers)).is_err());
        assert!(directory.open_file("input.raw").is_ok());
        drop(barriers);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn reclamation_never_follows_external_symlink() {
        let path = std::env::temp_dir().join(identity("workspace-reclaim-test").unwrap());
        let directory = Directory::create(&path).unwrap();
        std::os::unix::fs::symlink("/", path.join("escape")).unwrap();
        assert!(reclaim(&directory, &mut 0).is_err());
        directory.remove_file("escape").unwrap();
        std::fs::remove_dir(path).unwrap();
    }
}
