//! The closed reading of one Export inspection and the field-for-field check a
//! download must pass before the browser is offered the artifact.
//!
//! The service publishes an artifact object in `GET /api/exports/{id}` and the
//! same facts as response headers on `GET /api/exports/{id}/artifact`, so a
//! client validates a download by comparing every field. That comparison and
//! the wire reading live here, away from the page's DOM and network work, so
//! [Photo Development](../../../../design/photo-development.md#service-surface)
//! can be checked without a browser.

export type ExportArtifact = Readonly<{
  exportId: string;
  target: "development-tiff" | "film-jpeg";
  stage: "develop" | "film";
  contentType: "image/tiff" | "image/jpeg";
  width: number;
  height: number;
  profileIdentity: string;
  byteLength: number;
  sha256: string;
  expiresAt: string;
  filename: string;
  orientation: string;
  sampleFormat: string;
  colorSpace: string;
  iccEmbedded: boolean;
}>;

export type ExportInspection = Readonly<{
  exportId: string;
  target: "development-tiff" | "film-jpeg";
  state: "queued" | "running" | "succeeded" | "failed" | "cancelled";
  failureReason: string;
  createdAt?: string | undefined;
  recipeVersion?: string | undefined;
  sourceRevision?: string | undefined;
  artifact: ExportArtifact | null;
  inspectionPending?: boolean;
}>;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const readString = (
  source: Record<string, unknown>,
  key: string,
): string | undefined => {
  const value = source[key];
  return typeof value === "string" ? value : undefined;
};

const readCount = (
  source: Record<string, unknown>,
  key: string,
): number | undefined => {
  const value = source[key];
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0
    ? value
    : undefined;
};

/// The closed reading of one inspection. An unknown shape is not an Export, so
/// a client never presents a state it cannot attribute to the service.
export const parseExportInspection = (
  value: unknown,
): ExportInspection | undefined => {
  if (!isRecord(value)) return undefined;
  const exportId = readString(value, "exportId");
  const state = readString(value, "state");
  const target = readString(value, "target");
  if (
    !exportId ||
    (target !== "development-tiff" && target !== "film-jpeg") ||
    state === undefined ||
    !["queued", "running", "succeeded", "failed", "cancelled"].includes(state)
  )
    return undefined;
  const artifactValue = value["artifact"];
  let artifact: ExportArtifact | null = null;
  if (artifactValue !== null && artifactValue !== undefined) {
    if (!isRecord(artifactValue)) return undefined;
    const stage = readString(artifactValue, "stage");
    const contentType = readString(artifactValue, "contentType");
    const profileIdentity = readString(artifactValue, "profileIdentity");
    const sha256 = readString(artifactValue, "sha256");
    const expiresAt = readString(artifactValue, "expiresAt");
    const width = readCount(artifactValue, "width");
    const height = readCount(artifactValue, "height");
    const byteLength = readCount(artifactValue, "byteLength");
    const filename = readString(artifactValue, "filename");
    const orientation = readString(artifactValue, "orientation");
    const sampleFormat = readString(artifactValue, "sampleFormat");
    const colorSpace = readString(artifactValue, "colorSpace");
    const iccEmbedded = artifactValue["iccEmbedded"];
    if (
      artifactValue["exportId"] !== exportId ||
      artifactValue["target"] !== target ||
      (target === "development-tiff"
        ? stage !== "develop" || contentType !== "image/tiff"
        : stage !== "film" || contentType !== "image/jpeg") ||
      !profileIdentity?.trim() ||
      !sha256 ||
      !/^[a-f0-9]{64}$/.test(sha256) ||
      !expiresAt ||
      !Number.isFinite(Date.parse(expiresAt)) ||
      !filename?.trim() ||
      !orientation?.trim() ||
      !sampleFormat?.trim() ||
      !colorSpace?.trim() ||
      typeof iccEmbedded !== "boolean" ||
      width === undefined ||
      height === undefined ||
      byteLength === undefined
    )
      return undefined;
    artifact = Object.freeze({
      exportId,
      target,
      stage: stage as ExportArtifact["stage"],
      contentType: contentType as ExportArtifact["contentType"],
      width,
      height,
      profileIdentity,
      byteLength,
      sha256,
      expiresAt,
      filename,
      orientation,
      sampleFormat,
      colorSpace,
      iccEmbedded,
    });
  }
  return Object.freeze({
    exportId,
    target,
    createdAt: readString(value, "createdAt"),
    recipeVersion: readString(value, "recipeVersion"),
    sourceRevision: readString(value, "sourceRevision"),
    state: state as ExportInspection["state"],
    failureReason: readString(value, "failureReason") ?? "",
    artifact,
  });
};

/// Selects the newest attempt and newest retained success independently.
/// A later queued or failed retry never hides an older downloadable artifact.
export const selectExportPair = (
  inspections: readonly ExportInspection[],
  target: ExportInspection["target"],
): Readonly<{
  active: ExportInspection | null;
  retained: ExportInspection | null;
}> => {
  const matching = inspections.filter((entry) => entry.target === target);
  const newest = (
    entries: readonly ExportInspection[],
  ): ExportInspection | null =>
    entries.reduce<ExportInspection | null>(
      (best, entry) =>
        !best || (entry.createdAt ?? "") > (best.createdAt ?? "")
          ? entry
          : best,
      null,
    );
  return {
    active: newest(matching),
    retained: newest(
      matching.filter(
        (entry) => entry.state === "succeeded" && entry.artifact !== null,
      ),
    ),
  };
};

/// Whether a downloaded artifact is the one the inspection described. Every
/// field the download publishes must match, so a substituted, truncated, or
/// differently sized artifact is refused instead of offered as the Export.
export const artifactMatchesHeaders = (
  artifact: ExportArtifact,
  headers: Readonly<{ get(name: string): string | null }>,
): boolean =>
  headers.get("slipstream-artifact-export-id") === artifact.exportId &&
  headers.get("slipstream-artifact-target") === artifact.target &&
  headers.get("slipstream-artifact-stage") === artifact.stage &&
  headers.get("slipstream-artifact-content-type") === artifact.contentType &&
  headers.get("slipstream-artifact-width") === String(artifact.width) &&
  headers.get("slipstream-artifact-height") === String(artifact.height) &&
  headers.get("slipstream-artifact-profile-identity") ===
    artifact.profileIdentity &&
  headers.get("slipstream-artifact-byte-length") ===
    String(artifact.byteLength) &&
  headers.get("slipstream-artifact-sha256") === artifact.sha256 &&
  headers.get("slipstream-artifact-expires-at") === artifact.expiresAt &&
  headers.get("slipstream-artifact-filename") === artifact.filename &&
  headers.get("slipstream-artifact-orientation") === artifact.orientation &&
  headers.get("slipstream-artifact-sample-format") === artifact.sampleFormat &&
  headers.get("slipstream-artifact-color-space") === artifact.colorSpace &&
  headers.get("slipstream-artifact-icc-embedded") ===
    String(artifact.iccEmbedded);

/// The disclosed state of one Export in the workspace's words. An Export that
/// succeeded without a retained artifact, or one whose attempt failed, never
/// reads as a downloadable result.
export const describeExportState = (
  inspection: ExportInspection,
  byteCount: (bytes: number) => string,
): string => {
  const label =
    inspection.target === "film-jpeg" ? "Finished JPEG" : "Development TIFF";
  if (inspection.state === "queued")
    return `${label} queued; processing has not started.`;
  if (inspection.state === "running") return `The ${label} is being processed.`;
  if (inspection.state === "succeeded")
    return inspection.artifact
      ? `${label} ready: ${byteCount(inspection.artifact.byteLength)}, ${inspection.artifact.width}×${inspection.artifact.height}, downloadable until ${inspection.artifact.expiresAt}.`
      : `The ${label} succeeded without a retained artifact.`;
  if (inspection.state === "cancelled") return `The ${label} was cancelled.`;
  return inspection.failureReason
    ? `The ${label} failed: ${inspection.failureReason}`
    : `The ${label} failed.`;
};
