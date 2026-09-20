export type PreviewSource = "jpeg-original" | "raw-embedded-jpeg";
export type SelectionState = "undecided" | "selected" | "rejected";
/// One Selection State filter for an open source. `all` keeps every Photo of
/// the source order; every other value keeps only matching Photos.
export type SelectionFilter = "all" | SelectionState;
/// Bounded per-state Selection counts for one open source. They describe the
/// source order, not a filtered view of it.
export type SelectionCounts = Readonly<{
  selected: number;
  rejected: number;
  undecided: number;
}>;
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
    lastRecovery?: Readonly<{
      relocatedPhotos: number;
      fingerprintedOriginals: number;
      unavailablePhotos: number;
    }>;
    fingerprints?: Readonly<{ enrolled: number; pending: number }>;
  }>;
  albums: ReadonlyArray<AlbumSummary>;
}>;

export type BrowseOpenResponse = Readonly<{
  token: string;
  total: number;
  position: number;
  selectionCounts: SelectionCounts;
}>;

export type BrowsePositionResponse = Readonly<{
  position: number | null;
}>;

export type PhotoSummary = Readonly<{
  id: string;
  available: boolean;
  original: Readonly<{ kind: "raw" | "jpeg"; available: boolean }>;
  originalFilename?: string;
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

export type PhotoMetadataResponse = Readonly<{
  captureTime?: string;
  aperture?: string;
  iso?: number;
  shutterSpeed?: string;
  focalLength?: string;
}>;

/** Bounded per-Photo Album membership: Album identities only. */
export type PhotoAlbumsResponse = Readonly<{
  albums: ReadonlyArray<Readonly<{ id: string; name: string }>>;
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
