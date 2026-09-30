import { afterEach, expect, spyOn, test } from "bun:test";
import { createEditorRenditions } from "./editor-renditions.js";
import {
  encodedSourceRevision,
  type CurrentRendition,
} from "./edit-preview.js";

const initial: CurrentRendition = {
  photoId: "photo-a",
  stage: "develop",
  sourceRevision: "source-1",
  recipeVersion: "recipe-1",
  editSource: "original",
  editSourceProxyId: null,
};
const opened: Array<() => void> = [];
afterEach(() => {
  for (const close of opened.splice(0)) close();
});
function fixture() {
  let identity = initial;
  let alive = true;
  const requests: Array<{
    url: string;
    signal: AbortSignal;
    response: {
      promise: Promise<Response>;
      resolve: (value: Response | PromiseLike<Response>) => void;
      reject: (reason?: unknown) => void;
    };
  }> = [];
  const presented: string[] = [];
  const refusal = Promise.withResolvers<string>();
  const owner = createEditorRenditions({
    fetcher: (input, init) => {
      const response = Promise.withResolvers<Response>();
      requests.push({
        url: input instanceof Request ? input.url : String(input),
        signal: init!.signal!,
        response,
      });
      return response.promise;
    },
    capture: () => identity,
    ownsPhoto: (photoId) => alive && photoId === identity.photoId,
    describeRefusal: () => refusal.promise,
    changed: () => {},
    present: (_settings, url) => {
      presented.push(url);
    },
  });
  opened.push(owner.clear);
  return {
    owner,
    requests,
    presented,
    refusal,
    change: (next: CurrentRendition) => {
      identity = next;
    },
    leave: () => {
      alive = false;
      owner.clear();
    },
  };
}
function image(identity = initial, baseline = false) {
  return new Response(new Uint8Array([1, 2, 3]), {
    headers: {
      "content-type": "image/jpeg",
      "content-length": "3",
      "slipstream-edit-preview-width": "1",
      "slipstream-edit-preview-height": "1",
      "slipstream-edit-preview-sha256": "a".repeat(64),
      "slipstream-edit-preview-display-transform": "display-transform-v1",
      "slipstream-edit-preview-photo-id": identity.photoId,
      "slipstream-edit-preview-stage": identity.stage,
      "slipstream-edit-preview-settings": baseline ? "baseline" : "current",
      "slipstream-edit-preview-source-revision": encodedSourceRevision(
        identity.sourceRevision ?? "",
      ),
      "slipstream-edit-preview-recipe-version": identity.recipeVersion,
      "slipstream-edit-preview-source": identity.editSource,
      "slipstream-edit-preview-proxy-id": identity.editSourceProxyId ?? "",
    },
  });
}

test("current and baseline retain independent images; settings changes retain only the baseline as current", async () => {
  const f = fixture();
  const current = f.owner.requestCurrent("photo-a");
  const baseline = f.owner.requestComparison("photo-a");
  f.requests[0]!.response.resolve(image());
  f.requests[1]!.response.resolve(image(initial, true));
  await Promise.all([current, baseline]);
  const currentUrl = f.owner.current.url!;
  const baselineUrl = f.owner.comparison.url!;
  expect(currentUrl).not.toBe(baselineUrl);
  expect(f.presented).toEqual([currentUrl, baselineUrl]);
  f.owner.markStale();
  const next = { ...initial, recipeVersion: "recipe-2" };
  f.change(next);
  f.owner.retainComparison(next);
  expect(f.owner.current.stale).toBe(true);
  expect(f.owner.comparison.url).toBe(baselineUrl);
  const refreshed = f.owner.requestCurrent("photo-a");
  f.requests[2]!.response.resolve(image(next));
  await refreshed;
  expect(f.owner.current.outcome).toBe("ready");
  expect(f.owner.current.stale).toBe(false);
  expect(f.owner.current.url).not.toBe(currentUrl);
  expect(f.owner.comparison.url).toBe(baselineUrl);
  f.owner.retainComparison({ ...next, sourceRevision: "source-2" });
  expect(f.owner.comparison.url).toBeUndefined();
});

test("an identical current request joins through blob decoding; a different recipe supersedes it", async () => {
  const f = fixture();
  const body = Promise.withResolvers<Blob>();
  const reading = Promise.withResolvers<void>();
  const response = image();
  response.blob = () => {
    reading.resolve();
    return body.promise;
  };
  const old = f.owner.requestCurrent("photo-a");
  f.requests[0]!.response.resolve(response);
  await reading.promise;
  await f.owner.requestCurrent("photo-a");
  expect(f.requests.map((request) => request.url)).toEqual([
    "/api/photos/photo-a/edit-preview/develop?settings=current",
  ]);
  const next = { ...initial, recipeVersion: "recipe-2" };
  f.change(next);
  const newer = f.owner.requestCurrent("photo-a");
  expect(f.requests[0]!.signal.aborted).toBe(true);
  f.requests[1]!.response.resolve(image(next));
  await newer;
  const url = f.owner.current.url;
  body.resolve(new Blob([new Uint8Array([1, 2, 3])]));
  await old;
  expect(f.owner.current.url).toBe(url);
  expect(f.presented).toEqual([url!]);
});

for (const late of ["pending", "refusal", "unreadable"] as const) {
  test(`a superseded ${late} body cannot replace the newer rendition or schedule work`, async () => {
    const f = fixture();
    const started = Promise.withResolvers<void>();
    const body = Promise.withResolvers<unknown>();
    const response =
      late === "unreadable"
        ? image()
        : new Response(null, { status: late === "pending" ? 202 : 503 });
    if (late === "pending")
      response.json = () => {
        started.resolve();
        return body.promise;
      };
    if (late === "unreadable")
      response.blob = () => {
        started.resolve();
        return body.promise as Promise<Blob>;
      };
    if (late === "refusal") started.resolve();
    const old = f.owner.requestCurrent("photo-a");
    f.requests[0]!.response.resolve(response);
    await started.promise;
    // Advance both the Photo authority and lane generation, as leaving the
    // Editor does before the next Photo opens.
    f.owner.clear();
    const next = { ...initial, photoId: "photo-b" };
    f.change(next);
    const newer = f.owner.requestCurrent("photo-b");
    f.requests[1]!.response.resolve(image(next));
    await newer;
    const url = f.owner.current.url;
    if (late === "pending") body.resolve({ state: "running" });
    if (late === "refusal") f.refusal.resolve("Old refusal");
    if (late === "unreadable") body.reject(new Error("lost body"));
    await old;
    expect(f.owner.current.url).toBe(url);
    expect(f.owner.current.outcome).toBe("ready");
    expect(f.owner.current.pending).toBe(false);
    expect(f.presented).toEqual([url!]);
  });
}

test("a refusal retains an older current image and never accepts another proxy identity", async () => {
  const f = fixture();
  const ready = f.owner.requestCurrent("photo-a");
  f.requests[0]!.response.resolve(image());
  await ready;
  const url = f.owner.current.url;
  const next = {
    ...initial,
    editSource: "development-proxy" as const,
    editSourceProxyId: "proxy-2",
  };
  f.change(next);
  const wrong = f.owner.requestCurrent("photo-a");
  f.requests[1]!.response.resolve(
    image({ ...next, editSourceProxyId: "proxy-1" }),
  );
  await wrong;
  expect(f.owner.current.url).toBe(url);
  expect(f.owner.current.stale).toBe(true);
  expect(f.owner.current.outcome).toBe("failed");
  expect(f.owner.current.refused).toBe(false);
  expect(f.presented).toEqual([url!]);
});

test("clearing twice revokes both retained URLs once and late transport stays silent", async () => {
  const f = fixture();
  const current = f.owner.requestCurrent("photo-a");
  const baseline = f.owner.requestComparison("photo-a");
  f.requests[0]!.response.resolve(image());
  f.requests[1]!.response.resolve(image(initial, true));
  await Promise.all([current, baseline]);
  const urls = [f.owner.current.url!, f.owner.comparison.url!];
  const revoked = spyOn(URL, "revokeObjectURL");
  try {
    const late = f.owner.requestCurrent("photo-a");
    f.leave();
    f.owner.clear();
    f.requests[2]!.response.reject(new Error("aborted"));
    await late;
    expect(revoked.mock.calls).toEqual(urls.map((url) => [url]));
    expect(f.owner.current.url).toBeUndefined();
    expect(f.owner.comparison.url).toBeUndefined();
    expect(f.owner.current.note).toBe("");
    expect(f.owner.current.busy).toBe(false);
    expect(f.requests[2]!.signal.aborted).toBe(true);
  } finally {
    revoked.mockRestore();
  }
});

test("pending work has a finite independent poll budget and a cleared timer cannot readmit it", async () => {
  const callbacks: Array<() => void> = [];
  const clock = spyOn(globalThis, "setTimeout").mockImplementation(
    Object.assign(
      (...args: unknown[]) => {
        callbacks.push(args[0] as () => void);
        return undefined as never;
      },
      { __promisify__: setTimeout.__promisify__ },
    ),
  );
  const f = fixture();
  try {
    for (let attempt = 0; attempt <= 400; attempt += 1) {
      const request =
        attempt === 0 ? f.owner.requestCurrent("photo-a") : undefined;
      f.requests[attempt]!.response.resolve(
        Response.json({ state: "running" }, { status: 202 }),
      );
      if (request) await request;
      else {
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      }
      if (attempt < 400) callbacks.shift()!();
    }
    expect(f.owner.current.outcome).toBe("unknown");
    expect(f.owner.current.pending).toBe(false);
    expect(callbacks).toEqual([]);
    const baseline = f.owner.requestComparison("photo-a");
    f.requests[401]!.response.resolve(
      Response.json({ state: "queued" }, { status: 202 }),
    );
    await baseline;
    const scheduled = callbacks.shift()!;
    f.owner.clear();
    scheduled();
    expect(f.requests[401]!.url).toBe(
      "/api/photos/photo-a/edit-preview/develop?settings=baseline",
    );
    expect(f.owner.current.outcome).toBe("pending");
    expect(f.owner.comparison.busy).toBe(false);
    expect(callbacks).toEqual([]);
  } finally {
    clock.mockRestore();
  }
});
