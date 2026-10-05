import { fetchEditPreview } from "../api/processing-preview.js";
import { isRecord } from "../api/guards.js";
import {
  composablePreviewDigestRefusal,
  composablePreviewIdentityRefusal,
  composablePreviewPngRefusal,
  composablePreviewReadRefusal,
  type ComposableRecipeRead,
} from "./composable-preview.js";
import type { BrowserFetch } from "./access-session.js";
import { composablePreviewTarget } from "./composable-preview.js";
export function createEditorComposablePreview(
  fetcher: BrowserFetch,
  dependencies: Readonly<{
    cameraReference: () => boolean;
    comparison?: "baseline";
    read: () => ComposableRecipeRead | undefined;
    isDirty: () => boolean;
    editorOwnsPhoto: (photoId: string) => boolean;
    renderEditor: () => void;
    present: (url: string) => void;
    clearPresented: () => void;
    describePreviewRefusal: (response: Response) => Promise<string>;
  }>,
) {
  const {
    cameraReference,
    read,
    isDirty,
    editorOwnsPhoto,
    renderEditor,
    present,
    clearPresented,
    describePreviewRefusal,
  } = dependencies;
  const comparison = dependencies.comparison;
  let comparisonIdentity: string | undefined;
  let editorPreviewUrl: string | undefined;
  let editorPreviewNote = "";
  let editorPreviewOutcome: "pending" | "ready" | "failed" | "unknown" =
    "pending";
  let editorPreviewStale = false;
  let editorPreviewBusy = false;
  let editorPreviewAbort: AbortController | undefined;
  let editorPreviewGeneration = 0;
  /// In-flight selected-step identity. Equal complete identities coalesce.
  let editorPreviewIdentity: string | undefined;
  /// True when the last preview outcome was a refusal and no rendition is
  /// presented, so the Edit Preview axis reports a failure rather than a
  /// wait.
  let editorPreviewRefused = false;
  /// How many follow-up requests one admitted preview has already made.
  let editorPreviewAttempts = 0;
  let editorPreviewTimer: number | undefined;
  let editorPreviewSelection: string | null | undefined;
  const PREVIEW_POLL_MS = 750;
  const PREVIEW_POLL_LIMIT = 400;
  const clearEditorPreview = (): void => {
    editorPreviewAbort?.abort();
    editorPreviewAbort = undefined;
    editorPreviewGeneration += 1;
    editorPreviewBusy = false;
    editorPreviewIdentity = undefined;
    comparisonIdentity = undefined;
    editorPreviewSelection = undefined;
    editorPreviewAttempts = 0;
    if (editorPreviewTimer !== undefined) {
      clearTimeout(editorPreviewTimer);
      editorPreviewTimer = undefined;
    }
    if (editorPreviewUrl) URL.revokeObjectURL(editorPreviewUrl);
    editorPreviewUrl = undefined;
    editorPreviewNote = "";
    editorPreviewOutcome = "pending";
    editorPreviewStale = false;
    editorPreviewRefused = false;

    clearPresented();
  };
  /// Poll admitted Edit Previews within a bounded wait window.
  const scheduleEditorPreviewFollowUp = (photoId: string): void => {
    if (editorPreviewAttempts >= PREVIEW_POLL_LIMIT) {
      editorPreviewNote =
        "The preview is taking too long. Refresh the preview to check its result.";
      editorPreviewOutcome = "unknown";
      renderEditor();
      return;
    }
    editorPreviewAttempts += 1;
    clearTimeout(editorPreviewTimer);
    editorPreviewTimer = window.setTimeout(() => {
      editorPreviewTimer = undefined;
      void requestEditorPreview(photoId, true);
    }, PREVIEW_POLL_MS);
  };

  /// One preview request follows each completed edit action and each settled
  /// save. A retained image stays presented and is marked out of date until
  /// the matching rendition arrives. A different source, step, or settings
  /// snapshot cannot replace the current view.
  const requestEditorPreview = async (
    photoId: string,
    followUp = false,
  ): Promise<void> => {
    if (cameraReference() || !editorOwnsPhoto(photoId)) return;

    // Capture the selected recipe before requesting its bounded rendition.
    const composableRead = read();
    const target = composablePreviewTarget(composableRead);
    const dirty = isDirty();
    const refuseComposable = (note: string): void => {
      if (editorPreviewUrl) {
        editorPreviewStale = true;
      } else {
        editorPreviewRefused = true;
        clearPresented();
      }
      editorPreviewOutcome = "failed";
      editorPreviewNote = note;
      editorPreviewSelection = null;
      renderEditor();
    };
    if (target.kind === "unreadable") {
      refuseComposable(
        "The Edit State could not be read, so no Edit Preview is shown. Reload to check again.",
      );
      return;
    }
    if (target.kind === "none") {
      // An empty recipe renders no processing result: the earlier selection's
      // rendition is dropped rather than kept as the empty recipe's result.
      refuseComposable(
        "The Edit State selects no Processing Engine, so no processing result is shown.",
      );
      return;
    }
    if (target.kind === "step" && dirty) {
      // The route previews the saved recipe, so an unsaved draft never
      // presents another caller's intent as this one's result.
      if (editorPreviewUrl) editorPreviewStale = true;
      editorPreviewNote =
        "Waiting for the matching settings to be saved before updating the Preview.";
      editorPreviewOutcome = editorPreviewUrl ? "ready" : "pending";
      renderEditor();
      return;
    }
    const composableStep = target.kind === "step" ? target.step : null;
    if (!composableStep) return;
    const identity = JSON.stringify([
      photoId,
      composableStep,
      composableRead?.recipe?.revision ?? "",
      composableRead?.sourceRevision ?? "",
      comparison ?? "current",
    ]);
    // A retry of the identity already in flight joins that request: the
    // in-flight attempt settles for this owner, and no duplicate physical
    // render is admitted behind it.
    if (editorPreviewBusy && editorPreviewIdentity === identity) return;
    if (!followUp) editorPreviewAttempts = 0;
    clearTimeout(editorPreviewTimer);
    editorPreviewTimer = undefined;
    const generation = ++editorPreviewGeneration;
    editorPreviewAbort?.abort();
    const controller = new AbortController();
    editorPreviewAbort = controller;
    editorPreviewBusy = true;
    editorPreviewIdentity = identity;
    editorPreviewSelection = composableStep?.stepId ?? null;
    editorPreviewRefused = false;
    editorPreviewOutcome = "pending";
    if (editorPreviewUrl) {
      editorPreviewStale = true;
      editorPreviewNote = composableStep
        ? "This Edit Preview is older than the current Edit State."
        : "This preview is older than the current settings.";
    } else {
      editorPreviewNote = composableStep
        ? "Requesting an Edit Preview of the current Edit State…"
        : "Updating preview…";
    }
    renderEditor();
    let response: Response;
    try {
      response = await fetchEditPreview(
        fetcher,
        photoId,
        controller.signal,
        comparison,
      );
    } catch {
      if (generation === editorPreviewGeneration) {
        editorPreviewBusy = false;
        editorPreviewNote = "The preview request did not reach the service.";
        editorPreviewRefused = !editorPreviewUrl;
        editorPreviewOutcome = "failed";
        renderEditor();
      }
      return;
    }
    if (generation !== editorPreviewGeneration || !editorOwnsPhoto(photoId))
      return;
    editorPreviewBusy = false;
    if (response.status === 202) {
      const body: unknown = await response.json().catch(() => undefined);
      if (generation !== editorPreviewGeneration || !editorOwnsPhoto(photoId))
        return;
      const state =
        isRecord(body) && typeof body["state"] === "string"
          ? body["state"]
          : "queued";
      // Queued work is waiting for admission, not computing: the note names
      // the capacity wait, and a retained image stays presented as older than
      // the current settings instead of being replaced or hidden.
      editorPreviewNote =
        state === "running"
          ? "Rendering the preview. The image shown is older than the current settings."
          : "Updating preview…";
      if (editorPreviewUrl) editorPreviewStale = true;
      renderEditor();
      scheduleEditorPreviewFollowUp(photoId);
      return;
    }
    if (!response.ok) {
      editorPreviewRefused = !editorPreviewUrl;
      const note = await describePreviewRefusal(response);
      if (generation !== editorPreviewGeneration || !editorOwnsPhoto(photoId))
        return;
      editorPreviewNote = note;
      editorPreviewOutcome = "failed";
      renderEditor();
      return;
    }
    let image: Blob;
    try {
      image = await response.blob();
    } catch {
      if (generation !== editorPreviewGeneration || !editorOwnsPhoto(photoId))
        return;
      editorPreviewRefused = !editorPreviewUrl;
      editorPreviewNote =
        "The preview could not be read. Refresh the preview to try again.";
      editorPreviewOutcome = "failed";
      renderEditor();
      return;
    }
    if (generation !== editorPreviewGeneration || !editorOwnsPhoto(photoId))
      return;
    if (composableStep) {
      // The served rendition must be the selected step's own result. The
      // route's complete identity framing must name this Photo, this step,
      // the exact recipe and source revisions the request captured, the
      // served geometry, the Preview bound, and the bundle; the body must be
      // an image whose own PNG frame matches the declared geometry and whose
      // bytes hash to the published digest. Missing framing, a foreign
      // identity, or unverifiable bytes are refused rather than presented.
      const bytes = await image.arrayBuffer().catch(() => undefined);
      if (generation !== editorPreviewGeneration || !editorOwnsPhoto(photoId))
        return;
      const unread = bytes
        ? composablePreviewReadRefusal(
            response.headers.get("content-type"),
            image.size,
          ) ||
          composablePreviewIdentityRefusal(response.headers, {
            photoId,
            stepId: composableStep.stepId,
            recipeRevision: composableRead?.recipe?.revision ?? "",
            sourceRevision: composableRead?.sourceRevision ?? "",
            comparison: comparison ?? "current",
          }) ||
          composablePreviewPngRefusal(
            new Uint8Array(bytes),
            response.headers.get("slipstream-processing-preview-width"),
            response.headers.get("slipstream-processing-preview-height"),
          ) ||
          (await composablePreviewDigestRefusal(
            response.headers.get("slipstream-processing-preview-sha256"),
            bytes,
          ))
        : "The Edit Preview could not be read. Refresh the preview to try again.";
      if (generation !== editorPreviewGeneration || !editorOwnsPhoto(photoId))
        return;
      if (unread) {
        editorPreviewRefused = !editorPreviewUrl;
        editorPreviewNote = unread;
        editorPreviewOutcome = "failed";
        renderEditor();
        return;
      }
    }
    if (editorPreviewUrl) URL.revokeObjectURL(editorPreviewUrl);
    editorPreviewUrl = URL.createObjectURL(image);
    comparisonIdentity = JSON.stringify([
      photoId,
      composableStep.stepId,
      composableRead?.recipe?.revision,
      composableRead?.sourceRevision,
      response.headers.get("slipstream-processing-preview-width"),
      response.headers.get("slipstream-processing-preview-height"),
      response.headers.get("slipstream-processing-preview-bundle-id"),
      response.headers.get("slipstream-processing-preview-display-conversion"),
      response.headers.get("slipstream-processing-preview-geometry"),
      response.headers.get("slipstream-processing-preview-module"),
      response.headers.get(
        "slipstream-processing-preview-adapter-schema-version",
      ),
      response.headers.get("slipstream-processing-preview-input-sha256"),
      response.headers.get("slipstream-processing-preview-input-byte-length"),
    ]);
    editorPreviewStale = false;
    editorPreviewRefused = false;
    editorPreviewOutcome = "ready";
    if (composableStep) {
      editorPreviewNote = `Edit Preview from ${composableStep.module}, ${response.headers.get("slipstream-processing-preview-width")} × ${response.headers.get("slipstream-processing-preview-height")} pixels. This bounded rendition is for color and tone; inspect a full-resolution artifact for grain and halation detail.`;
    }
    present(editorPreviewUrl);
    renderEditor();
  };
  return {
    get current() {
      return {
        url: editorPreviewUrl,
        note: editorPreviewNote,
        outcome: editorPreviewOutcome,
        stale: editorPreviewStale,
        busy: editorPreviewBusy,
        pending: editorPreviewTimer !== undefined,
        refused: editorPreviewRefused,
        comparisonIdentity,
      };
    },
    get selection() {
      return editorPreviewSelection;
    },
    requestCurrent: requestEditorPreview,
    clear: clearEditorPreview,
    markStale: (): void => {
      editorPreviewGeneration += 1;
      editorPreviewAbort?.abort();
      editorPreviewAbort = undefined;
      editorPreviewBusy = false;
      editorPreviewIdentity = undefined;
      clearTimeout(editorPreviewTimer);
      editorPreviewTimer = undefined;
      if (editorPreviewUrl) editorPreviewStale = true;
    },
  };
}
