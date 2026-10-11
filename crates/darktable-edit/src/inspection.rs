use crate::ToolError;
use photo_files::{Directory, FileLease};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
fn invalid(error: impl std::fmt::Display) -> ToolError {
    ToolError::new("state_unavailable", error.to_string(), 6)
}
fn document(text: String) -> Result<Value, ToolError> {
    serde_json::from_str(&text).map_err(invalid)
}
fn revision(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let parse = |index| {
        let text: String = row.get(index)?;
        serde_json::from_str::<Value>(&text).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })
    };
    Ok(
        json!({"revision":row.get::<_,String>(0)?,"parent":row.get::<_,Option<String>>(1)?,"createdAt":row.get::<_,String>(2)?,"requestId":row.get::<_,Option<String>>(3)?,"snapshot":parse(4)?,"input":parse(5)?,"bundleId":row.get::<_,String>(6)?,"schemaVersion":row.get::<_,String>(7)?,"dependencies":parse(8)?,"changedTargets":parse(9)?}),
    )
}
/// Inspect canonical retained evidence under the host's existing admission barrier.
/// The caller retains the state directory's shared lifecycle lease for this call.
/// This read-only entry never acquires ancestors or initializes native state.
pub fn inspect_retained_directory(
    directory: &Directory,
    lease: &FileLease,
) -> Result<Value, ToolError> {
    let held = lease.file().metadata().map_err(invalid)?;
    let directory_facts = directory.descriptor().metadata().map_err(invalid)?;
    if !held.is_dir() || held.dev() != directory_facts.dev() || held.ino() != directory_facts.ino()
    {
        return Err(invalid(
            "Inspection lease does not identify retained state directory",
        ));
    }
    directory.verify_path().map_err(invalid)?;
    let database = directory.open_file("state.sqlite3").map_err(invalid)?;
    let path = format!(
        "/proc/self/fd/{}/state.sqlite3",
        directory.descriptor().as_raw_fd()
    );
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(invalid)?;
    connection.execute_batch("BEGIN").map_err(invalid)?;
    let integrity: String = connection
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(invalid)?;
    if integrity != "ok" {
        return Err(invalid("Canonical state database is corrupt"));
    }
    let identity = document(
        connection
            .query_row("SELECT value FROM metadata WHERE key='identity'", [], |r| {
                r.get(0)
            })
            .map_err(invalid)?,
    )?;
    if !identity.is_string() {
        return Err(invalid("Invalid state identity"));
    }
    let current = document(
        connection
            .query_row("SELECT value FROM metadata WHERE key='current'", [], |r| {
                r.get(0)
            })
            .map_err(invalid)?,
    )?;
    let baseline = document(
        connection
            .query_row("SELECT value FROM metadata WHERE key='baseline'", [], |r| {
                r.get(0)
            })
            .map_err(invalid)?,
    )?;
    let mut query=connection.prepare("SELECT id,parent,created,request,snapshot,input,bundle,schema,dependencies,targets FROM revisions").map_err(invalid)?;
    let mut revisions = Vec::new();
    for row in query.query_map([], revision).map_err(invalid)? {
        let revision = row.map_err(invalid)?;
        if !revision["snapshot"].is_object()
            || !revision["input"].is_object()
            || !revision["dependencies"].is_array()
        {
            return Err(invalid("Incomplete retained revision"));
        }
        revisions.push(revision);
    }
    let selected = revisions
        .iter()
        .find(|v| v["revision"] == current)
        .ok_or_else(|| invalid("Current revision absent"))?
        .clone();
    if !revisions.iter().any(|v| v["revision"] == baseline) {
        return Err(invalid("Baseline revision absent"));
    }
    let mut query = connection
        .prepare("SELECT id,revision,snapshot,input,dependencies,result FROM captures")
        .map_err(invalid)?;
    let mut captures = Vec::new();
    let mut renders = Vec::new();
    for row in query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(invalid)?
    {
        let (id, revision, snapshot, input, dependencies, result) = row.map_err(invalid)?;
        let mut capture = document(snapshot)?;
        if capture["captureId"] != id
            || capture["revision"] != revision
            || capture["input"] != document(input)?
            || capture["dependencies"] != document(dependencies)?
        {
            return Err(invalid("Inconsistent retained capture evidence"));
        }
        if let Some(result) = result {
            let result = document(result)?;
            capture["status"] = json!(if result["fileCommitted"] == true {
                "succeeded"
            } else {
                result["outcome"].as_str().unwrap_or("failed")
            });
            capture["result"] = result.clone();
            renders.push(result);
        } else {
            capture["status"] = json!("accepted");
            capture["result"] = Value::Null;
        }
        captures.push(capture);
    }
    drop(database);
    directory.verify_path().map_err(invalid)?;
    Ok(
        json!({"stateDirectory":directory.path(),"input":selected["input"],"revision":current,"baselineRevision":baseline,"bundleId":selected["bundleId"],"schemaVersion":selected["schemaVersion"],"snapshot":selected["snapshot"],"dependencies":selected["dependencies"],"retainedRevisions":revisions,"captures":captures,"renders":renders,"completedRenders":renders}),
    )
}
