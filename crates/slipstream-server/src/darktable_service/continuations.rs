use super::*;
use rusqlite::{Connection, OptionalExtension, params};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
impl ToolService {
    fn continuation_db(&self) -> Result<Connection> {
        match self.root.create_file("continuations.sqlite3", 0o600) {
            Ok(file) => file.sync_all().map_err(ServiceError::io)?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                self.root
                    .open_file("continuations.sqlite3")
                    .map_err(ServiceError::io)?;
            }
            Err(e) => return Err(ServiceError::io(e)),
        }
        let db = Connection::open(self.root.path().join("continuations.sqlite3"))
            .map_err(ServiceError::io)?;
        db.busy_timeout(Duration::from_secs(5))
            .map_err(ServiceError::io)?;
        db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS continuations(token TEXT PRIMARY KEY, operation TEXT NOT NULL, photo TEXT, edit TEXT, touched INTEGER NOT NULL);").map_err(ServiceError::io)?;
        Ok(db)
    }
    pub(super) fn continuation_scope(
        &self,
        token: &str,
        operation: &str,
    ) -> Result<(Option<String>, Option<String>)> {
        if !valid_id(token) {
            return Err(ServiceError::new("invalid_cursor", "Invalid cursor", 400));
        }
        let db = self.continuation_db()?;
        let row = db
            .query_row(
                "SELECT operation,photo,edit,touched FROM continuations WHERE token=?",
                [token],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(ServiceError::io)?;
        let (collection, photo, edit, touched) = row.ok_or_else(|| {
            ServiceError::new(
                "cursor_expired",
                "Cursor expired or is not authorized on this host",
                400,
            )
            .detail(json!({"cursorKind": "darktable", "reason": "idle_or_evicted"}))
        })?;
        if touched <= now() - 900 {
            return Err(ServiceError::new(
                "cursor_expired",
                "Cursor expired after 15 minutes idle",
                400,
            )
            .detail(json!({"cursorKind": "darktable", "reason": "idle_or_evicted"})));
        }
        if collection != operation {
            return Err(ServiceError::new(
                "invalid_cursor",
                "Cursor belongs to another collection",
                400,
            ));
        }
        Ok((photo, edit))
    }
    pub(super) fn retain_continuations(
        &self,
        result: &Value,
        operation: &str,
        scope: Option<&(String, String, String)>,
        consumed: Option<&str>,
    ) -> Result<()> {
        let mut tokens = Vec::new();
        if let Some(token) = result["nextCursor"].as_str() {
            tokens.push((token, operation));
        }
        if operation == "edit show" {
            for (key, operation) in [
                ("instances", "edit instances list"),
                ("masks", "edit masks list"),
                ("actions", "edit actions list"),
            ] {
                if let Some(token) = result[key]["nextCursor"].as_str() {
                    tokens.push((token, operation));
                }
            }
        }
        if tokens.is_empty() && consumed.is_none() {
            return Ok(());
        }
        let mut db = self.continuation_db()?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(ServiceError::io)?;
        tx.execute("DELETE FROM continuations WHERE touched<=?", [now() - 900])
            .map_err(ServiceError::io)?;
        if let Some(token) = consumed {
            tx.execute("DELETE FROM continuations WHERE token=?", [token])
                .map_err(ServiceError::io)?;
        }
        for (token, operation) in tokens {
            if !valid_id(token) {
                return Err(ServiceError::unknown("Tool returned invalid cursor"));
            }
            let (photo, edit) = scope
                .map(|(p, _, e)| (Some(p.as_str()), Some(e.as_str())))
                .unwrap_or((None, None));
            let changed = tx.execute("INSERT INTO continuations VALUES (?,?,?,?,?) ON CONFLICT(token) DO UPDATE SET touched=excluded.touched WHERE continuations.operation=excluded.operation AND continuations.photo IS excluded.photo AND continuations.edit IS excluded.edit", params![token,operation,photo,edit,now()]).map_err(ServiceError::io)?;
            if changed != 1 {
                return Err(ServiceError::unknown(
                    "Tool cursor collides with another authorized scope",
                ));
            }
        }
        tx.execute("DELETE FROM continuations WHERE token IN (SELECT token FROM continuations ORDER BY touched DESC,token DESC LIMIT -1 OFFSET 256)", []).map_err(ServiceError::io)?;
        tx.commit().map_err(ServiceError::io)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_only_routes_reauthorize_photo_and_retain_embedded_collection() {
        let path = std::env::temp_dir().join(photo_files::identity("host-cursor-test").unwrap());
        let root = Directory::create(&path).unwrap();
        let workspaces = Arc::new(
            WorkspaceManager::open(&path, path.clone(), Arc::new(|_| None), 1024 * 1024 * 1024)
                .unwrap(),
        );
        let service = ToolService {
            workspaces,
            jobs: jobs::Jobs::open(&root).unwrap(),
            root,
        };
        let scope = (
            "photo-inaccessible".into(),
            "workspace-test".into(),
            "edit-test".into(),
        );
        service
            .retain_continuations(
                &json!({"instances":{"nextCursor":"cursor-test"}}),
                "edit show",
                Some(&scope),
                None,
            )
            .unwrap();
        assert_eq!(
            service
                .continuation_scope("cursor-test", "edit instances list")
                .unwrap(),
            (Some("photo-inaccessible".into()), Some("edit-test".into()))
        );
        assert_eq!(
            service
                .continuation_scope("cursor-test", "history list")
                .unwrap_err()
                .value["error"]["code"],
            "invalid_cursor"
        );
        // The token supplies selectors, never Photo access: the absent Photo is refused before invoking the tool.
        assert!(
            service
                .command(
                    None,
                    None,
                    "edit instances list",
                    json!({"cursor":"cursor-test"})
                )
                .is_err()
        );
        assert_eq!(
            service
                .command(
                    Some("other-photo"),
                    None,
                    "edit instances list",
                    json!({"cursor":"cursor-test"})
                )
                .unwrap_err()
                .value["error"]["code"],
            "invalid_cursor"
        );
        service
            .continuation_db()
            .unwrap()
            .execute("UPDATE continuations SET touched=0", [])
            .unwrap();
        let expired = service
            .continuation_scope("cursor-test", "edit instances list")
            .unwrap_err();
        assert_eq!(expired.value["error"]["code"], "cursor_expired");
        assert_eq!(
            expired.value["error"]["details"],
            json!({"cursorKind": "darktable", "reason": "idle_or_evicted"}),
            "an expired darktable continuation discloses its route and reason"
        );
        drop(service);
        std::fs::remove_dir_all(path).unwrap();
    }
}
