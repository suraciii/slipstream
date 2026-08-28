export type PreviewSource = "matching-jpeg" | "embedded-raw-jpeg";
export type SelectionState = "undecided" | "selected" | "rejected";
export type UndoDescription = Readonly<{
  photoId: string;
  field: "selectionState" | "rating";
  priorValue: SelectionState | number;
  expectedCurrent: SelectionState | number;
}>;

export type PhotoSetSummary = Readonly<{
  id: string;
  name: string;
  photoCount: number;
  hasSavedPosition: boolean;
}>;

export type LibraryOverviewResponse = Readonly<{
  published: boolean;
  photoCount: number;
  scan: Readonly<{
    state: string;
    completed?: number;
    total?: number;
  }>;
  photoSets: ReadonlyArray<PhotoSetSummary>;
}>;

export type BrowseOpenResponse = Readonly<{
  token: string;
  total: number;
  position: number;
}>;

export type PhotoSummary = Readonly<{
  id: string;
  available: boolean;
  ambiguous: boolean;
  originals: ReadonlyArray<
    Readonly<{ kind: "raw" | "jpeg"; available: boolean }>
  >;
  selectionState: SelectionState;
  rating: number;
  preview: Readonly<{
    state: "inspection-pending" | "ready" | "failed" | "unavailable";
    source?: PreviewSource;
    width?: number;
    height?: number;
    limitedDetail?: boolean;
    message?: string;
  }>;
}>;

export type BrowseWindowResponse = Readonly<{
  start: number;
  total: number;
  photos: ReadonlyArray<PhotoSummary>;
}>;

export type PhotoListResponse = Readonly<{
  photos: ReadonlyArray<PhotoSummary>;
}>;

export type PhotoSetResponse = Readonly<{
  id: string;
  name: string;
  lastReviewedPhotoId?: string;
  members: ReadonlyArray<{
    photoId: string;
    position: number;
    available: boolean;
    selectionState: SelectionState;
    rating: number;
  }>;
}>;

export type PreviewResponse = Readonly<{
  state: "ready" | "unavailable" | "failed";
  source?: PreviewSource;
  stale?: boolean;
  width?: number;
  height?: number;
  limitedDetail?: boolean;
  url?: string;
  message?: string;
}>;
