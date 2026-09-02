import { describe, expect, test } from "bun:test";
import type { LibraryOverviewResponse } from "../api/contracts.js";
import {
  createApplicationOwner,
  type ApplicationEvent,
  type ApplicationSchedule,
  type ApplicationSummaryAction,
} from "./application-owner.js";

type Deferred<T> = Readonly<{
  promise: Promise<T>;
  resolve(value: T): void;
  reject(reason?: unknown): void;
}>;

type Scheduled = {
  readonly delayMs: number;
  active: boolean;
  run(): Promise<void>;
};

const deferred = <T>(): Deferred<T> => {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
};

const response = (body: unknown, status = 200): Response =>
  new Response(JSON.stringify(body), { status });

const album = (
  name: string,
  options: { id?: string; hasSavedPosition?: boolean } = {},
) => ({
  id: options.id ?? "album-1",
  name,
  photoCount: 1,
  hasSavedPosition: options.hasSavedPosition ?? false,
});

const overview = (
  publication: string,
  name: string,
  options: {
    scanState?: string;
    hasSavedPosition?: boolean;
  } = {},
): LibraryOverviewResponse => ({
  published: true,
  publication,
  photoCount: 1,
  scan: {
    state: options.scanState ?? "idle",
    publication,
  },
  albums: [
    album(name, {
      ...(options.hasSavedPosition === undefined
        ? {}
        : { hasSavedPosition: options.hasSavedPosition }),
    }),
  ],
});

const scan = (state: string, publication?: string) => ({
  state,
  ...(publication ? { publication } : {}),
});

const flush = async (): Promise<void> => {
  for (let count = 0; count < 12; count += 1) await Promise.resolve();
};

const harness = (
  fetcher: (input: string, init?: RequestInit) => Promise<Response>,
) => {
  const events: ApplicationEvent[] = [];
  const scheduled: Scheduled[] = [];
  const schedule: ApplicationSchedule = (delayMs, callback) => {
    const task: Scheduled = {
      delayMs,
      active: true,
      run: async () => {
        if (!task.active) return;
        task.active = false;
        await callback();
      },
    };
    scheduled.push(task);
    return () => {
      task.active = false;
    };
  };
  const owner = createApplicationOwner(fetcher, {
    emit: (event) => {
      events.push(event);
    },
    schedule,
  });
  const nextScheduled = (): Scheduled => {
    const task = scheduled.find((candidate) => candidate.active);
    if (!task) throw new Error("expected a scheduled Application task");
    return task;
  };
  return { owner, events, scheduled, nextScheduled };
};

const summaryEvents = (events: ReadonlyArray<ApplicationEvent>) =>
  events.filter((event) => event.kind === "summary");

const overviewEvents = (events: ReadonlyArray<ApplicationEvent>) =>
  events.filter((event) => event.kind === "overview");

const latestSummary = (events: ReadonlyArray<ApplicationEvent>) =>
  summaryEvents(events).at(-1)?.summary;

describe("ApplicationOwner", () => {
  test("keeps a newer Overview failure visible when an older success lands", async () => {
    const older = deferred<Response>();
    let overviewRequests = 0;
    const { owner, events } = harness((input) => {
      if (input === "/api/overview") {
        overviewRequests += 1;
        return overviewRequests === 1
          ? older.promise
          : Promise.resolve(new Response(null, { status: 503 }));
      }
      return Promise.resolve(response(scan("idle", "publication-1")));
    });

    const first = owner.loadOverview();
    await Promise.resolve();
    await owner.loadOverview();
    older.resolve(response(overview("publication-1", "Older valid Album")));
    await first;

    expect(owner.albums[0]?.name).toBe("Older valid Album");
    expect(latestSummary(events)?.text).toBe(
      "Could not reach Slipstream. Check the server and retry.",
    );
    expect(
      events.some(
        (event) =>
          event.kind === "fail-application-recovery" &&
          event.slot === "overview-reload",
      ),
    ).toBe(true);
    owner.dispose();
  });

  test("Album persistence fences older Overview when convergence fails", async () => {
    const older = deferred<Response>();
    let overviewRequests = 0;
    const { owner, events } = harness((input) => {
      if (input === "/api/overview") {
        overviewRequests += 1;
        if (overviewRequests === 1)
          return Promise.resolve(response(overview("publication-1", "Seed")));
        if (overviewRequests === 2) return older.promise;
        return Promise.resolve(new Response(null, { status: 503 }));
      }
      return Promise.resolve(response(scan("idle", "publication-1")));
    });

    expect(await owner.refreshOverview()).toBe(true);
    const staleRefresh = owner.refreshOverview();
    await Promise.resolve();
    const mutation = owner.beginAlbumMutation({
      noticeKey: "rename",
      surface: "summary",
      ownsSurface: () => true,
    });
    if (!mutation) throw new Error("expected admitted Album mutation");
    const outcome = await owner.settleAlbumMutation(mutation, {
      kind: "persisted",
    });
    older.resolve(response(overview("publication-1", "Stale Album")));
    expect(await staleRefresh).toBe(false);

    expect(outcome).toMatchObject({
      admitted: true,
      ok: true,
      disconnect: true,
    });
    expect(owner.albums[0]?.name).toBe("Seed");
    expect(
      overviewEvents(events).map((event) => event.albums[0]?.name),
    ).toEqual(["Seed"]);
    owner.dispose();
  });

  test("publication mismatch fences another old response before commit", async () => {
    const validations = [deferred<Response>(), deferred<Response>()];
    let statusRequests = 0;
    const { owner, events } = harness((input) => {
      if (input === "/api/overview")
        return Promise.resolve(response(overview("publication-1", "Stale")));
      const validation = validations[statusRequests];
      statusRequests += 1;
      if (!validation) throw new Error("unexpected status request");
      return validation.promise;
    });

    const observesReplacement = owner.refreshOverview();
    await flush();
    const otherwiseValidOldResponse = owner.refreshOverview();
    await flush();
    validations[0]!.resolve(response(scan("idle", "publication-2")));
    expect(await observesReplacement).toBe(false);
    validations[1]!.resolve(response(scan("idle", "publication-1")));
    expect(await otherwiseValidOldResponse).toBe(false);

    expect(owner.overview).toBeUndefined();
    expect(overviewEvents(events)).toHaveLength(0);
    owner.dispose();
  });

  test("uses idle and active poll cadence while status failures stay silent", async () => {
    let statusRequests = 0;
    const { owner, events, nextScheduled } = harness((input) => {
      if (input === "/api/overview")
        return Promise.resolve(response(overview("publication-1", "Album")));
      statusRequests += 1;
      if (statusRequests === 2) return Promise.reject(new Error("offline"));
      return Promise.resolve(
        response(
          statusRequests === 1
            ? scan("idle", "publication-1")
            : scan("discovering", "publication-1"),
        ),
      );
    });

    await owner.refreshOverview();
    const firstPoll = nextScheduled();
    expect(firstPoll.delayMs).toBe(2_000);
    const beforeFailure = events.length;
    await firstPoll.run();
    expect(events).toHaveLength(beforeFailure);

    const secondPoll = nextScheduled();
    expect(secondPoll.delayMs).toBe(2_000);
    await secondPoll.run();
    expect(latestSummary(events)?.text).toBe("Checking Library Folder…");
    expect(nextScheduled().delayMs).toBe(500);
    owner.dispose();
  });

  test("settles one terminal scan and signs a one-shot refresh intent", async () => {
    let overviewRequests = 0;
    let statusRequests = 0;
    const { owner, events, nextScheduled } = harness((input) => {
      if (input === "/api/overview") {
        overviewRequests += 1;
        return Promise.resolve(
          response(
            overview(
              overviewRequests === 1 ? "publication-1" : "publication-2",
              "Album",
              { scanState: overviewRequests === 1 ? "discovering" : "idle" },
            ),
          ),
        );
      }
      if (input === "/api/scan")
        return Promise.resolve(response(scan("idle", "publication-2")));
      statusRequests += 1;
      if (statusRequests === 1)
        return Promise.resolve(response(scan("discovering", "publication-1")));
      if (statusRequests === 2)
        return Promise.resolve(response(scan("failed", "publication-1")));
      return Promise.resolve(response(scan("idle", "publication-2")));
    });

    await owner.refreshOverview();
    await nextScheduled().run();
    const retry = latestSummary(events)?.action;
    if (!retry) throw new Error("expected retry action");
    expect(retry.kind).toBe("retry-library-check");
    expect(owner.activateSummaryAction(retry)).toBeUndefined();
    await flush();

    expect(owner.overview?.publication).toBe("publication-2");
    expect(
      events.filter((event) => event.kind === "reset-file-locations"),
    ).toHaveLength(1);
    const completion = summaryEvents(events).find(
      (event) => event.summary.action?.kind === "refresh-current-source",
    )?.summary.action;
    if (!completion) throw new Error("expected refresh action");
    expect(owner.activateSummaryAction(completion)).toEqual({
      kind: "refresh-current-source",
    });
    expect(owner.activateSummaryAction(completion)).toBeUndefined();

    await nextScheduled().run();
    expect(
      events.filter((event) => event.kind === "reset-file-locations"),
    ).toHaveLength(1);
    expect(
      summaryEvents(events).filter(
        (event) =>
          event.summary.text ===
          "Library check complete. Open Browse Snapshots remain unchanged.",
      ),
    ).toHaveLength(1);
    owner.dispose();
  });

  test("monitor consumes a held scan command exactly once without publication deduplication", async () => {
    const oldOverview = deferred<Response>();
    const terminalScan = deferred<Response>();
    let overviewRequests = 0;
    let statusRequests = 0;
    let scanRequests = 0;
    const { owner, events, nextScheduled } = harness((input) => {
      if (input === "/api/overview") {
        overviewRequests += 1;
        if (overviewRequests === 1)
          return Promise.resolve(
            response(
              overview("publication-1", "Album", {
                scanState: "discovering",
              }),
            ),
          );
        if (overviewRequests === 2) return oldOverview.promise;
        return Promise.resolve(new Response(null, { status: 503 }));
      }
      if (input === "/api/scan") {
        scanRequests += 1;
        return terminalScan.promise;
      }
      statusRequests += 1;
      if (statusRequests === 1)
        return Promise.resolve(response(scan("discovering", "publication-1")));
      if (statusRequests === 2)
        return Promise.resolve(response(scan("failed", "publication-1")));
      if (statusRequests === 3)
        return Promise.resolve(response(scan("applying")));
      if (statusRequests === 4) return Promise.resolve(response(scan("idle")));
      return Promise.resolve(response(scan("idle", "publication-1")));
    });

    await owner.refreshOverview();
    await nextScheduled().run();
    const retry = latestSummary(events)?.action;
    if (!retry || retry.kind !== "retry-library-check")
      throw new Error("expected retry action");
    const capturedBeforeCompletion = owner.refreshOverview();
    await flush();
    owner.activateSummaryAction(retry);
    expect(scanRequests).toBe(1);

    await nextScheduled().run();
    await nextScheduled().run();
    expect(
      events.filter((event) => event.kind === "reset-file-locations"),
    ).toHaveLength(1);
    expect(
      summaryEvents(events).filter(
        (event) =>
          event.summary.text ===
          "Library check complete. Open Browse Snapshots remain unchanged.",
      ),
    ).toHaveLength(1);
    expect(overviewRequests).toBe(3);

    terminalScan.resolve(response(scan("idle")));
    await flush();
    expect(
      events.filter((event) => event.kind === "reset-file-locations"),
    ).toHaveLength(1);
    expect(
      summaryEvents(events).filter(
        (event) =>
          event.summary.text ===
          "Library check complete. Open Browse Snapshots remain unchanged.",
      ),
    ).toHaveLength(1);
    expect(overviewRequests).toBe(3);

    oldOverview.resolve(response(overview("publication-1", "Stale Album")));
    expect(await capturedBeforeCompletion).toBe(false);
    expect(owner.albums[0]?.name).toBe("Album");
    owner.dispose();
  });

  test("classifies rejected scan HTTP status inside the Application owner", async () => {
    for (const [status, losesTransport] of [
      [409, false],
      [503, true],
    ] as const) {
      let statusRequests = 0;
      const { owner, events, nextScheduled } = harness((input) => {
        if (input === "/api/overview")
          return Promise.resolve(
            response(
              overview("publication-1", "Album", {
                scanState: "discovering",
              }),
            ),
          );
        if (input === "/api/scan")
          return Promise.resolve(new Response(null, { status }));
        statusRequests += 1;
        return Promise.resolve(
          response(
            statusRequests === 1
              ? scan("discovering", "publication-1")
              : scan("failed", "publication-1"),
          ),
        );
      });

      await owner.refreshOverview();
      await nextScheduled().run();
      const retry = latestSummary(events)?.action;
      if (!retry || retry.kind !== "retry-library-check")
        throw new Error("expected retry action");
      owner.activateSummaryAction(retry);
      await flush();

      expect(
        events.some(
          (event) =>
            event.kind === "fail-application-recovery" &&
            event.slot === "scan-command",
        ),
      ).toBe(losesTransport);
      owner.dispose();
    }
  });

  test("keeps the newest Album notice when an older failure settles late", async () => {
    const { owner, events } = harness(() =>
      Promise.reject(new Error("unexpected request")),
    );
    const older = owner.beginAlbumMutation({
      noticeKey: "older",
      surface: "summary",
      ownsSurface: () => true,
    });
    const newer = owner.beginAlbumMutation({
      noticeKey: "newer",
      surface: "summary",
      ownsSurface: () => true,
    });
    if (!older || !newer) throw new Error("expected admitted mutations");

    await owner.settleAlbumMutation(newer, {
      kind: "failed",
      message: "Newer failure",
      transportLost: false,
    });
    await owner.settleAlbumMutation(older, {
      kind: "failed",
      message: "Older failure",
      transportLost: false,
    });

    expect(latestSummary(events)?.text).toBe("Newer failure");
    owner.dispose();
  });

  test("saved-position confirmation advances the Overview data floor", async () => {
    const stale = deferred<Response>();
    let overviewRequests = 0;
    const { owner, events } = harness((input) => {
      if (input === "/api/overview") {
        overviewRequests += 1;
        return overviewRequests === 1
          ? Promise.resolve(response(overview("publication-1", "Album")))
          : stale.promise;
      }
      return Promise.resolve(response(scan("idle", "publication-1")));
    });

    await owner.refreshOverview();
    const capturedBeforeConfirmation = owner.refreshOverview();
    await Promise.resolve();
    owner.confirmSavedPosition("album-1");
    stale.resolve(
      response(overview("publication-1", "Album", { hasSavedPosition: false })),
    );

    expect(await capturedBeforeConfirmation).toBe(false);
    expect(owner.albums[0]?.hasSavedPosition).toBe(true);
    expect(overviewEvents(events).at(-1)?.albums[0]?.hasSavedPosition).toBe(
      true,
    );
    owner.dispose();
  });

  test("dispose silences a held Overview response", async () => {
    const held = deferred<Response>();
    const { owner, events } = harness((input) =>
      input === "/api/overview"
        ? held.promise
        : Promise.resolve(response(scan("idle", "publication-1"))),
    );

    const pending = owner.refreshOverview();
    owner.dispose();
    held.resolve(response(overview("publication-1", "Late")));

    expect(await pending).toBe(false);
    expect(events).toHaveLength(0);
    expect(owner.overview).toBeUndefined();
  });

  test("dispose silences a held status poll and cancels its cadence", async () => {
    const held = deferred<Response>();
    let statusRequests = 0;
    const { owner, events, scheduled, nextScheduled } = harness((input) => {
      if (input === "/api/overview")
        return Promise.resolve(response(overview("publication-1", "Album")));
      statusRequests += 1;
      return statusRequests === 1
        ? Promise.resolve(response(scan("idle", "publication-1")))
        : held.promise;
    });

    await owner.refreshOverview();
    const poll = nextScheduled().run();
    await Promise.resolve();
    const beforeDispose = events.length;
    owner.dispose();
    held.resolve(response(scan("discovering", "publication-1")));
    await poll;

    expect(events).toHaveLength(beforeDispose);
    expect(scheduled.every((task) => !task.active)).toBe(true);
  });

  test("dispose keeps an admitted Album write durable but its late refresh silent", async () => {
    const held = deferred<Response>();
    let overviewRequests = 0;
    const { owner, events } = harness((input) => {
      if (input === "/api/overview") {
        overviewRequests += 1;
        return overviewRequests === 1
          ? Promise.resolve(response(overview("publication-1", "Seed")))
          : held.promise;
      }
      return Promise.resolve(response(scan("idle", "publication-1")));
    });
    await owner.refreshOverview();
    events.splice(0);
    const mutation = owner.beginAlbumMutation({
      noticeKey: "rename",
      surface: "summary",
      ownsSurface: () => true,
    });
    if (!mutation) throw new Error("expected admitted mutation");
    const pending = owner.settleAlbumMutation(mutation, { kind: "persisted" });
    owner.dispose();
    held.resolve(response(overview("publication-1", "Late rename")));

    expect(await pending).toMatchObject({
      admitted: true,
      ok: true,
      presentOnSurface: false,
      disconnect: false,
    });
    expect(owner.albums[0]?.name).toBe("Seed");
    expect(events).toHaveLength(0);
  });

  test("dispose suppresses recovery from a late failed Album refresh", async () => {
    const held = deferred<Response>();
    let overviewRequests = 0;
    const { owner, events } = harness((input) => {
      if (input === "/api/overview") {
        overviewRequests += 1;
        return overviewRequests === 1
          ? Promise.resolve(response(overview("publication-1", "Seed")))
          : held.promise;
      }
      return Promise.resolve(response(scan("idle", "publication-1")));
    });
    await owner.refreshOverview();
    events.splice(0);
    const mutation = owner.beginAlbumMutation({
      noticeKey: "rename",
      surface: "summary",
      ownsSurface: () => true,
    });
    if (!mutation) throw new Error("expected admitted mutation");
    const pending = owner.settleAlbumMutation(mutation, { kind: "persisted" });
    owner.dispose();
    held.reject(new Error("late offline refresh"));

    expect(await pending).toMatchObject({
      admitted: true,
      ok: true,
      presentOnSurface: false,
      disconnect: false,
    });
    expect(events).toHaveLength(0);
  });

  test("disposed public commands are inert and cannot admit new work", async () => {
    let requests = 0;
    const { owner, events, scheduled } = harness(() => {
      requests += 1;
      return Promise.reject(new Error("unexpected request"));
    });
    owner.dispose();
    owner.dispose();

    await owner.loadOverview();
    expect(await owner.refreshOverview()).toBe(false);
    const presentation = owner.claimFileLocation("late", "Late failure");
    owner.presentFileLocation(presentation, "Still late");
    owner.releaseFileLocation(presentation);
    expect(
      owner.beginAlbumMutation({
        noticeKey: "late",
        admissionKey: "late",
        surface: "summary",
        ownsSurface: () => true,
      }),
    ).toBeUndefined();
    expect(owner.isAlbumMutationAdmitted("late")).toBe(false);
    owner.confirmSavedPosition("album-1");
    owner.notePublicationConflict();

    expect(requests).toBe(0);
    expect(events).toHaveLength(0);
    expect(scheduled).toHaveLength(0);
  });

  test("freezes emitted Summary actions against consumer forgery", async () => {
    let statusRequests = 0;
    const { owner, events, nextScheduled } = harness((input) => {
      if (input === "/api/overview")
        return Promise.resolve(
          response(
            overview("publication-1", "Album", { scanState: "discovering" }),
          ),
        );
      statusRequests += 1;
      return Promise.resolve(
        response(
          statusRequests === 1
            ? scan("discovering", "publication-1")
            : scan("failed", "publication-1"),
        ),
      );
    });
    await owner.refreshOverview();
    await nextScheduled().run();
    const emitted = latestSummary(events);
    const issued = emitted?.action;
    if (!emitted || !issued) throw new Error("expected issued Summary action");
    const forged = {
      kind: "refresh-current-source",
    } as ApplicationSummaryAction;

    expect(Object.isFrozen(emitted)).toBe(true);
    expect(() => {
      (emitted as { action?: ApplicationSummaryAction }).action = forged;
    }).toThrow();
    expect(emitted.action).toBe(issued);
    expect(owner.activateSummaryAction(forged)).toBeUndefined();
    owner.dispose();
  });

  test("consumer event failures cannot reclassify committed state or escape dispose", async () => {
    const emittedKinds: ApplicationEvent["kind"][] = [];
    const scheduled: Scheduled[] = [];
    const owner = createApplicationOwner(
      (input) =>
        Promise.resolve(
          input === "/api/overview"
            ? response(overview("publication-1", "Album"))
            : response(scan("idle", "publication-1")),
        ),
      {
        emit: (event) => {
          emittedKinds.push(event.kind);
          if (event.kind === "overview")
            throw new Error("sync consumer failure");
          return Promise.reject(new Error("async consumer failure"));
        },
        schedule: (delayMs, callback) => {
          const task: Scheduled = {
            delayMs,
            active: true,
            run: async () => {
              if (!task.active) return;
              task.active = false;
              await callback();
            },
          };
          scheduled.push(task);
          return () => {
            task.active = false;
          };
        },
      },
    );

    await owner.loadOverview();
    await flush();
    expect(owner.albums[0]?.name).toBe("Album");
    expect(emittedKinds).toContain("bootstrap");
    expect(emittedKinds).not.toContain("fail-application-recovery");
    expect(() => owner.dispose()).not.toThrow();
    await flush();
    expect(scheduled.every((task) => !task.active)).toBe(true);
  });
});
