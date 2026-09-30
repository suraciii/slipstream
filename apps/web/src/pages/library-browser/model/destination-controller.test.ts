import { expect, test } from "bun:test";
import { RecoveryGate } from "./async-ownership.js";
import {
  createDestinationController,
  type SourceReestablishment,
} from "./destination-controller.js";
import { createNavigationSession } from "./navigation-session.js";
import { createSourceGridOwner } from "./source-grid-owner.js";
import type { NavigationDestination } from "./browser-navigation.js";

type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T | PromiseLike<T>) => void;
  reject: (reason?: unknown) => void;
};
const deferred = <T>() => Promise.withResolvers<T>();
const destination = (photoId: string): NavigationDestination => ({
  source: "library",
  selection: "all",
  photoId,
});

async function fixture(reopenFailure?: "failed" | "missing" | "superseded") {
  const requests = new Map<string, Deferred<Response>>();
  const requestStarted = new Map<string, Deferred<void>>();
  const source = createSourceGridOwner(
    async (input) => {
      const path = input instanceof Request ? input.url : String(input);
      if (path === "/api/browse")
        return Response.json({
          token: "snapshot",
          total: 120,
          position: 0,
          selectionCounts: { selected: 0, rejected: 0, undecided: 120 },
        });
      const id = new URL(path, "http://test").searchParams.get("photoId")!;
      const response = deferred<Response>();
      requests.set(id, response);
      requestStarted.get(id)?.resolve();
      return response.promise;
    },
    async () => {},
  );
  await source.open({ kind: "library" });
  source.establish(source.authority);
  const session = createNavigationSession({ kind: "invalid" });
  const gate = new RecoveryGate();
  const opened: number[] = [];
  const replacements: NavigationDestination[] = [];
  const statuses: string[] = [];
  const reopenStarted = deferred<void>();
  const reopenContinue = deferred<void>();
  const controller = createDestinationController({
    source,
    session,
    gate,
    navigation: {
      replaceGrid: (value) => {
        replacements.push(value);
      },
    },
    locations: { publication: undefined, window: () => undefined },
    view: {
      prepareSourceOpen: () => {},
      setGridExplanation: () => {},
      showGrid: () => {},
      restoreGridAnchor: () => {},
      closeTransientSurfaces: () => {},
    },
    albums: () => [],
    canResume: () => true,
    openSource: async ({ kind }) => {
      if (kind !== "library" || reopenFailure !== "missing")
        throw new Error("unexpected source open");
      await source.open({ kind: "library" });
      source.establish(source.authority);
      return { kind: "established" } as const;
    },
    reopen: async (): Promise<SourceReestablishment> => {
      reopenStarted.resolve();
      await reopenContinue.promise;
      if (reopenFailure) return { kind: reopenFailure };
      await source.open({ kind: "library" });
      source.establish(source.authority);
      return { kind: "established", authority: source.authority };
    },
    openPhoto: (index) => {
      opened.push(index);
      return Promise.resolve(true);
    },
    leavePhoto: () => {},
    bindRoot: () => Promise.resolve(true),
    loadWindow: () => Promise.resolve(true),
    renderGrid: () => {},
    presentRangeStatus: () => {},
    setGridStatus: (message) => {
      statuses.push(message);
    },
    syncConnection: () => {},
    updateControls: () => {},
  });
  const start = async (id: string) => {
    const started = deferred<void>();
    requestStarted.set(id, started);
    const result = controller.establish(destination(id));
    await started.promise;
    return { result };
  };
  const close = () => {
    controller.dispose();
    session.dispose();
    source.dispose();
    gate.close();
  };
  const waitForLookup = (id: string) => {
    const started = deferred<void>();
    requestStarted.set(id, started);
    return started.promise;
  };
  return {
    controller,
    source,
    session,
    requests,
    opened,
    replacements,
    statuses,
    start,
    close,
    reopenStarted,
    reopenContinue,
    waitForLookup,
  };
}

test.each([
  Response.json({ position: 10 }),
  Response.json({ position: null }),
  new Response(null, { status: 503 }),
])(
  "an older same-Snapshot lookup cannot open, replace or fail the newer destination",
  async (response) => {
    const f = await fixture();
    try {
      const a = await f.start("photo-a");
      const b = await f.start("photo-b");
      f.requests.get("photo-b")!.resolve(Response.json({ position: 70 }));
      expect(await b.result).toBe(true);
      f.requests.get("photo-a")!.resolve(response);
      expect(await a.result).toBe(false);
      expect(f.opened).toEqual([70]);
      expect(f.replacements).toEqual([]);
      expect(f.session.takeRetry()).toBeUndefined();
    } finally {
      f.close();
    }
  },
);

test("disposal suppresses a position lookup already admitted by the real source owner", async () => {
  const f = await fixture();
  try {
    const pending = await f.start("photo-a");
    f.controller.dispose();
    f.requests.get("photo-a")!.resolve(Response.json({ position: 10 }));
    expect(await pending.result).toBe(false);
    expect(f.opened).toEqual([]);
    expect(f.replacements).toEqual([]);
  } finally {
    f.close();
  }
});

test("expired position resolves against the reopened authority without invalidating its destination", async () => {
  const f = await fixture();
  try {
    const pending = await f.start("photo-a");
    f.requests.get("photo-a")!.resolve(new Response(null, { status: 404 }));
    await f.reopenStarted.promise;
    const started = f.waitForLookup("photo-a");
    f.reopenContinue.resolve();
    await started;
    f.requests.get("photo-a")!.resolve(Response.json({ position: 11 }));
    expect(await pending.result).toBe(true);
    expect(f.opened).toEqual([11]);
  } finally {
    f.close();
  }
});

test.each(["failed", "missing", "superseded"] as const)(
  "expired lookup handles a %s reopen without opening an unrelated Photo",
  async (kind) => {
    const f = await fixture(kind);
    try {
      const pending = await f.start("photo-a");
      f.requests.get("photo-a")!.resolve(new Response(null, { status: 404 }));
      await f.reopenStarted.promise;
      f.reopenContinue.resolve();
      expect(await pending.result).toBe(kind === "missing");
      expect(f.opened).toEqual([]);
      expect(f.replacements).toEqual(
        kind === "missing" ? [{ source: "library", selection: "all" }] : [],
      );
      expect(f.session.takeRetry()).toEqual(
        kind === "failed" ? destination("photo-a") : undefined,
      );
    } finally {
      f.close();
    }
  },
);

test("a newer destination suppresses a delayed expired-source failure", async () => {
  const f = await fixture("failed");
  try {
    const old = await f.start("photo-a");
    f.requests.get("photo-a")!.resolve(new Response(null, { status: 404 }));
    await f.reopenStarted.promise;
    const next = await f.start("photo-b");
    f.requests.get("photo-b")!.resolve(Response.json({ position: 70 }));
    expect(await next.result).toBe(true);
    f.reopenContinue.resolve();
    expect(await old.result).toBe(false);
    expect(f.opened).toEqual([70]);
    expect(f.replacements).toEqual([]);
    expect(f.session.takeRetry()).toBeUndefined();
  } finally {
    f.close();
  }
});
