//! Bounded reviewed Location Recovery state.
//!
//! One review retains the evaluated membership of one unavailable-Photo
//! inspection or one Folder-prefix proposal. Every later page reads that
//! retained membership, so a Photo that is recovered, moved to Trash, or
//! newly unavailable cannot shift another reviewed position. Reviews expire
//! on idle and are evicted least-recently-used under pressure; an expired or
//! evicted continuation fails explicitly instead of silently re-evaluating a
//! different set.

use crate::queries::{QUERY_IDLE, RetainedKind};
use crate::wire::{RecoveryItemWire, RecoveryMappingWire};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

/// One review window is bounded by the shared list page bound: a caller
/// walks a long review in explicit pages instead of receiving an unbounded
/// evaluation.
pub(crate) const MAXIMUM_RECOVERY_PAGE: usize = crate::queries::MAXIMUM_LIST_PAGE;
/// One Folder-prefix review evaluates at most this many unavailable
/// Originals. A larger scope is refused before any content is read.
pub(crate) const MAXIMUM_RECOVERY_MAPPINGS: usize = 10_000;
/// One apply batch commits at most this many reviewed mappings.
pub(crate) const MAXIMUM_RECOVERY_APPLY: usize = 100;

/// Reviewed membership is retained for the shared query idle interval: long
/// enough to walk a bounded review, short enough that stale evaluations do
/// not accumulate.
pub(crate) const RECOVERY_REVIEW_IDLE: Duration = QUERY_IDLE;
const MAXIMUM_RECOVERY_REVIEWS: usize = 8;
const MAXIMUM_RETAINED_RECOVERY_ITEMS: usize = 20_000;

/// The retained membership of one review.
pub(crate) enum RecoveryReviewItems {
    Unavailable(Vec<RecoveryItemWire>),
    Mappings(Vec<RecoveryMappingWire>),
}

impl RecoveryReviewItems {
    pub(crate) fn kind(&self) -> RetainedKind {
        match self {
            Self::Unavailable(_) => RetainedKind::RecoveryUnavailable,
            Self::Mappings(_) => RetainedKind::RecoveryMappings,
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Unavailable(items) => items.len(),
            Self::Mappings(items) => items.len(),
        }
    }
}

struct RecoveryReview {
    items: RecoveryReviewItems,
    last_used: Instant,
    evaluated_at: SystemTime,
}

/// The public shape of one review window, shared by both review kinds.
pub(crate) struct RecoveryPage {
    pub(crate) items: RecoveryReviewItems,
    pub(crate) total: usize,
    pub(crate) evaluated_at: SystemTime,
    pub(crate) expires_at: SystemTime,
}

pub(crate) struct RecoveryReviews {
    entries: HashMap<String, RecoveryReview>,
}

impl RecoveryReviews {
    pub(crate) fn production() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Retains one evaluated review, evicting least recently used reviews
    /// when the process budget is reached. `Err` means the review is larger
    /// than the whole budget and must not be retained.
    pub(crate) fn insert(
        &mut self,
        token: String,
        items: RecoveryReviewItems,
        now: Instant,
        evaluated_at: SystemTime,
    ) -> Result<(), ()> {
        self.prune(now);
        let retained = items.len();
        if retained > MAXIMUM_RETAINED_RECOVERY_ITEMS {
            return Err(());
        }
        while self.entries.len() >= MAXIMUM_RECOVERY_REVIEWS
            || self.retained_items().saturating_add(retained) > MAXIMUM_RETAINED_RECOVERY_ITEMS
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, review)| review.last_used)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
        if self.entries.len() >= MAXIMUM_RECOVERY_REVIEWS
            || self.retained_items().saturating_add(retained) > MAXIMUM_RETAINED_RECOVERY_ITEMS
        {
            return Err(());
        }
        self.entries.insert(
            token,
            RecoveryReview {
                items,
                last_used: now,
                evaluated_at,
            },
        );
        Ok(())
    }

    /// One window of a retained review. `None` means the review expired, was
    /// evicted, or does not match the requested kind.
    pub(crate) fn page(
        &mut self,
        token: &str,
        kind: RetainedKind,
        start: usize,
        limit: usize,
        now: Instant,
        now_at: SystemTime,
    ) -> Option<RecoveryPage> {
        self.prune(now);
        let review = self.entries.get_mut(token)?;
        if review.items.kind() != kind || start > review.items.len() {
            return None;
        }
        let items = match &review.items {
            RecoveryReviewItems::Unavailable(items) => RecoveryReviewItems::Unavailable(
                items.iter().skip(start).take(limit).cloned().collect(),
            ),
            RecoveryReviewItems::Mappings(items) => RecoveryReviewItems::Mappings(
                items.iter().skip(start).take(limit).cloned().collect(),
            ),
        };
        review.last_used = now;
        Some(RecoveryPage {
            items,
            total: review.items.len(),
            evaluated_at: review.evaluated_at,
            expires_at: now_at + kind.idle(),
        })
    }

    fn prune(&mut self, now: Instant) {
        self.entries.retain(|_, review| {
            now.checked_duration_since(review.last_used)
                .is_some_and(|elapsed| elapsed < RECOVERY_REVIEW_IDLE)
        });
    }

    fn retained_items(&self) -> usize {
        self.entries.values().map(|review| review.items.len()).sum()
    }
}
