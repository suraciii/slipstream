//! Development Proxy persistence: one immutable, rebuildable proxy record
//! per Photo. The artifact bytes live in service-owned storage outside the
//! Library Folder; this table is the identity and publication record that
//! makes the artifact findable, current, or invalidated.

use super::{DatabaseName, PersistenceError, StateDirectory, owner::write_transaction};
use crate::DevelopmentProxyRecord;
use rusqlite::{Connection, OptionalExtension, params};

/// Reads the recorded Development Proxy of one Photo. A read failure is a
/// persistence error, not an absent proxy.
pub(super) fn read_development_proxy(
    connection: &Connection,
    photo_id: &str,
) -> Result<Option<DevelopmentProxyRecord>, PersistenceError> {
    connection
        .query_row(
            "SELECT p.source_revision,p.source_relative_path,p.source_sha256,p.source_size,
                    p.profile_id,p.pipeline_version,p.bundle_sha256,p.long_edge,
                    p.width,p.height,p.artifact_sha256,p.artifact_bytes,p.created_at
             FROM development_proxies p WHERE p.photo_id=?",
            [photo_id],
            |row| {
                Ok(DevelopmentProxyRecord {
                    photo_id: photo_id.to_owned(),
                    source_revision: row.get(0)?,
                    source_relative_path: row.get(1)?,
                    source_sha256: row.get(2)?,
                    source_size: u64::try_from(row.get::<_, i64>(3)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    profile_id: row.get(4)?,
                    pipeline_version: row.get(5)?,
                    bundle_sha256: row.get(6)?,
                    long_edge: u32::try_from(row.get::<_, i64>(7)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    width: u32::try_from(row.get::<_, i64>(8)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    height: u32::try_from(row.get::<_, i64>(9)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    artifact_sha256: row.get(10)?,
                    artifact_bytes: u64::try_from(row.get::<_, i64>(11)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    created_at: u64::try_from(row.get::<_, i64>(12)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)
}

/// Every recorded Development Proxy, for startup reconciliation against the
/// artifacts the service actually owns.
pub(super) fn all_development_proxies(
    connection: &Connection,
) -> Result<Vec<DevelopmentProxyRecord>, PersistenceError> {
    let mut statement = connection
        .prepare(
            "SELECT p.photo_id,p.source_revision,p.source_relative_path,p.source_sha256,
                    p.source_size,p.profile_id,p.pipeline_version,p.bundle_sha256,p.long_edge,
                    p.width,p.height,p.artifact_sha256,p.artifact_bytes,p.created_at
             FROM development_proxies p",
        )
        .map_err(|_| PersistenceError::Storage)?;
    let rows = statement
        .query_map([], |row| {
            Ok(DevelopmentProxyRecord {
                photo_id: row.get(0)?,
                source_revision: row.get(1)?,
                source_relative_path: row.get(2)?,
                source_sha256: row.get(3)?,
                source_size: u64::try_from(row.get::<_, i64>(4)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                profile_id: row.get(5)?,
                pipeline_version: row.get(6)?,
                bundle_sha256: row.get(7)?,
                long_edge: u32::try_from(row.get::<_, i64>(8)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                width: u32::try_from(row.get::<_, i64>(9)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                height: u32::try_from(row.get::<_, i64>(10)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                artifact_sha256: row.get(11)?,
                artifact_bytes: u64::try_from(row.get::<_, i64>(12)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                created_at: u64::try_from(row.get::<_, i64>(13)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
            })
        })
        .map_err(|_| PersistenceError::Storage)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)
}

/// Validates one record against the table's closed shape before any write.
/// An out-of-shape record is a caller bug, refused as invalid input rather
/// than persisted as a proxy no reader can trust.
fn valid_record(record: &DevelopmentProxyRecord) -> bool {
    !record.photo_id.is_empty()
        && !record.source_revision.is_empty()
        && !record.source_relative_path.is_empty()
        && record.source_sha256.len() == 64
        && record.source_size > 0
        && (1..=64).contains(&record.profile_id.len())
        && (1..=32).contains(&record.pipeline_version.len())
        && record.bundle_sha256.len() == 64
        && record.long_edge > 0
        && record.width > 0
        && record.height > 0
        && record.artifact_sha256.len() == 64
        && record.artifact_bytes > 0
}

/// Records the publication of one Development Proxy. The caller must have
/// already published the artifact file durably: this row is the last step of
/// publication, so a crash before it leaves an orphan artifact the startup
/// reconciliation removes, never a row without its artifact.
pub(super) fn record_development_proxy(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    record: DevelopmentProxyRecord,
) -> Result<bool, PersistenceError> {
    if !valid_record(&record) {
        return Ok(false);
    }
    write_transaction(state, database_name, connection, |transaction| {
        upsert_development_proxy_row(transaction, &record)?;
        Ok(true)
    })
}

/// Removes the Development Proxy record of one Photo. Returns whether a
/// recorded proxy was removed; the caller owns removing the artifact file
/// after the row is gone.
pub(super) fn remove_development_proxy(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    photo_id: &str,
) -> Result<bool, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        let removed = transaction
            .execute(
                "DELETE FROM development_proxies WHERE photo_id=?",
                [photo_id],
            )
            .map_err(|_| PersistenceError::Storage)?;
        Ok(removed > 0)
    })
}

/// The shared upsert of the one proxy row per Photo. A rebuild of the same
/// identity and a superseding rebuild both land here; the row is always the
/// identity the artifact file was published under.
fn upsert_development_proxy_row(
    transaction: &rusqlite::Transaction<'_>,
    record: &DevelopmentProxyRecord,
) -> Result<(), PersistenceError> {
    transaction
        .execute(
            "INSERT INTO development_proxies(
               photo_id,source_revision,source_relative_path,source_sha256,source_size,
               profile_id,pipeline_version,bundle_sha256,long_edge,width,height,
               artifact_sha256,artifact_bytes,created_at)
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)
             ON CONFLICT(photo_id) DO UPDATE SET
               source_revision=excluded.source_revision,
               source_relative_path=excluded.source_relative_path,
               source_sha256=excluded.source_sha256,
               source_size=excluded.source_size,
               profile_id=excluded.profile_id,
               pipeline_version=excluded.pipeline_version,
               bundle_sha256=excluded.bundle_sha256,
               long_edge=excluded.long_edge,
               width=excluded.width,
               height=excluded.height,
               artifact_sha256=excluded.artifact_sha256,
               artifact_bytes=excluded.artifact_bytes,
               created_at=excluded.created_at",
            params![
                record.photo_id,
                record.source_revision,
                record.source_relative_path,
                record.source_sha256,
                i64::try_from(record.source_size).map_err(|_| PersistenceError::Storage)?,
                record.profile_id,
                record.pipeline_version,
                record.bundle_sha256,
                i64::from(record.long_edge),
                i64::from(record.width),
                i64::from(record.height),
                record.artifact_sha256,
                i64::try_from(record.artifact_bytes).map_err(|_| PersistenceError::Storage)?,
                i64::try_from(record.created_at).map_err(|_| PersistenceError::Storage)?,
            ],
        )
        .map(|_| ())
        .map_err(|_| PersistenceError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_support::{RecipeTestPhoto, add_recipe_test_photo, fixture};
    use rusqlite::Connection;

    fn record(photo_id: &str, source_revision: &str) -> DevelopmentProxyRecord {
        DevelopmentProxyRecord {
            photo_id: photo_id.to_owned(),
            source_revision: source_revision.to_owned(),
            source_relative_path: "camera/raw.ARW".to_owned(),
            source_sha256: "a".repeat(64),
            source_size: 1024,
            profile_id: "sony-ilce-7rm5-arw".to_owned(),
            pipeline_version: crate::DEVELOPMENT_PROXY_PIPELINE_VERSION.to_owned(),
            bundle_sha256: "b".repeat(64),
            long_edge: crate::DEVELOPMENT_PROXY_LONG_EDGE,
            width: 2560,
            height: 1707,
            artifact_sha256: "c".repeat(64),
            artifact_bytes: 2048,
            created_at: 1_700_000_000,
        }
    }

    /// One migrated database with a Photo row, plus the opened persistence
    /// owner, its state directory and database name, and a private
    /// connection for exercising this module directly.
    #[allow(clippy::type_complexity)]
    async fn opened() -> (
        crate::persistence::test_support::TempTree,
        crate::persistence::admission::StateDirectory,
        crate::persistence::admission::DatabaseName,
        crate::persistence::Persistence,
        Connection,
    ) {
        let (base, library, state, name, path) = fixture();
        crate::persistence::test_support::seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v6.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "raw-original",
                photo_id: "photo",
                relative_path: "shoot/one.ARW",
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
        drop(connection);
        let persistence = crate::persistence::Persistence::open(
            crate::persistence::admission::StateDirectory::open_or_create(
                &library,
                state.canonical_path(),
            )
            .unwrap(),
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let direct = Connection::open(&path).unwrap();
        (base, state, name, persistence, direct)
    }

    #[tokio::test]
    async fn record_read_remove_follow_the_row_contract() {
        let (base, state, name, persistence, mut connection) = opened().await;
        assert_eq!(
            read_development_proxy(&connection, "photo").unwrap(),
            None,
            "no proxy is recorded before any publication"
        );
        assert!(
            record_development_proxy(&state, &name, &mut connection, record("photo", "rev-1"))
                .unwrap()
        );
        let stored = read_development_proxy(&connection, "photo")
            .unwrap()
            .expect("the publication is recorded");
        assert_eq!(stored.source_revision, "rev-1");
        assert_eq!(stored.identity_digest().len(), 64);
        // A superseding publication replaces the one row per Photo.
        assert!(
            record_development_proxy(&state, &name, &mut connection, record("photo", "rev-2"))
                .unwrap()
        );
        let superseding = read_development_proxy(&connection, "photo")
            .unwrap()
            .expect("the superseding publication is recorded");
        assert_eq!(superseding.source_revision, "rev-2");
        assert_eq!(all_development_proxies(&connection).unwrap().len(), 1);
        assert!(remove_development_proxy(&state, &name, &mut connection, "photo").unwrap());
        assert!(
            !remove_development_proxy(&state, &name, &mut connection, "photo").unwrap(),
            "removing again finds no row"
        );
        assert_eq!(read_development_proxy(&connection, "photo").unwrap(), None);
        persistence.shutdown().unwrap();
        drop(base);
    }

    #[tokio::test]
    async fn out_of_shape_records_are_refused_not_persisted() {
        let (base, state, name, persistence, mut connection) = opened().await;
        let mut broken = record("photo", "rev-1");
        broken.source_sha256 = "short".to_owned();
        assert!(!record_development_proxy(&state, &name, &mut connection, broken).unwrap());
        assert_eq!(read_development_proxy(&connection, "photo").unwrap(), None);
        persistence.shutdown().unwrap();
        drop(base);
    }

    #[test]
    fn currency_compares_every_identity_fact() {
        let stored = record("photo", "rev-1");
        let expected = crate::DevelopmentProxyExpectation {
            source_revision: &stored.source_revision,
            profile_id: &stored.profile_id,
            pipeline_version: &stored.pipeline_version,
            bundle_sha256: &stored.bundle_sha256,
        };
        assert!(stored.current_against(&expected));
        for stale in [
            crate::DevelopmentProxyExpectation {
                source_revision: "rev-2",
                profile_id: expected.profile_id,
                pipeline_version: expected.pipeline_version,
                bundle_sha256: expected.bundle_sha256,
            },
            crate::DevelopmentProxyExpectation {
                source_revision: expected.source_revision,
                profile_id: "sony-ilce-7cm2-arw",
                pipeline_version: expected.pipeline_version,
                bundle_sha256: expected.bundle_sha256,
            },
        ] {
            assert!(
                !stored.current_against(&stale),
                "a changed identity fact invalidates the proxy"
            );
        }
    }
}
