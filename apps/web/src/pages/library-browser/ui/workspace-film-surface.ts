import type { BrowserFetch } from "../api/browser-fetch.js";
import {
  fetchWorkspaceFilmModules,
  readWorkspaceFilm,
  type WorkspaceFilmFile,
} from "../api/workspace-film.js";
import {
  canContinueFilm,
  createWorkspaceFilmOwner,
} from "../model/workspace-film.js";
import { createSemanticControls } from "./semantic-controls.js";
import { isRecord } from "../api/guards.js";
import type { ProcessingModuleDescription } from "../api/processing-modules.js";

export interface WorkspaceFilmSurface {
  readonly element: HTMLElement;
  open(file: WorkspaceFilmFile): Promise<void>;
  dispose(): void;
}

export function createWorkspaceFilmSurface(
  fetcher: BrowserFetch,
  photoId: string,
  onFilesChanged: () => void,
): WorkspaceFilmSurface {
  const element = document.createElement("section");
  element.hidden = true;
  element.className = "workspace-film";
  element.setAttribute("aria-label", "Film processing");
  const title = document.createElement("h3");
  title.textContent = "Film";
  const status = document.createElement("p");
  status.setAttribute("role", "status");
  const controls = document.createElement("fieldset");
  const edits = document.createElement("div");
  const preview = document.createElement("img");
  preview.alt = "Film preview of selected saved settings";
  preview.hidden = true;
  const stale = document.createElement("p");
  const actions = document.createElement("div");
  element.append(title, status, edits, controls, preview, stale, actions);
  const lifetime = new AbortController();
  let module: ProcessingModuleDescription | undefined;
  let objectUrl: string | undefined,
    renderedTree: unknown,
    generation = 0,
    previewGeneration = 0;
  let poll: ReturnType<typeof setTimeout> | undefined;
  const owner = createWorkspaceFilmOwner(fetcher, photoId, render);
  const buttons: HTMLButtonElement[] = [];
  for (const [label, action] of [
    ["Save Film settings", () => owner.save()],
    [
      "Preview",
      async () => {
        const stamp = generation,
          previewStamp = ++previewGeneration;
        const image = await owner.preview();
        if (
          !image ||
          lifetime.signal.aborted ||
          stamp !== generation ||
          previewStamp !== previewGeneration
        )
          return;
        if (objectUrl) URL.revokeObjectURL(objectUrl);
        objectUrl = URL.createObjectURL(image);
        preview.src = objectUrl;
        preview.hidden = false;
        stale.textContent = "";
        onFilesChanged();
      },
    ],
    [
      "Export",
      async () => {
        await owner.export();
        schedule();
      },
    ],
    [
      "Reconcile request",
      async () => {
        await owner.reconcile();
        schedule();
      },
    ],
    [
      "Check Export",
      async () => {
        await owner.checkExport();
        onFilesChanged();
        schedule();
      },
    ],
    [
      "Cancel Export",
      async () => {
        await owner.checkExport("cancel");
        onFilesChanged();
        schedule();
      },
    ],
    [
      "Retry Export",
      async () => {
        await owner.checkExport("retry");
        schedule();
      },
    ],
    ["Inspect current Film state", () => owner.inspectConflict()],
    ["Reapply Film draft", () => owner.reapply()],
  ] as const) {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = label;
    button.addEventListener(
      "click",
      () => {
        void action();
      },
      { signal: lifetime.signal },
    );
    actions.append(button);
    buttons.push(button);
  }
  function schedule(): void {
    clearTimeout(poll);
    const state = owner.view().receipt?.state;
    if (
      !lifetime.signal.aborted &&
      (state === "accepted" || state === "executing")
    )
      poll = setTimeout(() => {
        void owner.checkExport().then(() => {
          onFilesChanged();
          schedule();
        });
      }, 1500);
    else if (state === "succeeded") onFilesChanged();
  }
  function render(): void {
    const view = owner.view();
    status.textContent = view.note;
    if (
      module &&
      view.step &&
      renderedTree !== view.step.parameters.tree &&
      isRecord(module.parameterSchema)
    ) {
      renderedTree = view.step.parameters.tree;
      controls.replaceChildren(
        createSemanticControls(
          module.parameterSchema,
          renderedTree,
          (value) => {
            previewGeneration += 1;
            owner.setParameters(value);
          },
          { label: "Film settings" },
        ),
      );
    }
    controls.disabled = view.busy || view.uncertain;
    buttons[0]!.disabled =
      view.busy || view.uncertain || view.conflict || !view.dirty;
    buttons[1]!.disabled =
      view.busy || view.uncertain || view.conflict || !view.step;
    buttons[2]!.disabled =
      view.busy || view.uncertain || view.conflict || !view.step;
    buttons[3]!.hidden = !view.uncertain;
    buttons[3]!.disabled = view.busy;
    const state = view.receipt?.state;
    buttons[4]!.hidden = !view.receipt;
    buttons[5]!.hidden = state !== "accepted" && state !== "executing";
    buttons[6]!.hidden = state !== "failed" && state !== "cancelled";
    for (const button of buttons.slice(4))
      button.disabled = view.busy || view.uncertain;
    buttons[7]!.hidden = !view.conflict;
    buttons[8]!.hidden = !view.conflict;
    if (objectUrl && (view.dirty || view.busy))
      stale.textContent = "Previous Film preview — settings may have changed.";
  }
  return {
    element,
    async open(file: WorkspaceFilmFile): Promise<void> {
      const active = owner.view();
      if (active.busy || active.uncertain || active.dirty)
        throw new Error(
          "Save or reconcile the current Film draft before selecting another input.",
        );
      const stamp = ++generation;
      previewGeneration += 1;
      element.hidden = false;
      clearTimeout(poll);
      if (objectUrl) {
        URL.revokeObjectURL(objectUrl);
        objectUrl = undefined;
      }
      preview.hidden = true;
      stale.textContent = "";
      controls.replaceChildren();
      renderedTree = undefined;
      edits.replaceChildren();
      status.textContent = "Checking Film compatibility…";
      actions.hidden = true;
      try {
        const modules = await fetchWorkspaceFilmModules(
          fetcher,
          lifetime.signal,
        );
        if (lifetime.signal.aborted || stamp !== generation) return;
        module = modules?.find((value) =>
          canContinueFilm(file.imageContract, value),
        );
        if (!module) {
          status.textContent =
            "No available Film tool accepts this image's verified color, precision and geometry. The file remains downloadable.";
          return;
        }
        const current = await readWorkspaceFilm(
          fetcher,
          photoId,
          lifetime.signal,
        );
        if (lifetime.signal.aborted || stamp !== generation) return;
        const matches =
          current.recipe?.steps.filter(
            (step) =>
              step.module === module!.id.name &&
              step.input.kind === "workspace-artifact" &&
              step.input.workspaceId === file.workspaceId &&
              step.input.artifactId === file.artifactId &&
              step.input.revision === file.revision,
          ) ?? [];
        const selectedModule = module;
        if (matches.length > 1) {
          status.textContent = "Choose a saved Film edit for this image.";
          for (const step of matches) {
            const choose = document.createElement("button");
            choose.type = "button";
            choose.textContent = `Open saved Film edit ${step.stepId}`;
            choose.addEventListener(
              "click",
              () => {
                void owner
                  .open(file, selectedModule, step.stepId)
                  .then(() => {
                    if (stamp === generation) actions.hidden = false;
                  })
                  .catch((error) => {
                    status.textContent =
                      error instanceof Error
                        ? error.message
                        : "Film edit could not be opened.";
                  });
              },
              { signal: lifetime.signal },
            );
            edits.append(choose);
          }
        } else {
          await owner.open(file, selectedModule);
          if (stamp === generation) actions.hidden = false;
        }
      } catch (error) {
        if (!lifetime.signal.aborted && stamp === generation)
          status.textContent =
            error instanceof Error
              ? error.message
              : "Film could not be opened.";
      }
    },
    dispose(): void {
      generation += 1;
      previewGeneration += 1;
      clearTimeout(poll);
      lifetime.abort();
      owner.dispose();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    },
  };
}
