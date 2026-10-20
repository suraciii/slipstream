//! Persistence row codecs for the scan-published state: the pure string
//! translations between SQLite columns and the domain values a scan
//! publishes and recovers. Every reader and writer in this module owns its
//! own error mapping, so these helpers preserve the exact `rusqlite`
//! result they return.

use crate::{
    CaptureFact, CaptureMetadataState, CaptureTimeField, OriginalErrorCategory, OriginalKind,
    PreviewState, SelectionState,
};

pub(super) fn parse_kind(value: &str) -> rusqlite::Result<OriginalKind> {
    match value {
        "raw" => Ok(OriginalKind::Raw),
        "jpeg" => Ok(OriginalKind::Jpeg),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub(super) fn parse_error_category(
    value: Option<String>,
) -> rusqlite::Result<Option<OriginalErrorCategory>> {
    value
        .map(|value| match value.as_str() {
            "unreadable" => Ok(OriginalErrorCategory::Unreadable),
            "changed" => Ok(OriginalErrorCategory::Changed),
            _ => Err(rusqlite::Error::InvalidQuery),
        })
        .transpose()
}

pub(super) fn capture_state_name(state: CaptureMetadataState) -> &'static str {
    match state {
        CaptureMetadataState::Pending => "pending",
        CaptureMetadataState::Known => "known",
        CaptureMetadataState::Missing => "missing",
        CaptureMetadataState::Invalid => "invalid",
        CaptureMetadataState::Failed => "failed",
    }
}

pub(super) fn camera_identity_name(identity: &crate::CameraIdentity) -> &'static str {
    match identity {
        crate::CameraIdentity::Pending => "pending",
        crate::CameraIdentity::Observed { .. } => "observed",
    }
}

pub(super) fn parse_camera_identity(
    state: String,
    make: Option<String>,
    model: Option<String>,
) -> rusqlite::Result<crate::CameraIdentity> {
    match state.as_str() {
        "pending" if make.is_none() && model.is_none() => Ok(crate::CameraIdentity::Pending),
        "observed" => Ok(crate::CameraIdentity::Observed { make, model }),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub(super) fn parse_capture_fact(
    state: String,
    order_key: Option<String>,
    field: Option<String>,
    offset_minutes: Option<i64>,
    source_revision: Option<String>,
    identity: crate::CameraIdentity,
) -> rusqlite::Result<CaptureFact> {
    let state = match state.as_str() {
        "pending" => CaptureMetadataState::Pending,
        "known" => CaptureMetadataState::Known,
        "missing" => CaptureMetadataState::Missing,
        "invalid" => CaptureMetadataState::Invalid,
        "failed" => CaptureMetadataState::Failed,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let field = match field.as_deref() {
        None => None,
        Some(value) => Some(
            CaptureTimeField::parse_database_name(value).ok_or(rusqlite::Error::InvalidQuery)?,
        ),
    };
    let offset_minutes = offset_minutes
        .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
        .transpose()?;
    let fact = CaptureFact {
        state,
        order_key,
        field,
        offset_minutes,
        source_revision,
        identity,
    };
    validate_capture_fact(&fact).map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(fact)
}

fn valid_capture_order_key(value: &str) -> bool {
    value.len() == 29
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value.as_bytes()[10] == b'T'
        && value.as_bytes()[13] == b':'
        && value.as_bytes()[16] == b':'
        && value.as_bytes()[19] == b'.'
        && value.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()
        })
}

pub(super) fn validate_capture_fact(fact: &CaptureFact) -> Result<(), ()> {
    let source_revision = fact
        .source_revision
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    let known = fact
        .order_key
        .as_deref()
        .is_some_and(valid_capture_order_key)
        && fact.field.is_some()
        && source_revision;
    let no_derived =
        fact.order_key.is_none() && fact.field.is_none() && fact.offset_minutes.is_none();
    // Only a completed inspection can carry a camera identity. Rows written
    // before the identity column existed keep `pending` identities with
    // their completed capture states until the next scan re-inspects and
    // publishes them.
    let identity_consistent = match fact.state {
        CaptureMetadataState::Pending | CaptureMetadataState::Failed => {
            !fact.identity.is_observed()
        }
        CaptureMetadataState::Known
        | CaptureMetadataState::Missing
        | CaptureMetadataState::Invalid => true,
    };
    match fact.state {
        CaptureMetadataState::Pending => no_derived && fact.source_revision.is_none(),
        CaptureMetadataState::Known => known,
        CaptureMetadataState::Missing | CaptureMetadataState::Invalid => {
            no_derived && source_revision
        }
        CaptureMetadataState::Failed => no_derived,
    }
    .then_some(())
    .ok_or(())?;
    identity_consistent.then_some(()).ok_or(())
}

pub(super) fn parse_preview_state(value: &str) -> rusqlite::Result<PreviewState> {
    match value {
        "inspection-pending" => Ok(PreviewState::InspectionPending),
        "ready" => Ok(PreviewState::Ready),
        "failed" => Ok(PreviewState::Failed),
        "unavailable" => Ok(PreviewState::Unavailable),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub(super) fn preview_state_name(state: PreviewState) -> &'static str {
    match state {
        PreviewState::InspectionPending => "inspection-pending",
        PreviewState::Ready => "ready",
        PreviewState::Failed => "failed",
        PreviewState::Unavailable => "unavailable",
    }
}

pub(super) fn parse_selection_state(value: &str) -> rusqlite::Result<SelectionState> {
    match value {
        "unflagged" => Ok(SelectionState::Unflagged),
        "picked" => Ok(SelectionState::Picked),
        "rejected" => Ok(SelectionState::Rejected),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub(super) fn selection_state_value(value: SelectionState) -> &'static str {
    match value {
        SelectionState::Unflagged => "unflagged",
        SelectionState::Picked => "picked",
        SelectionState::Rejected => "rejected",
    }
}

pub(super) fn parse_dimension(value: Option<i64>) -> rusqlite::Result<Option<u32>> {
    value
        .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
        .transpose()
}
