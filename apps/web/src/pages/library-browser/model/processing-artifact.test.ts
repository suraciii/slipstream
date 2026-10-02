import { describe, expect, test } from "bun:test";
import {
  parseProcessingArtifactRecord,
  processingArtifactDigestRefusal,
  processingArtifactInput,
  processingArtifactMatchesHeaders,
} from "./processing-artifact.js";

const record = (): {
  input: {
    binding: Record<string, unknown>;
    sha256: string;
    byteLength: number;
  };
  [key: string]: unknown;
} => ({
  artifactId: "a".repeat(64),
  photoId: "photo-1",
  stepId: "develop-1",
  module: "darktable",
  adapterSchemaVersion: "darktable-adapter-1",
  parameters: { schemaVersion: "darktable-params-1", tree: { stack: [] } },
  input: {
    binding: {
      kind: "original",
      photoId: "photo-1",
      sourceRevision: "source-1",
    },
    sha256: "b".repeat(64),
    byteLength: 4096,
  },
  outputContract: {
    format: "tiff",
    precision: "float32",
    colorSpace: "prophoto-rgb",
    transfer: "linear",
    geometry: { width: 9504, height: 6336 },
    encoding: "deflate",
  },
  bundleId: "c".repeat(64),
  sha256: "d".repeat(64),
  byteLength: 12_000_000,
});

const recordHeaders = () =>
  new Headers({
    "slipstream-artifact-id": "a".repeat(64),
    "slipstream-artifact-photo-id": "photo-1",
    "slipstream-artifact-step-id": "develop-1",
    "slipstream-artifact-module": "darktable",
    "slipstream-artifact-adapter-schema-version": "darktable-adapter-1",
    "slipstream-artifact-bundle-id": "c".repeat(64),
    "slipstream-artifact-width": "9504",
    "slipstream-artifact-height": "6336",
    "slipstream-artifact-byte-length": "12000000",
    "slipstream-artifact-sha256": "d".repeat(64),
  });

describe("parseProcessingArtifactRecord", () => {
  test("reads the closed provenance record", () => {
    const artifact = parseProcessingArtifactRecord(record());
    expect(artifact?.artifactId).toBe("a".repeat(64));
    expect(artifact?.outputContract.geometry.width).toBe(9504);
    expect(artifact?.input.binding.kind).toBe("original");
  });

  test("reads an artifact input binding with its concrete contract", () => {
    const value = record();
    value.input.binding = {
      kind: "artifact",
      artifactId: "e".repeat(64),
      contract: {
        format: "tiff",
        precision: "float32",
        colorSpace: "prophoto-rgb",
        transfer: "linear",
        geometry: { width: 9504, height: 6336 },
        encoding: "deflate",
      },
    };
    const artifact = parseProcessingArtifactRecord(value);
    expect(artifact?.input.binding.kind).toBe("artifact");
  });

  test("refuses an unknown, malformed, or unsigned shape", () => {
    expect(parseProcessingArtifactRecord({})).toBeUndefined();
    expect(parseProcessingArtifactRecord(null)).toBeUndefined();
    const badDigest = record();
    badDigest.sha256 = "not-a-digest";
    expect(parseProcessingArtifactRecord(badDigest)).toBeUndefined();
    const badGeometry = record();
    (
      badGeometry.outputContract as { geometry: { width: number } }
    ).geometry.width = 0;
    expect(parseProcessingArtifactRecord(badGeometry)).toBeUndefined();
    const badInput = record();
    badInput.input.binding = { kind: "sidecar" };
    expect(parseProcessingArtifactRecord(badInput)).toBeUndefined();
  });
});

describe("processingArtifactInput", () => {
  test("builds the explicit downstream binding with the record's own contract", () => {
    const artifact = parseProcessingArtifactRecord(record());
    expect(artifact && processingArtifactInput(artifact)).toEqual({
      kind: "artifact",
      artifactId: "a".repeat(64),
      contract: {
        format: "tiff",
        precision: "float32",
        colorSpace: "prophoto-rgb",
        transfer: "linear",
        geometry: { width: 9504, height: 6336 },
        encoding: "deflate",
      },
    });
  });
});

describe("processingArtifactMatchesHeaders", () => {
  const artifact = parseProcessingArtifactRecord(record());
  if (!artifact) throw new Error("fixture record must parse");

  test("accepts the record's own facts repeated field for field", () => {
    expect(processingArtifactMatchesHeaders(artifact, recordHeaders())).toBe(
      true,
    );
  });

  test("refuses a foreign, partial, or mismatched framing", () => {
    const foreign = recordHeaders();
    foreign.set("slipstream-artifact-id", "f".repeat(64));
    expect(processingArtifactMatchesHeaders(artifact, foreign)).toBe(false);
    const partial = new Headers();
    partial.set("slipstream-artifact-id", "a".repeat(64));
    expect(processingArtifactMatchesHeaders(artifact, partial)).toBe(false);
    const resized = recordHeaders();
    resized.set("slipstream-artifact-width", "42");
    expect(processingArtifactMatchesHeaders(artifact, resized)).toBe(false);
  });
});

describe("processingArtifactDigestRefusal", () => {
  const artifact = parseProcessingArtifactRecord(record());
  if (!artifact) throw new Error("fixture record must parse");

  test("accepts bytes of the record's length that hash to its digest", async () => {
    const bytes = new Uint8Array(artifact.byteLength);
    const digest = await crypto.subtle.digest("SHA-256", bytes);
    const hex = Array.from(new Uint8Array(digest), (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
    const matching = parseProcessingArtifactRecord({
      ...record(),
      sha256: hex,
    });
    if (!matching) throw new Error("fixture must parse");
    expect(await processingArtifactDigestRefusal(matching, bytes.buffer)).toBe(
      "",
    );
  });

  test("refuses bytes of another length or another digest", async () => {
    const short = new Uint8Array(8);
    expect(
      (await processingArtifactDigestRefusal(artifact, short.buffer)).length,
    ).toBeGreaterThan(0);
    const wrong = parseProcessingArtifactRecord({
      ...record(),
      byteLength: 8,
    });
    if (!wrong) throw new Error("fixture must parse");
    expect(
      (await processingArtifactDigestRefusal(wrong, short.buffer)).length,
    ).toBeGreaterThan(0);
  });
});
