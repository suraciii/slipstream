import type {
  PhotoSummary,
  SelectionCounts,
  SelectionState,
} from "../api/contracts.js";

export type DeferredPhotoDecision = Readonly<{
  selectionState?: SelectionState;
  rating?: number;
  selectionCountsAdjusted?: boolean;
}>;

const adjustedSelectionCounts = (
  counts: SelectionCounts,
  prior: SelectionState,
  next: SelectionState,
): SelectionCounts => {
  if (prior === next) return counts;
  return Object.freeze({
    ...counts,
    [prior]: Math.max(0, counts[prior] - 1),
    [next]: counts[next] + 1,
  });
};

export const recordDeferredPhotoDecision = (
  decisions: Map<string, DeferredPhotoDecision>,
  photoId: string,
  field: "selectionState" | "rating",
  value: SelectionState | number,
  current: PhotoSummary | undefined,
  counts: SelectionCounts,
): SelectionCounts => {
  const prior = decisions.get(photoId);
  let selectionCountsAdjusted = prior?.selectionCountsAdjusted ?? false;
  if (
    field === "selectionState" &&
    !selectionCountsAdjusted &&
    prior?.selectionState === undefined &&
    current !== undefined &&
    current.selectionState !== value
  ) {
    counts = adjustedSelectionCounts(
      counts,
      current.selectionState,
      value as SelectionState,
    );
    selectionCountsAdjusted = true;
  }
  decisions.set(photoId, {
    ...prior,
    ...(field === "selectionState"
      ? { selectionState: value as SelectionState }
      : { rating: value as number }),
    ...(selectionCountsAdjusted ? { selectionCountsAdjusted: true } : {}),
  });
  return counts;
};

export const presentDeferredPhotoFact = (
  decisions: Map<string, DeferredPhotoDecision>,
  photo: PhotoSummary,
  counts: SelectionCounts,
): Readonly<{ photo: PhotoSummary; counts: SelectionCounts }> | undefined => {
  const decision = decisions.get(photo.id);
  if (!decision) return undefined;
  decisions.delete(photo.id);
  const selectionState = decision.selectionState ?? photo.selectionState;
  const rating = decision.rating ?? photo.rating;
  if (
    selectionState !== photo.selectionState &&
    !decision.selectionCountsAdjusted
  )
    counts = adjustedSelectionCounts(
      counts,
      photo.selectionState,
      selectionState,
    );
  return {
    photo:
      selectionState === photo.selectionState && rating === photo.rating
        ? photo
        : Object.freeze({ ...photo, selectionState, rating }),
    counts,
  };
};
