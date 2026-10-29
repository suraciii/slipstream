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

const requestRange = (input: string) => {
  const query = new URL(input, "http://slipstream.test").searchParams;
  return {
    parent: query.get("parent") ?? "",
    page: Number(query.get("start")) / 60,
  };
};

const folderWindow = (
  parent: string,
  page: number,
  count: number,
  locations: string[],
  total = locations.length,
) =>
  Response.json({
    publication: "p1",
    parent,
    start: page * 60,
    limit: 60,
    total,
    children: locations.map((location) => ({
      location,
      name: location.split("/").at(-1),
      photoCount: count,
      hasDescendantFolders: true,
    })),
  });

// Detached caller promises can settle before their old transports are consumed.
const afterTransport = () => {
  const turn = Promise.withResolvers<void>();
  setImmediate(turn.resolve);
  return turn.promise;
};

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

test("count refresh replaces a first pending child expansion without waiting for its old transport", async () => {
  const held = deferred<Response>();
  const started = deferred<void>();
  let childRequests = 0;
  let fresh = false;
  const h = harness((input) => {
    const { parent, page } = requestRange(input);
    if (parent === "child" && ++childRequests === 1) {
      started.resolve();
      return held.promise;
    }
    return Promise.resolve(
      folderWindow(
        parent,
        page,
        fresh ? 9 : 1,
        parent ? ["child/leaf"] : ["child"],
      ),
    );
  });
  let stale: Promise<void> | undefined;
  let refresh: Promise<void> | undefined;
  let refreshed = false;
  let settled = false;
  try {
    await h.controller.load("", 0);
    stale = h.controller.toggle("child", false).then(() => {
      settled = true;
    });
    await started.promise;
    expect(h.controller.owner.window("child")).toBeUndefined();
    expect(h.controller.tree(() => false).rootChildren[0]?.loading).toBe(true);
    fresh = true;
    refresh = h.controller.refreshCounts().then(() => {
      refreshed = true;
    });
    await afterTransport();
    expect(refreshed).toBe(true);
    expect(settled).toBe(true);
    expect(h.controller.tree(() => false).rootChildren[0]).toMatchObject({
      photoCount: 9,
      expanded: true,
      loading: false,
      children: [{ location: "child/leaf", photoCount: 9 }],
    });
    held.resolve(folderWindow("child", 0, 1, ["child/old"]));
    await stale;
    await afterTransport();
    expect(h.controller.owner.window("child")?.children[0]).toMatchObject({
      location: "child/leaf",
      photoCount: 9,
    });
  } finally {
    held.resolve(folderWindow("child", 0, 1, ["child/old"]));
    await Promise.all([stale, refresh]);
    h.dispose();
  }
});

test("count refresh detaches pending pages hidden by an ancestor or a parent page", async () => {
  for (const hide of ["collapse", "page"] as const) {
    const held = deferred<Response>();
    const started = deferred<void>();
    let hold = false;
    let count = 1;
    const h = harness((input) => {
      const { parent, page } = requestRange(input);
      if (hold && parent === "parent/child" && page === 1) {
        started.resolve();
        return held.promise;
      }
      const children =
        parent === ""
          ? page === 0
            ? ["parent"]
            : ["other"]
          : parent === "parent"
            ? ["parent/child"]
            : ["parent/child/leaf"];
      return Promise.resolve(folderWindow(parent, page, count, children, 120));
    });
    let stale: Promise<void> | undefined;
    let refresh: Promise<void> | undefined;
    let refreshed = false;
    let settled = false;
    try {
      await h.controller.load("", 0);
      await h.controller.toggle("parent", false);
      await h.controller.toggle("parent/child", false);
      hold = true;
      stale = h.controller.page("parent/child", 1).then(() => {
        settled = true;
      });
      await started.promise;
      if (hide === "collapse") await h.controller.toggle("parent", true);
      else await h.controller.page("", 1);
      count = 9;
      refresh = h.controller.refreshCounts().then(() => {
        refreshed = true;
      });
      await afterTransport();
      expect(refreshed).toBe(true);
      expect(settled).toBe(true);
      expect(h.controller.owner.window("parent/child")).toBeUndefined();
      expect(h.controller.owner.isExpanded("parent/child")).toBe(false);
      held.resolve(
        folderWindow("parent/child", 1, 1, ["parent/child/old"], 120),
      );
      await stale;
      await afterTransport();
      expect(h.controller.owner.window("parent/child")).toBeUndefined();
      expect(h.controller.owner.isExpanded("parent/child")).toBe(false);
    } finally {
      held.resolve(
        folderWindow("parent/child", 1, 1, ["parent/child/old"], 120),
      );
      await Promise.all([stale, refresh]);
      h.dispose();
    }
  }
});

test("count refresh preserves the visible page requested while page zero is retained", async () => {
  const held = deferred<Response>();
  const started = deferred<void>();
  let refreshing = false;
  const h = harness((input) => {
    const { parent, page } = requestRange(input);
    if (parent === "child" && page === 1 && !refreshing) {
      started.resolve();
      return held.promise;
    }
    const locations = parent ? [`child/page${page}`] : ["child"];
    return Promise.resolve(
      folderWindow(parent, page, refreshing ? 9 : 1, locations, 120),
    );
  });
  let stale: Promise<void> | undefined;
  let refresh: Promise<void> | undefined;
  let refreshed = false;
  let settled = false;
  try {
    await h.controller.load("", 0);
    await h.controller.toggle("child", false);
    stale = h.controller.page("child", 1).then(() => {
      settled = true;
    });
    await started.promise;
    expect(h.controller.owner.window("child")?.page).toBe(0);
    refreshing = true;
    refresh = h.controller.refreshCounts().then(() => {
      refreshed = true;
    });
    await afterTransport();
    expect(refreshed).toBe(true);
    expect(settled).toBe(true);
    expect(h.controller.tree(() => false).rootChildren[0]).toMatchObject({
      expanded: true,
      pager: { page: 1 },
      children: [{ location: "child/page1", photoCount: 9 }],
    });
    held.resolve(folderWindow("child", 1, 1, ["child/old"], 120));
    await stale;
    await afterTransport();
    expect(h.controller.owner.window("child")).toMatchObject({
      page: 1,
      children: [{ location: "child/page1", photoCount: 9 }],
    });
  } finally {
    held.resolve(folderWindow("child", 1, 1, ["child/old"], 120));
    await Promise.all([stale, refresh]);
    h.dispose();
  }
});

test("count refresh detaches unbound root joiners and keeps expansion on one fresh binding", async () => {
  const old = deferred<Response>();
  const fresh = deferred<Response>();
  const oldStarted = deferred<void>();
  let refreshed = false;
  let requests = 0;
  const h = harness(() => {
    requests += 1;
    if (requests === 1) {
      oldStarted.resolve();
      return old.promise;
    }
    return fresh.promise;
  });
  let expansion: Promise<void> | undefined;
  let joined: Promise<void> | undefined;
  let binding: Promise<boolean> | undefined;
  let refresh: Promise<void> | undefined;
  let freshJoin: Promise<boolean> | undefined;
  let freshJoined: boolean | undefined;
  let joinedSettled = false;
  let bindingResult: boolean | undefined;
  try {
    expansion = h.controller.toggle("", false);
    joined = h.controller.load("", 0, false).then(() => {
      joinedSettled = true;
    });
    binding = h.controller.awaitRootBinding().then((result) => {
      bindingResult = result;
      return result;
    });
    await oldStarted.promise;
    refresh = h.controller.refreshCounts().then(() => {
      refreshed = true;
    });
    await afterTransport();
    expect(requests).toBe(2);
    expect(h.controller.tree(() => false).rootLoading).toBe(true);
    freshJoin = h.controller.awaitRootBinding().then((result) => {
      freshJoined = result;
      return result;
    });
    fresh.resolve(folderWindow("", 0, 9, ["child"]));
    await afterTransport();
    expect(refreshed).toBe(true);
    expect(joinedSettled).toBe(true);
    expect(bindingResult).toBe(false);
    expect(freshJoined).toBe(true);
    expect(requests).toBe(2);
    expect(h.controller.tree(() => false)).toMatchObject({
      rootExpanded: true,
      rootLoading: false,
      rootChildren: [{ location: "child", photoCount: 9 }],
    });
    old.resolve(folderWindow("", 0, 1, ["old"]));
    await Promise.all([expansion, joined, binding]);
    await afterTransport();
    expect(h.controller.tree(() => false).rootChildren).toMatchObject([
      { location: "child", photoCount: 9 },
    ]);
  } finally {
    old.resolve(folderWindow("", 0, 1, ["old"]));
    fresh.resolve(folderWindow("", 0, 9, ["child"]));
    await Promise.all([expansion, joined, binding, refresh, freshJoin]);
    h.dispose();
  }
});

test("late failures and publication conflicts cannot undo a fresh count refresh", async () => {
  for (const status of [503, 409]) {
    const old = deferred<Response>();
    const started = deferred<void>();
    let childRequests = 0;
    let count = 1;
    let overviewRequests = 0;
    const h = harness(
      (input) => {
        const { parent, page } = requestRange(input);
        if (parent === "child" && ++childRequests === 1) {
          started.resolve();
          return old.promise;
        }
        return Promise.resolve(
          folderWindow(
            parent,
            page,
            count,
            parent ? ["child/leaf"] : ["child"],
          ),
        );
      },
      () => {
        overviewRequests += 1;
        return Promise.resolve(overviewResponse());
      },
    );
    let stale: Promise<void> | undefined;
    let firstRefresh: Promise<void> | undefined;
    let firstRefreshed = false;
    try {
      await h.controller.load("", 0);
      stale = h.controller.toggle("child", false);
      await started.promise;
      count = 9;
      firstRefresh = h.controller.refreshCounts().then(() => {
        firstRefreshed = true;
      });
      await afterTransport();
      expect(firstRefreshed).toBe(true);
      count = 12;
      await h.controller.refreshCounts();
      old.resolve(new Response(null, { status }));
      await stale;
      await afterTransport();
      expect(overviewRequests).toBe(0);
      expect(h.controller.publication).toBe("p1");
      expect(h.controller.owner.failures()).toEqual([]);
      expect(h.gate.decisionReady).toBe(true);
      expect(h.controller.tree(() => false).rootChildren[0]).toMatchObject({
        photoCount: 12,
        expanded: true,
        loading: false,
        children: [{ location: "child/leaf", photoCount: 12 }],
      });
    } finally {
      old.resolve(new Response(null, { status }));
      await Promise.all([stale, firstRefresh]);
      h.dispose();
    }
  }
});

test("unrelated count refresh keeps exact failed ranges and their recovery claims", async () => {
  let fail = true;
  const h = harness((input) => {
    const { parent, page } = requestRange(input);
    if (fail && ((parent === "a" && page === 1) || parent === "b"))
      return Promise.resolve(new Response(null, { status: 503 }));
    const locations = parent ? [`${parent}/leaf`] : ["a", "b"];
    return Promise.resolve(folderWindow(parent, page, 1, locations, 120));
  });
  try {
    await h.controller.load("", 0);
    await h.controller.toggle("a", false);
    await h.controller.page("a", 1);
    await h.controller.toggle("b", false);
    const [aFailure, bFailure] = h.controller.owner.failures();
    if (!aFailure || !bFailure) throw new Error("expected both failed ranges");
    expect(aFailure).toMatchObject({ parent: "a", page: 1 });
    expect(bFailure).toMatchObject({ parent: "b", page: 0 });
    expect(h.gate.decisionReady).toBe(false);
    fail = false;
    await h.controller.refreshCounts();
    expect(h.controller.owner.window("a")?.page).toBe(0);
    expect(h.controller.owner.failures()).toEqual([aFailure, bFailure]);
    expect(h.controller.owner.failures()[0]).toBe(aFailure);
    expect(h.controller.owner.failures()[1]).toBe(bFailure);
    expect(h.gate.decisionReady).toBe(false);
    await h.controller.retry(aFailure);
    expect(h.controller.owner.failures()).toEqual([bFailure]);
    expect(h.gate.decisionReady).toBe(false);
    await h.controller.retry(bFailure);
    expect(h.controller.owner.failures()).toEqual([]);
    expect(h.gate.decisionReady).toBe(true);
  } finally {
    h.dispose();
  }
});

test("a Folder can collapse during retry without its response reopening it", async () => {
  const held = deferred<Response>();
  const started = deferred<void>();
  let fail = false;
  let retrying = false;
  const h = harness((input) => {
    const { parent, page } = requestRange(input);
    if (parent === "child" && retrying && page === 1) {
      started.resolve();
      return held.promise;
    }
    if (parent === "child" && fail)
      return Promise.resolve(new Response(null, { status: 503 }));
    return Promise.resolve(
      folderWindow(parent, page, 1, parent ? ["child/leaf"] : ["child"], 120),
    );
  });
  let retry: Promise<void> | undefined;
  let collapse: Promise<void> | undefined;
  try {
    await h.controller.load("", 0);
    await h.controller.toggle("child", false);
    fail = true;
    await h.controller.load("child", 0);
    await h.controller.load("child", 1);
    const [first, second] = h.controller.owner.failures();
    if (!first || !second) throw new Error("expected both failed ranges");
    expect(first?.page).toBe(0);
    expect(second?.page).toBe(1);
    retrying = true;
    retry = h.controller.retry(second);
    await started.promise;
    expect(h.controller.tree(() => false).failures).toMatchObject([
      { parent: "child", loading: false },
      { parent: "child", loading: true },
    ]);
    collapse = h.controller.toggle("child", true);
    expect(h.controller.owner.isExpanded("child")).toBe(false);
    expect(h.controller.tree(() => false).rootChildren[0]?.expanded).toBe(
      false,
    );
    held.resolve(folderWindow("child", 1, 9, ["child/leaf"], 120));
    await Promise.all([retry, collapse]);
    expect(h.controller.owner.isExpanded("child")).toBe(false);
    expect(h.controller.tree(() => false).rootChildren[0]).toMatchObject({
      expanded: false,
      loading: false,
      children: [],
    });
    expect(h.controller.owner.failures()).toEqual([first]);
    expect(h.gate.decisionReady).toBe(false);
  } finally {
    held.resolve(folderWindow("child", 1, 9, ["child/leaf"], 120));
    await Promise.all([retry, collapse]);
    h.dispose();
  }
});

test("count refresh transfers a pending retry and its failure loading state", async () => {
  const old = deferred<Response>();
  const fresh = deferred<Response>();
  const oldStarted = deferred<void>();
  let requests = 0;
  const h = harness((input) => {
    const { parent, page } = requestRange(input);
    if (!parent) return Promise.resolve(folderWindow("", page, 9, ["child"]));
    requests += 1;
    if (requests === 1)
      return Promise.resolve(new Response(null, { status: 503 }));
    if (requests === 2) {
      oldStarted.resolve();
      return old.promise;
    }
    return fresh.promise;
  });
  let retry: Promise<void> | undefined;
  let refresh: Promise<void> | undefined;
  let refreshed = false;
  try {
    await h.controller.load("", 0);
    await h.controller.toggle("child", false);
    const failure = h.controller.owner.failures()[0]!;
    retry = h.controller.retry(failure);
    await oldStarted.promise;
    refresh = h.controller.refreshCounts().then(() => {
      refreshed = true;
    });
    await afterTransport();
    expect(requests).toBe(3);
    expect(h.controller.owner.failures()[0]).toBe(failure);
    expect(h.controller.tree(() => false).failures).toMatchObject([
      { parent: "child", loading: true },
    ]);
    expect(h.gate.decisionReady).toBe(false);
    fresh.resolve(folderWindow("child", 0, 9, ["child/leaf"]));
    await afterTransport();
    expect(refreshed).toBe(true);
    expect(h.controller.owner.failures()).toEqual([]);
    expect(h.gate.decisionReady).toBe(true);
    expect(h.controller.tree(() => false).rootChildren[0]).toMatchObject({
      expanded: true,
      loading: false,
      children: [{ location: "child/leaf", photoCount: 9 }],
    });
    old.resolve(folderWindow("child", 0, 1, ["child/old"]));
    await retry;
    await afterTransport();
    expect(h.controller.owner.window("child")?.children[0]).toMatchObject({
      location: "child/leaf",
      photoCount: 9,
    });
  } finally {
    old.resolve(folderWindow("child", 0, 1, ["child/old"]));
    fresh.resolve(folderWindow("child", 0, 9, ["child/leaf"]));
    await Promise.all([retry, refresh]);
    h.dispose();
  }
});
