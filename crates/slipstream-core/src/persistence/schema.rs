use rusqlite::Connection;
use serde_json::{Value, json};
use std::fmt;

const SCHEMA_V1_MANIFEST: &str = include_str!("../../../../compatibility/sqlite/schema-v1.json");
const SCHEMA_V2_MANIFEST: &str = include_str!("../../../../compatibility/sqlite/schema-v2.json");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaVersion {
    V1,
    V2,
}

impl SchemaVersion {
    fn manifest(self) -> &'static str {
        match self {
            Self::V1 => SCHEMA_V1_MANIFEST,
            Self::V2 => SCHEMA_V2_MANIFEST,
        }
    }
}

#[derive(Debug)]
pub struct SchemaError;

impl fmt::Display for SchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SQLite schema is unsupported")
    }
}

impl std::error::Error for SchemaError {}

pub fn validate_canonical_schema(
    connection: &Connection,
    version: SchemaVersion,
) -> Result<(), SchemaError> {
    if schema_manifest(connection).map_err(|_| SchemaError)? != expected_manifest(version)? {
        return Err(SchemaError);
    }
    Ok(())
}

fn expected_manifest(version: SchemaVersion) -> Result<Value, SchemaError> {
    serde_json::from_str(version.manifest()).map_err(|_| SchemaError)
}

fn compact_sql(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace() && !"\"`[]".contains(*character))
        .collect()
}

fn schema_manifest(connection: &Connection) -> rusqlite::Result<Value> {
    let user_version: u32 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let mut statement = connection.prepare(
        "SELECT type,name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut objects = Vec::new();
    for row in rows {
        let (kind, name, sql) = row?;
        let mut object = json!({
            "type": kind,
            "name": name,
            "sql": compact_sql(&sql),
        });
        if kind == "table" {
            let columns = connection
                .prepare(&format!("PRAGMA table_info(\"{name}\")"))?
                .query_map([], |row| {
                    Ok(json!({
                        "name": row.get::<_, String>(1)?,
                        "type": row.get::<_, String>(2)?,
                        "notNull": row.get::<_, bool>(3)?,
                        "default": row.get::<_, Option<String>>(4)?,
                        "primaryKey": row.get::<_, u32>(5)?,
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let foreign_keys = connection
                .prepare(&format!("PRAGMA foreign_key_list(\"{name}\")"))?
                .query_map([], |row| {
                    Ok(json!({
                        "table": row.get::<_, String>(2)?,
                        "from": row.get::<_, String>(3)?,
                        "to": row.get::<_, String>(4)?,
                        "onUpdate": row.get::<_, String>(5)?,
                        "onDelete": row.get::<_, String>(6)?,
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            object["columns"] = columns.into();
            object["foreignKeys"] = foreign_keys.into();
        }
        objects.push(object);
    }
    Ok(json!({ "userVersion": user_version, "objects": objects }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA_V1_SQL: &str = include_str!("../../../../compatibility/sqlite/schema-v1.sql");
    const SCHEMA_V2_SQL: &str = include_str!("../../../../compatibility/sqlite/schema-v2.sql");

    fn execute_fixture(sql: &str) -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(sql).unwrap();
        connection
    }

    #[test]
    fn exact_v1_and_v2_manifests_match_shared_contracts() {
        let v1 = execute_fixture(SCHEMA_V1_SQL);
        validate_canonical_schema(&v1, SchemaVersion::V1).unwrap();
        let v2 = execute_fixture(SCHEMA_V2_SQL);
        validate_canonical_schema(&v2, SchemaVersion::V2).unwrap();
    }

    #[test]
    fn exact_manifest_rejects_extra_or_changed_objects() {
        let v1 = execute_fixture(SCHEMA_V1_SQL);
        v1.execute_batch("CREATE TABLE unexpected(value TEXT);")
            .unwrap();
        assert!(validate_canonical_schema(&v1, SchemaVersion::V1).is_err());

        let v2 = execute_fixture(SCHEMA_V2_SQL);
        v2.execute_batch("DROP INDEX photos_raw; CREATE INDEX photos_raw ON photos(sort_path);")
            .unwrap();
        assert!(validate_canonical_schema(&v2, SchemaVersion::V2).is_err());
    }
}
