import type { ViewSelectionState } from "./library-browser-view.js";

/// One item the bounded recovery review lists: the remembered facts of an
/// Original the review opened on, with the state the Library holds now.
export type RecoveryEntryViewModel = Readonly<{
  state: "unavailable" | "available" | "removed" | "missing";
  originalId: string;
  photoId: string;
  webUrl: string;
  location: string;
  kind: "raw" | "jpeg";
  rating: number;
  selectionState: ViewSelectionState;
  albumCount: number;
  fingerprintEnrolled: boolean;
}>;

/// One inspectable reviewed mapping for an unavailable Original. A mapping
/// with a blockedReason is presented as blocked and never applied; the
/// retire candidate names the Photo an explicit choice may replace.
export type RecoveryMappingViewModel = Readonly<{
  mappingId: string;
  originalId: string;
  fromLocation: string;
  toLocation: string;
  kind: "raw" | "jpeg";
  outcome:
    | "matched"
    | "content-mismatch"
    | "missing"
    | "kind-mismatch"
    | "unreadable"
    | "occupied"
    | "colliding";
  verified: boolean;
  blockedReason:
    | "colliding"
    | "content-mismatch"
    | "destination-in-use"
    | "destination-removed"
    | "kind-mismatch"
    | "missing"
    | "unreadable"
    | null;
  retire: Readonly<{
    photoId: string;
    originalId: string;
    location: string;
  }> | null;
}>;

/// The explicit paging of one recovery list: how many of the total are
/// loaded, and whether a continuation page remains.
export type RecoveryPagingViewModel = Readonly<{
  shown: number;
  total: number;
  more: boolean;
}>;
