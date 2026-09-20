import { describe, expect, test } from "bun:test";
import {
  createSourceGridOwner,
  type GridThumbnailImage,
  type SourceGridOwner,
  type SourceWindowOutcome,
} from "./source-grid-owner.js";

const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
};

const opened = (
  token: string,
  total = 180,
  position = 0,
  selectionCounts: Readonly<{
    selected: number;
    rejected: number;
    undecided: number;
  }> = { selected: 0, rejected: 0, undecided: total },
) =>
  new Response(JSON.stringify({ token, total, position, selectionCounts }), {
    status: 200,
  });

const photo = (id: string) => ({
  id,
  available: true,
  original: { kind: "jpeg" as const, available: true },
  selectionState: "undecided" as const,
  rating: 0,
  preview: { state: "inspection-pending" as const },
});

const windowResponse = (
  start: number,
  total = 180,
  count = 60,
  prefix = "photo",
) =>
  new Response(
    JSON.stringify({
      start,
      total,
      photos: Array.from(
        { length: Math.min(count, Math.max(0, total - start)) },
        (_, offset) => photo(`${prefix}-${start + offset}`),
      ),
    }),
    { status: 200 },
  );

const requestUrl = (input: RequestInfo | URL) =>
  new URL(
    typeof input === "string"
      ? input
      : input instanceof URL
        ? input.href
        : input.url,
    "http://slipstream.test",
  );

const openLibrary = async (owner: SourceGridOwner, token = "browse-1") => {
  const result = await owner.open({ kind: "library" });
  expect(result.kind).toBe("opened");
  expect(owner.token).toBe(token);
  return owner.authority;
};

const flushTasks = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

class FakeImage implements GridThumbnailImage {
  complete = false;
  deliveryFailed = false;
  isConnected = true;
  src = "";
  onload: GlobalEventHandlers["onload"] = null;
  onerror: GlobalEventHandlers["onerror"] = null;
  removeCalls = 0;

  removeAttribute(name: string): void {
    if (name === "src") {
      this.removeCalls += 1;
      this.src = "";
    }
  }

  setDeliveryFailed(failed: boolean): void {
    this.deliveryFailed = failed;
  }
}

describe("SourceGridOwner", () => {
  test("releases stale and disposed Browse tokens exactly once", async () => {
    const pending = [deferred<Response>(), deferred<Response>()];
    const releases: string[] = [];
    let opens = 0;
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (init?.method === "DELETE") {
        releases.push(url.pathname.split("/").at(-1)!);
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return pending[opens++]!.promise;
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const stale = owner.open({ kind: "library" });
    const current = owner.open({
      kind: "album",
      album: { id: "album-1", name: "Keepers" },
    });
    pending[1]!.resolve(opened("current-token"));
    expect((await current).kind).toBe("opened");
    pending[0]!.resolve(opened("stale-token"));
    expect((await stale).kind).toBe("detached");

    owner.dispose();
    owner.dispose();
    await Promise.resolve();
    expect(releases.sort()).toEqual(["current-token", "stale-token"]);
  });

  test("releases the original and every stale token across three overlapping replacements", async () => {
    const replacements = [
      deferred<Response>(),
      deferred<Response>(),
      deferred<Response>(),
    ];
    const releases: string[] = [];
    let replacementIndex = 0;
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (init?.method === "DELETE") {
        releases.push(url.pathname.split("/").at(-1)!);
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      if (url.pathname === "/api/browse" && init?.method === "POST") {
        if (replacementIndex === 0) {
          replacementIndex += 1;
          return Promise.resolve(opened("original-token"));
        }
        return replacements[replacementIndex++ - 1]!.promise;
      }
      throw new Error(`unexpected request ${url.pathname}`);
    });

    await openLibrary(owner, "original-token");
    const first = owner.open({ kind: "library" });
    const second = owner.open({ kind: "library" });
    const current = owner.open({ kind: "library" });
    expect(releases).toEqual(["original-token"]);

    replacements[0]!.resolve(opened("stale-token-1"));
    replacements[1]!.resolve(opened("stale-token-2"));
    replacements[2]!.resolve(opened("current-token"));
    expect((await first).kind).toBe("detached");
    expect((await second).kind).toBe("detached");
    expect((await current).kind).toBe("opened");
    expect(releases.sort()).toEqual([
      "original-token",
      "stale-token-1",
      "stale-token-2",
    ]);

    owner.dispose();
    expect(releases.sort()).toEqual([
      "current-token",
      "original-token",
      "stale-token-1",
      "stale-token-2",
    ]);
  });

  test("sends only the requested order and reports it as the open view order", async () => {
    const bodies: Array<Record<string, unknown>> = [];
    const releases: string[] = [];
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (init?.method === "DELETE") {
        releases.push(url.pathname.split("/").at(-1)!);
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      if (url.pathname === "/api/browse" && init?.method === "POST") {
        if (typeof init.body !== "string") throw new Error("expected a body");
        bodies.push(JSON.parse(init.body) as Record<string, unknown>);
        return Promise.resolve(opened(`browse-${bodies.length}`));
      }
      throw new Error(`unexpected request ${url.pathname}`);
    });

    expect(owner.order).toBe("source-default");
    await owner.open({ kind: "library" });
    expect(bodies[0]).toEqual({ source: "library" });
    expect(owner.order).toBe("source-default");

    const album = {
      kind: "album" as const,
      album: { id: "album-1", name: "Keepers" },
    };
    await owner.open(album, { order: "capture-time-asc" });
    expect(bodies[1]).toEqual({
      source: "album",
      albumId: "album-1",
      order: "capture-time-asc",
    });
    expect(owner.order).toBe("capture-time-asc");

    await owner.open(album, {
      mode: "reopen",
      order: "capture-time-desc",
      preferredPhotoId: "photo-7",
    });
    expect(bodies[2]).toEqual({
      source: "album",
      albumId: "album-1",
      order: "capture-time-desc",
      photoId: "photo-7",
    });
    expect(owner.order).toBe("capture-time-desc");
    expect(releases).toEqual(["browse-1", "browse-2"]);
  });

  test("sends only a non-default Selection filter and replaces the open counts", async () => {
    const bodies: Array<Record<string, unknown>> = [];
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      if (url.pathname === "/api/browse" && init?.method === "POST") {
        if (typeof init.body !== "string") throw new Error("expected a body");
        bodies.push(JSON.parse(init.body) as Record<string, unknown>);
        return Promise.resolve(
          opened(`browse-${bodies.length}`, 180, 0, {
            selected: 5,
            rejected: 2,
            undecided: 173,
          }),
        );
      }
      throw new Error(`unexpected request ${url.pathname}`);
    });
    expect(owner.selection).toBe("all");
    expect(owner.selectionCounts).toEqual({
      selected: 0,
      rejected: 0,
      undecided: 0,
    });
    await owner.open({ kind: "library" });
    // The default filter is the server's own default, so it stays off the wire.
    expect(bodies[0]).toEqual({ source: "library" });
    expect(owner.selection).toBe("all");
    expect(owner.selectionCounts).toEqual({
      selected: 5,
      rejected: 2,
      undecided: 173,
    });

    await owner.open(
      { kind: "library" },
      { selection: "rejected", preferredPhotoId: "photo-3" },
    );
    expect(bodies[1]).toEqual({
      source: "library",
      selection: "rejected",
      photoId: "photo-3",
    });
    expect(owner.selection).toBe("rejected");

    // A replace open reports no counts until its own response arrives.
    const replacement = owner.open({ kind: "library" });
    expect(owner.selectionCounts).toEqual({
      selected: 0,
      rejected: 0,
      undecided: 0,
    });
    await replacement;
    expect(owner.selection).toBe("all");
    expect(owner.selectionCounts).toEqual({
      selected: 5,
      rejected: 2,
      undecided: 173,
    });
  });

  test("moves the server counts only for one confirmed Selection transition", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(
          opened("browse-1", 10, 0, { selected: 3, rejected: 1, undecided: 6 }),
        );
      if (url.pathname === "/api/browse/browse-1")
        return Promise.resolve(windowResponse(0, 10, 10));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    await openLibrary(owner, "browse-1");
    const authority = owner.authority;
    const loaded = await owner.loadWindow(0, { kind: "source", authority });
    expect(loaded.kind).toBe("loaded");
    const [first] = [owner.photoAt(0)!];
    expect(first?.selectionState).toBe("undecided");

    // One confirmed transition carries the source counts with it: selected
    // becomes rejected, and neither count is re-derived from loaded windows.
    expect(owner.setPhotoSelection(authority, 0, first.id, "selected")).toBe(
      true,
    );
    expect(owner.selectionCounts).toEqual({
      selected: 4,
      rejected: 1,
      undecided: 5,
    });
    expect(owner.photoAt(0)?.selectionState).toBe("selected");
    expect(owner.setPhotoSelection(authority, 0, first.id, "rejected")).toBe(
      true,
    );
    expect(owner.selectionCounts).toEqual({
      selected: 3,
      rejected: 2,
      undecided: 5,
    });
    expect(owner.setPhotoSelection(authority, 0, first.id, "rejected")).toBe(
      true,
    );
    expect(owner.selectionCounts).toEqual({
      selected: 3,
      rejected: 2,
      undecided: 5,
    });

    // A patch that does not address the retained Photo changes nothing, and a
    // patch outside the open source authority is refused.
    expect(
      owner.setPhotoSelection(authority, 0, "other-photo", "selected"),
    ).toBe(false);
    expect(owner.selectionCounts).toEqual({
      selected: 3,
      rejected: 2,
      undecided: 5,
    });
    const stale = owner.authority;
    await owner.open({ kind: "library" });
    expect(owner.setPhotoSelection(stale, 0, first.id, "selected")).toBe(false);
    expect(owner.selectionCounts).toEqual({
      selected: 3,
      rejected: 1,
      undecided: 6,
    });
  });

  test("moves the counts for one confirmed batch outcome by Photo identity", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(
          opened("browse-1", 10, 0, { selected: 3, rejected: 1, undecided: 6 }),
        );
      if (url.pathname === "/api/browse/browse-1")
        return Promise.resolve(windowResponse(0, 10, 10));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    await openLibrary(owner, "browse-1");
    const authority = owner.authority;
    const loaded = await owner.loadWindow(0, { kind: "source", authority });
    expect(loaded.kind).toBe("loaded");
    const first = owner.photoAt(0)!;
    const second = owner.photoAt(1)!;

    // A loaded Photo is patched and moves the counts by the state the Grid
    // believed, whatever prior the server reported for it.
    expect(
      owner.applyBatchSelection(authority, first.id, "rejected", "selected"),
    ).toBe(true);
    expect(owner.photoAt(0)?.selectionState).toBe("selected");
    expect(owner.selectionCounts).toEqual({
      selected: 4,
      rejected: 1,
      undecided: 5,
    });
    // A Photo the Grid no longer holds still moves the counts once, by the
    // server's own prior value.
    expect(
      owner.applyBatchSelection(
        authority,
        "evicted-photo",
        "undecided",
        "selected",
      ),
    ).toBe(true);
    expect(owner.selectionCounts).toEqual({
      selected: 5,
      rejected: 1,
      undecided: 4,
    });
    // A confirmed outcome moves a loaded fact and its counts once; applying
    // the same outcome again changes nothing.
    expect(
      owner.applyBatchSelection(authority, second.id, "undecided", "selected"),
    ).toBe(true);
    expect(owner.photoAt(1)?.selectionState).toBe("selected");
    expect(owner.selectionCounts).toEqual({
      selected: 6,
      rejected: 1,
      undecided: 3,
    });
    expect(
      owner.applyBatchSelection(authority, second.id, "undecided", "selected"),
    ).toBe(true);
    expect(owner.selectionCounts).toEqual({
      selected: 6,
      rejected: 1,
      undecided: 3,
    });
    // A batch outcome outside the open source authority is refused.
    const stale = owner.authority;
    await owner.open({ kind: "library" });
    expect(
      owner.applyBatchSelection(stale, first.id, "undecided", "selected"),
    ).toBe(false);
    expect(owner.selectionCounts).toEqual({
      selected: 3,
      rejected: 1,
      undecided: 6,
    });
  });

  test("reconciles a refreshed Photo fact against the source counts", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(
          opened("browse-1", 10, 0, { selected: 3, rejected: 1, undecided: 6 }),
        );
      if (url.pathname === "/api/browse/browse-1") {
        const photos = Array.from({ length: 10 }, (_, index) =>
          index === 0
            ? { ...photo("photo-0"), selectionState: "rejected" as const }
            : photo(`photo-${index}`),
        );
        return Promise.resolve(
          new Response(JSON.stringify({ start: 0, total: 10, photos }), {
            status: 200,
          }),
        );
      }
      throw new Error(`unexpected request ${url.pathname}`);
    });
    await openLibrary(owner, "browse-1");
    const authority = owner.authority;
    expect(
      (await owner.loadWindow(0, { kind: "source", authority })).kind,
    ).toBe("loaded");
    const first = owner.photoAt(0)!;
    expect(first.selectionState).toBe("rejected");
    // The open counts still contain the browser's prior undecided belief. A
    // Review refresh reconciles that one contribution to the observed fact.
    expect(
      owner.reconcilePhotoSelection(
        authority,
        0,
        first.id,
        "undecided",
        "rejected",
      ),
    ).toBe(true);
    expect(owner.selectionCounts).toEqual({
      selected: 3,
      rejected: 2,
      undecided: 5,
    });
    const stale = owner.authority;
    await owner.open({ kind: "library" });
    expect(
      owner.reconcilePhotoSelection(
        stale,
        0,
        first.id,
        "undecided",
        "rejected",
      ),
    ).toBe(false);
  });

  test("keeps reconciled progress counts non-negative after an external write", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(
          opened("browse-1", 1, 0, { selected: 0, rejected: 1, undecided: 0 }),
        );
      if (url.pathname === "/api/browse/browse-1")
        return Promise.resolve(
          new Response(
            JSON.stringify({
              start: 0,
              total: 1,
              photos: [{ ...photo("photo-0"), selectionState: "rejected" }],
            }),
            { status: 200 },
          ),
        );
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const authority = await openLibrary(owner, "browse-1");
    await owner.loadWindow(0, { kind: "source", authority });
    expect(
      owner.reconcilePhotoSelection(
        authority,
        0,
        "photo-0",
        "selected",
        "rejected",
      ),
    ).toBe(true);
    expect(owner.selectionCounts).toEqual({
      selected: 0,
      rejected: 2,
      undecided: 0,
    });
  });

  test("keeps the attempted source and retry state after an open failure", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(new Response(null, { status: 503 }));
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const outcome = await owner.open({
      kind: "folder",
      folder: { location: "shoot", name: "Shoot" },
      publication: "published-1",
    });
    expect(outcome).toMatchObject({
      kind: "failed",
      transportLost: true,
    });
    expect(owner.source).toEqual({
      kind: "folder",
      folder: { location: "shoot", name: "Shoot" },
      publication: "published-1",
    });
    expect(owner.lastSource).toEqual(owner.source);
    expect(owner.retryRequired).toBe(true);
    expect(owner.token).toBe("");
  });

  test("retains at most three bounded Browse windows", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 300));
      if (url.pathname === "/api/browse/browse-1") {
        const start = Number(url.searchParams.get("start"));
        return Promise.resolve(windowResponse(start, 300));
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const authority = await openLibrary(owner);
    for (const index of [0, 60, 120, 180, 240]) {
      const outcome = await owner.loadWindow(index, {
        kind: "source",
        authority,
      });
      expect(outcome.kind).toBe("loaded");
      expect(owner.retainedFactCount).toBeLessThanOrEqual(180);
    }
    expect(owner.photoAt(0)).toBeUndefined();
    expect(owner.photoAt(240)?.id).toBe("photo-240");
  });

  test("retains Photo, Original kind, and Preview facts in a bounded window", async () => {
    const photos = [
      {
        ...photo("unavailable-photo"),
        available: false,
        preview: { state: "unavailable" as const },
      },
      {
        ...photo("raw-photo"),
        original: { kind: "raw" as const, available: true },
      },
      {
        ...photo("failed-preview"),
        preview: { state: "failed" as const },
      },
    ];
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", photos.length));
      if (url.pathname === "/api/browse/browse-1")
        return Promise.resolve(
          new Response(
            JSON.stringify({ start: 0, total: photos.length, photos }),
            { status: 200 },
          ),
        );
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const authority = await openLibrary(owner);
    expect(
      await owner.loadWindow(0, { kind: "source", authority }),
    ).toMatchObject({ kind: "loaded" });
    expect(owner.retainedFactCount).toBe(photos.length);
    expect(owner.photoAt(0)).toMatchObject({
      available: false,
      original: { kind: "jpeg", available: true },
      preview: { state: "unavailable" },
    });
    expect(owner.photoAt(1)).toMatchObject({
      available: true,
      original: { kind: "raw", available: true },
      preview: { state: "inspection-pending" },
    });
    expect(owner.photoAt(2)).toMatchObject({
      available: true,
      original: { kind: "jpeg", available: true },
      preview: { state: "failed" },
    });
  });

  test("resolves retained and remote Photo positions without loading the source", async () => {
    const positions: string[] = [];
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 180));
      if (url.pathname === "/api/browse/browse-1")
        return Promise.resolve(windowResponse(0, 180));
      if (url.pathname === "/api/browse/browse-1/position") {
        const photoId = url.searchParams.get("photoId");
        if (!photoId) throw new Error("missing Photo ID");
        positions.push(photoId);
        return Promise.resolve(
          Response.json({
            position: photoId === "remote-photo" ? 123 : null,
          }),
        );
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const authority = await openLibrary(owner);
    await owner.loadWindow(0, { kind: "source", authority });
    expect(
      await owner.resolvePhotoPosition(authority, "photo-5"),
    ).toMatchObject({ kind: "resolved", position: 5 });
    expect(
      await owner.resolvePhotoPosition(authority, "remote-photo"),
    ).toMatchObject({ kind: "resolved", position: 123 });
    expect(
      await owner.resolvePhotoPosition(authority, "removed-photo"),
    ).toMatchObject({ kind: "missing" });
    expect(positions).toEqual(["remote-photo", "removed-photo"]);
    owner.dispose();
  });

  test("does not commit a stale window into a replacement source", async () => {
    const oldWindow = deferred<Response>();
    let openCount = 0;
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened(`browse-${++openCount}`, 60));
      if (url.pathname === "/api/browse/browse-1") return oldWindow.promise;
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const oldAuthority = await openLibrary(owner);
    const stale = owner.loadWindow(0, {
      kind: "grid",
      authority: oldAuthority,
    });
    const replacement = await owner.open({ kind: "library" });
    expect(replacement.kind).toBe("opened");
    oldWindow.resolve(windowResponse(0, 60));
    expect((await stale).kind).toBe("detached");
    expect(owner.retainedFactCount).toBe(0);
  });

  test("promotes an aborted Grid window to explicit Photo-owned high priority", async () => {
    const gridWindow = deferred<Response>();
    const priorities: Array<string | undefined> = [];
    const signals: AbortSignal[] = [];
    let windowRequests = 0;
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 60));
      if (url.pathname === "/api/browse/browse-1") {
        windowRequests += 1;
        priorities.push(init?.priority);
        signals.push(init?.signal as AbortSignal);
        return windowRequests === 1
          ? gridWindow.promise
          : Promise.resolve(windowResponse(0, 60));
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const authority = await openLibrary(owner);
    const speculative = owner.loadWindow(
      0,
      { kind: "grid", authority },
      { quiet: true, priority: "low" },
    );
    await Promise.resolve();
    owner.stopGridWork();
    const photoAuthority = owner.renewPhotoWindow();
    const foreground = owner.loadWindow(
      0,
      { kind: "photo", authority: photoAuthority },
      { quiet: true, priority: "high" },
    );

    expect((await speculative).kind).toBe("detached");
    expect((await foreground).kind).toBe("loaded");
    expect(windowRequests).toBe(2);
    expect(priorities).toEqual(["low", "high"]);
    expect(signals[0]!.aborted).toBe(true);
    expect(signals[1]!.aborted).toBe(false);
    gridWindow.resolve(windowResponse(0, 60));
  });

  test("returns the cancelled opaque Photo owner without mapping it to the replacement lifetime", async () => {
    const staleWindow = deferred<Response>();
    let windowRequests = 0;
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 60));
      if (url.pathname === "/api/browse/browse-1") {
        windowRequests += 1;
        return windowRequests === 1
          ? staleWindow.promise
          : Promise.resolve(windowResponse(0, 60, 60, "current"));
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    await openLibrary(owner);

    const staleAuthority = owner.renewPhotoWindow();
    expect(Object.isFrozen(staleAuthority)).toBe(true);
    const stale = owner.loadWindow(0, {
      kind: "photo",
      authority: staleAuthority,
    });
    await Promise.resolve();
    const currentAuthority = owner.renewPhotoWindow();
    const staleOutcome = await stale;
    expect(staleOutcome).toMatchObject({
      kind: "detached",
      owner: { scope: "photo", authority: staleAuthority },
    });
    if (staleOutcome.owner.scope !== "photo")
      throw new Error("expected opaque Photo owner");
    expect(staleOutcome.owner.authority).not.toBe(currentAuthority);

    expect(
      await owner.loadWindow(0, {
        kind: "photo",
        authority: currentAuthority,
      }),
    ).toMatchObject({
      kind: "loaded",
      owner: { scope: "photo", authority: currentAuthority },
    });
    expect(owner.photoAt(0)?.id).toBe("current-0");
    staleWindow.resolve(windowResponse(0, 60, 60, "stale"));
    await Promise.resolve();
    expect(owner.photoAt(0)?.id).toBe("current-0");
  });

  test("classifies expired, answered, and transport window failures for the explicit owner", async () => {
    const responses: Array<Response | Error> = [
      new Response(null, { status: 404 }),
      new Response(null, { status: 503 }),
      new Error("offline"),
    ];
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 180));
      if (url.pathname === "/api/browse/browse-1") {
        const next = responses.shift()!;
        return next instanceof Error
          ? Promise.reject(next)
          : Promise.resolve(next);
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    await openLibrary(owner);
    const photoAuthority = owner.renewPhotoWindow();
    const operation = {
      kind: "photo" as const,
      authority: photoAuthority,
    };

    expect(await owner.loadWindow(0, operation)).toMatchObject({
      kind: "expired",
      owner: { scope: "photo", authority: photoAuthority },
    });
    expect(await owner.loadWindow(60, operation)).toMatchObject({
      kind: "failed",
      owner: { scope: "photo", authority: photoAuthority },
      transportLost: false,
      status: 503,
    });
    expect(await owner.loadWindow(120, operation)).toMatchObject({
      kind: "failed",
      owner: { scope: "photo", authority: photoAuthority },
      transportLost: true,
    });
  });

  test("rejects malformed opens and short non-tail windows", async () => {
    let mode: "malformed-open" | "short-window" = "malformed-open";
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(
          mode === "malformed-open"
            ? new Response(
                '{"token":"","total":60,"position":0,"selectionCounts":{"selected":0,"rejected":0,"undecided":60}}',
                {
                  status: 200,
                },
              )
            : opened("browse-1", 120),
        );
      if (url.pathname === "/api/browse/browse-1")
        return Promise.resolve(windowResponse(0, 120, 59));
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });

    expect(await owner.open({ kind: "library" })).toMatchObject({
      kind: "failed",
      transportLost: true,
    });
    expect(owner.token).toBe("");
    mode = "short-window";
    const openedSource = await owner.open({ kind: "library" });
    expect(openedSource.kind).toBe("opened");
    expect(
      await owner.loadWindow(0, {
        kind: "source",
        authority: owner.authority,
      }),
    ).toMatchObject({
      kind: "failed",
      malformed: true,
      transportLost: false,
    });
    expect(owner.retryRequired).toBe(false);
  });

  test("carries the ordering Original filename and rejects a malformed one", async () => {
    let filename: unknown = "IMG_4521.ARW";
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 1));
      if (url.pathname === "/api/browse/browse-1")
        return Promise.resolve(
          new Response(
            JSON.stringify({
              start: 0,
              total: 1,
              photos: [{ ...photo("photo-0"), originalFilename: filename }],
            }),
            { status: 200 },
          ),
        );
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const authority = await openLibrary(owner);
    const load = (index: number) =>
      owner.loadWindow(index, { kind: "source", authority });

    expect(await load(0)).toMatchObject({ kind: "loaded" });
    expect(owner.photoAt(0)?.originalFilename).toBe("IMG_4521.ARW");

    // The field is optional, but a present value must be a non-empty name.
    for (const malformed of ["", 42, null]) {
      filename = malformed;
      owner.invalidateWindow(0);
      expect(await load(0)).toMatchObject({
        kind: "failed",
        malformed: true,
        transportLost: false,
      });
    }
  });

  test("keeps a replacement source unready until its required window loads", async () => {
    let opens = 0;
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST") {
        opens += 1;
        return Promise.resolve(
          opened(
            [
              "old-token",
              "unavailable-token",
              "malformed-token",
              "ready-token",
            ][opens - 1]!,
            60,
          ),
        );
      }
      if (url.pathname === "/api/browse/old-token")
        return Promise.resolve(windowResponse(0, 60));
      if (url.pathname === "/api/browse/unavailable-token")
        return Promise.resolve(new Response(null, { status: 503 }));
      if (url.pathname === "/api/browse/malformed-token")
        return Promise.resolve(new Response('{"photos":[]}', { status: 200 }));
      if (url.pathname === "/api/browse/ready-token")
        return Promise.resolve(windowResponse(0, 60));
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const initial = await owner.open({ kind: "library" });
    if (initial.kind !== "opened") throw new Error("expected initial source");
    await owner.loadWindow(0, { kind: "source", authority: initial.authority });
    expect(owner.isReady(initial.authority)).toBe(true);

    for (const { expected, ready } of [
      { expected: { status: 503 }, ready: false },
      { expected: { malformed: true }, ready: false },
      { expected: { kind: "loaded" }, ready: true },
    ] as const) {
      const replacement = await owner.open(
        { kind: "library" },
        { mode: "reopen" },
      );
      if (replacement.kind !== "opened")
        throw new Error("expected replacement source");
      expect(owner.isReady(initial.authority)).toBe(false);
      expect(owner.isReady(replacement.authority)).toBe(false);
      expect(
        await owner.loadWindow(0, {
          kind: "source",
          authority: replacement.authority,
        }),
      ).toMatchObject(expected);
      expect(owner.isReady(replacement.authority)).toBe(ready);
    }
  });

  test("owns the Grid position and moves its pending anchor only for the current source", async () => {
    let openCount = 0;
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened(`browse-${++openCount}`, 60, 17));
      if (url.pathname.startsWith("/api/browse/browse-"))
        return Promise.resolve(windowResponse(0, 60));
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const first = await owner.open({ kind: "library" });
    if (first.kind !== "opened") throw new Error("expected first source");
    expect(owner.readGridPosition(first.authority)).toBe(17);
    expect(owner.photoAt(22)).toBeUndefined();
    expect(owner.moveGridPosition(first.authority, 22)).toBe(true);
    expect(owner.readGridPosition(first.authority)).toBe(22);
    await owner.loadWindow(17, {
      kind: "source",
      authority: first.authority,
    });
    expect(owner.moveGridPosition(first.authority, 60)).toBe(false);

    const replacement = await owner.open({ kind: "library" });
    expect(replacement.kind).toBe("opened");
    expect(owner.readGridPosition(replacement.authority)).toBe(17);
    expect(owner.readGridPosition(first.authority)).toBeUndefined();
    expect(owner.moveGridPosition(first.authority, 22)).toBe(false);
    expect(owner.readGridPosition(replacement.authority)).toBe(17);
  });

  test("rejects stale-source and evicted Photo patches instead of injecting facts", async () => {
    let openCount = 0;
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened(`browse-${++openCount}`, 240));
      if (url.pathname === "/api/browse/browse-1") {
        const start = Number(url.searchParams.get("start"));
        return Promise.resolve(windowResponse(start, 240, 60, "old"));
      }
      if (url.pathname === "/api/browse/browse-2") {
        const start = Number(url.searchParams.get("start"));
        return Promise.resolve(windowResponse(start, 240, 60, "current"));
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const first = await owner.open({ kind: "library" });
    if (first.kind !== "opened") throw new Error("expected first source");
    await owner.loadWindow(0, {
      kind: "source",
      authority: first.authority,
    });
    const replacement = await owner.open({ kind: "library" });
    if (replacement.kind !== "opened")
      throw new Error("expected replacement source");
    await owner.loadWindow(0, {
      kind: "source",
      authority: replacement.authority,
    });

    expect(
      owner.setPhotoSelection(first.authority, 0, "old-0", "selected"),
    ).toBe(false);
    expect(
      owner.setPhotoPreview(replacement.authority, 0, "old-0", {
        state: "ready",
        url: "/stale.jpg",
      }),
    ).toBe(false);
    expect(owner.photoAt(0)?.id).toBe("current-0");
    expect(owner.photoAt(0)?.selectionState).toBe("undecided");

    for (const index of [60, 120, 180])
      await owner.loadWindow(index, {
        kind: "source",
        authority: replacement.authority,
      });
    expect(owner.photoAt(0)).toBeUndefined();
    expect(owner.setPhotoRating(replacement.authority, 0, "current-0", 5)).toBe(
      false,
    );
    expect(owner.photoAt(0)).toBeUndefined();
  });

  test("clears an expired token before failed reopen and retries with a new snapshot", async () => {
    let openCount = 0;
    const gets: string[] = [];
    const releases: string[] = [];
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST") {
        openCount += 1;
        if (openCount === 1) return Promise.resolve(opened("old-token", 60));
        if (openCount === 2)
          return Promise.resolve(new Response(null, { status: 503 }));
        return Promise.resolve(opened("new-token", 60));
      }
      if (init?.method === "DELETE") {
        releases.push(url.pathname.split("/").at(-1)!);
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      if (init?.method !== "POST") {
        gets.push(url.pathname.split("/").at(-1)!);
        return Promise.resolve(
          gets.length === 1
            ? new Response(null, { status: 404 })
            : windowResponse(0, 60),
        );
      }
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const first = await owner.open({ kind: "library" });
    if (first.kind !== "opened") throw new Error("expected first source");
    expect(
      await owner.loadWindow(0, {
        kind: "grid",
        authority: first.authority,
      }),
    ).toMatchObject({ kind: "expired" });

    const failedReopen = await owner.open(
      { kind: "library" },
      { mode: "reopen" },
    );
    expect(failedReopen.kind).toBe("failed");
    expect(owner.token).toBe("");
    expect(releases).toEqual(["old-token"]);
    expect(
      await owner.loadWindow(0, {
        kind: "grid",
        authority: owner.authority,
      }),
    ).toMatchObject({ kind: "detached" });
    expect(gets).toEqual(["old-token"]);

    const retried = await owner.open(owner.lastSource!);
    if (retried.kind !== "opened") throw new Error("expected retry source");
    expect(
      await owner.loadWindow(0, {
        kind: "source",
        authority: retried.authority,
      }),
    ).toMatchObject({ kind: "loaded" });
    expect(openCount).toBe(3);
    expect(gets).toEqual(["old-token", "new-token"]);
    expect(releases.filter((token) => token === "old-token")).toHaveLength(1);
  });

  test("coalesces thumbnails and prevents a detached image from poisoning its replacement", async () => {
    const thumbnail = deferred<Response>();
    let thumbnailRequests = 0;
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 1));
      if (url.pathname === "/api/photos/photo-0/thumbnail") {
        thumbnailRequests += 1;
        return thumbnail.promise;
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    await openLibrary(owner);
    const first = new FakeImage();
    const replacement = new FakeImage();

    const firstRequest = owner.loadThumbnail("photo-0", first);
    const staleError = first.onerror;
    const secondRequest = owner.loadThumbnail("photo-0", replacement);
    thumbnail.resolve(
      new Response(JSON.stringify({ state: "ready", url: "/thumb.jpg" }), {
        status: 200,
      }),
    );
    await Promise.all([firstRequest, secondRequest]);

    expect(thumbnailRequests).toBe(1);
    expect(first.src).toBe("");
    expect(replacement.src).toBe("/thumb.jpg");
    staleError?.call(first, new Event("error"));
    expect(replacement.deliveryFailed).toBe(false);
    expect(owner.retainedThumbnailDeliveryFailureCount).toBe(0);
    replacement.onerror?.call(
      replacement,
      new Event("error"),
      "",
      0,
      0,
      new Error("failed"),
    );
    expect(replacement.deliveryFailed).toBe(true);
    expect(owner.retainedThumbnailDeliveryFailureCount).toBe(1);
  });

  test("retains the same failed hydrated URL across Grid replacement and accepts a changed URL", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 1));
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    await openLibrary(owner);

    const failed = new FakeImage();
    owner.presentThumbnail("photo-0", failed, "/hydrated.jpg", true);
    failed.onerror?.call(
      failed,
      new Event("error"),
      "",
      0,
      0,
      new Error("failed"),
    );
    expect(failed.deliveryFailed).toBe(true);
    expect(owner.retainedThumbnailDeliveryFailureCount).toBe(1);

    const sameUrl = new FakeImage();
    owner.releaseThumbnail("photo-0", failed);
    owner.presentThumbnail("photo-0", sameUrl, "/hydrated.jpg", true);
    expect(sameUrl.deliveryFailed).toBe(true);
    expect(sameUrl.src).toBe("");
    expect(owner.retainedThumbnailDeliveryFailureCount).toBe(1);

    const changedUrl = new FakeImage();
    owner.releaseThumbnail("photo-0", sameUrl);
    owner.presentThumbnail("photo-0", changedUrl, "/replacement.jpg", true);
    expect(changedUrl.deliveryFailed).toBe(false);
    expect(changedUrl.src).toBe("/replacement.jpg");
    expect(owner.retainedThumbnailDeliveryFailureCount).toBe(0);
  });

  test("bounds the rebuildable Thumbnail cache independently of Library size", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 241));
      if (url.pathname.startsWith("/api/photos/")) {
        const id = url.pathname.split("/")[3];
        return Promise.resolve(
          new Response(JSON.stringify({ state: "ready", url: `/${id}.jpg` }), {
            status: 200,
          }),
        );
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    await openLibrary(owner);

    let previous: Readonly<{ id: string; image: FakeImage }> | undefined;
    for (let index = 0; index < 241; index += 1) {
      const id = `photo-${index}`;
      const image = new FakeImage();
      await owner.loadThumbnail(id, image);
      if (previous) owner.releaseThumbnail(previous.id, previous.image);
      previous = { id, image };
    }
    expect(owner.retainedImageCount).toBe(1);
    expect(owner.retainedThumbnailCount).toBe(240);
  });

  test("bounds Thumbnail delivery failures and accepts a newly hydrated URL", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 241));
      if (url.pathname.startsWith("/api/photos/"))
        return Promise.resolve(new Response(null, { status: 503 }));
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    await openLibrary(owner);
    let previous: Readonly<{ id: string; image: FakeImage }> | undefined;
    for (let index = 0; index < 241; index += 1) {
      const id = `photo-${index}`;
      const image = new FakeImage();
      await owner.loadThumbnail(id, image);
      expect(image.deliveryFailed).toBe(true);
      if (previous) owner.releaseThumbnail(previous.id, previous.image);
      previous = { id, image };
    }
    expect(owner.retainedThumbnailDeliveryFailureCount).toBe(240);
    expect(owner.retainedImageCount).toBe(1);

    const hydrated = new FakeImage();
    owner.releaseThumbnail("photo-240", previous!.image);
    owner.presentThumbnail("photo-240", hydrated, "/hydrated.jpg", true);
    expect(hydrated.src).toBe("/hydrated.jpg");
    expect(hydrated.deliveryFailed).toBe(false);
    expect(owner.retainedThumbnailDeliveryFailureCount).toBe(239);
  });

  test("releases the owner's hold on Grid images the view drops", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 720));
      if (url.pathname.startsWith("/api/photos/")) {
        const id = url.pathname.split("/")[3];
        return Promise.resolve(
          new Response(JSON.stringify({ state: "ready", url: `/${id}.jpg` }), {
            status: 200,
          }),
        );
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    await openLibrary(owner);

    // A long scroll across twelve windows drops every cell it leaves behind.
    const rendered: Array<Readonly<{ id: string; image: FakeImage }>> = [];
    for (let index = 0; index < 720; index += 1) {
      const id = `photo-${index}`;
      const image = new FakeImage();
      await owner.loadThumbnail(id, image);
      expect(image.src).toBe(`/${id}.jpg`);
      rendered.push({ id, image });
      if (rendered.length > 200) {
        const dropped = rendered.shift()!;
        owner.releaseThumbnail(dropped.id, dropped.image);
      }
      expect(owner.retainedImageCount).toBeLessThanOrEqual(200);
    }
    for (const { id, image } of rendered) owner.releaseThumbnail(id, image);
    expect(owner.retainedImageCount).toBe(0);

    // Releasing a cell with a pending transfer drops the browser-managed
    // source without touching the rebuildable URL cache.
    const pending = new FakeImage();
    owner.presentThumbnail("photo-700", pending, "/pending.jpg", true);
    expect(pending.src).toBe("/pending.jpg");
    owner.releaseThumbnail("photo-700", pending);
    expect(pending.src).toBe("");
    expect(pending.onload).toBeNull();
    expect(pending.onerror).toBeNull();
    expect(owner.retainedThumbnailCount).toBeGreaterThan(0);
    owner.dispose();
  });

  test("cleans a browser-managed image transfer exactly once", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 1));
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    await openLibrary(owner);
    const image = new FakeImage();
    owner.presentThumbnail("photo-0", image, "/pending.jpg", true);
    owner.stopGridWork();
    owner.dispose();
    expect(image.src).toBe("");
    expect(image.removeCalls).toBe(1);
    expect(image.onload).toBeNull();
    expect(image.onerror).toBeNull();
  });

  test("coalesces concurrent range demands into one fetch and one settlement", async () => {
    const pendingWindow = deferred<Response>();
    let windowRequests = 0;
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 180));
      if (url.pathname === "/api/browse/browse-1") {
        const start = Number(url.searchParams.get("start"));
        windowRequests += 1;
        return windowRequests === 1
          ? pendingWindow.promise
          : Promise.resolve(windowResponse(start, 180));
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const authority = await openLibrary(owner);
    const settled: SourceWindowOutcome[] = [];
    const unsubscribe = owner.onWindowSettled((outcome) =>
      settled.push(outcome),
    );

    for (let demand = 0; demand < 5; demand += 1)
      owner.ensureRange(0, 60, { kind: "grid", authority });
    await flushTasks();
    expect(windowRequests).toBe(1);
    expect(settled).toHaveLength(0);

    pendingWindow.resolve(windowResponse(0, 180));
    await flushTasks();
    expect(settled).toHaveLength(1);
    expect(settled[0]).toMatchObject({
      kind: "loaded",
      start: 0,
      changed: true,
    });
    expect(windowRequests).toBe(1);

    unsubscribe();
    owner.ensureRange(60, 120, { kind: "grid", authority });
    await flushTasks();
    expect(settled).toHaveLength(1);
    expect(windowRequests).toBe(2);
    expect(owner.photoAt(60)?.id).toBe("photo-60");
    owner.dispose();
  });

  test("keeps facts for the latest reported range when an older window settles late", async () => {
    const staleWindow = deferred<Response>();
    const requests: number[] = [];
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 600));
      if (url.pathname === "/api/browse/browse-1") {
        const start = Number(url.searchParams.get("start"));
        requests.push(start);
        return start === 0
          ? staleWindow.promise
          : Promise.resolve(windowResponse(start, 600));
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const authority = await openLibrary(owner);
    const staleLoad = owner.loadWindow(0, { kind: "source", authority });
    await flushTasks();

    owner.ensureRange(240, 420, { kind: "grid", authority });
    owner.ensureRange(240, 420, { kind: "grid", authority });
    await flushTasks();
    expect(requests).toEqual([0, 240, 300, 360]);
    expect(owner.photoAt(240)?.id).toBe("photo-240");
    expect(owner.photoAt(419)?.id).toBe("photo-419");

    staleWindow.resolve(windowResponse(0, 600));
    expect(await staleLoad).toMatchObject({ kind: "loaded", start: 0 });
    expect(owner.photoAt(240)?.id).toBe("photo-240");
    expect(owner.photoAt(300)?.id).toBe("photo-300");
    expect(owner.photoAt(419)?.id).toBe("photo-419");
    owner.dispose();
  });

  test("covers a large reported range and stays bounded when distant windows load", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 1200));
      if (url.pathname === "/api/browse/browse-1") {
        const start = Number(url.searchParams.get("start"));
        return Promise.resolve(windowResponse(start, 1200));
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const authority = await openLibrary(owner);
    owner.ensureRange(0, 300, { kind: "grid", authority });
    await flushTasks();
    expect(owner.retainedFactCount).toBe(300);
    expect(owner.photoAt(0)?.id).toBe("photo-0");
    expect(owner.photoAt(299)?.id).toBe("photo-299");

    for (const index of [600, 660, 720, 780])
      expect(
        await owner.loadWindow(index, { kind: "source", authority }),
      ).toMatchObject({ kind: "loaded" });
    expect(owner.retainedFactCount).toBeLessThanOrEqual(420);
    expect(owner.photoAt(0)?.id).toBe("photo-0");
    expect(owner.photoAt(299)?.id).toBe("photo-299");
    owner.dispose();
  });

  test("a settled Photo window keeps its own facts under the retention bound", async () => {
    const requested: number[] = [];
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 600));
      if (url.pathname === "/api/browse/browse-1") {
        const start = Number(url.searchParams.get("start"));
        requested.push(start);
        return Promise.resolve(windowResponse(start, 600));
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const authority = await openLibrary(owner);
    await owner.loadWindow(0, { kind: "source", authority });
    for (const range of [
      { start: 60, end: 76 },
      { start: 120, end: 136 },
      { start: 100, end: 116 },
    ])
      owner.ensureRange(range.start, range.end, { kind: "grid", authority });
    await flushTasks();
    expect(requested).toEqual([0, 60, 120]);
    expect(owner.retainedFactCount).toBe(180);

    // The Photo window the caller awaited commits the newest facts; its own
    // settlement must not evict them for the older Grid range.
    const photoAuthority = owner.renewPhotoWindow();
    const outcome = await owner.loadWindow(180, {
      kind: "photo",
      authority: photoAuthority,
    });
    expect(outcome).toMatchObject({ kind: "loaded", changed: true });
    expect(owner.photoAt(180)?.id).toBe("photo-180");
    expect(owner.retainedFactCount).toBeLessThanOrEqual(196);
    owner.dispose();
  });

  test("protects the actual clamped tail anchor under retention pressure", async () => {
    const requested: number[] = [];
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 400));
      if (url.pathname === "/api/browse/browse-1") {
        const start = Number(url.searchParams.get("start"));
        requested.push(start);
        return Promise.resolve(windowResponse(start, 400));
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const authority = await openLibrary(owner);
    const grid = { kind: "grid" as const, authority };

    // Protect an earlier visible span, then fill the map to the bound with a
    // window that the old aligned tail anchor would protect.
    owner.ensureRange(60, 120, grid);
    await flushTasks();
    for (const start of [0, 120, 300])
      expect(
        await owner.loadWindow(start, { kind: "source", authority }),
      ).toMatchObject({
        kind: "loaded",
      });
    expect(owner.retainedFactCount).toBe(240);

    const photoAuthority = owner.renewPhotoWindow();
    expect(
      await owner.loadWindow(399, {
        kind: "photo",
        authority: photoAuthority,
      }),
    ).toMatchObject({ kind: "loaded" });
    expect(requested).toContain(340);
    expect(owner.retainedFactCount).toBe(240);
    // The old aligned anchor protected [300,360) and evicted 360–399. The
    // committed clamped [340,400) window must survive as one whole window.
    expect(owner.photoAt(340)?.id).toBe("photo-340");
    expect(owner.photoAt(360)?.id).toBe("photo-360");
    expect(owner.photoAt(399)?.id).toBe("photo-399");
    owner.dispose();
  });

  test("clamps pathological range reports and keeps the fixed floor", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 600));
      if (url.pathname === "/api/browse/browse-1") {
        const start = Number(url.searchParams.get("start"));
        return Promise.resolve(windowResponse(start, 600));
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const authority = await openLibrary(owner);

    owner.ensureRange(-100_000, 100_000, { kind: "grid", authority });
    owner.ensureRange(Number.NaN, 100, { kind: "grid", authority });
    await flushTasks();
    // One supported large-viewport range is retained with its buffer, not the
    // whole reported source.
    expect(owner.retainedFactCount).toBe(420);
    expect(owner.photoAt(0)?.id).toBe("photo-0");
    expect(owner.photoAt(359)?.id).toBe("photo-359");
    expect(owner.photoAt(420)).toBeUndefined();

    owner.ensureRange(300, 360, { kind: "grid", authority });
    // The reported range and its buffer stay protected while the oldest facts
    // leave until the bound holds.
    expect(owner.retainedFactCount).toBe(240);
    expect(owner.photoAt(300)?.id).toBe("photo-300");
    expect(owner.photoAt(359)?.id).toBe("photo-359");
    expect(owner.photoAt(179)).toBeUndefined();
    expect(owner.photoAt(420)).toBeUndefined();
    owner.dispose();
  });

  test("visits the clamped tail window the range start cannot align to", async () => {
    const rangeWindows = async (
      total: number,
      range: Readonly<{ start: number; end: number }>,
    ): Promise<Readonly<{ requested: number[]; missing: number[] }>> => {
      const requested: number[] = [];
      const owner = createSourceGridOwner((input, init) => {
        const url = requestUrl(input);
        if (url.pathname === "/api/browse" && init?.method === "POST")
          return Promise.resolve(opened("browse-1", total));
        if (url.pathname === "/api/browse/browse-1") {
          const start = Number(url.searchParams.get("start"));
          requested.push(start);
          return Promise.resolve(windowResponse(start, total));
        }
        if (init?.method === "DELETE")
          return Promise.resolve(new Response(null, { status: 204 }));
        throw new Error(`unexpected request ${url.pathname}`);
      });
      const authority = await openLibrary(owner);
      await owner.loadWindow(0, { kind: "source", authority });
      const before = requested.length;
      owner.ensureRange(range.start, range.end, { kind: "grid", authority });
      await flushTasks();
      const missing: number[] = [];
      for (
        let index = range.start;
        index < Math.min(range.end, total);
        index += 1
      )
        if (owner.photoAt(index) === undefined) missing.push(index);
      const result = { requested: requested.slice(before), missing };
      owner.dispose();
      return result;
    };

    // The 400-Photo tail window is [340,400): a range starting at 220 aligns
    // to 180 and would stop at 300 without the clamped tail step.
    expect(await rangeWindows(400, { start: 220, end: 400 })).toEqual({
      requested: [180, 240, 300, 340],
      missing: [],
    });
    expect(await rangeWindows(400, { start: 340, end: 400 })).toEqual({
      requested: [300, 340],
      missing: [],
    });
    // The 70-Photo tail window is [10,70): the window at 0 is already loaded.
    expect(await rangeWindows(70, { start: 58, end: 70 })).toEqual({
      requested: [10],
      missing: [],
    });
    // Multiples of the window size keep their aligned walk.
    expect(await rangeWindows(300, { start: 180, end: 300 })).toEqual({
      requested: [180, 240],
      missing: [],
    });
  });

  test("does not re-request a fully loaded tail window", async () => {
    const requested: number[] = [];
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 400));
      if (url.pathname === "/api/browse/browse-1") {
        const start = Number(url.searchParams.get("start"));
        requested.push(start);
        return Promise.resolve(windowResponse(start, 400));
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const authority = await openLibrary(owner);
    await owner.loadWindow(0, { kind: "source", authority });
    owner.ensureRange(220, 400, { kind: "grid", authority });
    await flushTasks();
    expect(requested).toEqual([0, 180, 240, 300, 340]);
    expect(owner.photoAt(340)?.id).toBe("photo-340");
    expect(owner.photoAt(399)?.id).toBe("photo-399");
    owner.ensureRange(220, 400, { kind: "grid", authority });
    await flushTasks();
    expect(requested).toEqual([0, 180, 240, 300, 340]);
    owner.dispose();
  });

  test("shares one in-flight window between range admission and a control-flow load", async () => {
    const pendingWindow = deferred<Response>();
    let windowRequests = 0;
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 180));
      if (url.pathname === "/api/browse/browse-1") {
        windowRequests += 1;
        return pendingWindow.promise;
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const authority = await openLibrary(owner);
    const settled: SourceWindowOutcome[] = [];
    owner.onWindowSettled((outcome) => settled.push(outcome));

    owner.ensureRange(0, 60, { kind: "source", authority });
    const awaiting = owner.loadWindow(0, { kind: "source", authority });
    await flushTasks();
    expect(windowRequests).toBe(1);

    pendingWindow.resolve(windowResponse(0, 180));
    expect(await awaiting).toMatchObject({
      kind: "loaded",
      start: 0,
      changed: true,
    });
    await flushTasks();
    // The awaiting caller settles this window: one completion is presented,
    // by the caller, and the merged notification reports only the windows no
    // caller joined.
    expect(settled).toHaveLength(0);
    owner.dispose();
  });

  test("shares one in-flight window between a Grid range and a source load", async () => {
    const pendingWindow = deferred<Response>();
    let windowRequests = 0;
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 180));
      if (url.pathname === "/api/browse/browse-1") {
        windowRequests += 1;
        return pendingWindow.promise;
      }
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const authority = await openLibrary(owner);
    const settled: SourceWindowOutcome[] = [];
    owner.onWindowSettled((outcome) => settled.push(outcome));

    // A Grid range admission and the source open that awaits the same aligned
    // window share one in-flight request, and the caller settles it.
    owner.ensureRange(0, 60, { kind: "grid", authority });
    const awaiting = owner.loadWindow(0, { kind: "source", authority });
    await flushTasks();
    expect(windowRequests).toBe(1);

    pendingWindow.resolve(windowResponse(0, 180));
    expect(await awaiting).toMatchObject({
      kind: "loaded",
      start: 0,
      changed: true,
    });
    await flushTasks();
    expect(settled).toHaveLength(0);
    owner.dispose();
  });

  test("one throwing settlement subscriber cannot silence the others", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 180));
      if (url.pathname === "/api/browse/browse-1")
        return Promise.resolve(new Response(null, { status: 503 }));
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const authority = await openLibrary(owner);
    const settled: SourceWindowOutcome[] = [];
    owner.onWindowSettled(() => {
      throw new Error("subscriber failed");
    });
    owner.onWindowSettled((outcome) => settled.push(outcome));

    owner.ensureRange(0, 60, { kind: "grid", authority });
    await flushTasks();
    expect(settled.map((outcome) => outcome.kind)).toEqual(["failed"]);
    owner.dispose();
  });

  test("notifies failed and expired range settlements once", async () => {
    const responses = [
      new Response(null, { status: 404 }),
      new Response(null, { status: 503 }),
    ];
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 180));
      if (url.pathname === "/api/browse/browse-1")
        return Promise.resolve(responses.shift()!);
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const authority = await openLibrary(owner);
    const settled: SourceWindowOutcome[] = [];
    owner.onWindowSettled((outcome) => settled.push(outcome));

    owner.ensureRange(0, 120, { kind: "grid", authority });
    await flushTasks();
    expect(settled.map((outcome) => outcome.kind).sort()).toEqual([
      "expired",
      "failed",
    ]);
    expect(owner.retryRequired).toBe(true);
    expect(owner.photoAt(0)).toBeUndefined();
    owner.dispose();
  });
});
