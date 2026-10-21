import type { BrowserFetch } from "../api/browser-fetch.js";
import { isRecord } from "../api/guards.js";
import {
  parseToolJob,
  requestIdentity,
  toolCommand,
  toolPath,
  workspaceRequest,
  WorkspaceApiError,
  type EditView,
  type JsonObject,
  type ToolJob,
  type WorkspaceFile,
} from "../api/workspace-edit.js";
import type { WorkspaceEditModel } from "./workspace-edit-owner.js";

const TERMINAL_STATUSES = ["succeeded", "failed", "cancelled"];

/** A retained job the workspace already admitted on the owner's behalf. */
export type RetainedJob = Readonly<{
  /** The request identity the service retained for this job. */
  requestId: string;
  /** The exact wire body the service retained. */
  body: JsonObject;
  operation: string;
  editId: string;
  revision: unknown;
}>;

/**
 * The Darktable supervised-job lane.
 *
 * Everything a job needs from the moment its admission answer arrives until the
 * workspace state it produced is observed lives here: the retained-job adoption
 * performed while an edit is opened, the single-flight poll guard, the delayed
 * file discovery that survives a temporary workspace lease, the stale-preview
 * fence that only lets the newest requested revision settle a preview, and the
 * timer that keeps polling while work is still admitted.
 *
 * The lane is private to the workspace edit owner: it holds no independent
 * authority and never decides what an edit should look like. It reports only
 * through the callbacks the owner hands it.
 */
export type WorkspaceJobLane = {
  /** Poll every retained job once and settle whatever has finished. */
  poll(): Promise<void>;
  /** Retain jobs the workspace already admitted on this edit. */
  retain(retained: readonly RetainedJob[]): void;
  /** Admit a job answer and start settling it. */
  admit(job: ToolJob): void;
  /** The newest saved revision still awaits its preview. */
  readonly previewAfterSave: boolean;
  /** True while an export holds the save barrier. */
  readonly barrierHeld: boolean;
  requestPreviewAfterSave(): void;
  supersedePreview(): void;
  holdBarrier(): void;
  releaseBarrier(): void;
  /**
   * Retry a failed export against its retained capture. Throws the admission
   * errors the owner already treats as an uncertain request.
   */
  retry(requestId: unknown): Promise<void>;
  dispose(): void;
};

/**
 * A job failure the lane itself decided. The owner presents it exactly as it
 * presents any other operation failure; the class only carries the operation
 * name the lane attributes the failure to.
 */
export class WorkspaceJobFailure extends WorkspaceApiError {
  constructor(
    code: string,
    message: string,
    details: JsonObject,
    public readonly operation: string,
  ) {
    super(code, message, "none", details);
  }
}

type WorkspaceJobLaneDependencies = Readonly<{
  fetcher: BrowserFetch;
  photoId: string;
  model: WorkspaceEditModel;
  publish: () => void;
  reportFailure(error: unknown, operation: string): void;
  /** Refresh file evidence after a job settles. */
  readFiles(): Promise<void>;
  /** Render a preview the lane asked to retry after a save. */
  render(operation: string, args?: JsonObject, captured?: EditView): void;
  /** File matching by retained artifact identity, else retained path. */
  findFile(
    artifact?: JsonObject,
    result?: JsonObject,
  ): WorkspaceFile | undefined;
  /** Comparison lookup preserves the original artifact/path matching rule. */
  findComparisonFile(comparison: JsonObject): WorkspaceFile | undefined;
  /** Present a settled preview against the current workspace. */
  presentPreview(
    file: WorkspaceFile,
    revision: string,
    identity: string,
    comparisonFile?: WorkspaceFile,
  ): void;
}>;

/** A writer still settling defers file observation instead of failing it. */
const isDelayedDiscovery = (error: unknown): boolean =>
  error instanceof WorkspaceApiError && error.code === "workspace_busy";

export function createWorkspaceJobLane(
  deps: WorkspaceJobLaneDependencies,
): WorkspaceJobLane {
  const jobs: ToolJob[] = [];
  let alive = true;
  let polling = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let wantedPreviewRequest: string | undefined;
  let previewAfterSave = false;
  let barrier = false;
  const isTerminal = (status: string): boolean =>
    TERMINAL_STATUSES.includes(status);

  /**
   * The published model carries the same job list the lane settles, so the
   * visible status of every retained job stays observable to the owner and its
   * surface without the lane exposing its internal array.
   */
  const syncJobs = (): void => {
    deps.model.jobs = [...jobs];
  };

  const setPreviewPending = (): void => {
    deps.model.previewPending = jobs.some(
      (job) => job.operation === "preview render" && !isTerminal(job.status),
    );
  };

  const isWantedPreview = (job: ToolJob): boolean => {
    const edit = deps.model.edit;
    return (
      job.operation === "preview render" &&
      job.requestId === wantedPreviewRequest &&
      !!edit &&
      edit.editId === job.editId &&
      edit.revision === job.revision &&
      !deps.model.draft.length
    );
  };

  /**
   * The stale-preview fence: a settled preview becomes the presented preview
   * only when it is the revision the current edit was last rendered from, the
   * edit has not advanced, and no draft is pending. An older, newer, or
   * contested revision never displaces what is on screen.
   */
  const settlePreview = (job: ToolJob, previous: ToolJob): boolean => {
    if (!isWantedPreview(previous)) return false;
    const edit = deps.model.edit;
    if (!edit) return false;
    const result = isRecord(job.result) ? job.result : job;
    const artifact = isRecord(job.artifact)
      ? job.artifact
      : isRecord(result.artifact)
        ? result.artifact
        : undefined;
    const file = artifact
      ? deps.findFile(artifact)
      : deps.findFile(undefined, result);
    if (!file || file.state !== "ready") return false;
    const comparison = isRecord(result.comparison)
      ? result.comparison
      : undefined;
    const comparisonFile = comparison
      ? deps.findComparisonFile(comparison)
      : undefined;
    deps.presentPreview(
      file,
      previous.revision as string,
      previous.requestId,
      comparisonFile,
    );
    deps.model.status = "Ready";
    return true;
  };

  /**
   * File discovery after a job settles is delayed rather than failing: a writer
   * still settling answers `workspace_busy`, and its file only becomes
   * observable on a later read. The job returns to `checking` so polling
   * continues until the file is visible.
   */
  const discoverSettledFiles = async (
    job: ToolJob,
    previous: ToolJob,
    index: number,
  ): Promise<boolean> => {
    try {
      await deps.readFiles();
    } catch (error) {
      if (!isDelayedDiscovery(error)) throw error;
      jobs[index] = { ...previous, ...job, status: "checking" };
      deps.model.status = "Checking file";
      syncJobs();
      return false;
    }
    if (job.status === "succeeded" && previous.operation === "export create")
      deps.model.status = "Ready";
    if (job.status === "failed")
      throw new WorkspaceJobFailure(
        "render_failed",
        typeof job.failure === "string"
          ? job.failure
          : "Rendering failed. Retry preview or export explicitly.",
        isRecord(job.failure) ? job.failure : {},
        "Rendering",
      );
    if (job.status === "succeeded" && settlePreview(job, previous)) return true;
    if (job.status === "succeeded" && isWantedPreview(previous)) {
      jobs[index] = { ...previous, ...job, status: "checking" };
      deps.model.status = "Checking file";
      return false;
    }
    return true;
  };

  const poll = async (): Promise<void> => {
    if (!alive || polling) return;
    clearTimeout(timer);
    polling = true;
    let pending = false;
    for (const previous of [...jobs]) {
      if (isTerminal(previous.status)) continue;
      try {
        const job = parseToolJob(
          await workspaceRequest(
            deps.fetcher,
            `${toolPath(deps.photoId)}/jobs/${encodeURIComponent(previous.requestId)}`,
          ),
        );
        const index = jobs.findIndex(
          (item) => item.requestId === job.requestId,
        );
        if (index < 0) continue;
        jobs[index] = { ...previous, ...job };
        if (!isTerminal(job.status)) {
          syncJobs();
          pending = true;
          continue;
        }
        pending =
          !(await discoverSettledFiles(job, previous, index)) || pending;
        syncJobs();
      } catch (error) {
        deps.reportFailure(error, "Rendering");
      }
    }
    setPreviewPending();
    deps.publish();
    polling = false;
    if (alive && previewAfterSave && !barrier && !deps.model.previewPending)
      deps.render("preview render");
    if (alive && pending)
      timer = setTimeout(() => {
        void poll();
      }, 1000);
  };

  return {
    poll,
    retain(retained: readonly RetainedJob[]): void {
      for (const entry of retained)
        if (!jobs.some((job) => job.requestId === entry.requestId))
          jobs.push(
            parseToolJob({
              ...entry.body,
              operation: entry.operation,
              editId: entry.editId,
              revision: entry.revision,
            }),
          );
      syncJobs();
      setPreviewPending();
    },
    admit(job: ToolJob): void {
      jobs.push(job);
      if (job.operation === "preview render")
        wantedPreviewRequest = job.requestId;
      syncJobs();
      setPreviewPending();
      void poll();
    },
    get previewAfterSave(): boolean {
      return previewAfterSave;
    },
    get barrierHeld(): boolean {
      return barrier;
    },
    requestPreviewAfterSave(): void {
      previewAfterSave = true;
    },
    supersedePreview(): void {
      wantedPreviewRequest = undefined;
      previewAfterSave = false;
    },
    holdBarrier(): void {
      barrier = true;
    },
    releaseBarrier(): void {
      barrier = false;
    },
    async retry(requestId: unknown): Promise<void> {
      const original = jobs.find((job) => job.requestId === requestId);
      const result = await toolCommand(
        deps.fetcher,
        deps.photoId,
        deps.model.edit?.editId ?? null,
        "export retry",
        { id: requestId, requestId: requestIdentity() },
      );
      if (
        typeof result.requestId !== "string" ||
        typeof result.status !== "string"
      )
        throw new WorkspaceJobFailure(
          "outcome_unknown",
          "Retry admission is uncertain. Inspect retained jobs.",
          {},
          "export retry",
        );
      jobs.push(
        parseToolJob({
          ...result,
          operation: original?.operation ?? "export create",
          revision: isRecord(result.capture)
            ? result.capture.revision
            : result.revision,
        }),
      );
      syncJobs();
      void poll();
    },
    dispose(): void {
      alive = false;
      clearTimeout(timer);
    },
  };
}
