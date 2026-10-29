import { expect, test } from "bun:test";
import { createFileLocationController } from "./file-location-controller.js";
import { RecoveryGate } from "./async-ownership.js";
import {
  createApplicationOwner,
  type ApplicationEvent,
} from "./application-owner.js";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((accept) => {
    resolve = accept;
  });
  return { promise, resolve };
}

const windowResponse = (publication: string) =>
  Response.json({
    publication,
    parent: "",
    start: 0,
    limit: 60,
    total: 1,
    children: [
      {
        location: "child",
        name: "child",
        photoCount: 1,
        hasDescendantFolders: false,
      },
    ],
  });
const overviewResponse = () =>
  Response.json({
    published: true,
    publication: "p2",
    photoCount: 1,
    scan: { state: "idle", publication: "p2" },
    albums: [],
  });

function harness(
  fetcher: (input: string) => Promise<Response>,
  refresh = () => Promise.resolve(overviewResponse()),
) {
  const events: ApplicationEvent[] = [];
  const application = createApplicationOwner(refresh, {
    emit: (event) => {
      events.push(event);
    },
    schedule: () => () => {},
  });
  let changes = 0;
  const gate = new RecoveryGate();
  const controller = createFileLocationController({
    fetcher,
    application,
    recoveryGate: gate,
    isAlive: () => true,
    onChanged: () => {
      changes += 1;
    },
    onReachable: () => {},
  });
  return {
    controller,
    gate,
    events,
    changes: () => changes,
    dispose: () => {
      controller.dispose();
      application.dispose();
      gate.close();
    },
  };
}

test("joined failed root loads publish one failure and recover on retry", async () => {
  let offline = true;
  const h = harness(() => {
    if (offline) return Promise.reject(new Error("offline"));
    return Promise.resolve(windowResponse("p1"));
  });
  try {
    await Promise.all([h.controller.load("", 0), h.controller.load("", 0)]);
    const summaries = h.events.filter((event) => event.kind === "summary");
    expect(summaries).toHaveLength(1);
    expect(h.gate.decisionReady).toBe(false);
    const failure = h.controller.owner.failures()[0]!;
    offline = false;
    await h.controller.retry(failure);
    expect(h.controller.owner.failures()).toEqual([]);
    expect(h.controller.publication).toBe("p1");
    expect(h.gate.decisionReady).toBe(true);
  } finally {
    h.dispose();
  }
});

test("a coordinated load preserves pending shared root expansion feedback", async () => {
  const held = deferred<Response>();
  const h = harness(() => held.promise);
  try {
    const expansion = h.controller.toggle("", false);
    const coordinatedLoad = h.controller.load("", 0, false);
    expect(h.controller.tree(() => false)).toMatchObject({
      rootLoading: true,
      rootExpanded: false,
    });
    held.resolve(windowResponse("p1"));
    await Promise.all([expansion, coordinatedLoad]);
    expect(h.controller.tree(() => false)).toMatchObject({
      rootLoading: false,
      rootExpanded: true,
    });
  } finally {
    held.resolve(windowResponse("p1"));
    h.dispose();
  }
});

test("superseded publication rebinding cannot announce another authority's folders", async () => {
  const refresh = deferred<Response>();
  const entered = deferred<void>();
  const h = harness(
    () => Promise.resolve(windowResponse("p2")),
    () => {
      entered.resolve();
      return refresh.promise;
    },
  );
  try {
    const stale = h.controller.rebind();
    await entered.promise;
    h.controller.reset();
    await h.controller.load("", 0);
    refresh.resolve(overviewResponse());
    await stale;
    expect(h.controller.publication).toBe("p2");
    expect(
      h.events.some(
        (event) =>
          event.kind === "summary" &&
          event.summary.text.includes("Reloaded folders"),
      ),
    ).toBe(false);
  } finally {
    h.dispose();
  }
});

test("dispose fences refresh continuation and cannot restart folder transport", async () => {
  const refresh = deferred<Response>();
  const entered = deferred<void>();
  let requests = 0;
  const h = harness(
    () => {
      requests += 1;
      return Promise.resolve(windowResponse("p2"));
    },
    () => {
      entered.resolve();
      return refresh.promise;
    },
  );
  const pending = h.controller.rebind();
  await entered.promise;
  h.controller.dispose();
  const changes = h.changes();
  refresh.resolve(overviewResponse());
  await pending;
  expect(requests).toBe(0);
  expect(h.changes()).toBe(changes);
  h.dispose();
});

test("reopens retained child windows immediately without another request", async () => {
  let requests = 0;
  const h = harness(() => {
    requests += 1;
    return Promise.resolve(windowResponse("p1"));
  });
  try {
    await h.controller.load("", 0, false);
    await h.controller.toggle("", false);
    expect(requests).toBe(1);
    await h.controller.toggle("child", false);
    const retained = h.controller.owner.window("child");
    await h.controller.toggle("child", true);
    const reopening = h.controller.toggle("child", false);
    expect(h.controller.owner.isExpanded("child")).toBe(true);
    expect(h.controller.owner.window("child")).toBe(retained);
    await reopening;
    expect(requests).toBe(2);
    h.controller.reset();
    await h.controller.toggle("", false);
    expect(requests).toBe(3);
  } finally {
    h.dispose();
  }
});

test("shows pending expansion and joins repeated clicks until children arrive", async () => {
  const response = deferred<Response>();
  let requests = 0;
  const h = harness(() => {
    requests += 1;
    return response.promise;
  });
  try {
    const first = h.controller.toggle("", false);
    const second = h.controller.toggle("", false);
    await Promise.resolve();
    expect(h.controller.tree(() => false).rootLoading).toBe(true);
    expect(requests).toBe(1);
    response.resolve(windowResponse("p1"));
    await Promise.all([first, second]);
    expect(h.controller.tree(() => false)).toMatchObject({
      rootLoading: false,
      rootExpanded: true,
      rootChildren: [{ location: "child" }],
    });
  } finally {
    h.dispose();
  }
});

test("an old expansion cannot clear a new generation's loading state", async () => {
  const responses = [deferred<Response>(), deferred<Response>()];
  let requests = 0;
  const h = harness(() => responses[requests++]!.promise);
  try {
    const stale = h.controller.toggle("", false);
    await Promise.resolve();
    h.controller.reset();
    const current = h.controller.toggle("", false);
    await Promise.resolve();
    responses[0]!.resolve(windowResponse("p1"));
    await stale;
    expect(h.controller.tree(() => false).rootLoading).toBe(true);
    responses[1]!.resolve(windowResponse("p2"));
    await current;
    expect(h.controller.tree(() => false).rootLoading).toBe(false);
    expect(h.controller.publication).toBe("p2");
  } finally {
    h.dispose();
  }
});

test("count refresh invalidates collapsed children without losing expanded siblings", async () => {
  const h = harness(() => Promise.resolve(windowResponse("p1")));
  try {
    await h.controller.load("", 0);
    await h.controller.load("child", 0);
    await h.controller.toggle("child", true);
    await h.controller.refreshCounts();
    expect(h.controller.owner.window("child")).toBeUndefined();
    expect(h.controller.owner.isExpanded("")).toBe(true);
  } finally {
    h.dispose();
  }
});

test("refresh discards expanded descendants hidden by collapse or parent paging", async () => {
  for (const hide of ["collapse", "page"] as const) {
    let count = 1;
    const h = harness((input) => {
      const query = new URL(input, "http://slipstream.test").searchParams;
      const parent = query.get("parent") ?? "";
      const location = parent ? `${parent}/child` : "parent";
      const children =
        query.get("start") === "60"
          ? []
          : [
              {
                location,
                name: location.split("/").at(-1),
                photoCount: count,
                hasDescendantFolders: parent !== "parent/child",
              },
            ];
      return Promise.resolve(
        Response.json({
          publication: "p1",
          parent,
          start: Number(query.get("start")),
          limit: 60,
          total: children.length,
          children,
        }),
      );
    });
    try {
      await h.controller.load("", 0);
      await h.controller.toggle("parent", false);
      await h.controller.toggle("parent/child", false);
      if (hide === "collapse") await h.controller.toggle("parent", true);
      else await h.controller.page("parent", 1);
      count = 9;
      await h.controller.refreshCounts();
      expect(h.controller.owner.window("parent/child")).toBeUndefined();
      expect(h.controller.owner.isExpanded("parent/child")).toBe(false);
      if (hide === "collapse") await h.controller.toggle("parent", false);
      else await h.controller.page("parent", -1);
      await h.controller.toggle("parent/child", false);
      expect(
        h.controller.owner.window("parent/child")?.children[0]?.photoCount,
      ).toBe(9);
    } finally {
      h.dispose();
    }
  }
});

test("refresh updates a child collapsed while the root request is pending", async () => {
  const response = Promise.withResolvers<Response>();
  let refreshing = false;
  const h = harness((input) => {
    const parent =
      new URL(input, "http://slipstream.test").searchParams.get("parent") ?? "";
    if (refreshing && !parent) return response.promise;
    if (!parent) return Promise.resolve(windowResponse("p1"));
    return Promise.resolve(
      Response.json({
        publication: "p1",
        parent,
        start: 0,
        limit: 60,
        total: 1,
        children: [
          {
            location: "child/leaf",
            name: "leaf",
            photoCount: refreshing ? 9 : 1,
            hasDescendantFolders: false,
          },
        ],
      }),
    );
  });
  try {
    await h.controller.load("", 0);
    await h.controller.toggle("child", false);
    refreshing = true;
    const refresh = h.controller.refreshCounts();
    await h.controller.toggle("child", true);
    response.resolve(windowResponse("p1"));
    await refresh;
    expect(h.controller.owner.isExpanded("child")).toBe(false);
    await h.controller.toggle("child", false);
    expect(h.controller.owner.window("child")?.children[0]?.photoCount).toBe(9);
  } finally {
    h.dispose();
  }
});

test("a late count refresh cannot discard the next publication's retained windows", async () => {
  const response = Promise.withResolvers<Response>();
  let holdRoot = false;
  const h = harness((input) => {
    const parent = new URL(input, "http://slipstream.test").searchParams.get(
      "parent",
    );
    return holdRoot && !parent
      ? response.promise
      : Promise.resolve(windowResponse("p1"));
  });
  try {
    await h.controller.load("", 0);
    holdRoot = true;
    const staleRefresh = h.controller.refreshCounts();
    h.controller.reset();
    holdRoot = false;
    await h.controller.load("", 0);
    await h.controller.toggle("child", false);
    await h.controller.toggle("child", true);
    const retained = h.controller.owner.window("child");
    response.resolve(windowResponse("p1"));
    await staleRefresh;
    expect(h.controller.owner.window("child")).toBe(retained);
    const reopened = h.controller.toggle("child", false);
    expect(h.controller.owner.isExpanded("child")).toBe(true);
    await reopened;
  } finally {
    h.dispose();
  }
});

test("a Folder reopened during a sibling refresh cannot keep old counts", async () => {
  const rootResponse = Promise.withResolvers<Response>();
  const siblingResponse = Promise.withResolvers<Response>();
  const siblingStarted = Promise.withResolvers<void>();
  let refreshing = false;
  const window = (parent: string, count: number) =>
    Response.json({
      publication: "p1",
      parent,
      start: 0,
      limit: 60,
      total: parent ? 1 : 2,
      children: (parent ? [`${parent}/leaf`] : ["a", "b"]).map((location) => ({
        location,
        name: location,
        photoCount: count,
        hasDescendantFolders: !parent,
      })),
    });
  const h = harness((input) => {
    const parent =
      new URL(input, "http://slipstream.test").searchParams.get("parent") ?? "";
    if (refreshing && !parent) return rootResponse.promise;
    if (refreshing && parent === "b") {
      siblingStarted.resolve();
      return siblingResponse.promise;
    }
    return Promise.resolve(window(parent, refreshing ? 9 : 1));
  });
  try {
    await h.controller.load("", 0);
    await h.controller.toggle("a", false);
    await h.controller.toggle("b", false);
    refreshing = true;
    const refresh = h.controller.refreshCounts();
    await h.controller.toggle("a", true);
    rootResponse.resolve(window("", 9));
    await siblingStarted.promise;
    await h.controller.toggle("a", false);
    siblingResponse.resolve(window("b", 9));
    await refresh;
    expect(h.controller.owner.window("a")?.children[0]?.photoCount).toBe(9);
    expect(h.controller.owner.isExpanded("a")).toBe(true);
  } finally {
    h.dispose();
  }
});

test("failed expansion clears loading and remains retryable", async () => {
  const response = deferred<Response>();
  const h = harness(() => response.promise);
  try {
    const pending = h.controller.toggle("", false);
    expect(h.controller.tree(() => false).rootLoading).toBe(true);
    response.resolve(new Response(null, { status: 503 }));
    await pending;
    expect(h.controller.tree(() => false)).toMatchObject({
      rootLoading: false,
      rootExpanded: false,
      failures: [{ range: "Library Folder items 1–60" }],
    });
  } finally {
    h.dispose();
  }
});

test("a winning retry releases a superseded expansion without waiting for its transport", async () => {
  const held = Promise.withResolvers<Response>();
  let childRequests = 0;
  const childResponse = () =>
    Response.json({
      publication: "p1",
      parent: "child",
      start: 0,
      limit: 60,
      total: 0,
      children: [],
    });
  const h = harness((input) => {
    const parent = new URL(input, "http://slipstream.test").searchParams.get(
      "parent",
    );
    if (!parent) return Promise.resolve(windowResponse("p1"));
    childRequests += 1;
    if (childRequests === 1)
      return Promise.resolve(new Response(null, { status: 503 }));
    return childRequests === 2
      ? held.promise
      : Promise.resolve(childResponse());
  });
  let expansion: Promise<void> | undefined;
  try {
    await h.controller.load("", 0);
    await h.controller.toggle("child", false);
    const failure = h.controller.owner.failures()[0]!;
    expansion = h.controller.toggle("child", false);
    await h.controller.retry(failure);
    expect(h.controller.tree(() => false).rootChildren[0]?.loading).toBe(false);
    expect(h.controller.owner.isExpanded("child")).toBe(true);
    const collapse = h.controller.toggle("child", true);
    expect(h.controller.owner.isExpanded("child")).toBe(false);
    await collapse;
  } finally {
    held.resolve(childResponse());
    await expansion;
    h.dispose();
  }
});
