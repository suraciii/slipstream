import { describe, expect, test } from "bun:test";
import {
  artifactMatchesHeaders,
  describeExportState,
  parseExportInspection,
  type ExportArtifact,
} from "./photo-export.js";

const artifact = (overrides: Partial<ExportArtifact> = {}): ExportArtifact => ({
  exportId: "export-1",
  target: "development-tiff",
  stage: "develop",
  contentType: "image/tiff",
  width: 6000,
  height: 4000,
  profileIdentity: "ProPhoto",
  byteLength: 2048,
  sha256: "abc",
  expiresAt: "2026-10-02T00:00:00Z",
  ...overrides,
});

const headers = (values: Record<string, string>) => ({
  get: (name: string) => values[name] ?? null,
});

const downloadHeaders = (overrides: Record<string, string> = {}) => ({
  "slipstream-artifact-export-id": "export-1",
  "slipstream-artifact-target": "development-tiff",
  "slipstream-artifact-stage": "develop",
  "slipstream-artifact-content-type": "image/tiff",
  "slipstream-artifact-width": "6000",
  "slipstream-artifact-height": "4000",
  "slipstream-artifact-profile-identity": "ProPhoto",
  "slipstream-artifact-byte-length": "2048",
  "slipstream-artifact-sha256": "abc",
  "slipstream-artifact-expires-at": "2026-10-02T00:00:00Z",
  ...overrides,
});

describe("Export inspection", () => {
  test("reads the closed state and its artifact object", () => {
    const inspection = parseExportInspection({
      exportId: "export-1",
      photoId: "photo-1",
      state: "succeeded",
      target: "development-tiff",
      recipeVersion: "recipe-2",
      sourceRevision: "rev-1",
      bundleId: "bundle",
      terminalOutcome: "succeeded",
      failureReason: null,
      receiptExpiresAt: "2026-10-02T00:00:00Z",
      artifact: {
        exportId: "export-1",
        target: "development-tiff",
        stage: "develop",
        contentType: "image/tiff",
        width: 6000,
        height: 4000,
        profileIdentity: "ProPhoto",
        byteLength: 2048,
        sha256: "abc",
        expiresAt: "2026-10-02T00:00:00Z",
      },
    });
    expect(inspection?.state).toBe("succeeded");
    expect(inspection?.artifact?.byteLength).toBe(2048);
    expect(inspection?.artifact?.width).toBe(6000);
  });

  test("a state outside the closed set is not an Export", () => {
    expect(
      parseExportInspection({ exportId: "export-1", state: "paused" }),
    ).toBeUndefined();
    expect(parseExportInspection({ state: "queued" })).toBeUndefined();
    expect(parseExportInspection(null)).toBeUndefined();
  });

  test("an artifact missing a published field is not an artifact", () => {
    const inspection = parseExportInspection({
      exportId: "export-1",
      state: "succeeded",
      artifact: { target: "development-tiff", byteLength: 2048 },
    });
    expect(inspection).toBeUndefined();
  });

  test("a state without a retained artifact reports none", () => {
    const inspection = parseExportInspection({
      exportId: "export-1",
      state: "failed",
      failureReason: "engine exit 1",
      artifact: null,
    });
    expect(inspection?.artifact).toBeNull();
    expect(describeExportState(inspection!, (bytes) => `${bytes} B`)).toContain(
      "engine exit 1",
    );
  });
});

describe("download validation", () => {
  test("a download matching every field is accepted", () => {
    expect(artifactMatchesHeaders(artifact(), headers(downloadHeaders()))).toBe(
      true,
    );
  });

  test("a substituted identity is refused", () => {
    expect(
      artifactMatchesHeaders(
        artifact(),
        headers(
          downloadHeaders({ "slipstream-artifact-export-id": "export-2" }),
        ),
      ),
    ).toBe(false);
  });

  test("a truncated artifact is refused", () => {
    expect(
      artifactMatchesHeaders(
        artifact(),
        headers(downloadHeaders({ "slipstream-artifact-byte-length": "1024" })),
      ),
    ).toBe(false);
  });

  test("another stage, target, or transform identity is refused", () => {
    expect(
      artifactMatchesHeaders(
        artifact(),
        headers(downloadHeaders({ "slipstream-artifact-stage": "film" })),
      ),
    ).toBe(false);
    expect(
      artifactMatchesHeaders(
        artifact(),
        headers(
          downloadHeaders({ "slipstream-artifact-content-type": "image/jpeg" }),
        ),
      ),
    ).toBe(false);
    expect(
      artifactMatchesHeaders(
        artifact(),
        headers(downloadHeaders({ "slipstream-artifact-sha256": "other" })),
      ),
    ).toBe(false);
  });

  test("an expired download is refused against the inspected expiry", () => {
    expect(
      artifactMatchesHeaders(
        artifact(),
        headers(
          downloadHeaders({
            "slipstream-artifact-expires-at": "2026-09-01T00:00:00Z",
          }),
        ),
      ),
    ).toBe(false);
  });
});
