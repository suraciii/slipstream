use super::{
    PersistenceError,
    owner::parse_white_balance_intent,
    scan::{parse_camera_identity, parse_capture_fact, parse_error_category, parse_kind},
};
use crate::{EditRecipe, EditRecipeRead, EditRecipeSettings};
use rusqlite::{Connection, OptionalExtension};

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
