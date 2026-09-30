import { describe, expect, test } from "bun:test";
import {
  artifactMatchesHeaders,
  describeExportState,
  parseExportInspection,
  selectExportPair,
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
  sha256: "a".repeat(64),
  expiresAt: "2026-10-02T00:00:00Z",
  filename: "export-1.tiff",
  orientation: "top-left",
  sampleFormat: "float32",
  colorSpace: "scene-linear ProPhoto RGB",
  iccEmbedded: true,
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
  "slipstream-artifact-sha256": "a".repeat(64),
  "slipstream-artifact-expires-at": "2026-10-02T00:00:00Z",
  "slipstream-artifact-filename": "export-1.tiff",
  "slipstream-artifact-orientation": "top-left",
  "slipstream-artifact-sample-format": "float32",
  "slipstream-artifact-color-space": "scene-linear ProPhoto RGB",
  "slipstream-artifact-icc-embedded": "true",
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
      artifact: artifact(),
    });
    expect(inspection?.state).toBe("succeeded");
    expect(inspection?.artifact?.byteLength).toBe(2048);
    expect(inspection?.artifact?.width).toBe(6000);
  });

  test("parses a Finished JPEG export as its own target and label", () => {
    const inspection = parseExportInspection({
      exportId: "film-1",
      photoId: "photo-1",
      state: "succeeded",
      target: "film-jpeg",
      recipeVersion: "recipe-2",
      sourceRevision: "rev-1",
      bundleId: "bundle",
      terminalOutcome: "succeeded",
      failureReason: null,
      receiptExpiresAt: "2026-10-02T00:00:00Z",
      artifact: artifact({
        exportId: "film-1",
        target: "film-jpeg",
        stage: "film",
        contentType: "image/jpeg",
        profileIdentity: "sRGB",
        filename: "film-1.jpg",
        sampleFormat: "uint8",
        colorSpace: "sRGB",
        byteLength: 1024,
        sha256: "d".repeat(64),
      }),
    });
    expect(inspection?.target).toBe("film-jpeg");
    expect(describeExportState(inspection!, (bytes) => `${bytes} B`)).toContain(
      "Finished JPEG",
    );
  });

  test("a state outside the closed set is not an Export", () => {
    expect(
      parseExportInspection({
        exportId: "export-1",
        target: "development-tiff",
        state: "paused",
      }),
    ).toBeUndefined();
    expect(
      parseExportInspection({ target: "development-tiff", state: "queued" }),
    ).toBeUndefined();
    expect(parseExportInspection(null)).toBeUndefined();
  });

  test("an inspection must identify its supported output target", () => {
    for (const target of [undefined, null, "other", 1]) {
      expect(
        parseExportInspection({
          exportId: "export-1",
          target,
          state: "queued",
          artifact: null,
        }),
      ).toBeUndefined();
    }
  });

  test("missing or mistyped artifact metadata cannot retain a download", () => {
    for (const field of [
      "exportId",
      "target",
      "stage",
      "contentType",
      "profileIdentity",
      "sha256",
      "expiresAt",
      "width",
      "height",
      "byteLength",
      "filename",
      "orientation",
      "sampleFormat",
      "colorSpace",
      "iccEmbedded",
    ] as const) {
      for (const replacement of [
        undefined,
        null,
        field === "iccEmbedded" ? "true" : false,
      ]) {
        expect(
          parseExportInspection({
            exportId: "export-1",
            target: "development-tiff",
            state: "succeeded",
            artifact: { ...artifact(), [field]: replacement },
          }),
        ).toBeUndefined();
      }
    }
  });

  test("artifact dimensions and size must be positive safe integers", () => {
    for (const field of ["width", "height", "byteLength"] as const) {
      for (const count of [
        0,
        -1,
        1.5,
        NaN,
        Infinity,
        Number.MAX_SAFE_INTEGER + 1,
      ]) {
        expect(
          parseExportInspection({
            exportId: "export-1",
            target: "development-tiff",
            state: "succeeded",
            artifact: artifact({ [field]: count }),
          }),
        ).toBeUndefined();
      }
    }
  });

  test("artifact identity and format must match the inspected TIFF or JPEG", () => {
    for (const replacement of [
      { exportId: "export-2" },
      { target: "other" },
      { target: "film-jpeg", stage: "film", contentType: "image/jpeg" },
      { stage: "film" },
      { stage: "other" },
      { contentType: "image/jpeg" },
      { contentType: "application/octet-stream" },
    ]) {
      expect(
        parseExportInspection({
          exportId: "export-1",
          target: "development-tiff",
          state: "succeeded",
          artifact: { ...artifact(), ...replacement },
        }),
      ).toBeUndefined();
    }
    expect(
      parseExportInspection({
        exportId: "export-1",
        target: "film-jpeg",
        state: "succeeded",
        artifact: artifact({ target: "film-jpeg" }),
      }),
    ).toBeUndefined();
  });

  test("empty metadata, malformed digests and invalid expiry are refused", () => {
    for (const field of [
      "filename",
      "orientation",
      "sampleFormat",
      "colorSpace",
      "profileIdentity",
    ] as const) {
      expect(
        parseExportInspection({
          exportId: "export-1",
          target: "development-tiff",
          state: "succeeded",
          artifact: artifact({ [field]: " " }),
        }),
      ).toBeUndefined();
    }
    for (const replacement of [
      { sha256: "abc" },
      { sha256: "g".repeat(64) },
      { sha256: "A".repeat(64) },
      { expiresAt: "tomorrow" },
      { expiresAt: "" },
    ]) {
      expect(
        parseExportInspection({
          exportId: "export-1",
          target: "development-tiff",
          state: "succeeded",
          artifact: artifact(replacement),
        }),
      ).toBeUndefined();
    }
  });

  test("expired evidence and explicit absent ICC remain inspectable", () => {
    const inspectedArtifact = artifact({
      expiresAt: "2000-01-01T00:00:00Z",
      iccEmbedded: false,
    });
    expect(
      parseExportInspection({
        exportId: "export-1",
        target: "development-tiff",
        state: "succeeded",
        artifact: inspectedArtifact,
      })?.artifact,
    ).toEqual(inspectedArtifact);
  });

  test("a state without a retained artifact reports none", () => {
    const inspection = parseExportInspection({
      exportId: "export-1",
      state: "failed",
      target: "development-tiff",
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

  test("each missing or substituted metadata header refuses the download", () => {
    const expected = downloadHeaders();
    for (const name of Object.keys(expected)) {
      const missing: Record<string, string> = { ...expected };
      delete missing[name];
      expect(artifactMatchesHeaders(artifact(), headers(missing))).toBe(false);
      expect(
        artifactMatchesHeaders(
          artifact(),
          headers({
            ...expected,
            [name]: "substituted",
          }),
        ),
      ).toBe(false);
    }
  });

  test("explicit absent ICC must match the download metadata", () => {
    expect(
      artifactMatchesHeaders(
        artifact({ iccEmbedded: false }),
        headers(
          downloadHeaders({ "slipstream-artifact-icc-embedded": "false" }),
        ),
      ),
    ).toBe(true);
    expect(
      artifactMatchesHeaders(
        artifact({ iccEmbedded: false }),
        headers(downloadHeaders()),
      ),
    ).toBe(false);
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

describe("independent target retention", () => {
  test("keeps the newest attempt and prior successful artifact separately", () => {
    const retained = parseExportInspection({
      exportId: "tiff-old",
      target: "development-tiff",
      state: "succeeded",
      createdAt: "2026-09-30T10:00:00Z",
      artifact: artifact({ exportId: "tiff-old" }),
    })!;
    const retry = parseExportInspection({
      exportId: "tiff-new",
      target: "development-tiff",
      state: "failed",
      createdAt: "2026-09-30T11:00:00Z",
      artifact: null,
    })!;
    const film = parseExportInspection({
      exportId: "film-1",
      target: "film-jpeg",
      state: "queued",
      createdAt: "2026-09-30T12:00:00Z",
      artifact: null,
    })!;
    const pair = selectExportPair([retained, retry, film], "development-tiff");
    expect(pair.active?.exportId).toBe("tiff-new");
    expect(pair.retained?.exportId).toBe("tiff-old");
    expect(
      selectExportPair([retained, retry, film], "film-jpeg").active?.exportId,
    ).toBe("film-1");
  });
});
