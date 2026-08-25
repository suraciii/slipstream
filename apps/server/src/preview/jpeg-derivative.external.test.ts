import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { performance } from "node:perf_hooks";
import { describe, expect, it } from "vitest";

import { DerivativeScheduler } from "./jpeg-derivative.js";
import { extractLargestEmbeddedJpeg } from "./libraw-preview.js";

const samplePath = process.env.SLIPSTREAM_RAW_SAMPLE;

describe("external RAW derivative validation", () => {
  it.skipIf(!samplePath)(
    "extracts the real embedded JPEG and creates bounded derivatives without modifying the Original",
    async () => {
      const before = await sha256(samplePath!);
      const extracted = extractLargestEmbeddedJpeg(samplePath!);
      expect(extracted.kind).toBe("preview");
      if (extracted.kind !== "preview") return;

      const cache = await mkdtemp(
        join(tmpdir(), "slipstream-external-derivative-"),
      );
      try {
        const scheduler = new DerivativeScheduler(cache);
        for (const targetLongEdge of [512, 2560] as const) {
          const result = await scheduler.generate(
            {
              photoIdentity: "external-sony-sample",
              sourceRelativePath: "external-sample.ARW",
              sourceSize: (await stat(samplePath!)).size,
              sourceMtimeMs: (await stat(samplePath!)).mtimeMs,
              embeddedCandidateIdentity: String(extracted.candidateIndex),
              targetLongEdge,
            },
            extracted.jpeg,
          );
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
        await rm(cache, { recursive: true, force: true });
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
