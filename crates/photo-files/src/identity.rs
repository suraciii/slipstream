use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata},
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub device: u64,
    pub inode: u64,
    pub byte_length: u64,
    pub modified_seconds: i64,
    pub modified_nanos: i64,
    pub changed_seconds: i64,
    pub changed_nanos: i64,
}

impl Observation {
    pub fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            byte_length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanos: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanos: metadata.ctime_nsec(),
        })
    }

    pub fn of(file: &File) -> io::Result<Self> {
        Self::from_metadata(&file.metadata()?)
    }

    pub fn verify(&self, file: &File) -> io::Result<()> {
        if self == &Self::of(file)? {
            Ok(())
        } else {
            Err(io::Error::other("file changed during access"))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Evidence {
    pub sha256: String,
    pub byte_length: u64,
    pub observation: Observation,
}

impl Evidence {
    /// A revision includes the observed object, so replacing a path with equal
    /// bytes still invalidates a previously admitted filesystem operation.
    pub fn revision(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.sha256.as_bytes());
        hash.update(self.observation.device.to_le_bytes());
        hash.update(self.observation.inode.to_le_bytes());
        hash.update(self.byte_length.to_le_bytes());
        hash.update(self.observation.modified_seconds.to_le_bytes());
        hash.update(self.observation.modified_nanos.to_le_bytes());
        hash.update(self.observation.changed_seconds.to_le_bytes());
        hash.update(self.observation.changed_nanos.to_le_bytes());
        format!("{:x}", hash.finalize())
    }
}

/// Hash a retained descriptor with bounded memory and reject concurrent writes.
/// The caller's seek position is preserved even when validation fails.
pub fn fingerprint(file: &mut File, maximum_bytes: u64) -> io::Result<Evidence> {
    let observed = Observation::of(file)?;
    if observed.byte_length == 0 || observed.byte_length > maximum_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file exceeds admitted byte bounds",
        ));
    }
    let position = file.stream_position()?;
    let result = (|| {
        file.seek(SeekFrom::Start(0))?;
        let mut hash = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut count = 0_u64;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            count = count
                .checked_add(read as u64)
                .filter(|count| *count <= maximum_bytes)
                .ok_or_else(|| io::Error::other("file grew beyond admitted byte bounds"))?;
            hash.update(&buffer[..read]);
        }
        observed.verify(file)?;
        if count != observed.byte_length {
            return Err(io::Error::other("file length changed during hashing"));
        }
        Ok(Evidence {
            sha256: format!("{:x}", hash.finalize()),
            byte_length: count,
            observation: observed,
        })
    })();
    file.seek(SeekFrom::Start(position))?;
    result
}

pub fn identity(prefix: &str) -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    let mut output = String::with_capacity(prefix.len() + 33);
    output.push_str(prefix);
    output.push('-');
    use std::fmt::Write as _;
    for byte in bytes {
        write!(output, "{byte:02x}").expect("writing to String");
    }
    Ok(output)
}

pub fn timestamp() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(io::Error::other)
}
