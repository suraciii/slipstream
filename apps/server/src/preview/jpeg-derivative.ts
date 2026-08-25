import { createHash, randomUUID } from "node:crypto";
import { mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import sharp from "sharp";

export const derivativeAlgorithmVersion = "sharp-v2";
export const derivativeConcurrencyLimit = 2;
export type DerivativeTarget = 512 | 2560;
export type DerivativePriority =
  | "current"
  | "adjacent"
  | "visible-grid"
  | "background";

export type DerivativeSource = "matching-jpeg" | "embedded-raw-jpeg";
export type DerivativeIdentity = Readonly<{
  photoIdentity: string;
  source: DerivativeSource;
  sourceRelativePath: string;
  sourceSize: number;
  sourceMtimeMs: number;
  embeddedCandidateIdentity?: string;
  targetLongEdge: DerivativeTarget;
}>;

export type DerivativeResult =
  | Readonly<{
      kind: "ready";
      cachePath: string;
      cacheKey: string;
      width: number;
      height: number;
      colorProfile: "preserved-icc" | "srgb";
      generated: boolean;
      stale: boolean;
      source: DerivativeSource;
    }>
  | Readonly<{
      kind: "malformed" | "resource-limit" | "io-error" | "internal-error";
      message: string;
    }>;

type Ready = Extract<DerivativeResult, { kind: "ready" }>;
type Failure = Readonly<{
  kind: "malformed" | "resource-limit" | "io-error" | "internal-error";
  message: string;
}>;
type PersistentFailure = Readonly<{
  kind: "malformed" | "resource-limit";
  message: string;
}>;
type Processed = Readonly<
  Pick<Ready, "width" | "height" | "colorProfile"> & { jpeg: Buffer }
>;
type ProcessJob = (
  jpeg: Buffer,
  target: DerivativeTarget,
) => Promise<Processed>;
type PreserveProfile = (pipeline: sharp.Sharp) => Promise<Processed>;
type GenerateOptions = Readonly<{
  priority?: DerivativePriority;
  retry?: boolean;
}>;
type SchedulerOptions = Readonly<{
  concurrency?: number;
  processJob?: ProcessJob;
  beforePublish?: (temporaryPath: string, finalPath: string) => Promise<void>;
  beforeManifestWrite?: (
    temporaryPath: string,
    finalPath: string,
  ) => Promise<void>;
  beforeManifestRename?: (
    temporaryPath: string,
    finalPath: string,
  ) => Promise<void>;
  beforeFailurePersist?: () => Promise<void>;
  preserveProfile?: PreserveProfile;
}>;
type Manifest = Readonly<{
  key: string;
  source: DerivativeSource;
  width: number;
  height: number;
  colorProfile: "preserved-icc" | "srgb";
}>;
type FailureRecord = Readonly<{
  key: string;
  kind: PersistentFailure["kind"];
  message: string;
}>;
type Waiter = { priority: number; order: number; resolve: () => void };
type QueuedJob = {
  priority: number;
  waiter: Waiter | undefined;
  manifestIdentity: string;
  generation: number;
};
type InFlight = { promise: Promise<DerivativeResult>; job: QueuedJob };
type SharedState = {
  concurrency: number;
  active: number;
  order: number;
  generation: number;
  waiters: Waiter[];
  inFlight: Map<string, InFlight>;
  failures: Map<string, PersistentFailure>;
  pendingManifests: Map<string, Manifest>;
  authoritativeGenerations: Map<string, number>;
  manifestWrites: Map<string, Promise<unknown>>;
};

const states = new Map<string, SharedState>();
const priorityValue: Record<DerivativePriority, number> = {
  current: 0,
  adjacent: 1,
  "visible-grid": 2,
  background: 3,
};
const maximumInputBytes = 128 * 1024 * 1024;
const maximumInputPixels = 100_000_000;
const maximumOutputBytes = 64 * 1024 * 1024;
const maximumFailureMessageLength = 512;
const keyPattern = /^[a-f0-9]{64}$/;

export function derivativeCacheKey(identity: DerivativeIdentity): string {
  return createHash("sha256")
    .update(
      JSON.stringify({
        version: derivativeAlgorithmVersion,
        photo: identity.photoIdentity,
        source: identity.source,
        path: identity.sourceRelativePath,
        size: identity.sourceSize,
        mtimeMs: identity.sourceMtimeMs,
        candidate: identity.embeddedCandidateIdentity ?? null,
        edge: identity.targetLongEdge,
      }),
    )
    .digest("hex");
}

function manifestKey(identity: DerivativeIdentity): string {
  return createHash("sha256")
    .update(
      JSON.stringify({
        photo: identity.photoIdentity,
        edge: identity.targetLongEdge,
      }),
    )
    .digest("hex");
}

export class DerivativeScheduler {
  readonly #cacheDirectory: string;
  readonly #state: SharedState;
  readonly #processJob: ProcessJob;
  readonly #beforePublish?: SchedulerOptions["beforePublish"];
  readonly #beforeManifestWrite?: SchedulerOptions["beforeManifestWrite"];
  readonly #beforeManifestRename?: SchedulerOptions["beforeManifestRename"];
  readonly #beforeFailurePersist?: SchedulerOptions["beforeFailurePersist"];

  constructor(cacheDirectory: string, options: SchedulerOptions = {}) {
    this.#cacheDirectory = resolve(cacheDirectory);
    const concurrency = options.concurrency ?? derivativeConcurrencyLimit;
    if (!Number.isInteger(concurrency) || concurrency < 1)
      throw new RangeError("Derivative concurrency must be positive");
    const existing = states.get(this.#cacheDirectory);
    if (existing && existing.concurrency !== concurrency)
      throw new Error("A cache directory has one concurrency owner");
    this.#state = existing ?? {
      concurrency,
      active: 0,
      order: 0,
      generation: 0,
      waiters: [],
      inFlight: new Map(),
      failures: new Map(),
      pendingManifests: new Map(),
      authoritativeGenerations: new Map(),
      manifestWrites: new Map(),
    };
    states.set(this.#cacheDirectory, this.#state);
    this.#processJob =
      options.processJob ??
      ((jpeg, target) => processJpeg(jpeg, target, options.preserveProfile));
    this.#beforePublish = options.beforePublish;
    this.#beforeManifestWrite = options.beforeManifestWrite;
    this.#beforeManifestRename = options.beforeManifestRename;
    this.#beforeFailurePersist = options.beforeFailurePersist;
  }

  generate(
    identity: DerivativeIdentity,
    jpeg: Buffer,
    options: GenerateOptions = {},
  ): Promise<DerivativeResult> {
    const key = derivativeCacheKey(identity);
    const priority = options.priority ?? "background";
    const requestedPriority = priorityValue[priority];
    const existing = this.#state.inFlight.get(key);
    if (existing) {
      if (requestedPriority < existing.job.priority) {
        existing.job.priority = requestedPriority;
        if (existing.job.waiter)
          existing.job.waiter.priority = requestedPriority;
      }
      return existing.promise;
    }
    const manifestIdentity = manifestKey(identity);
    const generation = ++this.#state.generation;
    this.#state.authoritativeGenerations.set(manifestIdentity, generation);
    const pending = this.#state.pendingManifests.get(manifestIdentity);
    if (pending && pending.key !== key)
      this.#state.pendingManifests.delete(manifestIdentity);
    const job: QueuedJob = {
      priority: requestedPriority,
      waiter: undefined,
      manifestIdentity,
      generation,
    };
    const promise = this.#generate(
      identity,
      key,
      jpeg,
      job,
      options.retry ?? false,
    ).finally(() => this.#state.inFlight.delete(key));
    this.#state.inFlight.set(key, { promise, job });
    return promise;
  }

  retry(
    identity: DerivativeIdentity,
    jpeg: Buffer,
    priority: DerivativePriority = "current",
  ) {
    return this.generate(identity, jpeg, { retry: true, priority });
  }

  async invalidate(identity: DerivativeIdentity): Promise<void> {
    const key = derivativeCacheKey(identity);
    this.#state.failures.delete(key);
    await rm(this.#failurePath(key), { force: true });
  }

  async #generate(
    identity: DerivativeIdentity,
    key: string,
    jpeg: Buffer,
    job: QueuedJob,
    retry: boolean,
  ): Promise<DerivativeResult> {
    const finalPath = this.#jpegPath(key);
    const priority = () => numericPriority(job.priority);
    if (retry) await this.invalidate(identity);

    const pending = this.#state.pendingManifests.get(job.manifestIdentity);
    if (pending?.key === key) {
      const current = await this.#inspectUnderLimit(
        finalPath,
        identity.targetLongEdge,
        job,
      );
      if (current && manifestMatches(pending, key, current)) {
        await this.#writeAuthoritativeManifest(identity, pending, job);
        return ready(finalPath, pending, false, false);
      }
      if (this.#isAuthoritative(job))
        this.#state.pendingManifests.delete(job.manifestIdentity);
    }

    const failure =
      this.#state.failures.get(key) ?? (await this.#readFailure(key));
    if (failure) {
      this.#state.failures.set(key, failure);
      return this.#staleOrFailure(identity, failure, priority());
    }

    const manifest = await this.#readManifest(identity);
    if (manifest?.key === key) {
      const cached = await this.#inspectUnderLimit(
        finalPath,
        identity.targetLongEdge,
        job,
      );
      if (cached && manifestMatches(manifest, key, cached))
        return ready(finalPath, manifest, false, false);
    }

    const prior = manifest && manifest.key !== key ? manifest : undefined;
    try {
      const processed = await this.#runLimited(job, () =>
        this.#processJob(jpeg, identity.targetLongEdge),
      );
      await mkdir(this.#cacheDirectory, { recursive: true });
      const temporaryPath = join(
        this.#cacheDirectory,
        `.${key}.${process.pid}.${randomUUID()}.tmp`,
      );
      try {
        await writeFile(temporaryPath, processed.jpeg, { flag: "wx" });
        await this.#beforePublish?.(temporaryPath, finalPath);
        await rename(temporaryPath, finalPath);
      } finally {
        await rm(temporaryPath, { force: true });
      }

      const published = await this.#inspectUnderLimit(
        finalPath,
        identity.targetLongEdge,
        job,
      );
      if (!published) throw new Error("Published derivative validation failed");
      const current: Manifest = {
        key,
        source: identity.source,
        width: processed.width,
        height: processed.height,
        colorProfile: processed.colorProfile,
      };
      if (!manifestMatches(current, key, published))
        throw new Error("Published derivative metadata mismatch");
      await this.#writeAuthoritativeManifest(identity, current, job);
      return ready(finalPath, current, true, false);
    } catch (error) {
      const classified = classifyError(error);
      if (isPersistentFailure(classified))
        await this.#persistFailure(key, classified);
      const latest = await this.#readManifest(identity);
      const staleManifest = latest ?? prior;
      const stale =
        staleManifest &&
        (await this.#validatedManifestDerivative(
          staleManifest,
          identity.targetLongEdge,
          job,
        ));
      if (stale)
        return ready(
          this.#jpegPath(staleManifest.key),
          staleManifest,
          false,
          true,
        );
      return classified;
    }
  }

  async #staleOrFailure(
    identity: DerivativeIdentity,
    failure: Failure,
    priority: DerivativePriority,
  ) {
    const prior = await this.#readManifest(identity);
    const stale =
      prior &&
      (await this.#validatedManifestDerivative(
        prior,
        identity.targetLongEdge,
        priority,
      ));
    return stale
      ? ready(this.#jpegPath(prior.key), prior, false, true)
      : failure;
  }

  async #validatedManifestDerivative(
    manifest: Manifest,
    target: DerivativeTarget,
    priority: DerivativePriority | QueuedJob,
  ) {
    if (!keyPattern.test(manifest.key)) return undefined;
    const inspected = await this.#inspectUnderLimit(
      this.#jpegPath(manifest.key),
      target,
      priority,
    );
    return inspected && manifestMatches(manifest, manifest.key, inspected)
      ? inspected
      : undefined;
  }

  async #inspectUnderLimit(
    path: string,
    target: DerivativeTarget,
    priority: DerivativePriority | QueuedJob,
  ) {
    try {
      return await this.#runLimited(priority, () =>
        inspectCached(path, target),
      );
    } catch {
      return undefined;
    }
  }

  async #readManifest(
    identity: DerivativeIdentity,
  ): Promise<Manifest | undefined> {
    try {
      const parsed: unknown = JSON.parse(
        await readFile(this.#manifestPath(identity), "utf8"),
      );
      return isManifest(parsed) ? parsed : undefined;
    } catch {
      return undefined;
    }
  }

  #isAuthoritative(job: QueuedJob): boolean {
    return (
      this.#state.authoritativeGenerations.get(job.manifestIdentity) ===
      job.generation
    );
  }

  async #writeAuthoritativeManifest(
    identity: DerivativeIdentity,
    manifest: Manifest,
    job: QueuedJob,
  ): Promise<void> {
    if (!this.#isAuthoritative(job)) return;
    const previous = this.#state.manifestWrites.get(job.manifestIdentity);
    const write = (async () => {
      try {
        await previous;
      } catch {
        // A prior metadata write cannot block the authoritative generation.
      }
      if (!this.#isAuthoritative(job)) return;
      try {
        await this.#writeManifest(identity, manifest, () =>
          this.#isAuthoritative(job),
        );
        if (this.#isAuthoritative(job))
          this.#state.pendingManifests.delete(job.manifestIdentity);
      } catch {
        if (this.#isAuthoritative(job))
          this.#state.pendingManifests.set(job.manifestIdentity, manifest);
      }
    })();
    this.#state.manifestWrites.set(job.manifestIdentity, write);
    try {
      await write;
    } finally {
      if (this.#state.manifestWrites.get(job.manifestIdentity) === write)
        this.#state.manifestWrites.delete(job.manifestIdentity);
    }
  }

  async #writeManifest(
    identity: DerivativeIdentity,
    manifest: Manifest,
    isCurrent: () => boolean,
  ): Promise<void> {
    const path = this.#manifestPath(identity);
    await mkdir(dirname(path), { recursive: true });
    const temporary = `${path}.${randomUUID()}.tmp`;
    try {
      await this.#beforeManifestWrite?.(temporary, path);
      await writeFile(temporary, JSON.stringify(manifest), { flag: "wx" });
      await this.#beforeManifestRename?.(temporary, path);
      if (isCurrent()) await rename(temporary, path);
    } finally {
      await rm(temporary, { force: true });
    }
  }

  async #readFailure(key: string): Promise<PersistentFailure | undefined> {
    try {
      const parsed: unknown = JSON.parse(
        await readFile(this.#failurePath(key), "utf8"),
      );
      if (!isFailureRecord(parsed) || parsed.key !== key) return undefined;
      return { kind: parsed.kind, message: sanitize(parsed.message) };
    } catch {
      return undefined;
    }
  }

  async #persistFailure(
    key: string,
    failure: PersistentFailure,
  ): Promise<void> {
    const record: FailureRecord = {
      key,
      kind: failure.kind,
      message: sanitize(failure.message),
    };
    this.#state.failures.set(key, record);
    const path = this.#failurePath(key);
    const temporary = `${path}.${randomUUID()}.tmp`;
    try {
      await this.#beforeFailurePersist?.();
      await mkdir(dirname(path), { recursive: true });
      await writeFile(temporary, JSON.stringify(record), { flag: "wx" });
      await rename(temporary, path);
    } catch {
      // Persistence is best-effort; the in-process memo still prevents request loops.
    } finally {
      try {
        await rm(temporary, { force: true });
      } catch {
        // Cleanup is also best-effort.
      }
    }
  }

  #manifestPath(identity: DerivativeIdentity) {
    return join(
      this.#cacheDirectory,
      "metadata",
      "manifests",
      `${manifestKey(identity)}.json`,
    );
  }
  #failurePath(key: string) {
    return join(this.#cacheDirectory, "metadata", "failures", `${key}.json`);
  }
  #jpegPath(key: string) {
    if (!keyPattern.test(key)) throw new Error("Invalid derivative cache key");
    return join(this.#cacheDirectory, `${key}.jpg`);
  }

  #runLimited<T>(
    priority: DerivativePriority | QueuedJob,
    job: () => Promise<T>,
  ): Promise<T> {
    return this.#acquire(priority).then(async () => {
      try {
        return await job();
      } finally {
        this.#release();
      }
    });
  }

  async #acquire(priority: DerivativePriority | QueuedJob): Promise<void> {
    if (
      this.#state.active < this.#state.concurrency &&
      this.#state.waiters.length === 0
    ) {
      this.#state.active++;
      return;
    }
    await new Promise<void>((resolve) => {
      const waiter: Waiter = {
        priority:
          typeof priority === "string"
            ? priorityValue[priority]
            : priority.priority,
        order: this.#state.order++,
        resolve,
      };
      if (typeof priority !== "string") priority.waiter = waiter;
      this.#state.waiters.push(waiter);
    });
    if (typeof priority !== "string") priority.waiter = undefined;
  }
  #release(): void {
    if (this.#state.waiters.length === 0) {
      this.#state.active--;
      return;
    }
    this.#state.waiters.sort(
      (a, b) => a.priority - b.priority || a.order - b.order,
    );
    this.#state.waiters.shift()!.resolve();
  }
}

function ready(
  path: string,
  manifest: Manifest,
  generated: boolean,
  stale: boolean,
): Ready {
  return {
    kind: "ready",
    cachePath: path,
    cacheKey: manifest.key,
    width: manifest.width,
    height: manifest.height,
    colorProfile: manifest.colorProfile,
    generated,
    stale,
    source: manifest.source,
  };
}

async function processJpeg(
  jpeg: Buffer,
  target: DerivativeTarget,
  preserveProfile: PreserveProfile = preserveProfiledJpeg,
): Promise<Processed> {
  if (jpeg.length === 0 || jpeg.length > maximumInputBytes)
    throw new ResourceLimitError("JPEG input exceeds byte limit");
  const inputOptions = {
    failOn: "warning" as const,
    limitInputPixels: maximumInputPixels,
  };
  const embeddedIcc = extractIccPayload(jpeg);
  let sourceJpeg =
    embeddedIcc && !basicIccStructureIsSane(embeddedIcc)
      ? stripIccMarkers(jpeg)
      : jpeg;
  let metadata;
  try {
    metadata = await sharp(sourceJpeg, inputOptions).metadata();
  } catch (error) {
    const stripped = stripIccMarkers(jpeg);
    if (stripped === jpeg) throw error;
    sourceJpeg = stripped;
    metadata = await sharp(sourceJpeg, inputOptions).metadata();
  }
  if (
    !metadata.width ||
    !metadata.height ||
    metadata.width * metadata.height > maximumInputPixels
  )
    throw new ResourceLimitError("JPEG input exceeds pixel limit");

  const resized = () =>
    sharp(sourceJpeg, inputOptions).autoOrient().resize({
      width: target,
      height: target,
      fit: "inside",
      withoutEnlargement: true,
      kernel: sharp.kernel.lanczos3,
    });
  const encode = (pipeline: sharp.Sharp) =>
    pipeline
      .jpeg({ quality: 85, chromaSubsampling: "4:4:4", mozjpeg: true })
      .toBuffer({ resolveWithObject: true });

  let result: Awaited<ReturnType<typeof encode>>;
  let colorProfile: Ready["colorProfile"] = "srgb";
  if (metadata.icc && metadata.channels === 3 && metadata.space !== "cmyk") {
    try {
      await sharp(sourceJpeg, inputOptions)
        .toColourspace("srgb")
        .raw()
        .toBuffer();
      const preserved = await preserveProfile(resized());
      result = {
        data: preserved.jpeg,
        info: {
          width: preserved.width,
          height: preserved.height,
        },
      } as Awaited<ReturnType<typeof encode>>;
      colorProfile = preserved.colorProfile;
    } catch {
      result = await encode(
        resized().toColourspace("srgb").withIccProfile("srgb"),
      );
    }
  } else if (metadata.icc) {
    try {
      result = await encode(
        resized().toColourspace("srgb").withIccProfile("srgb"),
      );
    } catch {
      result = await encode(
        srgbFromUnprofiled(sourceJpeg, inputOptions, target),
      );
    }
  } else {
    result = await encode(
      resized().toColourspace("srgb").withIccProfile("srgb"),
    );
  }
  if (result.data.length > maximumOutputBytes)
    throw new ResourceLimitError("Derivative exceeds output byte limit");
  return {
    jpeg: result.data,
    width: result.info.width,
    height: result.info.height,
    colorProfile,
  };
}

async function preserveProfiledJpeg(pipeline: sharp.Sharp): Promise<Processed> {
  const result = await pipeline
    .keepIccProfile()
    .jpeg({ quality: 85, chromaSubsampling: "4:4:4", mozjpeg: true })
    .toBuffer({ resolveWithObject: true });
  await sharp(result.data, {
    failOn: "warning",
    limitInputPixels: maximumInputPixels,
  })
    .toColourspace("srgb")
    .raw()
    .toBuffer();
  return {
    jpeg: result.data,
    width: result.info.width,
    height: result.info.height,
    colorProfile: "preserved-icc",
  };
}

async function inspectCached(path: string, target: DerivativeTarget) {
  const file = await stat(path);
  if (!file.isFile() || file.size === 0 || file.size > maximumOutputBytes)
    return undefined;
  const image = sharp(path, {
    failOn: "warning",
    limitInputPixels: maximumInputPixels,
  });
  const metadata = await image.metadata();
  if (
    metadata.format !== "jpeg" ||
    !metadata.width ||
    !metadata.height ||
    Math.max(metadata.width, metadata.height) > target ||
    (metadata.orientation && metadata.orientation !== 1) ||
    !metadata.icc
  )
    return undefined;
  await sharp(path, { failOn: "warning", limitInputPixels: maximumInputPixels })
    .toColourspace("srgb")
    .raw()
    .toBuffer();
  return { width: metadata.width, height: metadata.height };
}

function srgbFromUnprofiled(
  jpeg: Buffer,
  inputOptions: { failOn: "warning"; limitInputPixels: number },
  target: DerivativeTarget,
): sharp.Sharp {
  return sharp(stripIccMarkers(jpeg), inputOptions)
    .autoOrient()
    .resize({
      width: target,
      height: target,
      fit: "inside",
      withoutEnlargement: true,
      kernel: sharp.kernel.lanczos3,
    })
    .toColourspace("srgb")
    .withIccProfile("srgb");
}

function basicIccStructureIsSane(icc: Buffer): boolean {
  if (icc.length < 132 || icc.readUInt32BE(0) !== icc.length) return false;
  const count = icc.readUInt32BE(128);
  if (count > 4096 || 132 + count * 12 > icc.length) return false;
  for (let index = 0; index < count; index++) {
    const offset = 132 + index * 12;
    const dataOffset = icc.readUInt32BE(offset + 4);
    const dataSize = icc.readUInt32BE(offset + 8);
    if (dataOffset > icc.length || dataSize > icc.length - dataOffset)
      return false;
  }
  return true;
}

function extractIccPayload(jpeg: Buffer): Buffer | undefined {
  const chunks: Array<{ sequence: number; total: number; data: Buffer }> = [];
  scanJpegMarkers(jpeg, (marker, payload) => {
    if (
      marker === 0xe2 &&
      payload.length >= 14 &&
      payload.subarray(0, 12).toString("latin1") === "ICC_PROFILE\0"
    )
      chunks.push({
        sequence: payload[12]!,
        total: payload[13]!,
        data: payload.subarray(14),
      });
  });
  if (chunks.length === 0) return undefined;
  const total = chunks[0]!.total;
  if (
    total === 0 ||
    chunks.length !== total ||
    chunks.some((item) => item.total !== total)
  )
    return Buffer.alloc(0);
  chunks.sort((a, b) => a.sequence - b.sequence);
  if (chunks.some((item, index) => item.sequence !== index + 1))
    return Buffer.alloc(0);
  return Buffer.concat(chunks.map((item) => item.data));
}

function scanJpegMarkers(
  jpeg: Buffer,
  visit: (marker: number, payload: Buffer) => void,
): void {
  if (jpeg.length < 4 || jpeg[0] !== 0xff || jpeg[1] !== 0xd8) return;
  let offset = 2;
  while (offset + 4 <= jpeg.length && jpeg[offset] === 0xff) {
    const marker = jpeg[offset + 1]!;
    if (marker === 0xda || marker === 0xd9) return;
    const length = jpeg.readUInt16BE(offset + 2);
    if (length < 2 || offset + 2 + length > jpeg.length) return;
    visit(marker, jpeg.subarray(offset + 4, offset + 2 + length));
    offset += 2 + length;
  }
}

function stripIccMarkers(jpeg: Buffer): Buffer {
  if (jpeg.length < 4 || jpeg[0] !== 0xff || jpeg[1] !== 0xd8) return jpeg;
  const parts: Buffer[] = [jpeg.subarray(0, 2)];
  let offset = 2;
  while (offset + 4 <= jpeg.length && jpeg[offset] === 0xff) {
    const marker = jpeg[offset + 1]!;
    if (marker === 0xda || marker === 0xd9) break;
    if (marker === 0x00 || (marker >= 0xd0 && marker <= 0xd7)) {
      parts.push(jpeg.subarray(offset, offset + 2));
      offset += 2;
      continue;
    }
    const length = jpeg.readUInt16BE(offset + 2);
    if (length < 2 || offset + 2 + length > jpeg.length) return jpeg;
    const end = offset + 2 + length;
    const isIcc =
      marker === 0xe2 &&
      length >= 16 &&
      jpeg.subarray(offset + 4, offset + 16).toString("latin1") ===
        "ICC_PROFILE\0";
    if (!isIcc) parts.push(jpeg.subarray(offset, end));
    offset = end;
  }
  parts.push(jpeg.subarray(offset));
  return Buffer.concat(parts);
}

function manifestMatches(
  manifest: Manifest,
  key: string,
  inspected: Readonly<{ width: number; height: number }>,
): boolean {
  return (
    manifest.key === key &&
    keyPattern.test(manifest.key) &&
    manifest.width === inspected.width &&
    manifest.height === inspected.height &&
    (manifest.colorProfile === "preserved-icc" ||
      manifest.colorProfile === "srgb")
  );
}
function isManifest(value: unknown): value is Manifest {
  if (!value || typeof value !== "object") return false;
  const item = value as Record<string, unknown>;
  if (
    Object.keys(item).sort().join(",") !==
    "colorProfile,height,key,source,width"
  )
    return false;
  return (
    typeof item.key === "string" &&
    keyPattern.test(item.key) &&
    (item.source === "matching-jpeg" || item.source === "embedded-raw-jpeg") &&
    Number.isInteger(item.width) &&
    Number(item.width) > 0 &&
    Number.isInteger(item.height) &&
    Number(item.height) > 0 &&
    (item.colorProfile === "preserved-icc" || item.colorProfile === "srgb")
  );
}
function isFailureRecord(value: unknown): value is FailureRecord {
  if (!value || typeof value !== "object") return false;
  const item = value as Record<string, unknown>;
  if (Object.keys(item).sort().join(",") !== "key,kind,message") return false;
  return (
    typeof item.key === "string" &&
    keyPattern.test(item.key) &&
    (item.kind === "malformed" || item.kind === "resource-limit") &&
    typeof item.message === "string" &&
    item.message.length <= maximumFailureMessageLength
  );
}
function isPersistentFailure(failure: Failure): failure is PersistentFailure {
  return failure.kind === "malformed" || failure.kind === "resource-limit";
}

class ResourceLimitError extends Error {}
function classifyError(error: unknown): Failure {
  const message = sanitize(
    error instanceof Error ? error.message : "Derivative processing failed",
  );
  if (error instanceof ResourceLimitError)
    return { kind: "resource-limit", message };
  if (isFileSystemError(error)) return { kind: "io-error", message };
  if (
    error instanceof Error &&
    /Input buffer|jpeg|image|icc profile/i.test(error.message)
  )
    return { kind: "malformed", message };
  return { kind: "internal-error", message };
}
function sanitize(message: string): string {
  return message
    .replace(/(?:\/[\w .@-]+)+/g, "[path]")
    .slice(0, maximumFailureMessageLength);
}
function isFileSystemError(error: unknown) {
  return typeof error === "object" && error !== null && "code" in error;
}

function numericPriority(value: number): DerivativePriority {
  if (value === priorityValue.current) return "current";
  if (value === priorityValue.adjacent) return "adjacent";
  if (value === priorityValue["visible-grid"]) return "visible-grid";
  return "background";
}

export function resetDerivativeSchedulerStateForTests(
  cacheDirectory: string,
): void {
  states.delete(resolve(cacheDirectory));
}
