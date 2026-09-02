export type PreviewSource = "matching-jpeg" | "embedded-raw-jpeg";
export type SelectionState = "undecided" | "selected" | "rejected";
export type UndoDescription = Readonly<{
  photoId: string;
  field: "selectionState" | "rating";
  priorValue: SelectionState | number;
  expectedCurrent: SelectionState | number;
}>;

export type AlbumSummary = Readonly<{
  id: string;
  name: string;
  photoCount: number;
  hasSavedPosition: boolean;
}>;

export type LibraryOverviewResponse = Readonly<{
  published: boolean;
  publication?: string;
  photoCount: number;
  scan: Readonly<{
    state: string;
    publication?: string;
    completed?: number;
    total?: number;
  }>;
  albums: ReadonlyArray<AlbumSummary>;
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
    url?: string;
    thumbnailUrl?: string;
    message?: string;
  }>;
}>;

export type BrowseWindowResponse = Readonly<{
  start: number;
  total: number;
  photos: ReadonlyArray<PhotoSummary>;
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

export type FolderChild = Readonly<{
  location: string;
  name: string;
  photoCount: number;
  hasDescendantFolders: boolean;
}>;

export type FileLocationsResponse = Readonly<{
  publication: string;
  parent: string;
  start: number;
  limit: number;
  total: number;
  children: ReadonlyArray<FolderChild>;
}>;
