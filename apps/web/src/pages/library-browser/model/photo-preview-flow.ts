import type { PhotoSummary, PreviewSource } from "../api/contracts.js";
import type { RecoveryTransition } from "./async-ownership.js";
import type { PhotoOwner } from "./photo-owner.js";
import type {
  PhotoAuthority,
  PhotoOwnerEvent,
} from "./photo-preview-outcome.js";
import type { PhotoWindowAuthority } from "./source-grid-owner.js";

export interface PreviewFlowView {
  readonly photoStatusSurface: object;
  isPhotoStatusSurfaceCurrent(surface: object): boolean;
  setPhotoStatus(text: string): void;
  showPreviewUnavailable(text: string): void;
  reviewImageMatches(url: string): boolean;
  setPreviewFacts(source: PreviewSource | undefined, limited: boolean): void;
}

export type PhotoPreviewFlowDependencies = Readonly<{
  view: PreviewFlowView;
  photoOwner: PhotoOwner;
  sourceGrid: Readonly<{
    readonly total: number;
    photoAt(index: number): PhotoSummary | undefined;
  }>;
  currentPhotoId: () => string | undefined;
  editorReferenceChanged: (photoId: string) => void;
  failPhotoRecovery: (
    authority: PhotoAuthority,
    surface: "preview",
    transition?: RecoveryTransition,
  ) => void;
  renderReviewImage: (url: string, authority: PhotoAuthority) => void;
  updateControls: () => void;
  loadPhotoWindow: (
    index: number,
    windowAuthority: PhotoWindowAuthority,
    authority: PhotoAuthority,
  ) => Promise<unknown>;
  renderFilmstrip: () => void;
}>;

export interface PhotoPreviewFlow {
  showPreview(
    authority: PhotoAuthority,
    transition?: RecoveryTransition,
    refresh?: boolean,
  ): Promise<boolean>;
  prefetchAdjacent(index: number, authority: PhotoAuthority): Promise<void>;
}

/// Bounds automatic retries of a failed Preview URL so one broken transfer
/// cannot loop, while a later failure of a different URL still recovers.
export function createPreviewFailureRefresh(
  refresh: (event: PhotoOwnerEvent) => void,
  limit = 64,
): Readonly<{ claim(event: PhotoOwnerEvent): boolean }> {
  const attempted = new Set<string>();
  const claim = (event: PhotoOwnerEvent): boolean => {
    if (attempted.has(event.url)) return false;
    attempted.add(event.url);
    if (attempted.size > limit) {
      const oldest = attempted.values().next().value;
      if (oldest !== undefined) attempted.delete(oldest);
    }
    refresh(event);
    return true;
  };
  return Object.freeze({ claim });
}

export function createPhotoPreviewFlow(
  dependencies: PhotoPreviewFlowDependencies,
): PhotoPreviewFlow {
  const { view, photoOwner, sourceGrid } = dependencies;

  const prefetchAdjacent = async (
    index: number,
    authority: PhotoAuthority,
  ): Promise<void> => {
    if (!photoOwner.isCurrent(authority)) return;
    if (index < 0 || index >= sourceGrid.total) return;
    let photo = sourceGrid.photoAt(index);
    if (!photo) {
      const windowAuthority = photoOwner.windowAuthority;
      if (!windowAuthority) return;
      await dependencies.loadPhotoWindow(index, windowAuthority, authority);
      if (!photoOwner.isCurrent(authority)) return;
      // A window an awaited caller loaded settles through that caller, so the
      // strip asks for its own render here instead of waiting for a
      // window-settled notification awaited windows never raise: the neighbor
      // it just loaded stops being a placeholder now.
      dependencies.renderFilmstrip();
      photo = sourceGrid.photoAt(index);
    }
    if (!photo || !photo.available) return;
    await photoOwner.prefetchAdjacent(authority, index);
  };

  const showPreview = async (
    authority: PhotoAuthority,
    transition?: RecoveryTransition,
    refresh = false,
  ): Promise<boolean> => {
    const capturedStatus = view.photoStatusSurface;
    const outcome = await photoOwner.loadCurrentPreview(authority, refresh);
    if (outcome.kind === "detached") return false;
    if (outcome.kind === "failed") {
      if (view.isPhotoStatusSurfaceCurrent(capturedStatus))
        view.setPhotoStatus("Connection lost. Retry to refresh this Photo.");
      dependencies.failPhotoRecovery(authority, "preview", transition);
      return false;
    }
    if (!photoOwner.isCurrent(authority)) return false;
    const photoId = dependencies.currentPhotoId();
    if (photoId) dependencies.editorReferenceChanged(photoId);
    if (outcome.kind === "not-ready") {
      if (view.isPhotoStatusSurfaceCurrent(capturedStatus))
        view.setPhotoStatus(outcome.preview.message ?? "Preview unavailable");
      view.showPreviewUnavailable("Preview unavailable");
      return true;
    }
    const result = outcome.preview;
    if (!view.reviewImageMatches(result.url))
      dependencies.renderReviewImage(result.url, authority);
    view.setPreviewFacts(result.source, Boolean(result.limitedDetail));
    if (view.isPhotoStatusSurfaceCurrent(capturedStatus))
      view.setPhotoStatus(
        result.stale ? (result.message ?? "Showing a stale Preview.") : "",
      );
    dependencies.updateControls();
    void prefetchAdjacent(photoOwner.currentIndex - 1, authority);
    void prefetchAdjacent(photoOwner.currentIndex + 1, authority);
    return true;
  };

  return Object.freeze({ showPreview, prefetchAdjacent });
}
