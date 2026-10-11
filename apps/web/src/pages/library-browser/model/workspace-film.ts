import type { ProcessingModuleDescription } from "../api/processing-modules.js";
import { isRecord } from "../api/guards.js";
import {
  readWorkspaceFilm,
  postWorkspaceFilm,
  workspaceFilmRoot,
  WorkspaceFilmError,
  parseWorkspaceFilmRead,
  workspaceFilmJson,
  type WorkspaceFilmFile,
  type WorkspaceFilmRead,
  type WorkspaceFilmStep,
} from "../api/workspace-film.js";
import type { BrowserFetch } from "./access-session.js";
import { randomUuid, blobSha256Hex } from "./browser-crypto.js";

/** Discovery expresses precision in bits; inspection expresses actual sample type. */
export function canContinueFilm(
  contract: Record<string, unknown>,
  module: ProcessingModuleDescription,
): boolean {
  const geometry = contract.geometry;
  if (
    module.id.name !== "spektrafilm" ||
    module.availability.state !== "ready" ||
    !isRecord(module.parameterSchema) ||
    !isRecord(module.parameterSchema.default) ||
    !module.parameterVersions.length ||
    !isRecord(geometry)
  )
    return false;
  const width = geometry.width,
    height = geometry.height;
  if (
    typeof width !== "number" ||
    typeof height !== "number" ||
    !Number.isInteger(width) ||
    !Number.isInteger(height) ||
    width < 1 ||
    height < 1 ||
    width > 65536 ||
    height > 65536 ||
    typeof contract.encoding !== "string" ||
    !contract.encoding
  )
    return false;
  // The retained Film adapter is qualified for floating-point scene-linear TIFF only.
  if (contract.precision !== "float32" || contract.transfer !== "linear")
    return false;
  return module.admittedInputs.some(
    (input) =>
      input.format === contract.format &&
      input.colorSpace === contract.colorSpace &&
      input.transferFunction === contract.transfer &&
      input.precisionBits === 32 &&
      (input.width === 0 || input.width === width) &&
      (input.height === 0 || input.height === height),
  );
}
export function filmStepForFile(
  read: WorkspaceFilmRead,
  file: WorkspaceFilmFile,
  module: ProcessingModuleDescription,
  selectedStepId?: string,
): WorkspaceFilmStep {
  if (!canContinueFilm(file.imageContract, module))
    throw new WorkspaceFilmError(
      "This verified image is not compatible with the available Film tool.",
      "incompatible_input",
    );
  const input = {
    kind: "workspace-artifact",
    workspaceId: file.workspaceId,
    artifactId: file.artifactId,
    revision: file.revision,
    contract: structuredClone(file.imageContract),
  };
  const matches =
    read.recipe?.steps.filter(
      (step) =>
        step.module === module.id.name &&
        step.input.kind === input.kind &&
        step.input.workspaceId === file.workspaceId &&
        step.input.artifactId === file.artifactId &&
        step.input.revision === file.revision,
    ) ?? [];
  if (selectedStepId) {
    const selected = matches.find((value) => value.stepId === selectedStepId);
    if (!selected)
      throw new WorkspaceFilmError(
        "The selected Film edit no longer uses this exact image revision.",
        "edit_changed",
      );
    return structuredClone(selected);
  }
  if (matches.length > 1)
    throw new WorkspaceFilmError(
      "Several saved Film edits use this file. Choose an edit before continuing.",
      "ambiguous_edit",
    );
  if (matches[0]) return structuredClone(matches[0]);
  const used = new Set(read.recipe?.steps.map((step) => step.stepId));
  let index = 1;
  while (used.has(`film-${index}`)) index += 1;
  return {
    stepId: `film-${index}`,
    module: module.id.name,
    input,
    parameters: {
      schemaVersion: module.parameterVersions[0]!,
      tree: structuredClone(
        (module.parameterSchema as Record<string, unknown>).default,
      ),
    },
  };
}
export function filmSaveBody(
  read: WorkspaceFilmRead,
  step: WorkspaceFilmStep,
  requestId: string,
): string {
  const retained = read.recipe?.steps ?? [];
  return JSON.stringify({
    requestId,
    expectedRecipeRevision: read.recipe?.revision ?? null,
    expectedSourceRevision: read.sourceRevision,
    currentStepId: step.stepId,
    steps: retained.some((value) => value.stepId === step.stepId)
      ? retained.map((value) => (value.stepId === step.stepId ? step : value))
      : [...retained, step],
  });
}
export function createWorkspaceFilmOwner(
  fetcher: BrowserFetch,
  photoId: string,
  changed: () => void,
) {
  let read: WorkspaceFilmRead | undefined, step: WorkspaceFilmStep | undefined;
  let generation = 0,
    disposed = false,
    busy = false,
    dirty = false,
    conflict = false,
    conflictInspected = false;
  let pending:
    | { path: string; body: string; kind: "save" | "export" }
    | undefined;
  let receipt: Record<string, unknown> | undefined,
    note = "Choose a completed compatible image to continue with Film.";
  const controller = new AbortController();
  const owns = (stamp: number) => !disposed && stamp === generation;
  const publish = () => {
    if (!disposed) changed();
  };
  const root = workspaceFilmRoot(photoId);
  async function sendPending(): Promise<void> {
    if (!pending || busy) return;
    const captured = pending,
      stamp = generation;
    busy = true;
    publish();
    try {
      const result = await postWorkspaceFilm(
        fetcher,
        captured.path,
        captured.body,
      );
      if (!owns(stamp)) return;
      if (captured.kind === "save") {
        const next = parseSaved(result);
        read = next;
        step = next.recipe?.steps.find(
          (value) => value.stepId === step?.stepId,
        );
        dirty = false;
        note = "Film settings saved.";
      } else {
        const next = isRecord(result.receipt) ? result.receipt : result;
        const request = JSON.parse(captured.body) as Record<string, unknown>;
        if (
          next.photoId !== photoId ||
          next.requestId !== request.requestId ||
          next.stepId !== (request.stepId ?? receipt?.stepId)
        )
          throw new WorkspaceFilmError(
            "The Film Export response could not be identified. Reconcile this exact request.",
            "invalid_response",
            true,
          );
        receipt = next;
        note = "Film Export accepted.";
      }
      pending = undefined;
    } catch (error) {
      if (!owns(stamp)) return;
      if (error instanceof WorkspaceFilmError && !error.uncertain) {
        pending = undefined;
        conflict = error.code === "conflict";
        conflictInspected = false;
      }
      note = error instanceof Error ? error.message : "Film operation failed.";
    } finally {
      if (owns(stamp)) {
        busy = false;
        publish();
      }
    }
  }
  function parseSaved(value: Record<string, unknown>): WorkspaceFilmRead {
    // Save responses carry exactly the same source/recipe projection as get.
    if (
      value.photoId !== photoId ||
      typeof value.sourceRevision !== "string" ||
      !isRecord(value.recipe)
    )
      throw new WorkspaceFilmError(
        "The saved Film response could not be identified. Reconcile this request.",
        "invalid_response",
        true,
      );
    try {
      return parseWorkspaceFilmRead(value, photoId);
    } catch {
      throw new WorkspaceFilmError(
        "The saved Film response could not be identified. Reconcile this request.",
        "invalid_response",
        true,
      );
    }
  }
  async function save(): Promise<void> {
    if (pending) {
      await sendPending();
      return;
    }
    if (!read || !step || busy || !dirty || conflict) return;
    pending = {
      path: `${root}/processing-recipe`,
      body: filmSaveBody(read, step, randomUuid()),
      kind: "save",
    };
    note = "Saving Film settings…";
    await sendPending();
  }
  return {
    view: () => ({
      read,
      step,
      busy,
      dirty,
      note,
      conflict,
      uncertain: Boolean(pending),
      receipt,
    }),
    async open(
      file: WorkspaceFilmFile,
      module: ProcessingModuleDescription,
      selectedStepId?: string,
    ): Promise<void> {
      if (busy || pending)
        throw new WorkspaceFilmError(
          "Settle the current Film request before selecting another input.",
          "pending_action",
        );
      const stamp = ++generation;
      busy = true;
      note = `Opening Film for ${file.relativePath.split("/").at(-1)}…`;
      publish();
      try {
        const next = await readWorkspaceFilm(
          fetcher,
          photoId,
          controller.signal,
        );
        if (!owns(stamp)) return;
        const selected = filmStepForFile(next, file, module, selectedStepId);
        read = next;
        step = selected;
        receipt = undefined;
        conflict = false;
        dirty =
          next.recipe?.currentStepId !== selected.stepId ||
          !next.recipe?.steps.some((value) => value.stepId === selected.stepId);
        note = dirty
          ? "Film input selected. Save settings to start editing."
          : "Saved Film settings loaded.";
        try {
          const listing = await workspaceFilmJson(
            fetcher,
            `${root}/processing-exports`,
            { signal: controller.signal },
          );
          if (!owns(stamp)) return;
          if (listing.photoId === photoId && Array.isArray(listing.exports))
            receipt = listing.exports.find(
              (value: unknown): value is Record<string, unknown> =>
                isRecord(value) &&
                value.photoId === photoId &&
                value.stepId === selected.stepId,
            );
        } catch {
          /* Export discovery cannot invalidate loaded saved settings. */
        }
      } finally {
        if (owns(stamp)) {
          busy = false;
          publish();
        }
      }
    },
    setParameters(tree: unknown): void {
      if (!step || busy || pending) return;
      step = {
        ...step,
        parameters: { ...step.parameters, tree: structuredClone(tree) },
      };
      dirty = true;
      note = "Film settings have unsaved changes.";
      publish();
    },
    save,
    async export(): Promise<void> {
      if (busy || pending) return;
      await save();
      if (
        !read?.recipe ||
        !step ||
        dirty ||
        pending ||
        busy ||
        conflict ||
        disposed
      )
        return;
      pending = {
        path: `${root}/processing-exports`,
        body: JSON.stringify({
          requestId: randomUuid(),
          stepId: step.stepId,
          expectedRecipeRevision: read.recipe.revision,
          expectedSourceRevision: read.sourceRevision,
        }),
        kind: "export",
      };
      note = "Submitting Film Export…";
      await sendPending();
    },
    async preview(): Promise<Blob | undefined> {
      if (busy || pending) return;
      await save();
      if (
        !read?.recipe ||
        !step ||
        dirty ||
        pending ||
        busy ||
        conflict ||
        disposed
      )
        return;
      const stamp = generation,
        revision = read.recipe.revision,
        stepId = step.stepId;
      busy = true;
      note = "Rendering Film preview…";
      publish();
      try {
        const response = await fetcher(
          `${root}/processing-preview/${encodeURIComponent(stepId)}`,
          { signal: controller.signal },
        );
        if (!owns(stamp)) return;
        if (response.status === 202) {
          note =
            "Film preview is rendering. Check Preview again for readiness.";
          return;
        }
        if (!response.ok) {
          const body: unknown = await response.json();
          throw new Error(
            isRecord(body) &&
            isRecord(body.error) &&
            typeof body.error.message === "string"
              ? body.error.message
              : "Film preview failed.",
          );
        }
        const prefix = "slipstream-processing-preview-";
        if (
          response.headers.get(`${prefix}photo-id`) !== photoId ||
          response.headers.get(`${prefix}step-id`) !== stepId ||
          response.headers.get(`${prefix}recipe-revision`) !== revision ||
          response.headers.get(`${prefix}module`) !== step.module ||
          response.headers.get("content-type")?.split(";")[0] !== "image/png"
        )
          throw new Error(
            "Film preview identity does not match the selected saved settings.",
          );
        const image = await response.blob();
        if (
          (await blobSha256Hex(image)) !==
          response.headers.get(`${prefix}sha256`)
        )
          throw new Error(
            "Film preview bytes did not match their verified identity.",
          );
        if (!owns(stamp)) return;
        note = "Film preview ready.";
        return image;
      } catch (error) {
        if (owns(stamp))
          note =
            error instanceof Error ? error.message : "Film preview failed.";
      } finally {
        if (owns(stamp)) {
          busy = false;
          publish();
        }
      }
    },
    reconcile: sendPending,
    async inspectConflict(): Promise<void> {
      if (!conflict || busy || pending) return;
      const stamp = generation;
      busy = true;
      publish();
      try {
        const current = await readWorkspaceFilm(
          fetcher,
          photoId,
          controller.signal,
        );
        if (!owns(stamp)) return;
        read = current;
        conflictInspected = true;
        note = `Current saved revision: ${current.recipe?.revision ?? "no saved state"}. Your Film draft is retained. Reapply explicitly to replace these Film settings.`;
      } catch (error) {
        if (owns(stamp))
          note =
            error instanceof Error
              ? error.message
              : "Current Film state could not be inspected.";
      } finally {
        if (owns(stamp)) {
          busy = false;
          publish();
        }
      }
    },
    async reapply(): Promise<void> {
      if (!conflict || !conflictInspected || busy || pending) return;
      conflict = false;
      await save();
    },
    async checkExport(
      action: "status" | "cancel" | "retry" = "status",
    ): Promise<void> {
      const requestId = receipt?.requestId;
      if (typeof requestId !== "string" || busy || pending) return;
      if (action === "retry") {
        pending = {
          path: `${root}/processing-exports/${encodeURIComponent(requestId)}/retry`,
          body: JSON.stringify({ requestId: randomUuid() }),
          kind: "export",
        };
        await sendPending();
        return;
      }
      const stamp = generation;
      busy = true;
      publish();
      try {
        const path = `${root}/processing-exports/${encodeURIComponent(requestId)}`;
        const result =
          action === "status"
            ? await workspaceFilmJson(fetcher, path)
            : await postWorkspaceFilm(fetcher, `${path}/cancel`, "{}");
        if (!owns(stamp)) return;
        const next = isRecord(result.receipt) ? result.receipt : result;
        if (next.photoId !== photoId)
          throw new Error("Film Export response belongs to another Photo.");
        receipt = next;
        const state =
          typeof next.state === "string"
            ? next.state
            : typeof next.outcome === "string"
              ? next.outcome
              : "checked";
        note =
          typeof next.failureReason === "string"
            ? next.failureReason
            : `Film Export ${state}.`;
      } catch (error) {
        if (owns(stamp))
          note =
            error instanceof Error
              ? error.message
              : "Film Export could not be checked.";
      } finally {
        if (owns(stamp)) {
          busy = false;
          publish();
        }
      }
    },
    dispose(): void {
      disposed = true;
      generation += 1;
      controller.abort();
    },
  };
}
