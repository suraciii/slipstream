import type { BrowserFetch } from "./access-session.js";
import { isRecord } from "../api/editor.js";
import {
  artifactMatchesHeaders,
  parseExportInspection,
  selectExportPair,
  type ExportInspection,
  type ExportArtifact,
} from "./photo-export.js";

export type OutputTarget = "development-tiff" | "film-jpeg";
export type OutputState =
  | "idle"
  | "submitting"
  | "outcome-unknown"
  | ExportInspection["state"];
export type XmpArtifact = Readonly<{
  exportId: string;
  photoId: string;
  recipeVersion: string;
  sourceRevision: string;
  createdAt: string;
  expiresAt: string;
  filename: string;
  contentType: string;
  byteLength: number;
  sha256: string;
}>;
export type ImageOutputView = Readonly<{
  target: OutputTarget;
  state: OutputState;
  note: string;
  diagnostic: string;
  artifact: ExportArtifact | null;
  createdAt: string | null;
  isStale: boolean;
  canSubmit: boolean;
  canCancel: boolean;
  canRetry: boolean;
  canDownload: boolean;
}>;
export type XmpOutputView = Readonly<{
  state: OutputState;
  note: string;
  artifact: XmpArtifact | null;
  isStale: boolean;
  canSubmit: boolean;
  canDownload: boolean;
}>;
export type WorkspaceOutputsView = Readonly<{
  tiff: ImageOutputView;
  film: ImageOutputView;
  xmp: XmpOutputView;
}>;
export type OutputFacts = Readonly<{
  recipeVersion: string | null;
  sourceRevision: string | null;
  recipeSourceRevision: string | null;
  saving: boolean;
  dirty: boolean;
  conflict: boolean;
  canRender: boolean;
  canRenderFilm: boolean;
}>;
type Submission = Readonly<{
  requestId: string;
  expectedRecipeVersion: string;
  expectedSourceRevision: string;
  target?: OutputTarget;
  retryExportId?: string;
}>;
/// One admission per Photo: the unresolved submission body and the barrier
/// that holds only that Photo's later writes until the outcome is settled.
/// The body is immutable and the barrier is born with it, so a replay reuses
/// its exact identity and the two can never disagree about what is pending.
type Admission = Readonly<{
  body: Submission;
  barrier: Readonly<{ promise: Promise<void>; resolve: () => void }>;
}>;
type ImageSlot = {
  state: OutputState;
  note: string;
  active: ExportInspection | null;
  retained: ExportInspection | null;
};
type Session = {
  images: Record<OutputTarget, ImageSlot>;
  xmp: XmpArtifact | null;
  xmpState: OutputState;
  xmpNote: string;
  admission: Admission | undefined;
  generation: number;
  submitting: boolean;
};
const emptySlot = (): ImageSlot => ({
  state: "idle",
  note: "Not exported yet.",
  active: null,
  retained: null,
});
const newSession = (): Session => ({
  images: { "development-tiff": emptySlot(), "film-jpeg": emptySlot() },
  xmp: null,
  xmpState: "idle",
  xmpNote: "Not exported yet.",
  admission: undefined,
  generation: 0,
  submitting: false,
});
const targets: readonly OutputTarget[] = ["development-tiff", "film-jpeg"];
const label = (target: OutputTarget): string =>
  target === "film-jpeg" ? "Finished JPEG" : "Editing TIFF";
const expired = (time: string): boolean =>
  !Number.isFinite(Date.parse(time)) || Date.parse(time) <= Date.now();
const stale = (
  recipe: string | undefined,
  source: string | undefined,
  facts: OutputFacts,
): boolean =>
  recipe !== facts.recipeVersion ||
  source !== (facts.sourceRevision ?? facts.recipeSourceRevision);
export function parseXmpArtifact(
  value: unknown,
  photoId: string,
): XmpArtifact | undefined {
  if (
    !isRecord(value) ||
    value["photoId"] !== photoId ||
    value["target"] !== "edit-state-xmp" ||
    value["state"] !== "succeeded" ||
    !isRecord(value["artifact"])
  )
    return;
  const artifact = value["artifact"];
  for (const key of [
    "exportId",
    "recipeVersion",
    "sourceRevision",
    "createdAt",
    "expiresAt",
  ])
    if (typeof value[key] !== "string" || !value[key]) return;
  for (const key of ["filename", "contentType", "sha256"])
    if (typeof artifact[key] !== "string" || !artifact[key]) return;
  if (
    artifact["contentType"] !== "application/rdf+xml" ||
    !/^[a-f0-9]{64}$/.test(String(artifact["sha256"])) ||
    typeof artifact["byteLength"] !== "number" ||
    !Number.isSafeInteger(artifact["byteLength"]) ||
    artifact["byteLength"] <= 0 ||
    artifact["byteLength"] > 65536
  )
    return;
  return {
    photoId,
    exportId: String(value["exportId"]),
    recipeVersion: String(value["recipeVersion"]),
    sourceRevision: String(value["sourceRevision"]),
    createdAt: String(value["createdAt"]),
    expiresAt: String(value["expiresAt"]),
    filename: String(artifact["filename"]),
    contentType: String(artifact["contentType"]),
    sha256: String(artifact["sha256"]),
    byteLength: artifact["byteLength"],
  };
}
export function imageOutputView(
  slot: ImageSlot,
  target: OutputTarget,
  facts: OutputFacts,
  locked: boolean,
): ImageOutputView {
  const retained = slot.retained;
  const artifact = retained?.artifact ?? null;
  const isExpired = artifact !== null && expired(artifact.expiresAt);
  const isStale =
    retained !== null &&
    stale(retained.recipeVersion, retained.sourceRevision, facts);
  const waiting = facts.saving || facts.dirty;
  const supported =
    target === "film-jpeg" ? facts.canRenderFilm : facts.canRender;
  const active =
    slot.state === "queued" ||
    slot.state === "running" ||
    slot.state === "submitting";
  return {
    target,
    state: slot.state,
    note: waiting ? "Waiting for your edit to finish saving." : slot.note,
    diagnostic: slot.active?.failureReason ?? "",
    artifact,
    createdAt: retained?.createdAt ?? null,
    isStale,
    canSubmit:
      !locked &&
      !active &&
      !waiting &&
      !facts.conflict &&
      supported &&
      Boolean(facts.recipeVersion && facts.sourceRevision),
    canCancel: slot.state === "queued" || slot.state === "running",
    canRetry:
      !waiting &&
      !facts.conflict &&
      (slot.state === "outcome-unknown" ||
        (!locked &&
          Boolean(slot.active) &&
          (slot.state === "failed" || slot.state === "cancelled") &&
          supported)),
    canDownload: artifact !== null && !isExpired,
  };
}
const failureNote = async (
  response: Response,
  subject: string,
): Promise<string> => {
  const value: unknown = await response.json().catch(() => undefined);
  const code =
    isRecord(value) && isRecord(value["error"]) ? value["error"]["code"] : "";
  switch (code) {
    case "recipe_conflict":
    case "stale_edit":
    case "source_changed":
    case "requires_rebind":
      return `${subject} could not start because the saved edit or Original changed. Reload the edit and try again.`;
    case "resource_unavailable":
      return `${subject} could not start because the Original or processing capacity is unavailable. Your saved edit and earlier files are retained.`;
    case "retained_output_full":
      return `${subject} could not start because output storage is full. Earlier files are retained.`;
    case "processing_unavailable":
      return `${subject} is temporarily unavailable. Your saved edit and earlier files are retained.`;
    case "artifact_expired":
    case "export_expired":
      return "This file has expired. Export again.";
    case "missing_recipe":
      return "Make and save an adjustment before exporting editing state.";
    default:
      return `${subject} could not complete. Reload the edit or try again. Earlier files are retained.`;
  }
};
const inspectionNote = (inspection: ExportInspection): string => {
  if (inspection.state === "queued")
    return "Added to the export queue. You can leave this Photo.";
  if (inspection.state === "running")
    return `Generating ${label(inspection.target)}. You can leave this Photo.`;
  if (inspection.state === "succeeded")
    return inspection.artifact && !expired(inspection.artifact.expiresAt)
      ? "Ready to download."
      : "This file is unavailable or expired. Export again.";
  if (inspection.state === "cancelled")
    return "Export cancelled. Earlier files are retained.";
  return "Could not generate this file. Your saved edit and earlier files are retained. Check details, restore the Original if needed, and retry.";
};
async function blobMatches(
  blob: Blob,
  expected: { byteLength: number; sha256: string },
): Promise<boolean> {
  if (blob.size !== expected.byteLength) return false;
  const bytes = await crypto.subtle.digest("SHA-256", await blob.arrayBuffer());
  return (
    Array.from(new Uint8Array(bytes), (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("") === expected.sha256
  );
}
function xmpHeadersMatch(
  artifact: XmpArtifact,
  headers: Readonly<{ get(name: string): string | null }>,
): boolean {
  const expected: Record<string, string> = {
    "slipstream-artifact-export-id": artifact.exportId,
    "slipstream-artifact-target": "edit-state-xmp",
    "slipstream-artifact-filename": artifact.filename,
    "slipstream-artifact-content-type": artifact.contentType,
    "slipstream-artifact-byte-length": String(artifact.byteLength),
    "slipstream-artifact-sha256": artifact.sha256,
    "slipstream-artifact-created-at": artifact.createdAt,
    "slipstream-artifact-expires-at": artifact.expiresAt,
  };
  return (
    headers.get("content-type") === artifact.contentType &&
    Object.entries(expected).every(
      ([name, value]) => headers.get(name) === value,
    )
  );
}
function offerDownload(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}

export function createWorkspaceOutputController(
  fetcher: BrowserFetch,
  dependencies: Readonly<{
    facts(photoId: string): OutputFacts | undefined;
    owns(photoId: string): boolean;
    render(): void;
    settle(photoId: string): Promise<boolean>;
  }>,
) {
  const sessions = new Map<string, Session>();
  let photo: string | undefined;
  let scope = 0;
  let timer: number | undefined;
  const session = (id: string): Session => {
    let value = sessions.get(id);
    if (!value) {
      value = newSession();
      sessions.set(id, value);
    }
    return value;
  };
  /// The one place an admission begins: its barrier is born with its body.
  const admit = (value: Session, body: Submission): void => {
    value.admission = { body, barrier: Promise.withResolvers<void>() };
  };
  /// The one place an admission ends: the barrier releases exactly when the
  /// body it fences is settled, so a resolved outcome frees dependent writes
  /// and an uncertain one keeps holding them.
  const settleAdmission = (value: Session, admission: Admission): void => {
    if (value.admission !== admission) return;
    value.admission = undefined;
    admission.barrier.resolve();
  };
  const owns = (id: string, stamp: number) =>
    photo === id && scope === stamp && dependencies.owns(id);
  const render = (id: string) => {
    if (photo === id && dependencies.owns(id)) dependencies.render();
  };
  const schedule = (id: string) => {
    clearTimeout(timer);
    timer = undefined;
    if (
      photo !== id ||
      !targets.some((target) => {
        const slot = session(id).images[target];
        return (
          ["queued", "running"].includes(slot.state) ||
          (slot.state === "succeeded" &&
            slot.active?.inspectionPending === true)
        );
      })
    )
      return;
    timer = window.setTimeout(() => {
      timer = undefined;
      void refresh(id);
    }, 1500);
  };
  const refresh = async (id: string): Promise<void> => {
    const value = session(id);
    const stamp = scope;
    const generation = ++value.generation;
    try {
      const [imagesResponse, xmpResponse] = await Promise.all([
        fetcher(`/api/photos/${encodeURIComponent(id)}/exports`, {
          priority: "low",
        }),
        fetcher(`/api/photos/${encodeURIComponent(id)}/edit-state-exports`, {
          priority: "low",
        }),
      ]);
      const [images, xmps] = await Promise.all([
        imagesResponse.json().catch(() => undefined) as Promise<unknown>,
        xmpResponse.json().catch(() => undefined) as Promise<unknown>,
      ]);
      if (!owns(id, stamp) || value.generation !== generation) return;
      if (
        imagesResponse.ok &&
        isRecord(images) &&
        Array.isArray(images["exports"])
      ) {
        const summaries = images["exports"].filter(isRecord);
        const inspections = await Promise.all(
          summaries.map(async (summary) => {
            if (typeof summary["exportId"] !== "string") return undefined;
            const response = await fetcher(
              `/api/exports/${encodeURIComponent(summary["exportId"])}`,
              { priority: "low" },
            ).catch(() => undefined);
            if (!response?.ok) return undefined;
            const inspection = parseExportInspection(
              await response.json().catch(() => undefined),
            );
            return inspection?.exportId === summary["exportId"]
              ? inspection
              : undefined;
          }),
        );
        if (!owns(id, stamp) || value.generation !== generation) return;
        const entries = inspections.filter(
          (item): item is ExportInspection => item !== undefined,
        );
        for (const target of targets) {
          const pair = selectExportPair(entries, target);
          const slot = value.images[target];
          const latestSummary = summaries
            .filter(
              (summary) =>
                summary["target"] === target &&
                typeof summary["exportId"] === "string" &&
                [
                  "queued",
                  "running",
                  "succeeded",
                  "failed",
                  "cancelled",
                ].includes(String(summary["state"])),
            )
            .sort((a, b) =>
              (typeof b["createdAt"] === "string"
                ? b["createdAt"]
                : ""
              ).localeCompare(
                typeof a["createdAt"] === "string" ? a["createdAt"] : "",
              ),
            )[0];
          const hydrated = latestSummary
            ? entries.find(
                (entry) => entry.exportId === latestSummary["exportId"],
              )
            : undefined;
          const summaryActive: ExportInspection | null =
            latestSummary && !hydrated
              ? {
                  exportId: String(latestSummary["exportId"]),
                  target,
                  state: String(
                    latestSummary["state"],
                  ) as ExportInspection["state"],
                  failureReason:
                    typeof latestSummary["failureReason"] === "string"
                      ? latestSummary["failureReason"]
                      : "",
                  createdAt:
                    typeof latestSummary["createdAt"] === "string"
                      ? latestSummary["createdAt"]
                      : undefined,
                  recipeVersion:
                    typeof latestSummary["recipeVersion"] === "string"
                      ? latestSummary["recipeVersion"]
                      : undefined,
                  sourceRevision:
                    typeof latestSummary["sourceRevision"] === "string"
                      ? latestSummary["sourceRevision"]
                      : undefined,
                  artifact: null,
                  inspectionPending: true,
                }
              : null;
          if (pair.retained) slot.retained = pair.retained;
          if (value.admission?.body.target === target) continue;
          if (
            !pair.active &&
            !summaryActive &&
            inspections.some((entry) => entry === undefined)
          )
            continue;
          slot.active = summaryActive ?? pair.active;
          slot.state = slot.active?.state ?? "idle";
          slot.note = slot.active
            ? inspectionNote(slot.active)
            : "Not exported yet.";
        }
      }
      if (xmpResponse.ok && isRecord(xmps) && Array.isArray(xmps["exports"])) {
        const entries = xmps["exports"]
          .map((entry) => parseXmpArtifact(entry, id))
          .filter((entry): entry is XmpArtifact => entry !== undefined);
        value.xmp = entries[0] ?? null;
        if (!value.admission?.body.target) {
          value.xmpState = value.xmp ? "succeeded" : "idle";
          value.xmpNote = value.xmp
            ? expired(value.xmp.expiresAt)
              ? "This file has expired. Export again."
              : "Ready to download. XMP contains parameters, not rendered pixels."
            : "Not exported yet.";
        }
      }
      render(id);
      schedule(id);
    } catch {
      if (owns(id, stamp)) {
        render(id);
        schedule(id);
      }
    }
  };
  const inspect = async (
    id: string,
    target: OutputTarget,
    exportId: string,
  ): Promise<void> => {
    const value = session(id);
    const stamp = scope;
    const generation = ++value.generation;
    const slot = value.images[target];
    try {
      const response = await fetcher(
        `/api/exports/${encodeURIComponent(exportId)}`,
        { priority: "low" },
      );
      const inspection = parseExportInspection(
        await response.json().catch(() => undefined),
      );
      if (!owns(id, stamp) || generation !== value.generation) return;
      if (
        !response.ok ||
        !inspection ||
        inspection.exportId !== exportId ||
        inspection.target !== target
      ) {
        schedule(id);
        return;
      }
      slot.active = inspection;
      slot.state = inspection.state;
      slot.note = inspectionNote(inspection);
      if (inspection.artifact && inspection.state === "succeeded")
        slot.retained = inspection;
      render(id);
      schedule(id);
    } catch {
      if (owns(id, stamp)) schedule(id);
    }
  };
  const sendPending = async (id: string): Promise<void> => {
    const value = session(id);
    const admission = value.admission;
    const body = admission?.body;
    if (!admission || !body) return;
    const slot = body.target ? value.images[body.target] : undefined;
    const uncertain = () => {
      if (value.admission !== admission) return;
      if (slot) {
        slot.state = "outcome-unknown";
        slot.note =
          "The export result could not be confirmed. Check result before exporting again.";
      } else {
        value.xmpState = "outcome-unknown";
        value.xmpNote =
          "The edit state file result could not be confirmed. Check result before exporting again.";
      }
      render(id);
    };
    try {
      const path = body.retryExportId
        ? `/api/exports/${encodeURIComponent(body.retryExportId)}/retry`
        : `/api/photos/${encodeURIComponent(id)}/${body.target ? "exports" : "edit-state-exports"}`;
      const payload = body.retryExportId ? { requestId: body.requestId } : body;
      const response = await fetcher(path, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      if (value.admission !== admission) return;
      if (!response.ok) {
        const error: unknown = await response
          .clone()
          .json()
          .catch(() => undefined);
        if (value.admission !== admission) return;
        const refused =
          response.status === 503 &&
          isRecord(error) &&
          isRecord(error["error"]) &&
          error["error"]["effect"] === "none" &&
          [
            "processing_unavailable",
            "resource_unavailable",
            "retained_output_full",
          ].includes(String(error["error"]["code"]));
        if (response.status >= 500 && !refused) {
          uncertain();
          return;
        }
        const note = await failureNote(response, "Export");
        if (value.admission !== admission) return;
        settleAdmission(value, admission);
        if (slot) {
          slot.active = null;
          slot.state = "failed";
          slot.note = note;
        } else {
          value.xmpState = "failed";
          value.xmpNote = note;
        }
        render(id);
        return;
      }
      const record: unknown = await response.json().catch(() => undefined);
      if (value.admission !== admission) return;
      const xmp = body.target ? undefined : parseXmpArtifact(record, id);
      if (body.target) {
        if (
          !isRecord(record) ||
          typeof record["exportId"] !== "string" ||
          (!body.retryExportId &&
            (record["target"] !== body.target ||
              record["recipeVersion"] !== body.expectedRecipeVersion ||
              record["sourceRevision"] !== body.expectedSourceRevision)) ||
          (body.retryExportId && record["exportId"] !== body.retryExportId) ||
          !["queued", "running", "succeeded", "failed", "cancelled"].includes(
            String(record["state"]),
          )
        ) {
          uncertain();
          return;
        }
        settleAdmission(value, admission);
        if (slot) {
          slot.state = record["state"] as OutputState;
          slot.note = "Export accepted.";
          slot.active = {
            exportId: record["exportId"],
            target: body.target,
            state: record["state"] as ExportInspection["state"],
            recipeVersion: body.expectedRecipeVersion,
            sourceRevision: body.expectedSourceRevision,
            artifact: null,
            inspectionPending: true,
            failureReason: "",
          };
        }
        if (photo === id) await inspect(id, body.target, record["exportId"]);
        schedule(id);
      } else {
        if (
          !xmp ||
          xmp.recipeVersion !== body.expectedRecipeVersion ||
          xmp.sourceRevision !== body.expectedSourceRevision
        ) {
          uncertain();
          return;
        }
        settleAdmission(value, admission);
        value.xmp = xmp;
        value.xmpState = "succeeded";
        value.xmpNote =
          "Ready to download. Standard exposure and as-shot white balance can be read by compatible applications; Film and custom white balance require Slipstream.";
      }
      render(id);
    } catch {
      uncertain();
    }
  };
  const submit = async (id: string, target?: OutputTarget): Promise<void> => {
    const value = session(id);
    if (!dependencies.owns(id) || value.submitting) return;
    if (value.admission) {
      if (value.admission.body.target === target) await sendPending(id);
      return;
    }
    const before = dependencies.facts(id);
    if (!before) return;
    if (
      target &&
      !imageOutputView(value.images[target], target, before, false).canSubmit
    )
      return;
    value.submitting = true;
    value.generation++;
    if (target) {
      value.images[target].state = "submitting";
      value.images[target].note = "Waiting for your edit to finish saving.";
    } else {
      value.xmpState = "submitting";
      value.xmpNote = "Waiting for your edit to finish saving.";
    }
    render(id);
    try {
      if (!(await dependencies.settle(id))) {
        if (target) {
          value.images[target].state = "idle";
          value.images[target].note =
            "Finish saving or resolve the edit conflict before exporting.";
        } else {
          value.xmpState = "failed";
          value.xmpNote =
            "Finish saving or resolve the edit conflict before exporting.";
        }
        render(id);
        return;
      }
      const facts = dependencies.facts(id);
      const source = target
        ? facts?.sourceRevision
        : facts?.recipeSourceRevision;
      if (
        !facts?.recipeVersion ||
        !source ||
        facts.saving ||
        facts.dirty ||
        facts.conflict ||
        (target &&
          !(target === "film-jpeg" ? facts.canRenderFilm : facts.canRender))
      ) {
        if (target) value.images[target].state = "idle";
        else value.xmpState = "failed";
        render(id);
        return;
      }
      const body: Submission = {
        requestId: `web-${target ?? "xmp"}-${crypto.randomUUID()}`,
        expectedRecipeVersion: facts.recipeVersion,
        expectedSourceRevision: source,
        ...(target ? { target } : {}),
      };
      admit(value, body);
      await sendPending(id);
    } finally {
      value.submitting = false;
      render(id);
    }
  };
  const cancel = async (id: string, target: OutputTarget): Promise<void> => {
    const value = session(id);
    const active = value.images[target].active;
    if (!active || !dependencies.owns(id)) return;
    try {
      const response = await fetcher(
        `/api/exports/${encodeURIComponent(active.exportId)}/cancel`,
        { method: "POST" },
      );
      if (!response.ok) {
        value.images[target].note = await failureNote(response, "Cancellation");
        render(id);
        return;
      }
      await inspect(id, target, active.exportId);
    } catch {
      value.images[target].note =
        "Cancellation could not be confirmed. Reload the edit to check the task.";
      render(id);
    }
  };
  const retry = async (id: string, target: OutputTarget): Promise<void> => {
    const value = session(id);
    if (!dependencies.owns(id)) return;
    if (value.admission) {
      if (value.admission.body.target === target) await sendPending(id);
      return;
    }
    const active = value.images[target].active;
    if (!active) return;
    admit(value, {
      requestId: `web-retry-${crypto.randomUUID()}`,
      expectedRecipeVersion: active.recipeVersion ?? "",
      expectedSourceRevision: active.sourceRevision ?? "",
      target,
      retryExportId: active.exportId,
    });
    value.generation++;
    value.submitting = true;
    value.images[target].state = "submitting";
    render(id);
    try {
      await sendPending(id);
    } finally {
      value.submitting = false;
      render(id);
    }
  };
  const download = async (id: string, target?: OutputTarget): Promise<void> => {
    if (!dependencies.owns(id)) return;
    const value = session(id);
    const retained = target ? value.images[target].retained : null;
    const artifact = target ? retained?.artifact : value.xmp;
    if (!artifact || expired(artifact.expiresAt)) return;
    const stamp = scope;
    const note = (message: string) => {
      if (!owns(id, stamp)) return;
      if (target) value.images[target].note = message;
      else value.xmpNote = message;
      render(id);
    };
    try {
      const response = await fetcher(
        target
          ? `/api/exports/${encodeURIComponent(artifact.exportId)}/artifact`
          : `/api/photos/${encodeURIComponent(id)}/edit-state-exports/${encodeURIComponent(artifact.exportId)}/artifact`,
        { priority: "high" },
      );
      if (!response.ok) {
        note(await failureNote(response, "Download"));
        return;
      }
      if (
        (target &&
          !artifactMatchesHeaders(
            artifact as ExportArtifact,
            response.headers,
          )) ||
        (!target && !xmpHeadersMatch(artifact as XmpArtifact, response.headers))
      ) {
        note(
          "The downloaded file did not match this output and was discarded.",
        );
        return;
      }
      const blob = await response.blob();
      if (!(await blobMatches(blob, artifact))) {
        note(
          "The downloaded file was incomplete or changed and was discarded.",
        );
        return;
      }
      if (!owns(id, stamp)) return;
      offerDownload(
        blob,
        artifact.filename ??
          `slipstream-${artifact.exportId}.${target === "film-jpeg" ? "jpg" : "tiff"}`,
      );
      note(
        "Download started. This file preserves the state shown in its details.",
      );
    } catch {
      note(
        "The download could not complete. Your existing file is retained; try again.",
      );
    }
  };
  return {
    open(id: string) {
      photo = id;
      scope++;
      session(id);
      void refresh(id);
    },
    refresh,
    leave() {
      scope++;
      photo = undefined;
      clearTimeout(timer);
      timer = undefined;
    },
    pending(id: string) {
      return Boolean(session(id).admission);
    },
    /// The barrier holding this Photo's later writes behind its unresolved
    /// submission. It exists exactly while the admission does, so the edit
    /// stream can never wait on a barrier whose body already settled.
    writeBarrier(id: string): Promise<void> | undefined {
      return session(id).admission?.barrier.promise;
    },
    view(id: string): WorkspaceOutputsView {
      const value = session(id);
      const facts = dependencies.facts(id) ?? {
        recipeVersion: null,
        sourceRevision: null,
        recipeSourceRevision: null,
        saving: false,
        dirty: false,
        conflict: false,
        canRender: false,
        canRenderFilm: false,
      };
      const locked = Boolean(value.admission) || value.submitting;
      return {
        tiff: imageOutputView(
          value.images["development-tiff"],
          "development-tiff",
          facts,
          locked,
        ),
        film: imageOutputView(
          value.images["film-jpeg"],
          "film-jpeg",
          facts,
          locked,
        ),
        xmp: {
          state: value.xmpState,
          note:
            facts.saving || facts.dirty
              ? "Waiting for your edit to finish saving."
              : value.xmp && expired(value.xmp.expiresAt)
                ? "This file has expired. Export again."
                : value.xmpNote,
          artifact: value.xmp,
          isStale:
            value.xmp !== null &&
            stale(value.xmp.recipeVersion, value.xmp.sourceRevision, facts),
          canSubmit:
            (!locked || value.xmpState === "outcome-unknown") &&
            !facts.saving &&
            !facts.dirty &&
            !facts.conflict &&
            Boolean(facts.recipeVersion && facts.recipeSourceRevision),
          canDownload: value.xmp !== null && !expired(value.xmp.expiresAt),
        },
      };
    },
    submit,
    cancel,
    retry,
    download,
  };
}
