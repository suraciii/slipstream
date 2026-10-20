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

/// The recovery review surface the page view presents: the committed-counts
/// notice, the bounded review listing with its explicit paging, and the
/// reviewed mappings an explicit apply commits.
export interface RecoveryReviewView {
  /// Presents the committed recovery counts of the last scan and, while
  /// Originals remain unavailable, the one bounded review entry.
  setRecoveryNotice(model: RecoveryNoticeModel): void;
  openRecoveryPanel(
    entries: ReadonlyArray<RecoveryEntryViewModel>,
    paging: RecoveryPagingViewModel,
    invoker?: HTMLElement,
  ): void;
  /// Re-renders the entries of the open review, including its paging.
  renderRecoveryEntries(
    entries: ReadonlyArray<RecoveryEntryViewModel>,
    paging: RecoveryPagingViewModel,
  ): void;
  /// Presents the reviewed mappings and their paging; each occupied
  /// destination that an explicit retire-and-bind may replace carries its
  /// checkbox.
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
}

export interface RecoveryPanel extends RecoveryReviewView {
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
  selectionLabel: (value: "unflagged" | "picked" | "rejected") => string;
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
  /// Why one mapping stays unavailable. The occupied reasons are named by
  /// `recoveryEvidenceState`, which also offers the replace or later action;
  /// this table covers the rest.
  const recoveryBlockedLabel = (
    reason: Exclude<RecoveryMappingViewModel["blockedReason"], null>,
  ): string =>
    ({
      colliding: "Another mapping in this set targets the same destination",
      "content-mismatch":
        "Content at the destination differs from the remembered fingerprint",
      "destination-in-use": "Destination already holds another Photo",
      "destination-removed": "The destination Photo was removed",
      "kind-mismatch": "Destination format differs from this Original",
      missing: "No file at the destination yet",
      unreadable: "Destination cannot be read",
    })[reason];
  /// The plain-language evidence state of one proposed mapping. The review
  /// presents exactly three outcomes — updated automatically, ready for
  /// confirmation, still unavailable — so the evidence state below names what
  /// the Photographer can do next, never an internal reason code and never a
  /// claim that file bytes were restored.
  const recoveryEvidenceState = (mapping: RecoveryMappingViewModel): string => {
    if (mapping.blockedReason === "destination-in-use")
      return "Destination already holds another Photo";
    if (mapping.blockedReason === "destination-removed")
      return "The destination Photo was removed";
    if (mapping.blockedReason !== null)
      return recoveryBlockedLabel(mapping.blockedReason);
    if (mapping.outcome === "occupied")
      return mapping.retire === null
        ? "Destination already holds another Photo"
        : "Destination holds another Photo you can replace";
    // Ready for confirmation: content confirmed when the persisted
    // fingerprint matched, otherwise content not confirmed — the absence of
    // a fingerprint is never presented as a content match.
    return mapping.verified ? "Content confirmed" : "Content not confirmed";
  };
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
          `${entry.rating} stars`,
          `${entry.albumCount} album${entry.albumCount === 1 ? "" : "s"}`,
        ].join(" · ");
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
        photoLink.textContent =
          entry.location.split("/").at(-1) || entry.location;
        photoLink.title = entry.location;
        const location = document.createElement("p");
        location.className = "recovery-entry-location";
        location.append(
          photoLink,
          document.createTextNode(` — ${entry.location}`),
        );
        const facts = document.createElement("p");
        facts.className = "recovery-entry-facts";
        facts.textContent = `${state}${entry.kind.toUpperCase()} · ${decisions} · ${fingerprint}`;
        item.append(location, facts);
        if (entry.state === "unavailable") {
          const map = document.createElement("button");
          map.type = "button";
          map.className = "quiet";
          map.dataset.recoveryMapOriginal = entry.originalId;
          map.textContent = "Map this Original";
          map.addEventListener("click", () => {
            recoverySingleOriginal.value = entry.originalId;
            recoverySingleLocation.focus();
          });
          item.append(map);
        }
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
      const parts = [
        model.relocatedPhotos > 0
          ? `${formatPhotoCount(model.relocatedPhotos)} locations updated.`
          : "",
        model.unavailablePhotos > 0
          ? `${formatPhotoCount(model.unavailablePhotos)} remain unavailable.`
          : "",
      ].filter(Boolean);
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
        button.textContent = "Review unavailable Originals";
        button.addEventListener("click", () =>
          send({ kind: "recovery-entry" }),
        );
        recoveryNotice.append(" ", button);
      }
      recoveryNotice.hidden = false;
    },
    openRecoveryPanel(entries, paging, invoker) {
      if (!alive) return;
      // A new review discards the previous proposals and every choice made
      // for them; nothing carries over unreviewed.
      recoveryCurrentMappings = [];
      recoveryMappingChoices.clear();
      recoveryApplyBlocked = false;
      recoverySingleOriginal.replaceChildren();
      recoverySingleLocation.value = "";
      renderRecoveryEntryRows(entries, paging);
      recoveryProposalList.replaceChildren();
      recoveryProposalList.hidden = true;
      recoveryProposalSummary.hidden = true;
      recoveryMappingsMore.hidden = true;
      recoveryNote.hidden = true;
      recoveryApply.hidden = true;
      recoveryMessage.hidden = true;
      surfaces.open("recovery", invoker);
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
        // One plain-language evidence state per mapping: content confirmed,
        // content not confirmed, or destination occupied. The absence of a
        // fingerprint is never presented as a content match.
        facts.textContent = recoveryEvidenceState(mapping);
        item.append(heading, facts);
        if (mapping.blockedReason !== null) {
          // A blocked mapping offers no apply control: it can never join a
          // batch. The evidence state already names why; this names only the
          // next safe action.
          const later = document.createElement("p");
          later.className = "recovery-proposal-later";
          later.textContent =
            "This mapping cannot be applied now. Leave it and review it later.";
          item.append(later);
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
              `Replace the Photo the Library discovered at ${mapping.retire.location} with this one`,
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
              "Use this location even though the old content cannot be verified",
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

/// The recovery review dialog markup this panel owns: the review's bounded
/// listing, the batch and single mapping proposals, and their explicit apply
/// action.
export const RECOVERY_PANEL_TEMPLATE = `        <dialog class="recovery-dialog" data-recovery-panel aria-labelledby="recovery-title">
          <div class="recovery-sheet">
            <header class="recovery-header"><h3 id="recovery-title">Review unavailable Originals</h3><button type="button" class="quiet" data-recovery-close>Close</button></header>
            <div class="recovery-body">
              <p class="recovery-explanation">Recovery changes which file a Photo points to. No file is moved, renamed, or copied, and nothing is overwritten.</p>
              <div class="recovery-forms">
                <section class="recovery-batch" aria-labelledby="recovery-batch-title">
                  <h4 id="recovery-batch-title">Map a moved folder</h4>
                  <label>Old folder prefix<input data-recovery-old-prefix type="text" autocomplete="off" spellcheck="false" placeholder="2023/travel" /></label>
                  <label>New folder prefix<input data-recovery-new-prefix type="text" autocomplete="off" spellcheck="false" placeholder="2024/travel" /></label>
                  <button type="button" data-recovery-propose>Propose batch mappings</button>
                </section>
                <section class="recovery-single" aria-labelledby="recovery-single-title">
                  <h4 id="recovery-single-title">Map one renamed or moved Original</h4>
                  <label>Single Original<select data-recovery-single-original></select></label>
                  <label>New location<input data-recovery-single-location type="text" autocomplete="off" spellcheck="false" placeholder="2024/travel/renamed.ARW" /></label>
                  <button type="button" data-recovery-propose-single>Propose this mapping</button>
                </section>
              </div>
              <section class="recovery-results" aria-label="Proposed mappings">
                <p class="recovery-note" data-recovery-note hidden>Old content cannot be verified for Originals without a fingerprint. Review every mapping and confirm explicitly.</p>
                <p class="recovery-summary" data-recovery-proposal-summary role="status" hidden></p>
                <ul class="recovery-proposals" data-recovery-proposals tabindex="0" aria-label="Proposed mappings" hidden></ul>
                <div class="recovery-more"><button type="button" class="quiet" data-recovery-mappings-more hidden>Load more</button></div>
              </section>
              <section class="recovery-results" aria-label="Unavailable originals">
                <p class="recovery-summary" data-recovery-summary role="status"></p>
                <ul class="recovery-list" data-recovery-list tabindex="0" aria-label="Unavailable originals"></ul>
                <div class="recovery-more"><button type="button" class="quiet" data-recovery-more hidden>Load more</button></div>
              </section>
            </div>
            <footer class="recovery-footer">
              <p class="recovery-message" data-recovery-message role="alert" hidden></p>
              <div class="recovery-actions"><button type="button" data-recovery-apply hidden>Apply mappings</button></div>
            </footer>
          </div>
        </dialog>`;

/// Resolves the panel's elements from the page template above.
export function recoveryPanelElements(root: ParentNode): RecoveryPanelElements {
  return {
    recoveryNotice: required<HTMLElement>(root, "[data-recovery-notice]"),
    recoveryPanel: required<HTMLDialogElement>(root, "[data-recovery-panel]"),
    recoverySummary: required<HTMLElement>(root, "[data-recovery-summary]"),
    recoveryList: required<HTMLElement>(root, "[data-recovery-list]"),
    recoveryMore: required<HTMLButtonElement>(root, "[data-recovery-more]"),
    recoveryOldPrefix: required<HTMLInputElement>(
      root,
      "[data-recovery-old-prefix]",
    ),
    recoveryNewPrefix: required<HTMLInputElement>(
      root,
      "[data-recovery-new-prefix]",
    ),
    recoveryPropose: required<HTMLButtonElement>(
      root,
      "[data-recovery-propose]",
    ),
    recoverySingleOriginal: required<HTMLSelectElement>(
      root,
      "[data-recovery-single-original]",
    ),
    recoverySingleLocation: required<HTMLInputElement>(
      root,
      "[data-recovery-single-location]",
    ),
    recoveryProposeSingle: required<HTMLButtonElement>(
      root,
      "[data-recovery-propose-single]",
    ),
    recoveryNote: required<HTMLElement>(root, "[data-recovery-note]"),
    recoveryProposalSummary: required<HTMLElement>(
      root,
      "[data-recovery-proposal-summary]",
    ),
    recoveryProposalList: required<HTMLElement>(
      root,
      "[data-recovery-proposals]",
    ),
    recoveryMappingsMore: required<HTMLButtonElement>(
      root,
      "[data-recovery-mappings-more]",
    ),
    recoveryApply: required<HTMLButtonElement>(root, "[data-recovery-apply]"),
    recoveryMessage: required<HTMLElement>(root, "[data-recovery-message]"),
    recoveryClose: required<HTMLButtonElement>(root, "[data-recovery-close]"),
  };
}

function required<T extends Element>(root: ParentNode, selector: string): T {
  const value = root.querySelector<T>(selector);
  if (!value) throw new Error(`Missing ${selector}`);
  return value;
}
