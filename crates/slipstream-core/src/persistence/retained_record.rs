//! The bounded JSON substrate shared by every strict retained record in
//! persistence: one bounded serialize and one bounded strict parse, each
//! parameterized by the owning record family's byte bound. Both map every
//! failure to `PersistenceError::Storage`.
//!
//! This module holds no record-specific identity, digest, or state-shape
//! validation and no knowledge of which bound belongs to which family; each
//! family's `records` module keeps its own constant and passes it in, so the
//! bounds stay independently applied and independently owned.

use serde::{Deserialize, Serialize};

use super::owner::PersistenceError;

/// Serializes one record within the given metadata value bound. A value past
/// the bound is refused as a storage error rather than written oversized or
/// truncated; admitted records cannot reach it.
pub(super) fn serialize_record_within_bound<T: Serialize>(
    record: &T,
    maximum_bytes: usize,
) -> Result<String, PersistenceError> {
    let serialized = serde_json::to_string(record).map_err(|_| PersistenceError::Storage)?;
    if serialized.len() > maximum_bytes {
        return Err(PersistenceError::Storage);
    }
    Ok(serialized)
}

/// Parses one stored metadata value strictly: bounded by the given byte bound
/// and refusing unknown fields (unknown fields are refused by each record
/// shape's `#[serde(deny_unknown_fields)]`). Every failure — oversized,
/// malformed, or unknown-field — maps to `PersistenceError::Storage`.
pub(super) fn parse_stored_value_within_bound<T: for<'de> Deserialize<'de>>(
    value: &str,
    maximum_bytes: usize,
) -> Result<T, PersistenceError> {
    if value.len() > maximum_bytes {
        return Err(PersistenceError::Storage);
    }
    serde_json::from_str(value).map_err(|_| PersistenceError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    #[serde(deny_unknown_fields)]
    struct Shape {
        label: String,
    }

    #[test]
    fn value_within_bound_round_trips() {
        let serialized = serialize_record_within_bound(
            &Shape {
                label: "recipe".to_owned(),
            },
            64,
        )
        .unwrap();
        assert_eq!(
            parse_stored_value_within_bound::<Shape>(&serialized, 64).unwrap(),
            Shape {
                label: "recipe".to_owned(),
            }
        );
    }

    #[test]
    fn serialization_past_the_bound_is_a_storage_error() {
        let oversized = Shape {
            label: "x".repeat(65),
        };
        assert!(matches!(
            serialize_record_within_bound(&oversized, 64),
            Err(PersistenceError::Storage)
        ));
    }

    #[test]
    fn stored_value_past_the_bound_is_a_storage_error() {
        let oversized = format!("{{\"label\":\"{}\"}}", "x".repeat(65));
        assert!(matches!(
            parse_stored_value_within_bound::<Shape>(&oversized, 64),
            Err(PersistenceError::Storage)
        ));
    }

    #[test]
    fn malformed_stored_value_is_a_storage_error() {
        assert!(matches!(
            parse_stored_value_within_bound::<Shape>("{not json", 64),
            Err(PersistenceError::Storage)
        ));
        assert!(matches!(
            parse_stored_value_within_bound::<Shape>("", 64),
            Err(PersistenceError::Storage)
        ));
    }

    #[test]
    fn unknown_field_is_refused() {
        assert!(matches!(
            parse_stored_value_within_bound::<Shape>("{\"label\":\"x\",\"extra\":1}", 64),
            Err(PersistenceError::Storage)
        ));
    }

    #[test]
    fn bound_is_applied_exactly_not_off_by_one() {
        // A value whose length equals the bound is admitted; one byte past is
        // refused. This pins the boundary so the wrappers cannot drift.
        let exact = "{\"label\":\"x\"}";
        assert!(
            serialize_record_within_bound(
                &parse_stored_value_within_bound::<Shape>(exact, exact.len()).unwrap(),
                exact.len(),
            )
            .is_ok()
        );
        let over = format!("{{\"label\":\"{}\"}}", "x".repeat(exact.len()));
        assert!(matches!(
            parse_stored_value_within_bound::<Shape>(&over, exact.len()),
            Err(PersistenceError::Storage)
        ));
    }
}
