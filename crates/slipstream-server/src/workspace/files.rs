use super::*;
use photo_files::{
    EntryKind, Evidence, FileLease, LeaseMode, Observation, PendingFile, fingerprint, identity,
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{Read, Seek, SeekFrom},
    os::fd::AsRawFd,
    path::Path,
};

#[derive(Debug)]
pub(super) struct Record {
    pub id: String,
    pub path: String,
    pub kind: String,
    pub state: String,
    pub evidence: Option<Evidence>,
    pub contract: Option<Value>,
    pub source: Option<String>,
    pub managed: bool,
    pub provenance: Option<Value>,
    pub removed: bool,
    pub facts: Option<Value>,
    pub revision: Option<String>,
}

pub(super) fn record(session: &WorkspaceSession, id: &str) -> Result<Record> {
    session.db.query_row("SELECT id,path,kind,state,evidence,contract,source_revision,managed,provenance,removed,image_facts,revision FROM files WHERE id=?",[id], |r| {
        let evidence:Option<String>=r.get(4)?; let contract:Option<String>=r.get(5)?; let provenance:Option<String>=r.get(8)?; let facts:Option<String>=r.get(10)?;
        let parse = |s:Option<String>|->rusqlite::Result<Option<Value>> { s.map(|s|serde_json::from_str(&s).map_err(|e|rusqlite::Error::FromSqlConversionFailure(0,rusqlite::types::Type::Text,Box::new(e)))).transpose() };
        let evidence=evidence.map(|s|serde_json::from_str(&s).map_err(|e|rusqlite::Error::FromSqlConversionFailure(4,rusqlite::types::Type::Text,Box::new(e)))).transpose()?;
        Ok(Record{id:r.get(0)?,path:r.get(1)?,kind:r.get(2)?,state:r.get(3)?,evidence,contract:parse(contract)?,source:r.get(6)?,managed:r.get::<_,i64>(7)?!=0,provenance:parse(provenance)?,removed:r.get::<_,i64>(9)?!=0,facts:parse(facts)?,revision:r.get(11)?})
    }).optional()?.ok_or_else(||WorkspaceError::new("not_found",404,"Workspace artifact is not indexed"))
}
impl Record {
    pub(super) fn revision(&self) -> Option<String> {
        self.revision
            .clone()
            .or_else(|| self.evidence.as_ref().map(Evidence::revision))
    }
}

fn view(session: &WorkspaceSession, r: &Record) -> Value {
    json!({"workspaceId":session.workspace.id,"artifactId":r.id,"revision":r.revision(),"relativePath":r.path,"filename":Path::new(&r.path).file_name().and_then(|n|n.to_str()),"path":session.directory.path().join(&r.path),"kind":r.kind,"state":if r.removed {"removed"}else{r.state.as_str()},"byteLength":r.evidence.as_ref().map(|e|e.byte_length),"sha256":r.evidence.as_ref().filter(|_|r.state=="ready"&&!r.removed).map(|e|&e.sha256),"evidence":r.evidence,"imageContract":r.contract,"imageFacts":r.facts,"sourceRevision":r.source,"managed":r.managed,"provenance":r.provenance,"availability":if r.removed {"removed"}else{r.state.as_str()},"webUrl":format!("/?photoId={}&workspaceId={}&artifactId={}",session.workspace.photo_id,session.workspace.id,r.id)})
}

pub(super) fn file(session: &WorkspaceSession, id: &str) -> Result<Value> {
    Ok(view(session, &record(session, id)?))
}
pub(super) fn all_files(session: &WorkspaceSession) -> Result<Vec<Value>> {
    let mut q = session
        .db
        .prepare("SELECT id FROM files WHERE removed=0 ORDER BY path,id")?;
    let ids = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if ids.len() > ENTRY_MAX {
        return Err(WorkspaceError::new(
            "resource_limit",
            413,
            "Workspace artifact index exceeds finite query limit",
        ));
    }
    ids.into_iter().map(|id| file(session, &id)).collect()
}

fn scan(
    directory: &photo_files::Directory,
    prefix: &Path,
    seen: &mut BTreeSet<String>,
    count: &mut usize,
) -> Result<()> {
    if prefix.components().count() > 64 {
        return Err(WorkspaceError::new(
            "resource_limit",
            413,
            "Workspace directory nesting exceeds scan limit",
        ));
    }
    for entry in directory.entries(ENTRY_MAX)? {
        *count += 1;
        if *count > ENTRY_MAX {
            return Err(WorkspaceError::new(
                "resource_limit",
                413,
                "Workspace scan exceeds finite entry limit; no partial index is published",
            ));
        }
        if entry.name.starts_with('.') {
            continue;
        }
        let path = prefix.join(&entry.name);
        match entry.kind {
            EntryKind::File => {
                seen.insert(
                    path.to_str()
                        .ok_or_else(|| {
                            WorkspaceError::new("invalid_input", 400, "Workspace path is not UTF-8")
                        })?
                        .into(),
                );
            }
            EntryKind::Directory => scan(&directory.child(&entry.name)?, &path, seen, count)?,
            EntryKind::Symlink | EntryKind::Other => (),
        }
    }
    Ok(())
}

pub(super) fn reconcile(_manager: &WorkspaceManager, session: &mut WorkspaceSession) -> Result<()> {
    let mut seen = BTreeSet::new();
    scan(&session.directory, Path::new(""), &mut seen, &mut 0)?;
    let live = {
        let mut q = session
            .db
            .prepare("SELECT id,path FROM files WHERE removed=0")?;
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (id, path) in &live {
        if !seen.contains(path) {
            session
                .db
                .execute("UPDATE files SET state='missing' WHERE id=?", [id])?;
        }
    }
    for path in seen {
        let id = session
            .db
            .query_row(
                "SELECT id FROM files WHERE path=? AND removed=0",
                [&path],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        let id = match id {
            Some(id) => id,
            None => {
                let id = identity("artifact")?;
                let kind = match layout::component(&path) {
                    Some(layout::PREVIEWS) => "preview",
                    Some(layout::OUTPUTS) => "output",
                    _ => "file",
                };
                session.db.execute("INSERT INTO files(id,path,kind,state,managed,removed) VALUES(?,?,?,'checking',0,0)",params![id,path,kind])?;
                id
            }
        };
        observe(session, &id)?;
    }
    Ok(())
}

fn known_image(path: &str, magic: &[u8]) -> bool {
    let extension = Path::new(path)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "tif"
            | "tiff"
            | "webp"
            | "gif"
            | "bmp"
            | "heic"
            | "heif"
            | "avif"
            | "dng"
            | "raw"
            | "arw"
            | "cr2"
            | "cr3"
            | "nef"
            | "nrw"
            | "orf"
            | "rw2"
            | "raf"
            | "pef"
            | "srw"
    ) || magic.starts_with(b"\x89PNG")
        || magic.starts_with(&[0xff, 0xd8])
        || magic.starts_with(b"II\x2a\0")
        || magic.starts_with(b"MM\0\x2a")
}

fn observe(session: &WorkspaceSession, id: &str) -> Result<()> {
    let old = record(session, id)?;
    if old.removed {
        return Ok(());
    }
    let mut file = match session.directory.open_file(&old.path) {
        Ok(file) => file,
        Err(error) => {
            session.db.execute(
                "UPDATE files SET state=?,provenance=NULL WHERE id=?",
                params![
                    if error.kind() == std::io::ErrorKind::NotFound {
                        "missing"
                    } else {
                        "unavailable"
                    },
                    id
                ],
            )?;
            return Ok(());
        }
    };
    let observation = Observation::of(&file)?;
    if old.state == "ready"
        && old
            .evidence
            .as_ref()
            .is_some_and(|e| e.observation == observation)
    {
        return Ok(());
    }
    // A restored observation is rebased only through explicit manifest validation.
    // A live replacement, even equal bytes, loses provenance and RAW admission.
    let changed = old
        .evidence
        .as_ref()
        .is_some_and(|e| e.observation != observation);
    if changed {
        session.db.execute("UPDATE files SET state='changed',managed=0,source_revision=NULL,contract=NULL,provenance=NULL,image_facts=NULL WHERE id=?",[id])?;
    }
    let checked = (|| -> std::io::Result<(Evidence, Option<Value>)> {
        let evidence = fingerprint(&mut file, FILE_BYTES_MAX)?;
        let protected_raw = old.managed
            && old.kind == "input"
            && old.source.is_some()
            && !changed
            && old.contract.is_none();
        let mut magic = [0u8; 8];
        file.seek(SeekFrom::Start(0))?;
        let n = file.read(&mut magic)?;
        file.seek(SeekFrom::Start(0))?;
        let facts = if !protected_raw && known_image(&old.path, &magic[..n]) {
            let facts = photo_files::image::inspect_image(&mut file, FILE_BYTES_MAX)?;
            Some(
                json!({"format":facts.format,"precision":facts.precision,"channels":facts.channels,"width":facts.width,"height":facts.height,"iccSha256":facts.icc_sha256}),
            )
        } else {
            None
        };
        evidence.observation.verify(&file)?;
        evidence
            .observation
            .verify(&session.directory.open_file(&old.path)?)?;
        Ok((evidence, facts))
    })();
    match checked {
        Ok((evidence, facts)) => {
            let contract = qualified_contract(&mut file, facts.as_ref()).unwrap_or(None);
            session.db.execute("UPDATE files SET state='ready',evidence=?,image_facts=?,contract=COALESCE(?,contract),revision=? WHERE id=?",params![serde_json::to_string(&evidence)?,facts.as_ref().map(Value::to_string),contract.as_ref().map(Value::to_string),if changed{Some(evidence.revision())}else{old.revision().or_else(||Some(evidence.revision()))},id])?;
            if !changed
                && old.contract.is_some()
                && old.evidence.as_ref().is_some_and(|e| {
                    e.sha256 != evidence.sha256 || e.byte_length != evidence.byte_length
                })
            {
                session.db.execute(
                    "UPDATE files SET contract=NULL,provenance=NULL,managed=0 WHERE id=?",
                    [id],
                )?;
            }
        }
        Err(_error) => {
            session.db.execute("UPDATE files SET state='unavailable',contract=NULL,provenance=NULL,image_facts=NULL WHERE id=?",[id])?;
        }
    }
    Ok(())
}

fn qualified_contract(file: &mut std::fs::File, facts: Option<&Value>) -> Result<Option<Value>> {
    let Some(facts) = facts else {
        return Ok(None);
    };
    let profile = facts["iccSha256"].as_str();
    let (color, transfer, encoding, icc) = match (
        facts["format"].as_str(),
        facts["precision"].as_str(),
        profile,
    ) {
        (Some("tiff"), Some("float32"), Some(icc))
            if photo_files::image::tiff::is_qualified_profile(icc) =>
        {
            ("prophoto-rgb", "linear", "deflate", icc)
        }
        (Some("jpeg"), Some("uint8"), Some(icc))
            if icc == photo_files::image::jpeg::OUTPUT_ICC_SHA256 =>
        {
            ("srgb", "srgb", "jpeg", icc)
        }
        _ => return Ok(None),
    };
    let path = std::path::PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    photo_files::image::validate(&path, FILE_BYTES_MAX, icc)?;
    Ok(Some(
        json!({"format":format!("image/{}",facts["format"].as_str().unwrap()),"precision":facts["precision"],"colorSpace":color,"transfer":transfer,"geometry":{"width":facts["width"],"height":facts["height"]},"encoding":encoding}),
    ))
}

pub(super) fn publish(
    manager: &WorkspaceManager,
    session: &mut WorkspaceSession,
    path: &Path,
    provenance: &Value,
) -> Result<Value> {
    let relative = path.strip_prefix(session.directory.path()).map_err(|_| {
        WorkspaceError::new(
            "invalid_input",
            400,
            "Published output is outside the workspace",
        )
    })?;
    if !layout::in_completed_directory(relative.to_str().unwrap_or("")) {
        return Err(WorkspaceError::new(
            "invalid_input",
            400,
            "Completed output must be published under outputs or previews",
        ));
    }
    let relative = relative
        .to_str()
        .ok_or_else(|| WorkspaceError::new("invalid_input", 400, "Output path is not UTF-8"))?;
    reconcile(manager, session)?;
    let id: String = session.db.query_row(
        "SELECT id FROM files WHERE path=? AND removed=0",
        [relative],
        |r| r.get(0),
    )?;
    let row = record(session, &id)?;
    if row.state != "ready" || row.facts.is_none() {
        return Err(WorkspaceError::new(
            "output_invalid",
            409,
            "Exact returned output is not a decoded complete image",
        ));
    }
    let evidence = row.evidence.as_ref().unwrap();
    let digest = provenance["sha256"]
        .as_str()
        .or(provenance["byteEvidence"]["sha256"].as_str());
    let length = provenance["byteLength"]
        .as_u64()
        .or(provenance["length"].as_u64())
        .or(provenance["byteEvidence"]["byteLength"].as_u64());
    if digest != Some(evidence.sha256.as_str())
        || length != Some(evidence.byte_length)
        || provenance["path"]
            .as_str()
            .is_some_and(|p| Path::new(p) != path)
    {
        return Err(WorkspaceError::new(
            "output_invalid",
            409,
            "Output does not match its exact returned receipt",
        ));
    }
    let mut opened = session.directory.open_file(relative)?;
    let _guard = FileLease::acquire(opened.try_clone()?, LeaseMode::Shared, LOCK_WAIT)?;
    if fingerprint(&mut opened, FILE_BYTES_MAX)? != *evidence {
        return Err(WorkspaceError::new(
            "artifact_changed",
            409,
            "Output changed during receipt adoption",
        ));
    }
    let facts = row.facts.as_ref().unwrap();
    if provenance
        .get("iccSha256")
        .is_some_and(|v| v != &facts["iccSha256"])
        || provenance
            .get("dimensions")
            .is_some_and(|v| v["width"] != facts["width"] || v["height"] != facts["height"])
        || provenance
            .get("format")
            .and_then(Value::as_str)
            .is_some_and(|v| v.strip_prefix("image/").unwrap_or(v) != facts["format"])
    {
        return Err(WorkspaceError::new(
            "output_invalid",
            409,
            "Native returned profile, dimensions or format differs from decoded output",
        ));
    }
    let asserted = provenance
        .get("outputContract")
        .or(provenance.get("imageContract"));
    if let Some(asserted) = asserted.filter(|v| !v.is_null()) {
        let facts = row.facts.as_ref().unwrap();
        let format = asserted["format"]
            .as_str()
            .unwrap_or("")
            .strip_prefix("image/")
            .unwrap_or(asserted["format"].as_str().unwrap_or(""));
        if facts["format"] != format
            || facts["precision"] != asserted["precision"]
            || facts["width"] != asserted["geometry"]["width"]
            || facts["height"] != asserted["geometry"]["height"]
        {
            return Err(WorkspaceError::new(
                "output_invalid",
                409,
                "Returned output contract differs from decoded image facts",
            ));
        }
        if row.contract.as_ref() != Some(asserted)
            && !qualified_film_contract(&row, provenance, asserted)
        {
            return Err(WorkspaceError::new(
                "output_invalid",
                409,
                "Returned color or encoding contract is not established by qualified image bytes",
            ));
        }
    }
    evidence
        .observation
        .verify(&session.directory.open_file(relative)?)?;
    if opened.metadata()?.permissions().readonly() {
        session.db.execute(
            "UPDATE files SET managed=1,provenance=?,contract=COALESCE(?,contract) WHERE id=?",
            params![
                encode(provenance)?,
                asserted.filter(|v| !v.is_null()).map(Value::to_string),
                id
            ],
        )?;
    } else {
        return Err(WorkspaceError::new(
            "output_invalid",
            409,
            "Completed managed output is not protected against tool writes",
        ));
    }
    file(session, &id)
}

/// Film's qualified adapter establishes its bounded display conversion and
/// writer encoding; generic discovery establishes only decoded image facts.
fn qualified_film_contract(row: &Record, provenance: &Value, asserted: &Value) -> bool {
    if provenance["module"] != "spektrafilm"
        || asserted["precision"] != "uint8"
        || asserted["colorSpace"] != "srgb"
        || asserted["transfer"] != "srgb"
        || row
            .facts
            .as_ref()
            .is_none_or(|facts| facts["channels"] != 3)
    {
        return false;
    }
    match asserted["format"].as_str() {
        Some("image/png") => provenance["kind"] == "preview" && asserted["encoding"] == "deflate",
        Some("jpeg") => {
            asserted["encoding"] == "quality-85-baseline"
                && row.contract.as_ref().is_some_and(|contract| {
                    contract["format"] == "image/jpeg"
                        && contract["precision"] == asserted["precision"]
                        && contract["colorSpace"] == asserted["colorSpace"]
                        && contract["transfer"] == asserted["transfer"]
                        && contract["geometry"] == asserted["geometry"]
                        && contract["encoding"] == "jpeg"
                })
        }
        _ => false,
    }
}

pub(super) fn inspect(
    manager: &WorkspaceManager,
    session: &mut WorkspaceSession,
    id: &str,
) -> Result<Value> {
    reconcile(manager, session)?;
    let mut value = file(session, id)?;
    let inventory = super::dependencies::inventory(manager, session)?;
    value["dependencies"] = json!(super::dependencies::blockers(
        &inventory,
        Some(&value),
        session.directory.path()
    ));
    Ok(value)
}

pub(super) fn download(
    manager: &WorkspaceManager,
    mut session: WorkspaceSession,
    id: &str,
    revision: Option<&str>,
) -> Result<WorkspaceDownload> {
    reconcile(manager, &mut session)?;
    let row = record(&session, id)?;
    if row.removed || row.state != "ready" {
        return Err(WorkspaceError::new(
            "artifact_unavailable",
            409,
            "Selected artifact is missing, removed, or incomplete",
        ));
    }
    let selected_revision = row.revision().ok_or_else(|| {
        WorkspaceError::new(
            "artifact_unavailable",
            409,
            "Artifact revision is unavailable",
        )
    })?;
    let expected = row.evidence.ok_or_else(|| {
        WorkspaceError::new(
            "artifact_unavailable",
            409,
            "Artifact has no checked byte evidence",
        )
    })?;
    check_revision(&session, id, revision, &expected)?;
    let mut opened = session.directory.open_file(&row.path)?;
    let lease = FileLease::acquire(opened.try_clone()?, LeaseMode::Shared, LOCK_WAIT)?;
    let actual = fingerprint(&mut opened, FILE_BYTES_MAX)?;
    if actual != expected {
        return Err(changed(
            &session,
            id,
            revision.unwrap_or(&expected.revision()),
            Some(&actual.revision()),
        ));
    }
    expected
        .observation
        .verify(&session.directory.open_file(&row.path)?)?;
    opened.seek(SeekFrom::Start(0))?;
    let content_type = row
        .facts
        .as_ref()
        .and_then(|f| f["format"].as_str())
        .map(|f| match f {
            "png" => "image/png",
            "jpeg" => "image/jpeg",
            "tiff" => "image/tiff",
            _ => "application/octet-stream",
        })
        .unwrap_or("application/octet-stream")
        .to_owned();
    let mut leases = session._leases;
    drop(leases.pop());
    leases.push(lease);
    Ok(WorkspaceDownload {
        file: opened,
        filename: Path::new(&row.path)
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("artifact")
            .to_owned(),
        byte_length: expected.byte_length,
        sha256: expected.sha256.clone(),
        revision: selected_revision,
        content_type,
        leases,
    })
}

pub(super) fn check_revision(
    session: &WorkspaceSession,
    id: &str,
    revision: Option<&str>,
    evidence: &Evidence,
) -> Result<()> {
    let current = record(session, id)?
        .revision()
        .unwrap_or_else(|| evidence.revision());
    if let Some(revision) = revision
        && revision != current
    {
        return Err(changed(session, id, revision, Some(&current)));
    }
    Ok(())
}
fn changed(
    session: &WorkspaceSession,
    id: &str,
    expected: &str,
    current: Option<&str>,
) -> WorkspaceError {
    WorkspaceError::new("artifact_changed",409,"Selected artifact revision changed").detail(json!({"workspaceId":session.workspace.id,"artifactId":id,"expectedRevision":expected,"currentRevision":current}))
}

pub(super) fn admit(
    manager: &WorkspaceManager,
    session: &mut WorkspaceSession,
    id: &str,
    revision: &str,
) -> Result<Value> {
    reconcile(manager, session)?;
    let row = record(session, id)?;
    if row.removed || row.state != "ready" || row.kind == "preview" {
        return Err(WorkspaceError::new(
            "artifact_unavailable",
            409,
            "Selected artifact is not a complete processing input",
        ));
    }
    let evidence = row.evidence.as_ref().ok_or_else(|| {
        WorkspaceError::new(
            "artifact_unavailable",
            409,
            "Selected artifact has no validated evidence",
        )
    })?;
    check_revision(session, id, Some(revision), evidence)?;
    if row.facts.is_none() && !(row.managed && row.kind == "input" && row.source.is_some()) {
        return Err(WorkspaceError::new(
            "unsupported_input",
            422,
            "Generic attachments have no admitted image contract",
        ));
    }
    let mut source = session.directory.open_file(&row.path)?;
    let _lease = FileLease::acquire(source.try_clone()?, LeaseMode::Shared, LOCK_WAIT)?;
    if fingerprint(&mut source, FILE_BYTES_MAX)? != *evidence {
        return Err(changed(session, id, revision, None));
    }
    if row.managed {
        return Ok(admission(session, &row));
    }
    if let Some((captured, captured_revision)) = session
        .db
        .query_row(
            "SELECT captured_id,captured_revision FROM captures WHERE artifact_id=? AND revision=?",
            params![id, revision],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    {
        let target = record(session, &captured)?;
        if target.revision().as_deref() != Some(captured_revision.as_str()) {
            return Err(WorkspaceError::new(
                "artifact_changed",
                409,
                "Protected capture changed since its admission",
            ));
        }
        if target.removed || target.state != "ready" {
            return Err(WorkspaceError::new(
                "artifact_unavailable",
                409,
                "Previously admitted snapshot is no longer available; choose a new input explicitly",
            ));
        }
        return Ok(admission(session, &target));
    }
    manager.reserve_storage(evidence.byte_length)?;
    let input = session.directory.create_directory(layout::INPUT)?;
    let artifact = identity("artifact")?;
    let filename = Path::new(&row.path)
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("image");
    let name = format!("{artifact}-{filename}");
    let mut pending = PendingFile::create(&input)?;
    source.seek(SeekFrom::Start(0))?;
    let copied = std::io::copy(
        &mut std::io::Read::by_ref(&mut source).take(evidence.byte_length + 1),
        pending.file_mut(),
    )?;
    evidence.observation.verify(&source)?;
    evidence
        .observation
        .verify(&session.directory.open_file(&row.path)?)?;
    if copied != evidence.byte_length {
        return Err(changed(session, id, revision, None));
    }
    let copied_evidence = fingerprint(pending.file_mut(), FILE_BYTES_MAX)?;
    if copied_evidence.sha256 != evidence.sha256 {
        return Err(changed(session, id, revision, None));
    }
    photo_files::image::inspect_image(pending.file_mut(), FILE_BYTES_MAX)?;
    let published = pending.publish(&name, FILE_BYTES_MAX)?;
    let path = layout::managed_file(layout::INPUT, &name)
        .ok_or_else(|| WorkspaceError::new("invalid_input", 400, "Captured input name is unusable."))?;
    session.db.execute("INSERT INTO files(id,path,kind,state,evidence,contract,managed,provenance,removed,image_facts,revision) VALUES(?,?,'input','ready',?,?,1,?,0,?,?)",params![artifact,path,serde_json::to_string(&published)?,row.contract.as_ref().map(Value::to_string),row.provenance.as_ref().map(Value::to_string),row.facts.as_ref().map(Value::to_string),published.revision()])?;
    let result = admission(session, &record(session, &artifact)?);
    session.db.execute(
        "INSERT INTO captures(artifact_id,revision,captured_id,captured_revision) VALUES(?,?,?,?)",
        params![id, revision, artifact, published.revision()],
    )?;
    Ok(result)
}
fn admission(session: &WorkspaceSession, row: &Record) -> Value {
    json!({"workspaceId":session.workspace.id,"path":session.directory.path().join(&row.path),"artifactId":row.id,"revision":row.revision(),"evidence":row.evidence,"imageContract":row.contract,"imageFacts":row.facts})
}

/// Called only by an explicitly isolated restore after its complete manifest has
/// been verified. Startup and ordinary reconciliation never rebase observations.
pub(super) fn rebase_restored_observations(
    session: &WorkspaceSession,
    manifest: &Value,
) -> Result<()> {
    let entries = manifest.as_array().ok_or_else(|| {
        WorkspaceError::new(
            "invalid_input",
            400,
            "Restore manifest must be a complete array",
        )
    })?;
    if entries.len() > ENTRY_MAX {
        return Err(WorkspaceError::new(
            "resource_limit",
            413,
            "Restore manifest exceeds bound",
        ));
    }
    let rows = all_files(session)?;
    let mut checked = Vec::new();
    for row in rows.into_iter().filter(|v| v["state"] != "removed") {
        let id = row["artifactId"].as_str().unwrap();
        let stored = record(session, id)?;
        if stored.removed || stored.state != "ready" {
            continue;
        }
        let evidence = stored.evidence.ok_or_else(|| {
            WorkspaceError::new(
                "restore_mismatch",
                409,
                "Ready artifact has no retained evidence",
            )
        })?;
        let expected = entries
            .iter()
            .find(|entry| entry["relativePath"] == stored.path)
            .ok_or_else(|| {
                WorkspaceError::new(
                    "restore_mismatch",
                    409,
                    "Restore manifest does not include an indexed artifact",
                )
            })?;
        if expected["sha256"] != evidence.sha256 || expected["byteLength"] != evidence.byte_length {
            return Err(WorkspaceError::new(
                "restore_mismatch",
                409,
                "Restore manifest does not preserve artifact bytes",
            ));
        }
        let mut file = session.directory.open_file(&stored.path)?;
        let current = fingerprint(&mut file, FILE_BYTES_MAX)?;
        if current.sha256 != evidence.sha256 || current.byte_length != evidence.byte_length {
            return Err(WorkspaceError::new(
                "restore_mismatch",
                409,
                "Restored artifact differs from retained evidence",
            ));
        }
        let mut provenance = stored.provenance;
        if let Some(provenance) = provenance.as_mut().and_then(Value::as_object_mut)
            && provenance.get("path").is_some_and(Value::is_string)
        {
            provenance.insert(
                "path".into(),
                json!(session.directory.path().join(&stored.path)),
            );
        }
        checked.push((id.to_owned(), evidence.revision(), current, provenance));
    }
    let transaction = session.db.unchecked_transaction()?;
    for (id, revision, evidence, provenance) in checked {
        transaction.execute(
            "UPDATE files SET evidence=?,revision=COALESCE(revision,?),provenance=? WHERE id=?",
            params![
                serde_json::to_string(&evidence)?,
                revision,
                provenance.as_ref().map(Value::to_string),
                id
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
