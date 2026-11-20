import type { FolderPendingViewModel } from "./library-browser-view-contract.js";

export type FolderPendingPresenter = Readonly<{
  render(model: FolderPendingViewModel | undefined): void;
}>;

export const createFolderPendingPresenter = (
  root: ParentNode,
  onCheck: (location: string) => void,
): FolderPendingPresenter => {
  const section = root.querySelector<HTMLElement>("[data-folder-pending]");
  const text = root.querySelector<HTMLElement>("[data-folder-pending-text]");
  const check = root.querySelector<HTMLButtonElement>(
    "[data-folder-pending-check]",
  );
  if (!section || !text || !check)
    throw new Error("Folder freshness surface is missing");

  let model: FolderPendingViewModel | undefined;
  check.addEventListener("click", () => {
    if (!model?.actionEnabled) return;
    onCheck(model.location);
  });

  return {
    render(next) {
      model = next;
      if (!next) {
        section.hidden = true;
        text.textContent = "";
        check.disabled = true;
        return;
      }
      section.hidden = false;
      text.textContent =
        next.progress ??
        (next.pendingFiles === 1
          ? "1 file in this folder needs checking."
          : `${next.pendingFiles.toLocaleString()} files in this folder need checking.`);
      check.disabled = !next.actionEnabled;
      section.setAttribute("aria-busy", String(next.active));
    },
  };
};
