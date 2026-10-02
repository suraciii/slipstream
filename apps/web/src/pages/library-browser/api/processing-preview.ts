import type { BrowserFetch } from "../model/access-session.js";

export const processingPreviewUri = (
  photoId: string,
  stepId: string,
  comparison?: "baseline",
): string =>
  `/api/photos/${encodeURIComponent(photoId)}/processing-preview/${encodeURIComponent(stepId)}${comparison === "baseline" ? "?comparison=baseline" : ""}`;

/** Fetches only the recipe's caller-selected current Processing Step Preview. */
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
