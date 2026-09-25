use crate::wire::BrowseSelectionFilter;
use std::{
    collections::{HashMap, hash_map::RandomState},
    hash::{BuildHasher, Hasher},
    time::{Duration, Instant, SystemTime},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(crate) const CLI_CONTRACT_VERSION: u16 = 1;
pub(crate) const MAXIMUM_LIST_PAGE: usize = 60;
pub(crate) const MAXIMUM_RETAINED_IDS: usize = 1_000_000;
pub(crate) const QUERY_IDLE: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetainedKind {
    Browse,
    Album,
    Photo,
}

impl RetainedKind {
    fn cursor_prefix(self) -> char {
        match self {
            Self::Browse => 'b',
            Self::Album => 'a',
            Self::Photo => 'p',
        }
    }

    fn idle(self) -> Duration {
        match self {
            Self::Browse => crate::config::BROWSE_SNAPSHOT_IDLE,
            Self::Album | Self::Photo => QUERY_IDLE,
        }
    }
}

pub(crate) struct RetainedQuery {
    pub(crate) kind: RetainedKind,
    pub(crate) ids: Vec<String>,
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
        self.prune(now);
        if ids.len() > self.maximum_ids || self.maximum_collections == 0 {
            return Err(());
        }
        while self.entries.len() >= self.maximum_collections
            || self.retained_ids().saturating_add(ids.len()) > self.maximum_ids
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
            || self.retained_ids().saturating_add(ids.len()) > self.maximum_ids
        {
            return Err(());
        }
        self.entries.insert(
            token,
            RetainedQuery {
                kind,
                ids,
                filter,
                last_used: now,
                evaluated_at,
            },
        );
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
        if query.kind != kind || start > query.ids.len() {
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
        self.entries.values().map(|query| query.ids.len()).sum()
    }
}

pub(crate) struct RetainedPage {
    pub(crate) ids: Vec<String>,
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
