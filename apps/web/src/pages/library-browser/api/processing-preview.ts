import type { BrowserFetch } from "../model/access-session.js";

export const processingPreviewUri = (photoId: string, stepId: string): string =>
  `/api/photos/${encodeURIComponent(photoId)}/processing-preview/${encodeURIComponent(stepId)}`;

/** Fetches only the recipe's caller-selected current Processing Step Preview. */
export const fetchProcessingPreview = (
  fetcher: BrowserFetch,
  photoId: string,
  stepId: string,
  signal: AbortSignal,
): Promise<Response> =>
  fetcher(processingPreviewUri(photoId, stepId), {
    signal,
    priority: "high",
  });
