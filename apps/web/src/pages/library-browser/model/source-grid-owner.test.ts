import { describe, expect, test } from "bun:test";
import { TaskScope } from "./async-ownership.js";
import {
  createSourceGridOwner,
  type GridThumbnailImage,
  type SourceGridOwner,
} from "./source-grid-owner.js";

const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
};

const opened = (token: string, total = 180, position = 0) =>
  new Response(JSON.stringify({ token, total, position }), { status: 200 });

const photo = (id: string) => ({
  id,
  available: true,
  ambiguous: false,
  originals: [{ kind: "jpeg" as const, available: true }],
  selectionState: "undecided" as const,
  rating: 0,
  preview: { state: "inspection-pending" as const },
});

const windowResponse = (start: number, total = 180, count = 60) =>
  new Response(
    JSON.stringify({
      start,
      total,
      photos: Array.from(
        { length: Math.min(count, Math.max(0, total - start)) },
        (_, offset) => photo(`photo-${start + offset}`),
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

class FakeImage implements GridThumbnailImage {
  alt = "Photo";
  complete = false;
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
    const photoTasks = new TaskScope();
    const foreground = owner.loadWindow(
      0,
      { kind: "photo", authority, generation: 17, tasks: photoTasks },
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
    const authority = await openLibrary(owner);
    const tasks = new TaskScope();
    const operation = {
      kind: "photo" as const,
      authority,
      generation: 9,
      tasks,
    };

    expect(await owner.loadWindow(0, operation)).toMatchObject({
      kind: "expired",
      owner: { scope: "photo", generation: 9 },
    });
    expect(await owner.loadWindow(60, operation)).toMatchObject({
      kind: "failed",
      owner: { scope: "photo", generation: 9 },
      transportLost: false,
      status: 503,
    });
    expect(await owner.loadWindow(120, operation)).toMatchObject({
      kind: "failed",
      owner: { scope: "photo", generation: 9 },
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
            ? new Response('{"token":"","total":60,"position":0}', {
                status: 200,
              })
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

    owner.beginGridRender();
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
    expect(replacement.alt).toBe("Photo");
    replacement.onerror?.call(
      replacement,
      new Event("error"),
      "",
      0,
      0,
      new Error("failed"),
    );
    expect(replacement.alt).toContain("Thumbnail unavailable");
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

    for (let index = 0; index < 241; index += 1) {
      owner.beginGridRender();
      await owner.loadThumbnail(`photo-${index}`, new FakeImage());
    }
    expect(owner.retainedThumbnailCount).toBe(240);
  });

  test("bounds Thumbnail failures and accepts a newly hydrated URL", async () => {
    const owner = createSourceGridOwner((input, init) => {
      const url = requestUrl(input);
      if (url.pathname === "/api/browse" && init?.method === "POST")
        return Promise.resolve(opened("browse-1", 241));
      if (init?.method === "DELETE")
        return Promise.resolve(new Response(null, { status: 204 }));
      throw new Error(`unexpected request ${url.pathname}`);
    });
    await openLibrary(owner);
    for (let index = 0; index < 241; index += 1) {
      owner.beginGridRender();
      owner.markThumbnailUnavailable(`photo-${index}`, new FakeImage());
    }
    expect(owner.retainedThumbnailFailureCount).toBe(240);

    const hydrated = new FakeImage();
    owner.beginGridRender();
    owner.presentThumbnail("photo-240", hydrated, "/hydrated.jpg", true);
    expect(hydrated.src).toBe("/hydrated.jpg");
    expect(hydrated.alt).toBe("Photo");
    expect(owner.retainedThumbnailFailureCount).toBe(239);
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
    owner.beginGridRender();
    owner.presentThumbnail("photo-0", image, "/pending.jpg", true);
    owner.stopGridWork();
    owner.dispose();
    expect(image.src).toBe("");
    expect(image.removeCalls).toBe(1);
    expect(image.onload).toBeNull();
    expect(image.onerror).toBeNull();
  });
});
