import { describe, expect, test } from "bun:test";
import type { PhotoSummary } from "../api/contracts.js";
import {
  createPhotoOwner,
  type PhotoFetch,
  type PhotoSourcePort,
  type ReviewImageTransferPort,
} from "./photo-owner.js";
import type {
  PhotoWindowAuthority,
  SourceAuthority,
} from "./source-grid-owner.js";

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

const fact = (id: string): PhotoSummary => ({
  id,
  available: true,
  ambiguous: false,
  originals: [{ kind: "jpeg", available: true }],
  selectionState: "undecided",
  rating: 0,
  preview: { state: "inspection-pending" },
});

const sourceAuthority = () => Object.freeze({}) as SourceAuthority;

class FakeSource implements PhotoSourcePort {
  authority = sourceAuthority();
  facts = new Map<number, PhotoSummary>();
  moved: number[] = [];
  trimmed: number[] = [];
  windows: PhotoWindowAuthority[] = [];

  isSourceCurrent(authority: SourceAuthority): boolean {
    return authority === this.authority;
  }
  renewPhotoWindow(
    authority: SourceAuthority,
  ): PhotoWindowAuthority | undefined {
    if (!this.isSourceCurrent(authority)) return undefined;
    const value = Object.freeze({}) as PhotoWindowAuthority;
    this.windows.push(value);
    return value;
  }
  photoAt(authority: SourceAuthority, index: number): PhotoSummary | undefined {
    return this.isSourceCurrent(authority) ? this.facts.get(index) : undefined;
  }
  movePosition(authority: SourceAuthority, index: number): boolean {
    if (!this.isSourceCurrent(authority)) return false;
    this.moved.push(index);
    return true;
  }
  patchPreview(
    authority: SourceAuthority,
    index: number,
    photoId: string,
    preview: PhotoSummary["preview"],
  ): boolean {
    return this.patch(authority, index, photoId, { preview });
  }
  patchSelection(
    authority: SourceAuthority,
    index: number,
    photoId: string,
    selectionState: PhotoSummary["selectionState"],
  ): boolean {
    return this.patch(authority, index, photoId, { selectionState });
  }
  patchRating(
    authority: SourceAuthority,
    index: number,
    photoId: string,
    rating: number,
  ): boolean {
    return this.patch(authority, index, photoId, { rating });
  }
  trimFacts(authority: SourceAuthority, anchor: number): void {
    if (this.isSourceCurrent(authority)) this.trimmed.push(anchor);
  }
  patch(
    authority: SourceAuthority,
    index: number,
    photoId: string,
    value: Partial<PhotoSummary>,
  ): boolean {
    const current = this.photoAt(authority, index);
    if (!current || current.id !== photoId) return false;
    this.facts.set(index, Object.freeze({ ...current, ...value }));
    return true;
  }
}

const bind = (source: FakeSource, fetcher: PhotoFetch) => {
  const owner = createPhotoOwner(fetcher, source);
  owner.bindSource({
    sourceAuthority: source.authority,
    total: source.facts.size,
    index: 0,
    albumId: "album-1",
  });
  const opened = owner.beginOpen(0);
  if (!opened) throw new Error("expected Photo open");
  owner.finishOpen(opened.authority);
  return { owner, opened };
};

const mutationBody = (undo: unknown): Response =>
  new Response(JSON.stringify({ undo }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });

describe("PhotoOwner", () => {
  test("owns current and adjacent Preview priority and suppresses stale completion", async () => {
    const source = new FakeSource();
    source.facts.set(0, fact("photo-0"));
    source.facts.set(1, fact("photo-1"));
    const held = deferred<Response>();
    const requests: Array<Readonly<{ path: string; priority?: string }>> = [];
    const fetcher: PhotoFetch = (path, init) => {
      requests.push({
        path,
        ...(init?.priority ? { priority: init.priority } : {}),
      });
      return path.includes("priority=adjacent")
        ? Promise.resolve(new Response(null, { status: 202 }))
        : held.promise;
    };
    const { owner, opened } = bind(source, fetcher);
    const preview = owner.loadCurrentPreview(opened.authority);
    await owner.prefetchAdjacent(opened.authority, 1);
    expect(requests).toEqual([
      { path: "/api/photos/photo-0/preview", priority: "high" },
      {
        path: "/api/photos/photo-1/preview?priority=adjacent",
        priority: "low",
      },
    ]);

    owner.leave();
    held.resolve(
      Response.json({
        state: "ready",
        url: "/review.jpg",
        source: "matching-jpeg",
      }),
    );
    expect((await preview).kind).toBe("detached");
    expect(source.facts.get(0)?.preview.url).toBeUndefined();
    owner.dispose();
  });

  test("patches ready Preview facts and owns the browser image lease", async () => {
    const source = new FakeSource();
    source.facts.set(0, fact("photo-0"));
    const events: string[] = [];
    const owner = createPhotoOwner(
      () =>
        Promise.resolve(
          Response.json({
            state: "ready",
            url: "/review.jpg",
            source: "matching-jpeg",
          }),
        ),
      source,
      { emit: (event) => events.push(event.kind) },
    );
    owner.bindSource({ sourceAuthority: source.authority, total: 1, index: 0 });
    const opened = owner.beginOpen(0)!;
    owner.finishOpen(opened.authority);
    const outcome = await owner.loadCurrentPreview(opened.authority);
    expect(outcome.kind).toBe("ready");
    expect(source.facts.get(0)?.preview.url).toBe("/review.jpg");

    let removed = 0;
    let onLoad: (() => void) | undefined;
    let onError: (() => void) | undefined;
    let imageSource = "";
    const image: ReviewImageTransferPort = {
      connected: true,
      get source() {
        return imageSource;
      },
      setHandlers(nextLoad, nextError) {
        onLoad = nextLoad;
        onError = nextError;
      },
      clearHandlers() {
        onLoad = undefined;
        onError = undefined;
      },
      setSource(next) {
        imageSource = next;
      },
      clearSource() {
        removed += 1;
        imageSource = "";
      },
    };
    expect(
      owner.attachReviewImage(opened.authority, image, "/review.jpg", {}),
    ).toBe(true);
    expect(onLoad).toBeDefined();
    onError?.();
    expect(events).toEqual(["review-image-failed"]);
    expect(removed).toBe(1);
    expect(
      owner.attachReviewImage(opened.authority, image, "/review.jpg", {}),
    ).toBe(true);
    onLoad?.();
    expect(imageSource).toBe("/review.jpg");
    expect(onLoad).toBeUndefined();
    owner.dispose();
    expect(removed).toBe(2);
    expect(imageSource).toBe("");
  });

  test("always settles Selection and Rating writes with exact failure policy", async () => {
    for (const expected of [
      { response: new Response(null, { status: 409 }), connectivity: "lost" },
      {
        response: new Response(null, { status: 503 }),
        connectivity: "unchanged",
      },
    ] as const) {
      const source = new FakeSource();
      source.facts.set(0, fact("photo-0"));
      const { owner } = bind(source, () => Promise.resolve(expected.response));
      const admission = owner.mutate("rating", 4, false)!;
      expect(owner.busy).toBe(true);
      const outcome = await admission.settlement;
      expect(outcome.kind).toBe("failed");
      if (outcome.kind === "failed")
        expect(outcome.connectivity).toBe(expected.connectivity);
      expect(owner.busy).toBe(false);
      owner.dispose();
    }

    const source = new FakeSource();
    source.facts.set(0, fact("photo-0"));
    const { owner } = bind(source, () => Promise.reject(new Error("offline")));
    const outcome = await owner.mutate("selectionState", "selected", true)!
      .settlement;
    expect(outcome.kind).toBe("failed");
    if (outcome.kind === "failed") expect(outcome.connectivity).toBe("lost");
    expect(owner.canUndo).toBe(false);
    owner.dispose();
  });

  test("applies a persisted mutation, records one Undo, and advances only by outcome", async () => {
    const source = new FakeSource();
    source.facts.set(0, fact("photo-0"));
    source.facts.set(1, fact("photo-1"));
    const requests: string[] = [];
    const { owner } = bind(source, (path, init) => {
      requests.push(
        `${path}:${typeof init?.body === "string" ? init.body : ""}`,
      );
      return Promise.resolve(
        mutationBody({
          photoId: "photo-0",
          field: "selectionState",
          priorValue: "undecided",
          expectedCurrent: "selected",
        }),
      );
    });
    const outcome = await owner.mutate("selectionState", "selected", true)!
      .settlement;
    expect(outcome.kind).toBe("persisted");
    expect(outcome.advance).toBe(true);
    expect(owner.currentIndex).toBe(0);
    expect(owner.canUndo).toBe(true);
    expect(source.facts.get(0)?.selectionState).toBe("selected");
    expect(requests[0]).toContain('"albumId":"album-1"');
    owner.dispose();
  });

  test("keeps Retry authority current while its Photo fact is reloaded", () => {
    const source = new FakeSource();
    source.facts.set(0, fact("photo-0"));
    const { owner } = bind(source, () =>
      Promise.resolve(new Response(null, { status: 202 })),
    );
    source.facts.delete(0);

    const retry = owner.beginRetry()!;
    expect(owner.isCurrent(retry.authority)).toBe(true);
    expect(owner.retryIsCurrent(retry)).toBe(true);
    expect(owner.retryPhotoIsCurrent(retry)).toBe(false);

    source.facts.set(0, fact("photo-0"));
    expect(owner.retryPhotoIsCurrent(retry)).toBe(true);
    source.facts.set(0, fact("replacement"));
    expect(owner.retryPhotoIsCurrent(retry)).toBe(false);
    owner.finishRetry(retry);
    owner.dispose();
  });

  test("reloads an evicted Undo through an opaque window then returns to its Photo", async () => {
    const source = new FakeSource();
    source.facts.set(0, fact("photo-0"));
    source.facts.set(1, fact("photo-1"));
    let call = 0;
    const { owner } = bind(source, () => {
      call += 1;
      return Promise.resolve(
        call === 1
          ? mutationBody({
              photoId: "photo-0",
              field: "selectionState",
              priorValue: "undecided",
              expectedCurrent: "selected",
            })
          : new Response(null, { status: 204 }),
      );
    });
    await owner.mutate("selectionState", "selected", true)!.settlement;
    const second = owner.beginOpen(1)!;
    owner.finishOpen(second.authority);
    owner.leave();
    source.facts.delete(0);

    const preparation = owner.prepareUndo()!;
    expect(preparation.needsWindow).toBe(true);
    expect(owner.windowAuthority).toBeDefined();
    expect(preparation.windowAuthority).toBe(owner.windowAuthority!);
    source.facts.set(0, {
      ...fact("photo-0"),
      selectionState: "selected",
    });
    const outcome = await owner.performUndo(preparation);
    expect(outcome.kind).toBe("persisted");
    expect(owner.currentIndex).toBe(0);
    expect(owner.canUndo).toBe(false);
    expect(source.facts.get(0)?.selectionState).toBe("undecided");
    expect(source.trimmed).toEqual([0]);
    owner.dispose();
  });

  test("keeps answered non-conflict Undo retryable and retires conflict or transport", async () => {
    for (const failure of [409, 503, "transport"] as const) {
      const source = new FakeSource();
      source.facts.set(0, fact("photo-0"));
      let call = 0;
      const { owner } = bind(source, () => {
        call += 1;
        if (call === 1)
          return Promise.resolve(
            mutationBody({
              photoId: "photo-0",
              field: "rating",
              priorValue: 0,
              expectedCurrent: 5,
            }),
          );
        return failure === "transport"
          ? Promise.reject(new Error("offline"))
          : Promise.resolve(new Response(null, { status: failure }));
      });
      await owner.mutate("rating", 5, false)!.settlement;
      const preparation = owner.prepareUndo()!;
      const outcome = await owner.performUndo(preparation);
      expect(outcome.kind).toBe("failed");
      if (outcome.kind === "failed") {
        expect(outcome.connectivity).toBe(
          failure === 409 || failure === "transport" ? "lost" : "unchanged",
        );
        expect(outcome.retryable).toBe(failure === 503);
      }
      expect(owner.canUndo).toBe(failure === 503);
      owner.dispose();
    }
  });

  test("dispose is idempotent, aborts reads, and lets admitted writes settle detached", async () => {
    const source = new FakeSource();
    source.facts.set(0, fact("photo-0"));
    const held = deferred<Response>();
    const { owner, opened } = bind(source, () => held.promise);
    const preview = owner.loadCurrentPreview(opened.authority);
    owner.dispose();
    owner.dispose();
    held.resolve(Response.json({ state: "ready", url: "/late.jpg" }));
    expect((await preview).kind).toBe("detached");
    expect(owner.mutate("rating", 1, false)).toBeUndefined();

    const writeHeld = deferred<Response>();
    const nextSource = new FakeSource();
    nextSource.facts.set(0, fact("photo-1"));
    const next = bind(nextSource, () => writeHeld.promise).owner;
    const write = next.mutate("rating", 2, false)!;
    next.dispose();
    writeHeld.resolve(
      mutationBody({
        photoId: "photo-1",
        field: "rating",
        priorValue: 0,
        expectedCurrent: 2,
      }),
    );
    expect((await write.settlement).kind).toBe("detached");
  });
});
