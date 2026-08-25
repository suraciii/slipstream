import { lstat, mkdir, realpath, stat } from "node:fs/promises";
import { isAbsolute, join, resolve, sep } from "node:path";

import {
  PhotoLibrary,
  type OriginalRecord,
  type PhotoLibraryOptions,
  type PhotoRecord,
} from "./library/photo-library.js";
import type {
  PhotoListResponse,
  PhotoSummary,
  PreviewResponse,
  PreviewSource,
} from "./protocol.js";
import {
  DerivativeScheduler,
  type DerivativeIdentity,
} from "./preview/jpeg-derivative.js";

export type ApplicationConfig = Readonly<{
  libraryRoot: string;
  stateDirectory: string;
  databaseBasename: string;
  cacheDirectory: string;
  host: string;
  port: number;
  libraryOptions?: PhotoLibraryOptions;
}>;

export type ReadyDerivative = Readonly<{
  cacheKey: string;
  cachePath: string;
  source: PreviewSource;
  width: number;
  height: number;
  limitedDetail: boolean;
  stale: boolean;
}>;

const idPattern = /^[a-f0-9]{64}$/;
const databasePattern = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const maximumJpegOriginalBytes = 128 * 1024 * 1024;

export class SlipstreamApplication {
  readonly #library: PhotoLibrary;
  readonly #scheduler: DerivativeScheduler;
  readonly #cacheDirectory: string;
  #snapshot: ReturnType<PhotoLibrary["read"]>;

  private constructor(
    library: PhotoLibrary,
    scheduler: DerivativeScheduler,
    cacheDirectory: string,
    snapshot: ReturnType<PhotoLibrary["read"]>,
  ) {
    this.#library = library;
    this.#scheduler = scheduler;
    this.#cacheDirectory = cacheDirectory;
    this.#snapshot = snapshot;
  }

  static async open(config: ApplicationConfig): Promise<SlipstreamApplication> {
    await validateConfig(config);
    await mkdir(config.stateDirectory, { recursive: true, mode: 0o700 });
    await mkdir(config.cacheDirectory, { recursive: true, mode: 0o700 });
    await validateOwnedDirectory(
      config.cacheDirectory,
      "Preview cache directory",
    );
    const library = await PhotoLibrary.open(
      config.libraryRoot,
      join(config.stateDirectory, config.databaseBasename),
      config.libraryOptions,
    );
    try {
      const snapshot = await library.scan();
      return new SlipstreamApplication(
        library,
        new DerivativeScheduler(config.cacheDirectory),
        resolve(config.cacheDirectory),
        snapshot,
      );
    } catch (error) {
      await library.shutdown();
      throw error;
    }
  }

  listPhotos(): PhotoListResponse {
    return {
      photos: this.#snapshot.photos.map((photo) => this.#summary(photo)),
    };
  }

  async rescan(): Promise<PhotoListResponse> {
    this.#snapshot = await this.#library.scan();
    return this.listPhotos();
  }

  async preview(photoId: string): Promise<PreviewResponse> {
    if (!idPattern.test(photoId)) return unavailable("Unknown Photo");
    const photo = this.#snapshot.photos.find((item) => item.id === photoId);
    if (!photo) return unavailable("Unknown Photo");
    if (!photo.available) return unavailable("Original File is unavailable");

    const jpeg = this.#original(photo.jpegOriginalId);
    const raw = this.#original(photo.rawOriginalId);
    let failure = "No usable camera-produced Preview";
    let stale: PreviewResponse | undefined;
    if (jpeg?.available) {
      const selected = await this.#matchingJpeg(photo, jpeg);
      if (selected === "unavailable")
        return unavailable(
          "Original File changed or is unavailable; rescan required",
        );
      if (selected !== "unusable") {
        if (!selected.stale) return selected;
        stale = selected;
      }
      failure = "Matching JPEG is unusable";
    }
    if (raw?.available) {
      const selected = await this.#embeddedRaw(photo, raw);
      if (selected === "unavailable")
        return unavailable(
          "Original File changed or is unavailable; rescan required",
        );
      if (selected) return selected;
      failure = "RAW embedded JPEG is unavailable";
    }
    if (stale) return stale;
    await this.#recordFailure(photo, failure);
    return { state: "failed", message: failure };
  }

  async derivative(
    photoId: string,
    cacheKey: string,
  ): Promise<ReadyDerivative | undefined> {
    if (!idPattern.test(photoId) || !idPattern.test(cacheKey)) return undefined;
    const preview = await this.preview(photoId);
    if (
      preview.state !== "ready" ||
      preview.url !== `/api/derivatives/${photoId}/${cacheKey}.jpg`
    )
      return undefined;
    return {
      cacheKey,
      cachePath: join(this.#cacheDirectory, `${cacheKey}.jpg`),
      source: preview.source!,
      width: preview.width!,
      height: preview.height!,
      limitedDetail: preview.limitedDetail!,
      stale: preview.stale ?? false,
    };
  }

  async shutdown(): Promise<void> {
    await this.#library.shutdown();
  }

  #summary(photo: PhotoRecord): PhotoSummary {
    const originals = [
      this.#original(photo.rawOriginalId),
      this.#original(photo.jpegOriginalId),
    ]
      .filter((item): item is OriginalRecord => Boolean(item))
      .map((item) => ({ kind: item.kind, available: item.available }));
    const source = photo.previewSource;
    return {
      id: photo.id,
      available: photo.available,
      ambiguous: photo.ambiguous,
      originals,
      preview: {
        state: photo.previewState,
        ...(source ? { source } : {}),
        ...(photo.previewWidth ? { width: photo.previewWidth } : {}),
        ...(photo.previewHeight ? { height: photo.previewHeight } : {}),
        ...(photo.previewWidth && photo.previewHeight
          ? {
              limitedDetail:
                Math.max(photo.previewWidth, photo.previewHeight) < 2560,
            }
          : {}),
        ...(!photo.available
          ? { message: "Original File is unavailable" }
          : {}),
      },
    };
  }

  #original(id?: string): OriginalRecord | undefined {
    return id
      ? this.#snapshot.originals.find((item) => item.id === id)
      : undefined;
  }

  async #matchingJpeg(
    photo: PhotoRecord,
    original: OriginalRecord,
  ): Promise<PreviewResponse | "unusable" | "unavailable"> {
    try {
      const actual = await this.#library
        .confinedOriginal(original.relativePath)
        .readWhole(maximumJpegOriginalBytes);
      if (!sameScannedFacts(original, actual.sourceFacts)) return "unavailable";
      return (
        (await this.#generate(
          photo,
          original,
          "matching-jpeg",
          actual.bytes,
          photo.previewCandidate ?? "matching-jpeg",
        )) ?? "unusable"
      );
    } catch {
      return "unavailable";
    }
  }

  async #embeddedRaw(
    photo: PhotoRecord,
    original: OriginalRecord,
  ): Promise<PreviewResponse | "unavailable" | undefined> {
    try {
      const extracted = await this.#library
        .confinedOriginal(original.relativePath)
        .extractEmbeddedJpeg();
      if (!sameScannedFacts(original, extracted.sourceFacts))
        return "unavailable";
      if (extracted.kind !== "preview") return undefined;
      return await this.#generate(
        photo,
        original,
        "embedded-raw-jpeg",
        extracted.jpeg,
        photo.previewCandidate ?? "embedded-raw-jpeg",
        String(extracted.candidateIndex),
      );
    } catch {
      return undefined;
    }
  }

  async #generate(
    photo: PhotoRecord,
    original: OriginalRecord,
    source: PreviewSource,
    jpeg: Buffer,
    expectedCandidate: PreviewSource,
    embeddedCandidateIdentity?: string,
  ): Promise<PreviewResponse | undefined> {
    const revision = sourceRevision(original);
    const identity: DerivativeIdentity = {
      photoIdentity: photo.id,
      source,
      sourceRelativePath: original.relativePath,
      sourceSize: original.size,
      sourceMtimeMs: original.mtimeMs,
      ...(embeddedCandidateIdentity ? { embeddedCandidateIdentity } : {}),
      targetLongEdge: 2560,
    };
    const result = await this.#scheduler.generate(identity, jpeg, {
      priority: "current",
    });
    if (result.kind !== "ready") return undefined;
    const limitedDetail = Math.max(result.width, result.height) < 2560;
    if (result.stale) {
      return {
        state: "ready",
        source: result.source,
        stale: true,
        width: result.width,
        height: result.height,
        limitedDetail,
        url: `/api/derivatives/${photo.id}/${result.cacheKey}.jpg`,
        message: "Showing a stale Preview because current generation failed",
      };
    }
    const applied = await this.#library.seedInspectedPreview({
      photoId: photo.id,
      state: "ready",
      expectedCandidate,
      expectedSourceRevision: sourceRevision(
        expectedCandidate === "matching-jpeg"
          ? this.#original(photo.jpegOriginalId)!
          : this.#original(photo.rawOriginalId)!,
      ),
      width: result.width,
      height: result.height,
      cacheRevision: result.cacheKey,
      actualSource: source,
      actualSourceRevision: revision,
    });
    if (applied.kind !== "applied")
      return unavailable("Original File changed; rescan required");
    this.#snapshot = this.#library.read();
    return {
      state: "ready",
      source,
      width: result.width,
      height: result.height,
      limitedDetail,
      stale: false,
      url: `/api/derivatives/${photo.id}/${result.cacheKey}.jpg`,
    };
  }

  async #recordFailure(photo: PhotoRecord, message: string): Promise<void> {
    void message;
    if (!photo.previewCandidate) return;
    const original =
      photo.previewCandidate === "matching-jpeg"
        ? this.#original(photo.jpegOriginalId)
        : this.#original(photo.rawOriginalId);
    if (!original) return;
    await this.#library.seedInspectedPreview({
      photoId: photo.id,
      state: "failed",
      expectedCandidate: photo.previewCandidate,
      expectedSourceRevision: sourceRevision(original),
    });
    this.#snapshot = this.#library.read();
  }
}

function sameScannedFacts(
  original: OriginalRecord,
  facts: Readonly<{ size: number; mtimeMs: number }>,
): boolean {
  return facts.size === original.size && facts.mtimeMs === original.mtimeMs;
}

function sourceRevision(original: OriginalRecord): string {
  return `${original.relativePath}\0${original.size}\0${original.mtimeMs}`;
}

function unavailable(message: string): PreviewResponse {
  return { state: "unavailable", message };
}

export function etag(cacheKey: string): string {
  return `"${cacheKey}"`;
}

export async function validateConfig(config: ApplicationConfig): Promise<void> {
  for (const [name, value] of [
    ["Photo Library root", config.libraryRoot],
    ["state directory", config.stateDirectory],
    ["Preview cache directory", config.cacheDirectory],
  ] as const) {
    if (!isAbsolute(value)) throw new Error(`${name} must be absolute`);
  }
  if (!databasePattern.test(config.databaseBasename))
    throw new Error("Database basename is invalid");
  if (!config.host || config.host.length > 255 || /[\s/]/.test(config.host))
    throw new Error("Host is invalid");
  if (!Number.isInteger(config.port) || config.port < 1 || config.port > 65535)
    throw new Error("Port is invalid");
  const root = await realpath(config.libraryRoot);
  if (!(await stat(root)).isDirectory())
    throw new Error("Photo Library root must be a directory");
  const state = resolve(config.stateDirectory);
  const cache = resolve(config.cacheDirectory);
  if (inside(root, state) || inside(root, cache))
    throw new Error("Application state must be outside the Photo Library root");
  const existingState = await realpath(state).catch(() => state);
  const existingCache = await realpath(cache).catch(() => cache);
  if (inside(root, existingState) || inside(root, existingCache))
    throw new Error("Application state must be outside the Photo Library root");
  if (
    inside(existingState, existingCache) ||
    inside(existingCache, existingState)
  )
    throw new Error("State and Preview cache directories must be separate");
}

async function validateOwnedDirectory(
  path: string,
  name: string,
): Promise<void> {
  const linkFacts = await lstat(path);
  if (linkFacts.isSymbolicLink())
    throw new Error(`${name} must not be a symbolic link`);
  const facts = await stat(path);
  if (
    !facts.isDirectory() ||
    facts.uid !== process.getuid!() ||
    (facts.mode & 0o022) !== 0
  )
    throw new Error(`${name} is not safely owned`);
}

function inside(root: string, candidate: string): boolean {
  return candidate === root || candidate.startsWith(root + sep);
}
