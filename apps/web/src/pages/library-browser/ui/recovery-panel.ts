import type { ModalSurfaces } from "./modal-surface.js";
import { formatPhotoCount } from "./photo-count.js";
import {
  noRecoveryChoice,
  recoveryApplyMappings,
  type RecoveryMappingChoice,
} from "../model/recovery-review.js";
import type {
  LibraryBrowserIntent,
  RecoveryEntryViewModel,
  RecoveryMappingViewModel,
  RecoveryPagingViewModel,
} from "./library-browser-view.js";

type RecoveryPanelIntentKind =
  | "recovery-entry"
  | "recovery-close"
  | "recovery-more"
  | "recovery-mappings-more"
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
  recoveryMore: HTMLButtonElement;
  recoveryOldPrefix: HTMLInputElement;
  recoveryNewPrefix: HTMLInputElement;
  recoveryPropose: HTMLButtonElement;
  recoverySingleOriginal: HTMLSelectElement;
  recoverySingleLocation: HTMLInputElement;
  recoveryProposeSingle: HTMLButtonElement;
  recoveryNote: HTMLElement;
  recoveryProposalSummary: HTMLElement;
  recoveryProposalList: HTMLElement;
  recoveryMappingsMore: HTMLButtonElement;
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
  /// Opens the recovery review with the first page of its bounded listing:
  /// every loaded entry, how many of the total are shown, and whether more
  /// pages remain in the same review.
  openRecoveryPanel(
    entries: ReadonlyArray<RecoveryEntryViewModel>,
    paging: RecoveryPagingViewModel,
  ): void;
  /// Replaces the unavailable entries the open review presents, including
  /// the pages a Load more appended.
  renderRecoveryEntries(
    entries: ReadonlyArray<RecoveryEntryViewModel>,
    paging: RecoveryPagingViewModel,
  ): void;
  /// Presents inspectable reviewed mappings with their explicit paging; a
  /// blocked mapping carries its reason, and each mapping that needs an
  /// explicit choice carries its own unchecked control.
  renderRecoveryProposals(
    mappings: ReadonlyArray<RecoveryMappingViewModel>,
    paging: RecoveryPagingViewModel,
  ): void;
  clearRecoveryProposals(): void;
  markRecoveryProposalsUnusable(): void;
  resetRecoveryProposalChoices(): void;
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
    recoveryMore,
    recoveryOldPrefix,
    recoveryNewPrefix,
    recoveryPropose,
    recoverySingleOriginal,
    recoverySingleLocation,
    recoveryProposeSingle,
    recoveryNote,
    recoveryProposalSummary,
    recoveryProposalList,
    recoveryMappingsMore,
    recoveryApply,
    recoveryMessage,
    recoveryClose,
  } = elements;
  let alive = true;
  let recoveryPending = false;
  let recoveryApplyBlocked = false;
  let recoveryCurrentMappings: ReadonlyArray<RecoveryMappingViewModel> = [];
  /// The explicit per-mapping choices the Photographer made. Both start
  /// unchosen; a default-selected control is not an explicit choice.
  const recoveryMappingChoices = new Map<string, RecoveryMappingChoice>();
  const recoveryOutcomeLabel = (
    outcome: RecoveryMappingViewModel["outcome"],
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
  const recoveryBlockedLabel = (
    reason: Exclude<RecoveryMappingViewModel["blockedReason"], null>,
  ): string =>
    ({
      colliding: "Blocked: another mapping targets this destination",
      "content-mismatch":
        "Blocked: destination content differs from the remembered fingerprint",
      "destination-in-use": "Blocked: destination already holds another Photo",
      "destination-removed": "Blocked: the destination Photo was removed",
      "kind-mismatch": "Blocked: destination format differs",
      missing: "Blocked: no file at the destination",
      unreadable: "Blocked: destination cannot be read",
    })[reason];
  const recoveryEntryStateLabel = (
    state: RecoveryEntryViewModel["state"],
  ): string =>
    ({
      unavailable: "Still unavailable",
      available: "Already recovered",
      removed: "In the Trash",
      missing: "No longer in the Library",
    })[state];
  const recoveryPagingText = (model: RecoveryPagingViewModel): string =>
    `Showing ${model.shown.toLocaleString()} of ${model.total.toLocaleString()}`;
  const updateRecoveryApply = (): void => {
    const applicable = recoveryApplyMappings(
      recoveryCurrentMappings,
      recoveryMappingChoices,
    );
    recoveryApply.textContent =
      applicable.length === 1
        ? "Apply 1 mapping"
        : `Apply ${applicable.length} mappings`;
    recoveryApply.hidden = applicable.length === 0;
    recoveryApply.disabled = recoveryPending || recoveryApplyBlocked;
  };
  /// Renders the entries the review has loaded. The summary always names how
  /// many of the total are shown, so a bounded first page is never read as
  /// the complete set, and Load more appears exactly while a page remains.
  const renderRecoveryEntryRows = (
    entries: ReadonlyArray<RecoveryEntryViewModel>,
    paging: RecoveryPagingViewModel,
  ): void => {
    recoverySummary.textContent = `${recoveryPagingText(paging)} unavailable ${
      paging.total === 1 ? "original" : "originals"
    }`;
    recoveryList.replaceChildren(
      ...entries.map((entry) => {
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
        // An entry the Library settled since the review opened keeps its
        // remembered facts but names the state it is in now.
        const state =
          entry.state === "unavailable"
            ? ""
            : `${recoveryEntryStateLabel(entry.state)} — `;
        const photoLink = document.createElement("a");
        photoLink.href = entry.webUrl;
        photoLink.textContent = `Photo ${entry.photoId}`;
        photoLink.title = entry.photoId;
        item.append(
          document.createTextNode(
            `${state}${entry.location} — ${entry.kind.toUpperCase()} — ${decisions} — ${fingerprint} — `,
          ),
          photoLink,
        );
        return item;
      }),
    );
    recoveryMore.hidden = !paging.more;
    // Only an entry still waiting for a mapping can be proposed singly; a
    // selection that remains loaded stays selected as pages append.
    const selected = recoverySingleOriginal.value;
    recoverySingleOriginal.replaceChildren(
      ...entries
        .filter((entry) => entry.state === "unavailable")
        .map((entry) =>
          Object.assign(document.createElement("option"), {
            value: entry.originalId,
            textContent: entry.location,
          }),
        ),
    );
    if (
      Array.from(recoverySingleOriginal.options).some(
        (option) => option.value === selected,
      )
    )
      recoverySingleOriginal.value = selected;
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
  recoveryMore.addEventListener("click", () => send({ kind: "recovery-more" }));
  recoveryMappingsMore.addEventListener("click", () =>
    send({ kind: "recovery-mappings-more" }),
  );
  recoveryApply.addEventListener("click", () => {
    // Only mappings that are not blocked and whose required choices were
    // made explicitly can be committed; each repeats its reviewed
    // mappingId with the confirmations it needs.
    const items = recoveryApplyMappings(
      recoveryCurrentMappings,
      recoveryMappingChoices,
    );
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
    openRecoveryPanel(entries, paging) {
      if (!alive) return;
      // A new review discards the previous proposals and every choice made
      // for them; nothing carries over unreviewed.
      recoveryCurrentMappings = [];
      recoveryMappingChoices.clear();
      recoveryApplyBlocked = false;
      renderRecoveryEntryRows(entries, paging);
      recoveryProposalList.replaceChildren();
      recoveryProposalList.hidden = true;
      recoveryProposalSummary.hidden = true;
      recoveryMappingsMore.hidden = true;
      recoveryNote.hidden = true;
      recoveryApply.hidden = true;
      recoveryMessage.hidden = true;
      recoverySingleLocation.value = "";
      surfaces.open("recovery");
      recoveryClose.focus();
    },
    renderRecoveryEntries(entries, paging) {
      if (!alive) return;
      renderRecoveryEntryRows(entries, paging);
    },
    resetRecoveryProposalChoices() {
      if (!alive) return;
      recoveryMappingChoices.clear();
      recoveryApplyBlocked = false;
      updateRecoveryApply();
    },
    renderRecoveryProposals(mappings, paging) {
      if (!alive) return;
      recoveryCurrentMappings = mappings;
      recoveryNote.hidden = !mappings.some((mapping) => !mapping.verified);
      recoveryProposalSummary.textContent = `${recoveryPagingText(paging)} proposed ${
        paging.total === 1 ? "mapping" : "mappings"
      }`;
      recoveryProposalSummary.hidden = false;
      const rows = mappings.map((mapping) => {
        const item = document.createElement("li");
        const heading = document.createElement("p");
        heading.className = "recovery-proposal-path";
        heading.textContent = `${mapping.fromLocation} → ${mapping.toLocation}`;
        const facts = document.createElement("p");
        facts.className = "recovery-proposal-facts";
        facts.textContent = `${recoveryOutcomeLabel(mapping.outcome)} · ${
          mapping.verified
            ? "Content verified"
            : "Old content cannot be verified"
        }`;
        item.append(heading, facts);
        if (mapping.blockedReason !== null) {
          // A blocked mapping presents its reason and offers no control: it
          // can never join an apply batch.
          const blocked = document.createElement("p");
          blocked.className = "recovery-proposal-blocked";
          blocked.textContent = recoveryBlockedLabel(mapping.blockedReason);
          item.append(blocked);
          return item;
        }
        const choice = (): RecoveryMappingChoice =>
          recoveryMappingChoices.get(mapping.mappingId) ?? noRecoveryChoice;
        const recordChoice = (
          update: Readonly<{
            retireChosen?: boolean;
            unverifiedConfirmed?: boolean;
          }>,
        ): void => {
          recoveryMappingChoices.set(mapping.mappingId, {
            ...choice(),
            ...update,
          });
          updateRecoveryApply();
        };
        if (mapping.retire !== null) {
          const retireLabel = document.createElement("label");
          retireLabel.className = "recovery-retire";
          const checkbox = document.createElement("input");
          checkbox.type = "checkbox";
          // A continuation page re-renders every row, so each control
          // re-presents the choice already made for its mapping: the visible
          // control must match the batch the apply step would commit.
          checkbox.checked = choice().retireChosen;
          checkbox.addEventListener("change", () => {
            recordChoice({ retireChosen: checkbox.checked });
          });
          retireLabel.append(
            checkbox,
            document.createTextNode(
              `Replace the discovered Photo at ${mapping.retire.location}`,
            ),
          );
          item.append(retireLabel);
        }
        if (!mapping.verified) {
          // The unverified-content acknowledgement is one explicit control
          // per mapping that starts unchecked; without it the mapping cannot
          // be applied. A re-render re-presents the choice already made,
          // exactly like the retire control above.
          const confirmLabel = document.createElement("label");
          confirmLabel.className = "recovery-confirm";
          const checkbox = document.createElement("input");
          checkbox.type = "checkbox";
          checkbox.checked = choice().unverifiedConfirmed;
          checkbox.addEventListener("change", () => {
            recordChoice({ unverifiedConfirmed: checkbox.checked });
          });
          confirmLabel.append(
            checkbox,
            document.createTextNode(
              "Apply even though the old content cannot be verified",
            ),
          );
          item.append(confirmLabel);
        }
        return item;
      });
      recoveryProposalList.replaceChildren(...rows);
      recoveryProposalList.hidden = mappings.length === 0;
      recoveryMappingsMore.hidden = !paging.more;
      updateRecoveryApply();
    },
    clearRecoveryProposals() {
      if (!alive) return;
      recoveryCurrentMappings = [];
      recoveryMappingChoices.clear();
      recoveryApplyBlocked = false;
      recoveryProposalList.replaceChildren();
      recoveryProposalList.hidden = true;
      recoveryProposalSummary.hidden = true;
      recoveryMappingsMore.hidden = true;
      recoveryNote.hidden = true;
      updateRecoveryApply();
    },
    markRecoveryProposalsUnusable() {
      if (!alive) return;
      recoveryApplyBlocked = true;
      recoveryMappingsMore.disabled = true;
      updateRecoveryApply();
    },
    setRecoveryPending(pending) {
      if (!alive) return;
      recoveryPending = pending;
      recoveryPropose.disabled = pending;
      recoveryProposeSingle.disabled = pending;
      recoveryMore.disabled = pending;
      recoveryMappingsMore.disabled = pending || recoveryApplyBlocked;
      updateRecoveryApply();
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
      recoveryCurrentMappings = [];
      recoveryMappingChoices.clear();
    },
  };
}
