import type { BrowserFetch } from "../model/access-session.js";

export const processingPreviewUri = (
  photoId: string,
  stepId: string,
  comparison?: "baseline",
): string =>
  `/api/photos/${encodeURIComponent(photoId)}/processing-preview/${encodeURIComponent(stepId)}${comparison === "baseline" ? "?comparison=baseline" : ""}`;

/** Fetches a named Processing Step Preview for the compatibility surface. */
export const fetchProcessingPreview = (
  fetcher: BrowserFetch,
  photoId: string,
  stepId: string,
  signal: AbortSignal,
  comparison?: "baseline",
): Promise<Response> =>
  fetcher(processingPreviewUri(photoId, stepId, comparison), {
    signal,
    priority: "high",
  });

export const editPreviewUri = (
  photoId: string,
  comparison?: "baseline",
): string =>
  `/api/photos/${encodeURIComponent(photoId)}/edit/preview${comparison === "baseline" ? "?comparison=baseline" : ""}`;

/** Fetches the Preview of the Photo's confirmed current Edit State. */
export const fetchEditPreview = (
  fetcher: BrowserFetch,
  photoId: string,
  signal: AbortSignal,
  comparison?: "baseline",
): Promise<Response> =>
  fetcher(editPreviewUri(photoId, comparison), {
    signal,
    priority: "high",
  });
