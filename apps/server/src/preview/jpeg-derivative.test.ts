import { createHash } from "node:crypto";
import {
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  stat,
  truncate,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import sharp from "sharp";
import { afterEach, describe, expect, it } from "vitest";
import {
  DerivativeScheduler,
  derivativeCacheKey,
  resetDerivativeSchedulerStateForTests,
  type DerivativeIdentity,
  type DerivativePriority,
} from "./jpeg-derivative.js";

const dirs: string[] = [];
const identity: DerivativeIdentity = {
  photoIdentity: "photo-1",
  sourceRelativePath: "set/IMG.JPG",
  sourceSize: 100,
  sourceMtimeMs: 1,
  targetLongEdge: 512,
};
afterEach(async () => {
  await Promise.all(
    dirs.splice(0).map((p) => rm(p, { recursive: true, force: true })),
  );
});

describe("Sharp JPEG derivatives", () => {
  it("does not upscale and embeds sRGB", async () => {
    const jpeg = await fixture(320, 200);
    const result = await generate(jpeg, identity);
    expect(result).toMatchObject({
      kind: "ready",
      width: 320,
      height: 200,
      colorProfile: "srgb",
    });
    if (result.kind === "ready")
      expect((await sharp(result.cachePath).metadata()).icc).toBeDefined();
  });

  it("preserves a valid compatible profile and converts an incompatible profile to sRGB", async () => {
    const source = await fixture(32, 16);
    const p3 = await sharp(source).withIccProfile("p3").jpeg().toBuffer();
    const preserved = await generate(p3, { ...identity, sourceMtimeMs: 30 });
    expect(preserved).toMatchObject({
      kind: "ready",
      colorProfile: "preserved-icc",
    });

    const cmyk = await sharp(source).withIccProfile("cmyk").jpeg().toBuffer();
    const converted = await generate(cmyk, { ...identity, sourceMtimeMs: 31 });
    expect(converted).toMatchObject({ kind: "ready", colorProfile: "srgb" });
    if (converted.kind === "ready") {
      const metadata = await sharp(converted.cachePath).metadata();
      expect(metadata.space).toBe("srgb");
      expect(metadata.channels).toBe(3);
    }
  });

  it("converts a valid profiled source to sRGB when profile preservation fails", async () => {
    const source = await fixture(18, 12);
    const profiled = await sharp(source).withIccProfile("p3").jpeg().toBuffer();
    const cache = await directory();
    const result = await new DerivativeScheduler(cache, {
      preserveProfile: () =>
        Promise.reject(new Error("injected preserve failure")),
    }).generate({ ...identity, sourceMtimeMs: 33 }, profiled);
    expect(result).toMatchObject({ kind: "ready", colorProfile: "srgb" });
    if (result.kind !== "ready") return;
    const actual = await sharp(result.cachePath).raw().toBuffer();
    const expected = await sharp(profiled)
      .autoOrient()
      .resize({
        width: 512,
        height: 512,
        fit: "inside",
        withoutEnlargement: true,
      })
      .toColourspace("srgb")
      .withIccProfile("srgb")
      .jpeg({ quality: 85, chromaSubsampling: "4:4:4", mozjpeg: true })
      .toBuffer();
    const expectedPixels = await sharp(expected).raw().toBuffer();
    for (const offset of [0, 15, actual.length - 3]) {
      for (let channel = 0; channel < 3; channel++)
        expect(
          Math.abs(
            actual[offset + channel]! - expectedPixels[offset + channel]!,
          ),
        ).toBeLessThan(3);
    }
  });

  it("rejects header-conforming but structurally invalid ICC instead of copying it", async () => {
    const jpeg = await fixture(32, 16);
    const profile = Buffer.alloc(132);
    profile.writeUInt32BE(profile.length, 0);
    profile.write("RGB ", 16, "ascii");
    profile.write("acsp", 36, "ascii");
    profile.writeUInt32BE(1, 128); // Declares a tag whose required entry bytes do not exist.
    const invalid = insertAppMarker(
      jpeg,
      0xe2,
      Buffer.concat([
        Buffer.from("ICC_PROFILE\0", "latin1"),
        Buffer.from([1, 1]),
        profile,
      ]),
    );
    const result = await generate(invalid, { ...identity, sourceMtimeMs: 32 });
    expect(result).toMatchObject({ kind: "ready", colorProfile: "srgb" });
    if (result.kind === "ready") {
      const metadata = await sharp(result.cachePath).metadata();
      expect(metadata.space).toBe("srgb");
      expect(metadata.icc).toBeDefined();
      expect(metadata.icc).not.toEqual(profile);
    }
  });

  it("uses anti-aliased downsampling", async () => {
    const raw = Buffer.alloc(256 * 256 * 3);
    for (let y = 0; y < 256; y++)
      for (let x = 0; x < 256; x++) {
        const v = (x + y) % 2 ? 255 : 0;
        raw.fill(v, (y * 256 + x) * 3, (y * 256 + x) * 3 + 3);
      }
    const jpeg = await sharp(raw, {
      raw: { width: 256, height: 256, channels: 3 },
    })
      .jpeg({ quality: 100, chromaSubsampling: "4:4:4" })
      .toBuffer();
    const result = await generate(
      jpeg,
      { ...identity, targetLongEdge: 512 },
      32,
    );
    expect(result.kind).toBe("ready");
    if (result.kind !== "ready") return;
    const { data } = await sharp(result.cachePath)
      .raw()
      .toBuffer({ resolveWithObject: true });
    const mean = data.reduce((a, b) => a + b, 0) / data.length;
    expect(mean).toBeGreaterThan(90);
    expect(mean).toBeLessThan(165);
    expect(Math.min(...data)).toBeGreaterThan(90);
    expect(Math.max(...data)).toBeLessThan(165);
  });

  for (let orientation = 1; orientation <= 8; orientation++) {
    it(`normalizes EXIF orientation ${orientation} exactly once`, async () => {
      const jpeg = await orientedFixture(orientation);
      const result = await generate(jpeg, identity);
      expect(result.kind).toBe("ready");
      if (result.kind !== "ready") return;
      const meta = await sharp(result.cachePath).metadata();
      expect(meta.orientation).toBeUndefined();
      const expected = await sharp(jpeg)
        .autoOrient()
        .raw()
        .toBuffer({ resolveWithObject: true });
      const actual = await sharp(result.cachePath)
        .raw()
        .toBuffer({ resolveWithObject: true });
      expect(actual.info.width).toBe(expected.info.width);
      expect(actual.info.height).toBe(expected.info.height);
      const points: Array<[number, number]> = [
        [1, 1],
        [expected.info.width - 2, 1],
        [1, expected.info.height - 2],
        [expected.info.width - 2, expected.info.height - 2],
      ];
      for (const [x, y] of points) {
        const offset = (y * expected.info.width + x) * 3;
        for (let c = 0; c < 3; c++)
          expect(
            Math.abs(actual.data[offset + c]! - expected.data[offset + c]!),
          ).toBeLessThan(35);
      }
    });
  }

  it("rejects malformed and oversized input and memoizes failure until retry", async () => {
    const cache = await directory();
    let runs = 0;
    const scheduler = new DerivativeScheduler(cache, {
      processJob: () => {
        runs++;
        return Promise.reject(new Error("malformed jpeg"));
      },
    });
    const bad = Buffer.from([1, 2, 3]);
    expect((await scheduler.generate(identity, bad)).kind).toBe("malformed");
    expect((await scheduler.generate(identity, bad)).kind).toBe("malformed");
    expect(runs).toBe(1);
    await scheduler.retry(identity, bad);
    expect(runs).toBe(2);
    expect(
      (
        await new DerivativeScheduler(await directory()).generate(
          identity,
          Buffer.alloc(128 * 1024 * 1024 + 1),
        )
      ).kind,
    ).toBe("resource-limit");
  });

  it("invalidates cache identity", () => {
    const key = derivativeCacheKey(identity);
    expect(derivativeCacheKey({ ...identity, sourceMtimeMs: 2 })).not.toBe(key);
    expect(derivativeCacheKey({ ...identity, targetLongEdge: 2560 })).not.toBe(
      key,
    );
  });

  it("coalesces across scheduler instances and bounds cache-hit processing", async () => {
    const cache = await directory();
    const jpeg = await fixture(64, 32);
    let active = 0,
      max = 0,
      runs = 0;
    const processJob = async () => {
      runs++;
      active++;
      max = Math.max(max, active);
      await delay(20);
      active--;
      return processed(jpeg, 64, 32);
    };
    const a = new DerivativeScheduler(cache, { concurrency: 1, processJob });
    const b = new DerivativeScheduler(cache, { concurrency: 1, processJob });
    const [x, y] = await Promise.all([
      a.generate(identity, jpeg),
      b.generate(identity, jpeg),
    ]);
    expect(x).toEqual(y);
    expect(runs).toBe(1);
    await Promise.all([a.generate(identity, jpeg), b.generate(identity, jpeg)]);
    expect(max).toBe(1);
  });

  it("promotes a queued duplicate without running it twice", async () => {
    const cache = await directory();
    const order: number[] = [];
    let release!: () => void;
    const blocker = new Promise<void>((resolve) => (release = resolve));
    const processJob = async (jpeg: Buffer) => {
      const label = jpeg[0]!;
      order.push(label);
      if (label === 1) await blocker;
      return processed(await fixture(8, 8), 8, 8);
    };
    const scheduler = new DerivativeScheduler(cache, {
      concurrency: 1,
      processJob,
    });
    const active = scheduler.generate(
      { ...identity, sourceMtimeMs: 1 },
      Buffer.from([1]),
    );
    await delay(5);
    const duplicateIdentity = { ...identity, sourceMtimeMs: 2 };
    const duplicateBackground = scheduler.generate(
      duplicateIdentity,
      Buffer.from([2]),
      {
        priority: "background",
      },
    );
    const adjacent = scheduler.generate(
      { ...identity, sourceMtimeMs: 3 },
      Buffer.from([3]),
      {
        priority: "adjacent",
      },
    );
    const background = scheduler.generate(
      { ...identity, sourceMtimeMs: 4 },
      Buffer.from([4]),
      {
        priority: "background",
      },
    );
    const duplicateCurrent = scheduler.generate(
      duplicateIdentity,
      Buffer.from([2]),
      {
        priority: "current",
      },
    );
    expect(duplicateCurrent).toBe(duplicateBackground);
    release();
    await Promise.all([active, duplicateBackground, adjacent, background]);
    expect(order).toEqual([1, 2, 3, 4]);
    expect(order.filter((value) => value === 2)).toHaveLength(1);
  });

  it("uses strict priority and oldest-first within a priority", async () => {
    const cache = await directory();
    const order: number[] = [];
    let release!: () => void;
    const blocker = new Promise<void>((r) => (release = r));
    const processJob = async (jpeg: Buffer) => {
      const label = jpeg[0]!;
      order.push(label);
      if (order.length === 1) await blocker;
      return processed(await fixture(8, 8), 8, 8);
    };
    const scheduler = new DerivativeScheduler(cache, {
      concurrency: 1,
      processJob,
    });
    const jobs: Array<[number, DerivativePriority]> = [
      [1, "background"],
      [2, "background"],
      [3, "current"],
      [4, "background"],
    ];
    const promises: Array<Promise<unknown>> = [];
    for (const [label, priority] of jobs) {
      promises.push(
        scheduler.generate(
          { ...identity, sourceMtimeMs: label },
          Buffer.from([label]),
          { priority },
        ),
      );
      await delay(1);
    }
    await delay(10);
    release();
    await Promise.all(promises);
    expect(order).toEqual([1, 3, 2, 4]);
  });

  it("keeps the newest admitted source authoritative when an older job finishes last", async () => {
    const cache = await directory();
    const source = await profiledFixture(20, 10);
    const older = { ...identity, sourceMtimeMs: 40 };
    const newer = { ...identity, sourceMtimeMs: 41 };
    let releaseOlder!: () => void;
    let olderStarted!: () => void;
    const olderGate = new Promise<void>((resolve) => (releaseOlder = resolve));
    const started = new Promise<void>((resolve) => (olderStarted = resolve));
    const scheduler = new DerivativeScheduler(cache, {
      concurrency: 2,
      processJob: async (jpeg) => {
        if (jpeg[0] === 1) {
          olderStarted();
          await olderGate;
        }
        return processed(source, 20, 10);
      },
    });

    const olderJob = scheduler.generate(older, Buffer.from([1]));
    await started;
    const newerResult = await scheduler.generate(newer, Buffer.from([2]));
    releaseOlder();
    const olderResult = await olderJob;
    expect(newerResult).toMatchObject({
      kind: "ready",
      cacheKey: derivativeCacheKey(newer),
      stale: false,
    });
    expect(olderResult).toMatchObject({
      kind: "ready",
      cacheKey: derivativeCacheKey(older),
    });

    const current = await new DerivativeScheduler(cache).generate(
      newer,
      source,
    );
    expect(current).toMatchObject({
      kind: "ready",
      cacheKey: derivativeCacheKey(newer),
      generated: false,
      stale: false,
    });
    const failing = new DerivativeScheduler(cache, {
      processJob: () => Promise.reject(new Error("injected failure")),
    });
    expect(
      await failing.generate({ ...newer, sourceMtimeMs: 42 }, source),
    ).toMatchObject({
      kind: "ready",
      cacheKey: derivativeCacheKey(newer),
      stale: true,
    });
  });

  it("prevents an older manifest repair from replacing a newer publication", async () => {
    const cache = await directory();
    const source = await profiledFixture(20, 10);
    const older = { ...identity, sourceMtimeMs: 50 };
    const newer = { ...identity, sourceMtimeMs: 51 };
    let releaseOlderRepair!: () => void;
    let olderRepairStarted!: () => void;
    const repairGate = new Promise<void>(
      (resolve) => (releaseOlderRepair = resolve),
    );
    const repairStarted = new Promise<void>(
      (resolve) => (olderRepairStarted = resolve),
    );
    let blockOlderRepair = true;
    const scheduler = new DerivativeScheduler(cache, {
      concurrency: 2,
      beforeManifestRename: async (_temporary, finalPath) => {
        if (blockOlderRepair && finalPath.includes("manifests")) {
          blockOlderRepair = false;
          olderRepairStarted();
          await repairGate;
        }
      },
    });

    const olderJob = scheduler.generate(older, source);
    await repairStarted;
    const newerJob = scheduler.generate(newer, source);
    await delay(5);
    releaseOlderRepair();
    const [olderResult, newerResult] = await Promise.all([olderJob, newerJob]);
    expect(olderResult).toMatchObject({
      kind: "ready",
      cacheKey: derivativeCacheKey(older),
    });
    expect(newerResult).toMatchObject({
      kind: "ready",
      cacheKey: derivativeCacheKey(newer),
    });

    const current = await new DerivativeScheduler(cache).generate(
      newer,
      source,
    );
    expect(current).toMatchObject({
      kind: "ready",
      cacheKey: derivativeCacheKey(newer),
      generated: false,
      stale: false,
    });
  });

  it("returns a previous identity explicitly stale when replacement fails", async () => {
    const cache = await directory();
    const jpeg = await fixture(20, 10);
    const scheduler = new DerivativeScheduler(cache);
    const first = await scheduler.generate(identity, jpeg);
    expect(first).toMatchObject({ kind: "ready", stale: false });
    const changed = { ...identity, sourceMtimeMs: 2 };
    const failing = new DerivativeScheduler(cache, {
      processJob: () => Promise.reject(new Error("injected failure")),
    });
    expect(await failing.generate(changed, jpeg)).toMatchObject({
      kind: "ready",
      stale: true,
      cacheKey: derivativeCacheKey(identity),
    });
    expect(await failing.generate(changed, jpeg)).toMatchObject({
      kind: "ready",
      stale: true,
      cacheKey: derivativeCacheKey(identity),
    });
  });

  it("regenerates when manifest is missing, malformed, mismatched, or path-injecting", async () => {
    const cases: unknown[] = [
      undefined,
      "not-json",
      { key: "0".repeat(64), width: 20, height: 10, colorProfile: "srgb" },
      {
        key: derivativeCacheKey(identity),
        path: "/tmp/escape.jpg",
        width: 20,
        height: 10,
        colorProfile: "srgb",
      },
    ];
    for (let index = 0; index < cases.length; index++) {
      const cache = await directory();
      const jpeg = await profiledFixture(20, 10);
      const key = derivativeCacheKey(identity);
      await writeFile(join(cache, `${key}.jpg`), jpeg);
      if (cases[index] !== undefined) {
        const manifests = join(cache, "metadata", "manifests");
        await mkdir(manifests, { recursive: true });
        const path = join(manifests, `${photoManifestKey(identity)}.json`);
        await writeFile(
          path,
          typeof cases[index] === "string"
            ? String(cases[index])
            : JSON.stringify(cases[index]),
        );
      }
      let runs = 0;
      const scheduler = new DerivativeScheduler(cache, {
        processJob: () => {
          runs++;
          return Promise.resolve({
            jpeg,
            width: 20,
            height: 10,
            colorProfile: "srgb",
          });
        },
      });
      expect(await scheduler.generate(identity, jpeg)).toMatchObject({
        kind: "ready",
        generated: true,
      });
      expect(runs).toBe(1);
    }
  });

  it("persists terminal content failure across scheduler reconstruction until retry or invalidate", async () => {
    const cache = await directory();
    const bad = Buffer.from([1, 2, 3]);
    let runs = 0;
    const failing = () => {
      runs++;
      return Promise.reject(new Error("malformed jpeg /private/sample.jpg"));
    };
    expect(
      (
        await new DerivativeScheduler(cache, { processJob: failing }).generate(
          identity,
          bad,
        )
      ).kind,
    ).toBe("malformed");
    resetDerivativeSchedulerStateForTests(cache);
    expect(
      (
        await new DerivativeScheduler(cache, { processJob: failing }).generate(
          identity,
          bad,
        )
      ).kind,
    ).toBe("malformed");
    expect(runs).toBe(1);
    const record = JSON.parse(
      await readFile(
        join(
          cache,
          "metadata",
          "failures",
          `${derivativeCacheKey(identity)}.json`,
        ),
        "utf8",
      ),
    ) as unknown;
    if (
      !record ||
      typeof record !== "object" ||
      !("kind" in record) ||
      !("message" in record)
    )
      throw new Error("invalid failure record");
    expect(record.kind).toBe("malformed");
    expect(typeof record.message).toBe("string");
    expect(String(record.message)).not.toContain("/private");
    await new DerivativeScheduler(cache).invalidate(identity);
    resetDerivativeSchedulerStateForTests(cache);
    await new DerivativeScheduler(cache, { processJob: failing }).generate(
      identity,
      bad,
      { retry: true },
    );
    expect(runs).toBe(2);
  });

  it("keeps failure persistence fully best-effort", async () => {
    const bad = Buffer.from([1, 2, 3]);
    const cache = await directory();
    const result = await new DerivativeScheduler(cache, {
      processJob: () => Promise.reject(new Error("malformed jpeg")),
      beforeFailurePersist: () => Promise.reject(new Error("mkdir denied")),
    }).generate(identity, bad);
    expect(result).toMatchObject({ kind: "malformed" });

    const staleCache = await directory();
    const jpeg = await fixture(20, 10);
    await new DerivativeScheduler(staleCache).generate(identity, jpeg);
    const stale = await new DerivativeScheduler(staleCache, {
      processJob: () => Promise.reject(new Error("malformed jpeg")),
      beforeFailurePersist: () => Promise.reject(new Error("write denied")),
    }).generate({ ...identity, sourceMtimeMs: 99 }, jpeg);
    expect(stale).toMatchObject({ kind: "ready", stale: true });
  });

  it("serves a published JPEG when manifest write or rename fails and repairs metadata later", async () => {
    for (const stage of ["write", "rename"] as const) {
      const cache = await directory();
      const jpeg = await fixture(20, 10);
      const options =
        stage === "write"
          ? {
              beforeManifestWrite: () =>
                Promise.reject(new Error("manifest write failed")),
            }
          : {
              beforeManifestRename: () =>
                Promise.reject(new Error("manifest rename failed")),
            };
      const first = new DerivativeScheduler(cache, options);
      expect(await first.generate(identity, jpeg)).toMatchObject({
        kind: "ready",
        generated: true,
        stale: false,
      });
      const second = new DerivativeScheduler(cache);
      expect(await second.generate(identity, jpeg)).toMatchObject({
        kind: "ready",
        generated: false,
        stale: false,
      });
      const manifests = join(cache, "metadata", "manifests");
      expect(
        (await readdir(manifests)).some((name) => name.endsWith(".json")),
      ).toBe(true);
    }
  });

  it("rejects an oversized sparse cached JPEG before reading it", async () => {
    const cache = await directory();
    const key = derivativeCacheKey(identity);
    const path = join(cache, `${key}.jpg`);
    await writeFile(path, Buffer.from([0xff, 0xd8]));
    await truncate(path, 64 * 1024 * 1024 + 1);
    const before = await stat(path);
    let runs = 0;
    const jpeg = await profiledFixture(20, 10);
    const scheduler = new DerivativeScheduler(cache, {
      processJob: () => {
        runs++;
        return Promise.resolve({
          jpeg,
          width: 20,
          height: 10,
          colorProfile: "srgb",
        });
      },
    });
    expect(await scheduler.generate(identity, jpeg)).toMatchObject({
      kind: "ready",
      generated: true,
    });
    expect(runs).toBe(1);
    expect(before.size).toBeGreaterThan(64 * 1024 * 1024);
  });

  it("rejects corrupt cache bytes and atomically cleans failed publication", async () => {
    const cache = await directory();
    const jpeg = await fixture(20, 10);
    const path = join(cache, `${derivativeCacheKey(identity)}.jpg`);
    await writeFile(path, Buffer.from("not jpeg"));
    const scheduler = new DerivativeScheduler(cache, {
      beforePublish: () => Promise.reject(new Error("publish failed")),
    });
    expect((await scheduler.generate(identity, jpeg)).kind).not.toBe("ready");
    expect((await readdir(cache)).filter((x) => x.endsWith(".tmp"))).toEqual(
      [],
    );
  });
});

async function generate(jpeg: Buffer, id: DerivativeIdentity, target?: number) {
  const cache = await directory();
  return new DerivativeScheduler(cache).generate(
    target ? { ...id, targetLongEdge: target as 512 | 2560 } : id,
    jpeg,
  );
}
async function fixture(width: number, height: number) {
  return sharp({
    create: {
      width,
      height,
      channels: 3,
      background: { r: 60, g: 120, b: 200 },
    },
  })
    .jpeg({ quality: 100, chromaSubsampling: "4:4:4" })
    .toBuffer();
}
async function profiledFixture(width: number, height: number) {
  return sharp(await fixture(width, height))
    .withIccProfile("srgb")
    .jpeg()
    .toBuffer();
}

async function orientedFixture(orientation: number) {
  const raw = Buffer.alloc(12 * 8 * 3);
  const colors = [
    [240, 20, 20],
    [20, 240, 20],
    [20, 20, 240],
    [240, 220, 20],
  ];
  for (let y = 0; y < 8; y++)
    for (let x = 0; x < 12; x++) {
      const q = (y >= 4 ? 2 : 0) + (x >= 6 ? 1 : 0),
        o = (y * 12 + x) * 3;
      raw[o] = colors[q]![0]!;
      raw[o + 1] = colors[q]![1]!;
      raw[o + 2] = colors[q]![2]!;
    }
  return sharp(raw, { raw: { width: 12, height: 8, channels: 3 } })
    .jpeg({ quality: 100, chromaSubsampling: "4:4:4" })
    .withMetadata({ orientation })
    .toBuffer();
}
function photoManifestKey(id: DerivativeIdentity): string {
  return createHash("sha256")
    .update(
      JSON.stringify({ photo: id.photoIdentity, edge: id.targetLongEdge }),
    )
    .digest("hex");
}

function insertAppMarker(
  jpeg: Buffer,
  marker: number,
  payload: Buffer,
): Buffer {
  const header = Buffer.alloc(4);
  header[0] = 0xff;
  header[1] = marker;
  header.writeUInt16BE(payload.length + 2, 2);
  return Buffer.concat([
    jpeg.subarray(0, 2),
    header,
    payload,
    jpeg.subarray(2),
  ]);
}
function processed(jpeg: Buffer, width: number, height: number) {
  return Promise.resolve({
    jpeg,
    width,
    height,
    colorProfile: "srgb" as const,
  });
}
async function directory() {
  const p = await mkdtemp(join(tmpdir(), "slipstream-derivative-"));
  dirs.push(p);
  return p;
}
function delay(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}
