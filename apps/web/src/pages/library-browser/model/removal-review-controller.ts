import type { SourceAuthority } from "./source-grid-owner.js";
import type { RemovalOwner } from "./removal-owner.js";

export type RemovalReviewModel = Readonly<{
  reviewed: number;
  pending: boolean;
  canConfirm: boolean;
  tone?: "success" | "warning" | "failure";
  message?: string;
  undo?: Readonly<{ removed: number }>;
}>;

type Presentation = Readonly<{
  render(model: RemovalReviewModel): void;
  open(model: RemovalReviewModel): void;
  close(): void;
  renderListingPending(): void;
  reloadListingAfterUndo(message: string): Promise<void>;
  presentListingMessage(message: string): void;
}>;

type CurrentResult = Readonly<{
  token: string;
  total: number;
  authority: SourceAuthority;
  selection: "rejected" | "other";
}>;

export type RemovalReviewControllerOptions = Readonly<{
  removal: RemovalOwner;
  current: () => CurrentResult;
  connected: () => boolean;
  presentation: Presentation;
  refreshAfterChange: (
    reason: Readonly<{ progress: string; settled: string }>,
  ) => Promise<void>;
  refreshOverview: () => Promise<void>;
  refreshFolders: () => Promise<void>;
  reopenAfterRestore: () => Promise<void>;
  updateControls: () => void;
}>;

export interface RemovalReviewController {
  open(): void;
  close(): void;
  reconcile(): void;
  confirm(): Promise<void>;
  undo(surface: "review" | "listing"): Promise<void>;
  dispose(): void;
}

const photoCountText = (count: number): string =>
  `${count.toLocaleString()} ${count === 1 ? "Photo" : "Photos"}`;

export const restorationOutcomeMessage = (
  counts: Readonly<{
    restored: number;
    changedElsewhere: number;
    missing: number;
  }>,
): string => {
  const parts = [
    counts.restored > 0
      ? `${photoCountText(counts.restored)} restored to the Library.`
      : "Nothing was restored.",
  ];
  if (counts.changedElsewhere > 0)
    parts.push(
      `${photoCountText(counts.changedElsewhere)} could not be restored because their removal state changed elsewhere.`,
    );
  if (counts.missing > 0)
    parts.push(`${photoCountText(counts.missing)} no longer in the Library.`);
  return parts.join(" ");
};

const removalOutcomeMessage = (
  counts: Readonly<{
    removed: number;
    changedElsewhere: number;
    missing: number;
    alreadyRemoved: number;
  }>,
): string => {
  const parts = [
    counts.removed > 0
      ? `${photoCountText(counts.removed)} removed from the Library. Their Original Files are unchanged.`
      : "No Photos were removed.",
  ];
  if (counts.changedElsewhere > 0)
    parts.push(
      `${photoCountText(counts.changedElsewhere)} no longer rejected, so they stayed in the Library.`,
    );
  if (counts.alreadyRemoved > 0)
    parts.push(
      `${photoCountText(counts.alreadyRemoved)} already removed by another operation.`,
    );
  if (counts.missing > 0)
    parts.push(`${photoCountText(counts.missing)} no longer in the Library.`);
  return parts.join(" ");
};

export function createRemovalReviewController(
  options: RemovalReviewControllerOptions,
): RemovalReviewController {
  let closed = false;
  let reviewOpen = false;
  let reviewed = 0;
  let result:
    | Readonly<{
        tone: "success" | "warning" | "failure";
        message: string;
      }>
    | undefined;

  const coversPresented = () => {
    const review = options.removal.review;
    const current = options.current();
    return Boolean(
      review &&
        current.token !== "" &&
        current.token === review.token &&
        current.total === review.reviewed &&
        current.authority === review.sourceAuthority &&
        current.selection === "rejected",
    );
  };
  const model = (): RemovalReviewModel => {
    const review = options.removal.review;
    return Object.freeze({
      reviewed: review?.reviewed ?? reviewed,
      pending: options.removal.busy,
      canConfirm: coversPresented(),
      ...(result ? { tone: result.tone, message: result.message } : {}),
      ...(options.removal.operation
        ? { undo: { removed: options.removal.operation.removed } }
        : {}),
    });
  };
  const render = () => {
    if (!closed && reviewOpen) options.presentation.render(model());
  };

  return {
    open() {
      const current = options.current();
      if (
        closed ||
        !options.connected() ||
        current.selection !== "rejected" ||
        current.token === "" ||
        current.total === 0 ||
        options.removal.busy
      )
        return;
      const next = options.removal.openReview(
        current.token,
        current.total,
        current.authority,
      );
      if (!next) return;
      reviewed = next.reviewed;
      result = undefined;
      reviewOpen = true;
      options.presentation.open(model());
    },
    close() {
      if (closed) return;
      reviewOpen = false;
      options.presentation.close();
    },
    reconcile() {
      if (closed || !reviewOpen) return;
      if (options.removal.review === undefined || coversPresented()) return;
      options.removal.discardReview();
      result = {
        tone: "warning",
        message:
          "The reviewed result changed. Review the current rejected result again.",
      };
      render();
    },
    async confirm() {
      if (closed) return;
      const admission = options.removal.confirm();
      if (!admission) return;
      render();
      options.updateControls();
      const outcome = await admission.settlement;
      if (closed || outcome.kind === "detached") return;
      if (outcome.kind === "failed") {
        if (outcome.status === 404) {
          options.removal.discardReview();
          result = {
            tone: "warning",
            message:
              "The reviewed result is no longer available. Review the current rejected result again.",
          };
          await options.refreshAfterChange({
            progress: "Reopening this source with the current rejected result…",
            settled: "Source reopened with the current rejected result.",
          });
          if (closed) return;
        } else {
          result = {
            tone: "failure",
            message: "The removal could not be confirmed. Retry to continue.",
          };
        }
        render();
        options.updateControls();
        return;
      }
      result = {
        tone: outcome.result.counts.removed > 0 ? "success" : "warning",
        message: removalOutcomeMessage(outcome.result.counts),
      };
      render();
      await options.refreshAfterChange({
        progress: "Reopening this source after the removal…",
        settled: "Source reopened after the removal.",
      });
      if (closed) return;
      render();
      options.updateControls();
    },
    async undo(surface) {
      if (closed) return;
      const admission = options.removal.undo();
      if (!admission) return;
      if (surface === "review") render();
      else options.presentation.renderListingPending();
      options.updateControls();
      const outcome = await admission.settlement;
      if (closed || outcome.kind === "detached") return;
      if (outcome.kind === "failed") {
        const message = "The removal could not be undone. Retry to continue.";
        if (surface === "review") {
          result = { tone: "failure", message };
          render();
          options.updateControls();
        } else options.presentation.presentListingMessage(message);
        return;
      }
      options.removal.forgetOperation(admission.operationId);
      const message = restorationOutcomeMessage(outcome.result.counts);
      const tone = outcome.result.counts.restored > 0 ? "success" : "warning";
      await options.refreshOverview().catch(() => {});
      if (closed) return;
      await options.refreshFolders();
      if (closed) return;
      if (surface === "review") {
        result = { tone, message };
        render();
      } else await options.presentation.reloadListingAfterUndo(message);
      if (closed) return;
      await options.reopenAfterRestore();
      if (closed) return;
      options.updateControls();
    },
    dispose() {
      if (closed) return;
      closed = true;
      reviewOpen = false;
    },
  };
}
