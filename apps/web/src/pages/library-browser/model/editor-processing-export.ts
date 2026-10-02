import { isRecord } from "../api/editor.js";
import {
  describeProcessingArtifact,
  parseProcessingArtifactRecord,
  processingArtifactDigestRefusal,
  processingArtifactMatchesHeaders,
  type ProcessingArtifactRecord,
} from "./processing-artifact.js";
import { formatByteCount } from "./editor-presentation.js";
import type { BrowserFetch } from "./access-session.js";
import type {
  LibraryBrowserView,
  EditorExportViewModel,
} from "../ui/library-browser-view.js";
import type { createEditorComposableRecipe } from "./editor-composable-recipe.js";
import { randomUuid } from "./browser-crypto.js";
export function createEditorProcessingExport(
  fetcher: BrowserFetch,
  recipe: ReturnType<typeof createEditorComposableRecipe>,
  dependencies: Readonly<{
    editorOwnsPhoto: (photoId: string) => boolean;
    renderEditor: () => void;
    processingAvailable: () => boolean;
    describeEditRefusal: (
      response: Response,
      subject: string,
    ) => Promise<string>;
  }>,
) {
  const {
    editorOwnsPhoto,
    renderEditor,
    describeEditRefusal,
    processingAvailable,
  } = dependencies;
  let editorScopeGeneration = 0;
  const ownsEditorScope = (photoId: string, generation: number): boolean =>
    generation === editorScopeGeneration && editorOwnsPhoto(photoId);
  let editorExportState: EditorExportViewModel["state"] = "idle";
  let editorExportNote = "";
  let editorExportTimer: number | undefined;
  let editorProcessingRequestId: string | undefined;
  let editorProcessingArtifact: ProcessingArtifactRecord | null = null;
  const submitEditorExport = async (photoId: string): Promise<void> => {
    if (
      !editorOwnsPhoto(photoId) ||
      editorExportState === "submitting" ||
      editorExportState === "running" ||
      editorExportState === "outcome-unknown"
    )
      return;
    const generation = editorScopeGeneration;
    const composableRead = recipe.read;
    const previewTarget = recipe.target();
    const composableStep =
      previewTarget.kind === "step" ? previewTarget.step : null;
    // The composable Export is bounded to the saved recipe's selected
    // current step: an unreadable read, an empty recipe, or a dirty draft
    // is refused rather than exported against another caller's intent.
    const savedRecipe = composableRead?.recipe;
    if (!composableStep || !savedRecipe || !composableRead) {
      editorExportState = "idle";
      editorExportNote =
        previewTarget.kind === "unreadable"
          ? "The Processing Recipe could not be read, so the Export waits. Reload to check again."
          : previewTarget.kind === "none"
            ? "The Processing Recipe selects no Processing Step, so there is nothing to export."
            : "Save the Processing Recipe before exporting the selected Processing Step.";
      renderEditor();
      return;
    }
    if (recipe.dirty()) {
      editorExportState = "idle";
      editorExportNote =
        "Save the Processing Recipe before exporting the selected Processing Step.";
      renderEditor();
      return;
    }
    const body = {
      requestId: `web-processing-export-${randomUuid().replaceAll("-", "").slice(0, 24)}`,
      stepId: composableStep.stepId,
      expectedRecipeRevision: savedRecipe.revision,
      expectedSourceRevision: composableRead.sourceRevision,
    };
    editorExportState = "submitting";
    editorExportNote = `Exporting the selected Processing Step ${composableStep.stepId}…`;
    editorProcessingRequestId = body.requestId;
    editorProcessingArtifact = null;
    renderEditor();
    let response: Response;
    try {
      response = await fetcher(
        `/api/photos/${encodeURIComponent(photoId)}/processing-exports`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      );
    } catch {
      if (ownsEditorScope(photoId, generation)) {
        editorExportState = "outcome-unknown";
        editorExportNote =
          "The Processing Export outcome is uncertain. Check its result before exporting again.";
        renderEditor();
      }
      return;
    }
    if (!ownsEditorScope(photoId, generation)) return;
    const accepted: unknown = await response.json().catch(() => undefined);
    if (!ownsEditorScope(photoId, generation)) return;
    if (response.status === 202) {
      const receipt =
        isRecord(accepted) && isRecord(accepted["receipt"])
          ? accepted["receipt"]
          : undefined;
      if (
        !receipt ||
        receipt["photoId"] !== photoId ||
        receipt["requestId"] !== body.requestId ||
        receipt["stepId"] !== body.stepId ||
        !["accepted", "executing"].includes(String(receipt["state"]))
      ) {
        editorExportState = "outcome-unknown";
        editorExportNote =
          "The Processing Export response could not be confirmed.";
      } else {
        editorExportState = "running";
        editorExportNote =
          "The selected Processing Step Export is running. Check its result to reconcile it.";
      }
      renderEditor();
      scheduleProcessingExportPoll(photoId);
      return;
    }
    if (response.status !== 200 && response.status !== 201) {
      const error =
        isRecord(accepted) && isRecord(accepted["error"])
          ? accepted["error"]
          : undefined;
      const code =
        error && typeof error["code"] === "string" ? error["code"] : "";
      editorExportState =
        response.status >= 500 || code === "outcome_unknown"
          ? "outcome-unknown"
          : "failed";
      editorExportNote =
        code === "module_parameters_unavailable"
          ? "The selected Processing Step has no qualified Export adapter in this deployment."
          : code === "source_changed"
            ? "The selected Processing Step belongs to an earlier source revision. Reload and try again."
            : code === "recipe_conflict"
              ? "The selected Processing Recipe changed. Reload and try again."
              : code === "export_terminal"
                ? "The Processing Export already reached a terminal decision. Check its result."
                : "The selected Processing Step Export was refused.";
      renderEditor();
      return;
    }
    const artifactRecord = parseProcessingArtifactRecord(
      isRecord(accepted) ? accepted["artifact"] : undefined,
    );
    if (
      !artifactRecord ||
      artifactRecord.photoId !== photoId ||
      artifactRecord.stepId !== body.stepId
    ) {
      editorExportState = "outcome-unknown";
      editorExportNote =
        "The Processing Export response could not be confirmed.";
      renderEditor();
      return;
    }
    editorExportState = "succeeded";
    editorProcessingArtifact = artifactRecord;
    recipe.retainArtifact(artifactRecord);
    editorExportNote = `${describeProcessingArtifact(artifactRecord, formatByteCount)} It remains available below as a downstream Processing Step input.`;
    renderEditor();
    return;
  };
  const scheduleProcessingExportPoll = (photoId: string): void => {
    clearTimeout(editorExportTimer);
    if (editorExportState !== "queued" && editorExportState !== "running")
      return;
    editorExportTimer = window.setTimeout(() => {
      editorExportTimer = undefined;
      if (!editorOwnsPhoto(photoId)) return;
      void checkProcessingExport(photoId);
    }, 1500);
  };
  /// Reads one live composable Export's durable work record and reconciles
  /// the workspace's state with the committed lifecycle it reports. A
  /// succeeded record's artifact is inspected through its own provenance
  /// record before it is offered anywhere.
  const checkProcessingExport = async (photoId: string): Promise<void> => {
    const generation = editorScopeGeneration;
    const requestId = editorProcessingRequestId;
    if (!editorOwnsPhoto(photoId) || !requestId) return;
    let response: Response;
    try {
      response = await fetcher(
        `/api/photos/${encodeURIComponent(photoId)}/processing-exports/${encodeURIComponent(requestId)}`,
        { priority: "low" },
      );
    } catch {
      return;
    }
    if (
      response.status !== 200 ||
      !ownsEditorScope(photoId, generation) ||
      editorProcessingRequestId !== requestId
    )
      return;
    const work: unknown = await response.json().catch(() => undefined);
    if (
      !ownsEditorScope(photoId, generation) ||
      editorProcessingRequestId !== requestId
    )
      return;
    const state =
      isRecord(work) && typeof work["state"] === "string" ? work["state"] : "";
    const workRequestId =
      isRecord(work) && typeof work["requestId"] === "string"
        ? work["requestId"]
        : "";
    const workPhotoId =
      isRecord(work) && typeof work["photoId"] === "string"
        ? work["photoId"]
        : "";
    if (
      workRequestId !== requestId ||
      workPhotoId !== photoId ||
      !["accepted", "executing", "succeeded", "failed", "cancelled"].includes(
        state,
      )
    ) {
      editorExportState = "outcome-unknown";
      editorExportNote =
        "The Processing Export result could not be confirmed. Check its result again.";
      renderEditor();
      return;
    }
    if (state === "accepted" || state === "executing") {
      editorExportState = "running";
      editorExportNote = `The selected Processing Step Export is ${state}.`;
      renderEditor();
      scheduleProcessingExportPoll(photoId);
      return;
    }
    if (state === "succeeded") {
      const artifactId =
        isRecord(work) && typeof work["artifactId"] === "string"
          ? work["artifactId"]
          : "";
      if (!artifactId) {
        editorExportState = "outcome-unknown";
        editorExportNote =
          "The Processing Export result could not be confirmed. Check its result again.";
        renderEditor();
        return;
      }
      await recipe.fetchEditorArtifact(photoId, artifactId);
      if (
        !ownsEditorScope(photoId, generation) ||
        editorProcessingRequestId !== requestId
      )
        return;
      editorExportState = "succeeded";
      const artifact = recipe.artifacts.find(
        (item) => item.artifactId === artifactId,
      );
      if (artifact) {
        editorProcessingArtifact = artifact;
        editorExportNote = `${describeProcessingArtifact(artifact, formatByteCount)} It remains available below as a downstream Processing Step input.`;
      }
      renderEditor();
      return;
    }
    editorExportState = state === "cancelled" ? "cancelled" : "failed";
    const failureReason =
      isRecord(work) && typeof work["failureReason"] === "string"
        ? work["failureReason"]
        : "";
    editorExportNote =
      state === "cancelled"
        ? "The Processing Step Export was cancelled."
        : failureReason
          ? `The Processing Step Export failed: ${failureReason}.`
          : "The Processing Step Export failed. Retry.";
    renderEditor();
  };
  /// Cancels the live composable Export this workspace submitted. The
  /// service's committed record is authoritative: an already-terminal
  /// request answers with its own terminal decision.
  const cancelProcessingExport = async (photoId: string): Promise<void> => {
    const generation = editorScopeGeneration;
    const requestId = editorProcessingRequestId;
    if (!editorOwnsPhoto(photoId) || !requestId) return;
    editorExportNote = "Cancelling the Processing Step Export…";
    renderEditor();
    let response: Response;
    try {
      response = await fetcher(
        `/api/photos/${encodeURIComponent(photoId)}/processing-exports/${encodeURIComponent(requestId)}/cancel`,
        { method: "POST" },
      );
    } catch {
      if (ownsEditorScope(photoId, generation)) {
        editorExportNote = "The cancellation did not reach the service.";
        renderEditor();
      }
      return;
    }
    if (!ownsEditorScope(photoId, generation)) return;
    if (response.status !== 200) {
      const note = await describeEditRefusal(response, "Cancellation");
      if (!ownsEditorScope(photoId, generation)) return;
      editorExportNote = note;
      renderEditor();
      return;
    }
    await checkProcessingExport(photoId);
  };
  /// Cancels whichever Export surface is live: the composable request this
  /// workspace submitted, or the legacy Export record it inspected.
  const downloadEditorArtifact = async (
    photoId: string,
    artifactId: string,
  ): Promise<void> => {
    if (!editorOwnsPhoto(photoId)) return;
    const generation = editorScopeGeneration;
    const artifact = recipe.artifacts.find(
      (item) => item.artifactId === artifactId,
    );
    if (!artifact) return;
    editorExportNote = `Downloading the Processing Artifact ${artifactId}…`;
    renderEditor();
    let response: Response;
    try {
      response = await fetcher(
        `/api/processing-artifacts/${encodeURIComponent(artifactId)}/bytes`,
        { priority: "high" },
      );
    } catch {
      if (ownsEditorScope(photoId, generation)) {
        editorExportNote = "The download did not reach the service.";
        renderEditor();
      }
      return;
    }
    if (!ownsEditorScope(photoId, generation)) return;
    if (response.status !== 200) {
      const note = await describeEditRefusal(response, "The download");
      if (!ownsEditorScope(photoId, generation)) return;
      editorExportNote = note;
      renderEditor();
      return;
    }
    if (!processingArtifactMatchesHeaders(artifact, response.headers)) {
      editorExportNote =
        "The downloaded Processing Artifact did not match its record and was discarded.";
      renderEditor();
      return;
    }
    const image = await response.blob().catch(() => undefined);
    if (!ownsEditorScope(photoId, generation)) return;
    if (!image) {
      editorExportNote = "The downloaded file could not be read.";
      renderEditor();
      return;
    }
    const bytes = await image.arrayBuffer().catch(() => undefined);
    if (!ownsEditorScope(photoId, generation)) return;
    const digestRefusal = bytes
      ? await processingArtifactDigestRefusal(artifact, bytes)
      : "The downloaded file could not be read.";
    if (!ownsEditorScope(photoId, generation)) return;
    if (digestRefusal) {
      editorExportNote = digestRefusal;
      renderEditor();
      return;
    }
    const url = URL.createObjectURL(image);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = `slipstream-artifact-${artifactId}.tiff`;
    anchor.click();
    URL.revokeObjectURL(url);
    if (!ownsEditorScope(photoId, generation)) return;
    editorExportNote = `Downloaded the Processing Artifact ${artifactId} (${formatByteCount(image.size)}).`;
    renderEditor();
  };
  const reset = (): void => {
    editorScopeGeneration += 1;
    clearTimeout(editorExportTimer);
    editorExportTimer = undefined;
    editorProcessingRequestId = undefined;
    editorProcessingArtifact = null;
    editorExportState = "idle";
    editorExportNote = "";
  };
  return {
    reset,
    submit: submitEditorExport,
    cancel: cancelProcessingExport,
    check: checkProcessingExport,
    downloadArtifact: downloadEditorArtifact,
    retry: async (photoId: string): Promise<void> => {
      if (editorExportState === "outcome-unknown")
        await checkProcessingExport(photoId);
      else await submitEditorExport(photoId);
    },
    download: async (photoId: string): Promise<void> => {
      if (editorProcessingArtifact)
        await downloadEditorArtifact(
          photoId,
          editorProcessingArtifact.artifactId,
        );
    },
    view: (): NonNullable<
      Parameters<LibraryBrowserView["renderEditor"]>[0]["export"]
    > => ({
      target: "development-tiff",
      retainedTarget: null,
      state: editorExportState,
      note: editorExportNote,
      artifact: null,
      canSubmit:
        processingAvailable() &&
        recipe.target().kind === "step" &&
        !recipe.dirty() &&
        !["submitting", "outcome-unknown", "queued", "running"].includes(
          editorExportState,
        ),
      canCancel:
        editorExportState === "queued" || editorExportState === "running",
      canRetry: ["outcome-unknown", "failed", "cancelled"].includes(
        editorExportState,
      ),
      canDownload: editorProcessingArtifact !== null,
      processingRequestId: editorProcessingRequestId ?? null,
      processingArtifact: editorProcessingArtifact,
    }),
  };
}
