use super::*;
use rusqlite::{Connection, params};

#[tokio::test]
async fn legacy_edit_recipe_read_model_preserves_persisted_settings() {
    let (base, initial_config) = super::fixture();
    std::fs::write(
        base.0.join("originals/a.ARW"),
        super::raw_capture_fixture("2026:02:03 04:05:06"),
    )
    .unwrap();
    let library = Library::open(initial_config).unwrap();
    let first = library.scan().await.unwrap();
    let photo_id = first.photos[0].id.clone();
    let source_revision = library
        .edit_recipe(&photo_id)
        .await
        .unwrap()
        .unwrap()
        .current_source_revision
        .expect("published source");
    library.shutdown().unwrap();

    let connection = Connection::open(base.0.join("state").join("library.sqlite")).unwrap();
    connection
        .execute(
            "INSERT OR REPLACE INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode,temperature_kelvin,tint_milli) VALUES(?,?,?,?,?,?,?)",
            params![photo_id, "legacy-read", source_revision, 0.5, "temperature-tint", 6500, -12],
        )
        .unwrap();
    drop(connection);

    let library = Library::open(super::config(&base)).unwrap();
    let read = library
        .edit_recipe(&first.photos[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.current_source_revision, Some(source_revision));
    assert!(read.source_available);
    assert!(read.original_available);
    assert_eq!(read.original_location, "a.ARW");
    assert!(matches!(read.original_kind, OriginalKind::Raw));
    assert_eq!(
        read.recipe.unwrap().settings,
        crate::EditRecipeSettings {
            exposure_ev: 0.5,
            white_balance: crate::WhiteBalanceIntent::TemperatureTint {
                temperature_kelvin: 6500,
                tint_milli: -12,
            },
        }
    );
    library.shutdown().unwrap();
}

#[path = "recovery_safety.rs"]
mod recovery_safety;
