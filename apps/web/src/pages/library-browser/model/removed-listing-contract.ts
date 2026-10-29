import type { TrashCleanupOperation, TrashListingItem } from "../api/trash.js";

export type RemovalReviewViewModel = Readonly<{
  reviewed: number;
  pending: boolean;
  canConfirm: boolean;
  tone?: "success" | "warning" | "failure";
  message?: string;
  undo?: Readonly<{ removed: number }>;
}>;

export type TrashReviewViewModel = Readonly<{
  itemCount: number;
  albumNames: ReadonlyArray<string>;
  items: ReadonlyArray<
    Readonly<{
      photoId: string;
      originalLocation: string;
      originalKind: "raw" | "jpeg";
      originalSize: number | null;
      originalAvailable: boolean;
      state: TrashListingItem["state"];
      previewState: "captured" | "changed" | "unavailable" | undefined;
      absence: "absent" | "present" | "uninspectable" | undefined;
      blockers: ReadonlyArray<{ kind: string; name: string; action: string }>;
      message: string | null;
      albumNames: ReadonlyArray<string> | null;
    }>
  >;
  canConfirm: boolean;
  confirmLabel: string;
  cleaning: boolean;
}>;

export type TrashOutcomeViewModel =
  | Readonly<{ unconfirmed: true }>
  | Readonly<{
      unconfirmed?: false;
      counts: TrashCleanupOperation["counts"];
      items: TrashCleanupOperation["items"];
    }>;

export type RemovedPanelViewModel = Readonly<{
  status: "loading" | "ready" | "failed";
  start: number;
  total: number;
  limit: number;
  libraryRoot?: string;
  namespace?: "service";
  evaluatedAt?: string;
  expiresAt?: string | null;
  hasPrevious: boolean;
  hasNext: boolean;
  pending: boolean;
  cleaning: boolean;
  selectionCount: number;
  allSelected: boolean;
  canCleanup: boolean;
  canCopy: boolean;
  canRestore: boolean;
  canRetry: boolean;
  restoringPhotoId?: string;
  restoringSelection: boolean;
  message?: string;
  undo?: Readonly<{ removed: number }>;
  outcome?: TrashOutcomeViewModel;
  outcomeBusy?: "check";
  items: ReadonlyArray<
    Readonly<{
      photoId: string;
      originalId: string;
      filename: string;
      originalLocation: string;
      originalKind: "raw" | "jpeg";
      originalSize: number | null;
      originalAvailable: boolean;
      state: TrashListingItem["state"];
      removedAtMs: number;
      selected: boolean;
      excluded: boolean;
      preview: Readonly<{
        state: "inspection-pending" | "ready" | "unavailable" | "failed";
        thumbnailUrl?: string;
      }>;
    }>
  >;
}>;
