import { DatabaseSync } from "node:sqlite";
import {
  chmod,
  link,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rename,
  rm,
  symlink,
  unlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { classifyOriginalFile } from "./file-kinds.js";
import {
  PhotoLibrary,
  type PhotoRecord,
  type ScanResult,
} from "./photo-library.js";

const temporary: string[] = [];
afterEach(async () => {
  await Promise.all(
    temporary
      .splice(0)
      .map((path) => rm(path, { recursive: true, force: true })),
  );
});
async function fixture() {
  const base = await mkdtemp(join(tmpdir(), "slipstream-library-"));
  temporary.push(base);
  const root = join(base, "originals");
  await mkdir(root);
  return { base, root, db: join(base, "state", "library.sqlite") };
}
async function put(
  root: string,
  relative: string,
  contents: string | Uint8Array = relative,
) {
  const path = join(root, relative);
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, contents);
  return path;
}
function available(result: ScanResult) {
  return result.originals
    .filter((item) => item.available)
    .map((item) => item.relativePath);
}
function sourceRevision(result: ScanResult, path: string) {
  const item = result.originals.find((value) => value.relativePath === path)!;
  return `${item.relativePath}\0${item.size}\0${item.mtimeMs}`;
}
async function openLibrary(root: string, db: string, options = {}) {
  return PhotoLibrary.open(root, db, options);
}

async function snapshot(root: string): Promise<unknown[]> {
  const rows: unknown[] = [];
  async function walk(relative = "") {
    const names = await readdir(join(root, relative));
    names.sort();
    for (const name of names) {
      const rel = relative ? `${relative}/${name}` : name;
      const facts = await lstat(join(root, rel));
      rows.push({
        path: rel,
        type: facts.isSymbolicLink()
          ? "link"
          : facts.isDirectory()
            ? "directory"
            : "file",
        size: facts.size,
        mtimeMs: facts.mtimeMs,
        mode: facts.mode,
        bytes: facts.isFile()
          ? (await readFile(join(root, rel))).toString("hex")
          : undefined,
      });
      if (facts.isDirectory()) await walk(rel);
    }
  }
  await walk();
  return rows;
}

describe("Photo Library indexing", () => {
  it("classifies extensions case-insensitively but preserves stem case for pairing", async () => {
    expect(classifyOriginalFile("A.ArW")).toBe("raw");
    const { root, db } = await fixture();
    await put(root, "same.ArW");
    await put(root, "same.JpEg");
    await put(root, "Case.ARW");
    await put(root, "case.JPG");
    const library = await openLibrary(root, db);
    const result = await library.scan();
    expect(
      result.photos.filter(
        (photo) => photo.rawOriginalId && photo.jpegOriginalId,
      ),
    ).toHaveLength(1);
    expect(result.photos).toHaveLength(3);
    await library.shutdown();
  });

  it("discovers deterministically and leaves Preview source unset while inspection is pending", async () => {
    const { root, db } = await fixture();
    await put(root, "z/two.nef");
    await put(root, "a/one.ARW");
    await put(root, "a/one.jpg");
    await put(root, "notes.txt");
    const library = await openLibrary(root, db);
    const result = await library.scan();
    expect(available(result)).toEqual(["a/one.ARW", "a/one.jpg", "z/two.nef"]);
    expect(result.photos[0]).toMatchObject({
      previewState: "inspection-pending",
      previewCandidate: "matching-jpeg",
    });
    expect(result.photos[0]!.previewSource).toBeUndefined();
    await library.shutdown();
  });

  it("preserves a Photo ID when RAW-only gains JPEG and when JPEG-only gains RAW", async () => {
    for (const first of ["one.ARW", "one.JPG"]) {
      const { root, db } = await fixture();
      await put(root, first);
      const library = await openLibrary(root, db);
      const initial = await library.scan();
      const id = initial.photos[0]!.id;
      await put(root, first.endsWith("ARW") ? "one.JPG" : "one.ARW");
      const paired = await library.scan();
      expect(paired.photos.find((photo) => photo.available)).toMatchObject({
        id,
        ambiguous: false,
      });
      const pairedPhoto = paired.photos.find((photo) => photo.id === id)!;
      expect(typeof pairedPhoto.rawOriginalId).toBe("string");
      expect(typeof pairedPhoto.jpegOriginalId).toBe("string");
      await library.shutdown();
    }
  });

  it("preserves paired identity and references while one member is missing and reappears", async () => {
    const { root, db } = await fixture();
    await put(root, "one.ARW");
    const jpeg = await put(root, "one.JPG");
    const library = await openLibrary(root, db);
    const paired = await library.scan();
    const photo = paired.photos[0]!;
    await unlink(jpeg);
    const missing = await library.scan();
    expect(missing.photos.find((item) => item.id === photo.id)).toMatchObject({
      id: photo.id,
      rawOriginalId: photo.rawOriginalId,
      jpegOriginalId: photo.jpegOriginalId,
      available: true,
    });
    await put(root, "one.JPG");
    expect(
      (await library.scan()).photos.find((item) => item.id === photo.id),
    ).toMatchObject({ id: photo.id, available: true });
    await library.shutdown();
  });

  it("preserves the prior pair and marks it ambiguous when a conflicting member appears", async () => {
    const { root, db } = await fixture();
    await put(root, "one.ARW");
    await put(root, "one.JPG");
    const library = await openLibrary(root, db);
    const id = (await library.scan()).photos[0]!.id;
    await put(root, "one.RAF");
    const result = await library.scan();
    expect(result.photos.find((photo) => photo.id === id)).toMatchObject({
      id,
      ambiguous: true,
    });
    expect(result.photos.filter((photo) => photo.available)).toHaveLength(2);
    await library.shutdown();
  });

  it("preserves inspected Preview facts on unchanged rescan and invalidates changed selected source", async () => {
    const { root, db } = await fixture();
    await put(root, "one.ARW");
    const jpeg = await put(root, "one.JPG", "jpeg");
    const library = await openLibrary(root, db);
    const first = await library.scan();
    const photo = first.photos[0]!;
    await library.seedInspectedPreview({
      photoId: photo.id,
      state: "ready",
      expectedCandidate: "matching-jpeg",
      expectedSourceRevision: sourceRevision(first, "one.JPG"),
      width: 100,
      height: 50,
      cacheRevision: "v1",
    });
    const unchanged = await library.scan();
    expect(unchanged.photos.find((item) => item.id === photo.id)).toMatchObject(
      {
        previewState: "ready",
        previewSource: "matching-jpeg",
        cacheRevision: "v1",
      },
    );
    await new Promise((resolve) => setTimeout(resolve, 5));
    await writeFile(jpeg, "changed jpeg");
    const changed = await library.scan();
    const changedPhoto = changed.photos.find((item) => item.id === photo.id)!;
    expect(changedPhoto).toMatchObject({
      previewState: "inspection-pending",
      previewCandidate: "matching-jpeg",
    });
    expect(changedPhoto.previewSource).toBeUndefined();
    await library.shutdown();
  });

  it.each(["symlink", "hardlink"] as const)(
    "admits SQLite sidecars immediately before Preview CAS writes and rejects an unsafe journal %s",
    async (replacement) => {
      const { root, db } = await fixture();
      const jpeg = await put(root, "one.JPG", "irreplaceable-preview-source");
      const library = await openLibrary(root, db);
      const first = await library.scan();
      const photo = first.photos[0]!;
      const before = await snapshot(root);
      const journal = `${db}-journal`;
      if (replacement === "symlink") await symlink(jpeg, journal);
      else await link(jpeg, journal);

      await expect(
        library.seedInspectedPreview({
          photoId: photo.id,
          state: "ready",
          expectedCandidate: "matching-jpeg",
          expectedSourceRevision: sourceRevision(first, "one.JPG"),
          width: 100,
          height: 50,
          cacheRevision: "v1",
        }),
      ).rejects.toThrow(/sidecar|safely owned|database/);

      // Remove the hostile filesystem entry before querying SQLite again. The
      // rejected CAS must not have changed either persisted Preview state or
      // the linked Original inode.
      await unlink(journal);
      await library.refresh();
      expect(library.read().photos[0]).toMatchObject({
        id: photo.id,
        previewState: "inspection-pending",
        previewCandidate: "matching-jpeg",
      });
      expect(library.read().photos[0]!.previewSource).toBeUndefined();
      expect(await snapshot(root)).toEqual(before);
      await library.shutdown();
    },
  );

  it("persists across reopen, migrates version 0, initializes version 1, rejects newer versions and invalid rows", async () => {
    const { root, db } = await fixture();
    await put(root, "one.JPG");
    let library = await openLibrary(root, db);
    const first = await library.scan();
    await library.shutdown();
    library = await openLibrary(root, db);
    await library.refresh();
    expect(library.read().photos[0]!.id).toBe(first.photos[0]!.id);
    await library.shutdown();
    const database = new DatabaseSync(db);
    expect(database.prepare("PRAGMA user_version").get()).toMatchObject({
      user_version: 1,
    });
    expect(() =>
      database.exec(
        "INSERT INTO original_files VALUES('x','x','jpeg',-1,0,1,NULL,NULL)",
      ),
    ).toThrow();
    database.exec("PRAGMA user_version=99");
    database.close();
    await expect(openLibrary(root, db)).rejects.toThrow(/newer/);

    const legacy = join(dirname(db), "legacy.sqlite");
    const old = new DatabaseSync(legacy);
    old.exec(`CREATE TABLE library_metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
      CREATE TABLE original_files(id TEXT PRIMARY KEY,relative_path TEXT UNIQUE,kind TEXT,size INTEGER,mtime_ms REAL,available INTEGER,inspection_error TEXT);
      CREATE TABLE photos(id TEXT PRIMARY KEY,raw_original_id TEXT,jpeg_original_id TEXT,ambiguous INTEGER,available INTEGER,preview_state TEXT,preview_source TEXT,sort_path TEXT);`);
    old.close();
    const migrated = await openLibrary(root, legacy);
    await migrated.shutdown();
    const migratedDb = new DatabaseSync(legacy);
    expect(migratedDb.prepare("PRAGMA user_version").get()).toMatchObject({
      user_version: 1,
    });
    expect(
      migratedDb
        .prepare("PRAGMA table_info(photos)")
        .all()
        .map((row) => (row as { name: string }).name),
    ).toContain("preview_candidate");
    expect(() =>
      migratedDb.exec(
        "INSERT INTO original_files VALUES('x','x','jpeg',-1,0,1,NULL,NULL)",
      ),
    ).toThrow();
    expect(migratedDb.prepare("PRAGMA integrity_check").get()).toMatchObject({
      integrity_check: "ok",
    });
    expect(migratedDb.prepare("PRAGMA foreign_key_check").all()).toEqual([]);
    migratedDb.close();
  });

  it("rejects malformed version-0 shape without changing schema or version", async () => {
    const { root, db } = await fixture();
    await mkdir(dirname(db), { recursive: true, mode: 0o700 });
    await chmod(dirname(db), 0o700);
    const malformed = new DatabaseSync(db);
    malformed.exec(
      "CREATE TABLE original_files(id TEXT PRIMARY KEY); CREATE TABLE photos(id TEXT PRIMARY KEY);",
    );
    const before = malformed
      .prepare("SELECT type,name,sql FROM sqlite_master ORDER BY type,name")
      .all();
    malformed.close();
    await expect(openLibrary(root, db)).rejects.toThrow(/legacy schema/);
    const after = new DatabaseSync(db);
    expect(after.prepare("PRAGMA user_version").get()).toMatchObject({
      user_version: 0,
    });
    expect(
      after
        .prepare("SELECT type,name,sql FROM sqlite_master ORDER BY type,name")
        .all(),
    ).toEqual(before);
    after.close();
  });

  it("ignores stale Preview completion after source mutation or candidate replacement", async () => {
    const { root, db } = await fixture();
    const raw = await put(root, "one.ARW", "raw-a");
    const library = await openLibrary(root, db);
    let result = await library.scan();
    const photo = result.photos[0]!;
    const rawRevision = sourceRevision(result, "one.ARW");
    await writeFile(raw, "raw-b");
    await library.scan();
    await expect(
      library.seedInspectedPreview({
        photoId: photo.id,
        state: "ready",
        expectedCandidate: "embedded-raw-jpeg",
        expectedSourceRevision: rawRevision,
      }),
    ).resolves.toEqual({ kind: "stale-ignored" });
    expect(
      library.read().photos.find((item) => item.id === photo.id)?.previewState,
    ).toBe("inspection-pending");
    result = await library.scan();
    const currentRawRevision = sourceRevision(result, "one.ARW");
    await put(root, "one.JPG", "jpeg");
    await library.scan();
    await expect(
      library.seedInspectedPreview({
        photoId: photo.id,
        state: "ready",
        expectedCandidate: "embedded-raw-jpeg",
        expectedSourceRevision: currentRawRevision,
      }),
    ).resolves.toEqual({ kind: "stale-ignored" });
    expect(
      library.read().photos.find((item) => item.id === photo.id)
        ?.previewCandidate,
    ).toBe("matching-jpeg");
    await library.shutdown();
  });

  it("orders confined capability commands and bounds range reads off the event loop", async () => {
    const { root, db } = await fixture();
    await put(root, "large.JPG", Buffer.alloc(1024 * 1024, 7));
    const library = await openLibrary(root, db);
    const original = library.confinedOriginal("large.JPG");
    let ticked = false;
    setTimeout(() => {
      ticked = true;
    }, 0);
    const reads = Promise.all([
      original.read(0, 512 * 1024),
      original.facts(),
      original.read(512 * 1024, 512 * 1024),
    ]);
    await new Promise((resolve) => setTimeout(resolve, 5));
    expect(ticked).toBe(true);
    const [first, facts, second] = await reads;
    expect(first).toHaveLength(512 * 1024);
    expect(second).toHaveLength(512 * 1024);
    expect((facts as { size: number }).size).toBe(1024 * 1024);
    await expect(original.read(0, 16 * 1024 * 1024 + 1)).rejects.toThrow(
      /range/,
    );
    await expect(original.read(0, 2 ** 32 + 1)).rejects.toThrow(/range/);
    await library.shutdown();
  });

  it("returns immutable snapshots and rejects captured capabilities after shutdown", async () => {
    const { root, db } = await fixture();
    await put(root, "photo..final.JPG", "backslash-safe");
    await put(root, "a\\b.JPG", "slash");
    const library = await openLibrary(root, db);
    const result = await library.scan();
    expect(() => (result.photos as unknown as PhotoRecord[]).pop()).toThrow();
    expect(library.read().photos.length).toBe(result.photos.length);
    expect(
      (await library.confinedOriginal("a\\b.JPG").read(0, 5)).toString(),
    ).toBe("slash");
    expect(() => library.confinedOriginal("bad\0name.JPG")).toThrow();
    const captured = library.confinedOriginal("photo..final.JPG");
    const queued = captured.read(0, 4);
    const shutdown = library.shutdown();
    await expect(queued).resolves.toHaveLength(4);
    await shutdown;
    await expect(captured.facts()).rejects.toThrow(/closed/);
  });

  it("rejects startup failure instead of hanging", async () => {
    const { root, db } = await fixture();
    await expect(
      openLibrary(root, db, { startupFailure: true }),
    ).rejects.toThrow(/startup|worker/i);
  });

  it("rolls back actual partial mutation", async () => {
    const { root, db } = await fixture();
    await put(root, "one.JPG");
    const library = await openLibrary(root, db, {
      failureHook: "after-first-original",
    });
    await expect(library.scan()).rejects.toThrow(/injected/);
    await library.refresh();
    expect(library.read().originals).toEqual([]);
    await library.shutdown();
  });

  it("runs scanning off the event loop, coalesces scans, and shutdown waits safely", async () => {
    const { root, db } = await fixture();
    for (let i = 0; i < 1000; i++) await put(root, `set/${i}.JPG`);
    const library = await openLibrary(root, db);
    const first = library.scan();
    expect(library.scan()).toBe(first);
    let ticked = false;
    setTimeout(() => {
      ticked = true;
    }, 0);
    await new Promise((resolve) => setTimeout(resolve, 5));
    expect(ticked).toBe(true);
    const shutdown = library.shutdown();
    await expect(first).resolves.toBeDefined();
    await expect(shutdown).resolves.toBeUndefined();
    await expect(library.shutdown()).resolves.toBeUndefined();
  });

  it("validates the per-directory limit without uint32 narrowing", async () => {
    const { root, db } = await fixture();
    const accepted = await openLibrary(root, db, {
      maximumEntriesPerDirectory: 0xffffffff,
    });
    await accepted.shutdown();
    await expect(
      openLibrary(root, join(dirname(db), "too-large.sqlite"), {
        maximumEntriesPerDirectory: 0x100000000,
      }),
    ).rejects.toThrow(/32-bit/);
  });

  it("enforces recognized, total and per-directory entry limits early", async () => {
    for (const options of [
      { maximumFiles: 1, names: ["1.JPG", "2.JPG"], pattern: /recognized/ },
      { maximumEntries: 1, names: ["1.txt", "2.txt"], pattern: /total/ },
      {
        maximumEntriesPerDirectory: 1,
        names: ["1.txt", "2.txt"],
        pattern: /directory/,
      },
    ]) {
      const { root, db } = await fixture();
      for (const name of options.names) await put(root, name);
      const library = await openLibrary(root, db, options);
      await expect(library.scan()).rejects.toThrow(options.pattern);
      await library.shutdown();
    }
  });

  it("rejects a nonexistent nested database path under Originals without creating directories", async () => {
    const { root } = await fixture();
    await put(root, "one.ARW", "irreplaceable-original");
    const before = await snapshot(root);
    await expect(
      openLibrary(root, join(root, "new", "nested", "library.sqlite")),
    ).rejects.toThrow(/outside/);
    expect(await snapshot(root)).toEqual(before);
    await expect(lstat(join(root, "new"))).rejects.toMatchObject({
      code: "ENOENT",
    });
  });

  it("rejects preexisting and before-start hard links or symlinks to Originals", async () => {
    const { base, root } = await fixture();
    const original = await put(root, "one.ARW", "irreplaceable-original");
    const before = await snapshot(root);

    const preexistingState = join(base, "preexisting-state");
    await mkdir(preexistingState, { mode: 0o700 });
    await link(original, join(preexistingState, "library.sqlite"));
    await expect(
      openLibrary(root, join(preexistingState, "library.sqlite")),
    ).rejects.toThrow(/safely|inode/);
    expect(await snapshot(root)).toEqual(before);

    for (const replacement of ["hardlink", "symlink"] as const) {
      const state = join(base, `replace-${replacement}`);
      await mkdir(state, { mode: 0o700 });
      const database = join(state, "library.sqlite");
      await expect(
        openLibrary(root, database, {
          beforeWorkerStart: async () => {
            await unlink(database);
            if (replacement === "hardlink") await link(original, database);
            else await symlink(original, database);
          },
        }),
      ).rejects.toThrow(/inode|startup|worker/);
      expect(await snapshot(root)).toEqual(before);
    }
  });

  it.each(["-journal", "-wal", "-shm"])(
    "rejects unsafe SQLite %s sidecars before database writes",
    async (suffix) => {
      for (const replacement of ["symlink", "hardlink"] as const) {
        const { base, root } = await fixture();
        const original = await put(
          root,
          `${replacement}${suffix}.ARW`,
          "irreplaceable-sidecar-target",
        );
        const before = await snapshot(root);
        const state = join(base, `${replacement}${suffix}-state`);
        await mkdir(state, { mode: 0o700 });
        const sidecar = join(state, `library.sqlite${suffix}`);
        if (replacement === "symlink") await symlink(original, sidecar);
        else await link(original, sidecar);
        await expect(
          openLibrary(root, join(state, "library.sqlite")),
        ).rejects.toThrow(/sidecar|safely owned|worker/);
        expect(await snapshot(root)).toEqual(before);
      }
    },
  );

  it("accepts a safely owned stale rollback journal and keeps it in the state directory", async () => {
    const { base, root } = await fixture();
    const state = join(base, "safe-sidecar-state");
    await mkdir(state, { mode: 0o700 });
    await writeFile(join(state, "library.sqlite-journal"), "stale");
    await chmod(join(state, "library.sqlite-journal"), 0o600);
    const library = await openLibrary(root, join(state, "library.sqlite"));
    await library.scan();
    await library.shutdown();
    expect(await readdir(state)).toContain("library.sqlite");
  });

  it("keeps SQLite files in the descriptor-owned state directory after pathname replacement", async () => {
    const { base, root } = await fixture();
    const state = join(base, "journal-state");
    await mkdir(state, { mode: 0o700 });
    const moved = join(base, "journal-state-moved");
    const database = join(state, "library.sqlite");
    const library = await openLibrary(root, database, {
      beforeWorkerStart: async () => {
        await rename(state, moved);
        await symlink(root, state);
      },
    });
    await put(root, "one.JPG");
    await library.scan();
    await library.shutdown();
    const files = await readdir(moved);
    expect(files).toContain("library.sqlite");
    expect(
      (await readdir(root)).filter((name) =>
        /sqlite|journal|wal|shm/.test(name),
      ),
    ).toEqual([]);
  });

  it("owns the state directory descriptor across parent replacement and rejects unsafe permissions", async () => {
    const { base, root } = await fixture();
    const state = join(base, "owned-state");
    await mkdir(state, { mode: 0o700 });
    const moved = join(base, "owned-state-moved");
    const db = join(state, "library.sqlite");
    const library = await openLibrary(root, db, {
      beforeWorkerStart: async () => {
        await rename(state, moved);
        await symlink(root, state);
      },
    });
    await library.scan();
    await library.shutdown();
    expect((await readdir(moved)).sort()).toContain("library.sqlite");
    expect(await readdir(root)).not.toContain("library.sqlite");

    const unsafe = join(base, "unsafe-state");
    await mkdir(unsafe, { mode: 0o777 });
    await chmod(unsafe, 0o777);
    await expect(
      openLibrary(root, join(unsafe, "library.sqlite")),
    ).rejects.toThrow(/writable/);
    const nested = await openLibrary(
      root,
      join(base, "nested", "a", "b.sqlite"),
    );
    await nested.shutdown();
  });

  it("rejects database symlink placement attacks and creates no state beneath Originals", async () => {
    const { base, root } = await fixture();
    const inside = join(root, "state");
    await mkdir(inside);
    const outsideLink = join(base, "outside-link");
    await symlink(inside, outsideLink);
    await expect(
      openLibrary(root, join(outsideLink, "db.sqlite")),
    ).rejects.toThrow(/symbolic|outside|real directory/);
    const target = join(root, "linked.sqlite");
    const fileLink = join(base, "db-link.sqlite");
    await writeFile(target, "");
    await symlink(target, fileLink);
    await expect(openLibrary(root, fileLink)).rejects.toThrow(
      /symbolic|outside|regular file/,
    );
    const valid = join(base, "new", "deep", "db.sqlite");
    const library = await openLibrary(root, valid);
    await library.shutdown();
    const after = await snapshot(root);
    expect(after.map((item) => (item as { path: string }).path).sort()).toEqual(
      ["linked.sqlite", "state"],
    );
    expect(
      after.every((item) => !(item as { path: string }).path.includes("db")),
    ).toBe(true);
  });

  it("does not recurse into a directory replaced by a symlink after listing", async () => {
    const { base, root, db } = await fixture();
    await put(root, "inside/one.JPG", "inside");
    const outside = join(base, "outside-tree");
    await mkdir(outside);
    await put(outside, "many.JPG", "outside");
    let replaced = false;
    const library = await openLibrary(root, db, {
      beforeDirectoryRecursion: async (relativePath: string) => {
        if (relativePath !== "inside" || replaced) return;
        replaced = true;
        await rename(join(root, "inside"), join(root, "inside-old"));
        await symlink(outside, join(root, "inside"));
      },
    });
    await expect(library.scan()).rejects.toThrow(/enumerated safely/);
    expect(library.read().originals).toEqual([]);
    await library.shutdown();
  });

  it("rejects file replacement during actual confined LibRaw extraction", async () => {
    const { base, root, db } = await fixture();
    await put(root, "one.ARW", "not-a-raw");
    const outside = await put(base, "outside.ARW", "outside");
    let replaced = false;
    const library = await openLibrary(root, db, {
      beforeConfinedOperation: async (
        operation: "facts" | "read" | "extract",
        relativePath: string,
      ) => {
        if (operation !== "extract" || relativePath !== "one.ARW" || replaced)
          return;
        replaced = true;
        await unlink(join(root, "one.ARW"));
        await symlink(outside, join(root, "one.ARW"));
      },
    });
    await expect(
      library.confinedOriginal("one.ARW").extractEmbeddedJpeg(),
    ).resolves.toMatchObject({ kind: "io-error" });
    await library.shutdown();
  });

  it("uses confined openat2 reads and rejects file/directory replacement and symlink escape", async () => {
    const { base, root, db } = await fixture();
    await put(root, "dir/one.JPG", "inside");
    await put(base, "outside.JPG", "outside");
    const library = await openLibrary(root, db);
    await library.scan();
    expect(
      (await library.confinedOriginal("dir/one.JPG").read(0, 6)).toString(),
    ).toBe("inside");
    await rename(join(root, "dir"), join(root, "old"));
    await symlink(base, join(root, "dir"));
    await expect(
      library.confinedOriginal("dir/outside.JPG").facts(),
    ).rejects.toThrow(/safely/);
    await unlink(join(root, "dir"));
    await symlink(join(base, "outside.JPG"), join(root, "one.JPG"));
    await expect(
      library.confinedOriginal("one.JPG").read(0, 10),
    ).rejects.toThrow(/safely/);
    expect(() => library.confinedOriginal("../outside.JPG")).toThrow(/escapes/);
    await library.shutdown();
  });

  it("returns controlled bounded errors without absolute path disclosure", async () => {
    const { root, db } = await fixture();
    await put(root, "C:\\Users\\name\nodd.ARW");
    await symlink(
      join(root, "C:\\Users\\name\nodd.ARW"),
      join(root, "\\\\server\\share.ARW"),
    );
    const library = await openLibrary(root, db);
    const result = await library.scan();
    for (const error of result.errors) {
      expect(error.category).toBe("unreadable");
      expect(error.message).toBe("Original File could not be inspected");
      expect(error.message).not.toMatch(/\/|[A-Z]:\\|\\\\|\n/);
    }
    await library.shutdown();
  });

  it("does not mutate a nested Original tree on success, repeat, failure, unreadable or symlink scans", async () => {
    const { base, root, db } = await fixture();
    await put(root, "a/one.JPG", "one");
    await put(root, "b/two.ARW", "two");
    await chmod(join(root, "b/two.ARW"), 0o400);
    await symlink(join(base, "outside"), join(root, "link.ARW"));
    const before = await snapshot(root);
    const library = await openLibrary(root, db);
    await library.scan();
    await library.scan();
    expect(await snapshot(root)).toEqual(before);
    await library.shutdown();
    const failing = await openLibrary(root, join(base, "failure.sqlite"), {
      failureHook: "after-first-original",
    });
    await expect(failing.scan()).rejects.toThrow();
    expect(await snapshot(root)).toEqual(before);
    await failing.shutdown();
  });
});
