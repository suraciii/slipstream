use super::owner::{Command, Persistence, PersistenceError, random_uuid_v4};
use crate::{
    EditRecipe, EditRecipeSettings, WhiteBalanceIntent, XMP_RETENTION_SECONDS, XmpCreateOutcome,
    XmpExportRecord,
};
use rusqlite::{Connection, OptionalExtension, params};
use tokio::sync::oneshot;

type PhotoXmpExportsReceiver =
    oneshot::Receiver<Result<Option<Vec<XmpExportRecord>>, PersistenceError>>;

impl Persistence {
    /// Captures an immutable edit-state document without opening the Original.
    pub(crate) fn create_xmp_receiver(
        &self,
        photo_id: &str,
        request_id: &str,
        expected_recipe: &str,
        expected_source: &str,
        now: i64,
    ) -> Result<oneshot::Receiver<Result<XmpCreateOutcome, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::CreateXmp {
            photo_id: photo_id.to_owned(),
            request_id: request_id.to_owned(),
            expected_recipe: expected_recipe.to_owned(),
            expected_source: expected_source.to_owned(),
            now,
            reply: send,
        })?;
        Ok(receive)
    }
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

pub(super) fn create(
    connection: &mut Connection,
    photo_id: &str,
    request_id: &str,
    expected_recipe: &str,
    expected_source: &str,
    now: i64,
) -> Result<XmpCreateOutcome, PersistenceError> {
    let transaction = connection
        .transaction()
        .map_err(|_| PersistenceError::Storage)?;
    let payload_digest =
        crate::export_submission_payload_digest("edit-state-xmp", expected_recipe, expected_source);
    let existing = transaction
        .query_row(
            &format!(
                "SELECT {COLUMNS},payload_digest FROM xmp_exports WHERE photo_id=? AND request_id=?"
            ),
            params![photo_id, request_id],
            |row| Ok((record(row)?, row.get::<_, String>(14)?)),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if let Some((existing, captured_payload)) = existing {
        return Ok(if captured_payload != payload_digest {
            XmpCreateOutcome::Conflict
        } else if existing.expires_at <= now {
            XmpCreateOutcome::Expired
        } else {
            XmpCreateOutcome::Replay(existing)
        });
    }
    let exists = transaction
        .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |r| {
            r.get::<_, i64>(0)
        })
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if exists.is_none() {
        return Ok(XmpCreateOutcome::NotFound);
    }
    let current = transaction.query_row(
        "SELECT revision,source_revision,exposure_ev,white_balance_mode,temperature_kelvin,tint_milli FROM edit_recipes WHERE photo_id=?",
        [photo_id], |r| {
            let mode: String = r.get(3)?;
            Ok(EditRecipe { photo_id: photo_id.to_owned(), revision: r.get(0)?, source_revision: r.get(1)?,
                settings: EditRecipeSettings { exposure_ev: r.get(2)?, white_balance: balance(&mode, r.get(4)?, r.get(5)?)? } })
        },
    ).optional().map_err(|_| PersistenceError::Storage)?;
    let Some(recipe) = current else {
        return Ok(XmpCreateOutcome::MissingRecipe);
    };
    if recipe.revision != expected_recipe || recipe.source_revision != expected_source {
        return Ok(XmpCreateOutcome::Stale);
    }
    let id = format!("xmp-{}", random_uuid_v4()?);
    let document = crate::xmp::document(&recipe);
    let byte_length = i64::try_from(document.len()).map_err(|_| PersistenceError::Storage)?;
    let filename = format!("{id}.xmp");
    let sha256 = crate::xmp::digest(&document);
    let expires_at = now
        .checked_add(XMP_RETENTION_SECONDS)
        .ok_or(PersistenceError::Storage)?;
    let (mode, temp, tint) = match recipe.settings.white_balance {
        WhiteBalanceIntent::AsShot => ("as-shot", None, None),
        WhiteBalanceIntent::TemperatureTint {
            temperature_kelvin,
            tint_milli,
        } => (
            "temperature-tint",
            Some(temperature_kelvin),
            Some(tint_milli),
        ),
    };
    transaction.execute(
        "INSERT INTO xmp_exports(id,photo_id,request_id,payload_digest,recipe_revision,source_revision,exposure_ev,white_balance_mode,temperature_kelvin,tint_milli,created_at,expires_at,filename,document,byte_length,sha256) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![id, photo_id, request_id, payload_digest, recipe.revision, recipe.source_revision, recipe.settings.exposure_ev, mode, temp, tint, now, expires_at, filename, document, byte_length, sha256],
    ).map_err(|_| PersistenceError::Storage)?;
    let saved = transaction
        .query_row(
            &format!("SELECT {COLUMNS} FROM xmp_exports WHERE id=?"),
            [&id],
            record,
        )
        .map_err(|_| PersistenceError::Storage)?;
    transaction
        .commit()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(XmpCreateOutcome::Created(saved))
}

pub(super) fn read(
    connection: &Connection,
    id: &str,
) -> Result<Option<XmpExportRecord>, PersistenceError> {
    connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM xmp_exports WHERE id=?"),
            [id],
            record,
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)
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
    fn snapshots_replay_after_restart_without_recipe_or_original() {
        let (_base, _library, _state, _name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v14.sql"),
        );
        let mut connection = Connection::open(&path).unwrap();
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
        let source = "missing.ARW\0size\0mtime";
        connection.execute("INSERT INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode) VALUES('photo','recipe',?,0.5,'as-shot')", [source]).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(
            create(&mut connection, "photo", "stale", "wrong", source, now).unwrap(),
            XmpCreateOutcome::Stale
        );
        let XmpCreateOutcome::Created(saved) =
            create(&mut connection, "photo", "request", "recipe", source, now).unwrap()
        else {
            panic!("snapshot not created")
        };
        // A saved document is authoritative even when generation changes.
        let historical = b"<historical-document/>".to_vec();
        connection
            .execute(
                "UPDATE xmp_exports SET document=?,byte_length=?,sha256=? WHERE id=?",
                params![
                    historical,
                    i64::try_from(historical.len()).unwrap(),
                    crate::xmp::digest(&historical),
                    saved.export_id
                ],
            )
            .unwrap();
        connection
            .execute("DELETE FROM edit_recipes WHERE photo_id='photo'", [])
            .unwrap();
        drop(connection);
        let mut reopened = Connection::open(&path).unwrap();
        let replay = read(&reopened, &saved.export_id).unwrap().unwrap();
        assert_eq!(replay.document, historical);
        assert_eq!(
            create(&mut reopened, "photo", "request", "recipe", source, now + 1).unwrap(),
            XmpCreateOutcome::Replay(replay.clone())
        );
        assert_eq!(
            list(&reopened, "photo").unwrap(),
            Some(vec![replay.clone()])
        );
        reopened
            .execute(
                "UPDATE xmp_exports SET created_at=?,document=NULL,expires_at=? WHERE id=?",
                params![now - XMP_RETENTION_SECONDS - 1, now - 1, saved.export_id],
            )
            .unwrap();
        let tombstone = read(&reopened, &saved.export_id).unwrap().unwrap();
        assert!(tombstone.document.is_empty());
        assert_eq!(tombstone.byte_length, replay.byte_length);
        assert_eq!(tombstone.sha256, replay.sha256);
        assert_eq!(list(&reopened, "photo").unwrap(), Some(vec![tombstone]));
        assert_eq!(
            create(&mut reopened, "photo", "request", "recipe", source, now).unwrap(),
            XmpCreateOutcome::Expired
        );
        assert_eq!(
            create(
                &mut reopened,
                "photo",
                "request",
                "changed",
                source,
                now + 1
            )
            .unwrap(),
            XmpCreateOutcome::Conflict
        );
        assert_eq!(
            create(
                &mut reopened,
                "photo",
                "request",
                "recipe",
                source,
                replay.expires_at
            )
            .unwrap(),
            XmpCreateOutcome::Expired
        );
        assert_eq!(
            create(&mut reopened, "photo", "new", "recipe", source, now).unwrap(),
            XmpCreateOutcome::MissingRecipe
        );
        assert_eq!(
            create(&mut reopened, "unknown", "request", "recipe", source, now).unwrap(),
            XmpCreateOutcome::NotFound
        );
        reopened
            .execute(
                "UPDATE xmp_exports SET document=?,sha256=? WHERE id=?",
                params![historical, "0".repeat(64), saved.export_id],
            )
            .unwrap();
        assert!(matches!(
            read(&reopened, &saved.export_id),
            Err(PersistenceError::Storage)
        ));
    }
}
