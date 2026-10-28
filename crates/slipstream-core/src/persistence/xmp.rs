//! Historical XMP snapshots remain readable; generic processing no longer generates them.
use super::owner::{Command, Persistence, PersistenceError};
use crate::{WhiteBalanceIntent, XmpExportRecord};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

type PhotoXmpExportsReceiver =
    oneshot::Receiver<Result<Option<Vec<XmpExportRecord>>, PersistenceError>>;

impl Persistence {
    pub(crate) fn read_xmp_receiver(
        &self,
        export_id: &str,
    ) -> Result<
        oneshot::Receiver<Result<Option<XmpExportRecord>, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadXmp {
            export_id: export_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }
    pub(crate) fn list_xmp_receiver(
        &self,
        photo_id: &str,
    ) -> Result<PhotoXmpExportsReceiver, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ListPhotoXmp {
            photo_id: photo_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }
}

const RETAINED_PREFIX: &str = "retained_xmp_export:";
const COLUMNS: &str = "id,photo_id,recipe_revision,source_revision,created_at,expires_at,white_balance_mode,temperature_kelvin,tint_milli,exposure_ev,filename,document,byte_length,sha256";

fn balance(
    mode: &str,
    temp: Option<i32>,
    tint: Option<i32>,
) -> rusqlite::Result<WhiteBalanceIntent> {
    match (mode, temp, tint) {
        ("as-shot", None, None) => Ok(WhiteBalanceIntent::AsShot),
        ("temperature-tint", Some(temperature_kelvin), Some(tint_milli)) => {
            Ok(WhiteBalanceIntent::TemperatureTint {
                temperature_kelvin,
                tint_milli,
            })
        }
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}
fn record(row: &rusqlite::Row<'_>) -> rusqlite::Result<XmpExportRecord> {
    let mode: String = row.get(6)?;
    let document: Option<Vec<u8>> = row.get(11)?;
    let byte_length =
        usize::try_from(row.get::<_, i64>(12)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let sha256: String = row.get(13)?;
    if document.is_none() && row.get::<_, i64>(5)? > super::export::export_unix_seconds() as i64 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let document = document.unwrap_or_default();
    // Expiry tombstones intentionally retain the original byte length and
    // digest while releasing the immutable document blob.
    if !document.is_empty()
        && (document.len() != byte_length || crate::xmp::digest(&document) != sha256)
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(XmpExportRecord {
        export_id: row.get(0)?,
        photo_id: row.get(1)?,
        recipe_version: row.get(2)?,
        source_revision: row.get(3)?,
        created_at: row.get(4)?,
        expires_at: row.get(5)?,
        white_balance: balance(&mode, row.get(7)?, row.get(8)?)?,
        exposure_ev: row.get(9)?,
        filename: row.get(10)?,
        document,
        byte_length,
        sha256,
    })
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetainedXmp {
    export_id: String,
    photo_id: String,
    recipe_revision: String,
    source_revision: String,
    created_at: i64,
    expires_at: i64,
    white_balance_mode: String,
    temperature_kelvin: Option<i32>,
    tint_milli: Option<i32>,
    exposure_ev: f64,
    filename: String,
    byte_length: i64,
    sha256: String,
}

impl RetainedXmp {
    fn into_record(self) -> Result<XmpExportRecord, PersistenceError> {
        let byte_length =
            usize::try_from(self.byte_length).map_err(|_| PersistenceError::Storage)?;
        if self.expires_at < self.created_at || byte_length == 0 || self.sha256.len() != 64 {
            return Err(PersistenceError::Storage);
        }
        let white_balance = match (
            self.white_balance_mode.as_str(),
            self.temperature_kelvin,
            self.tint_milli,
        ) {
            ("as-shot", None, None) => WhiteBalanceIntent::AsShot,
            ("temperature-tint", Some(temperature_kelvin), Some(tint_milli)) => {
                WhiteBalanceIntent::TemperatureTint {
                    temperature_kelvin,
                    tint_milli,
                }
            }
            _ => return Err(PersistenceError::Storage),
        };
        Ok(XmpExportRecord {
            export_id: self.export_id,
            photo_id: self.photo_id,
            recipe_version: self.recipe_revision,
            source_revision: self.source_revision,
            created_at: self.created_at,
            expires_at: self.expires_at,
            byte_length,
            sha256: self.sha256,
            filename: self.filename,
            document: Vec::new(),
            exposure_ev: self.exposure_ev,
            white_balance,
        })
    }
}

/// Preserve an expired snapshot's provenance before the Photo row is removed.
/// The transient document bytes are already swept; no Photo FK remains.
pub(super) fn archive_for_photo(
    transaction: &Transaction<'_>,
    photo_id: &str,
) -> Result<(), PersistenceError> {
    let mut statement = transaction
        .prepare(
            "SELECT id,photo_id,recipe_revision,source_revision,created_at,expires_at,
                    white_balance_mode,temperature_kelvin,tint_milli,exposure_ev,
                    filename,byte_length,sha256
             FROM xmp_exports WHERE photo_id=? AND expires_at <= ?",
        )
        .map_err(|_| PersistenceError::Storage)?;
    let now = super::export::export_unix_seconds() as i64;
    let retained = statement
        .query_map(params![photo_id, now], |row| {
            Ok(RetainedXmp {
                export_id: row.get(0)?,
                photo_id: row.get(1)?,
                recipe_revision: row.get(2)?,
                source_revision: row.get(3)?,
                created_at: row.get(4)?,
                expires_at: row.get(5)?,
                white_balance_mode: row.get(6)?,
                temperature_kelvin: row.get(7)?,
                tint_milli: row.get(8)?,
                exposure_ev: row.get(9)?,
                filename: row.get(10)?,
                byte_length: row.get(11)?,
                sha256: row.get(12)?,
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    for snapshot in retained {
        let value = serde_json::to_string(&snapshot).map_err(|_| PersistenceError::Storage)?;
        super::metadata_store::upsert(
            transaction,
            &format!("{RETAINED_PREFIX}{}", snapshot.export_id),
            &value,
            |_| PersistenceError::Storage,
        )?;
    }
    Ok(())
}

pub(super) fn read(
    connection: &Connection,
    id: &str,
) -> Result<Option<XmpExportRecord>, PersistenceError> {
    let live = connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM xmp_exports WHERE id=?"),
            [id],
            record,
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if live.is_some() {
        return Ok(live);
    }
    let Some(value) =
        super::metadata_store::read_value(connection, &format!("{RETAINED_PREFIX}{id}"), |_| {
            PersistenceError::Storage
        })?
    else {
        return Ok(None);
    };
    let retained: RetainedXmp =
        serde_json::from_str(&value).map_err(|_| PersistenceError::Storage)?;
    if retained.export_id != id {
        return Err(PersistenceError::Storage);
    }
    retained.into_record().map(Some)
}
pub(super) fn list(
    connection: &Connection,
    photo_id: &str,
) -> Result<Option<Vec<XmpExportRecord>>, PersistenceError> {
    let exists = connection
        .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |r| {
            r.get::<_, i64>(0)
        })
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if exists.is_none() {
        return Ok(None);
    }
    let mut statement = connection
        .prepare(&format!(
            "SELECT {COLUMNS} FROM xmp_exports
             WHERE photo_id=? ORDER BY created_at DESC,id DESC LIMIT 64"
        ))
        .map_err(|_| PersistenceError::Storage)?;
    let rows = statement
        .query_map(params![photo_id], record)
        .map_err(|_| PersistenceError::Storage)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map(Some)
        .map_err(|_| PersistenceError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_support::*;

    #[test]
    fn historical_snapshots_read_after_restart_and_keep_expiry_tombstones() {
        let (_base, _library, _state, _name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v14.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "original",
                photo_id: "photo",
                relative_path: "missing.ARW",
                kind: "raw",
                available: false,
                size: 23,
                mtime_ms: 3000.0,
            },
        );
        let now = crate::persistence::export::export_unix_seconds() as i64;
        let source = "missing.ARW\0size\0mtime";
        let historical = b"<historical-document/>".to_vec();
        let digest = crate::xmp::digest(&historical);
        connection.execute(
            "INSERT INTO xmp_exports(id,photo_id,request_id,payload_digest,recipe_revision,source_revision,exposure_ev,white_balance_mode,temperature_kelvin,tint_milli,created_at,expires_at,filename,document,byte_length,sha256) VALUES(?,?,?,?,?,?,'0.5','as-shot',NULL,NULL,?,?,?,?,?,?)",
            params!["xmp-historical", "photo", "historical-request", "a".repeat(64), "retained-recipe", source, now, now + crate::XMP_RETENTION_SECONDS, "xmp-historical.xmp", historical, historical.len() as i64, digest],
        ).unwrap();
        drop(connection);
        let reopened = Connection::open(&path).unwrap();
        let retained = read(&reopened, "xmp-historical").unwrap().unwrap();
        assert_eq!(retained.document, historical);
        assert_eq!(retained.source_revision, source);
        assert_eq!(retained.recipe_version, "retained-recipe");
        assert_eq!(
            list(&reopened, "photo").unwrap(),
            Some(vec![retained.clone()])
        );
        assert_eq!(read(&reopened, "unknown").unwrap(), None);
        assert_eq!(list(&reopened, "unknown").unwrap(), None);
        reopened
            .execute(
                "UPDATE xmp_exports SET document=NULL,created_at=?1,expires_at=?2 WHERE id='xmp-historical'",
                [now - crate::XMP_RETENTION_SECONDS - 1, now - 1],
            )
            .unwrap();
        let tombstone = read(&reopened, "xmp-historical").unwrap().unwrap();
        assert!(tombstone.document.is_empty());
        assert_eq!(tombstone.byte_length, retained.byte_length);
        assert_eq!(tombstone.sha256, retained.sha256);
        assert_eq!(list(&reopened, "photo").unwrap(), Some(vec![tombstone]));
        reopened
            .execute(
                "UPDATE xmp_exports SET document=?,sha256=? WHERE id='xmp-historical'",
                params![historical, "0".repeat(64)],
            )
            .unwrap();
        assert!(matches!(
            read(&reopened, "xmp-historical"),
            Err(PersistenceError::Storage)
        ));
    }
}
