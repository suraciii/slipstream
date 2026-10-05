//! Library-root binding: the preserved-row projection and the plan that rebinds
//! a Library to its canonical root.

use super::migrations::validate_database;
use super::owner::PersistenceError;
use super::{DatabaseName, SchemaVersion, StateDirectory, migrations, validate_canonical_schema};
use crate::identity::classify_name;
use crate::{LibraryRoot, OriginalKind, RelativeOriginalPath, ScanLimits};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

type PreservedOriginal = (
    String,
    String,
    i64,
    f64,
    i64,
    Option<String>,
    Option<String>,
);
type PreservedPhoto = (String, String, i64, String, i64);
#[derive(Debug, PartialEq)]
struct ExpansionProjection {
    originals: Vec<PreservedOriginal>,
    photos: Vec<PreservedPhoto>,
    albums: Vec<(String, String, i64)>,
    members: Vec<(String, String, i64)>,
    progress: Vec<(String, String)>,
}

#[derive(Debug)]
struct ExpansionPlan {
    originals: Vec<(String, String, String)>,
    photo_sort_paths: Vec<(String, String)>,
}

pub(crate) fn expand_library_binding(
    proposed_root: &LibraryRoot,
    state: StateDirectory,
    database_name: DatabaseName,
    limits: ScanLimits,
    fail_after_first_update: bool,
) -> Result<(), PersistenceError> {
    let identity = state.prepare_existing_database(&database_name)?;
    let _database_lock = state.lock_database(&database_name)?;
    state.verify_database(&database_name, identity)?;
    let readonly = Connection::open_with_flags(
        state.sqlite_immutable_uri(&database_name),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|_| PersistenceError::Storage)?;
    // The read-only preflight accepts every schema the writable pass can migrate or use.
    if validate_canonical_schema(&readonly, SchemaVersion::V15).is_err()
        && validate_canonical_schema(&readonly, SchemaVersion::V14).is_err()
        && validate_canonical_schema(&readonly, SchemaVersion::V12).is_err()
        && validate_canonical_schema(&readonly, SchemaVersion::V11).is_err()
        && validate_canonical_schema(&readonly, SchemaVersion::V10).is_err()
        && validate_canonical_schema(&readonly, SchemaVersion::V9).is_err()
        && validate_canonical_schema(&readonly, SchemaVersion::V8).is_err()
        && validate_canonical_schema(&readonly, SchemaVersion::V7).is_err()
    {
        return Err(PersistenceError::UnsupportedSchema);
    }
    let stored_root = required_root_binding(&readonly)?;
    drop(readonly);

    let prefix = expansion_prefix(proposed_root.canonical_path(), &stored_root)?;
    let old_root =
        LibraryRoot::open(&stored_root).map_err(|_| PersistenceError::InvalidExpansion)?;
    let confined_old = proposed_root
        .descendant(
            RelativeOriginalPath::parse(prefix.clone())
                .map_err(|_| PersistenceError::InvalidExpansion)?,
        )
        .map_err(|_| PersistenceError::InvalidExpansion)?;
    if !old_root
        .identifies_same_directory(&confined_old)
        .map_err(|_| PersistenceError::InvalidExpansion)?
    {
        return Err(PersistenceError::InvalidExpansion);
    }
    proposed_root
        .scan(limits)
        .map_err(|_| PersistenceError::InvalidExpansion)?;
    let confined_after_scan = proposed_root
        .descendant(
            RelativeOriginalPath::parse(prefix.clone())
                .map_err(|_| PersistenceError::InvalidExpansion)?,
        )
        .map_err(|_| PersistenceError::InvalidExpansion)?;
    if !old_root
        .identifies_same_directory(&confined_after_scan)
        .map_err(|_| PersistenceError::InvalidExpansion)?
    {
        return Err(PersistenceError::InvalidExpansion);
    }

    state.verify_database(&database_name, identity)?;
    state.admit_sidecars(&database_name)?;
    let mut connection = Connection::open(state.sqlite_path(&database_name))
        .map_err(|_| PersistenceError::Storage)?;
    state.verify_database(&database_name, identity)?;
    state.admit_sidecars(&database_name)?;
    let journal: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(|_| PersistenceError::Storage)?;
    if !journal.eq_ignore_ascii_case("delete") {
        return Err(PersistenceError::UnsupportedSchema);
    }
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|_| PersistenceError::Storage)?;
    // Bring a previous-release schema up to the current one with the same
    // migration chain startup uses, so the expansion writes against the
    // canonical current tables.
    migrations::startup_schema(&state, &database_name, &mut connection, &stored_root)?;
    if required_root_binding(&connection)? != stored_root {
        return Err(PersistenceError::RootMismatch);
    }
    validate_database(&connection)?;
    let plan = expansion_plan(&connection, &prefix)?;
    let preserved = expansion_projection(&connection)?;

    state.admit_sidecars(&database_name)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(&transaction, SchemaVersion::V15)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    if required_root_binding(&transaction)? != stored_root
        || expansion_projection(&transaction)? != preserved
    {
        return Err(PersistenceError::InvalidExpansion);
    }
    for (index, (id, old_path, new_path)) in plan.originals.iter().enumerate() {
        let changed = transaction
            .execute(
                "UPDATE original_files SET relative_path=?,capture_metadata_state='pending',capture_order_key=NULL,capture_time_field=NULL,capture_offset_minutes=NULL,capture_source_revision=NULL,camera_identity_state='pending',camera_make=NULL,camera_model=NULL WHERE id=? AND relative_path=?",
                params![new_path, id, old_path],
            )
            .map_err(|_| PersistenceError::Storage)?;
        if changed != 1 {
            return Err(PersistenceError::InvalidExpansion);
        }
        if fail_after_first_update && index == 0 {
            return Err(PersistenceError::Storage);
        }
    }
    transaction
        .execute(
            "UPDATE photos SET association_generation=association_generation+1",
            [],
        )
        .map_err(|_| PersistenceError::Storage)?;
    transaction
        .execute(
            "UPDATE sidecar_associations SET sidecar_path=?||'/'||sidecar_path",
            [&prefix],
        )
        .map_err(|_| PersistenceError::Storage)?;
    transaction
        .execute(
            "UPDATE retained_sidecar_orphans SET sidecar_path=?||'/'||sidecar_path",
            [&prefix],
        )
        .map_err(|_| PersistenceError::Storage)?;
    for (id, sort_path) in &plan.photo_sort_paths {
        let changed = transaction
            .execute(
                "UPDATE photos SET sort_path=?,preview_state='inspection-pending',preview_source_revision=NULL,preview_width=NULL,preview_height=NULL,cache_revision=NULL WHERE id=?",
                params![sort_path, id],
            )
            .map_err(|_| PersistenceError::Storage)?;
        if changed != 1 {
            return Err(PersistenceError::InvalidExpansion);
        }
    }
    if transaction
        .execute(
            "UPDATE library_metadata SET value=? WHERE key='canonical_root' AND value=?",
            params![
                proposed_root
                    .canonical_path()
                    .to_str()
                    .ok_or(PersistenceError::InvalidExpansion)?,
                stored_root
            ],
        )
        .map_err(|_| PersistenceError::Storage)?
        != 1
        || expansion_projection(&transaction)? != preserved
    {
        return Err(PersistenceError::InvalidExpansion);
    }
    validate_database(&transaction)?;
    validate_canonical_schema(&transaction, SchemaVersion::V15)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction.commit().map_err(|_| PersistenceError::Storage)
}

fn required_root_binding(connection: &Connection) -> Result<String, PersistenceError> {
    connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key='canonical_root'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?
        .ok_or(PersistenceError::InvalidExpansion)
}

fn expansion_prefix(proposed: &Path, stored: &str) -> Result<String, PersistenceError> {
    let stored = Path::new(stored);
    let relative = stored
        .strip_prefix(proposed)
        .map_err(|_| PersistenceError::InvalidExpansion)?;
    let prefix = relative
        .to_str()
        .ok_or(PersistenceError::InvalidExpansion)?;
    RelativeOriginalPath::parse(prefix.to_owned())
        .map(|path| path.as_str().to_owned())
        .map_err(|_| PersistenceError::InvalidExpansion)
}

fn expansion_plan(
    connection: &Connection,
    prefix: &str,
) -> Result<ExpansionPlan, PersistenceError> {
    let originals = connection
        .prepare("SELECT id,relative_path,kind FROM original_files ORDER BY id")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let mut paths_by_id = HashMap::new();
    let mut targets = HashSet::new();
    let mut mapped = Vec::with_capacity(originals.len());
    for (id, old_path, kind) in originals {
        let old = RelativeOriginalPath::parse(old_path.clone())
            .map_err(|_| PersistenceError::InvalidExpansion)?;
        let classified = old
            .as_str()
            .rsplit('/')
            .next()
            .and_then(classify_name)
            .ok_or(PersistenceError::InvalidExpansion)?;
        if (classified == OriginalKind::Raw) != (kind == "raw")
            || !matches!(kind.as_str(), "raw" | "jpeg")
        {
            return Err(PersistenceError::InvalidExpansion);
        }
        let new_path = RelativeOriginalPath::parse(format!("{prefix}/{}", old.as_str()))
            .map_err(|_| PersistenceError::InvalidExpansion)?
            .as_str()
            .to_owned();
        if !targets.insert(new_path.clone()) {
            return Err(PersistenceError::InvalidExpansion);
        }
        paths_by_id.insert(id.clone(), new_path.clone());
        mapped.push((id, old_path, new_path));
    }
    let photos = connection
        .prepare("SELECT id,original_id FROM photos ORDER BY id")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let mut photo_sort_paths = Vec::with_capacity(photos.len());
    for (id, original) in photos {
        let sort_path = paths_by_id
            .get(&original)
            .ok_or(PersistenceError::InvalidExpansion)?
            .clone();
        photo_sort_paths.push((id, sort_path));
    }
    Ok(ExpansionPlan {
        originals: mapped,
        photo_sort_paths,
    })
}

fn expansion_projection(connection: &Connection) -> Result<ExpansionProjection, PersistenceError> {
    macro_rules! rows {
        ($sql:literal, $map:expr) => {{
            connection
                .prepare($sql)
                .map_err(|_| PersistenceError::Storage)?
                .query_map([], $map)
                .map_err(|_| PersistenceError::Storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| PersistenceError::Storage)?
        }};
    }
    Ok(ExpansionProjection {
        originals: rows!(
            "SELECT id,kind,size,mtime_ms,available,error_category,error_message FROM original_files ORDER BY id",
            |row| Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?
            ))
        ),
        photos: rows!(
            "SELECT id,original_id,available,selection_state,rating FROM photos ORDER BY id",
            |row| Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?
            ))
        ),
        albums: rows!("SELECT id,name,created_at FROM albums ORDER BY id", |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        }),
        members: rows!(
            "SELECT album_id,photo_id,position FROM album_members ORDER BY album_id,photo_id",
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        ),
        progress: rows!(
            "SELECT album_id,photo_id FROM album_progress ORDER BY album_id",
            |row| Ok((row.get(0)?, row.get(1)?))
        ),
    })
}

#[cfg(test)]
mod tests {
    use crate::persistence::PersistenceError;
    use crate::persistence::scan;
    use crate::persistence::test_support::*;
    use crate::{LibraryRoot, RelativeOriginalPath, RequestedRelocation, source_revision};
    use rusqlite::Connection;
    use rusqlite::params;
    use std::fs;

    #[tokio::test]
    async fn retire_and_bind_retires_before_delete() {
        let (_base, root, state, name, path) = fixture();
        fs::write(root.canonical_path().join("one.JPG"), b"one").unwrap();
        fs::write(root.canonical_path().join("two.JPG"), b"different").unwrap();
        let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
        let snapshot = library.scan().await.unwrap();
        let destination = &snapshot.photos[0];
        let retiring = &snapshot.photos[1];
        let old_path = snapshot
            .originals
            .iter()
            .find(|o| o.id == destination.original_id)
            .unwrap()
            .relative_path
            .as_str();
        let new_path = snapshot
            .originals
            .iter()
            .find(|o| o.id == retiring.original_id)
            .unwrap()
            .relative_path
            .clone();
        seed_sidecar(&path, &destination.id, "dir/source.xmp");
        seed_sidecar(&path, &retiring.id, "dir/destination.xmp");
        let destination_before = association_generation(&path, &destination.id);
        let retiring_before = association_generation(&path, &retiring.id);
        fs::remove_file(root.canonical_path().join(old_path)).unwrap();
        library.scan().await.unwrap();
        let facts = root
            .original(new_path.clone())
            .unwrap()
            .facts_if_present()
            .unwrap()
            .unwrap();
        library
            .apply_relocations(vec![RequestedRelocation {
                original_id: destination.original_id.clone(),
                from_location: old_path.to_owned(),
                to_location: new_path.to_string(),
                mapping_id: "test-mapping".to_owned(),
                fingerprint: None,
                facts,
                retire_photo_id: Some(retiring.id.clone()),
            }])
            .await
            .unwrap();
        let after = association_generation(&path, &destination.id);
        assert!(after > destination_before);
        assert_retired(
            &path,
            &retiring.id,
            new_path.as_str(),
            "dir/destination.xmp",
            retiring_before + 1,
        );
        assert_retired(
            &path,
            &destination.id,
            new_path.as_str(),
            "dir/source.xmp",
            after,
        );
        let connection = Connection::open(&path).unwrap();
        assert!(
            !connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM photos WHERE id=?)",
                    [&retiring.id],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
        );
        assert!(
            !connection
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap()
        );
        library.shutdown().unwrap();
    }

    #[test]
    fn retire_and_bind_refuses_a_photo_with_saved_recipe() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v13.sql"),
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
                original_id: "missing-original",
                photo_id: "missing-photo",
                relative_path: "shoot/missing.ARW",
                kind: "raw",
                available: false,
                size: 11,
                mtime_ms: 1_000.0,
            },
        );
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "occupant-original",
                photo_id: "occupant-photo",
                relative_path: "moved/occupied.ARW",
                kind: "raw",
                available: true,
                size: 19,
                mtime_ms: 2_000.0,
            },
        );
        connection
            .execute(
                "INSERT INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode)
                 VALUES(?,?,?,?, 'as-shot')",
                params![
                    "occupant-photo",
                    "recipe-revision",
                    source_revision("moved/occupied.ARW", 19, 2_000.0).unwrap(),
                    0.0_f64
                ],
            )
            .unwrap();

        fs::create_dir_all(library.canonical_path().join("moved")).unwrap();
        fs::write(
            library.canonical_path().join("moved/occupied.ARW"),
            b"1234567890123456789",
        )
        .unwrap();
        let root = LibraryRoot::open(library.canonical_path()).unwrap();
        let facts = root
            .original(RelativeOriginalPath::parse("moved/occupied.ARW").unwrap())
            .unwrap()
            .facts()
            .unwrap();
        let result = scan::apply_manual_relocations(
            &state,
            &name,
            &mut Connection::open(&path).unwrap(),
            &root,
            &[RequestedRelocation {
                original_id: "missing-original".to_owned(),
                from_location: "shoot/missing.ARW".to_owned(),
                to_location: "moved/occupied.ARW".to_owned(),
                mapping_id: "test-mapping".to_owned(),
                fingerprint: None,
                facts,
                retire_photo_id: Some("occupant-photo".to_owned()),
            }],
        );
        assert!(matches!(
            result,
            Err(PersistenceError::InvalidRecoveryMapping {
                reason: "destination-in-use",
                ..
            })
        ));

        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM photos WHERE id IN ('missing-photo','occupant-photo')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM edit_recipes WHERE photo_id='occupant-photo'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }
}
