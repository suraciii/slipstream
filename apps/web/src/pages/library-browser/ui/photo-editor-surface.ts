import { createEditorControlsRenderer } from "./photo-editor-controls.js";
import { isRecord } from "../api/guards.js";
import {
  fileBytesUrl,
  type JsonObject,
  type WorkspaceFile,
} from "../api/workspace-edit.js";
import type { EditChange } from "../model/workspace-edit-model.js";
import type { EditorViewModel } from "../model/editor-controller-contract.js";
import type { EditorIntent } from "../model/editor-intent-contract.js";
import "./workspace-editor.css";
import { displayScalar, paragraph } from "./presentation-helpers.js";
import { canContinueFilm } from "../model/workspace-film.js";
import { editorFactRows, saveStateRow } from "./editor-facts.js";
export interface PhotoEditorSurfaceController {
  open(photoId: string): void;
  visible(): boolean;
  render(model: EditorViewModel): void;
  presentPreview(url: string): void;
  clearPreview(): void;
  dispose(): void;
}

export function createPhotoEditorSurfaceController({
  root,
  send,
  isPhotoVisible,
  isEditorSurfaceVisible,
  openEditorSurface,
}: {
  root: ParentNode;
  send: (intent: EditorIntent) => void;
  isPhotoVisible: () => boolean;
  isEditorSurfaceVisible: () => boolean;
  openEditorSurface: () => void;
}): PhotoEditorSurfaceController {
  let alive = true;
  let photoId: string | undefined;
  let current: EditorViewModel | undefined;
  let controlsIdentity = "";
  const element = <T extends HTMLElement>(selector: string): T => {
    const found = root.querySelector<T>(selector);
    if (!found) throw new Error(`Missing editor element ${selector}`);
    return found;
  };
  const image = element<HTMLImageElement>("[data-photo-editor-preview-image]");
  const referenceImage = element<HTMLImageElement>(
    "[data-photo-editor-reference-image]",
  );
  const empty = element<HTMLElement>("[data-photo-editor-image-empty]");
  let failedReferenceUrl: string | undefined;
  referenceImage.addEventListener("error", () => {
    if (
      !current ||
      referenceImage.hidden ||
      !(current.cameraReference || (!current.edit && !current.resultSelected))
    )
      return;
    failedReferenceUrl = referenceImage.getAttribute("src") ?? undefined;
    referenceImage.hidden = true;
    empty.textContent = "Camera reference unavailable";
    empty.hidden = false;
  });
  const act = (action: string, args: JsonObject = {}) => {
    if (alive && photoId)
      send({
        kind: "editor-workspace-action",
        photoId,
        action,
        arguments: args,
      });
  };
  const viewListeners = new AbortController();
  const currentButton = element<HTMLButtonElement>(
    "[data-photo-editor-current]",
  );
  const cameraButton = element<HTMLButtonElement>(
    "[data-photo-editor-camera-reference]",
  );
  const compareButton = element<HTMLButtonElement>(
    "[data-photo-editor-compare]",
  );
  currentButton.addEventListener("click", () => act("current"), {
    signal: viewListeners.signal,
  });
  cameraButton.addEventListener("click", () => act("camera"), {
    signal: viewListeners.signal,
  });
  compareButton.addEventListener(
    "click",
    () => act("compare", { pressed: !current?.comparing }),
    { signal: viewListeners.signal },
  );
  const button = (
    label: string,
    action: string,
    args: JsonObject = {},
    disabled = false,
  ) => {
    const control = document.createElement("button");
    control.type = "button";
    control.textContent = label;
    control.disabled = disabled;
    control.addEventListener("click", () => act(action, args));
    return control;
  };
  const change = (value: EditChange) => {
    act("change", { change: value });
  };
  const select = (
    label: string,
    items: { value: string; label: string }[],
    selected: string | undefined,
    onChange: (value: string) => void,
  ) => {
    const wrapper = document.createElement("label");
    wrapper.append(label);
    const control = document.createElement("select");
    control.setAttribute("aria-label", label);
    control.append(new Option("Choose…", ""));
    for (const item of items)
      control.append(new Option(item.label, item.value));
    control.value = selected ?? "";
    control.addEventListener("change", () => {
      if (control.value) onChange(control.value);
    });
    wrapper.append(control);
    return wrapper;
  };
  const renderControls = createEditorControlsRenderer({
    target: element("[data-workspace-controls]"),
    button,
    paragraph,
    select,
    change,
  });
  const fileCard = (file: WorkspaceFile, model: EditorViewModel) => {
    const card = document.createElement("article");
    card.dataset.artifactId = file.artifactId;
    card.append(
      paragraph(
        `${file.relativePath} · ${file.kind} · ${file.byteLength === null ? "Unknown size" : `${file.byteLength.toLocaleString()} bytes`} · ${file.state === "checking" ? "Checking file" : file.state}`,
      ),
    );
    if (isRecord(file.imageContract)) {
      const geometry = isRecord(file.imageContract.geometry)
        ? file.imageContract.geometry
        : file.imageContract;
      card.append(
        paragraph(
          `${displayScalar(geometry.width, "?")} × ${displayScalar(geometry.height, "?")} · ${displayScalar(file.imageContract.format)} · ${displayScalar(file.imageContract.precision)} · ${displayScalar(file.imageContract.transfer)}`,
        ),
      );
    }
    if (model.workspace && file.state === "ready") {
      const download = document.createElement("a");
      download.textContent = "Download";
      download.href = fileBytesUrl(model.workspace.workspaceId, file);
      download.download = file.relativePath.split("/").at(-1) ?? "image";
      card.append(download);
      const format = isRecord(file.imageContract)
        ? displayScalar(file.imageContract.format).toLowerCase()
        : "";
      if (["png", "jpeg", "jpg", "webp"].includes(format)) {
        const thumbnail = document.createElement("img");
        thumbnail.loading = "lazy";
        thumbnail.alt = file.relativePath;
        thumbnail.src = download.href;
        thumbnail.className = "workspace-file-thumbnail";
        card.prepend(thumbnail);
      }
      if (file.kind === "output") {
        const compatible =
          isRecord(file.imageContract) &&
          model.filmModules?.some((module) =>
            canContinueFilm(file.imageContract!, module),
          );
        if (compatible)
          card.append(
            button("Continue processing with Film", "continue-film", {
              artifactId: file.artifactId,
              revision: file.revision,
            }),
          );
        else
          card.append(
            paragraph(
              model.filmModules
                ? "No available compatible next tool. Download remains available."
                : "Checking compatible processing tools…",
            ),
          );
      }
    }
    card.append(
      button("Inspect / Remove…", "inspect", { artifactId: file.artifactId }),
    );
    return card;
  };
  const renderRetainedRevisions = (model: EditorViewModel) => {
    const target = element("[data-workspace-revision-list]");
    target.replaceChildren();
    if (!model.history.length) {
      target.append(
        paragraph(
          "The editing tool has not retained any saved revision for this edit yet.",
        ),
      );
      return;
    }
    target.append(
      paragraph(
        "Retained revisions are the tool's own saved history. Restoring one is its own operation, not session Undo or Redo.",
      ),
    );
    for (const entry of model.history)
      target.append(
        button(
          `Restore retained revision ${displayScalar(entry.revision, "Unknown revision")} ${displayScalar(entry.createdAt)}`,
          "revision-restore",
          { revision: entry.revision },
          model.blocked,
        ),
      );
  };
  return {
    open(id) {
      if (!alive || !id || !isPhotoVisible()) return;
      if (photoId !== id) failedReferenceUrl = undefined;
      photoId = id;
      current = undefined;
      controlsIdentity = "";
      image.hidden = true;
      image.removeAttribute("src");
      referenceImage.hidden = true;
      referenceImage.removeAttribute("src");
      empty.hidden = false;
      openEditorSurface();
      send({ kind: "editor-open", photoId: id });
    },
    visible: () => alive && isPhotoVisible() && isEditorSurfaceVisible(),
    render(model) {
      if (!alive || photoId !== model.photoId) return;
      current = model;
      const showReference =
        model.cameraReference || (!model.edit && !model.resultSelected);
      element("[data-photo-editor-status]").textContent = model.status;
      const input = model.files.find(
        (file) => file.artifactId === model.selectedInput,
      );
      element("[data-photo-editor-input]").textContent = input
        ? `Input: ${input.relativePath}${input.state === "ready" ? "" : ` · ${input.state}`}`
        : "No default input selected. Select an existing compatible file or prepare input again.";
      const stale = Boolean(
        model.preview &&
          (model.preview.revision !== model.edit?.revision ||
            model.draft.length),
      );
      element("[data-photo-editor-render-status]").textContent = showReference
        ? `Camera reference${model.preview && stale ? " · Edit Preview is out of date" : model.previewPending ? " · Edit Preview is updating" : ""}`
        : model.preview
          ? `${model.comparing ? "Initialized baseline comparison" : "Edit Preview"} · ${stale ? "Previous saved revision · out of date" : model.previewPending ? "Updating" : "Ready"}`
          : model.previewPending
            ? "Rendering Edit Preview"
            : "Edit Preview has not been rendered";
      currentButton.disabled = !model.edit && !model.resultSelected;
      cameraButton.setAttribute("aria-pressed", String(showReference));
      compareButton.disabled = !model.preview?.comparisonUrl || stale;
      compareButton.setAttribute("aria-pressed", String(model.comparing));
      element(".photo-editor-view-actions").hidden =
        !model.edit && !model.resultSelected;
      const actions = element("[data-workspace-editor-actions]");
      actions.hidden = !model.edit;
      actions.replaceChildren(
        paragraph(saveStateRow(model).value),
        button("Undo", "session-undo", {}, !model.sessionUndoCount),
        button("Redo", "session-redo", {}, !model.sessionRedoCount),
        button(
          "Preview",
          "preview",
          {},
          !model.edit?.availability.execute ||
            model.blocked ||
            model.previewPending,
        ),
        button(
          "Export",
          "export",
          {},
          !model.edit?.availability.execute || model.blocked,
        ),
        button("Reload workspace", "refresh", {}, model.loading),
      );
      const facts = element("[data-workspace-facts]");
      facts.replaceChildren();
      for (const row of editorFactRows(model)) {
        const fact = paragraph(`${row.label}: ${row.value}`);
        fact.dataset.fact = row.label;
        facts.append(fact);
      }
      renderRetainedRevisions(model);
      const errors = element("[data-workspace-error]");
      errors.replaceChildren();
      errors.hidden = !model.error && !model.blocked;
      if (model.error) {
        errors.append(paragraph(model.error.message));
        const blockers = model.error.details.blockers;
        if (Array.isArray(blockers))
          for (const blocker of blockers)
            if (isRecord(blocker))
              errors.append(
                paragraph(
                  `${displayScalar(blocker.name, "Unknown blocker")}: ${displayScalar(blocker.action, "Inspect blocker details")}`,
                ),
              );
      }
      if (model.blocked)
        errors.append(
          button("Inspect current saved settings", "inspect-current"),
          button(
            "Reapply retained draft to inspected state",
            "reapply",
            {},
            Boolean(model.uncertain) || !model.conflictInspected,
          ),
          button("Discard draft", "discard", {}, Boolean(model.uncertain)),
        );
      if (model.uncertain)
        errors.append(button("Replay exact request", "replay"));
      const selection = element("[data-workspace-selection]");
      selection.replaceChildren();
      const edits =
        model.workspace?.edits.filter((edit) => edit.tool === "darktable") ??
        [];
      if (edits.length)
        selection.append(
          select(
            "Saved edit",
            edits.map((edit) => ({ value: edit.editId, label: edit.name })),
            model.edit?.editId,
            (editId) => act("edit-select", { editId }),
          ),
        );
      const discovery = model.toolDiscovery;
      const discoveryMatches =
        input &&
        discovery?.input === input.artifactId &&
        discovery.revision === input.revision;
      const toolState = discoveryMatches ? discovery?.state : undefined;
      selection.append(
        paragraph(
          !input
            ? "Select a ready editing input to check tools."
            : input.state !== "ready" || !input.revision
              ? "Editing input is not ready. Select another input or prepare it again."
              : toolState === "ready"
                ? "darktable engine available. Input compatibility and execution capacity are checked when requested."
                : toolState === "unavailable"
                  ? `darktable engine unavailable: ${discovery?.reasons.join("; ") || "No reason reported."}`
                  : toolState === "unknown"
                    ? "darktable availability could not be checked. Retry the check."
                    : "Checking darktable availability…",
        ),
      );
      if (toolState === "unknown" || toolState === "unavailable")
        selection.append(button("Retry tool check", "retry-tool-discovery"));
      selection.append(
        button(
          model.edit ? "Start separate darktable edit" : "Edit with darktable",
          "create",
          {},
          !input ||
            input.state !== "ready" ||
            toolState !== "ready" ||
            model.loading ||
            model.saving ||
            model.draft.length > 0,
        ),
        button(
          "Prepare input again",
          "refresh-input",
          {},
          !model.workspace?.observedSourceRevision,
        ),
      );
      selection.append(
        select(
          "Workspace input",
          model.files
            .filter((file) => file.state === "ready" && file.kind !== "preview")
            .map((file) => ({
              value: file.artifactId,
              label: file.relativePath,
            })),
          model.selectedInput,
          (artifactId) => act("input", { artifactId }),
        ),
      );
      const identity = JSON.stringify([
        model.edit?.editId,
        model.edit?.revision,
        model.blocked,
        model.loading,
        model.catalogs,
        model.draft,
        model.instanceDetails,
        model.maskDetails,
      ]);
      if (identity !== controlsIdentity) {
        controlsIdentity = identity;
        renderControls(model);
      }
      const jobs = element("[data-workspace-jobs]");
      jobs.replaceChildren();
      for (const job of model.jobs) {
        const row = document.createElement("article");
        row.dataset.requestId = job.requestId;
        row.append(
          paragraph(
            `${String(job.operation)} · ${job.status} · saved revision ${String(job.revision)}`,
          ),
          button("Check status", "job-check"),
          button(
            "Cancel",
            "job-cancel",
            { requestId: job.requestId },
            ["succeeded", "failed", "cancelled"].includes(job.status),
          ),
        );
        if (["failed", "cancelled"].includes(job.status))
          row.append(
            button("Retry captured settings", "job-retry", {
              requestId: job.requestId,
            }),
          );
        if (isRecord(job.result) && isRecord(job.result.error))
          row.append(
            paragraph(
              displayScalar(
                job.result.error.message,
                "Processing failed. Inspect job details.",
              ),
            ),
          );
        jobs.append(row);
      }
      const files = element("[data-workspace-file-groups]");
      files.replaceChildren();
      for (const [kind, name] of [
        ["input", "Inputs"],
        ["preview", "Previews"],
        ["output", "Outputs"],
        ["file", "Other files"],
        ["resource", "Resources"],
      ]) {
        const group = document.createElement("section");
        group.append(
          Object.assign(document.createElement("h4"), { textContent: name }),
        );
        for (const file of model.files.filter((file) => file.kind === kind))
          group.append(fileCard(file, model));
        files.append(group);
      }
      if (model.fileCursor) files.append(button("More files", "files-more"));
      const inspection = element("[data-workspace-file-inspection]");
      inspection.replaceChildren();
      if (model.inspectedFile) {
        const file = model.inspectedFile;
        inspection.append(
          paragraph(
            `Reviewed for removal: ${file.relativePath}. Removal preserves the Library Original. Dependent work may refuse removal.`,
          ),
          button("Remove this reviewed file", "remove", {
            artifactId: file.artifactId,
            revision: file.revision,
          }),
        );
        const details = document.createElement("pre");
        details.textContent = JSON.stringify(file, null, 2);
        inspection.append(details);
      }
      element("[data-workspace-details]").textContent = JSON.stringify(
        {
          workspace: model.workspace,
          input: model.edit?.input,
          revision: model.edit?.revision,
          bundleId: model.edit?.bundleId,
          schemaVersion: model.edit?.schemaVersion,
          error: model.error && {
            code: model.error.code,
            effect: model.error.effect,
            details: model.error.details,
          },
        },
        null,
        2,
      );
      if (model.filmElement)
        element("[data-workspace-film]").replaceChildren(model.filmElement);
      if (model.catalogElement)
        element("[data-workspace-catalog]").replaceChildren(
          model.catalogElement,
        );
      image.hidden = true;
      referenceImage.hidden = true;
      empty.hidden = false;
      if (showReference) {
        if (
          model.cameraReferenceUrl &&
          model.cameraReferenceUrl !== failedReferenceUrl
        ) {
          if (referenceImage.getAttribute("src") !== model.cameraReferenceUrl)
            referenceImage.src = model.cameraReferenceUrl;
          referenceImage.hidden = false;
          empty.hidden = true;
        } else {
          referenceImage.removeAttribute("src");
          empty.textContent = "Camera reference unavailable";
        }
      } else if (model.preview) {
        image.src =
          model.comparing && model.preview.comparisonUrl
            ? model.preview.comparisonUrl
            : model.preview.url;
        image.alt = model.comparing
          ? "Initialized tool baseline comparison"
          : "Selected workspace edit preview";
        image.hidden = false;
        empty.hidden = true;
      } else {
        image.removeAttribute("src");
        empty.textContent = model.previewPending
          ? "Rendering Edit Preview"
          : "No Edit Preview is available. Use Preview when processing is ready.";
      }
    },
    presentPreview(url) {
      if (alive && current && !current.cameraReference) {
        image.src = url;
        image.hidden = false;
        empty.hidden = true;
      }
    },
    clearPreview() {
      image.hidden = true;
      image.removeAttribute("src");
      referenceImage.hidden = true;
      referenceImage.removeAttribute("src");
      empty.hidden = false;
    },
    dispose() {
      viewListeners.abort();
      alive = false;
      image.removeAttribute("src");
      referenceImage.removeAttribute("src");
    },
  };
}
