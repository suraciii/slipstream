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
  target: string;
  stage: string;
  contentType: string;
  width: number;
  height: number;
  profileIdentity: string;
  byteLength: number;
  sha256: string;
  expiresAt: string;
}>;

export type ExportInspection = Readonly<{
  exportId: string;
  state: "queued" | "running" | "succeeded" | "failed" | "cancelled";
  failureReason: string;
  artifact: ExportArtifact | null;
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
  return typeof value === "number" && Number.isFinite(value)
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
  if (
    !exportId ||
    state === undefined ||
    !["queued", "running", "succeeded", "failed", "cancelled"].includes(state)
  )
    return undefined;
  const artifactValue = value["artifact"];
  let artifact: ExportArtifact | null = null;
  if (artifactValue !== null && artifactValue !== undefined) {
    if (!isRecord(artifactValue)) return undefined;
    const target = readString(artifactValue, "target");
    const stage = readString(artifactValue, "stage");
    const contentType = readString(artifactValue, "contentType");
    const profileIdentity = readString(artifactValue, "profileIdentity");
    const sha256 = readString(artifactValue, "sha256");
    const expiresAt = readString(artifactValue, "expiresAt");
    const width = readCount(artifactValue, "width");
    const height = readCount(artifactValue, "height");
    const byteLength = readCount(artifactValue, "byteLength");
    if (
      !target ||
      !stage ||
      !contentType ||
      !profileIdentity ||
      !sha256 ||
      !expiresAt ||
      width === undefined ||
      height === undefined ||
      byteLength === undefined
    )
      return undefined;
    artifact = Object.freeze({
      exportId,
      target,
      stage,
      contentType,
      width,
      height,
      profileIdentity,
      byteLength,
      sha256,
      expiresAt,
    });
  }
  return Object.freeze({
    exportId,
    state: state as ExportInspection["state"],
    failureReason: readString(value, "failureReason") ?? "",
    artifact,
  });
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
  headers.get("slipstream-artifact-expires-at") === artifact.expiresAt;

/// The disclosed state of one Export in the workspace's words. An Export that
/// succeeded without a retained artifact, or one whose attempt failed, never
/// reads as a downloadable result.
export const describeExportState = (
  inspection: ExportInspection,
  byteCount: (bytes: number) => string,
): string => {
  if (inspection.state === "queued")
    return "Development TIFF queued; processing has not started.";
  if (inspection.state === "running")
    return "The Development TIFF is being processed.";
  if (inspection.state === "succeeded")
    return inspection.artifact
      ? `Development TIFF ready: ${byteCount(inspection.artifact.byteLength)}, ${inspection.artifact.width}×${inspection.artifact.height}, downloadable until ${inspection.artifact.expiresAt}.`
      : "The Development TIFF succeeded without a retained artifact.";
  if (inspection.state === "cancelled")
    return "The Development TIFF was cancelled.";
  return inspection.failureReason
    ? `The Development TIFF failed: ${inspection.failureReason}`
    : "The Development TIFF failed.";
};
