import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { PhotoLibrary } from "../library/photo-library.js";
import { extractLargestEmbeddedJpeg } from "./libraw-preview.js";
import {
  createTestJpeg,
  selectTestCandidates,
} from "./libraw-preview.native-test.js";

const samplePath = process.env.SLIPSTREAM_RAW_SAMPLE;
const maximumJpegBytes = 128 * 1024 * 1024;
const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories
      .splice(0)
      .map((path) => rm(path, { recursive: true, force: true })),
  );
});

describe("LibRaw embedded JPEG extraction", () => {
  it.skipIf(!samplePath)(
    "selects the largest confined embedded JPEG and leaves the external Original unchanged",
    async () => {
      const before = await sha256(samplePath!);
      const state = await mkdtemp(join(tmpdir(), "slipstream-raw-state-"));
      temporaryDirectories.push(state);
      const library = await PhotoLibrary.open(
        dirname(samplePath!),
        join(state, "library.sqlite"),
      );
      try {
        const outcome = await extractLargestEmbeddedJpeg(
          library.confinedOriginal(basename(samplePath!)),
        );
        expect(outcome).toMatchObject({
          kind: "preview",
          candidateIndex: 2,
          width: 9504,
          height: 6336,
        });
      } finally {
        await library.shutdown();
      }
      expect(await sha256(samplePath!)).toBe(before);
    },
    30_000,
  );

  it("requires a Photo Library capability", () => {
    expect(() => extractLargestEmbeddedJpeg(undefined as never)).toThrow(
      TypeError,
    );
  });

  it("falls back when the largest candidate has malformed entropy", () => {
    const smaller = createTestJpeg(40, 30);
    const malformedLarger = createTestJpeg(80, 60).subarray(0, -12);
    const outcome = selectTestCandidates([
      { declaredLength: malformedLarger.length, jpeg: malformedLarger },
      { declaredLength: smaller.length, jpeg: smaller },
    ]);
    expect(outcome).toMatchObject({
      kind: "preview",
      candidateIndex: 1,
      width: 40,
      height: 30,
    });
  });

  it("orders by decoded dimensions rather than encoded byte size", () => {
    const largePixels = createTestJpeg(64, 64);
    const smallPixelsWithTrailingBytes = Buffer.concat([
      createTestJpeg(32, 32),
      Buffer.alloc(largePixels.length + 100),
    ]);
    expect(
      selectTestCandidates([
        {
          declaredLength: smallPixelsWithTrailingBytes.length,
          jpeg: smallPixelsWithTrailingBytes,
        },
        { declaredLength: largePixels.length, jpeg: largePixels },
      ]),
    ).toMatchObject({ kind: "preview", candidateIndex: 1 });
  });

  it("returns no usable preview when every JPEG is malformed", () => {
    expect(
      selectTestCandidates([
        { declaredLength: 4, jpeg: Buffer.from([0xff, 0xd8, 0xff, 0xd9]) },
      ]),
    ).toMatchObject({ kind: "no-usable-preview" });
  });

  it("rejects an oversized candidate before extraction", () => {
    const valid = createTestJpeg(16, 16);
    expect(
      selectTestCandidates([
        { declaredLength: maximumJpegBytes + 1, jpeg: valid },
        { declaredLength: valid.length, jpeg: valid },
      ]),
    ).toMatchObject({
      kind: "preview",
      candidateIndex: 1,
      extractedCandidateIndexes: [1],
    });
  });

  it("breaks equal-dimension ties by candidate order", () => {
    const jpeg = createTestJpeg(24, 24);
    expect(
      selectTestCandidates([
        { declaredLength: jpeg.length, jpeg },
        { declaredLength: jpeg.length, jpeg },
      ]),
    ).toMatchObject({ kind: "preview", candidateIndex: 0 });
  });

  it("continues after a resource unpack failure when a later candidate succeeds", () => {
    const jpeg = createTestJpeg(24, 16);
    expect(
      selectTestCandidates([
        { declaredLength: jpeg.length, jpeg, unpackOutcome: "resource" },
        { declaredLength: jpeg.length, jpeg },
      ]),
    ).toMatchObject({ kind: "preview", candidateIndex: 1 });
  });

  it("returns resource-limit when unpack resource failure has no later success", () => {
    const jpeg = createTestJpeg(24, 16);
    expect(
      selectTestCandidates([
        { declaredLength: jpeg.length, jpeg, unpackOutcome: "resource" },
      ]),
    ).toMatchObject({ kind: "resource-limit" });
  });

  it("returns internal-error for fatal unpack failure without a usable candidate", () => {
    const jpeg = createTestJpeg(24, 16);
    expect(
      selectTestCandidates([
        { declaredLength: jpeg.length, jpeg, unpackOutcome: "internal" },
      ]),
    ).toMatchObject({ kind: "internal-error" });
  });

  it("rejects malformed JPEG dimensions", () => {
    const jpeg = createTestJpeg(32, 16);
    const sof = jpeg.indexOf(Buffer.from([0xff, 0xc0]));
    const invalid = Buffer.from(jpeg);
    invalid[sof + 5] = 0;
    invalid[sof + 6] = 0;
    expect(
      selectTestCandidates([{ declaredLength: invalid.length, jpeg: invalid }]),
    ).toMatchObject({ kind: "no-usable-preview" });
  });

  it("keeps sensor unpack and RAW development outside the product boundary", async () => {
    const nativeSource = await readFile(
      new URL("../../native/libraw_preview.cc", import.meta.url),
      "utf8",
    );
    expect(nativeSource).not.toMatch(/\blibraw_unpack\s*\(/);
    expect(nativeSource).not.toMatch(/\blibraw_dcraw_process\s*\(/);
    expect(nativeSource).not.toMatch(/\blibraw_raw2image\s*\(/);
    expect(nativeSource).toContain("libraw_unpack_thumb_ex");
  });
});

async function sha256(path: string): Promise<string> {
  return createHash("sha256")
    .update(await readFile(path))
    .digest("hex");
}
