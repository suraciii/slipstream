//! Persistence and mutation error surfaces shared by the owner, its clients,
//! and the mutation session.

use super::admission::StateError;
use std::fmt;

#[derive(Clone, Debug)]
pub enum PersistenceError {
    Saturated,
    Closed,
    State(StateError),
    RecoveryRequired,
    UnsupportedSchema,
    NewerSchema,
    RootMismatch,
    InvalidLegacyData,
    InvalidExpansion,
    InvalidRecovery,
    InvalidRecoveryMapping {
        original_id: String,
        reason: &'static str,
    },
    IdCollision,
    Storage,
    OwnerStopped,
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Saturated => "SQLite persistence queue is saturated",
            Self::Closed => "SQLite persistence is closed",
            Self::State(error) => return error.fmt(formatter),
            Self::RecoveryRequired => "SQLite state requires operator recovery",
            Self::UnsupportedSchema => "SQLite schema is unsupported",
            Self::NewerSchema => "SQLite schema version is newer than this Slipstream build",
            Self::RootMismatch => "SQLite database belongs to a different Photo Library root",
            Self::InvalidLegacyData => "SQLite legacy data cannot be migrated safely",
            Self::InvalidExpansion => "Photo Library expansion could not be proven safely",
            Self::InvalidRecovery => "Original File recovery could not be proven safely",
            Self::InvalidRecoveryMapping { .. } => {
                "Original File recovery could not be proven safely"
            }
            Self::IdCollision => "SQLite identity allocation collided with existing state",
            Self::Storage => "SQLite persistence failed",
            Self::OwnerStopped => "SQLite persistence owner stopped unexpectedly",
        })
    }
}

impl std::error::Error for PersistenceError {}

impl From<StateError> for PersistenceError {
    fn from(value: StateError) -> Self {
        match value {
            StateError::SidecarPresent => Self::RecoveryRequired,
            other => Self::State(other),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationError {
    /// The request itself is malformed: an empty, over-limit, or duplicate
    /// address set. It is refused before any state is read or written.
    Invalid,
    NotFound,
    Conflict,
    Persistence,
    Saturated,
    Closed,
}

impl fmt::Display for MutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "Mutation request is not valid",
            Self::NotFound => "Mutation target not found",
            Self::Conflict => "Mutation conflicts with current state",
            Self::Persistence => "Mutation could not be persisted",
            Self::Saturated => "SQLite persistence queue is saturated",
            Self::Closed => "SQLite persistence is closed",
        })
    }
}

impl std::error::Error for MutationError {}
