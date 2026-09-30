//! Private owner of persisted cache metadata.
//!
//! Records crossing this boundary are bounded, schema-checked JSON values. In
//! particular, persisted modification times are restored from their exact bit
//! representation and cache keys are checked against the identity before a
//! record can influence scheduling or publication.

use super::{
    CACHE_RECORD_SCHEMA_VERSION, CacheError, DERIVATIVE_ALGORITHM_VERSION, DerivativeFailureKind,
    DerivativeIdentity, DerivativeProfile, DerivativeSource, DerivativeTarget,
    MAXIMUM_METADATA_BYTES, derivative_cache_key, is_hex_key, no_follow_flag,
};
use serde::{Deserialize, Serialize};
use std::{fs::OpenOptions, io::Read, path::Path};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ManifestFacts {
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Manifest {
    #[serde(rename = "schemaVersion")]
    pub(super) schema_version: u32,
    #[serde(rename = "algorithmVersion")]
    pub(super) algorithm_version: String,
    #[serde(rename = "photoIdentity")]
    pub(super) photo_identity: String,
    #[serde(rename = "targetLongEdge")]
    pub(super) target_long_edge: u32,
    pub(super) key: String,
    pub(super) source: DerivativeSource,
    #[serde(rename = "sourceRelativePath")]
    pub(super) source_relative_path: String,
    #[serde(rename = "sourceSize")]
    pub(super) source_size: u64,
    #[serde(rename = "sourceMtimeMs")]
    pub(super) source_mtime_ms: f64,
    #[serde(rename = "sourceMtimeBits")]
    pub(super) source_mtime_bits: u64,
    #[serde(rename = "embeddedCandidateIdentity")]
    pub(super) embedded_candidate_identity: Option<String>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) color_profile: DerivativeProfileWire,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum DerivativeProfileWire {
    Srgb,
    PreservedIcc,
}

impl From<DerivativeProfile> for DerivativeProfileWire {
    fn from(profile: DerivativeProfile) -> Self {
        match profile {
            DerivativeProfile::Srgb => Self::Srgb,
            DerivativeProfile::PreservedIcc => Self::PreservedIcc,
        }
    }
}

impl From<DerivativeProfileWire> for DerivativeProfile {
    fn from(profile: DerivativeProfileWire) -> Self {
        match profile {
            DerivativeProfileWire::Srgb => Self::Srgb,
            DerivativeProfileWire::PreservedIcc => Self::PreservedIcc,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FailureRecord {
    #[serde(rename = "schemaVersion")]
    pub(super) schema_version: u32,
    #[serde(rename = "algorithmVersion")]
    pub(super) algorithm_version: String,
    #[serde(rename = "photoIdentity")]
    pub(super) photo_identity: String,
    #[serde(rename = "targetLongEdge")]
    pub(super) target_long_edge: u32,
    pub(super) key: String,
    pub(super) source: DerivativeSource,
    #[serde(rename = "sourceRelativePath")]
    pub(super) source_relative_path: String,
    #[serde(rename = "sourceSize")]
    pub(super) source_size: u64,
    #[serde(rename = "sourceMtimeMs")]
    pub(super) source_mtime_ms: f64,
    #[serde(rename = "sourceMtimeBits")]
    pub(super) source_mtime_bits: u64,
    #[serde(rename = "embeddedCandidateIdentity")]
    pub(super) embedded_candidate_identity: Option<String>,
    pub(super) kind: PersistentFailureKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum PersistentFailureKind {
    Unsupported,
    Malformed,
    ResourceLimit,
}
pub(super) fn manifest_record(
    identity: &DerivativeIdentity,
    key: &str,
    width: u32,
    height: u32,
    profile: DerivativeProfile,
) -> Manifest {
    Manifest {
        schema_version: CACHE_RECORD_SCHEMA_VERSION,
        algorithm_version: DERIVATIVE_ALGORITHM_VERSION.to_owned(),
        photo_identity: identity.photo_identity.clone(),
        target_long_edge: identity.target.long_edge(),
        key: key.to_owned(),
        source: identity.source,
        source_relative_path: identity.source_relative_path.clone(),
        source_size: identity.source_size,
        source_mtime_ms: identity.source_mtime_ms,
        source_mtime_bits: identity.source_mtime_ms.to_bits(),
        embedded_candidate_identity: identity.embedded_candidate_identity.clone(),
        width,
        height,
        color_profile: profile.into(),
    }
}

pub(super) fn serialize_manifest(manifest: &Manifest) -> Result<Vec<u8>, CacheError> {
    serde_json::to_vec(manifest).map_err(|_| CacheError::Io)
}

pub(super) fn serialize_failure(failure: &FailureRecord) -> Result<Vec<u8>, CacheError> {
    serde_json::to_vec(failure).map_err(|_| CacheError::Io)
}

pub(super) fn read_manifest(path: &Path) -> Result<Manifest, CacheError> {
    let mut bytes = Vec::new();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(no_follow_flag())
        .open(path)
        .map_err(|_| CacheError::Io)?;
    let metadata = file.metadata().map_err(|_| CacheError::Io)?;
    if !metadata.is_file() || metadata.len() > MAXIMUM_METADATA_BYTES {
        return Err(CacheError::InvalidCachedDerivative);
    }
    file.take(MAXIMUM_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CacheError::Io)?;
    if bytes.len() as u64 > MAXIMUM_METADATA_BYTES {
        return Err(CacheError::InvalidCachedDerivative);
    }
    let mut manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|_| CacheError::InvalidCachedDerivative)?;
    let exact_mtime_ms = f64::from_bits(manifest.source_mtime_bits);
    if !exact_mtime_ms.is_finite() || exact_mtime_ms < 0.0 {
        return Err(CacheError::InvalidCachedDerivative);
    }
    manifest.source_mtime_ms = exact_mtime_ms;
    if manifest.schema_version != CACHE_RECORD_SCHEMA_VERSION
        || manifest.algorithm_version != DERIVATIVE_ALGORITHM_VERSION
        || manifest.width == 0
        || manifest.height == 0
        || !matches!(manifest.target_long_edge, 512 | 2560)
        || !is_hex_key(&manifest.key)
    {
        return Err(CacheError::InvalidCachedDerivative);
    }
    Ok(manifest)
}

pub(super) fn read_failure(
    path: &Path,
    identity: &DerivativeIdentity,
    expected_key: &str,
) -> Option<FailureRecord> {
    let mut bytes = Vec::new();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(no_follow_flag())
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAXIMUM_METADATA_BYTES {
        return None;
    }
    file.take(MAXIMUM_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAXIMUM_METADATA_BYTES {
        return None;
    }
    let mut failure: FailureRecord = serde_json::from_slice(&bytes).ok()?;
    let exact_mtime_ms = f64::from_bits(failure.source_mtime_bits);
    if !exact_mtime_ms.is_finite() || exact_mtime_ms < 0.0 {
        return None;
    }
    failure.source_mtime_ms = exact_mtime_ms;
    if failure.schema_version != CACHE_RECORD_SCHEMA_VERSION
        || failure.algorithm_version != DERIVATIVE_ALGORITHM_VERSION
        || failure.photo_identity != identity.photo_identity
        || failure.target_long_edge != identity.target.long_edge()
        || failure.source != identity.source
        || failure.source_relative_path != identity.source_relative_path
        || failure.source_size != identity.source_size
        || failure.source_mtime_ms != identity.source_mtime_ms
        || failure.embedded_candidate_identity != identity.embedded_candidate_identity
        || failure.key != expected_key
        || derivative_cache_key(identity).ok().as_deref() != Some(failure.key.as_str())
    {
        return None;
    }
    Some(failure)
}

pub(super) fn failure_record(
    identity: &DerivativeIdentity,
    key: &str,
    kind: DerivativeFailureKind,
) -> FailureRecord {
    FailureRecord {
        schema_version: CACHE_RECORD_SCHEMA_VERSION,
        algorithm_version: DERIVATIVE_ALGORITHM_VERSION.to_owned(),
        photo_identity: identity.photo_identity.clone(),
        target_long_edge: identity.target.long_edge(),
        key: key.to_owned(),
        source: identity.source,
        source_relative_path: identity.source_relative_path.clone(),
        source_size: identity.source_size,
        source_mtime_ms: identity.source_mtime_ms,
        source_mtime_bits: identity.source_mtime_ms.to_bits(),
        embedded_candidate_identity: identity.embedded_candidate_identity.clone(),
        kind: match kind {
            DerivativeFailureKind::Unsupported => PersistentFailureKind::Unsupported,
            DerivativeFailureKind::Malformed => PersistentFailureKind::Malformed,
            DerivativeFailureKind::ResourceLimit => PersistentFailureKind::ResourceLimit,
            _ => PersistentFailureKind::Malformed,
        },
    }
}

pub(super) fn manifest_matches_identity(
    manifest: &Manifest,
    identity: &DerivativeIdentity,
) -> bool {
    manifest.schema_version == CACHE_RECORD_SCHEMA_VERSION
        && manifest.algorithm_version == DERIVATIVE_ALGORITHM_VERSION
        && manifest.photo_identity == identity.photo_identity
        && manifest.target_long_edge == identity.target.long_edge()
        && manifest.source == identity.source
        && manifest.source_relative_path == identity.source_relative_path
        && manifest.source_size == identity.source_size
        && manifest.source_mtime_ms == identity.source_mtime_ms
        && manifest.embedded_candidate_identity == identity.embedded_candidate_identity
        && derivative_cache_key(identity).ok().as_deref() == Some(manifest.key.as_str())
}

pub(super) fn manifest_is_for_photo_target(
    manifest: &Manifest,
    identity: &DerivativeIdentity,
) -> bool {
    manifest.schema_version == CACHE_RECORD_SCHEMA_VERSION
        && manifest.algorithm_version == DERIVATIVE_ALGORITHM_VERSION
        && manifest.photo_identity == identity.photo_identity
        && manifest.target_long_edge == identity.target.long_edge()
        && manifest_key_is_valid(manifest)
}

pub(super) fn manifest_is_stale_for_identity(
    manifest: &Manifest,
    identity: &DerivativeIdentity,
) -> bool {
    manifest_is_for_photo_target(manifest, identity)
        && (manifest.source_relative_path != identity.source_relative_path
            || manifest.source_size != identity.source_size
            || manifest.source_mtime_ms != identity.source_mtime_ms
            || manifest.source != identity.source
            || manifest.embedded_candidate_identity != identity.embedded_candidate_identity)
}

pub(super) fn manifest_key_is_valid(manifest: &Manifest) -> bool {
    let target = match manifest.target_long_edge {
        512 => DerivativeTarget::Thumbnail512,
        2560 => DerivativeTarget::Review2560,
        _ => return false,
    };
    let identity = DerivativeIdentity {
        photo_identity: manifest.photo_identity.clone(),
        source: manifest.source,
        source_relative_path: manifest.source_relative_path.clone(),
        source_size: manifest.source_size,
        source_mtime_ms: manifest.source_mtime_ms,
        embedded_candidate_identity: manifest.embedded_candidate_identity.clone(),
        target,
    };
    derivative_cache_key(&identity).ok().as_deref() == Some(manifest.key.as_str())
}
