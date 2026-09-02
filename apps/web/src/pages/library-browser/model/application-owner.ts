import type {
  AlbumSummary,
  LibraryOverviewResponse,
} from "../api/contracts.js";
import {
  fetchLibraryOverview,
  fetchLibraryStatus,
  requestLibraryScan,
  type ApplicationFetch,
} from "../api/application.js";
import {
  SettlementFamily,
  SummaryNoticeChannel,
  TaskScope,
  type NoticeHandle,
  type NoticeUpdate,
} from "./async-ownership.js";

const ALBUM_NOTICE_PRIORITY = 10;
const ACTIONABLE_NOTICE_PRIORITY = 30;

export type ApplicationSchedule = (
  delayMs: number,
  run: () => void | Promise<void>,
) => () => void;

declare const applicationSummaryActionBrand: unique symbol;
export type ApplicationSummaryAction = Readonly<{
  [applicationSummaryActionBrand]: true;
  kind: "retry-library-check" | "refresh-current-source";
}>;

export type ApplicationSummary = Readonly<{
  text: string;
  action?: ApplicationSummaryAction;
}>;

export type ApplicationPresentation =
  | Readonly<{ kind: "summary"; summary: ApplicationSummary }>
  | Readonly<{
      kind: "overview";
      overview: LibraryOverviewResponse;
      albums: ReadonlyArray<AlbumSummary>;
    }>;

declare const applicationRecoveryBrand: unique symbol;
export type ApplicationRecovery = Readonly<{
  [applicationRecoveryBrand]: true;
}>;

export type ApplicationCoordination =
  | Readonly<{
      kind: "bootstrap";
      overview: LibraryOverviewResponse;
      isCurrent(): boolean;
    }>
  | Readonly<{ kind: "reset-file-locations" }>
  | Readonly<{ kind: "load-file-location-root" }>
  | Readonly<{
      kind: "fail-application-recovery";
      recovery: ApplicationRecovery;
      slot: "overview-reload" | "scan-command";
    }>
  | Readonly<{
      kind: "recover";
      recovery: ApplicationRecovery;
    }>
  | Readonly<{ kind: "mark-reachable" }>;

export type ApplicationEvent =
  | ApplicationPresentation
  | ApplicationCoordination;

declare const fileLocationPresentationBrand: unique symbol;
export type FileLocationPresentation = Readonly<{
  [fileLocationPresentationBrand]: true;
}>;

declare const albumSummaryPresentationBrand: unique symbol;
export type AlbumSummaryPresentation = Readonly<{
  [albumSummaryPresentationBrand]: true;
}>;

export interface ApplicationOwner {
  readonly overview: LibraryOverviewResponse | undefined;
  readonly albums: ReadonlyArray<AlbumSummary>;
  loadOverview(): Promise<void>;
  refreshOverview(): Promise<boolean>;
  notePublicationConflict(): void;
  advanceAlbumMutationFloor(): boolean;
  confirmSavedPosition(albumId: string): void;
  claimFileLocation(key: string, message: string): FileLocationPresentation;
  presentFileLocation(
    presentation: FileLocationPresentation,
    message: string,
  ): void;
  releaseFileLocation(presentation: FileLocationPresentation): void;
  claimAlbumSummary(key: string): AlbumSummaryPresentation;
  presentAlbumSummary(
    presentation: AlbumSummaryPresentation,
    message: string,
  ): void;
  releaseAlbumSummary(presentation: AlbumSummaryPresentation): void;
  resolveAlbumSummary(presentation: AlbumSummaryPresentation): void;
  activateSummaryAction(
    action: ApplicationSummaryAction,
  ): Readonly<{ kind: "refresh-current-source" }> | undefined;
  dispose(): void;
}

type FileLocationRecord = Readonly<{
  notice: NoticeHandle;
}>;

type AlbumSummaryRecord = Readonly<{
  notice: NoticeHandle;
}>;

type ScanCycle = {
  consumed: boolean;
  admission: "pending" | "admitted";
};

const defaultSchedule: ApplicationSchedule = (delayMs, run) => {
  const timer = setTimeout(() => void run(), delayMs);
  return () => clearTimeout(timer);
};

export function createApplicationOwner(
  fetcher: ApplicationFetch,
  options: {
    emit: (event: ApplicationEvent) => void | Promise<void>;
    schedule?: ApplicationSchedule;
  },
): ApplicationOwner {
  let closed = false;
  const tasks = new TaskScope();
  const scanSettlements = new SettlementFamily();
  const notices = new SummaryNoticeChannel<ApplicationSummary>();
  const fileLocationRecords = new Map<
    FileLocationPresentation,
    FileLocationRecord
  >();
  const albumSummaryRecords = new Map<
    AlbumSummaryPresentation,
    AlbumSummaryRecord
  >();
  const schedule = options.schedule ?? defaultSchedule;

  let overviewDataFloor = 0;
  let overview: LibraryOverviewResponse | undefined;
  let albums: ReadonlyArray<AlbumSummary> = [];
  let visibleSummary: ApplicationSummary = { text: "Loading Library…" };
  let overviewRecovery: ApplicationRecovery | undefined;
  let scanCommandRecovery: ApplicationRecovery | undefined;
  let observedScanState: string | undefined;
  let observedPublication: string | undefined;
  let publicationBaselineEstablished = false;
  let scanFailureNotice: NoticeHandle | undefined;
  let scanCompletionNotice: NoticeHandle | undefined;
  let activeScanCycle: ScanCycle | undefined;
  let lastCompletedPublication: string | undefined;

  const recovery = (): ApplicationRecovery =>
    Object.freeze({}) as ApplicationRecovery;

  const summaryAction = (
    kind: ApplicationSummaryAction["kind"],
  ): ApplicationSummaryAction =>
    Object.freeze({ kind }) as ApplicationSummaryAction;

  const summary = (
    text: string,
    action?: ApplicationSummaryAction,
  ): ApplicationSummary =>
    Object.freeze({ text, ...(action ? { action } : {}) });

  const emit = (event: ApplicationEvent): Promise<void> => {
    if (closed) return Promise.resolve();
    try {
      return Promise.resolve(options.emit(event)).catch(() => {});
    } catch {
      return Promise.resolve();
    }
  };

  const publishSummary = (value: ApplicationSummary): void => {
    if (closed) return;
    visibleSummary = value;
    void emit({ kind: "summary", summary: value });
  };

  const applySummaryUpdate = (
    update: NoticeUpdate<ApplicationSummary>,
  ): void => {
    if (closed || !("visible" in update)) return;
    if (update.visible === null) {
      const epoch = notices.backgroundEpoch();
      applySummaryUpdate(
        notices.presentBackground(
          epoch,
          summary(overview ? scanLabel(overview.scan) : "Loading Library…"),
        ),
      );
      return;
    }
    if (update.visible !== undefined) publishSummary(update.visible);
  };

  const publishOverview = (): void => {
    if (closed || !overview) return;
    void emit({ kind: "overview", overview, albums });
  };

  const failApplicationRecovery = (
    applicationRecovery: ApplicationRecovery,
    slot: "overview-reload" | "scan-command",
  ): void => {
    if (closed) return;
    void emit({
      kind: "fail-application-recovery",
      recovery: applicationRecovery,
      slot,
    });
  };

  const recover = (applicationRecovery: ApplicationRecovery): void => {
    if (!closed) void emit({ kind: "recover", recovery: applicationRecovery });
  };

  const markReachable = (): void => {
    if (!closed) void emit({ kind: "mark-reachable" });
  };

  const observePublication = (publication?: string): boolean => {
    if (!publicationBaselineEstablished) {
      publicationBaselineEstablished = true;
      observedPublication = publication;
      return false;
    }
    if (publication === observedPublication) return false;
    observedPublication = publication;
    overviewDataFloor += 1;
    return true;
  };

  const startBootstrap = (committed: LibraryOverviewResponse): void => {
    const lease = tasks.beginLatest("overview-bootstrap", {
      abortTransport: false,
    });
    void emit({
      kind: "bootstrap",
      overview: committed,
      isCurrent: () => lease.isCurrent(),
    }).finally(() => lease.finish());
  };

  const refreshOverview = async (refreshOptions?: {
    bootstrap?: boolean;
    markReachable?: () => boolean;
  }): Promise<boolean> => {
    if (closed) return false;
    const task = tasks.beginOrdered("overview", overviewDataFloor);
    const background = notices.backgroundEpoch();
    let backgroundSettled = false;
    try {
      const body = await fetchLibraryOverview(fetcher);
      const validationEpoch = notices.backgroundEpoch();
      let validationSettled = false;
      try {
        const validation = await fetchLibraryStatus(fetcher);
        notices.discardBackground(validationEpoch);
        validationSettled = true;
        if (body.publication !== validation.publication) {
          if (validation.publication !== observedPublication)
            overviewDataFloor += 1;
          return false;
        }
      } finally {
        if (!validationSettled) notices.discardBackground(validationEpoch);
      }
      if (!task.commit(overviewDataFloor)) return false;
      observePublication(body.publication);
      overview = body;
      albums = Object.freeze([...body.albums]);
      ensureStatusMonitor(body.scan);
      applySummaryUpdate(
        notices.presentBackground(background, summary(scanLabel(body.scan))),
      );
      backgroundSettled = true;
      publishOverview();
      if (refreshOptions?.markReachable?.()) markReachable();
      if (refreshOptions?.bootstrap) startBootstrap(body);
      return true;
    } finally {
      if (!backgroundSettled) notices.discardBackground(background);
      task.finish();
    }
  };

  const releaseScanFailure = (): void => {
    if (!scanFailureNotice) return;
    applySummaryUpdate(notices.release(scanFailureNotice));
    scanFailureNotice = undefined;
  };

  const claimScanFailure = (): void => {
    if (!scanFailureNotice)
      scanFailureNotice = notices.issue(
        "scan-failure",
        "library",
        ACTIONABLE_NOTICE_PRIORITY,
      );
    applySummaryUpdate(
      notices.present(
        scanFailureNotice,
        summary(
          "Library check failed; the last complete Library remains available.",
          summaryAction("retry-library-check"),
        ),
      ),
    );
  };

  const completeScan = (cycle?: ScanCycle, publication?: string): void => {
    if (cycle?.consumed || (cycle && cycle.admission !== "admitted")) return;
    if (cycle) cycle.consumed = true;
    if (activeScanCycle === cycle) activeScanCycle = undefined;
    if (cycle && scanCommandRecovery) {
      recover(scanCommandRecovery);
      scanCommandRecovery = undefined;
      markReachable();
    }
    if (closed) return;
    if (publication && publication === lastCompletedPublication) return;
    if (publication) lastCompletedPublication = publication;
    if (!observePublication(publication) && !publication)
      overviewDataFloor += 1;
    releaseScanFailure();
    if (scanCompletionNotice)
      applySummaryUpdate(notices.release(scanCompletionNotice));
    const notice = notices.issue(
      "scan-completion",
      "library",
      ACTIONABLE_NOTICE_PRIORITY,
    );
    scanCompletionNotice = notice;
    applySummaryUpdate(
      notices.present(
        notice,
        summary(
          "Library check complete. Open Browse Snapshots remain unchanged.",
          summaryAction("refresh-current-source"),
        ),
      ),
    );
    void emit({ kind: "reset-file-locations" });
    void refreshOverview()
      .then((committed) => {
        if (committed && !closed)
          void emit({ kind: "load-file-location-root" });
      })
      .catch(() => {});
  };

  const ensureStatusMonitor = (
    baseline?: LibraryOverviewResponse["scan"],
  ): void => {
    if (observedScanState === undefined && baseline)
      observedScanState = baseline.state;
    if (tasks.current("publication-status")) return;
    const monitor = tasks.beginLatest("publication-status", {
      abortTransport: false,
    });
    let cancelScheduled: (() => void) | undefined;
    monitor.onCleanup(() => cancelScheduled?.());

    const queueNext = (): void => {
      if (!monitor.isCurrent()) return;
      cancelScheduled = schedule(
        observedScanState === "idle" ? 2_000 : 500,
        async () => {
          cancelScheduled = undefined;
          if (!monitor.isCurrent()) return;
          const background = notices.backgroundEpoch();
          let settled = false;
          let keepMonitoring = true;
          try {
            const scan = await fetchLibraryStatus(fetcher);
            if (!monitor.isCurrent()) return;
            applySummaryUpdate(
              notices.presentBackground(background, summary(scanLabel(scan))),
            );
            settled = true;
            const prior = observedScanState;
            const publicationChanged =
              scan.publication !== undefined &&
              publicationBaselineEstablished &&
              scan.publication !== observedPublication;
            observedScanState = scan.state;
            if (scan.state !== "idle" && scan.state !== "failed") {
              if (activeScanCycle?.admission === "pending")
                activeScanCycle.admission = "admitted";
            }
            if (scan.state === "failed") {
              if (activeScanCycle) activeScanCycle.consumed = true;
              activeScanCycle = undefined;
              claimScanFailure();
              keepMonitoring = false;
            } else if (
              scan.state === "idle" &&
              prior &&
              prior !== "idle" &&
              prior !== "failed"
            ) {
              const admitted =
                activeScanCycle?.admission === "admitted"
                  ? activeScanCycle
                  : undefined;
              completeScan(admitted, scan.publication);
            } else if (scan.state === "idle" && publicationChanged) {
              completeScan(undefined, scan.publication);
            }
          } catch {
            /* answered and transport status failures stay silent */
          } finally {
            if (!settled) notices.discardBackground(background);
            if (monitor.isCurrent() && keepMonitoring) queueNext();
            else monitor.finish();
          }
        },
      );
    };
    queueNext();
  };

  const retryLibraryCheck = async (): Promise<void> => {
    const settlement = scanSettlements.begin({ admissionKey: "scan" });
    if (!settlement) return;
    const cycle: ScanCycle = { consumed: false, admission: "pending" };
    activeScanCycle = cycle;
    ensureStatusMonitor();
    try {
      const result = await requestLibraryScan(fetcher);
      if (closed) return;
      if (result.kind === "rejected") {
        if (activeScanCycle === cycle) activeScanCycle = undefined;
        if (result.status >= 500) {
          scanCommandRecovery ??= recovery();
          failApplicationRecovery(scanCommandRecovery, "scan-command");
        }
        return;
      }
      cycle.admission = "admitted";
      if (scanCommandRecovery) {
        recover(scanCommandRecovery);
        scanCommandRecovery = undefined;
        markReachable();
      }
      observedScanState = result.scan.state;
      if (result.scan.state === "idle")
        completeScan(cycle, result.scan.publication);
      else if (result.scan.state === "failed") claimScanFailure();
    } catch {
      if (!closed) {
        scanCommandRecovery ??= recovery();
        failApplicationRecovery(scanCommandRecovery, "scan-command");
      }
    } finally {
      settlement.finish();
    }
  };

  const loadOverview = async (): Promise<void> => {
    if (closed) return;
    const load = tasks.beginLatest("overview-load", { abortTransport: false });
    const barrier = notices.beginBarrier(
      "reload",
      "overview",
      ACTIONABLE_NOTICE_PRIORITY,
      summary("Loading Library summary…"),
    );
    applySummaryUpdate(barrier.update);
    try {
      await refreshOverview({
        bootstrap: true,
        markReachable: () => load.isCurrent(),
      });
      if (load.isCurrent() && overviewRecovery) {
        recover(overviewRecovery);
        overviewRecovery = undefined;
        markReachable();
      }
      applySummaryUpdate(notices.release(barrier.handle));
    } catch {
      if (!load.isCurrent()) return;
      applySummaryUpdate(
        notices.present(
          barrier.handle,
          summary("Could not reach Slipstream. Check the server and retry."),
        ),
      );
      overviewRecovery ??= recovery();
      failApplicationRecovery(overviewRecovery, "overview-reload");
    } finally {
      load.finish();
    }
  };

  const owner: ApplicationOwner = {
    get overview() {
      return overview;
    },
    get albums() {
      return albums;
    },
    loadOverview,
    refreshOverview,
    notePublicationConflict: () => {
      if (!closed) overviewDataFloor += 1;
    },
    advanceAlbumMutationFloor: () => {
      if (closed) return false;
      overviewDataFloor += 1;
      return true;
    },
    confirmSavedPosition: (albumId) => {
      if (closed) return;
      const next = albums.map((album) =>
        album.id === albumId ? { ...album, hasSavedPosition: true } : album,
      );
      if (next.every((album, index) => album === albums[index])) return;
      // Saved position is shared Album summary state. Fence any Overview body
      // captured before this confirmation so it cannot regress Resume state.
      overviewDataFloor += 1;
      albums = Object.freeze(next);
      if (overview) overview = { ...overview, albums };
      publishOverview();
    },
    claimFileLocation: (key, message) => {
      const presentation = Object.freeze({}) as FileLocationPresentation;
      if (closed) return presentation;
      const notice = notices.issue(
        "file-location",
        key,
        ACTIONABLE_NOTICE_PRIORITY,
      );
      fileLocationRecords.set(presentation, {
        notice,
      });
      applySummaryUpdate(notices.present(notice, summary(message)));
      return presentation;
    },
    presentFileLocation: (presentation, message) => {
      const record = fileLocationRecords.get(presentation);
      if (record)
        applySummaryUpdate(notices.present(record.notice, summary(message)));
    },
    releaseFileLocation: (presentation) => {
      const record = fileLocationRecords.get(presentation);
      if (!record) return;
      fileLocationRecords.delete(presentation);
      applySummaryUpdate(notices.release(record.notice));
    },
    claimAlbumSummary: (key) => {
      const presentation = Object.freeze({}) as AlbumSummaryPresentation;
      if (closed) return presentation;
      albumSummaryRecords.set(presentation, {
        notice: notices.issue("album", key, ALBUM_NOTICE_PRIORITY),
      });
      return presentation;
    },
    presentAlbumSummary: (presentation, message) => {
      const record = albumSummaryRecords.get(presentation);
      if (!record) return;
      albumSummaryRecords.delete(presentation);
      applySummaryUpdate(
        notices.present(record.notice, summary(message), { fallback: true }),
      );
    },
    releaseAlbumSummary: (presentation) => {
      const record = albumSummaryRecords.get(presentation);
      if (!record) return;
      albumSummaryRecords.delete(presentation);
      applySummaryUpdate(notices.release(record.notice));
    },
    resolveAlbumSummary: (presentation) => {
      const record = albumSummaryRecords.get(presentation);
      if (!record) return;
      albumSummaryRecords.delete(presentation);
      applySummaryUpdate(notices.releaseKind("album", record.notice));
    },
    activateSummaryAction: (action) => {
      if (closed || visibleSummary.action !== action) return undefined;
      if (action.kind === "retry-library-check") {
        void retryLibraryCheck();
        return undefined;
      }
      const notice = scanCompletionNotice;
      if (notice) {
        scanCompletionNotice = undefined;
        applySummaryUpdate(notices.release(notice));
      }
      return { kind: "refresh-current-source" };
    },
    dispose: () => {
      if (closed) return;
      closed = true;
      tasks.halt();
      scanSettlements.closePresentation();
      notices.close();
      fileLocationRecords.clear();
      albumSummaryRecords.clear();
    },
  };
  return owner;
}

function scanLabel(scan: LibraryOverviewResponse["scan"]): string {
  const completed = scan.completed?.toLocaleString();
  switch (scan.state) {
    case "idle":
      return "Library ready";
    case "initializing":
      return "Preparing Photo Library…";
    case "discovering":
      return completed
        ? `Checking Library Folder… ${completed} found`
        : "Checking Library Folder…";
    case "inspecting":
      return scan.total
        ? `Inspecting Capture Time… ${completed ?? 0} / ${scan.total.toLocaleString()}`
        : "Inspecting Capture Time…";
    case "applying":
      return "Applying Library updates…";
    case "failed":
      return "Library check failed; the last complete Library remains available";
    default:
      return `Library ${scan.state}`;
  }
}
