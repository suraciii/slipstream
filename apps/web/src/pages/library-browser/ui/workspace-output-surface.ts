import type { WorkspaceOutputsView } from "../model/workspace-output-controller.js";
import type { LibraryBrowserIntent } from "./library-browser-view.js";

export function createWorkspaceOutputSurface(
  root: ParentNode,
  send: (
    intent: Extract<LibraryBrowserIntent, { kind: `editor-${string}` }>,
  ) => void,
  signal: AbortSignal,
) {
  const card = root.querySelector<HTMLElement>('[data-editor-output="xmp"]');
  let photoId = "";
  card?.addEventListener(
    "click",
    (event) => {
      const button =
        event.target instanceof Element
          ? event.target.closest<HTMLButtonElement>("[data-output-action]")
          : null;
      if (!button || button.disabled || !photoId) return;
      if (button.dataset.outputAction === "submit")
        send({ kind: "editor-xmp-submit", photoId });
      if (button.dataset.outputAction === "download")
        send({ kind: "editor-xmp-download", photoId });
    },
    { signal },
  );
  return {
    render(
      id: string,
      model: WorkspaceOutputsView,
      waiting: boolean,
      loading: boolean,
    ) {
      photoId = id;
      if (!card) return;
      const state = card.querySelector<HTMLElement>("[data-output-state]");
      const details = card.querySelector<HTMLElement>("[data-output-details]");
      const reason = card.querySelector<HTMLElement>("[data-output-reason]");
      const submit = card.querySelector<HTMLButtonElement>(
        '[data-output-action="submit"]',
      );
      const download = card.querySelector<HTMLButtonElement>(
        '[data-output-action="download"]',
      );
      if (state)
        state.textContent = loading ? "Loading XMP outputs…" : model.xmp.note;
      if (details)
        details.textContent = model.xmp.artifact
          ? `${model.xmp.artifact.filename} · ${new Date(model.xmp.artifact.createdAt).toLocaleString()}. ${model.xmp.artifact.supportNote}`
          : "";
      if (reason)
        reason.textContent = model.xmp.isStale
          ? "This file captures an earlier saved recipe."
          : waiting
            ? "Finish saving the recipe before exporting XMP."
            : "";
      if (submit) {
        submit.disabled = loading || !model.xmp.canSubmit;
        submit.textContent =
          model.xmp.state === "outcome-unknown"
            ? "Check result"
            : "Export edit state";
      }
      if (download) {
        download.hidden = !model.xmp.canDownload;
        download.disabled = loading || !model.xmp.canDownload;
      }
    },
  };
}
export const WORKSPACE_OUTPUT_TEMPLATE = `<section class="photo-editor-export" aria-label="Edit state file"><section class="photo-editor-output-card" data-editor-output="xmp"><strong>Edit state file</strong><p data-output-state role="status"></p><p data-output-details></p><p data-output-reason></p><div class="photo-editor-actions"><button type="button" data-output-action="submit">Export edit state</button><button type="button" class="quiet" data-output-action="download" hidden>Download XMP</button></div></section></section>`;
