import "./composable-editor.css";
import type { ComposableModuleChoice } from "../model/composable-recipe-draft.js";
import type { ExportArtifact } from "../model/photo-export.js";
import type { ProcessingArtifactRecord } from "../model/processing-artifact.js";
import type {
  EditorViewModel,
  LibraryBrowserIntent,
} from "./library-browser-view.js";

export type EditorExportViewModel = Readonly<{
  target: "development-tiff" | "film-jpeg";
  retainedTarget: "development-tiff" | "film-jpeg" | null;
  state:
    | "idle"
    | "submitting"
    | "outcome-unknown"
    | "queued"
    | "running"
    | "succeeded"
    | "failed"
    | "cancelled";
  note: string;
  artifact: ExportArtifact | null;
  canSubmit: boolean;
  canCancel: boolean;
  canRetry: boolean;
  canDownload: boolean;
  /// The live composable Export request identity, when one is in force.
  processingRequestId: string | null;
  /// The retained immutable Processing Artifact of the composable surface.
  processingArtifact: ProcessingArtifactRecord | null;
}>;

/// One step row of the composable Processing Recipe list.
export type EditorComposableStepView = Readonly<{
  stepId: string;
  module: string;
  inputNote: string;
  current: boolean;
  editing: boolean;
}>;

/// The step editor's own editing state: which step, which module and
/// parameter version, which explicit input binding, and the parameter text
/// as the caller typed it.
export type EditorComposableEditingView = Readonly<{
  stepId: string;
  module: string;
  schemaVersion: string;
  schemaVersions: ReadonlyArray<string>;
  inputChoice: "original" | "artifact";
  artifactChoice: string;
  parametersText: string;
  parametersValid: boolean;
}>;

export type EditorComposableViewModel = Readonly<{
  /// True while a composable recipe is saved or a draft is in progress.
  composing: boolean;
  /// True while this Photo keeps the legacy two-control surface only.
  legacyOnly: boolean;
  unreadable: boolean;
  readPending: boolean;
  note: string;
  modules: ReadonlyArray<ComposableModuleChoice>;
  steps: ReadonlyArray<EditorComposableStepView>;
  currentStepId: string | null;
  dirty: boolean;
  saving: boolean;
  savePending: boolean;
  canAddStep: boolean;
  editing: EditorComposableEditingView | null;
  artifacts: ReadonlyArray<{
    artifactId: string;
    note: string;
    canUse: boolean;
  }>;
}>;

type EditorIntent = Extract<LibraryBrowserIntent, { kind: `editor-${string}` }>;

export function createComposableEditorSurface(
  root: ParentNode,
  send: (intent: EditorIntent) => void,
  signal: AbortSignal,
) {
  let editorPhotoId: string | undefined;
  const listeners = { signal };
  const composablePanel = required<HTMLElement>(
    root,
    "[data-photo-editor-composable]",
  );
  const composableState = required<HTMLElement>(
    root,
    "[data-photo-editor-composable-state]",
  );
  const composableStart = required<HTMLElement>(
    root,
    "[data-photo-editor-composable-start]",
  );
  const composableBody = required<HTMLElement>(
    root,
    "[data-photo-editor-composable-body]",
  );
  const composableSteps = required<HTMLUListElement>(
    root,
    "[data-photo-editor-composable-steps]",
  );
  const composableModule = required<HTMLSelectElement>(
    root,
    "[data-photo-editor-composable-module]",
  );
  const composableAdd = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-composable-add]",
  );
  const composableEditor = required<HTMLElement>(
    root,
    "[data-photo-editor-composable-editor]",
  );
  const composableEditingStep = required<HTMLElement>(
    root,
    "[data-photo-editor-composable-editing-step]",
  );
  const composableEditingModule = required<HTMLElement>(
    root,
    "[data-photo-editor-composable-editing-module]",
  );
  const composableSchema = required<HTMLSelectElement>(
    root,
    "[data-photo-editor-composable-schema]",
  );
  const composableInputOriginal = required<HTMLInputElement>(
    root,
    "[data-photo-editor-composable-input-original]",
  );
  const composableInputArtifact = required<HTMLInputElement>(
    root,
    "[data-photo-editor-composable-input-artifact]",
  );
  const composableArtifactSelect = required<HTMLSelectElement>(
    root,
    "[data-photo-editor-composable-artifact]",
  );
  const composableParameters = required<HTMLTextAreaElement>(
    root,
    "[data-photo-editor-composable-parameters]",
  );
  const composableParametersNote = required<HTMLElement>(
    root,
    "[data-photo-editor-composable-parameters-note]",
  );
  const composableSave = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-composable-save]",
  );
  const composableDiscard = required<HTMLButtonElement>(
    root,
    "[data-photo-editor-composable-discard]",
  );
  const composableArtifactsPanel = required<HTMLElement>(
    root,
    "[data-photo-editor-composable-artifacts]",
  );
  const composableArtifactList = required<HTMLUListElement>(
    root,
    "[data-photo-editor-composable-artifact-list]",
  );
  const composableArtifactId = required<HTMLInputElement>(
    root,
    "[data-photo-editor-composable-artifact-id]",
  );
  composablePanel.addEventListener(
    "click",
    (event) => {
      if (!editorPhotoId) return;
      const target = event.target;
      if (!(target instanceof Element)) return;
      const composeButton = target.closest("[data-photo-editor-compose]");
      if (composeButton) {
        send({ kind: "editor-compose", photoId: editorPhotoId });
        return;
      }
      const fetchButton = target.closest(
        "[data-photo-editor-composable-artifact-fetch]",
      );
      if (fetchButton) {
        const artifactId = composableArtifactId.value.trim();
        if (artifactId)
          send({
            kind: "editor-composable-artifact-fetch",
            photoId: editorPhotoId,
            artifactId,
          });
        return;
      }
      const saveButton = target.closest("[data-photo-editor-composable-save]");
      if (saveButton && !composableSave.disabled) {
        send({ kind: "editor-composable-save", photoId: editorPhotoId });
        return;
      }
      const discardButton = target.closest(
        "[data-photo-editor-composable-discard]",
      );
      if (discardButton && !composableDiscard.disabled) {
        send({ kind: "editor-composable-discard", photoId: editorPhotoId });
        return;
      }
      const addButton = target.closest("[data-photo-editor-composable-add]");
      if (addButton && !composableAdd.disabled && composableModule.value) {
        send({
          kind: "editor-composable-add",
          photoId: editorPhotoId,
          module: composableModule.value,
        });
        return;
      }
      const stepButton = target.closest<HTMLButtonElement>(
        "button[data-composable-action]",
      );
      if (stepButton) {
        const stepId = stepButton.closest<HTMLLIElement>("li")?.dataset.stepId;
        if (!stepId) return;
        const action = stepButton.dataset.composableAction;
        if (action === "select")
          send({
            kind: "editor-composable-select",
            photoId: editorPhotoId,
            stepId,
          });
        else if (action === "edit")
          send({
            kind: "editor-composable-edit",
            photoId: editorPhotoId,
            stepId,
          });
        else if (action === "remove")
          send({
            kind: "editor-composable-remove",
            photoId: editorPhotoId,
            stepId,
          });
        return;
      }
      const artifactButton = target.closest<HTMLButtonElement>(
        "button[data-artifact-action]",
      );
      if (artifactButton) {
        const artifactId =
          artifactButton.closest<HTMLLIElement>("li")?.dataset.artifactId;
        if (!artifactId) return;
        if (artifactButton.dataset.artifactAction === "download")
          send({
            kind: "editor-artifact-download",
            photoId: editorPhotoId,
            artifactId,
          });
        else if (!artifactButton.disabled)
          send({
            kind: "editor-artifact-use",
            photoId: editorPhotoId,
            artifactId,
          });
      }
    },
    { signal: listeners.signal },
  );
  composableSchema.addEventListener(
    "change",
    () => {
      if (editorPhotoId && composableSchema.value)
        send({
          kind: "editor-composable-schema",
          photoId: editorPhotoId,
          schemaVersion: composableSchema.value,
        });
    },
    { signal: listeners.signal },
  );
  composableInputOriginal.addEventListener(
    "change",
    () => {
      if (editorPhotoId && composableInputOriginal.checked)
        send({
          kind: "editor-composable-input",
          photoId: editorPhotoId,
          choice: "original",
        });
    },
    { signal: listeners.signal },
  );
  composableInputArtifact.addEventListener(
    "change",
    () => {
      if (editorPhotoId && composableInputArtifact.checked)
        send({
          kind: "editor-composable-input",
          photoId: editorPhotoId,
          choice: "artifact",
          ...(composableArtifactSelect.value
            ? { artifactId: composableArtifactSelect.value }
            : {}),
        });
    },
    { signal: listeners.signal },
  );
  composableArtifactSelect.addEventListener(
    "change",
    () => {
      if (
        editorPhotoId &&
        composableArtifactSelect.value &&
        composableInputArtifact.checked
      )
        send({
          kind: "editor-composable-input",
          photoId: editorPhotoId,
          choice: "artifact",
          artifactId: composableArtifactSelect.value,
        });
    },
    { signal: listeners.signal },
  );
  composableParameters.addEventListener(
    "input",
    () => {
      if (editorPhotoId && !composableParameters.disabled)
        send({
          kind: "editor-composable-parameters",
          photoId: editorPhotoId,
          text: composableParameters.value,
        });
    },
    { signal: listeners.signal },
  );
  const render = (model: EditorViewModel): void => {
    editorPhotoId = model.photoId;
    const composable = model.composable;
    composableState.textContent = composable.readPending
      ? "Reading the Processing Recipe…"
      : composable.unreadable
        ? "The Processing Recipe could not be read, so no processing result is shown. Reload to check again."
        : composable.note;
    composableState.hidden = !composableState.textContent;
    // The legacy two-control surface offers composing as its one composable
    // gesture; a saved recipe or draft presents the full step surface.
    composableStart.hidden = !composable.legacyOnly;
    composableBody.hidden = !composable.composing;
    composableSteps.replaceChildren();
    if (composable.composing && composable.steps.length === 0) {
      const empty = document.createElement("li");
      empty.className = "photo-editor-composable-empty";
      empty.textContent = "No Processing Steps yet. Add one below.";
      composableSteps.append(empty);
    }
    for (const step of composable.steps) {
      const item = document.createElement("li");
      item.className = "photo-editor-composable-step";
      item.dataset.stepId = step.stepId;
      const select = document.createElement("button");
      select.type = "button";
      select.className = "photo-editor-composable-step-name";
      select.dataset.composableAction = "select";
      select.setAttribute("aria-pressed", String(step.current));
      select.textContent = `${step.stepId} — ${step.module} (${step.inputNote})`;
      const edit = document.createElement("button");
      edit.type = "button";
      edit.className = "quiet";
      edit.dataset.composableAction = "edit";
      edit.setAttribute("aria-pressed", String(step.editing));
      edit.textContent = step.editing ? "Editing" : "Edit";
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "quiet";
      remove.dataset.composableAction = "remove";
      remove.textContent = "Remove";
      item.append(select, edit, remove);
      composableSteps.append(item);
    }
    // Module discovery is the only source of addable steps; an unavailable
    // module is named as unavailable, never silently omitted.
    const selectedModule = composableModule.value;
    composableModule.replaceChildren();
    for (const choice of composable.modules) {
      const option = document.createElement("option");
      option.value = choice.name;
      option.textContent = choice.ready
        ? choice.name
        : `${choice.name} (unavailable: ${choice.refusalNote})`;
      composableModule.append(option);
    }
    if (
      selectedModule &&
      composable.modules.some((choice) => choice.name === selectedModule)
    )
      composableModule.value = selectedModule;
    composableAdd.disabled = !composable.canAddStep;
    const editing = composable.editing;
    composableEditor.hidden = !editing;
    if (editing) {
      composableEditingStep.textContent = editing.stepId;
      composableEditingModule.textContent = editing.module;
      const selectedSchema = composableSchema.value;
      composableSchema.replaceChildren();
      for (const version of editing.schemaVersions) {
        const option = document.createElement("option");
        option.value = version;
        option.textContent = version;
        composableSchema.append(option);
      }
      composableSchema.value = editing.schemaVersions.includes(
        editing.schemaVersion,
      )
        ? editing.schemaVersion
        : selectedSchema && editing.schemaVersions.includes(selectedSchema)
          ? selectedSchema
          : (editing.schemaVersions[0] ?? "");
      composableSchema.disabled =
        editing.schemaVersions.length === 0 || composable.saving;
      composableInputOriginal.checked = editing.inputChoice === "original";
      composableInputArtifact.checked = editing.inputChoice === "artifact";
      const selectedArtifact = composableArtifactSelect.value;
      composableArtifactSelect.replaceChildren();
      if (composable.artifacts.length === 0) {
        const option = document.createElement("option");
        option.value = "";
        option.textContent = "No retained Processing Artifacts yet";
        composableArtifactSelect.append(option);
      } else {
        for (const artifact of composable.artifacts) {
          const option = document.createElement("option");
          option.value = artifact.artifactId;
          option.textContent = artifact.artifactId;
          composableArtifactSelect.append(option);
        }
        composableArtifactSelect.value =
          editing.artifactChoice ||
          (composable.artifacts.some(
            (artifact) => artifact.artifactId === selectedArtifact,
          )
            ? selectedArtifact
            : (composable.artifacts[0]?.artifactId ?? ""));
      }
      composableArtifactSelect.disabled =
        composable.artifacts.length === 0 || composable.saving;
      // The parameter text is the caller's own typing; a focused textarea is
      // never rewritten under the caret.
      if (document.activeElement !== composableParameters)
        composableParameters.value = editing.parametersText;
      composableParameters.disabled = composable.saving;
      composableParametersNote.textContent = editing.parametersValid
        ? ""
        : "The module parameters are not one JSON object, so the recipe cannot be saved.";
      composableParametersNote.hidden = editing.parametersValid;
    }
    composableSave.disabled = composable.saving;
    composableSave.textContent = composable.saving
      ? "Saving…"
      : "Save Processing Recipe";
    composableDiscard.hidden = !composable.dirty;
    composableDiscard.disabled = composable.saving;
    composableArtifactsPanel.hidden = !composable.composing;
    composableArtifactList.replaceChildren();
    if (composable.composing && composable.artifacts.length === 0) {
      const empty = document.createElement("li");
      empty.className = "photo-editor-composable-empty";
      empty.textContent =
        "No retained Processing Artifacts yet. Read one by its identity below.";
      composableArtifactList.append(empty);
    }
    for (const artifact of composable.artifacts) {
      const item = document.createElement("li");
      item.className = "photo-editor-composable-artifact";
      item.dataset.artifactId = artifact.artifactId;
      const note = document.createElement("span");
      note.className = "photo-editor-composable-artifact-note";
      note.textContent = artifact.note;
      const download = document.createElement("button");
      download.type = "button";
      download.className = "quiet";
      download.dataset.artifactAction = "download";
      download.textContent = "Download";
      const use = document.createElement("button");
      use.type = "button";
      use.className = "quiet";
      use.dataset.artifactAction = "use";
      use.textContent = "Use as step input";
      use.disabled = !artifact.canUse;
      item.append(note, download, use);
      composableArtifactList.append(item);
    }
  };
  return { render };
}

function required<T extends Element>(root: ParentNode, selector: string): T {
  const value = root.querySelector<T>(selector);
  if (!value) throw new Error(`Missing ${selector}`);
  return value;
}
