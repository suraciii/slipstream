import { describe, expect, test } from "bun:test";
import { RecoveryGate } from "./async-ownership.js";
import {
  createGridPhotoNavigation,
  type GridPhotoNavigation,
} from "./grid-photo-navigation.js";
import type { NavigationDestination } from "./browser-navigation.js";
import type { PhotoAuthority, PhotoOperation } from "./photo-owner.js";
import type { SourceAuthority } from "./source-grid-owner.js";
import type { PhotoSummary } from "../api/contracts.js";

const deferred = <T>() => Promise.withResolvers<T>();

const photoSummary = (id: string): PhotoSummary => ({
  id,
  available: true,
  original: { kind: "jpeg", available: true },
  selectionState: "unflagged",
  rating: 0,
  hasSavedEdits: false,
  preview: { state: "ready" },
});

/// Runs one draw with a stubbed cryptographic source. The stub answers with the
/// requested 53-bit word, so the test owns the sampled position instead of
/// asserting against the helper's own domain rule.
const withRandomDraw = async (
  value: number,
  run: () => void | Promise<void>,
): Promise<void> => {
  const original = crypto.getRandomValues.bind(crypto);
  const words = new Uint32Array(2);
  words[0] = Math.floor(value / 0x1_0000_0000);
  words[1] = value % 0x1_0000_0000;
  crypto.getRandomValues = ((array: Uint32Array) => {
    array.set(words.subarray(0, array.length));
    return array;
  }) as typeof crypto.getRandomValues;
  try {
    await run();
  } finally {
    crypto.getRandomValues = original;
  }
};

function fixture(
  options: Readonly<{
    total?: number;
    loadWindow?: () => Promise<boolean>;
  }> = {},
) {
  const total = options.total ?? 8;
  const authority = {} as SourceAuthority;
  const photoAuthority = {} as PhotoAuthority;
  const facts = new Map<number, PhotoSummary>();
  for (let index = 0; index < total; index += 1)
    facts.set(index, photoSummary(`photo-${index}`));

  const events: string[] = [];
  const statuses: string[] = [];
  const addresses: Array<{ photoId: string; address: string }> = [];
  const destinations: Array<string | undefined> = [];
  const windowLoads: Array<{
    index: number;
    operation: string;
    reopenExpired: boolean | undefined;
  }> = [];
  const positions: number[] = [];
  const gate = new RecoveryGate();
  const attemptSettled = deferred<void>();
  let sawPending = false;
  let gridVisible = true;
  let sourceCurrent = true;
  let retryRequired = false;
  let open: PhotoOperation | undefined;
  const owner: { current?: GridPhotoNavigation } = {};

  /// Resolves once the one pending attempt has cleared its own guard. The
  /// attempt's last step is the control refresh it runs after `pending` drops,
  /// so the test awaits a real signal rather than a guessed duration.
  const updateControls = () => {
    events.push("updateControls");
    if (!owner.current) return;
    if (owner.current.pending) sawPending = true;
    else if (sawPending) attemptSettled.resolve();
  };

  owner.current = createGridPhotoNavigation({
    applicationAlive: () => true,
    source: {
      authority,
      total,
      isCurrent: (candidate) => sourceCurrent && candidate === authority,
      isReady: (candidate) => sourceCurrent && candidate === authority,
      photoAt: (index) => facts.get(index),
      get retryRequired() {
        return retryRequired;
      },
      stopGridWork: () => events.push("stopGridWork"),
    },
    photo: {
      authority: photoAuthority,
      beginOpen: (index) => {
        events.push("beginOpen");
        open = {
          authority: photoAuthority,
          sourceAuthority: authority,
          windowAuthority:
            photoAuthority as unknown as PhotoOperation["windowAuthority"],
          index,
          ...(facts.get(index) ? { photoId: facts.get(index)!.id } : {}),
        };
        return open;
      },
      commitOpen: (candidate) => {
        if (candidate !== open) return undefined;
        events.push("commitOpen");
        positions.push(candidate.index);
        return facts.get(candidate.index);
      },
      cancelOpen: () => events.push("cancelOpen"),
      isCurrent: (candidate) => candidate === photoAuthority,
    },
    view: {
      captureGridRestoration: () => undefined,
      gridVisible: () => gridVisible,
      enterPhoto: () => events.push("enterPhoto"),
    },
    navigationSession: {
      begin: (destination: NavigationDestination) => {
        events.push("sessionBegin");
        return { destination };
      },
      isCurrent: () => true,
      captureGrid: () => events.push("captureGrid"),
    },
    recoveryGate: gate,
    photoRecoveryKey: () => "1",
    canOpenGridPhoto: () => sourceCurrent,
    loadWindow: async (
      index,
      operation,
      _quiet,
      _priority,
      _transition,
      _photo,
      reopen,
    ) => {
      windowLoads.push({
        index,
        operation: operation?.kind ?? "grid",
        reopenExpired: reopen,
      });
      events.push("loadWindow");
      if (options.loadWindow) return options.loadWindow();
      return index < total;
    },
    destinationFor: (photoId) => {
      destinations.push(photoId);
      return {
        source: "library",
        selection: "all",
        ...(photoId ? { photoId } : {}),
      };
    },
    recordPhotoAddress: (photoId, address) => {
      addresses.push({ photoId, address });
    },
    renderPhotoShell: () => {
      events.push("renderPhotoShell");
      return false;
    },
    showPreview: () => {
      events.push("showPreview");
      return Promise.resolve(true);
    },
    persistPosition: () => {
      events.push("persistPosition");
      return Promise.resolve(true);
    },
    syncConnection: () => events.push("syncConnection"),
    updateControls,
    setGridStatus: (message) => statuses.push(message),
  });
  const navigation = owner.current;

  return {
    navigation,
    events,
    statuses,
    addresses,
    destinations,
    windowLoads,
    positions,
    attemptSettled: attemptSettled.promise,
    setGridVisible: (value: boolean) => {
      gridVisible = value;
    },
    setSourceCurrent: (value: boolean) => {
      sourceCurrent = value;
    },
    setRetryRequired: (value: boolean) => {
      retryRequired = value;
    },
  };
}

describe("grid Photo navigation", () => {
  test("opens the ordinary Photo destination for a Grid activation", async () => {
    const f = fixture();
    expect(await f.navigation.openPhoto(3, "push")).toBe(true);
    expect(f.positions).toEqual([3]);
    expect(f.addresses).toEqual([{ photoId: "photo-3", address: "push" }]);
    expect(f.events).toContain("enterPhoto");
    expect(f.events).toContain("renderPhotoShell");
    expect(f.events).toContain("persistPosition");
  });

  test("samples the complete Snapshot, not only loaded facts", async () => {
    // Position 7 is deliberately absent from the loaded facts: the draw must
    // still target it through one bounded window read that never reopens an
    // expired Snapshot on its own.
    const f = fixture();
    await withRandomDraw(7, async () => {
      f.navigation.openRandomPhoto();
      await f.attemptSettled;
    });
    expect(f.windowLoads).toEqual([
      { index: 7, operation: "grid", reopenExpired: false },
    ]);
    expect(f.destinations).toEqual(["photo-7"]);
    expect(f.positions).toEqual([7]);
    expect(f.addresses).toEqual([{ photoId: "photo-7", address: "push" }]);
    expect(f.navigation.pending).toBe(false);
  });

  test("admits at most one pending attempt and refuses ordinary opens while it runs", async () => {
    const windowRead = deferred<boolean>();
    const f = fixture({ loadWindow: () => windowRead.promise });
    await withRandomDraw(2, async () => {
      f.navigation.openRandomPhoto();
      expect(f.navigation.pending).toBe(true);
      f.navigation.openRandomPhoto();
      expect(f.windowLoads).toHaveLength(1);
      // The Grid cannot start an ordinary open while the attempt resolves.
      expect(await f.navigation.openPhoto(0)).toBe(false);
      windowRead.resolve(true);
      await f.attemptSettled;
    });
    expect(f.positions).toEqual([2]);
    expect(f.navigation.pending).toBe(false);
  });

  test("keeps the Grid and names Refresh Current Source when the Snapshot expired", async () => {
    const f = fixture({ loadWindow: () => Promise.resolve(false) });
    f.setRetryRequired(true);
    await withRandomDraw(1, async () => {
      f.navigation.openRandomPhoto();
      await f.attemptSettled;
    });
    expect(f.addresses).toEqual([]);
    expect(f.positions).toEqual([]);
    expect(f.events).not.toContain("enterPhoto");
    expect(f.statuses).toEqual([
      "Library order expired. Refresh Current Source to continue.",
    ]);
  });

  test("creates no Photo history entry when the bounded read failed", async () => {
    const f = fixture({ loadWindow: () => Promise.resolve(false) });
    await withRandomDraw(1, async () => {
      f.navigation.openRandomPhoto();
      await f.attemptSettled;
    });
    expect(f.addresses).toEqual([]);
    expect(f.statuses).toEqual([]);
    expect(f.navigation.pending).toBe(false);
  });

  test("abandons a target whose Snapshot was replaced mid-read", async () => {
    const windowRead = deferred<boolean>();
    const f = fixture({ loadWindow: () => windowRead.promise });
    await withRandomDraw(5, async () => {
      f.navigation.openRandomPhoto();
      f.setSourceCurrent(false);
      windowRead.resolve(true);
      await f.attemptSettled;
    });
    expect(f.positions).toEqual([]);
    expect(f.addresses).toEqual([]);
    expect(f.events).not.toContain("enterPhoto");
    expect(f.navigation.pending).toBe(false);
  });

  test("does not start when the current Snapshot is not ready", async () => {
    const f = fixture();
    f.setSourceCurrent(false);
    await withRandomDraw(5, () => f.navigation.openRandomPhoto());
    expect(f.windowLoads).toEqual([]);
    expect(f.navigation.pending).toBe(false);
  });

  test("does not start from Photo View or an empty Snapshot", async () => {
    const inPhoto = fixture();
    inPhoto.setGridVisible(false);
    await withRandomDraw(0, () => inPhoto.navigation.openRandomPhoto());
    expect(inPhoto.windowLoads).toEqual([]);
    expect(inPhoto.navigation.pending).toBe(false);

    const empty = fixture({ total: 0 });
    await withRandomDraw(0, () => empty.navigation.openRandomPhoto());
    expect(empty.windowLoads).toEqual([]);
    expect(empty.navigation.pending).toBe(false);
  });

  test("stops admitting work once disposed", async () => {
    const f = fixture();
    f.navigation.dispose();
    expect(await f.navigation.openPhoto(0)).toBe(false);
    await withRandomDraw(0, () => f.navigation.openRandomPhoto());
    expect(f.windowLoads).toEqual([]);
  });
});
