use super::transport::OUTPUT_MAX;
use photo_files::{Directory, EntryKind};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Manifest {
    pub format: u32,
    pub schema_version: String,
    pub protocol_version: String,
    pub binary: PathBuf,
    pub data_directory: PathBuf,
    pub files: BTreeMap<String, String>,
    pub tools: Value,
    pub discovery: Value,
    pub output_profiles: BTreeMap<String, String>,
}
pub(super) struct Bundle {
    pub directory: Directory,
    pub manifest: Manifest,
    pub id: String,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn relative(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid("bundle path escapes root"));
    }
    Ok(())
}
fn digest_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn inventory(
    directory: &Directory,
    prefix: &Path,
    files: &BTreeMap<String, String>,
    count: &mut usize,
) -> io::Result<()> {
    for entry in directory.entries(20_000)? {
        *count += 1;
        if *count > 20_000 {
            return Err(invalid("bundle inventory exceeds bound"));
        }
        let path = prefix.join(&entry.name);
        match entry.kind {
            EntryKind::Directory => inventory(&directory.child(&entry.name)?, &path, files, count)?,
            EntryKind::File => {
                let name = path
                    .to_str()
                    .ok_or_else(|| invalid("bundle filename encoding"))?;
                if name != "bundle" && name != "edit-manifest.json" && !files.contains_key(name) {
                    return Err(invalid("bundle contains unpinned file"));
                }
            }
            _ => return Err(invalid("bundle contains symlink or special file")),
        }
    }
    Ok(())
}
impl Bundle {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let directory = Directory::open(path)?;
        let mut bytes = Vec::new();
        directory
            .open_file("edit-manifest.json")?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(invalid("bundle manifest exceeds bound"));
        }
        let id = format!("{:x}", Sha256::digest(&bytes));
        let mut identity = String::new();
        directory
            .open_file("bundle")?
            .take(66)
            .read_to_string(&mut identity)?;
        if identity != format!("{id}\n") {
            return Err(invalid("bundle manifest digest mismatch"));
        }
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if manifest.format != 1
            || manifest.schema_version.is_empty()
            || manifest.protocol_version != "2025-06-18"
            || !manifest.tools.is_array()
            || !manifest.discovery.is_object()
            || manifest.files.is_empty()
            || manifest.files.len() > 20_000
        {
            return Err(invalid("bundle manifest contract mismatch"));
        }
        relative(&manifest.binary)?;
        relative(&manifest.data_directory)?;
        if !manifest.files.contains_key(
            manifest
                .binary
                .to_str()
                .ok_or_else(|| invalid("bundle binary encoding"))?,
        ) {
            return Err(invalid("native binary is not pinned"));
        }
        directory.child(&manifest.data_directory)?;
        if manifest
            .discovery
            .get("schemaVersion")
            .and_then(Value::as_str)
            != Some(manifest.schema_version.as_str())
            || manifest.discovery.get("outputProfiles")
                != Some(&serde_json::to_value(&manifest.output_profiles).map_err(io::Error::other)?)
        {
            return Err(invalid(
                "manifest semantic identity differs from native discovery",
            ));
        }
        for (path, expected) in &manifest.files {
            relative(Path::new(path))?;
            if !digest_valid(expected) {
                return Err(invalid("bundle digest invalid"));
            }
            let mut file = directory.open_file(path)?;
            if file.metadata()?.len() > OUTPUT_MAX {
                return Err(invalid("bundle asset exceeds bound"));
            }
            let mut hash = Sha256::new();
            let mut buffer = [0; 65536];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            if format!("{:x}", hash.finalize()) != *expected {
                return Err(invalid("bundle asset digest mismatch"));
            }
        }
        if manifest.output_profiles.is_empty()
            || manifest.output_profiles.values().any(|v| !digest_valid(v))
        {
            return Err(invalid("bundle output profiles are not pinned"));
        }
        inventory(&directory, Path::new(""), &manifest.files, &mut 0)?;
        Ok(Self {
            directory,
            manifest,
            id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundle_paths_cannot_escape() {
        assert!(relative(Path::new("bin/native")).is_ok());
        for p in ["", "/tmp/native", "../native", "bin/../native"] {
            assert!(relative(Path::new(p)).is_err());
        }
    }
    #[test]
    fn hashes_require_exact_lowercase_hex() {
        assert!(digest_valid(&"a".repeat(64)));
        assert!(!digest_valid(&"A".repeat(64)));
        assert!(!digest_valid("abc"));
    }
}
