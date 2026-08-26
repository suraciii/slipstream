import { createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import sharp from "sharp";

import { SlipstreamApplication } from "./application.js";
import { createHttpApp } from "./http-server.js";
import { PhotoLibrary } from "./library/photo-library.js";
import { startupOptions } from "./main.js";
import { derivativeCacheKey } from "./preview/jpeg-derivative.js";

const root = resolve(import.meta.dirname, "../../../compatibility");
const digest = (value: string) =>
  createHash("sha256").update(value).digest("hex");

async function json<T>(path: string): Promise<T> {
  return JSON.parse(await readFile(resolve(root, path), "utf8")) as T;
}

function compactSql(value: unknown): string {
  if (typeof value !== "string") return "";
  return value
    .toLowerCase()
    .replace(/\s+/g, "")
    .replace(/["`[\]]/g, "");
}

function schemaManifest(database: DatabaseSync) {
  return {
    userVersion: database.prepare("PRAGMA user_version").get()?.user_version,
    objects: database
      .prepare(
        "SELECT type,name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
      )
      .all()
      .map(({ type, name, sql }) => ({
        type,
        name,
        sql: compactSql(sql),
        ...(type === "table"
          ? {
              columns: database
                .prepare(`PRAGMA table_info("${String(name)}")`)
                .all()
                .map((row) => ({
                  name: row.name,
                  type: row.type,
                  notNull: Boolean(row.notnull),
                  default: row.dflt_value,
                  primaryKey: row.pk,
                })),
              foreignKeys: database
                .prepare(`PRAGMA foreign_key_list("${String(name)}")`)
                .all()
                .map((row) => ({
                  table: row.table,
                  from: row.from,
                  to: row.to,
                  onUpdate: row.on_update,
                  onDelete: row.on_delete,
                })),
            }
          : {}),
      })),
  };
}

describe("shared Rust migration compatibility vectors", () => {
  it("parses the deterministic Preview fixture contract and resource defaults", async () => {
    const contract = await json<{
      schemaVersion: number;
      algorithmVersionGate: {
        required: string;
        selectedRustVersion: string | null;
      };
      targets: number[];
      limits: {
        inputBytes: number;
        decodedPixels: number;
        outputBytes: number;
        librawMemoryMb: number;
        queueConcurrency: number;
      };
      cases: Array<{
        name: string;
        kind: string;
        width?: number;
        height?: number;
        profile: string;
        orientations: number[];
      }>;
    }>("preview/fixtures.json");
    expect(contract.schemaVersion).toBe(1);
    expect(contract.algorithmVersionGate.required).toBe("fixture-matrix-pass");
    expect(contract.algorithmVersionGate.selectedRustVersion).toBe(
      "rust-vips-v1",
    );
    expect(contract.targets).toEqual([512, 2560]);
    expect(contract.limits).toEqual({
      inputBytes: 128 * 1024 * 1024,
      decodedPixels: 100_000_000,
      outputBytes: 64 * 1024 * 1024,
      librawMemoryMb: 256,
      queueConcurrency: 2,
    });
    expect(contract.cases.map((item) => item.name)).toEqual([
      "rgb-small-orientation-1-8",
      "rgb-large",
      "rgb-valid-icc",
      "rgb-invalid-icc",
      "cmyk",
      "corrupt-jpeg",
      "truncated-jpeg",
    ]);
    const orientationCase = contract.cases[0]!;
    expect(orientationCase.orientations).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
    expect(orientationCase.width).toBe(12);
    expect(orientationCase.height).toBe(8);
    expect(contract.cases.find((item) => item.kind === "cmyk")?.profile).toBe(
      "cmyk",
    );
    expect(
      contract.cases.find((item) => item.profile === "invalid")?.kind,
    ).toBe("rgb");
  });

  it("keeps protocol optional fields omitted instead of null", async () => {
    const responses = await json<unknown[]>("protocol/responses.json");
    expect(responses.length).toBeGreaterThanOrEqual(6);
    expect(JSON.stringify(responses)).not.toContain(":null");
  });

  it("matches deterministic identities in the current TypeScript authority", async () => {
    const contract = await json<{
      algorithmVersion: string;
      vectors: Array<{
        path: string;
        source: "matching-jpeg" | "embedded-raw-jpeg";
        size: number;
        mtimeMs: number;
        candidate: string | null;
        edge: 512 | 2560;
        originalId: string;
        photoId: string;
        sourceRevision: string;
        cacheKey: string;
        manifestIdentity: string;
      }>;
      paired: {
        rawOriginalId: string;
        jpegOriginalId: string;
        photoId: string;
      };
    }>("identity/vectors.json");

    expect(contract.algorithmVersion).toBe("sharp-v2");
    for (const vector of contract.vectors) {
      expect(digest(`original\0${vector.path}`)).toBe(vector.originalId);
      expect(digest(`photo\0${vector.originalId}`)).toBe(vector.photoId);
      expect(`${vector.path}\0${vector.size}\0${vector.mtimeMs}`).toBe(
        vector.sourceRevision,
      );
      expect(
        derivativeCacheKey({
          photoIdentity: vector.photoId,
          source: vector.source,
          sourceRelativePath: vector.path,
          sourceSize: vector.size,
          sourceMtimeMs: vector.mtimeMs,
          ...(vector.candidate
            ? { embeddedCandidateIdentity: vector.candidate }
            : {}),
          targetLongEdge: vector.edge,
        }),
      ).toBe(vector.cacheKey);
      expect(
        digest(JSON.stringify({ photo: vector.photoId, edge: vector.edge })),
      ).toBe(vector.manifestIdentity);
    }
    expect(
      digest(
        `photo\0${contract.paired.rawOriginalId}\0${contract.paired.jpegOriginalId}`,
      ),
    ).toBe(contract.paired.photoId);
  });

  it("keeps startup parsing compatible", async () => {
    const vectors = await json<
      Array<{
        environment: Record<string, string>;
        expected: ReturnType<typeof startupOptions>;
      }>
    >("startup/vectors.json");
    for (const vector of vectors)
      expect(startupOptions(vector.environment)).toEqual(vector.expected);
  });

  it("executes black-box protocol vectors against the current HTTP authority", async () => {
    const vectors = await json<
      Array<{
        request: {
          method: string;
          path: string;
          headers?: Record<string, string>;
          body?: unknown;
        };
        expected: {
          status: number;
          headers?: Record<string, string>;
          body?: unknown;
          bodyText?: string;
        };
      }>
    >("protocol/vectors.json");
    const base = await mkdtemp(join(tmpdir(), "slipstream-compat-protocol-"));
    const libraryRoot = join(base, "originals");
    const webRoot = join(base, "web");
    await mkdir(libraryRoot);
    await mkdir(webRoot);
    await writeFile(
      join(webRoot, "index.html"),
      "<main>compatibility web</main>",
    );
    const application = await SlipstreamApplication.open({
      libraryRoot,
      stateDirectory: join(base, "state"),
      databaseBasename: "library.sqlite",
      cacheDirectory: join(base, "cache"),
      host: "127.0.0.1",
      port: 3000,
    });
    try {
      const app = createHttpApp(application, webRoot);
      for (const vector of vectors) {
        const response = await app.request(
          `http://camera.local${vector.request.path}`,
          {
            method: vector.request.method,
            ...(vector.request.headers === undefined
              ? {}
              : { headers: vector.request.headers }),
            ...(vector.request.body === undefined
              ? {}
              : { body: JSON.stringify(vector.request.body) }),
          },
        );
        expect(response.status).toBe(vector.expected.status);
        for (const [name, value] of Object.entries(
          vector.expected.headers ?? {},
        ))
          expect(response.headers.get(name)).toBe(value);
        if (vector.expected.body !== undefined)
          expect(await response.json()).toEqual(vector.expected.body);
        if (vector.expected.bodyText !== undefined)
          expect(await response.text()).toBe(vector.expected.bodyText);
      }
    } finally {
      await application.shutdown();
      await rm(base, { recursive: true, force: true });
    }
  });

  it("executes shared derivative and immutable-cache vectors", async () => {
    const vectors = await json<
      Array<{
        setup: "matching-jpeg" | "web-asset";
        request: { path?: string };
        expected: {
          status: number;
          headers: Record<string, string>;
          etagPattern?: string;
          minimumBodyBytes?: number;
        };
        revalidation?: { header: string; expectedStatus: number };
      }>
    >("protocol/cache-vectors.json");
    const base = await mkdtemp(join(tmpdir(), "slipstream-compat-cache-"));
    const libraryRoot = join(base, "originals");
    const webRoot = join(base, "web");
    await mkdir(libraryRoot);
    await mkdir(join(webRoot, "assets"), { recursive: true });
    await writeFile(
      join(webRoot, "index.html"),
      "<main>compatibility web</main>",
    );
    await writeFile(join(webRoot, "assets/app.js"), "export default 'compat';");
    await writeFile(
      join(libraryRoot, "photo.jpg"),
      await sharp({
        create: { width: 8, height: 4, channels: 3, background: "#2040c0" },
      })
        .jpeg()
        .toBuffer(),
    );
    const application = await SlipstreamApplication.open({
      libraryRoot,
      stateDirectory: join(base, "state"),
      databaseBasename: "library.sqlite",
      cacheDirectory: join(base, "cache"),
      host: "127.0.0.1",
      port: 3000,
    });
    try {
      const app = createHttpApp(application, webRoot);
      for (const vector of vectors) {
        let path = vector.request.path;
        if (vector.setup === "matching-jpeg") {
          const photoId = application.listPhotos().photos[0]!.id;
          const preview = (await (
            await app.request(`/api/photos/${photoId}/preview`)
          ).json()) as { url: string };
          path = preview.url;
        }
        const response = await app.request(path!);
        expect(response.status).toBe(vector.expected.status);
        for (const [name, value] of Object.entries(vector.expected.headers))
          expect(response.headers.get(name)).toBe(value);
        const entityTag = response.headers.get("etag");
        if (vector.expected.etagPattern)
          expect(entityTag).toMatch(new RegExp(vector.expected.etagPattern));
        const bytes = await response.arrayBuffer();
        if (vector.expected.minimumBodyBytes !== undefined)
          expect(bytes.byteLength).toBeGreaterThanOrEqual(
            vector.expected.minimumBodyBytes,
          );
        if (vector.revalidation)
          expect(
            (
              await app.request(path!, {
                headers: { [vector.revalidation.header]: entityTag! },
              })
            ).status,
          ).toBe(vector.revalidation.expectedStatus);
      }
    } finally {
      await application.shutdown();
      await rm(base, { recursive: true, force: true });
    }
  });

  it("matches the exact canonical v2 schema from SQL and current initialization", async () => {
    const expected = await json<ReturnType<typeof schemaManifest>>(
      "sqlite/schema-v2.json",
    );
    const schemaDatabase = new DatabaseSync(":memory:");
    try {
      schemaDatabase.exec(
        await readFile(resolve(root, "sqlite/schema-v2.sql"), "utf8"),
      );
      expect(schemaManifest(schemaDatabase)).toEqual(expected);
      expect(schemaDatabase.prepare("PRAGMA foreign_key_check").all()).toEqual(
        [],
      );
    } finally {
      schemaDatabase.close();
    }

    const base = await mkdtemp(join(tmpdir(), "slipstream-compat-schema-"));
    const libraryRoot = join(base, "originals");
    const stateRoot = join(base, "state");
    await mkdir(libraryRoot);
    await mkdir(stateRoot, { mode: 0o700 });
    const databasePath = join(stateRoot, "library.sqlite");
    const library = await PhotoLibrary.open(libraryRoot, databasePath);
    await library.shutdown();
    const initialized = new DatabaseSync(databasePath);
    try {
      expect(schemaManifest(initialized)).toEqual(expected);
    } finally {
      initialized.close();
      await rm(base, { recursive: true, force: true });
    }
  });

  it("migrates shared v0 and v1 fixtures and rolls rejection fixtures back", async () => {
    for (const version of ["v0", "v1"] as const) {
      const base = await mkdtemp(
        join(tmpdir(), `slipstream-compat-${version}-`),
      );
      const libraryRoot = join(base, "originals");
      const stateRoot = join(base, "state");
      await mkdir(libraryRoot);
      await mkdir(stateRoot, { mode: 0o700 });
      const databasePath = join(stateRoot, "library.sqlite");
      const database = new DatabaseSync(databasePath);
      database.exec(
        await readFile(resolve(root, `sqlite/${version}.sql`), "utf8"),
      );
      database.close();
      const library = await PhotoLibrary.open(libraryRoot, databasePath);
      await library.shutdown();
      const migrated = new DatabaseSync(databasePath);
      expect(migrated.prepare("PRAGMA user_version").get()?.user_version).toBe(
        2,
      );
      expect(
        migrated.prepare("PRAGMA integrity_check").get()?.integrity_check,
      ).toBe("ok");
      expect(migrated.prepare("PRAGMA foreign_key_check").all()).toEqual([]);
      migrated.close();
      await rm(base, { recursive: true, force: true });
    }

    const rejections = await json<
      Array<{ sql: string; version: number; expectedError: string }>
    >("sqlite/rejections.json");
    for (const rejection of rejections) {
      const base = await mkdtemp(join(tmpdir(), "slipstream-compat-reject-"));
      const libraryRoot = join(base, "originals");
      const stateRoot = join(base, "state");
      await mkdir(libraryRoot);
      await mkdir(stateRoot, { mode: 0o700 });
      const databasePath = join(stateRoot, "library.sqlite");
      const database = new DatabaseSync(databasePath);
      database.exec(rejection.sql);
      const before = database
        .prepare("SELECT type,name,sql FROM sqlite_master ORDER BY type,name")
        .all();
      database.close();
      await expect(
        PhotoLibrary.open(libraryRoot, databasePath),
      ).rejects.toThrow(rejection.expectedError);
      const after = new DatabaseSync(databasePath);
      expect(after.prepare("PRAGMA user_version").get()?.user_version).toBe(
        rejection.version,
      );
      expect(
        after
          .prepare("SELECT type,name,sql FROM sqlite_master ORDER BY type,name")
          .all(),
      ).toEqual(before);
      after.close();
      await rm(base, { recursive: true, force: true });
    }
  });
});
