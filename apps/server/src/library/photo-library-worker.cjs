const { parentPort, workerData } = require("node:worker_threads");
const { DatabaseSync } = require("node:sqlite");
const { createHash, randomUUID } = require("node:crypto");
const { classifyOriginalFile: classify, pairingBaseName: stem } = require(
  workerData.fileKindsPath,
);
const binding = require(workerData.addonPath);

const SCHEMA_VERSION = 2;
let db;
let closed = false;

function admitStateSidecars() {
  const admitted = binding.admitStateSidecars(
    workerData.stateFd,
    workerData.databaseBasename,
  );
  if (!admitted || admitted.kind !== "admitted")
    throw new Error("SQLite sidecar is not safely owned");
}

function beginWriteTransaction() {
  admitStateSidecars();
  db.exec("BEGIN IMMEDIATE");
}
let nextHookId = 1;
const hookWaiters = new Map();

function digest(value) {
  return createHash("sha256").update(value).digest("hex");
}
function originalId(path) {
  return digest(`original\0${path}`);
}
function newPhotoId(seed) {
  return digest(`photo\0${seed}`);
}
function safeError(category) {
  const messages = {
    unreadable: "Original File could not be inspected",
    changed: "Original File changed during scan",
    limit: "Photo Library exceeds its configured scan limit",
  };
  return {
    category,
    message: messages[category] || "Original File inspection failed",
  };
}
function validateNumber(value, label) {
  if (!Number.isSafeInteger(value) || value < 0)
    throw new Error(`Invalid ${label}`);
}
function tableNames() {
  return db
    .prepare(
      "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .all()
    .map((row) => row.name);
}
function columns(name) {
  return db
    .prepare(`PRAGMA table_info(${name})`)
    .all()
    .map((row) => row.name);
}
function compactSql(value) {
  return String(value || "")
    .toLowerCase()
    .replace(/\s+/g, "")
    .replace(/["`\[\]]/g, "");
}
function schemaSql(name, type = "table") {
  return db
    .prepare("SELECT sql FROM sqlite_master WHERE type=? AND name=?")
    .get(type, name)?.sql;
}
function requireSql(name, fragments, type = "table") {
  const sql = compactSql(schemaSql(name, type));
  if (!sql || fragments.some((fragment) => !sql.includes(compactSql(fragment))))
    throw new Error("SQLite schema is unsupported");
}
function validateCanonicalV1Shape() {
  if (
    JSON.stringify(tableNames()) !==
    JSON.stringify(["library_metadata", "original_files", "photos"])
  )
    throw new Error("SQLite schema is unsupported");
  requireSql("library_metadata", [
    "key text primary key",
    "value text not null",
  ]);
  requireSql("original_files", [
    "id text primary key",
    "relative_path text not null unique",
    "kind text not null check(kind in ('raw','jpeg'))",
    "size integer not null check(size >= 0)",
    "mtime_ms real not null check(mtime_ms >= 0)",
    "available integer not null check(available in (0,1))",
    "error_category text check(error_category is null or error_category in ('unreadable','changed'))",
    "error_message text check(error_message is null or length(error_message) <= 120)",
  ]);
  requireSql("photos", [
    "id text primary key",
    "raw_original_id text references original_files(id)",
    "jpeg_original_id text references original_files(id)",
    "ambiguous integer not null check(ambiguous in (0,1))",
    "available integer not null check(available in (0,1))",
    "preview_state text not null check(preview_state in ('inspection-pending','ready','failed','unavailable'))",
    "preview_candidate text check(preview_candidate is null or preview_candidate in ('matching-jpeg','embedded-raw-jpeg'))",
    "preview_source text check(preview_source is null or preview_source in ('matching-jpeg','embedded-raw-jpeg'))",
    "preview_width integer check(preview_width is null or preview_width > 0)",
    "preview_height integer check(preview_height is null or preview_height > 0)",
    "sort_path text not null",
  ]);
  requireSql(
    "photos_raw",
    ["create index photos_raw on photos(raw_original_id)"],
    "index",
  );
  requireSql(
    "photos_jpeg",
    ["create index photos_jpeg on photos(jpeg_original_id)"],
    "index",
  );
  if (db.prepare("PRAGMA foreign_key_check").all().length)
    throw new Error("SQLite schema data is invalid");
}
function validateLegacyShape() {
  const expectedTables = ["library_metadata", "original_files", "photos"];
  if (JSON.stringify(tableNames()) !== JSON.stringify(expectedTables))
    throw new Error("SQLite legacy schema is unsupported");
  const expected = {
    library_metadata: ["key", "value"],
    original_files: [
      "id",
      "relative_path",
      "kind",
      "size",
      "mtime_ms",
      "available",
      "inspection_error",
    ],
    photos: [
      "id",
      "raw_original_id",
      "jpeg_original_id",
      "ambiguous",
      "available",
      "preview_state",
      "preview_source",
      "sort_path",
    ],
  };
  for (const [name, list] of Object.entries(expected))
    if (JSON.stringify(columns(name)) !== JSON.stringify(list))
      throw new Error("SQLite legacy schema is unsupported");
}
function validateLegacyData() {
  const invalidOriginal = db
    .prepare(
      `SELECT 1 FROM original_files WHERE
    typeof(id) != 'text' OR id = '' OR typeof(relative_path) != 'text' OR relative_path = '' OR
    kind NOT IN ('raw','jpeg') OR typeof(size) != 'integer' OR size < 0 OR
    typeof(mtime_ms) NOT IN ('integer','real') OR mtime_ms < 0 OR
    typeof(available) != 'integer' OR available NOT IN (0,1) LIMIT 1`,
    )
    .get();
  const invalidPhoto = db
    .prepare(
      `SELECT 1 FROM photos WHERE
    typeof(id) != 'text' OR id = '' OR typeof(ambiguous) != 'integer' OR ambiguous NOT IN (0,1) OR
    typeof(available) != 'integer' OR available NOT IN (0,1) OR
    preview_state NOT IN ('inspection-pending','ready','failed','unavailable') OR
    (preview_source IS NOT NULL AND preview_source NOT IN ('matching-jpeg','embedded-raw-jpeg')) OR
    typeof(sort_path) != 'text' OR
    (raw_original_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM original_files o WHERE o.id=photos.raw_original_id)) OR
    (jpeg_original_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM original_files o WHERE o.id=photos.jpeg_original_id)) LIMIT 1`,
    )
    .get();
  const duplicateOriginal = db
    .prepare(
      "SELECT 1 FROM original_files GROUP BY id HAVING count(*) > 1 UNION ALL SELECT 1 FROM original_files GROUP BY relative_path HAVING count(*) > 1 LIMIT 1",
    )
    .get();
  const duplicatePhoto = db
    .prepare("SELECT 1 FROM photos GROUP BY id HAVING count(*) > 1 LIMIT 1")
    .get();
  if (invalidOriginal || invalidPhoto || duplicateOriginal || duplicatePhoto)
    throw new Error("SQLite legacy data cannot be migrated safely");
}
function migrate() {
  beginWriteTransaction();
  try {
    const version = db.prepare("PRAGMA user_version").get().user_version;
    if (version > SCHEMA_VERSION)
      throw new Error(
        "SQLite schema version is newer than this Slipstream build",
      );
    if (version === 0) {
      const hasOriginals = db
        .prepare(
          "SELECT 1 FROM sqlite_master WHERE type='table' AND name='original_files'",
        )
        .get();
      if (hasOriginals) {
        validateLegacyShape();
        validateLegacyData();
        db.exec(`
          ALTER TABLE original_files RENAME TO original_files_legacy;
          ALTER TABLE photos RENAME TO photos_legacy;
          CREATE TABLE original_files(
            id TEXT PRIMARY KEY, relative_path TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL CHECK(kind IN ('raw','jpeg')),
            size INTEGER NOT NULL CHECK(size >= 0), mtime_ms REAL NOT NULL CHECK(mtime_ms >= 0),
            available INTEGER NOT NULL CHECK(available IN (0,1)),
            error_category TEXT CHECK(error_category IS NULL OR error_category IN ('unreadable','changed')),
            error_message TEXT CHECK(error_message IS NULL OR length(error_message) <= 120));
          CREATE TABLE photos(
            id TEXT PRIMARY KEY, raw_original_id TEXT REFERENCES original_files(id), jpeg_original_id TEXT REFERENCES original_files(id),
            ambiguous INTEGER NOT NULL CHECK(ambiguous IN (0,1)), available INTEGER NOT NULL CHECK(available IN (0,1)),
            preview_state TEXT NOT NULL CHECK(preview_state IN ('inspection-pending','ready','failed','unavailable')),
            preview_candidate TEXT CHECK(preview_candidate IS NULL OR preview_candidate IN ('matching-jpeg','embedded-raw-jpeg')),
            preview_source TEXT CHECK(preview_source IS NULL OR preview_source IN ('matching-jpeg','embedded-raw-jpeg')),
            preview_source_revision TEXT, preview_width INTEGER CHECK(preview_width IS NULL OR preview_width > 0),
            preview_height INTEGER CHECK(preview_height IS NULL OR preview_height > 0), cache_revision TEXT, sort_path TEXT NOT NULL);
          INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,error_category,error_message)
            SELECT id,relative_path,kind,size,mtime_ms,available,NULL,NULL FROM original_files_legacy;
          INSERT INTO photos(id,raw_original_id,jpeg_original_id,ambiguous,available,preview_state,preview_source,sort_path)
            SELECT id,raw_original_id,jpeg_original_id,ambiguous,available,preview_state,preview_source,sort_path FROM photos_legacy;
          DROP TABLE photos_legacy; DROP TABLE original_files_legacy;
          CREATE INDEX photos_raw ON photos(raw_original_id); CREATE INDEX photos_jpeg ON photos(jpeg_original_id);
        `);
        if (db.prepare("PRAGMA foreign_key_check").all().length)
          throw new Error("SQLite migration foreign-key validation failed");
        if (db.prepare("PRAGMA integrity_check").get().integrity_check !== "ok")
          throw new Error("SQLite migration integrity validation failed");
        db.exec("PRAGMA user_version = 1");
      } else
        db.exec(`
        CREATE TABLE library_metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE original_files(
          id TEXT PRIMARY KEY, relative_path TEXT NOT NULL UNIQUE,
          kind TEXT NOT NULL CHECK(kind IN ('raw','jpeg')),
          size INTEGER NOT NULL CHECK(size >= 0), mtime_ms REAL NOT NULL CHECK(mtime_ms >= 0),
          available INTEGER NOT NULL CHECK(available IN (0,1)),
          error_category TEXT CHECK(error_category IS NULL OR error_category IN ('unreadable','changed')),
          error_message TEXT CHECK(error_message IS NULL OR length(error_message) <= 120));
        CREATE TABLE photos(
          id TEXT PRIMARY KEY, raw_original_id TEXT REFERENCES original_files(id), jpeg_original_id TEXT REFERENCES original_files(id),
          ambiguous INTEGER NOT NULL CHECK(ambiguous IN (0,1)), available INTEGER NOT NULL CHECK(available IN (0,1)),
          preview_state TEXT NOT NULL CHECK(preview_state IN ('inspection-pending','ready','failed','unavailable')),
          preview_candidate TEXT CHECK(preview_candidate IS NULL OR preview_candidate IN ('matching-jpeg','embedded-raw-jpeg')),
          preview_source TEXT CHECK(preview_source IS NULL OR preview_source IN ('matching-jpeg','embedded-raw-jpeg')),
          preview_source_revision TEXT, preview_width INTEGER CHECK(preview_width IS NULL OR preview_width > 0),
          preview_height INTEGER CHECK(preview_height IS NULL OR preview_height > 0), cache_revision TEXT,
          sort_path TEXT NOT NULL);
        CREATE INDEX photos_raw ON photos(raw_original_id); CREATE INDEX photos_jpeg ON photos(jpeg_original_id);
        PRAGMA user_version = 1;`);
    }
    const migratedVersion = db
      .prepare("PRAGMA user_version")
      .get().user_version;
    if (migratedVersion === 1) {
      validateCanonicalV1Shape();
      db.exec(`
        ALTER TABLE photos ADD COLUMN selection_state TEXT NOT NULL DEFAULT 'undecided'
          CHECK(selection_state IN ('undecided','selected','rejected'));
        ALTER TABLE photos ADD COLUMN rating INTEGER NOT NULL DEFAULT 0
          CHECK(rating BETWEEN 0 AND 5);
        CREATE TABLE photo_sets(
          id TEXT PRIMARY KEY,
          name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(name) BETWEEN 1 AND 120),
          created_at INTEGER NOT NULL);
        CREATE TABLE photo_set_members(
          photo_set_id TEXT NOT NULL REFERENCES photo_sets(id) ON DELETE CASCADE,
          photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE RESTRICT,
          position INTEGER NOT NULL CHECK(position >= 0),
          PRIMARY KEY(photo_set_id, photo_id),
          UNIQUE(photo_set_id, position));
        CREATE TABLE review_progress(
          photo_set_id TEXT PRIMARY KEY REFERENCES photo_sets(id) ON DELETE CASCADE,
          photo_id TEXT NOT NULL,
          FOREIGN KEY(photo_set_id, photo_id)
            REFERENCES photo_set_members(photo_set_id, photo_id) ON DELETE CASCADE);
        CREATE INDEX photo_set_members_photo ON photo_set_members(photo_id);
        PRAGMA user_version = 2;
      `);
      if (db.prepare("PRAGMA foreign_key_check").all().length)
        throw new Error("SQLite migration foreign-key validation failed");
      if (db.prepare("PRAGMA integrity_check").get().integrity_check !== "ok")
        throw new Error("SQLite migration integrity validation failed");
    }
    db.exec("COMMIT");
  } catch (e) {
    db.exec("ROLLBACK");
    throw e;
  }
}
function openDatabase() {
  try {
    db = new DatabaseSync(
      `/proc/self/fd/${workerData.stateFd}/${workerData.databaseBasename}`,
    );
    const verified = binding.verifyStateFile(
      workerData.stateFd,
      workerData.databaseBasename,
      workerData.stateIdentity,
    );
    if (!verified || verified.kind !== "verified")
      throw new Error("SQLite database inode changed before startup");
    admitStateSidecars();
    db.exec("PRAGMA journal_mode = DELETE");
    admitStateSidecars();
    db.exec("PRAGMA foreign_keys = ON");
    migrate();
    const stored = db
      .prepare("SELECT value FROM library_metadata WHERE key='canonical_root'")
      .get();
    if (stored && stored.value !== workerData.root)
      throw new Error(
        "SQLite database belongs to a different Photo Library root",
      );
    if (!stored) {
      beginWriteTransaction();
      try {
        db.prepare(
          "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
        ).run(workerData.root);
        db.exec("COMMIT");
      } catch (error) {
        db.exec("ROLLBACK");
        throw error;
      }
    }
  } catch (e) {
    try {
      db?.close();
    } catch {}
    db = undefined;
    throw e;
  }
}

async function walk() {
  const discovered = [];
  const errors = [];
  let total = 0;
  let recognized = 0;
  async function visit(rel) {
    const result = binding.listConfinedDirectory(
      workerData.rootFd,
      rel,
      workerData.maximumEntriesPerDirectory,
    );
    if (!result || result.kind !== "directory")
      throw new Error("Photo Library directory could not be enumerated safely");
    const entries = result.entries;
    total += entries.length;
    if (total > workerData.maximumEntries)
      throw new Error("Photo Library exceeds total entry limit");
    for (const entry of entries) {
      const child = rel ? `${rel}/${entry.name}` : entry.name;
      if (entry.kind === "directory") {
        if (workerData.beforeDirectoryRecursion)
          await requestHook("beforeDirectoryRecursion", child);
        await visit(child);
        continue;
      }
      if (entry.kind !== "file") {
        if (classify(entry.name))
          errors.push({ relativePath: child, ...safeError("unreadable") });
        continue;
      }
      const kind = classify(entry.name);
      if (!kind) continue;
      recognized++;
      if (recognized > workerData.maximumFiles)
        throw new Error("Photo Library exceeds recognized file limit");
      try {
        const facts = binding.confinedOriginalFacts(workerData.rootFd, child);
        validateNumber(facts.size, "Original size");
        if (!Number.isFinite(facts.mtimeMs) || facts.mtimeMs < 0)
          throw new Error("Invalid Original modification time");
        discovered.push({
          relativePath: child,
          kind,
          size: facts.size,
          mtimeMs: facts.mtimeMs,
        });
      } catch {
        const err = safeError("unreadable");
        discovered.push({
          relativePath: child,
          kind,
          size: 0,
          mtimeMs: 0,
          errorCategory: err.category,
          errorMessage: err.message,
        });
        errors.push({ relativePath: child, ...err });
      }
    }
  }
  await visit("");
  return { discovered, errors };
}
function revision(file) {
  return file ? `${file.relativePath}\0${file.size}\0${file.mtimeMs}` : null;
}
function currentRows() {
  return db.prepare("SELECT * FROM photos").all();
}
function reconcile(discovered) {
  const groups = new Map();
  for (const f of discovered) {
    const slash = f.relativePath.lastIndexOf("/");
    const dir = slash < 0 ? "" : f.relativePath.slice(0, slash);
    const name = slash < 0 ? f.relativePath : f.relativePath.slice(slash + 1);
    const key = `${dir}\0${stem(name)}`;
    const g = groups.get(key) || [];
    g.push(f);
    groups.set(key, g);
  }
  const existing = currentRows();
  const byOriginal = new Map();
  for (const p of existing) {
    if (p.raw_original_id) byOriginal.set(p.raw_original_id, p);
    if (p.jpeg_original_id) byOriginal.set(p.jpeg_original_id, p);
  }
  const updates = [];
  const used = new Set();
  for (const [key, group] of [...groups].sort(([a], [b]) =>
    Buffer.from(a).compare(Buffer.from(b)),
  )) {
    const raws = group.filter((x) => x.kind === "raw"),
      jpegs = group.filter((x) => x.kind === "jpeg");
    if (raws.length === 1 && jpegs.length === 1) {
      const r = raws[0],
        j = jpegs[0],
        ri = originalId(r.relativePath),
        ji = originalId(j.relativePath);
      const prior = byOriginal.get(ri) || byOriginal.get(ji);
      updates.push({
        id: prior?.id || newPhotoId(`${ri}\0${ji}`),
        raw: r,
        jpeg: j,
        rawPresent: true,
        jpegPresent: true,
        rawId: ri,
        jpegId: ji,
        ambiguous: false,
        prior,
      });
      used.add(updates.at(-1).id);
      continue;
    }
    const ambiguous = group.length > 1;
    const priorPairs = ambiguous
      ? new Set(
          group
            .map((f) => byOriginal.get(originalId(f.relativePath)))
            .filter((p) => p && p.raw_original_id && p.jpeg_original_id),
        )
      : new Set();
    for (const prior of priorPairs) {
      updates.push({
        id: prior.id,
        rawId: prior.raw_original_id,
        jpegId: prior.jpeg_original_id,
        rawPresent: true,
        jpegPresent: true,
        ambiguous: true,
        prior,
        sortPath: prior.sort_path,
      });
      used.add(prior.id);
    }
    const belongsToPriorPair = new Set();
    for (const prior of priorPairs) {
      belongsToPriorPair.add(prior.raw_original_id);
      belongsToPriorPair.add(prior.jpeg_original_id);
    }
    for (const f of group) {
      const oid = originalId(f.relativePath);
      const prior = byOriginal.get(oid);
      if (belongsToPriorPair.has(oid)) continue;
      if (
        prior &&
        prior.raw_original_id &&
        prior.jpeg_original_id &&
        !ambiguous
      )
        continue;
      const id = prior?.id || newPhotoId(oid);
      updates.push({
        id,
        [f.kind]: f,
        rawPresent: f.kind === "raw",
        jpegPresent: f.kind === "jpeg",
        rawId: f.kind === "raw" ? oid : null,
        jpegId: f.kind === "jpeg" ? oid : null,
        ambiguous,
        prior,
      });
      used.add(id);
    }
  }
  // A missing member of an existing pair preserves the paired record when one constituent remains.
  for (const p of existing) {
    if (used.has(p.id) || !p.raw_original_id || !p.jpeg_original_id) continue;
    const raw = discovered.find(
      (f) => originalId(f.relativePath) === p.raw_original_id,
    );
    const jpeg = discovered.find(
      (f) => originalId(f.relativePath) === p.jpeg_original_id,
    );
    if (raw || jpeg) {
      updates.push({
        id: p.id,
        raw,
        jpeg,
        rawPresent: Boolean(raw),
        jpegPresent: Boolean(jpeg),
        rawId: p.raw_original_id,
        jpegId: p.jpeg_original_id,
        ambiguous: Boolean(p.ambiguous),
        prior: p,
        sortPath: p.sort_path,
      });
      used.add(p.id);
    }
  }
  return updates;
}
function applyScan(discovered, hook) {
  beginWriteTransaction();
  const upsertOriginal = db.prepare(
    `INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,error_category,error_message) VALUES(?,?,?,?,?,1,?,?) ON CONFLICT(relative_path) DO UPDATE SET kind=excluded.kind,size=excluded.size,mtime_ms=excluded.mtime_ms,available=1,error_category=excluded.error_category,error_message=excluded.error_message`,
  );
  const upsertPhoto = db.prepare(
    `INSERT INTO photos(id,raw_original_id,jpeg_original_id,ambiguous,available,preview_state,preview_candidate,preview_source,preview_source_revision,preview_width,preview_height,cache_revision,sort_path) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET raw_original_id=excluded.raw_original_id,jpeg_original_id=excluded.jpeg_original_id,ambiguous=excluded.ambiguous,available=excluded.available,preview_state=excluded.preview_state,preview_candidate=excluded.preview_candidate,preview_source=excluded.preview_source,preview_source_revision=excluded.preview_source_revision,preview_width=excluded.preview_width,preview_height=excluded.preview_height,cache_revision=excluded.cache_revision,sort_path=excluded.sort_path`,
  );
  try {
    const previousFacts = new Map(
      db
        .prepare("SELECT relative_path,size,mtime_ms FROM original_files")
        .all()
        .map((row) => [row.relative_path, row]),
    );
    db.exec("UPDATE original_files SET available=0");
    let n = 0;
    for (const f of discovered) {
      upsertOriginal.run(
        originalId(f.relativePath),
        f.relativePath,
        f.kind,
        f.size,
        f.mtimeMs,
        f.errorCategory || null,
        f.errorMessage || null,
      );
      if (hook === "after-first-original" && n++ === 0)
        throw new Error("injected DB failure");
    }
    db.exec("UPDATE photos SET available=0");
    for (const c of reconcile(discovered)) {
      const selected =
        c.jpeg && !c.jpeg.errorCategory
          ? c.jpeg
          : c.raw && !c.raw.errorCategory
            ? c.raw
            : null;
      const candidate = selected
        ? selected.kind === "jpeg"
          ? "matching-jpeg"
          : "embedded-raw-jpeg"
        : null;
      const rev = revision(selected);
      const previousRevision = c.prior?.preview_source_revision;
      const previousPath = previousRevision?.split("\0", 1)[0];
      const selectedPath = selected?.relativePath;
      const priorFacts = selectedPath
        ? previousFacts.get(selectedPath)
        : undefined;
      const selectedUnchanged =
        priorFacts &&
        selected &&
        priorFacts.size === selected.size &&
        priorFacts.mtime_ms === selected.mtimeMs;
      const preserve =
        c.prior &&
        c.prior.preview_state !== "inspection-pending" &&
        previousPath === selectedPath &&
        selectedUnchanged;
      const available = Boolean(c.rawPresent || c.jpegPresent);
      const sortPath =
        c.sortPath || c.raw?.relativePath || c.jpeg?.relativePath || "";
      upsertPhoto.run(
        c.id,
        c.rawId || null,
        c.jpegId || null,
        c.ambiguous ? 1 : 0,
        available ? 1 : 0,
        preserve
          ? c.prior.preview_state
          : available
            ? "inspection-pending"
            : "unavailable",
        candidate,
        preserve ? c.prior.preview_source : null,
        preserve ? rev : null,
        preserve ? c.prior.preview_width : null,
        preserve ? c.prior.preview_height : null,
        preserve ? c.prior.cache_revision : null,
        sortPath,
      );
    }
    db.exec("COMMIT");
  } catch (e) {
    db.exec("ROLLBACK");
    throw e;
  }
}
function readAll() {
  return {
    originals: db
      .prepare(
        "SELECT * FROM original_files ORDER BY relative_path COLLATE BINARY",
      )
      .all(),
    photos: db
      .prepare("SELECT * FROM photos ORDER BY sort_path COLLATE BINARY,id")
      .all(),
  };
}
function readPhotoSets() {
  const sets = db
    .prepare("SELECT id,name FROM photo_sets ORDER BY created_at,id")
    .all();
  const members = db
    .prepare(
      `SELECT m.photo_set_id,m.photo_id,m.position,p.available,p.selection_state,p.rating
       FROM photo_set_members m JOIN photos p ON p.id=m.photo_id
       ORDER BY m.photo_set_id,m.position`,
    )
    .all();
  const progress = new Map(
    db
      .prepare("SELECT photo_set_id,photo_id FROM review_progress")
      .all()
      .map((row) => [row.photo_set_id, row.photo_id]),
  );
  return sets.map((set) => ({
    id: set.id,
    name: set.name,
    lastReviewedPhotoId: progress.get(set.id) || null,
    members: members
      .filter((member) => member.photo_set_id === set.id)
      .map((member) => ({
        photoId: member.photo_id,
        position: member.position,
        available: Boolean(member.available),
        selectionState: member.selection_state,
        rating: member.rating,
      })),
  }));
}
class DomainFailure extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}
function notFound(message) {
  throw new DomainFailure("not-found", message);
}
function conflict(message) {
  throw new DomainFailure("conflict", message);
}
function rollbackMutation() {
  try {
    db.exec("ROLLBACK");
  } catch {}
}
function mutationFailure(error) {
  if (error instanceof DomainFailure) return error;
  if (
    String(error?.code || "").includes("CONSTRAINT") ||
    /constraint|unique/i.test(String(error?.message || ""))
  )
    return new DomainFailure(
      "conflict",
      "Mutation conflicts with current state",
    );
  return new DomainFailure("persistence", "Mutation could not be persisted");
}
function photoSetMutation(p) {
  try {
    beginWriteTransaction();
    let result;
    switch (p.kind) {
      case "create": {
        const id = randomUUID();
        db.prepare(
          "INSERT INTO photo_sets(id,name,created_at) VALUES(?,?,?)",
        ).run(id, p.name, Date.now());
        result = { kind: "applied", photoSetId: id };
        break;
      }
      case "rename": {
        const changed = db
          .prepare("UPDATE photo_sets SET name=? WHERE id=?")
          .run(p.name, p.photoSetId);
        if (changed.changes !== 1) notFound("Unknown Photo Set");
        result = { kind: "applied", photoSetId: p.photoSetId };
        break;
      }
      case "delete": {
        const changed = db
          .prepare("DELETE FROM photo_sets WHERE id=?")
          .run(p.photoSetId);
        if (changed.changes !== 1) notFound("Unknown Photo Set");
        result = { kind: "applied", photoSetId: p.photoSetId };
        break;
      }
      case "addMembers": {
        if (
          !db.prepare("SELECT 1 FROM photo_sets WHERE id=?").get(p.photoSetId)
        )
          notFound("Unknown Photo Set");
        const exists = db.prepare("SELECT 1 FROM photos WHERE id=?");
        const insert = db.prepare(
          "INSERT INTO photo_set_members(photo_set_id,photo_id,position) VALUES(?,?,?)",
        );
        let position = db
          .prepare(
            "SELECT COALESCE(MAX(position)+1,0) AS position FROM photo_set_members WHERE photo_set_id=?",
          )
          .get(p.photoSetId).position;
        for (const photoId of p.photoIds) {
          if (!exists.get(photoId)) notFound("Unknown Photo");
          insert.run(p.photoSetId, photoId, position++);
        }
        result = { kind: "applied", photoSetId: p.photoSetId };
        break;
      }
      case "removeMember": {
        const member = db
          .prepare(
            "SELECT position FROM photo_set_members WHERE photo_set_id=? AND photo_id=?",
          )
          .get(p.photoSetId, p.photoId);
        if (!member) notFound("Unknown Photo Set member");
        db.prepare(
          "DELETE FROM photo_set_members WHERE photo_set_id=? AND photo_id=?",
        ).run(p.photoSetId, p.photoId);
        db.prepare(
          "UPDATE photo_set_members SET position=position-1 WHERE photo_set_id=? AND position>?",
        ).run(p.photoSetId, member.position);
        result = { kind: "applied", photoSetId: p.photoSetId };
        break;
      }
      case "reorder": {
        const current = db
          .prepare(
            "SELECT photo_id FROM photo_set_members WHERE photo_set_id=? ORDER BY position",
          )
          .all(p.photoSetId)
          .map((row) => row.photo_id);
        if (
          current.length !== p.photoIds.length ||
          new Set(p.photoIds).size !== p.photoIds.length ||
          current.some((id) => !p.photoIds.includes(id))
        )
          conflict("Photo Set order must contain every member once");
        db.prepare(
          "UPDATE photo_set_members SET position=position+1000000 WHERE photo_set_id=?",
        ).run(p.photoSetId);
        const update = db.prepare(
          "UPDATE photo_set_members SET position=? WHERE photo_set_id=? AND photo_id=?",
        );
        p.photoIds.forEach((id, index) => update.run(index, p.photoSetId, id));
        result = { kind: "applied", photoSetId: p.photoSetId };
        break;
      }
      case "setProgress": {
        if (
          !db
            .prepare(
              "SELECT 1 FROM photo_set_members WHERE photo_set_id=? AND photo_id=?",
            )
            .get(p.photoSetId, p.photoId)
        )
          notFound("Unknown Photo Set member");
        db.prepare(
          `INSERT INTO review_progress(photo_set_id,photo_id) VALUES(?,?)
           ON CONFLICT(photo_set_id) DO UPDATE SET photo_id=excluded.photo_id`,
        ).run(p.photoSetId, p.photoId);
        result = { kind: "applied", photoSetId: p.photoSetId };
        break;
      }
      default:
        throw new Error("Unknown Photo Set mutation");
    }
    db.exec("COMMIT");
    return result;
  } catch (error) {
    rollbackMutation();
    throw mutationFailure(error);
  }
}
function photoStateMutation(p) {
  try {
    beginWriteTransaction();
    const column = p.field === "selectionState" ? "selection_state" : "rating";
    const row = db
      .prepare(`SELECT ${column} AS value FROM photos WHERE id=?`)
      .get(p.photoId);
    if (!row) notFound("Unknown Photo");
    if (p.expectedCurrent !== undefined && row.value !== p.expectedCurrent)
      conflict("Photo state changed");
    db.prepare(`UPDATE photos SET ${column}=? WHERE id=?`).run(
      p.value,
      p.photoId,
    );
    if (p.photoSetId) {
      if (
        !db
          .prepare(
            "SELECT 1 FROM photo_set_members WHERE photo_set_id=? AND photo_id=?",
          )
          .get(p.photoSetId, p.photoId)
      )
        notFound("Unknown Photo Set member");
      db.prepare(
        `INSERT INTO review_progress(photo_set_id,photo_id) VALUES(?,?)
         ON CONFLICT(photo_set_id) DO UPDATE SET photo_id=excluded.photo_id`,
      ).run(p.photoSetId, p.photoId);
    }
    db.exec("COMMIT");
    return {
      kind: "applied",
      photoId: p.photoId,
      undo: {
        photoId: p.photoId,
        field: p.field,
        priorValue: row.value,
        expectedCurrent: p.value,
      },
    };
  } catch (error) {
    rollbackMutation();
    throw mutationFailure(error);
  }
}
async function scan(payload) {
  const { discovered, errors } = await walk();
  applyScan(discovered, payload?.failureHook);
  return { ...readAll(), errors };
}
function seedPreview(p) {
  beginWriteTransaction();
  try {
    const photo = db
      .prepare(
        "SELECT preview_candidate,raw_original_id,jpeg_original_id FROM photos WHERE id=?",
      )
      .get(p.photoId);
    if (!photo || photo.preview_candidate !== p.expectedCandidate) {
      db.exec("COMMIT");
      return { kind: "stale-ignored" };
    }
    const sourceId =
      p.expectedCandidate === "matching-jpeg"
        ? photo.jpeg_original_id
        : photo.raw_original_id;
    const original = sourceId
      ? db
          .prepare(
            "SELECT relative_path,size,mtime_ms,available FROM original_files WHERE id=?",
          )
          .get(sourceId)
      : undefined;
    const revision =
      original && original.available
        ? `${original.relative_path}\0${original.size}\0${original.mtime_ms}`
        : null;
    if (revision !== p.expectedSourceRevision) {
      db.exec("COMMIT");
      return { kind: "stale-ignored" };
    }
    let actualRevision = revision;
    if (p.actualSource && p.actualSource !== p.expectedCandidate) {
      const actualSourceId =
        p.actualSource === "matching-jpeg"
          ? photo.jpeg_original_id
          : photo.raw_original_id;
      const actualOriginal = actualSourceId
        ? db
            .prepare(
              "SELECT relative_path,size,mtime_ms,available FROM original_files WHERE id=?",
            )
            .get(actualSourceId)
        : undefined;
      actualRevision =
        actualOriginal && actualOriginal.available
          ? `${actualOriginal.relative_path}\0${actualOriginal.size}\0${actualOriginal.mtime_ms}`
          : null;
      if (actualRevision !== p.actualSourceRevision) {
        db.exec("COMMIT");
        return { kind: "stale-ignored" };
      }
    }
    const changed = db
      .prepare(
        "UPDATE photos SET preview_state=?,preview_source=?,preview_source_revision=?,preview_width=?,preview_height=?,cache_revision=? WHERE id=? AND preview_candidate=?",
      )
      .run(
        p.state,
        p.actualSource || p.expectedCandidate,
        actualRevision,
        p.width,
        p.height,
        p.cacheRevision,
        p.photoId,
        p.expectedCandidate,
      );
    db.exec("COMMIT");
    return changed.changes === 1
      ? { kind: "applied" }
      : { kind: "stale-ignored" };
  } catch (error) {
    db.exec("ROLLBACK");
    throw error;
  }
}
function requestHook(hook, relativePath, operation) {
  const hookId = nextHookId++;
  return new Promise((resolve) => {
    hookWaiters.set(hookId, resolve);
    parentPort.postMessage({ hook, hookId, relativePath, operation });
  });
}
async function handle(message) {
  if (closed && message.command !== "shutdown")
    throw new Error("Photo Library is closed");
  switch (message.command) {
    case "scan":
      return scan(message.payload);
    case "read":
      return readAll();
    case "readPhotoSets":
      return readPhotoSets();
    case "photoSetMutation":
      return photoSetMutation(message.payload);
    case "photoStateMutation":
      return photoStateMutation(message.payload);
    case "seedPreview":
      return seedPreview(message.payload);
    case "confinedFacts": {
      if (workerData.beforeConfinedOperation)
        await requestHook(
          "beforeConfinedOperation",
          message.payload.path,
          "facts",
        );
      const value = binding.confinedOriginalFacts(
        workerData.rootFd,
        message.payload.path,
      );
      if (!value || value.kind !== "facts")
        throw new Error("Original File is not safely readable");
      return { size: value.size, mtimeMs: value.mtimeMs, mode: value.mode };
    }
    case "confinedReadWhole": {
      if (workerData.beforeConfinedOperation)
        await requestHook(
          "beforeConfinedOperation",
          message.payload.path,
          "read",
        );
      const value = binding.readConfinedOriginalWhole(
        workerData.rootFd,
        message.payload.path,
        message.payload.maximumBytes,
      );
      if (!value || value.kind !== "file" || !Buffer.isBuffer(value.bytes))
        throw new Error("Original File is not safely readable");
      return { bytes: value.bytes, sourceFacts: value.sourceFacts };
    }
    case "confinedRead": {
      if (workerData.beforeConfinedOperation)
        await requestHook(
          "beforeConfinedOperation",
          message.payload.path,
          "read",
        );
      const value = binding.readConfinedOriginalRange(
        workerData.rootFd,
        message.payload.path,
        message.payload.offset,
        message.payload.length,
      );
      if (!Buffer.isBuffer(value))
        throw new Error("Original File is not safely readable");
      return value;
    }
    case "confinedExtract":
      if (workerData.beforeConfinedOperation)
        await requestHook(
          "beforeConfinedOperation",
          message.payload.path,
          "extract",
        );
      return binding.extractLargestEmbeddedJpegFromLibrary(
        workerData.rootFd,
        message.payload.path,
      );
    case "shutdown":
      closed = true;
      db?.close();
      db = undefined;
      return null;
    default:
      throw new Error("Unknown Photo Library worker command");
  }
}
try {
  if (workerData.startupFailure)
    throw new Error("injected worker startup failure");
  openDatabase();
  parentPort.postMessage({ ready: true });
} catch (e) {
  parentPort.postMessage({ ready: false, error: e.message });
}
let commandTail = Promise.resolve();
parentPort.on("message", (m) => {
  if (m.hookId !== undefined) {
    hookWaiters.get(m.hookId)?.();
    hookWaiters.delete(m.hookId);
    return;
  }
  const operation = commandTail.then(() => handle(m));
  commandTail = operation.catch(() => undefined);
  operation.then(
    (result) => parentPort.postMessage({ id: m.id, result }),
    (e) =>
      parentPort.postMessage({
        id: m.id,
        error: e instanceof Error ? e.message : "Photo Library command failed",
        errorCode: e instanceof DomainFailure ? e.code : undefined,
      }),
  );
});
