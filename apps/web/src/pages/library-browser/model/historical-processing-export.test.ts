import { describe, expect, test } from "bun:test";
import {
  historicalArtifactMatchesHeaders,
  parseHistoricalExport,
} from "./historical-processing-export.js";

const historical = () => ({
  exportId: "old",
  target: "development-tiff",
  state: "succeeded",
  createdAt: "2026-09-01T00:00:00Z",
  artifact: {
    exportId: "old",
    target: "development-tiff",
    stage: "develop",
    contentType: "image/tiff",
    width: 10,
    height: 20,
    profileIdentity: "ProPhoto RGB",
    byteLength: 1,
    sha256: "a".repeat(64),
    expiresAt: "2099-10-01T00:00:00Z",
    filename: "old.tiff",
    orientation: "top-left",
    sampleFormat: "float32",
    colorSpace: "prophoto-rgb",
    iccEmbedded: true,
  },
});

describe("historical readonly exports", () => {
  test("preserves real legacy provenance without making a composable artifact", () => {
    const parsed = parseHistoricalExport(historical());
    expect(parsed?.artifact?.profileIdentity).toBe("ProPhoto RGB");
    expect(parsed?.artifact?.filename).toBe("old.tiff");
    expect(parsed?.createdAt).toBe("2026-09-01T00:00:00Z");
    expect(parsed?.artifact).not.toHaveProperty("artifactId");
    const raw = historical();
    expect(
      parseHistoricalExport({
        ...raw,
        artifact: { ...raw.artifact, target: "film-jpeg" },
      }),
    ).toBeUndefined();
    expect(
      parseHistoricalExport({
        ...raw,
        artifact: { ...raw.artifact, filename: "../old.tiff" },
      }),
    ).toBeUndefined();
  });

  test("refuses changed legacy download provenance", () => {
    const artifact = parseHistoricalExport(historical())!.artifact!;
    const headers = new Headers({
      "slipstream-artifact-export-id": artifact.exportId,
      "slipstream-artifact-target": artifact.target,
      "slipstream-artifact-stage": artifact.stage,
      "slipstream-artifact-content-type": artifact.contentType,
      "slipstream-artifact-width": "10",
      "slipstream-artifact-height": "20",
      "slipstream-artifact-profile-identity": artifact.profileIdentity,
      "slipstream-artifact-byte-length": "1",
      "slipstream-artifact-sha256": artifact.sha256,
      "slipstream-artifact-expires-at": artifact.expiresAt,
      "slipstream-artifact-filename": artifact.filename,
      "slipstream-artifact-orientation": artifact.orientation,
      "slipstream-artifact-sample-format": artifact.sampleFormat,
      "slipstream-artifact-color-space": artifact.colorSpace,
      "slipstream-artifact-icc-embedded": "true",
    });
    expect(historicalArtifactMatchesHeaders(artifact, headers)).toBe(true);
    for (const field of [
      "sha256",
      "filename",
      "orientation",
      "icc-embedded",
      "profile-identity",
      "width",
    ]) {
      const changed = new Headers(headers);
      changed.set(`slipstream-artifact-${field}`, "substituted");
      expect(historicalArtifactMatchesHeaders(artifact, changed)).toBe(false);
    }
  });
});
