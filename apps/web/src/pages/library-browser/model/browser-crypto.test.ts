import { describe, expect, test } from "bun:test";
import { randomUuid, blobSha256Hex } from "./browser-crypto.js";
import { composablePreviewDigestRefusal } from "./composable-preview.js";
import { processingArtifactDigestRefusal } from "./processing-artifact.js";

describe("browser crypto helpers", () => {
  test("creates RFC 4122 version 4 UUIDs with secure-random identity", () => {
    const value = randomUuid();
    expect(value).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
  });

  test("computes the complete Blob SHA-256", async () => {
    expect(await blobSha256Hex(new Blob(["abc"]))).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
    expect(await blobSha256Hex(new Blob())).toBe(
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
  });

  test("ordinary HTTP verifies preview and artifact bytes with the SHA-256 worker", async () => {
    const previousSubtle = Object.getOwnPropertyDescriptor(crypto, "subtle");
    Object.defineProperty(crypto, "subtle", {
      configurable: true,
      value: undefined,
    });
    try {
      const bytes = new TextEncoder().encode("abc").buffer;
      const digest =
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
      expect(await blobSha256Hex(new Blob([bytes]))).toBe(digest);
      expect(await composablePreviewDigestRefusal(digest, bytes)).toBe("");
      expect(
        await composablePreviewDigestRefusal("0".repeat(64), bytes),
      ).toContain("could not be verified");
      const artifact = {
        artifactId: "a".repeat(64),
        photoId: "photo-1",
        stepId: "develop-1",
        module: "darktable",
        adapterSchemaVersion: "adapter-1",
        parameters: { schemaVersion: "params-1", tree: {} },
        input: {
          binding: {
            kind: "original" as const,
            photoId: "photo-1",
            sourceRevision: "source-1",
          },
          sha256: digest,
          byteLength: 3,
        },
        outputContract: {
          format: "tiff",
          precision: "float32",
          colorSpace: "prophoto-rgb",
          transfer: "linear",
          geometry: { width: 1, height: 1 },
          encoding: "deflate",
        },
        bundleId: "b".repeat(64),
        sha256: digest,
        byteLength: 3,
      };
      expect(await processingArtifactDigestRefusal(artifact, bytes)).toBe("");
      expect(
        await processingArtifactDigestRefusal(
          { ...artifact, sha256: "0".repeat(64) },
          bytes,
        ),
      ).toContain("did not match its digest");
    } finally {
      if (previousSubtle)
        Object.defineProperty(crypto, "subtle", previousSubtle);
      else Reflect.deleteProperty(crypto, "subtle");
    }
  });
});
