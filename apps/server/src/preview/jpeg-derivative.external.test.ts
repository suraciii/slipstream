import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { performance } from "node:perf_hooks";
import { describe, expect, it } from "vitest";

import { PhotoLibrary } from "../library/photo-library.js";
import { DerivativeScheduler } from "./jpeg-derivative.js";
import { extractLargestEmbeddedJpeg } from "./libraw-preview.js";

const samplePath = process.env.SLIPSTREAM_RAW_SAMPLE;

describe("external RAW derivative validation", () => {
  it.skipIf(!samplePath)(
    "extracts the real embedded JPEG and creates bounded derivatives without modifying the Original",
    async () => {
      const before = await sha256(samplePath!);
      const state = await mkdtemp(join(tmpdir(), "slipstream-external-state-"));
      const cache = join(state, "derivatives");
      const library = await PhotoLibrary.open(
        dirname(samplePath!),
        join(state, "library.sqlite"),
      );
      const extracted = await extractLargestEmbeddedJpeg(
        library.confinedOriginal(basename(samplePath!)),
      );
      expect(extracted.kind).toBe("preview");
      if (extracted.kind !== "preview") {
        await library.shutdown();
        return;
      }

      try {
        const scheduler = new DerivativeScheduler(cache);
        for (const targetLongEdge of [512, 2560] as const) {
          const result = await scheduler.generate(
            {
              photoIdentity: "external-sony-sample",
              source: "embedded-raw-jpeg",
              sourceRelativePath: "external-sample.ARW",
              sourceSize: (await stat(samplePath!)).size,
              sourceMtimeMs: (await stat(samplePath!)).mtimeMs,
              embeddedCandidateIdentity: String(extracted.candidateIndex),
              targetLongEdge,
            },
            extracted.jpeg,
          );
          if (result.kind !== "ready")
            throw new Error(`${result.kind}: ${result.message}`);
          expect(result).toMatchObject({ kind: "ready" });
          if (result.kind === "ready") {
            expect(Math.max(result.width, result.height)).toBe(targetLongEdge);
            expect((await stat(result.cachePath)).size).toBeLessThan(
              64 * 1024 * 1024,
            );
          }
        }

        const measure = async (count: number) => {
          const started = performance.now();
          const beforeRss = process.memoryUsage().rss;
          const source = await stat(samplePath!);
          await Promise.all(
            Array.from({ length: count }, (_, index) =>
              scheduler.generate(
                {
                  photoIdentity: `external-measure-${count}-${index}`,
                  source: "embedded-raw-jpeg",
                  sourceRelativePath: "external-sample.ARW",
                  sourceSize: source.size,
                  sourceMtimeMs: source.mtimeMs + index,
                  embeddedCandidateIdentity: String(extracted.candidateIndex),
                  targetLongEdge: 2560,
                },
                extracted.jpeg,
                { priority: "current", retry: true },
              ),
            ),
          );
          console.log(
            JSON.stringify({
              derivativeMeasurement: {
                count,
                elapsedMs: Math.round(performance.now() - started),
                rssDeltaBytes: process.memoryUsage().rss - beforeRss,
              },
            }),
          );
        };
        await measure(1);
        await measure(2);
      } finally {
        await library.shutdown();
        await rm(state, { recursive: true, force: true });
      }
      expect(await sha256(samplePath!)).toBe(before);
    },
    120_000,
  );
});

async function sha256(path: string): Promise<string> {
  return createHash("sha256")
    .update(await readFile(path))
    .digest("hex");
}
