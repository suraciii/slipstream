use super::*;
use crate::queries::{CursorSigner, QueryRegistry, RetainedKind, format_time};
use std::{
    collections::HashMap,
    time::{Instant, SystemTime},
};

pub(super) struct Queries {
    registry: QueryRegistry,
    signer: CursorSigner,
    namespace: u128,
    scopes: HashMap<String, String>,
}
impl Queries {
    pub(super) fn new() -> Self {
        Self {
            registry: QueryRegistry::with_limits(64, 1_000_000),
            signer: CursorSigner::new(),
            namespace: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            scopes: HashMap::new(),
        }
    }
}
impl WorkspaceManager {
    pub(crate) fn resolve_photo(&self, photo_id: &str) -> Result<Value> {
        let workspace = self.lookup_photo(photo_id)?.ok_or_else(|| {
            WorkspaceError::new(
                "not_found",
                404,
                "Open the Photo Workspace before using an editing tool.",
            )
        })?;
        self.show(&workspace.id)
    }
    pub(crate) fn show(&self, id: &str) -> Result<Value> {
        let mut session = self.locked(id, LeaseMode::Shared)?;
        files::reconcile(self, &mut session)?;
        let inventory = dependencies::inventory(self, &session)?;
        let all = files::all_files(&session)?;
        let default = store::setting(&session, "defaultInput")?;
        let default_item = default.as_str().and_then(|id| {
            all.iter()
                .find(|item| item["artifactId"].as_str() == Some(id))
        });
        let default_path = default_item
            .and_then(|item| item["relativePath"].as_str())
            .map(|path| session.directory.path().join(path));
        let observed_source =
            (self.resolve)(&session.workspace.photo_id).and_then(|source| source.revision);
        let source_matches = default_item
            .and_then(|item| item["sourceRevision"].as_str())
            .zip(observed_source.as_deref())
            .map(|(prepared, current)| prepared == current);
        let input_revision = default_item.and_then(|item| item.get("revision")).cloned();
        let input_availability = default_item
            .and_then(|item| item.get("state"))
            .cloned()
            .unwrap_or_else(|| json!("absent"));
        let mut counts = serde_json::Map::new();
        let mut bytes = 0_u64;
        for file in &all {
            let key = match file["kind"].as_str() {
                Some("input") => "input",
                Some("preview") => "preview",
                Some("output") => "output",
                Some("resource") => "resource",
                _ => "otherFile",
            };
            let count = counts.get(key).and_then(Value::as_u64).unwrap_or(0) + 1;
            counts.insert(key.to_owned(), json!(count));
            bytes = bytes.saturating_add(file["byteLength"].as_u64().unwrap_or(0));
        }
        for kind in ["input", "preview", "output", "resource", "otherFile"] {
            counts.entry(kind.to_owned()).or_insert(json!(0));
        }
        let edits = inventory.get("edits").cloned().unwrap_or_else(|| json!([]));
        counts.insert(
            "editState".to_owned(),
            json!(edits.as_array().map_or(0, Vec::len)),
        );
        let listing = self.new_page(id, None, all, 50)?;
        let host_id = std::env::var("HOSTNAME")
            .ok()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "service-host".to_owned());
        Ok(json!({
            "workspaceId":id,"photoId":session.workspace.photo_id,"path":session.directory.path(),
            "inputPath":default_path,"inputArtifactId":default,"inputRevision":input_revision,
            "inputAvailability":input_availability,
            "previewDirectory":session.directory.path().join("previews"),"outputDirectory":session.directory.path().join("outputs"),
            "suggestedEditStatePaths":{"darktable":session.directory.path().join(".edits/darktable/main")},
            "execution":{"hostId":host_id,"mode":"service-host","accessibleToCaller":false,"supervisedTools":self.supervised_tools.lock().clone()},
            "observedSourceRevision":observed_source,"inputMatchesObservedSource":source_matches,
            "workspaceStateToken":token(&session,&inventory)?,"edits":edits,"files":listing,
            "deletionSummary":{"counts":counts,"totalKnownBytes":bytes,"externalDependencies":inventory.get("externalDependencies").cloned().unwrap_or_else(||json!([])),"activeOperations":inventory.get("activeOperations").cloned().unwrap_or_else(||json!([])),"unknown":inventory.get("unknown").cloned().unwrap_or_else(||json!([])),"sample":listing.get("items"),"nextCursor":listing.get("nextCursor")},
            "webUrl":format!("/?photoId={}&edit=1",session.workspace.photo_id)
        }))
    }
    pub(crate) fn files(
        &self,
        id: Option<&str>,
        kind: Option<&str>,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<Value> {
        if !(1..=60).contains(&limit) {
            return Err(WorkspaceError::new(
                "invalid_input",
                400,
                "File page size must be 1 through 60.",
            ));
        }
        if let Some(cursor) = cursor {
            if id.is_some() || kind.is_some() {
                return Err(WorkspaceError::new(
                    "invalid_input",
                    400,
                    "A continuation cursor cannot be combined with a new file query.",
                ));
            }
            return self.continue_page(cursor);
        }
        let id = id.ok_or_else(|| {
            WorkspaceError::new(
                "invalid_input",
                400,
                "A workspace identity or continuation cursor is required.",
            )
        })?;
        if kind.is_some_and(|kind| {
            !matches!(kind, "input" | "preview" | "output" | "resource" | "file")
        }) {
            return Err(WorkspaceError::new(
                "invalid_input",
                400,
                "Choose a disclosed file kind.",
            ));
        }
        let mut session = self.locked(id, LeaseMode::Shared)?;
        files::reconcile(self, &mut session)?;
        self.new_page(id, kind, files::all_files(&session)?, limit)
    }
    fn new_page(
        &self,
        id: &str,
        kind: Option<&str>,
        all: Vec<Value>,
        limit: usize,
    ) -> Result<Value> {
        let items = all
            .into_iter()
            .filter(|item| kind.is_none_or(|kind| item["kind"].as_str() == Some(kind)))
            .collect::<Vec<_>>();
        let total = items.len();
        let now = SystemTime::now();
        let expires = now + crate::queries::QUERY_IDLE;
        let next = if total > limit {
            let mut queries = self.queries.lock();
            let token = photo_files::identity("files")?;
            queries
                .registry
                .insert(
                    token.clone(),
                    RetainedKind::WorkspaceFiles,
                    None,
                    items
                        .iter()
                        .filter_map(|item| item["artifactId"].as_str().map(str::to_owned))
                        .collect(),
                    Instant::now(),
                    now,
                )
                .map_err(|()| {
                    WorkspaceError::new(
                        "resource_limit",
                        503,
                        "Too many retained file queries; finish an existing query first.",
                    )
                })?;
            let retained = queries
                .registry
                .entries
                .keys()
                .cloned()
                .collect::<std::collections::HashSet<_>>();
            queries.scopes.retain(|key, _| retained.contains(key));
            queries.scopes.insert(token.clone(), id.to_owned());
            Some(queries.signer.query_cursor(
                queries.namespace,
                RetainedKind::WorkspaceFiles,
                &token,
                limit,
                limit,
            ))
        } else {
            None
        };
        Ok(
            json!({"items":items.into_iter().take(limit).collect::<Vec<_>>(),"total":total,"nextCursor":next,"evaluatedAt":format_time(now),"expiresAt":format_time(expires)}),
        )
    }
    fn continue_page(&self, cursor: &str) -> Result<Value> {
        let (id, page, next) = {
            let mut queries = self.queries.lock();
            let parsed = queries
                .signer
                .parse_query_cursor(cursor, queries.namespace, RetainedKind::WorkspaceFiles)
                .map_err(|_| {
                    WorkspaceError::new(
                        "cursor_expired",
                        410,
                        "The file query is unavailable; list the workspace again.",
                    )
                })?;
            let id = queries.scopes.get(&parsed.token).cloned().ok_or_else(|| {
                WorkspaceError::new(
                    "cursor_expired",
                    410,
                    "The file query expired; list the workspace again.",
                )
            })?;
            let page = queries
                .registry
                .page(
                    &parsed.token,
                    RetainedKind::WorkspaceFiles,
                    parsed.offset,
                    parsed.limit,
                    Instant::now(),
                    SystemTime::now(),
                )
                .ok_or_else(|| {
                    WorkspaceError::new(
                        "cursor_expired",
                        410,
                        "The file query expired; list the workspace again.",
                    )
                })?;
            let next_offset = parsed.offset + page.ids.len();
            let next = (next_offset < page.total).then(|| {
                queries.signer.query_cursor(
                    queries.namespace,
                    RetainedKind::WorkspaceFiles,
                    &parsed.token,
                    next_offset,
                    parsed.limit,
                )
            });
            (id, page, next)
        };
        let mut session = self.locked(&id, LeaseMode::Shared)?;
        files::reconcile(self, &mut session)?;
        let items = page
            .ids
            .iter()
            .map(|id| {
                files::file(&session, id).or_else(|error| {
                    if error.code == "not_found" {
                        Ok(json!({"artifactId":id,"state":"missing"}))
                    } else {
                        Err(error)
                    }
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(
            json!({"items":items,"total":page.total,"nextCursor":next,"evaluatedAt":format_time(page.evaluated_at),"expiresAt":format_time(page.expires_at)}),
        )
    }
}
