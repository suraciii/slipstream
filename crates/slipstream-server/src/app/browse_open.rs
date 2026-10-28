// Browse Snapshot creation: one source order, its per-state counts, and the
// combined filter conditions that select the frozen Photo ID sequence.
//
// The ordering rule is order, then filter, then freeze: the requested view
// order is resolved against the complete source order, the Selection State
// and defined fact conditions are applied to that order, and the resulting
// sequence is frozen into one Snapshot. Per-state counts are computed over
// the complete ordered source before any filter is applied, so a filtered
// Snapshot reports its own total while its counts still describe the whole
// source.
use super::*;

/// Creates one Browse Snapshot.
///
/// `conditions` carries the defined Photo fact conditions. Every value was
/// validated against its own fact before this call, so no Snapshot is created
/// for an out-of-range Rating, an inverted Rating range, an unknown `kind`
/// value, or a malformed or reversed Capture Time range.
pub(super) async fn open(
    application: &Application,
    source: BrowseSourceRequest,
    order: BrowseViewOrder,
    selection: BrowseSelectionFilter,
    conditions: BrowseFilterConditions,
    preferred_photo_id: Option<&str>,
    resume: bool,
) -> Result<BrowseOpenResponse, ServerError> {
    // Only an Album source owns persisted membership position, so
    // `album-order` is rejected for every other source before any
    // Snapshot is created instead of silently behaving as a time view.
    if order == BrowseViewOrder::AlbumOrder && !matches!(source, BrowseSourceRequest::Album(_)) {
        return Err(ServerError::BrowseOrder);
    }
    // Each arm resolves the complete source order, its per-state counts, the
    // combined filter conditions, and the identity the open should anchor on.
    // Counts always describe the unfiltered source.
    let (photo_ids, selection_counts, anchor_ids): (Vec<String>, SelectionCountsWire, Vec<String>) =
        match source {
            BrowseSourceRequest::Library => {
                let guard = application
                    .shared
                    .snapshot
                    .read()
                    .expect("published Library poisoned");
                let Some(published) = guard.as_ref() else {
                    return Err(ServerError::NotPublished);
                };
                let photo_ids = ordered_library_ids(published, order);
                let selection_counts = selection_counts_for_ids(published, &photo_ids);
                let photo_ids = filter_ids_by_selection(published, photo_ids, selection);
                let photo_ids = filter_ids_by_conditions(published, photo_ids, &conditions);
                (
                    photo_ids,
                    selection_counts,
                    preferred_photo_id.map(str::to_owned).into_iter().collect(),
                )
            }
            BrowseSourceRequest::Folder {
                location,
                publication,
            } => {
                if !crate::folders::valid_folder_location(&location) {
                    return Err(ServerError::FolderInvalid);
                }
                let guard = application
                    .shared
                    .snapshot
                    .read()
                    .expect("published Library poisoned");
                let Some(published) = guard.as_ref() else {
                    return Err(ServerError::NotPublished);
                };
                if published.publication_value() != publication {
                    return Err(ServerError::FileLocationsExpired);
                }
                let index = published.folder_index();
                if !index.is_known(&location) {
                    return Err(ServerError::FolderNotFound);
                }
                let photo_ids = index.filter_photo_ids(
                    &published.snapshot.photos,
                    &published.originals_by_id,
                    &published.snapshot.originals,
                    &location,
                );
                let photo_ids = order_ids_by_capture_time(published, photo_ids, order);
                let selection_counts = selection_counts_for_ids(published, &photo_ids);
                let photo_ids = filter_ids_by_selection(published, photo_ids, selection);
                let photo_ids = filter_ids_by_conditions(published, photo_ids, &conditions);
                (
                    photo_ids,
                    selection_counts,
                    preferred_photo_id.map(str::to_owned).into_iter().collect(),
                )
            }
            BrowseSourceRequest::Album(id) => {
                let target = application
                    .library
                    .album_browse_target(&id)
                    .await?
                    .ok_or(ServerError::BrowseNotFound)?;
                let resume_member_id = (resume && preferred_photo_id.is_none())
                    .then(|| album_resume_member(&target.members, target.saved_photo_id.as_deref()))
                    .flatten();
                let guard = application
                    .shared
                    .snapshot
                    .read()
                    .expect("published Library poisoned");
                let Some(published) = guard.as_ref() else {
                    return Err(ServerError::NotPublished);
                };
                let members = if matches!(order, BrowseViewOrder::AlbumOrder) {
                    target.members
                } else {
                    order_album_members(published, target.members, order)
                };
                let photo_ids: Vec<String> =
                    members.into_iter().map(|member| member.photo_id).collect();
                let selection_counts = selection_counts_for_ids(published, &photo_ids);
                let photo_ids = filter_ids_by_selection(published, photo_ids, selection);
                let photo_ids = filter_ids_by_conditions(published, photo_ids, &conditions);
                let mut anchor_ids: Vec<String> =
                    preferred_photo_id.map(str::to_owned).into_iter().collect();
                if let Some(resume) = resume_member_id {
                    anchor_ids.push(resume);
                }
                (photo_ids, selection_counts, anchor_ids)
            }
        };
    // Positions resolve inside the view the browser actually opens: a
    // filtered Snapshot has its own sequence, so one position has one
    // meaning for windows, identity lookup, and navigation.
    let position = anchor_ids
        .into_iter()
        .find_map(|anchor| photo_ids.iter().position(|id| id == &anchor))
        .unwrap_or(0);
    let token = application
        .retained_queries
        .token(application.browse_namespace, RetainedKind::Browse);
    let total = photo_ids.len();
    application
        .retained_queries
        .insert(
            token.clone(),
            RetainedKind::Browse,
            Some(selection),
            photo_ids,
            Instant::now(),
            SystemTime::now(),
        )
        .map_err(|()| ServerError::QueryCapacity)?;
    Ok(BrowseOpenResponse {
        token,
        total,
        position,
        selection_counts,
    })
}
