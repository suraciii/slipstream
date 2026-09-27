use super::{
    DatabaseName, PersistenceError, StateDirectory,
    owner::{
        parse_white_balance_intent, photo_processing_source, random_uuid_v4,
        white_balance_intent_name, white_balance_intent_values, write_transaction,
    },
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
                let source_available = row.get::<_, i64>(3)? != 0 && row.get::<_, i64>(4)? != 0;
                let recipe_revision: Option<String> = row.get(5)?;
                let recipe_source_revision: Option<String> = row.get(6)?;
                let exposure_ev: Option<f64> = row.get(7)?;
                let white_balance_mode: Option<String> = row.get(8)?;
                let temperature_kelvin: Option<i32> = row.get(9)?;
                let tint_milli: Option<i32> = row.get(10)?;
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
                Ok((relative_path, size, mtime_ms, source_available, recipe))
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    let Some((relative_path, size, mtime_ms, source_available, recipe)) = row else {
        return Ok(None);
    };
    let current_source_revision = crate::source_revision(&relative_path, size, mtime_ms)
        .map_err(|_| PersistenceError::Storage)?;
    Ok(Some(EditRecipeRead {
        recipe,
        current_source_revision,
        source_available,
    }))
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
            return Ok(EditRecipeWriteOutcome::Unavailable);
        }
        if current.current_source_revision != mutation.expected_source_revision {
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
        let Some(recipe) = current.recipe.as_ref() else {
            return Ok(EditRecipeWriteOutcome::MissingRecipe);
        };
        if recipe.revision != mutation.expected_recipe_version {
            return Ok(EditRecipeWriteOutcome::Conflict(current));
        }
        if current.current_source_revision != mutation.new_source_revision {
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
