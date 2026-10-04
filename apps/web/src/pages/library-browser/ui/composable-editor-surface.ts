import "./composable-editor.css";
import type { ComposableModuleChoice } from "../model/composable-recipe-draft.js";
import type { ProcessingArtifactRecord } from "../model/processing-artifact.js";
import {
  moduleParameterControls,
  replaceModuleParameter,
} from "../model/module-parameter-controls.js";
import type {
  EditorViewModel,
  LibraryBrowserIntent,
} from "./library-browser-view.js";

export type EditorExportViewModel = Readonly<{
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
  canSubmit: boolean;
  canCancel: boolean;
  canRetry: boolean;
  canDownload: boolean;
  processingRequestId: string | null;
  processingArtifact: ProcessingArtifactRecord | null;
  processingExports: ReadonlyArray<
    Readonly<{
      requestId: string;
      state: string;
      module: string;
      stepId: string;
      note: string;
      canCancel: boolean;
      canRetry: boolean;
    }>
  >;
  processingArtifacts: ReadonlyArray<
    ProcessingArtifactRecord &
      Readonly<{
        isExpired: boolean;
        isStale: boolean;
        canDownload: boolean;
        note: string;
      }>
  >;
  historicalExports: ReadonlyArray<
    Readonly<{
      artifactId: string;
      filename: string;
      type: string;
      canDownload: boolean;
      note: string;
    }>
  >;
}>;
export type EditorComposableStepView = Readonly<{
  stepId: string;
  module: string;
  inputNote: string;
  current: boolean;
  editing: boolean;
}>;
export type EditorComposableEditingView = Readonly<{
  stepId: string;
  module: string;
  schemaVersion: string;
  schemaVersions: ReadonlyArray<string>;
  inputChoice: "original" | "artifact";
  artifactChoice: string;
  parametersText: string;
  parametersValid: boolean;
  parameterSchema?: unknown;
  automaticAdjustments: ReadonlyArray<{
    operation: string;
    label: string;
    multiPriority: number;
    instruction: Record<string, unknown>;
  }>;
}>;
export type EditorComposableViewModel = Readonly<{
  composing: boolean;
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
  const element = <T extends Element>(selector: string): T => {
    const node = root.querySelector<T>(selector);
    if (!node) throw new Error(`Missing editor element ${selector}`);
    return node;
  };
  const panel = element<HTMLElement>("[data-photo-editor-composable]");
  const state = element<HTMLElement>("[data-photo-editor-composable-state]");
  const steps = element<HTMLElement>("[data-photo-editor-composable-steps]");
  const moduleSelect = element<HTMLSelectElement>(
    "[data-photo-editor-composable-module]",
  );
  const newStepInput = element<HTMLSelectElement>(
    "[data-photo-editor-new-step-input]",
  );
  const add = element<HTMLButtonElement>("[data-photo-editor-composable-add]");
  const editor = element<HTMLElement>("[data-photo-editor-composable-editor]");
  const controls = element<HTMLElement>("[data-photo-editor-module-controls]");
  const original = element<HTMLInputElement>(
    "[data-photo-editor-composable-input-original]",
  );
  const artifactInput = element<HTMLInputElement>(
    "[data-photo-editor-composable-input-artifact]",
  );
  const artifactSelect = element<HTMLSelectElement>(
    "[data-photo-editor-composable-artifact]",
  );
  const artifactList = element<HTMLElement>(
    "[data-photo-editor-composable-artifact-list]",
  );
  const exportList = element<HTMLElement>("[data-photo-editor-export-list]");
  let model: EditorViewModel | undefined;
  let controlsIdentity = "";
  let controlsParametersText = "";
  const button = (
    label: string,
    action: () => void,
    disabled = false,
  ): HTMLButtonElement => {
    const node = document.createElement("button");
    node.type = "button";
    node.className = "quiet";
    node.textContent = label;
    node.disabled = disabled;
    node.addEventListener("click", action, { signal });
    return node;
  };
  add.addEventListener(
    "click",
    () => {
      if (model && moduleSelect.value)
        send({
          kind: "editor-composable-add",
          photoId: model.photoId,
          module: moduleSelect.value,
          ...(newStepInput.value.startsWith("artifact:")
            ? { artifactId: newStepInput.value.slice(9) }
            : {}),
        });
    },
    { signal },
  );
  panel.addEventListener(
    "click",
    (event) => {
      if (
        !model ||
        !(event.target instanceof Element) ||
        !event.target.closest("[data-photo-editor-composable-artifact-fetch]")
      )
        return;
      const artifactId = element<HTMLInputElement>(
        "[data-photo-editor-composable-artifact-id]",
      ).value.trim();
      if (artifactId)
        send({
          kind: "editor-composable-artifact-fetch",
          photoId: model.photoId,
          artifactId,
        });
    },
    { signal },
  );
  const bindInput = (): void => {
    if (model)
      send({
        kind: "editor-composable-input",
        photoId: model.photoId,
        choice: original.checked ? "original" : "artifact",
        ...(artifactSelect.value ? { artifactId: artifactSelect.value } : {}),
      });
  };
  original.addEventListener("change", bindInput, { signal });
  artifactInput.addEventListener("change", bindInput, { signal });
  artifactSelect.addEventListener("change", bindInput, { signal });
  const render = (next: EditorViewModel): void => {
    model = next;
    const composable = next.composable;
    element<HTMLButtonElement>(
      "[data-photo-editor-composable-artifact-fetch]",
    ).disabled = next.loading;
    const blocked =
      next.loading || next.mutationsBlocked || Boolean(next.conflict);
    original.disabled = blocked;
    artifactInput.disabled = blocked;
    state.textContent = composable.readPending
      ? "Reading the Processing Recipe…"
      : composable.note;
    steps.replaceChildren();
    for (const step of composable.steps) {
      const item = document.createElement("li");
      item.dataset.stepId = step.stepId;
      const select = button(
        `${step.stepId} — ${step.module} (${step.inputNote})`,
        () =>
          send({
            kind: "editor-composable-select",
            photoId: next.photoId,
            stepId: step.stepId,
          }),
      );
      select.disabled = blocked;
      select.setAttribute("aria-pressed", String(step.current));
      item.append(
        select,
        button(
          "Edit",
          () =>
            send({
              kind: "editor-composable-edit",
              photoId: next.photoId,
              stepId: step.stepId,
            }),
          blocked,
        ),
        button(
          "Remove",
          () =>
            send({
              kind: "editor-composable-remove",
              photoId: next.photoId,
              stepId: step.stepId,
            }),
          blocked,
        ),
      );
      steps.append(item);
    }
    const selectedModule = moduleSelect.value;
    moduleSelect.replaceChildren();
    for (const module of composable.modules) {
      const option = document.createElement("option");
      option.value = module.name;
      option.textContent = module.ready
        ? module.name
        : `${module.name} (unavailable: ${module.refusalNote})`;
      option.disabled = !module.ready;
      moduleSelect.append(option);
    }
    if (composable.modules.some((module) => module.name === selectedModule))
      moduleSelect.value = selectedModule;
    add.disabled = blocked || !composable.canAddStep;
    moduleSelect.disabled = blocked;
    const selectedInput = newStepInput.value;
    newStepInput.replaceChildren();
    const originalOption = document.createElement("option");
    originalOption.value = "original";
    originalOption.textContent = "Original of this Photo";
    newStepInput.append(originalOption);
    for (const artifact of next.export.processingArtifacts.filter(
      (artifact) => !artifact.isExpired,
    )) {
      const option = document.createElement("option");
      option.value = `artifact:${artifact.artifactId}`;
      option.textContent = artifact.filename;
      newStepInput.append(option);
    }
    if (
      Array.from(newStepInput.options).some(
        (option) => option.value === selectedInput,
      )
    )
      newStepInput.value = selectedInput;
    newStepInput.disabled = blocked;
    const editing = composable.editing;
    editor.hidden = !editing;
    if (editing) {
      element<HTMLElement>(
        "[data-photo-editor-composable-editing-step]",
      ).textContent = editing.stepId;
      element<HTMLElement>(
        "[data-photo-editor-composable-editing-module]",
      ).textContent = editing.module;
      element<HTMLElement>(
        "[data-photo-editor-composable-schema]",
      ).textContent = editing.schemaVersion;
      original.checked = editing.inputChoice === "original";
      artifactInput.checked = !original.checked;
      artifactSelect.replaceChildren();
      for (const artifact of composable.artifacts) {
        const option = document.createElement("option");
        option.value = artifact.artifactId;
        option.textContent = artifact.note;
        artifactSelect.append(option);
      }
      artifactSelect.value = editing.artifactChoice;
      artifactSelect.disabled = blocked || composable.artifacts.length === 0;
      let tree: unknown;
      try {
        tree = JSON.parse(editing.parametersText);
      } catch {
        tree = null;
      }
      const automaticButtons = Array.from(
        controls.querySelectorAll<HTMLElement>("[data-automatic-adjustment]"),
      );
      for (const old of automaticButtons) old.remove();
      for (const adjustment of editing.automaticAdjustments) {
        const action = button(
          adjustment.label,
          () =>
            send({
              kind: "editor-composable-automatic",
              photoId: next.photoId,
              stepId: editing.stepId,
              operation: adjustment.operation,
              multiPriority: adjustment.multiPriority,
              instruction: adjustment.instruction,
            }),
          blocked || editing.inputChoice !== "original",
        );
        action.dataset.automaticAdjustment = adjustment.operation;
        controls.append(action);
      }
      const parameterControls = moduleParameterControls(
        editing.parameterSchema,
        tree,
      );
      const identity = JSON.stringify([
        next.photoId,
        editing.stepId,
        editing.parameterSchema,
        parameterControls.map((control) => [
          control.path,
          control.kind,
          control.fixed,
          control.choices,
          control.minimum,
          control.maximum,
          control.step,
          control.defaultValue,
        ]),
      ]);
      // Status and value updates retain the controls' focus and pointer owner.
      if (identity !== controlsIdentity) {
        controlsIdentity = identity;
        controlsParametersText = "";
        controls.replaceChildren();
        for (const control of parameterControls) {
          const field = document.createElement("div");
          field.className = "photo-editor-field";
          const label = document.createElement("label");
          label.textContent = control.label || "Module parameters";
          if (control.fixed) {
            const detail = document.createElement("details");
            const summary = document.createElement("summary");
            summary.textContent = `${label.textContent} — fixed`;
            const value = document.createElement("pre");
            value.textContent = JSON.stringify(control.value, null, 2);
            value.id = `module-fixed-${control.path.map(String).join("-")}`;
            detail.append(summary, value);
            field.append(detail);
          } else {
            const input =
              control.kind === "enum"
                ? document.createElement("select")
                : document.createElement("input");
            const id = `module-control-${control.path.map(String).join("-")}`;
            input.id = id;
            label.htmlFor = id;
            input.disabled = blocked;
            if (input instanceof HTMLSelectElement) {
              for (const choice of control.choices ?? []) {
                const option = document.createElement("option");
                option.value = String(choice);
                option.textContent = String(choice);
                input.append(option);
              }
              input.value = String(control.value);
            } else if (control.kind === "boolean") {
              input.type = "checkbox";
              input.checked = control.value === true;
            } else {
              input.type = "number";
              input.required = true;
              input.value = String(control.value);
              input.step = String(control.step ?? 0.01);
              if (control.minimum !== undefined)
                input.min = String(control.minimum);
              if (control.maximum !== undefined)
                input.max = String(control.maximum);
            }
            input.addEventListener(
              "change",
              () => {
                if (
                  !model?.composable.editing ||
                  model.composable.editing.stepId !== editing.stepId
                )
                  return;
                if (input instanceof HTMLInputElement && !input.checkValidity())
                  return;
                const value =
                  control.kind === "boolean" &&
                  input instanceof HTMLInputElement
                    ? input.checked
                    : control.kind === "number"
                      ? Number(input.value)
                      : control.choices?.find(
                          (choice) => String(choice) === input.value,
                        );
                if (control.kind === "number" && !Number.isFinite(value))
                  return;
                let current: unknown;
                try {
                  current = JSON.parse(model.composable.editing.parametersText);
                } catch {
                  return;
                }
                send({
                  kind: "editor-composable-parameters",
                  photoId: model.photoId,
                  text: JSON.stringify(
                    replaceModuleParameter(current, control.path, value),
                  ),
                });
              },
              { signal },
            );
            field.append(label, input);
            if (control.defaultValue !== undefined)
              field.append(
                button(
                  "Reset",
                  () => {
                    if (!model?.composable.editing || model.mutationsBlocked)
                      return;
                    let current: unknown;
                    try {
                      current = JSON.parse(
                        model.composable.editing.parametersText,
                      );
                    } catch {
                      return;
                    }
                    send({
                      kind: "editor-composable-parameters",
                      photoId: model.photoId,
                      text: JSON.stringify(
                        replaceModuleParameter(
                          current,
                          control.path,
                          control.defaultValue,
                        ),
                      ),
                    });
                  },
                  blocked,
                ),
              );
          }
          controls.append(field);
        }
        element<HTMLElement>(
          "[data-photo-editor-composable-parameters-note]",
        ).textContent =
          "Only qualified controls are editable. Fixed groups and unsupported saved intent are retained without substitution.";
      }
      if (editing.parametersText !== controlsParametersText) {
        controlsParametersText = editing.parametersText;
        for (const control of parameterControls) {
          const key = control.path.map(String).join("-");
          if (control.fixed) {
            const value = controls.querySelector<HTMLElement>(
              `#${CSS.escape(`module-fixed-${key}`)}`,
            );
            if (value)
              value.textContent = JSON.stringify(control.value, null, 2);
            continue;
          }
          const input = controls.querySelector<
            HTMLInputElement | HTMLSelectElement
          >(`#${CSS.escape(`module-control-${key}`)}`);
          if (input instanceof HTMLInputElement && control.kind === "boolean")
            input.checked = control.value === true;
          else if (input) input.value = String(control.value);
        }
      }
      if (
        !controls.querySelector("[data-automatic-adjustment]") &&
        editing.automaticAdjustments.length
      ) {
        for (const adjustment of editing.automaticAdjustments) {
          const action = button(
            adjustment.label,
            () =>
              send({
                kind: "editor-composable-automatic",
                photoId: next.photoId,
                stepId: editing.stepId,
                operation: adjustment.operation,
                multiPriority: adjustment.multiPriority,
                instruction: adjustment.instruction,
              }),
            blocked || editing.inputChoice !== "original",
          );
          action.dataset.automaticAdjustment = adjustment.operation;
          controls.append(action);
        }
      }
    } else {
      controlsIdentity = "";
      controlsParametersText = "";
      controls.replaceChildren();
    }
    controls
      .querySelectorAll<
        HTMLInputElement | HTMLSelectElement | HTMLButtonElement
      >("input, select, button")
      .forEach(
        (input: HTMLInputElement | HTMLSelectElement | HTMLButtonElement) => {
          input.disabled = blocked;
        },
      );
    artifactList.replaceChildren();
    for (const artifact of next.export.processingArtifacts) {
      const item = document.createElement("li");
      item.dataset.artifactId = artifact.artifactId;
      const note = document.createElement("span");
      note.textContent = artifact.note;
      const canUse = !blocked && !artifact.isExpired && Boolean(editing);
      item.append(
        note,
        button(
          "Download",
          () =>
            send({
              kind: "editor-artifact-download",
              photoId: next.photoId,
              artifactId: artifact.artifactId,
            }),
          !artifact.canDownload,
        ),
        button(
          "Use as step input",
          () =>
            send({
              kind: "editor-artifact-use",
              photoId: next.photoId,
              artifactId: artifact.artifactId,
            }),
          !canUse,
        ),
      );
      artifactList.append(item);
    }
    exportList.replaceChildren();
    for (const work of next.export.processingExports) {
      const item = document.createElement("li");
      item.dataset.requestId = work.requestId;
      const note = document.createElement("span");
      note.textContent = `${work.module} / ${work.stepId}: ${work.note}`;
      item.append(
        note,
        button("Check status", () =>
          send({
            kind: "editor-processing-export-check",
            photoId: next.photoId,
            requestId: work.requestId,
          }),
        ),
      );
      if (work.canCancel)
        item.append(
          button("Cancel", () =>
            send({
              kind: "editor-export-cancel",
              photoId: next.photoId,
              requestId: work.requestId,
            }),
          ),
        );
      if (work.canRetry)
        item.append(
          button("Retry captured settings", () =>
            send({
              kind: "editor-export-retry",
              photoId: next.photoId,
              requestId: work.requestId,
            }),
          ),
        );
      exportList.append(item);
    }
    for (const historical of next.export.historicalExports) {
      const item = document.createElement("li");
      item.dataset.historicalExportId = historical.artifactId;
      const note = document.createElement("span");
      note.textContent = historical.note;
      item.append(
        note,
        button(
          "Download historical output",
          () =>
            send({
              kind: "editor-historical-download",
              photoId: next.photoId,
              exportId: historical.artifactId,
            }),
          !historical.canDownload,
        ),
      );
      exportList.append(item);
    }
  };
  return { render };
}
