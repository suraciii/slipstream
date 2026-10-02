import { isRecord } from "../api/guards.js";

export type HistoricalExportArtifact = Readonly<{
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
  filename: string;
  orientation: string;
  sampleFormat: string;
  colorSpace: string;
  iccEmbedded: boolean;
}>;
export type HistoricalExport = Readonly<{
  exportId: string;
  target: string;
  state: "queued" | "running" | "succeeded" | "failed" | "cancelled";
  failureReason: string;
  createdAt: string | null;
  artifact: HistoricalExportArtifact | null;
}>;

export function parseHistoricalExport(
  value: unknown,
): HistoricalExport | undefined {
  if (
    !isRecord(value) ||
    typeof value["exportId"] !== "string" ||
    !value["exportId"] ||
    (value["target"] !== "development-tiff" &&
      value["target"] !== "film-jpeg") ||
    !["queued", "running", "succeeded", "failed", "cancelled"].includes(
      String(value["state"]),
    )
  )
    return;
  const raw = value["artifact"];
  let artifact: HistoricalExportArtifact | null = null;
  if (raw !== null && raw !== undefined) {
    if (
      !isRecord(raw) ||
      raw["exportId"] !== value["exportId"] ||
      raw["target"] !== value["target"] ||
      (value["target"] === "development-tiff"
        ? raw["stage"] !== "develop" || raw["contentType"] !== "image/tiff"
        : raw["stage"] !== "film" || raw["contentType"] !== "image/jpeg")
    )
      return;
    for (const key of [
      "profileIdentity",
      "sha256",
      "expiresAt",
      "filename",
      "orientation",
      "sampleFormat",
      "colorSpace",
    ])
      if (typeof raw[key] !== "string" || !raw[key].trim()) return;
    for (const key of ["width", "height", "byteLength"])
      if (
        typeof raw[key] !== "number" ||
        !Number.isSafeInteger(raw[key]) ||
        raw[key] <= 0
      )
        return;
    if (
      !/^[a-f0-9]{64}$/.test(raw["sha256"] as string) ||
      !Number.isFinite(Date.parse(raw["expiresAt"] as string)) ||
      /[\\/]/.test(raw["filename"] as string) ||
      typeof raw["iccEmbedded"] !== "boolean"
    )
      return;
    for (const character of raw["filename"] as string)
      if (character.charCodeAt(0) < 32) return;
    artifact = Object.freeze({
      exportId: raw["exportId"],
      target: raw["target"] as string,
      stage: raw["stage"] as string,
      contentType: raw["contentType"] as string,
      width: raw["width"] as number,
      height: raw["height"] as number,
      byteLength: raw["byteLength"] as number,
      profileIdentity: raw["profileIdentity"] as string,
      sha256: raw["sha256"] as string,
      expiresAt: raw["expiresAt"] as string,
      filename: raw["filename"] as string,
      orientation: raw["orientation"] as string,
      sampleFormat: raw["sampleFormat"] as string,
      colorSpace: raw["colorSpace"] as string,
      iccEmbedded: raw["iccEmbedded"],
    });
  }
  return Object.freeze({
    exportId: value["exportId"],
    target: value["target"],
    state: value["state"] as HistoricalExport["state"],
    failureReason:
      typeof value["failureReason"] === "string" ? value["failureReason"] : "",
    createdAt:
      typeof value["createdAt"] === "string" ? value["createdAt"] : null,
    artifact,
  });
}

export function historicalArtifactMatchesHeaders(
  artifact: HistoricalExportArtifact,
  headers: Headers,
): boolean {
  const fields: Record<string, string> = {
    "export-id": artifact.exportId,
    target: artifact.target,
    stage: artifact.stage,
    "content-type": artifact.contentType,
    width: String(artifact.width),
    height: String(artifact.height),
    "profile-identity": artifact.profileIdentity,
    "byte-length": String(artifact.byteLength),
    sha256: artifact.sha256,
    "expires-at": artifact.expiresAt,
    filename: artifact.filename,
    orientation: artifact.orientation,
    "sample-format": artifact.sampleFormat,
    "color-space": artifact.colorSpace,
    "icc-embedded": String(artifact.iccEmbedded),
  };
  return Object.entries(fields).every(
    ([name, expected]) =>
      headers.get(`slipstream-artifact-${name}`) === expected,
  );
}
