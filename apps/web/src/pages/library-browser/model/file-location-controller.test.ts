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
