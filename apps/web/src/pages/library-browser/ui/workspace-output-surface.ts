import type {
  WorkspaceOutputsView,
  OutputTarget,
  ImageOutputView,
} from "../model/workspace-output-controller.js";
import type { LibraryBrowserIntent } from "./library-browser-view.js";

export function createWorkspaceOutputSurface(
  root: ParentNode,
  send: (
    intent: Extract<LibraryBrowserIntent, { kind: `editor-${string}` }>,
  ) => void,
  signal: AbortSignal,
) {
  const cards = Array.from(
    root.querySelectorAll<HTMLElement>("[data-editor-output]"),
  );
  let photoId = "";
  root.addEventListener(
    "click",
    (event) => {
      const element =
        event.target instanceof Element
          ? event.target.closest<HTMLButtonElement>("[data-output-action]")
          : null;
      if (!element || element.disabled || !photoId) return;
      const card = element.closest<HTMLElement>("[data-editor-output]");
      const target = card?.dataset.editorOutput;
      const action = element.dataset.outputAction;
      if (target === "xmp") {
        if (action === "submit" || action === "download")
          send({
            kind:
              action === "submit" ? "editor-xmp-submit" : "editor-xmp-download",
            photoId,
          });
      } else if (
        (target === "development-tiff" || target === "film-jpeg") &&
        (action === "submit" ||
          action === "cancel" ||
          action === "retry" ||
          action === "download")
      ) {
        send({ kind: `editor-export-${action}`, photoId, target });
      }
    },
    { signal },
  );
  const formatTime = (value: string): string =>
    new Date(value).toLocaleString();
  const renderImage = (
    card: HTMLElement,
    model: ImageOutputView,
    waiting: boolean,
    loading: boolean,
  ) => {
    const name =
      model.target === "film-jpeg" ? "Finished JPEG" : "Editing TIFF";
    const state = card.querySelector<HTMLElement>("[data-output-state]")!;
    const details = card.querySelector<HTMLElement>("[data-output-details]")!;
    state.textContent = `${model.note}${model.isStale ? ` This ${name} is based on an earlier edit.` : ""}`;
    const artifact = model.artifact;
    details.textContent = artifact
      ? [
          artifact.filename,
          `${artifact.byteLength.toLocaleString()} bytes`,
          `${artifact.width}×${artifact.height}`,
          artifact.orientation,
          artifact.colorSpace,
          artifact.sampleFormat,
          artifact.iccEmbedded ? "ICC embedded" : "ICC not embedded",
          model.createdAt
            ? `Created ${formatTime(model.createdAt)}`
            : undefined,
          `Available until ${formatTime(artifact.expiresAt)}`,
        ]
          .filter(Boolean)
          .join(" · ")
      : "";
    const diagnostics = card.querySelector<HTMLDetailsElement>(
      "[data-output-diagnostics]",
    )!;
    diagnostics.hidden = !model.diagnostic && !artifact;
    diagnostics.querySelector<HTMLElement>(
      "[data-output-diagnostic]",
    )!.textContent = [
      model.diagnostic,
      artifact ? `ICC profile SHA-256: ${artifact.profileIdentity}` : "",
    ]
      .filter(Boolean)
      .join(" · ");
    const submit = card.querySelector<HTMLButtonElement>(
      '[data-output-action="submit"]',
    )!;
    submit.disabled = loading || waiting || !model.canSubmit;
    submit.textContent = waiting
      ? "Waiting for save…"
      : artifact
        ? `Export ${name} again`
        : `Export ${name}`;
    const cancel = card.querySelector<HTMLButtonElement>(
      '[data-output-action="cancel"]',
    )!;
    cancel.hidden = !model.canCancel;
    const retry = card.querySelector<HTMLButtonElement>(
      '[data-output-action="retry"]',
    )!;
    retry.hidden = !model.canRetry;
    retry.textContent =
      model.state === "outcome-unknown" ? "Check result" : "Retry";
    const download = card.querySelector<HTMLButtonElement>(
      '[data-output-action="download"]',
    )!;
    download.hidden = !model.canDownload;
    card.querySelector<HTMLElement>("[data-output-reason]")!.textContent =
      !model.canSubmit &&
      !waiting &&
      !loading &&
      !["queued", "running", "submitting", "outcome-unknown"].includes(
        model.state,
      )
        ? "New exports need an available Original and processing. Saved edits and existing files are retained."
        : "";
  };
  return {
    render(
      id: string,
      model: WorkspaceOutputsView,
      waiting: boolean,
      loading: boolean,
    ) {
      photoId = id;
      for (const card of cards) {
        const target = card.dataset.editorOutput;
        if (target !== "xmp") {
          renderImage(
            card,
            target === "film-jpeg" ? model.film : model.tiff,
            waiting,
            loading,
          );
          continue;
        }
        const xmp = model.xmp;
        card.querySelector<HTMLElement>("[data-output-state]")!.textContent =
          `${xmp.note}${xmp.isStale ? " This file is based on an earlier edit." : ""}`;
        card.querySelector<HTMLElement>("[data-output-details]")!.textContent =
          xmp.artifact
            ? `${xmp.artifact.filename} · ${xmp.artifact.byteLength.toLocaleString()} bytes · Created ${formatTime(xmp.artifact.createdAt)} · Available until ${formatTime(xmp.artifact.expiresAt)}`
            : "";
        const submit = card.querySelector<HTMLButtonElement>(
          '[data-output-action="submit"]',
        )!;
        submit.disabled = loading || waiting || !xmp.canSubmit;
        submit.textContent = waiting
          ? "Waiting for save…"
          : xmp.state === "outcome-unknown"
            ? "Check result"
            : "Export edit state";
        card.querySelector<HTMLButtonElement>(
          '[data-output-action="download"]',
        )!.hidden = !xmp.canDownload;
        card.querySelector<HTMLElement>("[data-output-reason]")!.textContent =
          "XMP parameter file. Film and custom white balance use Slipstream fields; other editors may not reproduce them.";
      }
    },
  };
}

export const WORKSPACE_OUTPUT_TEMPLATE = `<section class="photo-editor-export" aria-label="Outputs"><p class="photo-editor-export-heading">Outputs</p>
  <section class="photo-editor-output-card" data-editor-output="xmp" aria-label="Edit state file"><strong>Edit state file</strong><p data-output-state role="status"></p><p data-output-details></p><p data-output-reason></p><div class="photo-editor-actions"><button type="button" data-output-action="submit">Export edit state</button><button type="button" class="quiet" data-output-action="download" hidden>Download XMP</button></div></section>
  ${(["development-tiff", "film-jpeg"] as readonly OutputTarget[])
    .map((target) => {
      const name = target === "film-jpeg" ? "Finished JPEG" : "Editing TIFF";
      return `<section class="photo-editor-output-card" data-editor-output="${target}" aria-label="${name}"><strong>${name}</strong><p data-output-state role="status"></p><p data-output-details></p><p data-output-reason></p><details data-output-diagnostics hidden><summary>Output details</summary><p data-output-diagnostic></p></details><div class="photo-editor-actions"><button type="button" data-output-action="submit">Export ${name}</button><button type="button" class="quiet" data-output-action="cancel" hidden>Cancel</button><button type="button" class="quiet" data-output-action="retry" hidden>Retry</button><button type="button" class="quiet" data-output-action="download" hidden>Download ${name}</button></div></section>`;
    })
    .join("")}
</section>`;
