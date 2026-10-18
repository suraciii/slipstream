use super::*;
use photo_files::{Observation, PendingFile, fingerprint};
use std::{
    io::{Read, Seek, SeekFrom},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

impl WorkspaceManager {
    /// Materialize an already validated private engine result into the durable
    /// directory. The alias settles only after checked no-replace publication.
    pub(crate) fn adopt_output(
        &self,
        workspace_id: &str,
        handle: &str,
        path: &Path,
        provenance: &Value,
    ) -> Result<Value> {
        require_id(handle)?;
        let mut session = self.locked(workspace_id, LeaseMode::Shared)?;
        let preview = provenance["kind"].as_str() == Some("preview");
        let category = layout::completed_directory(preview);
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 16
                    && value.bytes().all(|b| b.is_ascii_alphanumeric())
            })
            .ok_or_else(|| {
                WorkspaceError::new(
                    "output_invalid",
                    409,
                    "Completed image has no usable format extension.",
                )
            })?;
        let destination = session.directory.create_directory(category)?;
        let filename = format!("{handle}.{extension}");
        let completed_path = destination.path().join(&filename);
        let declared_digest = provenance["sha256"]
            .as_str()
            .or(provenance["byteEvidence"]["sha256"].as_str())
            .ok_or_else(|| {
                WorkspaceError::new(
                    "output_invalid",
                    409,
                    "Completed output receipt has no byte digest.",
                )
            })?;
        let declared_size = provenance["byteLength"]
            .as_u64()
            .or(provenance["length"].as_u64())
            .or(provenance["byteEvidence"]["byteLength"].as_u64())
            .ok_or_else(|| {
                WorkspaceError::new(
                    "output_invalid",
                    409,
                    "Completed output receipt has no byte length.",
                )
            })?;
        match destination.open_file(&filename) {
            Ok(mut existing) => {
                let evidence = fingerprint(&mut existing, FILE_BYTES_MAX)?;
                if evidence.sha256 != declared_digest || evidence.byte_length != declared_size {
                    return Err(WorkspaceError::new(
                        "output_invalid",
                        409,
                        "The completed output identity is occupied by different bytes.",
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.reserve_storage(declared_size)?;
                let mut source = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                    .open(path)?;
                let observation = Observation::of(&source)?;
                let _lease = FileLease::acquire(source.try_clone()?, LeaseMode::Shared, LOCK_WAIT)?;
                let evidence = fingerprint(&mut source, FILE_BYTES_MAX)?;
                if evidence.sha256 != declared_digest || evidence.byte_length != declared_size {
                    return Err(WorkspaceError::new(
                        "output_invalid",
                        409,
                        "The engine output bytes differ from their completion receipt.",
                    ));
                }
                source.seek(SeekFrom::Start(0))?;
                let mut pending = PendingFile::create(&destination)?;
                let copied = io::copy(
                    &mut Read::by_ref(&mut source).take(declared_size.saturating_add(1)),
                    pending.file_mut(),
                )?;
                observation.verify(&source)?;
                if copied != declared_size {
                    return Err(WorkspaceError::new(
                        "output_invalid",
                        409,
                        "Completed output changed during publication.",
                    ));
                }
                photo_files::image::inspect_image(pending.file_mut(), FILE_BYTES_MAX)?;
                let copied_evidence = fingerprint(pending.file_mut(), FILE_BYTES_MAX)?;
                if copied_evidence.sha256 != declared_digest {
                    return Err(WorkspaceError::new(
                        "output_invalid",
                        409,
                        "The completed copy differs from its receipt.",
                    ));
                }
                pending.publish(&filename, FILE_BYTES_MAX)?;
            }
            Err(error) => return Err(error.into()),
        }
        let mut completed_provenance = provenance.clone();
        completed_provenance["path"] = json!(completed_path);
        let result = files::publish(self, &mut session, &completed_path, &completed_provenance)?;
        if !preview {
            let id = result["artifactId"].as_str().ok_or_else(|| {
                WorkspaceError::new("output_invalid", 409, "The indexed output has no identity.")
            })?;
            let revision = result["revision"].as_str().ok_or_else(|| {
                WorkspaceError::new(
                    "output_invalid",
                    409,
                    "The indexed output has no checked revision.",
                )
            })?;
            self.record_artifact_alias(handle, workspace_id, id, revision)?;
        }
        Ok(result)
    }
}
