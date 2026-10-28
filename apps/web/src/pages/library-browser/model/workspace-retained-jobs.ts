//! Retained jobs of one edit, recovered from the tool's own export list.
//!
//! A completed or failed job the tool already knows stays observable through
//! the per-edit job lane. Nothing here admits a new operation: it only recovers
//! what the tool reported, so a reopened workspace shows the same job outcome
//! without re-requesting it.
//!
//! See [Web Interaction](../../../../../specs/photo-development/workspace/spec.md#web-interaction).

import { isRecord } from "../api/guards.js";
import type { JsonObject } from "../api/workspace-edit.js";

/** One job the tool retained for this edit. */
export type RetainedJob = Readonly<{
  requestId: string;
  body: JsonObject;
  operation: string;
  editId: string;
  revision: unknown;
}>;

/**
 * The jobs in the tool's export list that this owner has not retained yet.
 * A job this owner already knows is never replaced, so a later read can never
 * overwrite a newer outcome with an older one.
 */
export function retainedExportJobs(
  jobs: unknown,
  editId: string,
  known: readonly { requestId: string }[],
): RetainedJob[] {
  if (!Array.isArray(jobs)) return [];
  return jobs.flatMap((job) => {
    if (!isRecord(job) || typeof job.requestId !== "string") return [];
    if (known.some((existing) => existing.requestId === job.requestId))
      return [];
    return [
      {
        requestId: job.requestId,
        body: job,
        operation: "export create",
        editId,
        revision: isRecord(job.capture) ? job.capture.revision : job.revision,
      },
    ];
  });
}
