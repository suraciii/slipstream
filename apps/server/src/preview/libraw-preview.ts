export type EmbeddedJpegOutcome =
  | Readonly<{
      kind: "preview";
      candidateIndex: number;
      width: number;
      height: number;
      jpeg: Buffer;
    }>
  | Readonly<{ kind: "unsupported"; message: string }>
  | Readonly<{ kind: "malformed"; message: string }>
  | Readonly<{ kind: "io-error"; message: string }>
  | Readonly<{ kind: "resource-limit"; message: string }>
  | Readonly<{ kind: "internal-error"; message: string }>
  | Readonly<{ kind: "no-usable-preview"; message: string }>;

export type EmbeddedJpegSource = Readonly<{
  extractEmbeddedJpeg(): Promise<EmbeddedJpegOutcome>;
}>;

export function extractLargestEmbeddedJpeg(
  source: EmbeddedJpegSource,
): Promise<EmbeddedJpegOutcome> {
  if (!source || typeof source.extractEmbeddedJpeg !== "function") {
    throw new TypeError("Expected a Photo Library Original capability");
  }
  return source.extractEmbeddedJpeg();
}
