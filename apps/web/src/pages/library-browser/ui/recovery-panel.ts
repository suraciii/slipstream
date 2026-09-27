import type { ModalSurfaces } from "./modal-surface.js";
import { formatPhotoCount } from "./photo-count.js";
import type {
  LibraryBrowserIntent,
  RecoveryEntryViewModel,
  RecoveryProposalViewModel,
} from "./library-browser-view.js";

type RecoveryPanelIntentKind =
  | "recovery-entry"
  | "recovery-close"
  | "recovery-propose"
  | "recovery-propose-single"
  | "recovery-apply";

export type RecoveryPanelIntent = Extract<
  LibraryBrowserIntent,
  { kind: RecoveryPanelIntentKind }
>;

export type RecoveryPanelElements = Readonly<{
  recoveryNotice: HTMLElement;
  recoveryPanel: HTMLDialogElement;
  recoverySummary: HTMLElement;
  recoveryList: HTMLElement;
  recoveryOldPrefix: HTMLInputElement;
  recoveryNewPrefix: HTMLInputElement;
  recoveryPropose: HTMLButtonElement;
  recoverySingleOriginal: HTMLSelectElement;
  recoverySingleLocation: HTMLInputElement;
  recoveryProposeSingle: HTMLButtonElement;
  recoveryNote: HTMLElement;
  recoveryProposalList: HTMLElement;
  recoveryApply: HTMLButtonElement;
  recoveryMessage: HTMLElement;
  recoveryClose: HTMLButtonElement;
}>;

type RecoveryNoticeModel = Readonly<{
  relocatedPhotos: number;
  unavailablePhotos: number;
}>;

export interface RecoveryPanel {
  setRecoveryNotice(model: RecoveryNoticeModel): void;
  openRecoveryPanel(entries: ReadonlyArray<RecoveryEntryViewModel>): void;
  renderRecoveryProposals(
    proposals: ReadonlyArray<RecoveryProposalViewModel>,
  ): void;
  setRecoveryPending(pending: boolean): void;
  setRecoveryMessage(text?: string): void;
  closeRecoveryPanel(): void;
  dispose(): void;
}

export function createRecoveryPanel({
  elements,
  send,
  surfaces,
  selectionLabel,
}: Readonly<{
  elements: RecoveryPanelElements;
  send: (intent: RecoveryPanelIntent) => void;
  surfaces: ModalSurfaces;
  selectionLabel: (value: "undecided" | "selected" | "rejected") => string;
}>): RecoveryPanel {
  const {
    recoveryNotice,
    recoveryPanel,
    recoverySummary,
    recoveryList,
    recoveryOldPrefix,
    recoveryNewPrefix,
    recoveryPropose,
    recoverySingleOriginal,
    recoverySingleLocation,
    recoveryProposeSingle,
    recoveryNote,
    recoveryProposalList,
    recoveryApply,
    recoveryMessage,
    recoveryClose,
  } = elements;
  let alive = true;
  let currentProposals: ReadonlyArray<RecoveryProposalViewModel> = [];
  const retireSelection = new Map<string, boolean>();

  const outcomeLabel = (
    outcome: RecoveryProposalViewModel["outcome"],
  ): string =>
    ({
      matched: "Ready to recover",
      "content-mismatch": "Content differs from the remembered fingerprint",
      missing: "No file at the destination",
      "kind-mismatch": "Destination format differs",
      unreadable: "Destination cannot be read",
      occupied: "Destination already holds another Photo",
      colliding: "Another mapping targets this destination",
    })[outcome];
  const updateApply = (): void => {
    const applicable = currentProposals.filter(
      (proposal) =>
        proposal.outcome === "matched" ||
        (proposal.outcome === "occupied" &&
          proposal.retire &&
          retireSelection.get(proposal.originalId)),
    );
    recoveryApply.textContent =
      applicable.length === 1
        ? "Apply 1 mapping"
        : `Apply ${applicable.length} mappings`;
    recoveryApply.hidden = applicable.length === 0;
  };

  surfaces.register("recovery", {
    dialog: recoveryPanel,
    modal: () => true,
  });
  recoveryClose.addEventListener("click", () =>
    send({ kind: "recovery-close" }),
  );
  recoveryPropose.addEventListener("click", () =>
    send({
      kind: "recovery-propose",
      oldPrefix: recoveryOldPrefix.value.trim(),
      newPrefix: recoveryNewPrefix.value.trim(),
    }),
  );
  recoveryProposeSingle.addEventListener("click", () =>
    send({
      kind: "recovery-propose-single",
      originalId: recoverySingleOriginal.value,
      newLocation: recoverySingleLocation.value.trim(),
    }),
  );
  recoveryApply.addEventListener("click", () => {
    const items = currentProposals
      .filter(
        (proposal) =>
          proposal.outcome === "matched" ||
          (proposal.outcome === "occupied" &&
            proposal.retire &&
            retireSelection.get(proposal.originalId)),
      )
      .map((proposal) => ({
        originalId: proposal.originalId,
        newLocation: proposal.toLocation,
        retireDestination: proposal.outcome === "occupied",
      }));
    if (items.length === 0) return;
    send({ kind: "recovery-apply", items });
  });

  return {
    setRecoveryNotice(model) {
      if (!alive) return;
      const parts: string[] = [];
      if (model.relocatedPhotos > 0)
        parts.push(
          `Updated locations for ${formatPhotoCount(model.relocatedPhotos)}.`,
        );
      if (model.unavailablePhotos > 0)
        parts.push(
          `${formatPhotoCount(model.unavailablePhotos)} still unavailable.`,
        );
      recoveryNotice.replaceChildren();
      if (parts.length === 0) {
        recoveryNotice.hidden = true;
        return;
      }
      recoveryNotice.append(document.createTextNode(parts.join(" ")));
      if (model.unavailablePhotos > 0) {
        const button = document.createElement("button");
        button.type = "button";
        button.className = "summary-action";
        button.textContent = "Review unavailable originals";
        button.addEventListener("click", () =>
          send({ kind: "recovery-entry" }),
        );
        recoveryNotice.append(" ", button);
      }
      recoveryNotice.hidden = false;
    },
    openRecoveryPanel(entries) {
      if (!alive) return;
      currentProposals = [];
      retireSelection.clear();
      recoverySummary.textContent = `${formatPhotoCount(entries.length)} unavailable`;
      const rows = entries.slice(0, 100).map((entry) => {
        const item = document.createElement("li");
        const decisions = [
          selectionLabel(entry.selectionState),
          entry.rating > 0 ? `${entry.rating} stars` : null,
          entry.albumCount > 0
            ? `${entry.albumCount} album${entry.albumCount === 1 ? "" : "s"}`
            : null,
        ]
          .filter(Boolean)
          .join(" · ");
        const fingerprint = entry.fingerprintEnrolled
          ? "Fingerprint on file"
          : "No fingerprint";
        item.textContent = `${entry.location} — ${entry.kind.toUpperCase()} — ${decisions} — ${fingerprint}`;
        return item;
      });
      if (entries.length > 100) {
        const more = document.createElement("li");
        more.textContent = `…and ${entries.length - 100} more`;
        rows.push(more);
      }
      recoveryList.replaceChildren(...rows);
      recoverySingleOriginal.replaceChildren(
        ...entries.map((entry) =>
          Object.assign(document.createElement("option"), {
            value: entry.originalId,
            textContent: entry.location,
          }),
        ),
      );
      recoveryProposalList.replaceChildren();
      recoveryProposalList.hidden = true;
      recoveryNote.hidden = true;
      recoveryApply.hidden = true;
      recoveryMessage.hidden = true;
      recoverySingleLocation.value = "";
      surfaces.open("recovery");
      recoveryClose.focus();
    },
    renderRecoveryProposals(proposals) {
      if (!alive) return;
      currentProposals = proposals;
      recoveryNote.hidden = !proposals.some((proposal) => !proposal.verified);
      const rows = proposals.map((proposal) => {
        const item = document.createElement("li");
        const heading = document.createElement("p");
        heading.className = "recovery-proposal-path";
        heading.textContent = `${proposal.fromLocation} → ${proposal.toLocation}`;
        const facts = document.createElement("p");
        facts.className = "recovery-proposal-facts";
        facts.textContent = `${outcomeLabel(proposal.outcome)} · ${
          proposal.verified
            ? "Content verified"
            : "Old content cannot be verified"
        }`;
        item.append(heading, facts);
        if (proposal.outcome === "occupied" && proposal.retire) {
          const retireLabel = document.createElement("label");
          retireLabel.className = "recovery-retire";
          const checkbox = document.createElement("input");
          checkbox.type = "checkbox";
          checkbox.addEventListener("change", () => {
            retireSelection.set(proposal.originalId, checkbox.checked);
            updateApply();
          });
          retireLabel.append(
            checkbox,
            document.createTextNode(
              `Replace the discovered Photo at ${proposal.retire.location}`,
            ),
          );
          item.append(retireLabel);
        }
        return item;
      });
      recoveryProposalList.replaceChildren(...rows);
      recoveryProposalList.hidden = proposals.length === 0;
      updateApply();
    },
    setRecoveryPending(pending) {
      if (!alive) return;
      recoveryPropose.disabled = pending;
      recoveryProposeSingle.disabled = pending;
      recoveryApply.disabled = pending;
    },
    setRecoveryMessage(text) {
      if (!alive) return;
      if (!text) {
        recoveryMessage.hidden = true;
        return;
      }
      recoveryMessage.textContent = text;
      recoveryMessage.hidden = false;
    },
    closeRecoveryPanel() {
      if (!alive) return;
      surfaces.close("recovery");
    },
    dispose() {
      if (!alive) return;
      alive = false;
      currentProposals = [];
      retireSelection.clear();
    },
  };
}
