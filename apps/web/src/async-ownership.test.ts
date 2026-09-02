import { describe, expect, test } from "bun:test";
import {
  RecoveryGate,
  SettlementFamily,
  SummaryNoticeChannel,
  TaskScope,
} from "./async-ownership";

describe("TaskScope", () => {
  test("latest work aborts or detaches its predecessor and cleans exactly once", () => {
    const scope = new TaskScope();
    const cleaned: string[] = [];
    const first = scope.beginLatest("photo", { abortTransport: true });
    first.onCleanup(() => cleaned.push("first"));
    const second = scope.beginLatest("photo", { abortTransport: true });
    expect(first.signal?.aborted).toBe(true);
    expect(first.isCurrent()).toBe(false);
    expect(second.isCurrent()).toBe(true);
    first.finish();
    expect(cleaned).toEqual(["first"]);

    const detached = scope.beginLatest("folder", { abortTransport: false });
    detached.onCleanup(() => cleaned.push("detached"));
    const replacement = scope.beginLatest("folder", {
      abortTransport: false,
    });
    expect(detached.signal).toBeUndefined();
    expect(detached.isCurrent()).toBe(false);
    expect(replacement.isCurrent()).toBe(true);
    expect(cleaned).toEqual(["first", "detached"]);

    scope.halt();
    expect(second.signal?.aborted).toBe(true);
    expect(replacement.isCurrent()).toBe(false);
    expect(() => scope.beginLatest("late", { abortTransport: true })).toThrow();
  });

  test("coalesced callers join one keyed operation and cleanup after shared settlement", async () => {
    const scope = new TaskScope();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    let starts = 0;
    let cleanups = 0;
    const first = scope.joinOrStart(
      "root-binding",
      { abortTransport: false, cleanup: () => (cleanups += 1) },
      async () => {
        starts += 1;
        await gate;
        return "bound";
      },
    );
    const second = scope.joinOrStart(
      "root-binding",
      { abortTransport: false },
      () => {
        starts += 1;
        return Promise.resolve("duplicate");
      },
    );
    expect(first.started).toBe(true);
    expect(second.started).toBe(false);
    expect(second.promise).toBe(first.promise);
    release();
    expect(await first.promise).toBe("bound");
    expect(await second.promise).toBe("bound");
    expect(starts).toBe(1);
    expect(cleanups).toBe(1);
    expect(first.isCurrent()).toBe(false);
  });

  test("ordered work keeps older success after newer failure but honors commit order and floor", () => {
    const scope = new TaskScope();
    const older = scope.beginOrdered("overview", 4);
    const newer = scope.beginOrdered("overview", 4);
    // The newer request fails without committing, so the older valid response
    // may still become the committed shared state.
    newer.finish();
    expect(older.commit(4)).toBe(true);
    older.finish();

    const stale = scope.beginOrdered("overview", 4);
    const current = scope.beginOrdered("overview", 4);
    expect(current.commit(4)).toBe(true);
    expect(stale.commit(4)).toBe(false);

    const preMutation = scope.beginOrdered("overview", 4);
    expect(preMutation.commit(5)).toBe(false);
    const postMutation = scope.beginOrdered("overview", 5);
    expect(postMutation.commit(5)).toBe(true);
  });
});

describe("SettlementFamily", () => {
  test("deduplicates admission while preserving global latest settlement ownership", () => {
    const family = new SettlementFamily();
    let firstSurface = true;
    const first = family.begin({
      admissionKey: "add:album:photo",
      ownsSurface: () => firstSurface,
    });
    expect(first).toBeDefined();
    expect(family.isAdmitted("add:album:photo")).toBe(true);
    expect(family.begin({ admissionKey: "add:album:photo" })).toBeUndefined();
    const second = family.begin({ admissionKey: "remove:album:photo" })!;
    expect(first!.isNewest()).toBe(false);
    expect(second.isNewest()).toBe(true);
    expect(first!.canPresent()).toBe(false);
    firstSurface = false;
    expect(first!.ownsSurface()).toBe(false);

    second.finish();
    expect(first!.isNewest()).toBe(false);
    first!.finish();
    expect(family.isAdmitted("add:album:photo")).toBe(false);
    expect(family.begin({ admissionKey: "add:album:photo" })).toBeDefined();
    family.closePresentation();
    expect(family.begin({ admissionKey: "late" })).toBeUndefined();
  });
});

describe("SummaryNoticeChannel", () => {
  test("preserves an older admitted failure behind newer actionable recovery", () => {
    const channel = new SummaryNoticeChannel<string>();
    const album = channel.issue("album", "add", 10);
    const folder = channel.issue("file-location", "RAW:0", 30);
    expect(channel.present(folder, "Retry RAW range").visible).toBe(
      "Retry RAW range",
    );
    expect(
      channel.present(album, "Album add failed", { fallback: true }).result,
    ).toBe("blocked");
    const released = channel.release(folder);
    expect(released.visible).toBe("Album add failed");
    expect(channel.activeHandle).toBe(album);
  });

  test("a newer Album success clears displayed failure but not a failure that settles later", () => {
    const channel = new SummaryNoticeChannel<string>();
    const oldFailure = channel.issue("album", "old-add", 10);
    const displayed = channel.issue("album", "displayed", 10);
    expect(channel.present(displayed, "Displayed failure").result).toBe(
      "applied",
    );
    const newerSuccess = channel.issue("album", "new-success", 10);
    expect(channel.releaseKind("album", newerSuccess).visible).toBeNull();
    expect(
      channel.present(oldFailure, "Old admitted add failed", {
        fallback: true,
      }).visible,
    ).toBe("Old admitted add failed");
  });

  test("reload atomically invalidates active, pending, release, and background work", () => {
    const channel = new SummaryNoticeChannel<string>();
    const active = channel.issue("album", "active", 10);
    const pending = channel.issue("album", "pending", 10);
    const oldBackground = channel.backgroundEpoch();
    channel.present(active, "Active failure", { fallback: true });
    channel.present(pending, "Pending failure", { fallback: true });

    const reload = channel.beginBarrier(
      "reload",
      "explicit",
      30,
      "Loading Library summary…",
    );
    expect(reload.update.visible).toBe("Loading Library summary…");
    expect(channel.release(active).result).toBe("stale");
    expect(channel.present(pending, "Late pending").result).toBe("stale");
    expect(
      channel.presentBackground(oldBackground, "Old scan progress").result,
    ).toBe("stale");

    expect(channel.release(reload.handle).visible).toBeNull();
    const fresh = channel.backgroundEpoch();
    expect(channel.presentBackground(fresh, "Current scan").visible).toBe(
      "Current scan",
    );
  });

  test("silent failures discard their one-shot background epoch", () => {
    const channel = new SummaryNoticeChannel<string>();
    const failed = channel.backgroundEpoch();
    expect(channel.discardBackground(failed)).toBe("applied");
    expect(channel.discardBackground(failed)).toBe("stale");
    expect(channel.presentBackground(failed, "late").result).toBe("stale");
  });

  test("releaseKind requires an authorizer from the same owner kind", () => {
    const channel = new SummaryNoticeChannel<string>();
    const album = channel.issue("album", "failure", 10);
    channel.present(album, "Album failed");
    const folder = channel.issue("file-location", "RAW:0", 30);
    expect(channel.releaseKind("album", folder).result).toBe("stale");
    expect(channel.activeHandle).toBe(album);
  });

  test("each background request gets one monotonic one-shot epoch", () => {
    const channel = new SummaryNoticeChannel<string>();
    const first = channel.backgroundEpoch();
    const second = channel.backgroundEpoch();
    expect(channel.presentBackground(second, "new").visible).toBe("new");
    expect(channel.presentBackground(first, "old").result).toBe("stale");
    expect(channel.presentBackground(second, "repeat").result).toBe("stale");
    const third = channel.backgroundEpoch();
    expect(channel.presentBackground(third, "newest").visible).toBe("newest");
  });
});

describe("RecoveryGate", () => {
  test("unrelated reachability cannot release a current Photo recovery claim", () => {
    const gate = new RecoveryGate();
    gate.setOwner("photo", "photo-1");
    const photo = gate.issue("preview", "photo-1", {
      owner: { scope: "photo", generation: "photo-1" },
    });
    const folder = gate.issue("file-location", "RAW:0");
    expect(gate.fail(photo, { transportLost: true })).toBe(true);
    expect(gate.decisionReady).toBe(false);
    gate.markReachable();
    expect(gate.transportReachable).toBe(true);
    expect(gate.decisionReady).toBe(false);
    expect(gate.recover(folder)).toBe(false);
    expect(gate.recover(photo)).toBe(true);
    expect(gate.decisionReady).toBe(true);
  });

  test("A to B success retires predecessor claims without touching independent ranges", () => {
    const gate = new RecoveryGate();
    gate.setOwner("photo", "A");
    const a = gate.issue("preview", "A", {
      owner: { scope: "photo", generation: "A" },
    });
    const folder = gate.issue("file-location", "RAW:60");
    gate.fail(a, { transportLost: true });
    gate.fail(folder);

    const transition = gate.beginTransition("photo", "B");
    expect(gate.recover(a)).toBe(false);
    expect(gate.succeedTransition(transition)).toBe(true);
    expect(gate.isActive(a)).toBe(false);
    expect(gate.isActive(folder)).toBe(true);
    expect(gate.decisionReady).toBe(false);
    expect(gate.recover(folder)).toBe(true);
    expect(gate.decisionReady).toBe(true);
  });

  test("B failure replaces A predecessor and stale A never reactivates", () => {
    const gate = new RecoveryGate();
    gate.setOwner("source", "A");
    const a = gate.issue("source", "A", {
      owner: { scope: "source", generation: "A" },
    });
    gate.fail(a, { transportLost: true });
    const transition = gate.beginTransition("source", "B");
    const b = gate.issue("source", "B", {
      owner: { scope: "source", generation: "B" },
      transition,
    });
    expect(gate.failTransition(transition, b, { transportLost: true })).toBe(
      true,
    );
    expect(gate.isActive(a)).toBe(false);
    expect(gate.isActive(b)).toBe(true);
    expect(gate.fail(a, { transportLost: true })).toBe(false);
    expect(gate.recover(a)).toBe(false);
    expect(gate.recover(b)).toBe(true);
    expect(gate.decisionReady).toBe(true);
  });

  test("a recovered claim is consumed and cannot be reactivated", () => {
    const gate = new RecoveryGate();
    gate.setOwner("photo", "A");
    const claim = gate.issue("preview", "A", {
      owner: { scope: "photo", generation: "A" },
    });
    expect(gate.fail(claim, { transportLost: true })).toBe(true);
    expect(gate.recover(claim)).toBe(true);
    expect(gate.fail(claim, { transportLost: true })).toBe(false);
    expect(gate.isActive(claim)).toBe(false);
  });

  test("transition identity is gate-local, atomic, and one-shot", () => {
    const gate = new RecoveryGate();
    gate.setOwner("source", "A");
    const a = gate.issue("source", "A", {
      owner: { scope: "source", generation: "A" },
    });
    gate.fail(a, { transportLost: true });
    const transition = gate.beginTransition("source", "B");
    const forged = Object.freeze({
      ticket: transition.ticket,
      scope: "source" as const,
      next: "B",
    });
    expect(gate.succeedTransition(forged)).toBe(false);
    expect(gate.isActive(a)).toBe(true);

    const other = new RecoveryGate();
    other.setOwner("source", "B");
    const foreign = other.issue("source", "B", {
      owner: { scope: "source", generation: "B" },
    });
    expect(gate.failTransition(transition, foreign)).toBe(false);
    expect(gate.isActive(a)).toBe(true);

    const replacement = gate.issue("source", "B", {
      owner: { scope: "source", generation: "B" },
      transition,
    });
    expect(gate.failTransition(transition, replacement)).toBe(true);
    expect(gate.failTransition(transition, replacement)).toBe(false);
    expect(gate.succeedTransition(transition)).toBe(false);
    expect(gate.isActive(replacement)).toBe(true);
  });

  test("rapid A to B to C retires the full predecessor lineage but keeps unrelated C claims", () => {
    const gate = new RecoveryGate();
    gate.setOwner("photo", "A");
    const a = gate.issue("preview", "A", {
      owner: { scope: "photo", generation: "A" },
    });
    gate.fail(a, { transportLost: true });

    const toB = gate.beginTransition("photo", "B");
    const bEstablishment = gate.issue("window", "B", {
      owner: { scope: "photo", generation: "B" },
      transition: toB,
    });
    gate.fail(bEstablishment);

    const toC = gate.beginTransition("photo", "C");
    const cEstablishment = gate.issue("window", "C", {
      owner: { scope: "photo", generation: "C" },
      transition: toC,
    });
    gate.fail(cEstablishment);
    const currentWrite = gate.issue("photo-write", "C", {
      owner: { scope: "photo", generation: "C" },
    });
    gate.fail(currentWrite);

    expect(gate.succeedTransition(toC)).toBe(true);
    expect(gate.isActive(a)).toBe(false);
    expect(gate.isActive(bEstablishment)).toBe(false);
    expect(gate.isActive(cEstablishment)).toBe(false);
    expect(gate.isActive(currentWrite)).toBe(true);
    expect(gate.succeedTransition(toB)).toBe(false);
    expect(gate.recover(currentWrite)).toBe(true);
    expect(gate.decisionReady).toBe(true);
  });
});
