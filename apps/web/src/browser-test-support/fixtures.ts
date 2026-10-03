import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createHash } from "node:crypto";
import {
  mkdir,
  mkdtemp,
  open,
  readFile,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { expect, test, type BrowserContext, type Page } from "@playwright/test";

import {
  fixtureFetch,
  startBrowserServer,
  type BrowserServer,
} from "../browser-server.js";

export const sample = process.env.SLIPSTREAM_RAW_SAMPLE;
/// The optional native Photo bundle used by the local single-container smoke.
/// The scenario stays skipped unless an operator supplies the bundle directory
/// built by tools/processing/photo/build.py.
const photoBundleDirectory =
  process.env.SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY?.trim();
export const processingEnvironmentOverrides = {
  SLIPSTREAM_PHOTO_DEVELOPMENT: photoBundleDirectory ? "enabled" : undefined,
  SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY: photoBundleDirectory,
  SLIPSTREAM_EXPORT_RETAINED_OUTPUT_BYTES:
    process.env.SLIPSTREAM_EXPORT_RETAINED_OUTPUT_BYTES?.trim(),
} as const;
const noProcessingEnvironment = Object.fromEntries(
  Object.keys(processingEnvironmentOverrides).map((name) => [name, undefined]),
);
const processingEnvironment = [
  ["SLIPSTREAM_RAW_SAMPLE", sample],
  [
    "SLIPSTREAM_PHOTO_DEVELOPMENT",
    processingEnvironmentOverrides.SLIPSTREAM_PHOTO_DEVELOPMENT,
  ],
  ["SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY", photoBundleDirectory],
  [
    "SLIPSTREAM_EXPORT_RETAINED_OUTPUT_BYTES",
    processingEnvironmentOverrides.SLIPSTREAM_EXPORT_RETAINED_OUTPUT_BYTES,
  ],
] as const;
export const missingProcessingEnvironment = () =>
  processingEnvironment
    .filter(([, value]) => !value || !value.trim())
    .map(([name]) => name);
const temporary: string[] = [];
export const servers: BrowserServer[] = [];

let activeContext: BrowserContext;
let transportFailures: Array<{ method: string; path: string; error: string }>;
export function setupBrowserSmoke() {
  test.beforeEach(({ context, page }) => {
    activeContext = context;
    transportFailures = [];
    page.on("requestfailed", (request) => {
      transportFailures.push({
        method: request.method(),
        path: new URL(request.url()).pathname,
        error: request.failure()?.errorText ?? "unknown",
      });
      if (transportFailures.length > 50) transportFailures.shift();
    });
  });

  test.afterEach(async () => {
    const testInfo = test.info();
    if (testInfo.status !== testInfo.expectedStatus) {
      await testInfo.attach("transport-failures", {
        body: JSON.stringify(transportFailures, null, 2),
        contentType: "application/json",
      });
    }
    await Promise.all(servers.splice(0).map((server) => server.close()));
    await Promise.all(
      temporary
        .splice(0)
        .map((path) => rm(path, { recursive: true, force: true })),
    );
  });
}

export async function jpeg() {
  return readFile(new URL("../../test-fixtures/review.jpg", import.meta.url));
}
export function withCaptureTime(
  source: Uint8Array,
  captureTime: string,
): Uint8Array {
  const value = new TextEncoder().encode(`${captureTime}\0`);
  const dataOffset = 8 + 2 + 12 + 4;
  const tiff = new Uint8Array(dataOffset + value.length);
  tiff.set([0x49, 0x49, 0x2a, 0, 8, 0, 0, 0]);
  tiff.set([1, 0], 8);
  tiff.set([0x03, 0x90, 2, 0], 10);
  new DataView(tiff.buffer).setUint32(14, value.length, true);
  new DataView(tiff.buffer).setUint32(18, dataOffset, true);
  tiff.set(value, dataOffset);
  const payload = new Uint8Array([69, 120, 105, 102, 0, 0, ...tiff]);
  const app1 = new Uint8Array(payload.length + 4);
  app1.set([
    0xff,
    0xe1,
    (payload.length + 2) >> 8,
    (payload.length + 2) & 0xff,
  ]);
  app1.set(payload, 4);
  return new Uint8Array([...source.slice(0, 2), ...app1, ...source.slice(2)]);
}
/**
 * Writes one EXIF Orientation tag into a JPEG, mirroring the TIFF layout the
 * Rust capture and derivative pipeline already understands. Orientation 6
 * rotates the displayed image 90 degrees clockwise.
 */
export function withExifOrientation(
  source: Uint8Array,
  value: number,
): Uint8Array {
  const dataOffset = 8 + 2 + 12 + 4;
  const tiff = new Uint8Array(dataOffset);
  tiff.set([0x49, 0x49, 0x2a, 0, 8, 0, 0, 0]);
  tiff.set([1, 0], 8);
  tiff.set([0x12, 0x01, 3, 0], 10);
  const view = new DataView(tiff.buffer);
  view.setUint32(14, 1, true);
  view.setUint16(18, value, true);
  const payload = new Uint8Array([69, 120, 105, 102, 0, 0, ...tiff]);
  const app1 = new Uint8Array(payload.length + 4);
  app1.set([
    0xff,
    0xe1,
    (payload.length + 2) >> 8,
    (payload.length + 2) & 0xff,
  ]);
  app1.set(payload, 4);
  return new Uint8Array([...source.slice(0, 2), ...app1, ...source.slice(2)]);
}
export async function jpegWithSize(
  page: Page,
  width: number,
  height: number,
): Promise<Buffer> {
  const bytes = await page.evaluate(
    async ({ width, height }) => {
      const canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext("2d");
      if (!context) throw new Error("Canvas is unavailable");
      context.fillStyle = "#7a7d82";
      context.fillRect(0, 0, width, height);
      const blob = await new Promise<Blob | null>((resolve) =>
        canvas.toBlob(resolve, "image/jpeg", 0.92),
      );
      if (!blob) throw new Error("JPEG encoding failed");
      return Array.from(new Uint8Array(await blob.arrayBuffer()));
    },
    { width, height },
  );
  return Buffer.from(bytes);
}
export async function fixture() {
  const base = await mkdtemp(join(tmpdir(), "slipstream-browser-"));
  temporary.push(base);
  const root = join(base, "originals");
  await mkdir(root);
  return { base, root };
}
export async function writePhotos(root: string, count: number) {
  const data = await jpeg();
  for (let index = 0; index < count; index += 1)
    await writeFile(join(root, `${String(index).padStart(3, "0")}.jpg`), data);
}
export async function server(
  base: string,
  root: string,
  environment: Readonly<
    Record<string, string | undefined>
  > = noProcessingEnvironment,
) {
  const running = await startBrowserServer({ base, root, environment });
  servers.push(running);
  const login = await activeContext.request.post(
    `${running.url}/api/access/session`,
    { headers: { Origin: running.url }, data: { token: running.token } },
  );
  expect(login.status()).toBe(204);
  // The server binds before its owned startup scan finishes, so tests wait
  // for the Library to settle before driving the UI.
  await expect
    .poll(
      async () => {
        const response = await fixtureFetch(`${running.url}/api/status`);
        return ((await response.json()) as { state: string }).state;
      },
      { timeout: 60_000 },
    )
    .toBe("idle");
  return running;
}
export async function post(url: string, path: string, body: unknown) {
  return fixtureFetch(`${url}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Origin: url },
    body: JSON.stringify(body),
  });
}
type BrowsePhoto = {
  id: string;
  available: boolean;
  selectionState: string;
  rating: number;
  originalFilename?: string;
  original?: Readonly<{ kind: string; available: boolean }>;
};
type AlbumMember = BrowsePhoto & { photoId: string; position: number };
type AlbumState = { id: string; position: number; members: AlbumMember[] };

async function browseWindow(
  url: string,
  token: string,
  start: number,
): Promise<{ start: number; total: number; photos: BrowsePhoto[] }> {
  const window = (await (
    await fixtureFetch(`${url}/api/browse/${token}?start=${start}&limit=60`)
  ).json()) as { start: number; total: number; photos: BrowsePhoto[] };
  if (window.start !== start)
    throw new Error("browse window start is inconsistent");
  return window;
}

export async function browseIds(url: string): Promise<string[]> {
  const opened = (await (
    await post(url, "/api/browse", { source: "library" })
  ).json()) as { token: string; total: number };
  const ids: string[] = [];
  let start = 0;
  for (;;) {
    const window = await browseWindow(url, opened.token, start);
    if (window.total !== opened.total)
      throw new Error("browse window total is inconsistent");
    ids.push(...window.photos.map((photo) => photo.id));
    start += window.photos.length;
    if (window.photos.length === 0 || start >= opened.total) break;
  }
  await fixtureFetch(`${url}/api/browse/${opened.token}`, {
    method: "DELETE",
    headers: { Origin: url },
  });
  return ids;
}

/// The current Library facts of one Photo, read through a fresh Browse
/// token.

export async function libraryPhoto(
  url: string,
  index: number,
): Promise<BrowsePhoto> {
  const opened = (await (
    await post(url, "/api/browse", { source: "library" })
  ).json()) as { token: string; total: number };
  const window = await browseWindow(url, opened.token, 0);
  await fixtureFetch(`${url}/api/browse/${opened.token}`, {
    method: "DELETE",
    headers: { Origin: url },
  });
  const photo = window.photos[index];
  if (!photo) throw new Error(`Library has no Photo at ${index}`);
  return photo;
}
export async function createAlbum(
  url: string,
  name = "Review",
  photoIds?: string[],
) {
  const photos = photoIds ?? (await browseIds(url));
  const created = (await (await post(url, "/api/albums", { name })).json()) as {
    albums: Array<{ id: string; name: string }>;
  };
  const album = created.albums.find((item) => item.name === name)!;
  for (let offset = 0; offset < photos.length; offset += 100)
    await post(url, `/api/albums/${album.id}/members`, {
      photoIds: photos.slice(offset, offset + 100),
    });
  return { albumId: album.id };
}

/// Runs one compiled CLI client command against a real service, mirroring the
/// server-binary resolution browser-server.ts already uses.
export async function cli(server: string, invocation: string[]) {
  const binary = resolve(
    process.env.SLIPSTREAM_CLI_BINARY ?? "target/debug/slipstream",
  );
  const running = servers.find((candidate) => candidate.url === server);
  if (!running) throw new Error("Unknown CLI fixture");
  const completed = await promisify(execFile)(
    binary,
    ["--server", server, "--token-file", running.tokenFile, ...invocation],
    {
      env: {
        ...process.env,
        SSL_CERT_FILE: resolve("tools/test-tls/cert.pem"),
      },
      encoding: "utf8",
    },
  );
  if (completed.stderr)
    throw new Error(`unexpected CLI stderr: ${completed.stderr}`);
  const envelope = JSON.parse(completed.stdout) as {
    status: string;
    data?: Record<string, unknown>;
    error?: unknown;
  };
  if (envelope.status !== "ok")
    throw new Error(`CLI command failed: ${JSON.stringify(envelope.error)}`);
  return envelope;
}

// Membership order and per-member facts are observable only through a
// fresh Album Browse Snapshot. The resolved open position exposes the
// saved Album position under the unavailable-member fallback rules.
export async function state(url: string, albumId: string): Promise<AlbumState> {
  const opened = (await (
    await post(url, "/api/browse", {
      source: "album",
      albumId: albumId,
      resume: true,
    })
  ).json()) as { token: string; total: number; position: number };
  const members: AlbumMember[] = [];
  let start = 0;
  for (;;) {
    const window = await browseWindow(url, opened.token, start);
    if (window.total !== opened.total)
      throw new Error("browse window total is inconsistent");
    window.photos.forEach((photo, index) =>
      members.push({ ...photo, photoId: photo.id, position: start + index }),
    );
    start += window.photos.length;
    if (window.photos.length === 0 || start >= opened.total) break;
  }
  await fixtureFetch(`${url}/api/browse/${opened.token}`, {
    method: "DELETE",
    headers: { Origin: url },
  });
  return { id: albumId, position: opened.position, members };
}

type OriginalSnapshot = Readonly<{
  sha256: string;
  length: bigint;
  device: bigint;
  inode: bigint;
  mode: bigint;
  owner: bigint;
  group: bigint;
  modifiedNanoseconds: bigint;
}>;

export async function originalSnapshot(
  path: string,
): Promise<OriginalSnapshot> {
  const [bytes, metadata] = await Promise.all([
    readFile(path),
    stat(path, { bigint: true }),
  ]);
  if (!metadata.isFile())
    throw new Error(`Original safety sample must be a regular file: ${path}`);
  return {
    sha256: createHash("sha256").update(bytes).digest("hex"),
    length: metadata.size,
    device: metadata.dev,
    inode: metadata.ino,
    mode: metadata.mode,
    owner: metadata.uid,
    group: metadata.gid,
    modifiedNanoseconds: metadata.mtimeNs,
  };
}
export async function optionalOriginalSnapshot(
  path: string,
): Promise<OriginalSnapshot | null> {
  try {
    return await originalSnapshot(path);
  } catch (error) {
    if (
      typeof error === "object" &&
      error !== null &&
      "code" in error &&
      error.code === "ENOENT"
    ) {
      return null;
    }
    throw error;
  }
}

/// True when the explicit path is a readable regular file. The RAW gate
/// validates the sample for its own runs; a scenario that runs outside the
/// gate checks it here so an unusable path reports as a skip.
export async function readableRegularFile(path: string): Promise<boolean> {
  try {
    const handle = await open(path, "r");
    try {
      return (await handle.stat()).isFile();
    } finally {
      await handle.close();
    }
  } catch {
    return false;
  }
}
