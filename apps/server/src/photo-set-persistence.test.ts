/* eslint-disable @typescript-eslint/no-explicit-any, @typescript-eslint/no-unsafe-assignment, @typescript-eslint/no-unsafe-call, @typescript-eslint/no-unsafe-member-access, @typescript-eslint/no-unsafe-return */
import {
  link,
  mkdtemp,
  mkdir,
  readFile,
  rm,
  unlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { afterEach, describe, expect, it } from "vitest";
import sharp from "sharp";

import { SlipstreamApplication } from "./application.js";
import { createHttpApp } from "./http-server.js";

const temporary: string[] = [];
afterEach(async () => {
  await Promise.all(
    temporary
      .splice(0)
      .map((path) => rm(path, { recursive: true, force: true })),
  );
});

async function fixture() {
  const base = await mkdtemp(join(tmpdir(), "slipstream-sets-"));
  temporary.push(base);
  const root = join(base, "originals");
  const web = join(base, "web");
  await mkdir(root);
  await mkdir(web);
  await writeFile(join(web, "index.html"), "built");
  for (const name of ["a.jpg", "b.jpg", "c.jpg"])
    await writeFile(
      join(root, name),
      await sharp({
        create: { width: 8, height: 4, channels: 3, background: "red" },
      })
        .jpeg()
        .toBuffer(),
    );
  return { base, root, web };
}

async function open(base: string, root: string) {
  return SlipstreamApplication.open({
    libraryRoot: root,
    stateDirectory: join(base, "state"),
    databaseBasename: "library.sqlite",
    cacheDirectory: join(base, "cache"),
    host: "127.0.0.1",
    port: 3000,
  });
}

async function post(
  app: ReturnType<typeof createHttpApp>,
  path: string,
  body: unknown,
  origin = "http://camera.local",
) {
  return app.request(`http://camera.local${path}`, {
    method: "POST",
    headers: { Origin: origin, "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

describe("Photo Set persistence protocol", () => {
  it("persists CRUD, ordered membership, global independent state, progress, and CAS undo", async () => {
    const { base, root, web } = await fixture();
    let application = await open(base, root);
    let app = createHttpApp(application, web);
    const ids = application.listPhotos().photos.map((photo) => photo.id);

    const created = await post(app, "/api/photo-sets", { name: " Picks " });
    expect(created.status).toBe(200);
    let sets = (await created.json()).photoSets;
    const setA = sets[0].id;
    await post(app, `/api/photo-sets/${setA}/members`, { photoIds: ids });
    await post(app, `/api/photo-sets/${setA}/order`, {
      photoIds: [ids[2], ids[0], ids[1]],
    });
    await post(app, `/api/photo-sets/${setA}/progress`, { photoId: ids[0] });
    const second = await post(app, "/api/photo-sets", { name: "Other" });
    const setB = ((await second.json()).photoSets as any[]).find(
      (set) => set.name === "Other",
    ).id;
    await post(app, `/api/photo-sets/${setB}/members`, { photoIds: [ids[0]] });

    const selected = await post(app, `/api/photos/${ids[0]}/state`, {
      field: "selectionState",
      value: "selected",
      photoSetId: setA,
    });
    const undo = (await selected.json()).undo;
    await post(app, `/api/photos/${ids[0]}/state`, {
      field: "rating",
      value: 4,
    });
    sets = (await (await app.request("/api/photo-sets")).json()).photoSets;
    expect(
      sets
        .find((set: any) => set.id === setA)
        .members.map((m: any) => m.photoId),
    ).toEqual([ids[2], ids[0], ids[1]]);
    expect(
      sets
        .filter((set: any) =>
          set.members.some((m: any) => m.photoId === ids[0]),
        )
        .every((set: any) => {
          const member = set.members.find((m: any) => m.photoId === ids[0]);
          return member.selectionState === "selected" && member.rating === 4;
        }),
    ).toBe(true);

    expect(
      (
        await post(app, `/api/photos/${ids[0]}/state`, {
          field: undo.field,
          value: undo.priorValue,
          expectedCurrent: undo.expectedCurrent,
        })
      ).status,
    ).toBe(200);
    await post(app, `/api/photos/${ids[0]}/state`, {
      field: "selectionState",
      value: "rejected",
    });
    expect(
      (
        await post(app, `/api/photos/${ids[0]}/state`, {
          field: "selectionState",
          value: "selected",
          expectedCurrent: "undecided",
        })
      ).status,
    ).toBe(409);

    await application.shutdown();
    application = await open(base, root);
    app = createHttpApp(application, web);
    sets = (await (await app.request("/api/photo-sets")).json()).photoSets;
    expect(sets.find((set: any) => set.id === setA).lastReviewedPhotoId).toBe(
      ids[0],
    );
    expect(
      application.listPhotos().photos.find((photo) => photo.id === ids[0]),
    ).toMatchObject({ selectionState: "rejected", rating: 4 });
    await rm(join(root, "a.jpg"));
    await application.rescan();
    expect(
      application.listPhotos().photos.find((photo) => photo.id === ids[0]),
    ).toMatchObject({
      available: false,
      selectionState: "rejected",
      rating: 4,
    });

    await post(app, `/api/photo-sets/${setA}/members/remove`, {
      photoId: ids[0],
    });
    sets = (await (await app.request("/api/photo-sets")).json()).photoSets;
    expect(
      sets.find((set: any) => set.id === setA).lastReviewedPhotoId,
    ).toBeUndefined();
    const beforeOriginal = await readFile(join(root, "b.jpg"));
    await post(app, `/api/photo-sets/${setA}/delete`, {});
    expect(await readFile(join(root, "b.jpg"))).toEqual(beforeOriginal);
    expect(application.listPhotos().photos).toHaveLength(3);
    await application.shutdown();
  });

  it("migrates a schema v1 database transactionally", async () => {
    const { base, root } = await fixture();
    const state = join(base, "state");
    await mkdir(state, { mode: 0o700 });
    const database = new DatabaseSync(join(state, "library.sqlite"));
    database.exec(`
      CREATE TABLE library_metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
      CREATE TABLE original_files(
        id TEXT PRIMARY KEY,relative_path TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL CHECK(kind IN ('raw','jpeg')),
        size INTEGER NOT NULL CHECK(size >= 0),mtime_ms REAL NOT NULL CHECK(mtime_ms >= 0),
        available INTEGER NOT NULL CHECK(available IN (0,1)),
        error_category TEXT CHECK(error_category IS NULL OR error_category IN ('unreadable','changed')),
        error_message TEXT CHECK(error_message IS NULL OR length(error_message) <= 120));
      CREATE TABLE photos(
        id TEXT PRIMARY KEY,raw_original_id TEXT REFERENCES original_files(id),jpeg_original_id TEXT REFERENCES original_files(id),
        ambiguous INTEGER NOT NULL CHECK(ambiguous IN (0,1)),available INTEGER NOT NULL CHECK(available IN (0,1)),
        preview_state TEXT NOT NULL CHECK(preview_state IN ('inspection-pending','ready','failed','unavailable')),
        preview_candidate TEXT CHECK(preview_candidate IS NULL OR preview_candidate IN ('matching-jpeg','embedded-raw-jpeg')),
        preview_source TEXT CHECK(preview_source IS NULL OR preview_source IN ('matching-jpeg','embedded-raw-jpeg')),
        preview_source_revision TEXT,preview_width INTEGER CHECK(preview_width IS NULL OR preview_width > 0),
        preview_height INTEGER CHECK(preview_height IS NULL OR preview_height > 0),cache_revision TEXT,sort_path TEXT NOT NULL);
      CREATE INDEX photos_raw ON photos(raw_original_id);
      CREATE INDEX photos_jpeg ON photos(jpeg_original_id);
      INSERT INTO library_metadata VALUES('canonical_root','${root.replaceAll("'", "''")}');
      PRAGMA user_version=1;
    `);
    database.close();
    const application = await open(base, root);
    await application.shutdown();
    const migrated = new DatabaseSync(join(state, "library.sqlite"));
    expect(migrated.prepare("PRAGMA user_version").get()).toMatchObject({
      user_version: 2,
    });
    expect(
      migrated
        .prepare("PRAGMA table_info(photos)")
        .all()
        .map((row) => (row as { name: string }).name),
    ).toEqual(expect.arrayContaining(["selection_state", "rating"]));
    expect(
      migrated
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .all()
        .map((row) => (row as { name: string }).name),
    ).toEqual(
      expect.arrayContaining([
        "photo_sets",
        "photo_set_members",
        "review_progress",
      ]),
    );
    migrated.close();
  });

  it("rejects malformed v1 schema without changing schema, rows, or version", async () => {
    const { base, root } = await fixture();
    const state = join(base, "state");
    await mkdir(state, { mode: 0o700 });
    const path = join(state, "library.sqlite");
    const database = new DatabaseSync(path);
    database.exec(`
      CREATE TABLE library_metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
      CREATE TABLE original_files(id TEXT PRIMARY KEY,relative_path TEXT UNIQUE,kind TEXT,size INTEGER,mtime_ms REAL,available INTEGER,error_category TEXT,error_message TEXT);
      CREATE TABLE photos(id TEXT PRIMARY KEY,raw_original_id TEXT,jpeg_original_id TEXT,ambiguous INTEGER,available INTEGER,preview_state TEXT,preview_candidate TEXT,preview_source TEXT,preview_source_revision TEXT,preview_width INTEGER,preview_height INTEGER,cache_revision TEXT,sort_path TEXT);
      CREATE INDEX photos_raw ON photos(raw_original_id); CREATE INDEX photos_jpeg ON photos(jpeg_original_id);
      INSERT INTO library_metadata VALUES('canonical_root','${root.replaceAll("'", "''")}');
      INSERT INTO original_files VALUES('bad','bad.jpg','jpeg',1,0,1,NULL,NULL);
      PRAGMA user_version=1;`);
    const before = database
      .prepare("SELECT type,name,sql FROM sqlite_master ORDER BY type,name")
      .all();
    database.close();
    await expect(open(base, root)).rejects.toThrow(/schema/);
    const after = new DatabaseSync(path);
    expect(after.prepare("PRAGMA user_version").get()).toMatchObject({
      user_version: 1,
    });
    expect(
      after.prepare("SELECT count(*) AS count FROM original_files").get(),
    ).toMatchObject({ count: 1 });
    expect(
      after
        .prepare("SELECT type,name,sql FROM sqlite_master ORDER BY type,name")
        .all(),
    ).toEqual(before);
    after.close();
  });

  it("admits SQLite sidecars before every new domain write", async () => {
    const { base, root } = await fixture();
    const application = await open(base, root);
    try {
      const target = join(base, "sidecar-target");
      await writeFile(target, "unchanged");
      const journal = join(base, "state", "library.sqlite-journal");
      await link(target, journal);
      await expect(application.createPhotoSet("Blocked")).rejects.toMatchObject(
        {
          kind: "persistence",
        },
      );
      expect((await readFile(target)).toString()).toBe("unchanged");
      await unlink(journal);
      expect(await application.listPhotoSets()).toEqual([]);
    } finally {
      await application.shutdown();
    }
  });

  it("bounds mutation JSON bodies before mutation while scan needs no JSON body", async () => {
    const { base, root, web } = await fixture();
    const application = await open(base, root);
    try {
      const app = createHttpApp(application, web);
      const declared = await app.request("http://camera.local/api/photo-sets", {
        method: "POST",
        headers: {
          Origin: "http://camera.local",
          "Content-Type": "application/json",
          "Content-Length": "65537",
        },
        body: JSON.stringify({ name: "Never" }),
      });
      expect(declared.status).toBe(413);
      const oversized = JSON.stringify({ name: "x".repeat(70_000) });
      const stream = new ReadableStream({
        start(controller) {
          controller.enqueue(new TextEncoder().encode(oversized));
          controller.close();
        },
      });
      const chunked = await app.fetch(
        new Request("http://camera.local/api/photo-sets", {
          method: "POST",
          headers: {
            Origin: "http://camera.local",
            "Content-Type": "application/json",
          },
          body: stream,
          duplex: "half",
        } as RequestInit & { duplex: "half" }),
      );
      expect(chunked.status).toBe(413);
      expect((await app.request("/api/scan", { method: "POST" })).status).toBe(
        200,
      );
      expect(await application.listPhotoSets()).toEqual([]);
    } finally {
      await application.shutdown();
    }
  });

  it("returns typed unknown, conflict, and persistence failures without partial progress", async () => {
    const { base, root, web } = await fixture();
    const application = await open(base, root);
    try {
      const app = createHttpApp(application, web);
      const ids = application.listPhotos().photos.map((photo) => photo.id);
      const created = await post(app, "/api/photo-sets", { name: "Safe" });
      const setId = (await created.json()).photoSets[0].id;
      expect(
        (await post(app, "/api/photo-sets", { name: "safe" })).status,
      ).toBe(409);
      expect(
        (
          await post(app, `/api/photo-sets/${setId}/members`, {
            photoIds: [ids[0], ids[0]],
          })
        ).status,
      ).toBe(400);
      expect(
        (
          await post(app, `/api/photo-sets/${setId}/members`, {
            photoIds: [ids[0], "a".repeat(64)],
          })
        ).status,
      ).toBe(404);
      expect(
        (
          await post(app, `/api/photo-sets/${setId}/members`, {
            photoIds: [ids[0]],
          })
        ).status,
      ).toBe(200);
      expect(
        (
          await post(app, `/api/photo-sets/${setId}/members`, {
            photoIds: [ids[0]],
          })
        ).status,
      ).toBe(409);
      expect(
        (await post(app, `/api/photo-sets/${setId}/order`, { photoIds: [] }))
          .status,
      ).toBe(409);
      expect(
        (
          await post(
            app,
            `/api/photo-sets/${setId}/members`,
            { photoIds: ids },
            "https://foreign.example",
          )
        ).status,
      ).toBe(403);
      expect(
        (
          await post(app, `/api/photos/${ids[0]}/state`, {
            field: "rating",
            value: 9,
          })
        ).status,
      ).toBe(400);
      expect(
        (
          await post(app, `/api/photos/${ids[0]}/state`, {
            field: "rating",
            value: 5,
            photoSetId: "a".repeat(64),
          })
        ).status,
      ).toBe(404);
      expect(
        (
          await post(app, `/api/photos/${ids[0]}/state`, {
            field: "rating",
            value: 1,
            expectedCurrent: 5,
          })
        ).status,
      ).toBe(409);
      expect(
        application.listPhotos().photos.find((photo) => photo.id === ids[0])
          ?.rating,
      ).toBe(0);
      const sets = (await (await app.request("/api/photo-sets")).json())
        .photoSets;
      expect(sets[0].members.map((member: any) => member.photoId)).toEqual([
        ids[0],
      ]);
      expect(sets[0].lastReviewedPhotoId).toBeUndefined();
    } finally {
      await application.shutdown();
    }
  });
});
