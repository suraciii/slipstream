//! Deterministic operator-only barriers for the qualification verifier.
use crate::{
    journal::Record,
    protocol::{Config, ErrorCode, now},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    thread,
    time::Duration,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Phase {
    #[serde(rename = "after-intent")]
    Intent,
    #[serde(rename = "after-slice")]
    Slice,
    #[serde(rename = "after-create-response")]
    CreateResponse,
    #[serde(rename = "after-container-bound")]
    ContainerBound,
    #[serde(rename = "after-release-intent")]
    ReleaseIntent,
    #[serde(rename = "after-exit")]
    Exit,
    #[serde(rename = "after-evidence")]
    Evidence,
    #[serde(rename = "after-container-removal")]
    ContainerRemoval,
    #[serde(rename = "after-storage-unmount")]
    StorageUnmount,
    #[serde(rename = "after-slice-stop")]
    SliceStop,
    #[serde(rename = "after-stage-release-intent")]
    StageReleaseIntent,
    #[serde(rename = "after-stage-ack")]
    StageAck,
    #[serde(rename = "after-snapshot-sealed")]
    SnapshotSealed,
    #[serde(rename = "after-engine-release-intent")]
    EngineReleaseIntent,
    #[serde(rename = "after-validated-result")]
    ValidatedResult,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arm {
    phase: Phase,
    incarnation: String,
    sequence: u64,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Marker {
    phase: Phase,
    incarnation: String,
    sequence: u64,
    launch_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriterFault {
    incarnation: String,
    sequence: u64,
}
fn writer_fault(config: &Config) -> Result<Option<WriterFault>, ErrorCode> {
    let selected =
        read::<WriterFault>(&Path::new(&config.root).join("faults/retain-snapshot-writer.json"))?;
    if let Some(fault) = &selected
        && (config.mode != "film-measurement"
            || fault.sequence == 0
            || fault.incarnation.len() != 32
            || !fault
                .incarnation
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    {
        return Err(ErrorCode::Uncertain);
    }
    Ok(selected)
}
pub(crate) fn validate_mode(config: &Config) -> Result<(), ErrorCode> {
    writer_fault(config).map(|_| ())
}
pub(crate) fn retain_snapshot_writer(config: &Config, record: &Record) -> Result<bool, ErrorCode> {
    let Some(selected) = writer_fault(config)? else {
        return Ok(false);
    };
    if selected.incarnation != record.receipt.incarnation
        || selected.sequence != record.receipt.sequence
    {
        return Ok(false);
    }
    if !record.film.as_ref().is_some_and(|film| {
        matches!(
            film.grant.fixture.source,
            crate::film::Source::DevelopmentTiff { .. }
        )
    }) {
        return Err(ErrorCode::Uncertain);
    }
    Ok(true)
}

pub(crate) fn at(config: &Config, record: &Record, phase: Phase) -> Result<(), ErrorCode> {
    validate_mode(config)?;
    let directory = Path::new(&config.root).join("faults");
    let Some(arm) = read::<Arm>(&directory.join("arm.json"))? else {
        return Ok(());
    };
    if !["qualification", "film-measurement"].contains(&config.mode.as_str()) {
        return Err(ErrorCode::Uncertain);
    }
    if arm.phase != phase
        || arm.incarnation != record.receipt.incarnation
        || arm.sequence != record.receipt.sequence
    {
        return Ok(());
    }
    let marker = Marker {
        phase,
        incarnation: arm.incarnation,
        sequence: arm.sequence,
        launch_id: record.launch_id.clone(),
    };
    let bytes = serde_json::to_vec(&marker).map_err(|_| ErrorCode::Uncertain)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("marker.next"))
        .map_err(|_| ErrorCode::Uncertain)?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| ErrorCode::Uncertain)?;
    fs::rename(directory.join("marker.next"), directory.join("marker.json"))
        .map_err(|_| ErrorCode::Uncertain)?;
    File::open(&directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| ErrorCode::Uncertain)?;
    while now()? < record.receipt.deadline_unix_ms {
        if let Some(release) = read::<Marker>(&directory.join("release.json"))? {
            if release != marker {
                return Err(ErrorCode::Uncertain);
            }
            for name in ["release.json", "marker.json", "arm.json"] {
                fs::remove_file(directory.join(name)).map_err(|_| ErrorCode::Uncertain)?;
            }
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(ErrorCode::Uncertain)
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, ErrorCode> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ErrorCode::Uncertain),
    };
    let metadata = file.metadata().map_err(|_| ErrorCode::Uncertain)?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 1024
    {
        return Err(ErrorCode::Uncertain);
    }
    let mut bytes = Vec::new();
    file.take(1025)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::Uncertain)?;
    if bytes.len() > 1024 {
        return Err(ErrorCode::Uncertain);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| ErrorCode::Uncertain)
}
