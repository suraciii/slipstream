import { describe, expect, test } from "bun:test";
import type { PhotoAuthority } from "./photo-owner.js";
import {
  createSavedPositionOwner,
  type SavedPositionAuthorityPort,
  type SavedPositionFetch,
  type SavedPositionTarget,
} from "./saved-position-owner.js";
import type { SourceAuthority } from "./source-grid-owner.js";

type Deferred<T> = Readonly<{
  promise: Promise<T>;
  resolve(value: T): void;
  reject(reason?: unknown): void;
}>;

const deferred = <T>(): Deferred<T> => {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
};

const sourceAuthority = () => Object.freeze({}) as SourceAuthority;
const photoAuthority = () => Object.freeze({}) as PhotoAuthority;

const target = (suffix: string): SavedPositionTarget =>
  Object.freeze({
    sourceAuthority: sourceAuthority(),
    photoAuthority: photoAuthority(),
    albumId: `album-${suffix}`,
    photoId: `photo-${suffix}`,
  });

const currentPort = (initial: SavedPositionTarget) => {
  let current = initial;
  const port: SavedPositionAuthorityPort = {
    isSourceCurrent: (authority, albumId) =>
      current.sourceAuthority === authority && current.albumId === albumId,
    isPhotoCurrent: (authority, photoId) =>
      current.photoAuthority === authority && current.photoId === photoId,
  };
  return {
    port,
    setCurrent(next: SavedPositionTarget) {
      current = next;
    },
  };
};

describe("SavedPositionOwner", () => {
  test("serializes admitted writes and skips queued stale targets before HTTP", async () => {
    const first = target("first");
    const skipped = target("skipped");
    const current = target("current");
    const authorities = currentPort(first);
    const firstResponse = deferred<Response>();
    const currentResponse = deferred<Response>();
    const firstStarted = deferred<void>();
    const requests: Array<Readonly<{ path: string; photoId: string }>> = [];
    let active = 0;
    let maximumActive = 0;
    const fetcher: SavedPositionFetch = async (path, init) => {
      const body = JSON.parse(
        typeof init?.body === "string" ? init.body : "null",
      ) as { photoId: string };
      requests.push({ path, photoId: body.photoId });
      active += 1;
      maximumActive = Math.max(maximumActive, active);
      if (requests.length === 1) firstStarted.resolve();
      try {
        return await (requests.length === 1
          ? firstResponse.promise
          : currentResponse.promise);
      } finally {
        active -= 1;
      }
    };
    const owner = createSavedPositionOwner(fetcher, authorities.port);
    const firstAdmission = owner.save(first)!;
    await firstStarted.promise;
    const skippedAdmission = owner.save(skipped)!;
    const currentAdmission = owner.save(current)!;
    authorities.setCurrent(current);

    firstResponse.resolve(new Response(null, { status: 200 }));
    expect((await firstAdmission.settlement).kind).toBe("detached");
    expect((await skippedAdmission.settlement).kind).toBe("skipped");
    await Promise.resolve();
    expect(requests).toEqual([
      {
        path: "/api/albums/album-first/progress",
        photoId: "photo-first",
      },
      {
        path: "/api/albums/album-current/progress",
        photoId: "photo-current",
      },
    ]);
    expect(maximumActive).toBe(1);
    currentResponse.resolve(new Response(null, { status: 200 }));
    expect((await currentAdmission.settlement).kind).toBe("confirmed");
    owner.dispose();
  });

  test("classifies confirmed, stale, and answered failures without policy leakage", async () => {
    for (const expected of [
      { status: 200, kind: "confirmed" },
      { status: 404, kind: "stale" },
      { status: 409, kind: "stale" },
      { status: 503, kind: "failed" },
    ] as const) {
      const captured = target(String(expected.status));
      const authorities = currentPort(captured);
      const owner = createSavedPositionOwner(
        () => Promise.resolve(new Response(null, { status: expected.status })),
        authorities.port,
      );
      const outcome = await owner.save(captured)!.settlement;
      expect(outcome.kind).toBe(expected.kind);
      if (outcome.kind === "failed") {
        expect(outcome.status).toBe(503);
        expect(outcome.transportLost).toBe(false);
      }
      owner.dispose();
    }
  });

  test("reports current transport failure and detaches every stale settlement", async () => {
    const current = target("current");
    const authorities = currentPort(current);
    let attempts = 0;
    const owner = createSavedPositionOwner(() => {
      attempts += 1;
      return attempts === 1
        ? Promise.reject(new Error("offline"))
        : Promise.resolve(new Response(null, { status: 200 }));
    }, authorities.port);
    const first = owner.save(current)!;
    const next = owner.save(current)!;
    const failed = await first.settlement;
    expect(failed.kind).toBe("failed");
    if (failed.kind === "failed") {
      expect(failed.status).toBeUndefined();
      expect(failed.transportLost).toBe(true);
    }
    expect((await next.settlement).kind).toBe("confirmed");
    expect(attempts).toBe(2);
    owner.dispose();

    for (const settlement of [
      () => Promise.resolve(new Response(null, { status: 200 })),
      () => Promise.resolve(new Response(null, { status: 503 })),
      () => Promise.reject(new Error("offline")),
    ]) {
      const initial = target("initial");
      const replacement = target("replacement");
      const state = currentPort(initial);
      const held = deferred<Response>();
      const started = deferred<void>();
      const next = createSavedPositionOwner(async () => {
        started.resolve();
        await held.promise;
        return settlement();
      }, state.port);
      const admission = next.save(initial)!;
      await started.promise;
      state.setCurrent(replacement);
      held.resolve(new Response(null, { status: 200 }));
      expect((await admission.settlement).kind).toBe("detached");
      next.dispose();
    }
  });

  test("dispose rejects new writes, detaches in-flight work, and skips queued work", async () => {
    const captured = target("dispose");
    const authorities = currentPort(captured);
    const held = deferred<Response>();
    const started = deferred<void>();
    let requests = 0;
    const owner = createSavedPositionOwner(async () => {
      requests += 1;
      started.resolve();
      return held.promise;
    }, authorities.port);
    const inFlight = owner.save(captured)!;
    await started.promise;
    const queued = owner.save(captured)!;
    owner.dispose();
    owner.dispose();
    expect(owner.save(captured)).toBeUndefined();
    held.resolve(new Response(null, { status: 200 }));
    expect((await inFlight.settlement).kind).toBe("detached");
    expect((await queued.settlement).kind).toBe("skipped");
    expect(requests).toBe(1);
  });
});
