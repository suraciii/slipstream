use crate::wire::BrowseSelectionFilter;
use slipstream_core::TrashInspectionRecord;
use std::{
    collections::{HashMap, hash_map::RandomState},
    hash::{BuildHasher, Hasher},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(crate) const CLI_CONTRACT_VERSION: u16 = 2;
pub(crate) const MAXIMUM_LIST_PAGE: usize = 60;
pub(crate) const MAXIMUM_RETAINED_IDS: usize = 1_000_000;
pub(crate) const QUERY_IDLE: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetainedKind {
    Browse,
    Album,
    Photo,
    Trash,
    WorkspaceFiles,
    /// One unavailable-Photo inspection review.
    RecoveryUnavailable,
    /// One Folder-prefix relocation proposal review.
    RecoveryMappings,
}

impl RetainedKind {
    fn cursor_prefix(self) -> char {
        match self {
            Self::Browse => 'b',
            Self::Album => 'a',
            Self::Photo => 'p',
            Self::Trash => 't',
            Self::WorkspaceFiles => 'w',
            Self::RecoveryUnavailable => 'u',
            Self::RecoveryMappings => 'm',
        }
    }

    pub(crate) fn token(self, namespace: u128, counter: u64) -> String {
        format!("{}{:032x}{counter:016x}", self.cursor_prefix(), namespace)
    }

    pub(crate) fn idle(self) -> Duration {
        match self {
            Self::Browse => crate::config::BROWSE_SNAPSHOT_IDLE,
            Self::Album
            | Self::Photo
            | Self::Trash
            | Self::WorkspaceFiles
            | Self::RecoveryUnavailable
            | Self::RecoveryMappings => QUERY_IDLE,
        }
    }
}

pub(crate) struct RetainedQuery {
    pub(crate) kind: RetainedKind,
    pub(crate) ids: Vec<String>,
    pub(crate) trash: Option<Vec<TrashInspectionRecord>>,
    pub(crate) trash_operation: Option<(String, usize)>,
    /// The Selection State filter one Browse Snapshot was created with.
    /// Removal admission reads it, so a reviewed result can only be removed
    /// from the view it was reviewed in.
    pub(crate) filter: Option<BrowseSelectionFilter>,
    pub(crate) last_used: Instant,
    pub(crate) evaluated_at: SystemTime,
}

pub(crate) struct QueryRegistry {
    pub(crate) entries: HashMap<String, RetainedQuery>,
    maximum_collections: usize,
    maximum_ids: usize,
}

impl QueryRegistry {
    pub(crate) fn production() -> Self {
        Self::with_limits(crate::config::MAX_BROWSE_SNAPSHOTS, MAXIMUM_RETAINED_IDS)
    }

    pub(crate) fn with_limits(maximum_collections: usize, maximum_ids: usize) -> Self {
        Self {
            entries: HashMap::new(),
            maximum_collections,
            maximum_ids,
        }
    }

    pub(crate) fn insert(
        &mut self,
        token: String,
        kind: RetainedKind,
        filter: Option<BrowseSelectionFilter>,
        ids: Vec<String>,
        now: Instant,
        evaluated_at: SystemTime,
    ) -> Result<(), ()> {
        self.insert_query(
            token,
            RetainedQuery {
                kind,
                ids,
                trash: None,
                trash_operation: None,
                filter,
                last_used: now,
                evaluated_at,
            },
            now,
        )
    }

    pub(crate) fn insert_trash(
        &mut self,
        token: String,
        items: Vec<TrashInspectionRecord>,
        operation: Option<(String, usize)>,
        now: Instant,
        evaluated_at: SystemTime,
    ) -> Result<(), ()> {
        self.insert_query(
            token,
            RetainedQuery {
                kind: RetainedKind::Trash,
                ids: Vec::new(),
                trash: Some(items),
                trash_operation: operation,
                filter: None,
                last_used: now,
                evaluated_at,
            },
            now,
        )
    }

    fn insert_query(
        &mut self,
        token: String,
        query: RetainedQuery,
        now: Instant,
    ) -> Result<(), ()> {
        self.prune(now);
        let count = query.ids.len() + query.trash.as_ref().map_or(0, Vec::len);
        if count > self.maximum_ids || self.maximum_collections == 0 {
            return Err(());
        }
        while self.entries.len() >= self.maximum_collections
            || self.retained_ids().saturating_add(count) > self.maximum_ids
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, query)| query.last_used)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
        if self.entries.len() >= self.maximum_collections
            || self.retained_ids().saturating_add(count) > self.maximum_ids
        {
            return Err(());
        }
        self.entries.insert(token, query);
        Ok(())
    }

    pub(crate) fn page(
        &mut self,
        token: &str,
        kind: RetainedKind,
        start: usize,
        limit: usize,
        now: Instant,
        now_at: SystemTime,
    ) -> Option<RetainedPage> {
        self.prune(now);
        let query = self.entries.get_mut(token)?;
        if query.kind != kind || query.trash.is_some() || start > query.ids.len() {
            return None;
        }
        query.last_used = now;
        Some(RetainedPage {
            ids: query.ids.iter().skip(start).take(limit).cloned().collect(),
            total: query.ids.len(),
            evaluated_at: query.evaluated_at,
            expires_at: now_at + kind.idle(),
        })
    }

    pub(crate) fn page_trash(
        &mut self,
        token: &str,
        start: usize,
        limit: usize,
        now: Instant,
        now_at: SystemTime,
    ) -> Option<RetainedTrashPage> {
        self.prune(now);
        let query = self.entries.get_mut(token)?;
        if query.kind != RetainedKind::Trash {
            return None;
        }
        let items = query.trash.as_ref()?;
        if start > items.len() {
            return None;
        }
        query.last_used = now;
        Some(RetainedTrashPage {
            items: items.iter().skip(start).take(limit).cloned().collect(),
            operation: query.trash_operation.clone(),
            total: items.len(),
            evaluated_at: query.evaluated_at,
            expires_at: now_at + RetainedKind::Trash.idle(),
        })
    }

    pub(crate) fn position(
        &mut self,
        token: &str,
        id: &str,
        now: Instant,
    ) -> Option<Option<usize>> {
        self.prune(now);
        let query = self.entries.get_mut(token)?;
        if query.kind != RetainedKind::Browse {
            return None;
        }
        query.last_used = now;
        Some(query.ids.iter().position(|candidate| candidate == id))
    }

    /// The complete frozen sequence of one Browse Snapshot together with the
    /// Selection State filter it was created with. Removal reads the whole
    /// reviewed result, never one window of it.
    pub(crate) fn browse_snapshot(
        &mut self,
        token: &str,
        now: Instant,
    ) -> Option<(Vec<String>, Option<BrowseSelectionFilter>)> {
        self.prune(now);
        let query = self.entries.get_mut(token)?;
        if query.kind != RetainedKind::Browse {
            return None;
        }
        query.last_used = now;
        Some((query.ids.clone(), query.filter))
    }

    pub(crate) fn remove(&mut self, token: &str) {
        self.entries.remove(token);
    }
    fn prune(&mut self, now: Instant) {
        self.entries.retain(|_, query| {
            now.checked_duration_since(query.last_used)
                .is_some_and(|elapsed| elapsed < query.kind.idle())
        });
    }

    fn retained_ids(&self) -> usize {
        self.entries
            .values()
            .map(|query| query.ids.len() + query.trash.as_ref().map_or(0, Vec::len))
            .sum()
    }
}

/// Thread-safe ownership boundary for application-retained query snapshots.
///
/// The registry remains the deterministic policy object; this façade owns its
/// synchronization so callers cannot bypass the shared eviction budget or
/// retain a second lock convention.
pub(crate) struct RetainedQueryStore {
    registry: Mutex<QueryRegistry>,
    counter: AtomicU64,
}

impl RetainedQueryStore {
    pub(crate) fn production() -> Self {
        Self {
            registry: Mutex::new(QueryRegistry::production()),
            counter: AtomicU64::new(0),
        }
    }

    pub(crate) fn token(&self, namespace: u128, kind: RetainedKind) -> String {
        kind.token(namespace, self.counter.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) fn insert(
        &self,
        token: String,
        kind: RetainedKind,
        filter: Option<BrowseSelectionFilter>,
        ids: Vec<String>,
        now: Instant,
        evaluated_at: SystemTime,
    ) -> Result<(), ()> {
        self.registry
            .lock()
            .expect("retained queries poisoned")
            .insert(token, kind, filter, ids, now, evaluated_at)
    }

    pub(crate) fn insert_trash(
        &self,
        token: String,
        items: Vec<TrashInspectionRecord>,
        operation: Option<(String, usize)>,
        now: Instant,
        evaluated_at: SystemTime,
    ) -> Result<(), ()> {
        self.registry
            .lock()
            .expect("retained queries poisoned")
            .insert_trash(token, items, operation, now, evaluated_at)
    }

    pub(crate) fn page(
        &self,
        token: &str,
        kind: RetainedKind,
        start: usize,
        limit: usize,
        now: Instant,
        now_at: SystemTime,
    ) -> Option<RetainedPage> {
        self.registry
            .lock()
            .expect("retained queries poisoned")
            .page(token, kind, start, limit, now, now_at)
    }

    pub(crate) fn page_trash(
        &self,
        token: &str,
        start: usize,
        limit: usize,
        now: Instant,
        now_at: SystemTime,
    ) -> Option<RetainedTrashPage> {
        self.registry
            .lock()
            .expect("retained queries poisoned")
            .page_trash(token, start, limit, now, now_at)
    }

    pub(crate) fn position(&self, token: &str, id: &str, now: Instant) -> Option<Option<usize>> {
        self.registry
            .lock()
            .expect("retained queries poisoned")
            .position(token, id, now)
    }

    pub(crate) fn browse_snapshot(
        &self,
        token: &str,
        now: Instant,
    ) -> Option<(Vec<String>, Option<BrowseSelectionFilter>)> {
        self.registry
            .lock()
            .expect("retained queries poisoned")
            .browse_snapshot(token, now)
    }

    pub(crate) fn remove(&self, token: &str) {
        self.registry
            .lock()
            .expect("retained queries poisoned")
            .remove(token);
    }

    #[cfg(test)]
    pub(crate) fn contains_kind(&self, kind: RetainedKind) -> bool {
        self.registry
            .lock()
            .expect("retained queries poisoned")
            .entries
            .values()
            .any(|query| query.kind == kind)
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, token: &str) -> bool {
        self.registry
            .lock()
            .expect("retained queries poisoned")
            .entries
            .contains_key(token)
    }

    #[cfg(test)]
    pub(crate) fn age(&self, token: &str, duration: Duration) -> bool {
        let mut registry = self.registry.lock().expect("retained queries poisoned");
        let Some(query) = registry.entries.get_mut(token) else {
            return false;
        };
        query.last_used = query
            .last_used
            .checked_sub(duration)
            .unwrap_or_else(Instant::now);
        true
    }
}

pub(crate) struct RetainedPage {
    pub(crate) ids: Vec<String>,
    pub(crate) total: usize,
    pub(crate) evaluated_at: SystemTime,
    pub(crate) expires_at: SystemTime,
}

pub(crate) struct RetainedTrashPage {
    pub(crate) items: Vec<TrashInspectionRecord>,
    pub(crate) operation: Option<(String, usize)>,
    pub(crate) total: usize,
    pub(crate) evaluated_at: SystemTime,
    pub(crate) expires_at: SystemTime,
}

pub(crate) struct CursorSigner {
    secret: RandomState,
}

impl CursorSigner {
    pub(crate) fn new() -> Self {
        Self {
            secret: RandomState::new(),
        }
    }

    pub(crate) fn query_cursor(
        &self,
        namespace: u128,
        kind: RetainedKind,
        token: &str,
        offset: usize,
        limit: usize,
    ) -> String {
        self.sign(format!(
            "{}|{namespace:032x}|{token}|{offset}|{limit}",
            kind.cursor_prefix()
        ))
    }

    pub(crate) fn parse_query_cursor(
        &self,
        cursor: &str,
        namespace: u128,
        expected_kind: RetainedKind,
    ) -> Result<QueryCursor, CursorError> {
        let payload = decode_signed_payload(cursor)?;
        let mut parts = payload.split('|');
        let kind = parts.next().and_then(|value| value.chars().next());
        let cursor_namespace = parts
            .next()
            .and_then(|value| u128::from_str_radix(value, 16).ok())
            .ok_or(CursorError::Invalid)?;
        if cursor_namespace != namespace {
            return Err(CursorError::ProcessRestarted);
        }
        if !self.valid_signature(cursor, &payload) {
            return Err(CursorError::Invalid);
        }
        let token = parts
            .next()
            .filter(|value| !value.is_empty())
            .ok_or(CursorError::Invalid)?
            .to_owned();
        let offset = parts
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or(CursorError::Invalid)?;
        let limit = parts
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|limit| (1..=MAXIMUM_LIST_PAGE).contains(limit))
            .ok_or(CursorError::Invalid)?;
        if parts.next().is_some() || kind != Some(expected_kind.cursor_prefix()) {
            return Err(CursorError::Invalid);
        }
        Ok(QueryCursor {
            token,
            offset,
            limit,
        })
    }

    pub(crate) fn folder_cursor(
        &self,
        namespace: u128,
        publication: &str,
        parent: &str,
        offset: usize,
        limit: usize,
    ) -> String {
        self.sign(format!(
            "f|{namespace:032x}|{publication}|{offset}|{limit}|{}",
            hex_encode(parent.as_bytes())
        ))
    }

    pub(crate) fn parse_folder_cursor(
        &self,
        cursor: &str,
        namespace: u128,
    ) -> Result<FolderCursor, CursorError> {
        let payload = decode_signed_payload(cursor)?;
        let mut parts = payload.split('|');
        if parts.next() != Some("f") {
            return Err(CursorError::Invalid);
        }
        let cursor_namespace = parts
            .next()
            .and_then(|value| u128::from_str_radix(value, 16).ok())
            .ok_or(CursorError::Invalid)?;
        if cursor_namespace != namespace {
            return Err(CursorError::PublicationReplaced);
        }
        if !self.valid_signature(cursor, &payload) {
            return Err(CursorError::Invalid);
        }
        let publication = parts
            .next()
            .filter(|value| !value.is_empty())
            .ok_or(CursorError::Invalid)?
            .to_owned();
        let offset = parts
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or(CursorError::Invalid)?;
        let limit = parts
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|limit| (1..=MAXIMUM_LIST_PAGE).contains(limit))
            .ok_or(CursorError::Invalid)?;
        let parent = parts
            .next()
            .and_then(|value| hex_decode(value).ok())
            .and_then(|value| String::from_utf8(value).ok())
            .ok_or(CursorError::Invalid)?;
        if parts.next().is_some() {
            return Err(CursorError::Invalid);
        }
        Ok(FolderCursor {
            publication,
            parent,
            offset,
            limit,
        })
    }

    fn sign(&self, payload: String) -> String {
        let signature = self.signature(&payload);
        format!("{}.{signature:016x}", hex_encode(payload.as_bytes()))
    }

    fn valid_signature(&self, cursor: &str, payload: &str) -> bool {
        let Some((_, raw_signature)) = cursor.rsplit_once('.') else {
            return false;
        };
        u64::from_str_radix(raw_signature, 16).ok() == Some(self.signature(payload))
    }

    fn signature(&self, payload: &str) -> u64 {
        let mut hasher = self.secret.build_hasher();
        hasher.write(payload.as_bytes());
        hasher.finish()
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum CursorError {
    Invalid,
    ProcessRestarted,
    PublicationReplaced,
    /// The retained state behind the cursor expired or was evicted.
    Idle,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct QueryCursor {
    pub(crate) token: String,
    pub(crate) offset: usize,
    pub(crate) limit: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct FolderCursor {
    pub(crate) publication: String,
    pub(crate) parent: String,
    pub(crate) offset: usize,
    pub(crate) limit: usize,
}

fn decode_signed_payload(cursor: &str) -> Result<String, CursorError> {
    let (encoded, signature) = cursor.rsplit_once('.').ok_or(CursorError::Invalid)?;
    if signature.len() != 16 || !signature.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CursorError::Invalid);
    }
    String::from_utf8(hex_decode(encoded).map_err(|_| CursorError::Invalid)?)
        .map_err(|_| CursorError::Invalid)
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn hex_decode(value: &str) -> Result<Vec<u8>, ()> {
    if !value.len().is_multiple_of(2) {
        return Err(());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).ok_or(())?;
            let low = (pair[1] as char).to_digit(16).ok_or(())?;
            Ok((high * 16 + low) as u8)
        })
        .collect()
}

pub(crate) fn format_time(value: SystemTime) -> String {
    OffsetDateTime::from(value)
        .format(&Rfc3339)
        .expect("SystemTime formats as RFC 3339")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_registry_evicts_least_recent_collection_for_count_and_id_pressure() {
        let start = Instant::now();
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut registry = QueryRegistry::with_limits(3, 5);
        registry
            .insert(
                "browse".to_owned(),
                RetainedKind::Browse,
                Some(BrowseSelectionFilter::Rejected),
                vec!["1".to_owned(), "2".to_owned()],
                start,
                wall,
            )
            .unwrap();
        registry
            .insert(
                "album".to_owned(),
                RetainedKind::Album,
                None,
                vec!["3".to_owned()],
                start + Duration::from_secs(1),
                wall,
            )
            .unwrap();
        registry
            .insert(
                "photo".to_owned(),
                RetainedKind::Photo,
                None,
                vec!["4".to_owned(), "5".to_owned(), "6".to_owned()],
                start + Duration::from_secs(2),
                wall,
            )
            .unwrap();
        assert!(!registry.entries.contains_key("browse"));
        assert!(registry.entries.contains_key("album"));
        assert!(registry.entries.contains_key("photo"));
        assert!(
            registry
                .insert(
                    "oversized".to_owned(),
                    RetainedKind::Photo,
                    None,
                    vec!["x".to_owned(); 6],
                    start,
                    wall,
                )
                .is_err()
        );
    }

    #[test]
    fn trash_pages_preserve_evidence_and_expire_after_idle() {
        let start = Instant::now();
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let item = |id: &str, marker: i64, path: &str| TrashInspectionRecord {
            photo_id: id.to_owned(),
            removed_at_ms: marker,
            original_id: format!("original-{id}"),
            relative_path: slipstream_core::RelativeOriginalPath::parse(path).unwrap(),
            kind: slipstream_core::OriginalKind::Jpeg,
            size: 42,
            available: true,
        };
        let records = vec![item("1", 10, "one.jpg"), item("2", 20, "two.jpg")];
        let mut registry = QueryRegistry::with_limits(2, 2);
        registry
            .insert_trash("trash".to_owned(), records.clone(), None, start, wall)
            .unwrap();
        let first = registry.page_trash("trash", 0, 1, start, wall).unwrap();
        assert_eq!(first.items, records[..1]);
        assert_eq!(first.total, 2);
        assert_eq!(first.evaluated_at, wall);
        assert_eq!(first.expires_at, wall + QUERY_IDLE);
        let later = start + Duration::from_secs(60);
        let second = registry.page_trash("trash", 1, 1, later, wall).unwrap();
        assert_eq!(second.items, records[1..]);
        assert!(registry.page_trash("trash", 3, 1, later, wall).is_none());
        assert!(
            registry
                .page("trash", RetainedKind::Trash, 0, 1, later, wall)
                .is_none()
        );
        assert!(
            registry
                .page_trash("trash", 0, 1, later + QUERY_IDLE, wall)
                .is_none()
        );
    }

    #[test]
    fn trash_captures_share_the_registry_budget_and_cursor_kind() {
        let start = Instant::now();
        let wall = SystemTime::UNIX_EPOCH;
        let item = TrashInspectionRecord {
            photo_id: "photo".to_owned(),
            removed_at_ms: 1,
            original_id: "original".to_owned(),
            relative_path: slipstream_core::RelativeOriginalPath::parse("one.jpg").unwrap(),
            kind: slipstream_core::OriginalKind::Jpeg,
            size: 1,
            available: false,
        };
        let mut registry = QueryRegistry::with_limits(2, 2);
        registry
            .insert_trash("trash".to_owned(), vec![item.clone(); 2], None, start, wall)
            .unwrap();
        assert!(
            registry
                .insert_trash("oversized".to_owned(), vec![item; 3], None, start, wall)
                .is_err()
        );
        registry
            .insert(
                "photo".to_owned(),
                RetainedKind::Photo,
                None,
                vec!["1".to_owned()],
                start + Duration::from_secs(1),
                wall,
            )
            .unwrap();
        assert!(!registry.entries.contains_key("trash"));
        let signer = CursorSigner::new();
        let cursor = signer.query_cursor(7, RetainedKind::Trash, "trash", 1, 1);
        assert_eq!(
            signer
                .parse_query_cursor(&cursor, 7, RetainedKind::Trash)
                .unwrap()
                .token,
            "trash"
        );
        assert_eq!(
            signer.parse_query_cursor(&cursor, 7, RetainedKind::Photo),
            Err(CursorError::Invalid)
        );
        assert!(RetainedKind::Trash.token(7, 1).starts_with('t'));
    }

    #[test]
    fn cursors_bind_kind_process_page_and_folder_publication() {
        let signer = CursorSigner::new();
        let cursor = signer.query_cursor(7, RetainedKind::Photo, "token", 60, 60);
        let parsed = signer
            .parse_query_cursor(&cursor, 7, RetainedKind::Photo)
            .unwrap();
        assert_eq!(
            (parsed.token.as_str(), parsed.offset, parsed.limit),
            ("token", 60, 60)
        );
        assert_eq!(
            signer.parse_query_cursor(&cursor, 8, RetainedKind::Photo),
            Err(CursorError::ProcessRestarted)
        );
        assert_eq!(
            signer.parse_query_cursor(&cursor, 7, RetainedKind::Album),
            Err(CursorError::Invalid)
        );
        let folder = signer.folder_cursor(7, "publication", "a/b", 2, 1);
        let parsed = signer.parse_folder_cursor(&folder, 7).unwrap();
        assert_eq!(
            (
                parsed.publication.as_str(),
                parsed.parent.as_str(),
                parsed.offset,
                parsed.limit
            ),
            ("publication", "a/b", 2, 1)
        );
        assert_eq!(
            signer.parse_folder_cursor(&folder, 8),
            Err(CursorError::PublicationReplaced)
        );
    }
}
