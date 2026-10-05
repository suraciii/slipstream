import { isRecord } from "../api/guards.js";
import {
  describeProcessingArtifact,
  parseProcessingArtifactRecord,
  processingArtifactDigestRefusal,
  processingArtifactMatchesHeaders,
  type ProcessingArtifactRecord,
} from "./processing-artifact.js";
import {
  parseProcessingExportWork,
  type ProcessingExportWork,
} from "./processing-export.js";
import { formatByteCount } from "./editor-presentation.js";
import type { BrowserFetch } from "./access-session.js";
import type {
  ComposableRecipeRead,
  ComposablePreviewTarget,
} from "./composable-preview.js";
import { blobSha256Hex, randomUuid } from "./browser-crypto.js";
import {
  historicalArtifactMatchesHeaders,
  parseHistoricalExport,
  type HistoricalExport,
} from "./historical-processing-export.js";

type ExportState =
  | "idle"
  | "submitting"
  | "outcome-unknown"
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled";
type PendingExport = Readonly<{
  requestId: string;
  stepId: string;
  path: string;
  body: string;
}>;
type PhotoExports = {
  state: ExportState;
  note: string;
  requestId: string | null;
  artifact: ProcessingArtifactRecord | null;
  exports: ProcessingExportWork[];
  artifacts: ProcessingArtifactRecord[];
  pending: PendingExport | null;
  historicalExports: HistoricalExport[];
};

export interface ProcessingExportRecipe {
  readonly read: ComposableRecipeRead | undefined;
  readonly artifacts: ReadonlyArray<ProcessingArtifactRecord>;
  target(): ComposablePreviewTarget;
  dirty(): boolean;
  retainArtifact(artifact: ProcessingArtifactRecord): void;
  fetchEditorArtifact(photoId: string, artifactId: string): Promise<void>;
}

export function createEditorProcessingExport(
  fetcher: BrowserFetch,
  recipe: ProcessingExportRecipe,
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
    processingAvailable,
    describeEditRefusal,
  } = dependencies;
  const photos = new Map<string, PhotoExports>();
  let photo: string | null = null;
  let generation = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const empty = (): PhotoExports => ({
    state: "idle",
    note: "",
    requestId: null,
    artifact: null,
    exports: [],
    artifacts: [],
    pending: null,
    historicalExports: [],
  });
  const slot = (photoId: string): PhotoExports => {
    let value = photos.get(photoId);
    if (!value) {
      value = empty();
      photos.set(photoId, value);
    }
    return value;
  };
  const owns = (photoId: string, stamp: number): boolean =>
    generation === stamp && editorOwnsPhoto(photoId);
  const publish = (photoId: string, stamp: number): void => {
    if (!owns(photoId, stamp)) return;
    photo = photoId;
    renderEditor();
  };
  const retain = (
    photoId: string,
    artifact: ProcessingArtifactRecord,
    stamp: number,
  ): void => {
    const value = slot(photoId);
    value.artifacts = [
      artifact,
      ...value.artifacts.filter(
        (item) => item.artifactId !== artifact.artifactId,
      ),
    ].slice(0, 64);
    if (owns(photoId, stamp)) recipe.retainArtifact(artifact);
  };
  const reconcile = (
    photoId: string,
    work: ProcessingExportWork,
    stamp: number,
  ): void => {
    const value = slot(photoId);
    value.exports = [
      work,
      ...value.exports.filter((item) => item.requestId !== work.requestId),
    ].slice(0, 64);
    if (value.pending?.requestId === work.requestId) value.pending = null;
    if (value.requestId !== work.requestId) return;
    value.state =
      work.state === "accepted" || work.state === "executing"
        ? "running"
        : work.state;
    value.artifact = work.artifactId
      ? (value.artifacts.find((item) => item.artifactId === work.artifactId) ??
        null)
      : null;
    value.note = value.artifact
      ? describeProcessingArtifact(value.artifact, formatByteCount)
      : work.state === "failed"
        ? `The Edit State Export failed${work.failureReason ? `: ${work.failureReason}` : ""}.`
        : work.state === "cancelled"
          ? "The Edit State Export was cancelled."
          : `The Edit State Export is ${work.state}.`;
    publish(photoId, stamp);
  };
  const schedule = (photoId: string): void => {
    if (!editorOwnsPhoto(photoId)) return;
    clearTimeout(timer);
    const value = slot(photoId);
    if (
      !editorOwnsPhoto(photoId) ||
      (!value.exports.some(
        (item) => item.state === "accepted" || item.state === "executing",
      ) &&
        value.state !== "running")
    )
      return;
    timer = setTimeout(() => {
      timer = undefined;
      if (editorOwnsPhoto(photoId)) void load(photoId);
    }, 1500);
  };
  const load = async (photoId: string): Promise<void> => {
    if (!editorOwnsPhoto(photoId)) return;
    photo = photoId;
    const stamp = generation;
    const value = slot(photoId);
    publish(photoId, stamp);
    try {
      const response = await fetcher(
        `/api/photos/${encodeURIComponent(photoId)}/processing-exports`,
        { priority: "low" },
      );
      const body: unknown = await response.json().catch(() => undefined);
      if (!owns(photoId, stamp)) return;
      if (
        response.status !== 200 ||
        !isRecord(body) ||
        body["photoId"] !== photoId ||
        !Array.isArray(body["exports"]) ||
        !Array.isArray(body["artifacts"])
      ) {
        value.note =
          "Retained Processing Exports could not be read. Check again.";
        publish(photoId, stamp);
        return;
      }
      const works = body["exports"].map(parseProcessingExportWork);
      const artifacts = body["artifacts"].map(parseProcessingArtifactRecord);
      const historical = Array.isArray(body["historicalExports"])
        ? body["historicalExports"].map(parseHistoricalExport)
        : [];
      if (
        works.some((item) => !item || item.photoId !== photoId) ||
        artifacts.some((item) => !item || item.photoId !== photoId) ||
        historical.some((item) => !item)
      ) {
        value.note =
          "Retained Processing Exports could not be confirmed. Check again.";
        publish(photoId, stamp);
        return;
      }
      value.exports = works as ProcessingExportWork[];
      value.artifacts = artifacts as ProcessingArtifactRecord[];
      value.historicalExports = historical as HistoricalExport[];
      for (const artifact of value.artifacts) recipe.retainArtifact(artifact);
      if (!value.requestId && value.exports[0])
        value.requestId = value.exports[0].requestId;
      const active = value.exports.find(
        (item) => item.requestId === value.requestId,
      );
      if (active && !value.pending) reconcile(photoId, active, stamp);
      publish(photoId, stamp);
    } catch {
      if (owns(photoId, stamp)) {
        value.note =
          "Retained Processing Exports could not reach the service. Check again.";
        publish(photoId, stamp);
      }
    } finally {
      if (owns(photoId, stamp)) schedule(photoId);
    }
  };
  const check = async (
    photoId: string,
    selectedRequestId?: string,
  ): Promise<void> => {
    if (!editorOwnsPhoto(photoId)) return;
    const value = slot(photoId);
    if (value.pending) {
      if (value.state !== "submitting") await send(photoId, value.pending);
      return;
    }
    const requestId = selectedRequestId ?? value.requestId;
    if (!requestId) {
      await load(photoId);
      return;
    }
    const stamp = generation;
    try {
      const response = await fetcher(
        `/api/photos/${encodeURIComponent(photoId)}/processing-exports/${encodeURIComponent(requestId)}`,
        { priority: "low" },
      );
      const work = parseProcessingExportWork(
        await response.json().catch(() => undefined),
      );
      if (!owns(photoId, stamp)) return;
      if (
        response.status !== 200 ||
        !work ||
        work.photoId !== photoId ||
        work.requestId !== requestId
      ) {
        value.note =
          "The Processing Export result could not be confirmed. Retry the exact request if its outcome is uncertain.";
        publish(photoId, stamp);
        return;
      }
      if (work.artifactId) {
        await recipe.fetchEditorArtifact(photoId, work.artifactId);
        if (!owns(photoId, stamp)) return;
        const artifact = recipe.artifacts.find(
          (item) => item.artifactId === work.artifactId,
        );
        if (artifact) retain(photoId, artifact, stamp);
      }
      reconcile(photoId, work, stamp);
    } catch {
      if (owns(photoId, stamp)) {
        value.note =
          "The Processing Export result could not reach the service. Check again.";
        publish(photoId, stamp);
      }
    } finally {
      if (owns(photoId, stamp)) schedule(photoId);
    }
  };
  const send = async (
    photoId: string,
    pending: PendingExport,
  ): Promise<void> => {
    const stamp = generation;
    const value = slot(photoId);
    value.pending = pending;
    value.requestId = pending.requestId;
    value.state = "submitting";
    value.note = "Submitting the current Edit State for Export…";
    value.artifact = null;
    publish(photoId, stamp);
    try {
      const response = await fetcher(pending.path, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: pending.body,
      });
      const body: unknown = await response.json().catch(() => undefined);
      const receipt = parseProcessingExportWork(
        isRecord(body) ? body["receipt"] : undefined,
      );
      if (
        response.status === 202 &&
        receipt?.photoId === photoId &&
        receipt.requestId === pending.requestId &&
        receipt.stepId === pending.stepId
      ) {
        reconcile(photoId, receipt, stamp);
        schedule(photoId);
        return;
      }
      const artifact = parseProcessingArtifactRecord(
        isRecord(body) ? body["artifact"] : undefined,
      );
      if (
        response.status === 201 &&
        isRecord(body) &&
        body["replayed"] === true &&
        artifact?.photoId === photoId &&
        artifact.stepId === pending.stepId
      ) {
        value.pending = null;
        value.state = "succeeded";
        value.artifact = artifact;
        retain(photoId, artifact, stamp);
        value.note = describeProcessingArtifact(artifact, formatByteCount);
        publish(photoId, stamp);
        return;
      }
      const error =
        isRecord(body) && isRecord(body["error"]) ? body["error"] : undefined;
      const details = error?.["details"];
      const work = parseProcessingExportWork(
        isRecord(details) ? details["receipt"] : undefined,
      );
      if (work?.photoId === photoId && work.requestId === pending.requestId) {
        reconcile(photoId, work, stamp);
        return;
      }
      if (
        (response.status >= 400 &&
          response.status < 500 &&
          error?.["code"] !== "outcome_unknown") ||
        (response.status === 503 &&
          (error?.["code"] === "module_parameters_unavailable" ||
            error?.["code"] === "processing_unavailable" ||
            error?.["code"] === "resource_unavailable"))
      ) {
        value.pending = null;
        value.state = "failed";
        value.note =
          typeof error?.["message"] === "string"
            ? error["message"]
            : "The Edit State Export was refused.";
      } else {
        value.state = "outcome-unknown";
        value.note =
          "The Processing Export outcome is uncertain. Check or retry this exact request.";
      }
      publish(photoId, stamp);
    } catch {
      value.state = "outcome-unknown";
      value.note =
        "The Processing Export outcome is uncertain. Check or retry this exact request.";
      publish(photoId, stamp);
    }
  };
  const submit = async (photoId: string): Promise<void> => {
    if (!editorOwnsPhoto(photoId)) return;
    photo = photoId;
    const value = slot(photoId);
    if (value.pending || value.state === "submitting") return;
    const target = recipe.target();
    const read = recipe.read;
    if (target.kind !== "step" || !read?.recipe || recipe.dirty()) {
      value.note = "Save the Edit State before exporting.";
      renderEditor();
      return;
    }
    const requestId = randomUuid();
    await send(photoId, {
      requestId,
      stepId: target.step.stepId,
      path: `/api/photos/${encodeURIComponent(photoId)}/edit/export`,
      body: JSON.stringify({
        requestId,
        expectedEditRevision: read.recipe.revision,
      }),
    });
  };
  const retry = async (
    photoId: string,
    selectedRequestId?: string,
  ): Promise<void> => {
    if (!editorOwnsPhoto(photoId)) return;
    const value = slot(photoId);
    if (value.state === "submitting") return;
    if (value.pending) {
      await send(photoId, value.pending);
      return;
    }
    const old = value.exports.find(
      (item) => item.requestId === (selectedRequestId ?? value.requestId),
    );
    if (!old || (old.state !== "failed" && old.state !== "cancelled")) return;
    const requestId = randomUuid();
    await send(photoId, {
      requestId,
      stepId: old.stepId,
      path: `/api/photos/${encodeURIComponent(photoId)}/processing-exports/${encodeURIComponent(old.requestId)}/retry`,
      body: JSON.stringify({ requestId }),
    });
  };
  const cancel = async (
    photoId: string,
    selectedRequestId?: string,
  ): Promise<void> => {
    const value = slot(photoId);
    const requestId = selectedRequestId ?? value.requestId;
    if (!editorOwnsPhoto(photoId) || !requestId) return;
    const stamp = generation;
    try {
      const response = await fetcher(
        `/api/photos/${encodeURIComponent(photoId)}/processing-exports/${encodeURIComponent(requestId)}/cancel`,
        { method: "POST" },
      );
      if (!owns(photoId, stamp)) return;
      if (response.status !== 200) {
        value.note = await describeEditRefusal(response, "Cancellation");
        publish(photoId, stamp);
        return;
      }
      await check(photoId, requestId);
    } catch {
      if (owns(photoId, stamp)) {
        value.note =
          "The cancellation did not reach the service. Check its result.";
        publish(photoId, stamp);
      }
    }
  };
  const downloadArtifact = async (
    photoId: string,
    artifactId: string,
  ): Promise<void> => {
    if (!editorOwnsPhoto(photoId)) return;
    const stamp = generation;
    const value = slot(photoId);
    const artifact =
      value.artifacts.find((item) => item.artifactId === artifactId) ??
      recipe.artifacts.find((item) => item.artifactId === artifactId);
    if (!artifact || artifact.photoId !== photoId) return;
    if (Date.parse(artifact.expiresAt) <= Date.now()) {
      value.note = "This Processing Artifact has expired.";
      publish(photoId, stamp);
      return;
    }
    try {
      const response = await fetcher(
        `/api/processing-artifacts/${encodeURIComponent(artifactId)}/bytes`,
        { priority: "high" },
      );
      if (!owns(photoId, stamp)) return;
      if (response.status !== 200) {
        value.note = await describeEditRefusal(response, "The download");
        publish(photoId, stamp);
        return;
      }
      if (!processingArtifactMatchesHeaders(artifact, response.headers)) {
        value.note =
          "The downloaded Processing Artifact did not match its record and was discarded.";
        publish(photoId, stamp);
        return;
      }
      const image = await response.blob();
      const refusal = await processingArtifactDigestRefusal(
        artifact,
        await image.arrayBuffer(),
      );
      if (!owns(photoId, stamp)) return;
      if (refusal) {
        value.note = refusal;
        publish(photoId, stamp);
        return;
      }
      const url = URL.createObjectURL(image);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = artifact.filename;
      anchor.click();
      URL.revokeObjectURL(url);
      value.note = `Downloaded ${artifact.filename} (${formatByteCount(image.size)}).`;
      publish(photoId, stamp);
    } catch {
      if (owns(photoId, stamp)) {
        value.note = "The downloaded file could not be read. Try again.";
        publish(photoId, stamp);
      }
    }
  };
  const downloadHistorical = async (
    photoId: string,
    exportId: string,
  ): Promise<void> => {
    if (!editorOwnsPhoto(photoId)) return;
    const stamp = generation;
    const value = slot(photoId);
    const retained = value.historicalExports.find(
      (item) => item.exportId === exportId,
    );
    const artifact = retained?.state === "succeeded" ? retained.artifact : null;
    if (!artifact || Date.parse(artifact.expiresAt) <= Date.now()) return;
    try {
      const response = await fetcher(
        `/api/exports/${encodeURIComponent(exportId)}/artifact`,
        { priority: "low" },
      );
      if (!owns(photoId, stamp)) return;
      if (
        response.status !== 200 ||
        !historicalArtifactMatchesHeaders(artifact, response.headers)
      ) {
        value.note =
          "The historical export download did not match its published metadata and was discarded.";
        publish(photoId, stamp);
        return;
      }
      const bytes = await response.blob();
      const digest = await blobSha256Hex(bytes);
      if (!owns(photoId, stamp)) return;
      if (bytes.size !== artifact.byteLength || digest !== artifact.sha256) {
        value.note =
          "The historical export download did not match its digest and was discarded.";
        publish(photoId, stamp);
        return;
      }
      const url = URL.createObjectURL(bytes);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = artifact.filename;
      anchor.click();
      URL.revokeObjectURL(url);
      value.note = `Downloaded ${artifact.filename} (${formatByteCount(bytes.size)}).`;
      publish(photoId, stamp);
    } catch {
      if (owns(photoId, stamp)) {
        value.note =
          "The historical export could not be downloaded. Try again.";
        publish(photoId, stamp);
      }
    }
  };
  return {
    reset: (): void => {
      generation += 1;
      clearTimeout(timer);
      timer = undefined;
      photo = null;
    },
    load,
    submit,
    check,
    cancel,
    retry,
    downloadArtifact,
    downloadHistorical,
    unresolved: (photoId: string): boolean => slot(photoId).pending !== null,
    download: async (photoId: string): Promise<void> => {
      const artifact = slot(photoId).artifact;
      if (artifact) await downloadArtifact(photoId, artifact.artifactId);
    },
    view: () => {
      const value = photo ? slot(photo) : empty();
      const artifacts = [...value.artifacts];
      for (const artifact of recipe.artifacts) {
        if (
          artifact.photoId === photo &&
          !artifacts.some((item) => item.artifactId === artifact.artifactId)
        )
          artifacts.push(artifact);
      }
      return {
        historicalExports: value.historicalExports.map((item) => {
          const artifact = item.artifact;
          const canDownload =
            item.state === "succeeded" &&
            artifact !== null &&
            Date.parse(artifact.expiresAt) > Date.now();
          return {
            ...item,
            artifactId: item.exportId,
            filename: artifact?.filename ?? "",
            type: artifact?.contentType ?? "",
            canDownload,
            note: artifact
              ? `Historical export: ${artifact.filename}, ${formatByteCount(artifact.byteLength)}, ${artifact.width}×${artifact.height}, ${artifact.orientation}, ${artifact.sampleFormat}, ${artifact.profileIdentity}, ICC ${artifact.iccEmbedded ? "embedded" : "absent"}. Created ${item.createdAt ?? "unknown"}. ${canDownload ? `Available until ${artifact.expiresAt}.` : "This export has expired. Export again."}`
              : `Historical export ${item.state}${item.failureReason ? `: ${item.failureReason}` : ""}.`,
          };
        }),
        state: value.state,
        note: value.note,
        canSubmit:
          processingAvailable() &&
          recipe.target().kind === "step" &&
          !recipe.dirty() &&
          !value.pending &&
          value.state !== "submitting",
        canCancel: value.state === "running",
        canRetry:
          value.state === "outcome-unknown" ||
          value.exports.some(
            (item) =>
              item.requestId === value.requestId &&
              (item.state === "failed" || item.state === "cancelled"),
          ),
        canDownload:
          value.artifact !== null &&
          Date.parse(value.artifact.expiresAt) > Date.now(),
        processingRequestId: value.requestId,
        processingArtifact: value.artifact,
        processingExports: value.exports.map((work) => ({
          ...work,
          note:
            work.state === "accepted"
              ? "Queued / waiting"
              : work.state === "executing"
                ? "Exporting"
                : work.state === "succeeded"
                  ? value.artifacts.some(
                      (artifact) =>
                        artifact.artifactId === work.artifactId &&
                        Date.parse(artifact.expiresAt) <= Date.now(),
                    )
                    ? "This export has expired. Export again."
                    : "Ready to download"
                  : work.state === "cancelled"
                    ? "Export cancelled"
                    : `Export failed${work.failureReason ? `: ${work.failureReason}` : ""}`,
          canCancel: work.state === "accepted" || work.state === "executing",
          canRetry:
            !value.pending &&
            value.state !== "submitting" &&
            (work.state === "failed" || work.state === "cancelled"),
        })),
        processingArtifacts: artifacts.map((artifact) => {
          const currentStep = recipe.read?.recipe?.steps.find(
            (step) => step.stepId === artifact.stepId,
          );
          const isExpired = Date.parse(artifact.expiresAt) <= Date.now();
          const work = value.exports.find(
            (item) => item.artifactId === artifact.artifactId,
          );
          const isStale =
            recipe.dirty() ||
            !currentStep ||
            currentStep.module !== artifact.module ||
            JSON.stringify(currentStep.parameters) !==
              JSON.stringify(artifact.parameters) ||
            JSON.stringify(currentStep.input) !==
              JSON.stringify(artifact.input.binding) ||
            Boolean(
              work &&
                (work.recipeRevision !== recipe.read?.recipe?.revision ||
                  work.sourceRevision !== recipe.read?.sourceRevision),
            );
          return {
            ...artifact,
            isExpired,
            isStale,
            canDownload: !isExpired,
            note: `${describeProcessingArtifact(artifact, formatByteCount)}${isStale ? " Based on earlier settings." : ""}${isExpired ? " This export has expired. Export again." : ` Available until ${artifact.expiresAt}.`}`,
          };
        }),
      };
    },
  };
}
