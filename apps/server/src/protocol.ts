export type PreviewSource = "matching-jpeg" | "embedded-raw-jpeg";

export type PhotoSummary = Readonly<{
  id: string;
  available: boolean;
  ambiguous: boolean;
  originals: ReadonlyArray<
    Readonly<{ kind: "raw" | "jpeg"; available: boolean }>
  >;
  preview: Readonly<{
    state: "inspection-pending" | "ready" | "failed" | "unavailable";
    source?: PreviewSource;
    width?: number;
    height?: number;
    limitedDetail?: boolean;
    message?: string;
  }>;
}>;

export type PhotoListResponse = Readonly<{
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
