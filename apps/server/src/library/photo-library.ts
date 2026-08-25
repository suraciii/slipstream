import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { lstat, mkdir, open, realpath, stat } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
import { Worker } from "node:worker_threads";
import { fileURLToPath } from "node:url";

import type { EmbeddedJpegOutcome } from "../preview/libraw-preview.js";
import type { OriginalKind } from "./file-kinds.js";

export type PreviewState =
  | "inspection-pending"
  | "ready"
  | "failed"
  | "unavailable";
export type PreviewCandidate = "matching-jpeg" | "embedded-raw-jpeg";
export type ErrorCategory = "unreadable" | "changed";
export type OriginalRecord = Readonly<{
  id: string;
  relativePath: string;
  kind: OriginalKind;
  size: number;
  mtimeMs: number;
  available: boolean;
  errorCategory?: ErrorCategory;
  errorMessage?: string;
}>;
export type PhotoRecord = Readonly<{
  id: string;
  rawOriginalId?: string;
  jpegOriginalId?: string;
  ambiguous: boolean;
  available: boolean;
  previewState: PreviewState;
  previewCandidate?: PreviewCandidate;
  previewSource?: PreviewCandidate;
  previewSourceRevision?: string;
  previewWidth?: number;
  previewHeight?: number;
  cacheRevision?: string;
}>;
export type ScanResult = Readonly<{
  originals: ReadonlyArray<OriginalRecord>;
  photos: ReadonlyArray<PhotoRecord>;
  errors: ReadonlyArray<{
    relativePath: string;
    category: ErrorCategory;
    message: string;
  }>;
}>;
export type ConfinedOriginal = Readonly<{
  relativePath: string;
  facts(): Promise<Readonly<{ size: number; mtimeMs: number; mode: number }>>;
  read(offset: number, length: number): Promise<Buffer>;
  extractEmbeddedJpeg(): Promise<EmbeddedJpegOutcome>;
}>;

type PhotoLibraryOptions = Readonly<{
  maximumFiles?: number;
  maximumEntries?: number;
  maximumEntriesPerDirectory?: number;
  failureHook?: "after-first-original";
  startupFailure?: boolean;
  beforeWorkerStart?: () => Promise<void> | void;
  beforeDirectoryRecursion?: (relativePath: string) => Promise<void> | void;
  beforeConfinedOperation?: (
    operation: "facts" | "read" | "extract",
    relativePath: string,
  ) => Promise<void> | void;
}>;
type WorkerResponse = {
  id?: number;
  ready?: boolean;
  result?: unknown;
  error?: string;
  hook?: "beforeDirectoryRecursion" | "beforeConfinedOperation";
  hookId?: number;
  operation?: "facts" | "read" | "extract";
  relativePath?: string;
};
type Pending = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
};
export type PreviewSeedResult = Readonly<{ kind: "applied" | "stale-ignored" }>;
type StateFileIdentity = Readonly<{
  device: bigint;
  inode: bigint;
  uid: number;
  mode: number;
  linkCount: number;
}>;
type PreparedStateFile = { kind: "prepared" } & StateFileIdentity;
type NativeBinding = {
  prepareStateFile(
    stateFd: number,
    name: string,
  ): PreparedStateFile | { kind: "io-error"; message: string };
  admitStateSidecars(
    stateFd: number,
    name: string,
  ): { kind: "admitted" } | { kind: "io-error"; message: string };
};

const workerPath = fileURLToPath(
  new URL("./photo-library-worker.cjs", import.meta.url),
);
const addonPath = fileURLToPath(
  new URL("../../build/Release/raw_preview.node", import.meta.url),
);
const fileKindsPath = fileURLToPath(
  new URL("./file-kinds.cjs", import.meta.url),
);

export class PhotoLibrary {
  readonly #root: string;
  readonly #rootHandle: Awaited<ReturnType<typeof open>>;
  readonly #stateHandle: Awaited<ReturnType<typeof open>>;
  readonly #worker: Worker;
  readonly #pending = new Map<number, Pending>();
  readonly #failureHook: PhotoLibraryOptions["failureHook"];
  readonly #beforeDirectoryRecursion: PhotoLibraryOptions["beforeDirectoryRecursion"];
  readonly #beforeConfinedOperation: PhotoLibraryOptions["beforeConfinedOperation"];
  #nextId = 1;
  #scanPromise: Promise<ScanResult> | undefined;
  #shutdownPromise: Promise<void> | undefined;
  #closed = false;
  #snapshot: ScanResult = { originals: [], photos: [], errors: [] };

  private constructor(
    root: string,
    rootHandle: Awaited<ReturnType<typeof open>>,
    stateHandle: Awaited<ReturnType<typeof open>>,
    worker: Worker,
    options: PhotoLibraryOptions,
  ) {
    this.#root = root;
    this.#rootHandle = rootHandle;
    this.#stateHandle = stateHandle;
    this.#worker = worker;
    this.#failureHook = options.failureHook;
    this.#beforeDirectoryRecursion = options.beforeDirectoryRecursion;
    this.#beforeConfinedOperation = options.beforeConfinedOperation;
    worker.on("message", (message: WorkerResponse) => this.#onMessage(message));
    worker.on("error", (error: Error) => this.#rejectAll(error));
    worker.on("exit", (code) => {
      if (!this.#closed && code !== 0)
        this.#rejectAll(new Error("Photo Library worker stopped unexpectedly"));
    });
  }

  static async open(
    rootPath: string,
    databasePath: string,
    options: PhotoLibraryOptions = {},
  ): Promise<PhotoLibrary> {
    if (process.platform !== "linux")
      throw new Error(
        "Photo Library confinement is currently supported on Linux only",
      );
    if (!isAbsolute(rootPath))
      throw new Error("Photo Library root must be absolute");
    const canonicalRoot = await realpath(rootPath);
    if (!(await stat(canonicalRoot)).isDirectory())
      throw new Error("Photo Library root must be a readable directory");
    const { directory: stateDirectory, basename: databaseBasename } =
      await validateStateDirectory(canonicalRoot, databasePath);
    const directoryFlags =
      constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW;
    let rootHandle: Awaited<ReturnType<typeof open>> | undefined;
    let stateHandle: Awaited<ReturnType<typeof open>> | undefined;
    let prepared: PreparedStateFile;
    try {
      rootHandle = await open(canonicalRoot, directoryFlags);
      stateHandle = await open(stateDirectory, directoryFlags);
      const openedStateFacts = await stateHandle.stat();
      if (
        !openedStateFacts.isDirectory() ||
        openedStateFacts.uid !== process.getuid!() ||
        (openedStateFacts.mode & 0o022) !== 0
      )
        throw new Error("SQLite state directory is not safely owned");
      const preparedResult = nativeBinding().prepareStateFile(
        stateHandle.fd,
        databaseBasename,
      );
      if (preparedResult.kind === "io-error")
        throw new Error("SQLite database could not be created safely");
      prepared = preparedResult;
      const sidecars = nativeBinding().admitStateSidecars(
        stateHandle.fd,
        databaseBasename,
      );
      if (sidecars.kind !== "admitted")
        throw new Error("SQLite sidecar is not safely owned");
      await options.beforeWorkerStart?.();
    } catch (error) {
      await rootHandle?.close();
      await stateHandle?.close();
      throw error;
    }
    // The 0700 state directory is single-process owned. Mutation by another
    // same-uid process after this admission check is outside that boundary.
    const stateIdentity: StateFileIdentity = {
      device: prepared.device,
      inode: prepared.inode,
      uid: prepared.uid,
      mode: prepared.mode,
      linkCount: prepared.linkCount,
    };
    let worker: Worker;
    try {
      worker = new Worker(workerPath, {
        workerData: {
          root: canonicalRoot,
          rootFd: rootHandle.fd,
          stateFd: stateHandle.fd,
          databaseBasename,
          stateIdentity,
          addonPath,
          fileKindsPath,
          maximumFiles: positive(
            options.maximumFiles ?? 100_000,
            "recognized file",
          ),
          maximumEntries: positive(
            options.maximumEntries ?? 250_000,
            "total entry",
          ),
          maximumEntriesPerDirectory: positiveUint32(
            options.maximumEntriesPerDirectory ?? 25_000,
            "directory entry",
          ),
          startupFailure: options.startupFailure ?? false,
          beforeDirectoryRecursion:
            options.beforeDirectoryRecursion !== undefined,
          beforeConfinedOperation:
            options.beforeConfinedOperation !== undefined,
        },
      });
    } catch (error) {
      await rootHandle.close();
      await stateHandle.close();
      throw error;
    }
    const library = new PhotoLibrary(
      canonicalRoot,
      rootHandle,
      stateHandle,
      worker,
      options,
    );
    try {
      await library.#ready();
      return library;
    } catch (error) {
      await library.shutdown();
      throw error;
    }
  }

  get canonicalRoot(): string {
    return this.#root;
  }

  scan(): Promise<ScanResult> {
    this.#assertOpen();
    if (this.#scanPromise) return this.#scanPromise;
    const promise = this.#request<RawScanResult>("scan", {
      failureHook: this.#failureHook,
    })
      .then((value) => (this.#snapshot = normalizeResult(value)))
      .finally(() => {
        if (this.#scanPromise === promise) this.#scanPromise = undefined;
      });
    this.#scanPromise = promise;
    return promise;
  }

  read(): ScanResult {
    return this.#snapshot;
  }

  async refresh(): Promise<ScanResult> {
    this.#assertOpen();
    this.#snapshot = normalizeResult(
      await this.#request<RawScanResult>("read"),
    );
    return this.#snapshot;
  }

  async seedInspectedPreview(input: {
    photoId: string;
    state: "ready" | "failed";
    expectedCandidate: PreviewCandidate;
    expectedSourceRevision: string;
    width?: number;
    height?: number;
    cacheRevision?: string;
  }): Promise<PreviewSeedResult> {
    this.#assertOpen();
    const result = await this.#request<PreviewSeedResult>("seedPreview", input);
    await this.refresh();
    return result;
  }

  confinedOriginal(relativePath: string): ConfinedOriginal {
    this.#assertOpen();
    const normalized = normalizeRelativePath(relativePath);
    return {
      relativePath: normalized,
      facts: async () => {
        this.#assertOpen();
        return this.#request("confinedFacts", { path: normalized });
      },
      read: async (offset, length) => {
        this.#assertOpen();
        validateReadRange(offset, length);
        const value = await this.#request<Uint8Array>("confinedRead", {
          path: normalized,
          offset,
          length,
        });
        return Buffer.from(value);
      },
      extractEmbeddedJpeg: async () => {
        this.#assertOpen();
        const outcome = await this.#request<EmbeddedJpegOutcome>(
          "confinedExtract",
          { path: normalized },
        );
        return outcome.kind === "preview"
          ? { ...outcome, jpeg: Buffer.from(outcome.jpeg) }
          : outcome;
      },
    };
  }

  shutdown(): Promise<void> {
    if (this.#shutdownPromise) return this.#shutdownPromise;
    this.#closed = true;
    this.#shutdownPromise = (async () => {
      try {
        await this.#scanPromise?.catch(() => undefined);
        if (this.#worker.threadId !== -1)
          await this.#request("shutdown").catch(() => undefined);
      } finally {
        await this.#worker.terminate();
        await this.#rootHandle.close();
        await this.#stateHandle.close();
      }
    })();
    return this.#shutdownPromise;
  }

  close(): void {
    void this.shutdown();
  }

  #ready(): Promise<void> {
    return new Promise((resolve, reject) => {
      const cleanup = () => {
        this.#worker.off("message", onMessage);
        this.#worker.off("error", onError);
        this.#worker.off("exit", onExit);
      };
      const onMessage = (message: WorkerResponse) => {
        if (message.ready === undefined) return;
        cleanup();
        if (message.ready) resolve();
        else
          reject(
            new Error(message.error ?? "Photo Library initialization failed"),
          );
      };
      const onError = () => {
        cleanup();
        reject(new Error("Photo Library worker failed during startup"));
      };
      const onExit = () => {
        cleanup();
        reject(new Error("Photo Library worker exited during startup"));
      };
      this.#worker.on("message", onMessage);
      this.#worker.once("error", onError);
      this.#worker.once("exit", onExit);
    });
  }

  #request<T = unknown>(command: string, payload?: unknown): Promise<T> {
    const id = this.#nextId++;
    return new Promise<T>((resolve, reject) => {
      this.#pending.set(id, {
        resolve: resolve as (value: unknown) => void,
        reject,
      });
      this.#worker.postMessage({ id, command, payload });
    });
  }

  #onMessage(message: WorkerResponse): void {
    if (message.hook !== undefined && message.hookId !== undefined) {
      const operation =
        message.hook === "beforeDirectoryRecursion"
          ? this.#beforeDirectoryRecursion?.(message.relativePath ?? "")
          : this.#beforeConfinedOperation?.(
              message.operation ?? "facts",
              message.relativePath ?? "",
            );
      void Promise.resolve(operation).finally(() =>
        this.#worker.postMessage({ hookId: message.hookId }),
      );
      return;
    }
    if (message.id === undefined) return;
    const pending = this.#pending.get(message.id);
    if (!pending) return;
    this.#pending.delete(message.id);
    if (message.error) pending.reject(new Error(message.error));
    else pending.resolve(message.result);
  }

  #rejectAll(error: Error): void {
    for (const pending of this.#pending.values()) pending.reject(error);
    this.#pending.clear();
  }

  #assertOpen(): void {
    if (this.#closed) throw new Error("Photo Library is closed");
  }
}

const require = createRequire(import.meta.url);
let binding: NativeBinding | undefined;
function nativeBinding(): NativeBinding {
  binding ??= requireAddon(addonPath);
  return binding;
}
function requireAddon(path: string): NativeBinding {
  return require(path) as NativeBinding;
}

async function validateStateDirectory(
  root: string,
  databasePath: string,
): Promise<{ directory: string; basename: string }> {
  if (!isAbsolute(databasePath))
    throw new Error("SQLite database path must be absolute");
  const resolved = resolve(databasePath);
  const databaseBasename = resolved.slice(dirname(resolved).length + 1);
  if (
    !databaseBasename ||
    databaseBasename.includes("/") ||
    databaseBasename.includes("\\") ||
    databaseBasename === "." ||
    databaseBasename === ".."
  )
    throw new Error("SQLite database path must name one file");
  const stateDirectory = dirname(resolved);
  // Reject lexical placement before mkdir so an invalid configuration cannot
  // create application state inside the Original File tree.
  rejectUnderRoot(root, stateDirectory);
  await mkdir(stateDirectory, { recursive: true, mode: 0o700 });
  const directoryFacts = await lstat(stateDirectory);
  if (directoryFacts.isSymbolicLink() || !directoryFacts.isDirectory())
    throw new Error("SQLite state directory must be a real directory");
  const canonicalDirectory = await realpath(stateDirectory);
  rejectUnderRoot(root, canonicalDirectory);
  if (
    typeof process.getuid !== "function" ||
    directoryFacts.uid !== process.getuid()
  )
    throw new Error("SQLite state directory must be owned by the server user");
  if ((directoryFacts.mode & 0o022) !== 0)
    throw new Error(
      "SQLite state directory must not be group or other writable",
    );
  try {
    const fileFacts = await lstat(resolved);
    if (fileFacts.isSymbolicLink() || !fileFacts.isFile())
      throw new Error("SQLite database path must be a regular file");
  } catch (error) {
    if (!isNotFound(error)) throw error;
  }
  return { directory: canonicalDirectory, basename: databaseBasename };
}
function rejectUnderRoot(root: string, candidate: string): void {
  const rel = relative(root, candidate);
  if (
    rel === "" ||
    (!rel.startsWith(`..${sep}`) && rel !== ".." && !isAbsolute(rel))
  )
    throw new Error(
      "SQLite database must be outside the read-only Photo Library root",
    );
}
function validateReadRange(offset: number, length: number): void {
  if (
    !Number.isSafeInteger(offset) ||
    offset < 0 ||
    !Number.isSafeInteger(length) ||
    length < 0 ||
    length > 16 * 1024 * 1024
  )
    throw new RangeError("Original read range is invalid");
}
function normalizeRelativePath(value: string): string {
  if (typeof value !== "string" || isAbsolute(value) || value.includes("\0"))
    throw new Error("Original path must be a valid relative path");
  if (
    !value ||
    value.split("/").some((part) => !part || part === "." || part === "..")
  )
    throw new Error("Original path escapes the Photo Library root");
  return value;
}
function positive(value: number, label: string): number {
  if (!Number.isSafeInteger(value) || value < 1)
    throw new RangeError(`${label} limit must be positive`);
  return value;
}
function positiveUint32(value: number, label: string): number {
  const checked = positive(value, label);
  if (checked > 0xffffffff)
    throw new RangeError(`${label} limit must fit an unsigned 32-bit integer`);
  return checked;
}
function isNotFound(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    error.code === "ENOENT"
  );
}

type RawScanResult = {
  originals: RawOriginal[];
  photos: RawPhoto[];
  errors?: RawError[];
};
type RawOriginal = {
  id: string;
  relative_path: string;
  kind: OriginalKind;
  size: number;
  mtime_ms: number;
  available: number;
  error_category: ErrorCategory | null;
  error_message: string | null;
};
type RawPhoto = {
  id: string;
  raw_original_id: string | null;
  jpeg_original_id: string | null;
  ambiguous: number;
  available: number;
  preview_state: PreviewState;
  preview_candidate: PreviewCandidate | null;
  preview_source: PreviewCandidate | null;
  preview_source_revision: string | null;
  preview_width: number | null;
  preview_height: number | null;
  cache_revision: string | null;
};
type RawError = {
  relativePath: string;
  category: ErrorCategory;
  message: string;
};
function normalizeResult(value: RawScanResult): ScanResult {
  const originals = value.originals.map((row) =>
    Object.freeze({
      id: row.id,
      relativePath: row.relative_path,
      kind: row.kind,
      size: row.size,
      mtimeMs: row.mtime_ms,
      available: Boolean(row.available),
      ...(row.error_category
        ? {
            errorCategory: row.error_category,
            errorMessage:
              row.error_message ?? "Original File inspection failed",
          }
        : {}),
    }),
  );
  const photos = value.photos.map((row) =>
    Object.freeze({
      id: row.id,
      ...(row.raw_original_id ? { rawOriginalId: row.raw_original_id } : {}),
      ...(row.jpeg_original_id ? { jpegOriginalId: row.jpeg_original_id } : {}),
      ambiguous: Boolean(row.ambiguous),
      available: Boolean(row.available),
      previewState: row.preview_state,
      ...(row.preview_candidate
        ? { previewCandidate: row.preview_candidate }
        : {}),
      ...(row.preview_source ? { previewSource: row.preview_source } : {}),
      ...(row.preview_source_revision
        ? { previewSourceRevision: row.preview_source_revision }
        : {}),
      ...(row.preview_width ? { previewWidth: row.preview_width } : {}),
      ...(row.preview_height ? { previewHeight: row.preview_height } : {}),
      ...(row.cache_revision ? { cacheRevision: row.cache_revision } : {}),
    }),
  );
  const errors = (value.errors ?? []).map((error) =>
    Object.freeze({ ...error }),
  );
  return Object.freeze({
    originals: Object.freeze(originals),
    photos: Object.freeze(photos),
    errors: Object.freeze(errors),
  });
}

export function photoManifestSeed(photoId: string): string {
  return createHash("sha256").update(photoId).digest("hex");
}
