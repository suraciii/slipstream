import { createHash } from "node:crypto";
import { mkdtemp, mkdir, rm, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import sharp from "sharp";

import { SlipstreamApplication } from "./application.js";
import { createHttpApp, startServer } from "./http-server.js";

const temporary: string[] = [];
afterEach(async () => {
  await Promise.all(
    temporary
      .splice(0)
      .map((path) => rm(path, { recursive: true, force: true })),
  );
});

async function fixture() {
  const base = await mkdtemp(join(tmpdir(), "slipstream-http-"));
  temporary.push(base);
  const root = join(base, "originals");
  const web = join(base, "web");
  await mkdir(root);
  await mkdir(web);
  await writeFile(join(web, "index.html"), "<main>built web</main>");
  return { base, root, web };
}

async function jpeg(width = 80, height = 40, color = "#c04020") {
  return sharp({ create: { width, height, channels: 3, background: color } })
    .jpeg()
    .toBuffer();
}

async function open(root: string, base: string) {
  return SlipstreamApplication.open({
    libraryRoot: root,
    stateDirectory: join(base, "state"),
    databaseBasename: "library.sqlite",
    cacheDirectory: join(base, "cache"),
    host: "127.0.0.1",
    port: 3000,
  });
}

describe("production HTTP protocol", () => {
  it("releases application resources when asynchronous listen fails", async () => {
    const { base, root, web } = await fixture();
    await writeFile(join(root, "photo.jpg"), await jpeg());
    const blocker = createServer();
    await new Promise<void>((resolve) =>
      blocker.listen(0, "127.0.0.1", resolve),
    );
    const address = blocker.address();
    const occupiedPort =
      typeof address === "object" && address ? address.port : 0;
    try {
      await expect(
        startServer({
          libraryRoot: root,
          stateDirectory: join(base, "state-b"),
          databaseBasename: "library.sqlite",
          cacheDirectory: join(base, "cache-b"),
          host: "127.0.0.1",
          port: occupiedPort,
          webRoot: web,
        }),
      ).rejects.toThrow();
      await new Promise<void>((resolve) => blocker.close(() => resolve()));
      const recovered = await startServer({
        libraryRoot: root,
        stateDirectory: join(base, "state-b"),
        databaseBasename: "library.sqlite",
        cacheDirectory: join(base, "cache-b"),
        host: "127.0.0.1",
        port: occupiedPort,
        webRoot: web,
      });
      await recovered.close();
    } finally {
      if (blocker.listening)
        await new Promise<void>((resolve) => blocker.close(() => resolve()));
    }
  });

  it("allows missing and same-origin scan Origin but rejects foreign Origin before mutation", async () => {
    const { base, root, web } = await fixture();
    await writeFile(join(root, "photo.jpg"), await jpeg());
    const application = await open(root, base);
    try {
      const app = createHttpApp(application, web);
      expect(
        (await app.request("http://camera.local/api/scan", { method: "POST" }))
          .status,
      ).toBe(200);
      expect(
        (
          await app.request("http://camera.local/api/scan", {
            method: "POST",
            headers: { Origin: "http://camera.local" },
          })
        ).status,
      ).toBe(200);
      expect(
        (
          await app.request("http://camera.local/api/scan", {
            method: "POST",
            headers: { Origin: "https://foreign.example" },
          })
        ).status,
      ).toBe(403);
      expect(
        (
          await app.request("http://camera.local/api/scan", {
            method: "POST",
            headers: { Origin: "not an origin" },
          })
        ).status,
      ).toBe(403);
    } finally {
      await application.shutdown();
    }
  });

  it("lists deterministic path-free Photo facts and serves matching JPEG with immutable ETag validation", async () => {
    const { base, root, web } = await fixture();
    await writeFile(join(root, "b.JPG"), await jpeg(120, 60));
    await writeFile(join(root, "a.jpg"), await jpeg(90, 45));
    const application = await open(root, base);
    try {
      const app = createHttpApp(application, web);
      const list = await app.request("/api/photos");
      expect(list.status).toBe(200);
      const body = (await list.json()) as {
        photos: Array<{ id: string; originals: unknown[] }>;
      };
      const expectedIds = ["a.jpg", "b.JPG"].map((path) =>
        createHash("sha256")
          .update(
            `photo\0${createHash("sha256").update(`original\0${path}`).digest("hex")}`,
          )
          .digest("hex"),
      );
      expect(body.photos.map((photo) => photo.id)).toEqual(expectedIds);
      expect(JSON.stringify(body)).not.toContain(root);
      const rescanned = (await (
        await app.request("http://camera.local/api/scan", { method: "POST" })
      ).json()) as typeof body;
      expect(rescanned.photos.map((photo) => photo.id)).toEqual(expectedIds);
      const repeated = (await (
        await app.request("/api/photos")
      ).json()) as typeof body;
      expect(repeated.photos.map((photo) => photo.id)).toEqual(expectedIds);
      const id = body.photos[0]!.id;
      const preview = await app.request(`/api/photos/${id}/preview`);
      expect(preview.status).toBe(200);
      const metadata = (await preview.json()) as {
        source: string;
        url: string;
        limitedDetail: boolean;
      };
      expect(metadata).toMatchObject({
        source: "matching-jpeg",
        limitedDetail: true,
      });
      const derivative = await app.request(metadata.url);
      expect(derivative.status).toBe(200);
      expect(derivative.headers.get("content-type")).toBe("image/jpeg");
      expect(derivative.headers.get("cache-control")).toContain("immutable");
      const entityTag = derivative.headers.get("etag")!;
      expect(entityTag).toMatch(/^"[a-f0-9]{64}"$/);
      expect((await derivative.arrayBuffer()).byteLength).toBeGreaterThan(0);
      expect(
        (
          await app.request(metadata.url, {
            headers: { "If-None-Match": entityTag },
          })
        ).status,
      ).toBe(304);
    } finally {
      await application.shutdown();
    }
  });

  it("serves a bounded matching JPEG larger than the generic 16 MiB range-read limit", async () => {
    const { base, root } = await fixture();
    const image = await jpeg(80, 40);
    await writeFile(
      join(root, "large.jpg"),
      Buffer.concat([image, Buffer.alloc(17 * 1024 * 1024)]),
    );
    const application = await open(root, base);
    try {
      const id = application.listPhotos().photos[0]!.id;
      expect(await application.preview(id)).toMatchObject({
        state: "ready",
        source: "matching-jpeg",
      });
    } finally {
      await application.shutdown();
    }
  });

  it("rejects matching JPEG replacement between scan and descriptor-confined read", async () => {
    const { base, root } = await fixture();
    const path = join(root, "photo.jpg");
    await writeFile(path, await jpeg(80, 40));
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
          if (operation === "read" && !replaced) {
            replaced = true;
            await writeFile(path, await jpeg(100, 50, "#2040c0"));
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
    }
  });

  it("invalidates derivative identity after source change and rescan", async () => {
    const { base, root, web } = await fixture();
    const path = join(root, "photo.jpg");
    await writeFile(path, await jpeg(80, 40));
    const application = await open(root, base);
    try {
      const app = createHttpApp(application, web);
      const id = (
        (await (await app.request("/api/photos")).json()) as {
          photos: Array<{ id: string }>;
        }
      ).photos[0]!.id;
      const first = (await (
        await app.request(`/api/photos/${id}/preview`)
      ).json()) as { url: string };
      await new Promise((resolve) => setTimeout(resolve, 10));
      await writeFile(path, await jpeg(100, 50, "#2040c0"));
      await app.request("/api/scan", { method: "POST" });
      const second = (await (
        await app.request(`/api/photos/${id}/preview`)
      ).json()) as { url: string };
      expect(second.url).not.toBe(first.url);
      expect((await app.request(first.url)).status).toBe(404);
      expect((await stat(path)).isFile()).toBe(true);
    } finally {
      await application.shutdown();
    }
  });

  it("keeps a cross-source stale fallback labeled with the cached matching JPEG source", async () => {
    const { base, root, web } = await fixture();
    const jpegPath = join(root, "photo.jpg");
    await writeFile(jpegPath, await jpeg());
    await writeFile(join(root, "photo.ARW"), "not a usable RAW");
    const application = await open(root, base);
    try {
      const app = createHttpApp(application, web);
      const id = application.listPhotos().photos[0]!.id;
      const first = (await (
        await app.request(`/api/photos/${id}/preview`)
      ).json()) as { url: string; source: string };
      expect(first.source).toBe("matching-jpeg");
      await writeFile(jpegPath, "malformed replacement");
      await app.request("http://camera.local/api/scan", { method: "POST" });
      const stale = (await (
        await app.request(`/api/photos/${id}/preview`)
      ).json()) as {
        stale: boolean;
        source: string;
        url: string;
      };
      expect(stale).toMatchObject({
        stale: true,
        source: "matching-jpeg",
        url: first.url,
      });
      expect((await app.request(stale.url)).status).toBe(200);
    } finally {
      await application.shutdown();
    }
  });

  it("returns and labels the previous derivative explicitly stale when current replacement fails", async () => {
    const { base, root, web } = await fixture();
    const path = join(root, "photo.jpg");
    await writeFile(path, await jpeg());
    const application = await open(root, base);
    try {
      const app = createHttpApp(application, web);
      const id = application.listPhotos().photos[0]!.id;
      const first = (await (
        await app.request(`/api/photos/${id}/preview`)
      ).json()) as { url: string };
      await writeFile(path, "malformed replacement");
      await app.request("http://camera.local/api/scan", { method: "POST" });
      const stale = (await (
        await app.request(`/api/photos/${id}/preview`)
      ).json()) as {
        state: string;
        stale: boolean;
        url: string;
        message: string;
      };
      expect(stale).toMatchObject({
        state: "ready",
        stale: true,
        url: first.url,
      });
      expect(stale.message).toContain("stale");
      expect((await app.request(stale.url)).status).toBe(200);
    } finally {
      await application.shutdown();
    }
  });

  it("keeps a missing Photo in order, rejects unknown/traversal targets, and sanitizes errors", async () => {
    const { base, root, web } = await fixture();
    const original = join(root, "missing.jpg");
    await writeFile(original, await jpeg());
    const application = await open(root, base);
    try {
      const app = createHttpApp(application, web);
      const id = (
        (await (await app.request("/api/photos")).json()) as {
          photos: Array<{ id: string }>;
        }
      ).photos[0]!.id;
      await rm(original);
      await app.request("/api/scan", { method: "POST" });
      const listText = await (await app.request("/api/photos")).text();
      expect(listText).toContain(id);
      expect(listText).toContain("unavailable");
      const missing = await app.request(`/api/photos/${id}/preview`);
      expect(missing.status).toBe(404);
      expect(await missing.text()).not.toContain(root);
      expect(
        (await app.request("/api/photos/..%2F..%2Fetc/preview")).status,
      ).toBe(404);
      expect(
        (await app.request("/api/derivatives/not-an-id/../../secret.jpg"))
          .status,
      ).toBe(404);
      expect((await app.request("/../../etc/passwd")).status).toBe(200);
      expect(await (await app.request("/")).text()).toContain("built web");
    } finally {
      await application.shutdown();
    }
  });
});
