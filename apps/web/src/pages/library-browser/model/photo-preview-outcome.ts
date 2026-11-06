import type {
  PhotoSummary,
  PreviewResponse,
  PreviewSource,
} from "../api/contracts.js";
import type {
  PhotoWindowAuthority,
  SourceAuthority,
} from "./source-grid-owner.js";

declare const photoAuthorityBrand: unique symbol;
export type PhotoAuthority = Readonly<{ [photoAuthorityBrand]: true }>;

export type PhotoOwnerEvent = Readonly<{
  kind: "review-image-failed";
  authority: PhotoAuthority;
  photoId: string;
  url: string;
  surface: object;
  photoSurface: object;
}>;

export type PhotoOperation = Readonly<{
  authority: PhotoAuthority;
  sourceAuthority: SourceAuthority;
  windowAuthority: PhotoWindowAuthority;
  index: number;
  photoId?: string;
}>;

export type PhotoPreviewOutcome =
  | (PhotoOperation &
      Readonly<{
        kind: "ready";
        preview: PreviewResponse & { state: "ready"; url: string };
      }>)
  | (PhotoOperation &
      Readonly<{
        kind: "not-ready";
        preview: PreviewResponse & { state: "unavailable" | "failed" };
      }>)
  | (PhotoOperation &
      Readonly<{
        kind: "failed";
        failure: "answered" | "malformed" | "transport";
        status?: number;
      }>)
  | (PhotoOperation & Readonly<{ kind: "detached" }>);

export type CompleteReadyPreview = Readonly<{
  source: PreviewSource;
  width: number;
  height: number;
  limitedDetail?: boolean;
  url: string;
}>;

export const detachedPreview = (
  operation: PhotoOperation,
): PhotoPreviewOutcome => Object.freeze({ ...operation, kind: "detached" });

/// Returns the facts a ready browse-window summary can present without another
/// Preview request. A summary missing any fact falls back to the endpoint.
export function completeReadyPreview(
  photo: PhotoSummary,
): CompleteReadyPreview | undefined {
  const preview = photo.preview;
  if (
    !photo.available ||
    preview.state !== "ready" ||
    !preview.url ||
    !preview.source ||
    preview.width === undefined ||
    preview.height === undefined
  )
    return undefined;
  return Object.freeze({
    source: preview.source,
    width: preview.width,
    height: preview.height,
    ...(preview.limitedDetail !== undefined
      ? { limitedDetail: preview.limitedDetail }
      : {}),
    url: preview.url,
  });
}
