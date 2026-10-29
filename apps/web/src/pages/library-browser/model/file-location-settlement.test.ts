import { expect, test } from "bun:test";
import {
  createApplicationOwner,
  type ApplicationEvent,
} from "./application-owner.js";
import { RecoveryGate } from "./async-ownership.js";
import { createFileLocationController } from "./file-location-controller.js";

const folderWindow = (publication: string, parent: string) =>
  Response.json({
    publication,
    parent,
    start: 0,
    limit: 60,
    total: 1,
    children: [
      {
        location: parent ? `${parent}/leaf` : "child",
        name: parent ? "leaf" : "child",
        photoCount: 1,
        hasDescendantFolders: !parent,
      },
    ],
  });

const overview = () =>
  Response.json({
    published: true,
    publication: "p2",
    photoCount: 1,
    scan: { state: "idle", publication: "p2" },
    albums: [],
  });

function harness(
  fetcher: (input: string) => Promise<Response>,
  refresh = () => Promise.resolve(overview()),
) {
  const events: ApplicationEvent[] = [];
  const gate = new RecoveryGate();
  const application = createApplicationOwner(refresh, {
    emit: (event) => {
      events.push(event);
    },
    schedule: () => () => {},
  });
  const controller = createFileLocationController({
    fetcher,
    application,
    recoveryGate: gate,
    isAlive: () => true,
    onChanged: () => {},
    onReachable: () => {},
  });
  return {
    controller,
    gate,
    events,
    dispose: () => {
      controller.dispose();
      application.dispose();
      gate.close();
    },
  };
}

test("refresh at the recovery settlement boundary does not strand a recovery claim", async () => {
  const held = Promise.withResolvers<Response>();
  const started = Promise.withResolvers<void>();
  const boundary = Promise.withResolvers<{
    observed: boolean;
    blocked: boolean;
  }>();
  let childRequests = 0;
  const h = harness((input) => {
    const parent =
      new URL(input, "http://slipstream.test").searchParams.get("parent") ?? "";
    if (!parent) return Promise.resolve(folderWindow("p1", ""));
    childRequests += 1;
    if (childRequests === 1)
      return Promise.resolve(new Response(null, { status: 503 }));
    if (childRequests === 2) {
      started.resolve();
      return held.promise;
    }
    return Promise.resolve(folderWindow("p1", parent));
  });
  let retry: Promise<void> | undefined;
  let refresh: Promise<void> | undefined;
  let probing = true;
  try {
    await h.controller.load("", 0);
    await h.controller.toggle("child", false);
    const failure = h.controller.owner.failures()[0]!;
    expect(failure.parent).toBe("child");
    expect(h.gate.decisionReady).toBe(false);
    retry = h.controller.retry(failure);
    await started.promise;
    let turns = 0;
    const probe = () => {
      if (!probing) return;
      if (!h.controller.owner.failures().includes(failure)) {
        const blocked = !h.gate.decisionReady;
        refresh = h.controller.refreshCounts();
        boundary.resolve({ observed: true, blocked });
      } else if (++turns === 10000) {
        boundary.resolve({ observed: false, blocked: false });
      } else {
        queueMicrotask(probe);
      }
    };
    queueMicrotask(probe);
    held.resolve(folderWindow("p1", "child"));
    const reached = await boundary.promise;
    expect(reached).toEqual({ observed: true, blocked: true });
    await Promise.all([retry, refresh]);
    expect(h.controller.owner.failures()).toEqual([]);
    expect(h.gate.decisionReady).toBe(true);
    expect(h.controller.tree(() => false).rootChildren[0]).toMatchObject({
      expanded: true,
      children: [{ location: "child/leaf" }],
    });
  } finally {
    probing = false;
    held.resolve(folderWindow("p1", "child"));
    await Promise.all([retry, refresh]);
    h.dispose();
  }
});

test("count refresh does not retry a failed initial root binding", async () => {
  let requests = 0;
  const h = harness(() => {
    requests += 1;
    return Promise.resolve(new Response(null, { status: 503 }));
  });
  try {
    await h.controller.toggle("", false);
    const failure = h.controller.owner.failures()[0]!;
    expect(failure).toMatchObject({ parent: "", page: 0 });
    expect(h.gate.decisionReady).toBe(false);
    await h.controller.refreshCounts();
    expect(requests).toBe(1);
    expect(h.controller.publication).toBeUndefined();
    expect(h.controller.owner.failures()).toEqual([failure]);
    expect(h.controller.owner.failures()[0]).toBe(failure);
    expect(h.controller.tree(() => false)).toMatchObject({
      rootExpanded: false,
      rootLoading: false,
      rootChildren: [],
    });
    expect(h.gate.decisionReady).toBe(false);
  } finally {
    h.dispose();
  }
});

test("count refresh during publication rebind preserves the reload notice", async () => {
  const heldOverview = Promise.withResolvers<Response>();
  const entered = Promise.withResolvers<void>();
  let rootRequests = 0;
  const h = harness(
    (input) => {
      const parent =
        new URL(input, "http://slipstream.test").searchParams.get("parent") ??
        "";
      if (parent) return Promise.resolve(new Response(null, { status: 409 }));
      rootRequests += 1;
      return Promise.resolve(
        folderWindow(rootRequests === 1 ? "p1" : "p2", ""),
      );
    },
    () => {
      entered.resolve();
      return heldOverview.promise;
    },
  );
  let conflict: Promise<void> | undefined;
  try {
    await h.controller.load("", 0);
    conflict = h.controller.toggle("child", false);
    await entered.promise;
    await h.controller.refreshCounts();
    expect(rootRequests).toBe(1);
    heldOverview.resolve(overview());
    await conflict;
    expect(h.controller.publication).toBe("p2");
    expect(rootRequests).toBe(2);
    expect(h.controller.tree(() => false)).toMatchObject({
      rootExpanded: true,
      rootChildren: [{ location: "child" }],
    });
    expect(
      h.events.some(
        (event) =>
          event.kind === "summary" &&
          event.summary.text === "Library changed. Reloaded folders.",
      ),
    ).toBe(true);
    expect(h.gate.decisionReady).toBe(false);
  } finally {
    heldOverview.resolve(overview());
    await conflict;
    h.dispose();
  }
});
