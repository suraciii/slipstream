use super::{
    DatabaseName, PersistenceError, StateDirectory,
    owner::{
        parse_white_balance_intent, random_uuid_v4, white_balance_intent_name,
        white_balance_intent_values, write_transaction,
    },
    processing_export::photo_processing_source,
    scan::{parse_camera_identity, parse_capture_fact, parse_error_category, parse_kind},
};
use crate::{
    EditRecipe, EditRecipeRead, EditRecipeSettings, EditRecipeWriteOutcome, RebindEditRecipe,
    SaveEditRecipe, WhiteBalanceIntent,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
// Save receipts stay durable at this internal boundary. Before Web or CLI
// exposes request identities, the protocol must add an explicit age, expiry,
// and expired-identity outcome; deleting keys without that contract could
// allow an old identity to be reused for a different save.
const EDIT_RECIPE_RECEIPT_PREFIX: &str = "edit_recipe_receipt:";
const MAXIMUM_EDIT_RECIPE_REQUEST_ID_BYTES: usize = 128;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EditRecipeReceipt {
    photo_id: String,
    payload_digest: String,
    outcome: EditRecipeReceiptOutcome,
    revision: String,
    source_revision: String,
    exposure_ev: f64,
    white_balance_mode: String,
    #[serde(default)]
    temperature_kelvin: Option<i32>,
    #[serde(default)]
    tint_milli: Option<i32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum EditRecipeReceiptOutcome {
    Saved,
    Unchanged,
}
pub(super) fn read_edit_recipe(
    connection: &Connection,
    photo_id: &str,
) -> Result<Option<EditRecipeRead>, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT o.relative_path,o.size,o.mtime_ms,o.available,p.available,
                    o.kind,o.error_category,
                    o.capture_metadata_state,o.capture_order_key,o.capture_time_field,
                    o.capture_offset_minutes,o.capture_source_revision,
                    o.camera_identity_state,o.camera_make,o.camera_model,
                    e.revision,e.source_revision,e.exposure_ev,e.white_balance_mode,
                    e.temperature_kelvin,e.tint_milli
             FROM photos p JOIN original_files o ON o.id=p.original_id
             LEFT JOIN edit_recipes e ON e.photo_id=p.id WHERE p.id=?",
            [photo_id],
            |row| {
                let relative_path: String = row.get(0)?;
                let size = u64::try_from(row.get::<_, i64>(1)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let mtime_ms: f64 = row.get(2)?;
                let original_available = row.get::<_, i64>(3)? != 0;
                let photo_available = row.get::<_, i64>(4)? != 0;
                let kind = parse_kind(&row.get::<_, String>(5)?)?;
                let original_error = parse_error_category(row.get(6)?)?;
                let capture = parse_capture_fact(
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    parse_camera_identity(row.get(12)?, row.get(13)?, row.get(14)?)?,
                )?;
                let recipe_revision: Option<String> = row.get(15)?;
                let recipe_source_revision: Option<String> = row.get(16)?;
                let exposure_ev: Option<f64> = row.get(17)?;
                let white_balance_mode: Option<String> = row.get(18)?;
                let temperature_kelvin: Option<i32> = row.get(19)?;
                let tint_milli: Option<i32> = row.get(20)?;
                let recipe = match (
                    recipe_revision,
                    recipe_source_revision,
                    exposure_ev,
                    white_balance_mode,
                ) {
                    (None, None, None, None) => None,
                    (Some(revision), Some(source_revision), Some(exposure_ev), Some(mode)) => {
                        Some(EditRecipe {
                            photo_id: photo_id.to_owned(),
                            revision,
                            source_revision,
                            settings: EditRecipeSettings {
                                exposure_ev,
                                white_balance: parse_white_balance_intent(
                                    &mode,
                                    temperature_kelvin,
                                    tint_milli,
                                )?,
                            },
                        })
                    }
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                Ok(FactsRow {
                    relative_path,
                    size,
                    mtime_ms,
                    original_available,
                    photo_available,
                    kind,
                    original_error,
                    capture,
                    recipe,
                })
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    let Some(row) = row else {
        return Ok(None);
    };
    // The current source revision is published evidence only: it is named
    // exactly when the published Capture fact revision is bound to the
    // durable descriptor of the persisted source facts. Anything else is a
    // publication gap (a retryable wait state), never a synthesized
    // revision.
    let observed_revision = crate::source_revision(&row.relative_path, row.size, row.mtime_ms)
        .map_err(|_| PersistenceError::Storage)?;
    let current_source_revision = row
        .capture
        .source_revision
        .as_deref()
        .filter(|published| {
            crate::capture_revision_matches_descriptor(published, &observed_revision)
        })
        .map(|_| observed_revision);
    Ok(Some(EditRecipeRead {
        recipe: row.recipe,
        current_source_revision,
        source_available: row.original_available && row.photo_available,
        original_available: row.original_available,
        original_location: row.relative_path,
        original_kind: row.kind,
        original_error: row.original_error,
        capture: row.capture,
    }))
}

/// The committed columns one recipe facts read derives its response from.
struct FactsRow {
    relative_path: String,
    size: u64,
    mtime_ms: f64,
    original_available: bool,
    photo_available: bool,
    kind: crate::OriginalKind,
    original_error: Option<crate::OriginalErrorCategory>,
    capture: crate::CaptureFact,
    recipe: Option<EditRecipe>,
}

fn validate_edit_recipe_request_id(request_id: &str) -> bool {
    !request_id.is_empty()
        && request_id.len() <= MAXIMUM_EDIT_RECIPE_REQUEST_ID_BYTES
        && !request_id.chars().any(char::is_control)
}

fn edit_recipe_payload_digest(mutation: &SaveEditRecipe) -> Result<String, PersistenceError> {
    // The digest formula is durable, not a wire field: receipts written by
    // earlier releases hold digests over these exact serialized keys, so the
    // internal rename of the recipe-version field must not change them.
    // The as-shot white-balance value stays a bare string for the same
    // reason; a value-carrying intent serializes its values because two
    // different payloads under one request identity must never collide.
    let white_balance = match mutation.settings.white_balance {
        WhiteBalanceIntent::AsShot => serde_json::json!("as-shot"),
        WhiteBalanceIntent::TemperatureTint {
            temperature_kelvin,
            tint_milli,
        } => serde_json::json!({
            "mode": "temperature-tint",
            "temperature_kelvin": temperature_kelvin,
            "tint_milli": tint_milli,
        }),
    };
    let payload = serde_json::json!({
        "photo_id": mutation.photo_id,
        "expected_recipe_revision": mutation.expected_recipe_version,
        "expected_source_revision": mutation.expected_source_revision,
        "exposure_ev": mutation.settings.exposure_ev,
        "white_balance": white_balance,
    });
    let bytes = serde_json::to_vec(&payload).map_err(|_| PersistenceError::Storage)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn edit_recipe_receipt_key(request_id: &str) -> String {
    format!("{EDIT_RECIPE_RECEIPT_PREFIX}{request_id}")
}

fn read_edit_recipe_receipt(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<EditRecipeReceipt>, PersistenceError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [edit_recipe_receipt_key(request_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| PersistenceError::Storage))
        .transpose()
}

fn write_edit_recipe_receipt(
    transaction: &Transaction<'_>,
    request_id: &str,
    receipt: &EditRecipeReceipt,
) -> Result<(), PersistenceError> {
    let value = serde_json::to_string(receipt).map_err(|_| PersistenceError::Storage)?;
    transaction
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![edit_recipe_receipt_key(request_id), value],
        )
        .map_err(|_| PersistenceError::Storage)?;
    Ok(())
}

fn receipt_recipe(receipt: EditRecipeReceipt) -> Result<EditRecipe, PersistenceError> {
    let white_balance = parse_white_balance_intent(
        &receipt.white_balance_mode,
        receipt.temperature_kelvin,
        receipt.tint_milli,
    )
    .map_err(|_| PersistenceError::Storage)?;
    Ok(EditRecipe {
        photo_id: receipt.photo_id,
        revision: receipt.revision,
        source_revision: receipt.source_revision,
        settings: EditRecipeSettings {
            exposure_ev: receipt.exposure_ev,
            white_balance,
        },
    })
}

pub(super) fn save_edit_recipe(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: SaveEditRecipe,
) -> Result<EditRecipeWriteOutcome, PersistenceError> {
    if !mutation.settings.exposure_ev.is_finite()
        || !mutation.settings.white_balance.within_payload_bounds()
        || mutation.expected_source_revision.is_empty()
        || !validate_edit_recipe_request_id(&mutation.request_id)
    {
        return Ok(EditRecipeWriteOutcome::InvalidSettings);
    }
    let payload_digest = edit_recipe_payload_digest(&mutation)?;
    write_transaction(state, database_name, connection, |transaction| {
        if let Some(receipt) = read_edit_recipe_receipt(transaction, &mutation.request_id)? {
            if receipt.photo_id != mutation.photo_id || receipt.payload_digest != payload_digest {
                return Ok(EditRecipeWriteOutcome::RequestConflict);
            }
            let recipe = receipt_recipe(receipt.clone())?;
            return Ok(match receipt.outcome {
                EditRecipeReceiptOutcome::Saved => EditRecipeWriteOutcome::Replayed(recipe),
                EditRecipeReceiptOutcome::Unchanged => EditRecipeWriteOutcome::Unchanged(recipe),
            });
        }
        let Some(current) = read_edit_recipe(transaction, &mutation.photo_id)? else {
            return Ok(EditRecipeWriteOutcome::MissingPhoto);
        };
        let Some((kind, available)) = photo_processing_source(transaction, &mutation.photo_id)?
        else {
            return Ok(EditRecipeWriteOutcome::MissingPhoto);
        };
        if kind != crate::OriginalKind::Raw {
            return Ok(EditRecipeWriteOutcome::UnsupportedPhoto);
        }
        if !available || !current.source_available {
            // The Original is unavailable, so the only guarded save is one a
            // current Development Proxy stands in for: the proxy was derived
            // from exactly the last observed source revision, and the caller
            // guards against that same revision. The stored recipe stays
            // bound to the Original's source revision; nothing here rebinds
            // it to the proxy. Any other offline save stays refused.
            let guarded =
                super::development_proxy::read_development_proxy(transaction, &mutation.photo_id)?
                    .is_some_and(|proxy| {
                        proxy.source_revision == mutation.expected_source_revision
                            && current.current_source_revision.as_deref()
                                == Some(proxy.source_revision.as_str())
                    });
            if !guarded {
                return Ok(EditRecipeWriteOutcome::Unavailable);
            }
        } else if current.current_source_revision.is_none() {
            // No published Capture fact is bound to the observed source
            // facts: a retryable publication gap, not a confirmed change.
            return Ok(EditRecipeWriteOutcome::Unavailable);
        } else if current.current_source_revision.as_deref()
            != Some(mutation.expected_source_revision.as_str())
        {
            return Ok(EditRecipeWriteOutcome::SourceChanged(current));
        }
        if current
            .recipe
            .as_ref()
            .map(|recipe| recipe.revision.as_str())
            != mutation.expected_recipe_version.as_deref()
        {
            return Ok(EditRecipeWriteOutcome::Conflict(current));
        }
        if current
            .recipe
            .as_ref()
            .is_some_and(|recipe| recipe.source_revision != mutation.expected_source_revision)
        {
            return Ok(EditRecipeWriteOutcome::RequiresRebind(current));
        }
        if let Some(recipe) = &current.recipe
            && recipe.settings == mutation.settings
        {
            let (temperature_kelvin, tint_milli) =
                white_balance_intent_values(recipe.settings.white_balance);
            write_edit_recipe_receipt(
                transaction,
                &mutation.request_id,
                &EditRecipeReceipt {
                    photo_id: recipe.photo_id.clone(),
                    payload_digest,
                    outcome: EditRecipeReceiptOutcome::Unchanged,
                    revision: recipe.revision.clone(),
                    source_revision: recipe.source_revision.clone(),
                    exposure_ev: recipe.settings.exposure_ev,
                    white_balance_mode: white_balance_intent_name(recipe.settings.white_balance)
                        .to_owned(),
                    temperature_kelvin,
                    tint_milli,
                },
            )?;
            return Ok(EditRecipeWriteOutcome::Unchanged(recipe.clone()));
        }

        let revision = random_uuid_v4()?;
        let (temperature_kelvin, tint_milli) =
            white_balance_intent_values(mutation.settings.white_balance);
        transaction
            .execute(
                "INSERT INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode,temperature_kelvin,tint_milli)
                 VALUES(?,?,?,?,?,?,?)
                 ON CONFLICT(photo_id) DO UPDATE SET revision=excluded.revision,
                    source_revision=excluded.source_revision,exposure_ev=excluded.exposure_ev,
                    white_balance_mode=excluded.white_balance_mode,
                    temperature_kelvin=excluded.temperature_kelvin,tint_milli=excluded.tint_milli",
                params![
                    mutation.photo_id,
                    revision,
                    mutation.expected_source_revision,
                    mutation.settings.exposure_ev,
                    white_balance_intent_name(mutation.settings.white_balance),
                    temperature_kelvin,
                    tint_milli,
                ],
            )
            .map_err(|_| PersistenceError::Storage)?;
        let recipe = EditRecipe {
            photo_id: mutation.photo_id,
            revision,
            source_revision: mutation.expected_source_revision,
            settings: mutation.settings,
        };
        let (temperature_kelvin, tint_milli) =
            white_balance_intent_values(recipe.settings.white_balance);
        write_edit_recipe_receipt(
            transaction,
            &mutation.request_id,
            &EditRecipeReceipt {
                photo_id: recipe.photo_id.clone(),
                payload_digest,
                outcome: EditRecipeReceiptOutcome::Saved,
                revision: recipe.revision.clone(),
                source_revision: recipe.source_revision.clone(),
                exposure_ev: recipe.settings.exposure_ev,
                white_balance_mode: white_balance_intent_name(recipe.settings.white_balance)
                    .to_owned(),
                temperature_kelvin,
                tint_milli,
            },
        )?;
        Ok(EditRecipeWriteOutcome::Saved(recipe))
    })
}

fn edit_recipe_rebind_payload_digest(
    mutation: &RebindEditRecipe,
) -> Result<String, PersistenceError> {
    let payload = serde_json::json!({
        "kind": "rebind",
        "photo_id": mutation.photo_id,
        "expected_recipe_version": mutation.expected_recipe_version,
        "new_source_revision": mutation.new_source_revision,
    });
    let bytes = serde_json::to_vec(&payload).map_err(|_| PersistenceError::Storage)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(super) fn rebind_edit_recipe(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: RebindEditRecipe,
) -> Result<EditRecipeWriteOutcome, PersistenceError> {
    if mutation.new_source_revision.is_empty()
        || !validate_edit_recipe_request_id(&mutation.request_id)
    {
        return Ok(EditRecipeWriteOutcome::InvalidSettings);
    }
    let payload_digest = edit_recipe_rebind_payload_digest(&mutation)?;
    write_transaction(state, database_name, connection, |transaction| {
        // The rebind identity follows the save rules: the same identity and
        // payload replays the committed receipt, and the same identity with
        // a different payload is refused.
        if let Some(receipt) = read_edit_recipe_receipt(transaction, &mutation.request_id)? {
            if receipt.photo_id != mutation.photo_id || receipt.payload_digest != payload_digest {
                return Ok(EditRecipeWriteOutcome::RequestConflict);
            }
            let recipe = receipt_recipe(receipt.clone())?;
            return Ok(match receipt.outcome {
                EditRecipeReceiptOutcome::Saved => EditRecipeWriteOutcome::Replayed(recipe),
                EditRecipeReceiptOutcome::Unchanged => EditRecipeWriteOutcome::Unchanged(recipe),
            });
        }
        let Some(current) = read_edit_recipe(transaction, &mutation.photo_id)? else {
            return Ok(EditRecipeWriteOutcome::MissingPhoto);
        };
        let Some((kind, available)) = photo_processing_source(transaction, &mutation.photo_id)?
        else {
            return Ok(EditRecipeWriteOutcome::MissingPhoto);
        };
        if kind != crate::OriginalKind::Raw {
            return Ok(EditRecipeWriteOutcome::UnsupportedPhoto);
        }
        if !available || !current.source_available {
            return Ok(EditRecipeWriteOutcome::Unavailable);
        }
        if current.current_source_revision.is_none() {
            // No published Capture fact is bound to the observed source
            // facts: a retryable publication gap, not a confirmed change.
            return Ok(EditRecipeWriteOutcome::Unavailable);
        }
        let Some(recipe) = current.recipe.as_ref() else {
            return Ok(EditRecipeWriteOutcome::MissingRecipe);
        };
        if recipe.revision != mutation.expected_recipe_version {
            return Ok(EditRecipeWriteOutcome::Conflict(current));
        }
        if current.current_source_revision.as_deref() != Some(mutation.new_source_revision.as_str())
        {
            return Ok(EditRecipeWriteOutcome::SourceChanged(current));
        }
        if recipe.source_revision == mutation.new_source_revision {
            let (temperature_kelvin, tint_milli) =
                white_balance_intent_values(recipe.settings.white_balance);
            write_edit_recipe_receipt(
                transaction,
                &mutation.request_id,
                &EditRecipeReceipt {
                    photo_id: recipe.photo_id.clone(),
                    payload_digest,
                    outcome: EditRecipeReceiptOutcome::Unchanged,
                    revision: recipe.revision.clone(),
                    source_revision: recipe.source_revision.clone(),
                    exposure_ev: recipe.settings.exposure_ev,
                    white_balance_mode: white_balance_intent_name(recipe.settings.white_balance)
                        .to_owned(),
                    temperature_kelvin,
                    tint_milli,
                },
            )?;
            return Ok(EditRecipeWriteOutcome::Unchanged(recipe.clone()));
        }
        let revision = random_uuid_v4()?;
        let changed = transaction
            .execute(
                "UPDATE edit_recipes SET revision=?,source_revision=?
                 WHERE photo_id=? AND revision=?",
                params![
                    revision,
                    mutation.new_source_revision,
                    mutation.photo_id,
                    mutation.expected_recipe_version,
                ],
            )
            .map_err(|_| PersistenceError::Storage)?;
        if changed != 1 {
            return Ok(EditRecipeWriteOutcome::Conflict(current));
        }
        let rebound = EditRecipe {
            photo_id: mutation.photo_id,
            revision,
            source_revision: mutation.new_source_revision,
            settings: recipe.settings,
        };
        let (temperature_kelvin, tint_milli) =
            white_balance_intent_values(rebound.settings.white_balance);
        write_edit_recipe_receipt(
            transaction,
            &mutation.request_id,
            &EditRecipeReceipt {
                photo_id: rebound.photo_id.clone(),
                payload_digest,
                outcome: EditRecipeReceiptOutcome::Saved,
                revision: rebound.revision.clone(),
                source_revision: rebound.source_revision.clone(),
                exposure_ev: rebound.settings.exposure_ev,
                white_balance_mode: white_balance_intent_name(rebound.settings.white_balance)
                    .to_owned(),
                temperature_kelvin,
                tint_milli,
            },
        )?;
        Ok(EditRecipeWriteOutcome::Saved(rebound))
    })
}

#[cfg(test)]
mod tests {
    use crate::persistence::Persistence;
    use crate::persistence::test_support::*;
    use crate::{
        EditRecipe, EditRecipeRead, EditRecipeSettings, EditRecipeWriteOutcome, RebindEditRecipe,
        SaveEditRecipe, WhiteBalanceIntent, source_revision,
    };
    use rusqlite::Connection;
    use sha2::{Digest, Sha256};

    #[tokio::test]
    async fn edit_recipe_compare_and_set_rebind_and_read_model_are_guarded() {
        let (_base, library, state, name, path) = fixture();
        seed(
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
                photo_id: "raw-photo",
                relative_path: "shoot/one.ARW",
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
        connection.execute("UPDATE original_files SET capture_metadata_state='missing',capture_source_revision=? WHERE id='raw-original'", [format!("{}\0fixture-device\0fixture-inode", source_revision("shoot/one.ARW", 17, 1_000.0).unwrap())]).unwrap();
        drop(connection);

        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let initial_source = source_revision("shoot/one.ARW", 17, 1_000.0).unwrap();
        let initial = persistence
            .edit_recipe_receiver("raw-photo")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(initial.recipe.is_none());
        assert!(initial.source_available);
        assert_eq!(
            initial.current_source_revision,
            Some(initial_source.clone())
        );

        let mutation = SaveEditRecipe {
            photo_id: "raw-photo".to_owned(),
            request_id: "first-save".to_owned(),
            expected_recipe_version: None,
            expected_source_revision: initial_source.clone(),
            settings: EditRecipeSettings {
                exposure_ev: 0.0,
                white_balance: WhiteBalanceIntent::AsShot,
            },
        };
        let first = persistence
            .save_edit_recipe_receiver(mutation.clone())
            .unwrap();
        let concurrent = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                request_id: "concurrent-save".to_owned(),
                ..mutation.clone()
            })
            .unwrap();
        let (first, concurrent) = tokio::join!(first, concurrent);
        let first = first.unwrap().unwrap();
        let concurrent = concurrent.unwrap().unwrap();
        let recipe = match first {
            EditRecipeWriteOutcome::Saved(recipe) => recipe,
            outcome => panic!("first compare-and-set should save, got {outcome:?}"),
        };
        assert!(matches!(
            concurrent,
            EditRecipeWriteOutcome::Conflict(EditRecipeRead {
                recipe: Some(_),
                ..
            })
        ));
        assert_eq!(recipe.settings.exposure_ev, 0.0);

        let unchanged = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "unchanged-save".to_owned(),
                expected_recipe_version: Some(recipe.revision.clone()),
                expected_source_revision: initial_source.clone(),
                settings: recipe.settings,
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unchanged, EditRecipeWriteOutcome::Unchanged(recipe.clone()));

        let replay = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "first-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: initial_source.clone(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replay, EditRecipeWriteOutcome::Replayed(recipe.clone()));

        let request_conflict = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "first-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: initial_source.clone(),
                settings: EditRecipeSettings {
                    exposure_ev: 1.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request_conflict, EditRecipeWriteOutcome::RequestConflict);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE original_files SET size=18,mtime_ms=2_000.0,capture_source_revision=? WHERE id='raw-original'",
                [format!("{}\0fixture-device\0fixture-inode", source_revision("shoot/one.ARW", 18, 2_000.0).unwrap())],
            )
            .unwrap();
        drop(connection);
        let changed_source = source_revision("shoot/one.ARW", 18, 2_000.0).unwrap();
        let stale_save = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "stale-save".to_owned(),
                expected_recipe_version: Some(recipe.revision.clone()),
                expected_source_revision: initial_source.clone(),
                settings: EditRecipeSettings {
                    exposure_ev: 1.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            stale_save,
            EditRecipeWriteOutcome::SourceChanged(EditRecipeRead {
                recipe: Some(_),
                ..
            })
        ));
        let rebound = persistence
            .rebind_edit_recipe_receiver(RebindEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "rebind-1".to_owned(),
                expected_recipe_version: recipe.revision.clone(),
                new_source_revision: changed_source.clone(),
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let rebound = match rebound {
            EditRecipeWriteOutcome::Saved(recipe) => recipe,
            outcome => panic!("explicit rebind should save, got {outcome:?}"),
        };
        assert_ne!(rebound.revision, recipe.revision);
        assert_eq!(rebound.source_revision, changed_source);
        assert_eq!(rebound.settings, recipe.settings);

        // A replay stays exact even after another write advanced the recipe:
        // persistence decides the outcome inside the write transaction, so
        // the receipt's version is reported whatever happened since.
        persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "advance-save".to_owned(),
                expected_recipe_version: Some(rebound.revision.clone()),
                expected_source_revision: rebound.source_revision.clone(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.5,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let stale_replay = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "first-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: initial_source.clone(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stale_replay,
            EditRecipeWriteOutcome::Replayed(recipe.clone()),
            "the replay must carry the receipt's version, not the current one"
        );

        // The rebind identity replays exactly like a save identity: the same
        // payload replays the receipt, a different payload is refused.
        let rebind_replay = persistence
            .rebind_edit_recipe_receiver(RebindEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "rebind-1".to_owned(),
                expected_recipe_version: recipe.revision.clone(),
                new_source_revision: changed_source.clone(),
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            rebind_replay,
            EditRecipeWriteOutcome::Replayed(rebound.clone())
        );
        let rebind_conflict = persistence
            .rebind_edit_recipe_receiver(RebindEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "rebind-1".to_owned(),
                expected_recipe_version: rebound.revision.clone(),
                new_source_revision: rebound.source_revision.clone(),
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rebind_conflict, EditRecipeWriteOutcome::RequestConflict);
        assert!(matches!(
            persistence
                .save_edit_recipe_receiver(SaveEditRecipe {
                    photo_id: "raw-photo".to_owned(),
                    request_id: "after-rebind-save".to_owned(),
                    expected_recipe_version: Some(recipe.revision),
                    expected_source_revision: changed_source,
                    settings: EditRecipeSettings {
                        exposure_ev: 2.0,
                        white_balance: WhiteBalanceIntent::AsShot,
                    },
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap(),
            EditRecipeWriteOutcome::Conflict(_)
        ));
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE original_files SET available=0 WHERE id='raw-original'",
                [],
            )
            .unwrap();
        connection
            .execute("UPDATE photos SET available=0 WHERE id='raw-photo'", [])
            .unwrap();
        drop(connection);
        let photo = persistence
            .photo_receiver("raw-photo")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!photo.original_available);
        assert!(photo.has_saved_edits);
        let edit_read = persistence
            .edit_recipe_receiver("raw-photo")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!edit_read.source_available);
        assert!(persistence.snapshot().await.unwrap().photos[0].has_saved_edits);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn pre_upgrade_receipts_replay_with_the_original_digest_formula() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v8.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();

        // A receipt written by a release before the internal recipe-version
        // rename: its digest covers the original serialized key set, and its
        // shape carries no temperature or tint values because the as-shot
        // intent predates the value-carrying columns.
        let source_revision = "legacy-source-revision";
        let legacy_payload = serde_json::json!({
            "photo_id": "raw-photo",
            "expected_recipe_revision": serde_json::Value::Null,
            "expected_source_revision": source_revision,
            "exposure_ev": 0.25,
            "white_balance": "as-shot",
        });
        let legacy_digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&legacy_payload).unwrap())
        );
        let legacy_receipt = serde_json::json!({
            "photo_id": "raw-photo",
            "payload_digest": legacy_digest,
            "outcome": "Saved",
            "revision": "legacy-recipe-version",
            "source_revision": source_revision,
            "exposure_ev": 0.25,
            "white_balance_mode": "as-shot",
        });
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('edit_recipe_receipt:legacy-save',?)",
                [serde_json::to_string(&legacy_receipt).unwrap()],
            )
            .unwrap();
        drop(connection);

        // The same logical save retried after the upgrade must replay the
        // committed receipt, not refuse the identity as conflicted.
        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let replay = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "legacy-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: source_revision.to_owned(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.25,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            replay,
            EditRecipeWriteOutcome::Replayed(EditRecipe {
                photo_id: "raw-photo".to_owned(),
                revision: "legacy-recipe-version".to_owned(),
                source_revision: source_revision.to_owned(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.25,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            }),
            "a pre-upgrade receipt must keep replaying with the committed version"
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn edit_recipe_writes_reject_missing_unavailable_and_non_raw_photos() {
        let (_base, library, state, name, path) = fixture();
        seed(
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
                original_id: "missing-raw-original",
                photo_id: "missing-raw-photo",
                relative_path: "shoot/missing.ARW",
                kind: "raw",
                available: false,
                size: 13,
                mtime_ms: 1_000.0,
            },
        );
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "jpeg-original",
                photo_id: "jpeg-photo",
                relative_path: "shoot/one.JPG",
                kind: "jpeg",
                available: true,
                size: 11,
                mtime_ms: 2_000.0,
            },
        );
        drop(connection);
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();

        let unavailable = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "missing-raw-photo".to_owned(),
                request_id: "unavailable-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: source_revision("shoot/missing.ARW", 13, 1_000.0)
                    .unwrap(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unavailable, EditRecipeWriteOutcome::Unavailable);

        let unsupported = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "jpeg-photo".to_owned(),
                request_id: "jpeg-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: source_revision("shoot/one.JPG", 11, 2_000.0).unwrap(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unsupported, EditRecipeWriteOutcome::UnsupportedPhoto);

        let missing = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "not-a-photo".to_owned(),
                request_id: "missing-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: "source-revision".to_owned(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(missing, EditRecipeWriteOutcome::MissingPhoto);
        persistence.shutdown().unwrap();
    }

    /// A current Development Proxy stands in for an unavailable Original:
    /// the offline save stays guarded by exactly the source revision the
    /// proxy was derived from, the stored recipe keeps its Original binding,
    /// and every other offline save stays refused.
    #[tokio::test]
    async fn development_proxy_guards_offline_saves_against_the_derived_revision() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v13.sql"),
        );
        let mut connection = Connection::open(&path).unwrap();
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
                photo_id: "raw-photo",
                relative_path: "shoot/proxy.ARW",
                kind: "raw",
                available: false,
                size: 23,
                mtime_ms: 3_000.0,
            },
        );
        let derived = source_revision("shoot/proxy.ARW", 23, 3_000.0).unwrap();
        // The published Capture fact is bound to the same source revision
        // the proxy was derived from, so the offline read names it.
        connection
            .execute(
                "UPDATE original_files SET capture_metadata_state='missing',capture_source_revision=? WHERE id='raw-original'",
                [format!("{}\0fixture-device\0fixture-inode", derived)],
            )
            .unwrap();
        let record = |revision: &str| crate::DevelopmentProxyRecord {
            photo_id: "raw-photo".to_owned(),
            source_revision: revision.to_owned(),
            source_relative_path: "shoot/proxy.ARW".to_owned(),
            source_sha256: "a".repeat(64),
            source_size: 23,
            profile_id: "sony-ilce-7rm5-arw".to_owned(),
            pipeline_version: crate::DEVELOPMENT_PROXY_PIPELINE_VERSION.to_owned(),
            bundle_sha256: "b".repeat(64),
            long_edge: crate::DEVELOPMENT_PROXY_LONG_EDGE,
            width: 2560,
            height: 1707,
            artifact_sha256: "c".repeat(64),
            artifact_bytes: 2048,
            created_at: 1_700_000_000,
        };
        assert!(
            crate::persistence::development_proxy::record_development_proxy(
                &state,
                &name,
                &mut connection,
                record(&derived),
            )
            .unwrap()
        );
        drop(connection);
        let persistence = Persistence::open(
            crate::persistence::admission::StateDirectory::open_or_create(
                &library,
                state.canonical_path(),
            )
            .unwrap(),
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();

        let save = |request_id: &str,
                    expected_recipe_version: Option<String>,
                    expected_source_revision: &str| {
            let receiver = persistence
                .save_edit_recipe_receiver(SaveEditRecipe {
                    photo_id: "raw-photo".to_owned(),
                    request_id: request_id.to_owned(),
                    expected_recipe_version,
                    expected_source_revision: expected_source_revision.to_owned(),
                    settings: EditRecipeSettings {
                        exposure_ev: 0.5,
                        white_balance: WhiteBalanceIntent::AsShot,
                    },
                })
                .unwrap();
            async move { receiver.await.unwrap().unwrap() }
        };
        let saved = save("proxy-save-1", None, &derived).await;
        let EditRecipeWriteOutcome::Saved(recipe) = saved else {
            panic!("a proxy-guarded offline save must be admitted: {saved:?}");
        };
        assert_eq!(recipe.source_revision, derived);
        assert_eq!(recipe.settings.exposure_ev, 0.5);
        let read = persistence
            .edit_recipe_receiver("raw-photo")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!read.source_available);
        assert_eq!(
            read.current_source_revision.as_deref(),
            Some(derived.as_str())
        );
        assert_eq!(
            read.recipe
                .as_ref()
                .map(|recipe| recipe.source_revision.as_str()),
            Some(derived.as_str()),
            "the stored recipe stays bound to the Original's source revision"
        );

        // A guard against any other revision is not the proxy's derivation.
        let foreign = save("proxy-save-2", Some(recipe.revision.clone()), "rev-foreign").await;
        assert_eq!(foreign, EditRecipeWriteOutcome::Unavailable);

        // Without the proxy row the offline save is refused outright.
        assert!(
            crate::persistence::development_proxy::remove_development_proxy(
                &state,
                &name,
                &mut Connection::open(&path).unwrap(),
                "raw-photo",
            )
            .unwrap()
        );
        let unguarded = save("proxy-save-3", Some(recipe.revision), &derived).await;
        assert_eq!(unguarded, EditRecipeWriteOutcome::Unavailable);
        persistence.shutdown().unwrap();
    }
}
