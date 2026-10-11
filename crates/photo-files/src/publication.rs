use crate::{Directory, Evidence, canonical_json, fingerprint, identity};
use serde::Serialize;
use std::{
    fs::{File, Permissions},
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

/// An unpublished output on its destination filesystem. Dropping it removes only
/// this attempt's temporary name; existing completed outputs are never touched.
pub struct PendingFile {
    directory: Directory,
    name: PathBuf,
    file: File,
    published: bool,
}

impl PendingFile {
    pub fn create(directory: &Directory) -> io::Result<Self> {
        let name = PathBuf::from(format!(".{}.partial", identity("photo")?));
        let file = directory.create_file(&name, 0o600)?;
        Ok(Self {
            directory: directory.try_clone()?,
            name,
            file,
            published: false,
        })
    }
    pub fn file(&self) -> &File {
        &self.file
    }
    pub fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }
    pub fn path(&self) -> PathBuf {
        self.directory.path().join(&self.name)
    }

    /// Callers validate the image itself before completing publication. This
    /// method checks byte stability and no-replace installation durably.
    pub fn publish(mut self, name: impl AsRef<Path>, maximum_bytes: u64) -> io::Result<Evidence> {
        self.file.sync_all()?;
        self.file.set_permissions(Permissions::from_mode(0o400))?;
        self.file.sync_all()?;
        let evidence = fingerprint(&mut self.file, maximum_bytes)?;
        self.directory.verify_path()?;
        let named = self.directory.open_file(&self.name)?;
        evidence.observation.verify(&named)?;
        self.directory
            .rename(&self.name, &self.directory, name, false)?;
        self.published = true;
        // rename changes ctime: derive the completed object's observation.
        let mut committed = evidence;
        committed.observation = crate::Observation::of(&self.file)?;
        Ok(committed)
    }
}

impl Drop for PendingFile {
    fn drop(&mut self) {
        if !self.published {
            let _ = self.directory.remove_file(&self.name);
        }
    }
}

/// Replace one metadata document only while the caller holds its owning lock.
/// The complete temporary document and both directory entries are synced.
pub fn atomic_json(
    directory: &Directory,
    name: impl AsRef<Path>,
    value: &impl Serialize,
) -> io::Result<()> {
    let bytes = canonical_json(value)?;
    let mut pending = PendingFile::create(directory)?;
    pending.file.write_all(&bytes)?;
    pending.file.sync_all()?;
    directory.rename(&pending.name, directory, name, true)?;
    pending.published = true;
    Ok(())
}
