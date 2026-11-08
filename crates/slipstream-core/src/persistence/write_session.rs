//! Admitted mutation ownership: one session owns the transaction and the
//! process-local version journal, so SQL changes and published mutation
//! versions commit or roll back together.

use super::admission::{DatabaseName, StateDirectory};
use super::errors::{MutationError, PersistenceError};
use super::identity::random_uuid_v4;
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::collections::HashMap;

pub(super) struct MutationVersions {
    pub(super) epoch: String,
    pub(super) photo: HashMap<String, u64>,
    pub(super) album: HashMap<String, u64>,
}

impl MutationVersions {
    pub(super) fn new() -> Result<Self, PersistenceError> {
        Ok(Self {
            epoch: random_uuid_v4()?,
            photo: HashMap::new(),
            album: HashMap::new(),
        })
    }

    pub(super) fn photo(&self, id: &str) -> String {
        self.token("photo", id, *self.photo.get(id).unwrap_or(&0))
    }

    pub(super) fn album(&self, id: &str) -> String {
        self.token("album", id, *self.album.get(id).unwrap_or(&0))
    }

    pub(super) fn can_advance_photo(&self, id: &str) -> bool {
        self.photo.get(id).copied().unwrap_or(0) < u64::MAX
    }

    pub(super) fn can_advance_album(&self, id: &str) -> bool {
        self.album.get(id).copied().unwrap_or(0) < u64::MAX
    }

    pub(super) fn advance_photo(&mut self, id: &str) -> Result<(), MutationError> {
        advance_counter(&mut self.photo, id)
    }

    pub(super) fn advance_album(&mut self, id: &str) -> Result<(), MutationError> {
        advance_counter(&mut self.album, id)
    }

    pub(super) fn token(&self, kind: &str, id: &str, counter: u64) -> String {
        format!("{}:{kind}:{id}:{counter}", self.epoch)
    }
}

fn advance_counter(counters: &mut HashMap<String, u64>, id: &str) -> Result<(), MutationError> {
    let counter = counters.entry(id.to_owned()).or_default();
    *counter = counter.checked_add(1).ok_or(MutationError::Persistence)?;
    Ok(())
}

#[derive(Default)]
struct VersionJournal {
    photo: HashMap<String, Option<u64>>,
    album: HashMap<String, Option<u64>>,
}

impl VersionJournal {
    fn record_photo(&mut self, versions: &MutationVersions, id: &str) {
        self.photo
            .entry(id.to_owned())
            .or_insert_with(|| versions.photo.get(id).copied());
    }

    fn record_album(&mut self, versions: &MutationVersions, id: &str) {
        self.album
            .entry(id.to_owned())
            .or_insert_with(|| versions.album.get(id).copied());
    }

    fn rollback(&mut self, versions: &mut MutationVersions) {
        for (id, prior) in self.photo.drain() {
            match prior {
                Some(counter) => {
                    versions.photo.insert(id, counter);
                }
                None => {
                    versions.photo.remove(&id);
                }
            }
        }
        for (id, prior) in self.album.drain() {
            match prior {
                Some(counter) => {
                    versions.album.insert(id, counter);
                }
                None => {
                    versions.album.remove(&id);
                }
            }
        }
    }

    fn clear(&mut self) {
        self.photo.clear();
        self.album.clear();
    }
}

pub(super) struct SessionVersions<'a> {
    versions: &'a mut MutationVersions,
    journal: &'a mut VersionJournal,
}

impl SessionVersions<'_> {
    pub(super) fn can_advance_photo(&self, id: &str) -> bool {
        self.versions.can_advance_photo(id)
    }

    pub(super) fn can_advance_album(&self, id: &str) -> bool {
        self.versions.can_advance_album(id)
    }

    pub(super) fn advance_photo(&mut self, id: &str) -> Result<(), MutationError> {
        self.journal.record_photo(self.versions, id);
        self.versions.advance_photo(id)
    }

    pub(super) fn advance_photo_if_changed(
        &mut self,
        id: &str,
        changed: bool,
    ) -> Result<(), MutationError> {
        if changed {
            self.advance_photo(id)
        } else {
            Ok(())
        }
    }

    pub(super) fn advance_album(&mut self, id: &str) -> Result<(), MutationError> {
        self.journal.record_album(self.versions, id);
        self.versions.advance_album(id)
    }

    pub(super) fn remove_album(&mut self, id: &str) {
        self.journal.record_album(self.versions, id);
        self.versions.album.remove(id);
    }

    pub(super) fn photo_token(&self, id: &str) -> String {
        self.versions.photo(id)
    }
}

/// One owner-thread mutation scope. The journal is rolled back when the
/// operation fails or SQLite refuses the commit, keeping the process-local
/// version tokens aligned with the durable database state.
pub(super) struct WriteSession<'a> {
    state: &'a StateDirectory,
    database_name: &'a DatabaseName,
    connection: &'a mut Connection,
    versions: &'a mut MutationVersions,
    journal: VersionJournal,
}

impl<'a> WriteSession<'a> {
    pub(super) fn new(
        state: &'a StateDirectory,
        database_name: &'a DatabaseName,
        connection: &'a mut Connection,
        versions: &'a mut MutationVersions,
    ) -> Self {
        Self {
            state,
            database_name,
            connection,
            versions,
            journal: VersionJournal::default(),
        }
    }

    pub(super) fn connection(&self) -> &Connection {
        self.connection
    }

    pub(super) fn versions(&self) -> &MutationVersions {
        self.versions
    }

    pub(super) fn require_photo_capacity(&self, id: &str) -> Result<(), MutationError> {
        if self.can_advance_photo(id) {
            Ok(())
        } else {
            Err(MutationError::Persistence)
        }
    }

    pub(super) fn can_advance_photo(&self, id: &str) -> bool {
        self.versions.can_advance_photo(id)
    }

    pub(super) fn can_advance_album(&self, id: &str) -> bool {
        self.versions.can_advance_album(id)
    }

    pub(super) fn album_token(&self, id: &str) -> String {
        self.versions.album(id)
    }

    pub(super) fn mutation<T>(
        &mut self,
        operation: impl FnOnce(&Transaction<'_>, &mut SessionVersions<'_>) -> Result<T, MutationError>,
    ) -> Result<T, MutationError> {
        self.state
            .admit_sidecars(self.database_name)
            .map_err(|_| MutationError::Persistence)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| MutationError::Persistence)?;
        let result = {
            let mut versions = SessionVersions {
                versions: self.versions,
                journal: &mut self.journal,
            };
            operation(&transaction, &mut versions)
        };
        match result {
            Ok(value) => match transaction.commit() {
                Ok(()) => {
                    self.journal.clear();
                    Ok(value)
                }
                Err(_) => {
                    self.journal.rollback(self.versions);
                    Err(MutationError::Persistence)
                }
            },
            Err(error) => {
                drop(transaction);
                self.journal.rollback(self.versions);
                Err(error)
            }
        }
    }
}
