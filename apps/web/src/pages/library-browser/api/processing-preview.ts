import type { BrowserFetch } from "../model/access-session.js";

export const processingPreviewUri = (
  photoId: string,
  _stepId: string,
  comparison?: "baseline",
): string =>
  `/api/photos/${encodeURIComponent(photoId)}/edit/preview${comparison === "baseline" ? "?comparison=baseline" : ""}`;

/** Fetches the current Edit State Preview. The step id remains a local
 * identity guard for the returned rendition, not a route selector. */
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
