import { createHash } from "node:crypto";
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join } from "node:path";
import { describe, expect, it } from "vitest";
import sharp from "sharp";

import { SlipstreamApplication } from "./application.js";
import { createHttpApp } from "./http-server.js";

const sample = process.env.SLIPSTREAM_RAW_SAMPLE;

describe("external production Preview protocol", () => {
  it.skipIf(!sample)(
    "rejects RAW replacement between scan and descriptor-confined extraction",
    async () => {
      const base = await mkdtemp(join(tmpdir(), "slipstream-http-race-"));
      const root = join(base, "originals");
      await mkdir(root);
      const raw = join(root, `camera${extname(sample!)}`);
      await copyFile(sample!, raw);
      let replaced = false;
      const application = await SlipstreamApplication.open({
        libraryRoot: root,
        stateDirectory: join(base, "state"),
        databaseBasename: "library.sqlite",
        cacheDirectory: join(base, "cache"),
        host: "127.0.0.1",
        port: 3000,
        libraryOptions: {
          beforeConfinedOperation: async (operation) => {
            if (operation === "extract" && !replaced) {
              replaced = true;
              await writeFile(raw, "changed RAW");
            }
          },
        },
      });
      try {
        const id = application.listPhotos().photos[0]!.id;
        const result = await application.preview(id);
        expect(result.state).toBe("unavailable");
        expect(result.message).toContain("rescan");
      } finally {
        await application.shutdown();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );

  it.skipIf(!sample)(
    "uses matching JPEG, falls back to the confined RAW embedded JPEG, and preserves the Original",
    async () => {
      const before = await sha256(sample!);
      const base = await mkdtemp(join(tmpdir(), "slipstream-http-external-"));
      const root = join(base, "originals");
      const web = join(base, "web");
      await mkdir(root);
      await mkdir(web);
      await writeFile(join(web, "index.html"), "Slipstream");
      const raw = join(root, `camera${extname(sample!)}`);
      const matching = join(root, "camera.jpg");
      await copyFile(sample!, raw);
      await writeFile(
        matching,
        await sharp({
          create: {
            width: 160,
            height: 80,
            channels: 3,
            background: "#34905d",
          },
        })
          .jpeg()
          .toBuffer(),
      );
      const application = await SlipstreamApplication.open({
        libraryRoot: root,
        stateDirectory: join(base, "state"),
        databaseBasename: "library.sqlite",
        cacheDirectory: join(base, "cache"),
        host: "127.0.0.1",
        port: 3000,
      });
      try {
        const app = createHttpApp(application, web);
        const id = (
          (await (await app.request("/api/photos")).json()) as {
            photos: Array<{ id: string }>;
          }
        ).photos[0]!.id;
        const first = (await (
          await app.request(`/api/photos/${id}/preview`)
        ).json()) as { source: string; url: string };
        expect(first.source).toBe("matching-jpeg");
        await writeFile(matching, "corrupt jpeg");
        await app.request("/api/scan", { method: "POST" });
        const fallback = (await (
          await app.request(`/api/photos/${id}/preview`)
        ).json()) as {
          source: string;
          url: string;
          width: number;
          height: number;
        };
        expect(fallback.source).toBe("embedded-raw-jpeg");
        expect(fallback.url).not.toBe(first.url);
        expect(Math.max(fallback.width, fallback.height)).toBe(2560);
        expect(
          (await app.request(fallback.url)).headers.get("content-type"),
        ).toBe("image/jpeg");
      } finally {
        await application.shutdown();
        await rm(base, { recursive: true, force: true });
      }
      expect(await sha256(sample!)).toBe(before);
    },
    120_000,
  );
});

async function sha256(path: string): Promise<string> {
  return createHash("sha256")
    .update(await readFile(path))
    .digest("hex");
}
