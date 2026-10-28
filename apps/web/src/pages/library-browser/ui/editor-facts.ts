//! The Edit surface's independent facts, each with its own explanation.
//!
//! The tool, the Original/input, the saved State, Preview freshness, the job
//! outcome and the Library's own progress are separate facts. A recovering
//! Library must never look like an unreadable Original, and a missing live
//! Original must not erase the retained input, the saved State or the completed
//! files.
//!
//! See [Web Interaction](../../../../../specs/photo-development/workspace/spec.md#web-interaction).

import { isRecord } from "../api/guards.js";
import type { EditorViewModel } from "../model/editor-controller-contract.js";
import type { WorkspaceFile } from "../api/workspace-edit.js";

/** One independent fact of the Edit surface. */
export type EditorFactRow = Readonly<{
  label: string;
  value: string;
}>;

type Edit = NonNullable<EditorViewModel["edit"]>;

const availability = (edit: Edit): Readonly<Record<string, unknown>> =>
  isRecord(edit.availability) ? edit.availability : {};

const execute = (edit: Edit): boolean => availability(edit).execute !== false;

const libraryPhase = (model: EditorViewModel): string => {
  const phase = model.libraryPhase;
  return typeof phase === "string" && phase ? phase : "unknown";
};

const libraryFact = (model: EditorViewModel): EditorFactRow => {
  const phase = libraryPhase(model);
  switch (phase) {
    case "recovering":
    case "repairing":
      return {
        label: "Library",
        value:
          "The Library is still rebuilding its own index. Your saved edits, retained input and completed files are unaffected.",
      };
    case "failed":
      return {
        label: "Library",
        value:
          "The Library could not finish its own scan. Your saved edits and completed files are unaffected.",
      };
    case "initializing":
    case "discovering":
    case "inspecting":
    case "applying":
      return {
        label: "Library",
        value:
          "The Library is checking its own index. Your saved edits, retained input and completed files are unaffected.",
      };
    case "idle":
      return { label: "Library", value: "The Library scan is not running." };
    default:
      return {
        label: "Library",
        value:
          "The Library's own progress is unavailable right now. Your saved edits and completed files are unaffected.",
      };
  }
};

const originalFact = (model: EditorViewModel): EditorFactRow => {
  const input = model.files.find(
    (file) => file.artifactId === model.selectedInput,
  );
  if (input)
    return {
      label: "Original input",
      value: `${input.relativePath} is the selected input for this edit.`,
    };
  const alternatives = model.files.filter(
    (file) => file.state === "ready" && file.kind !== "preview",
  );
  if (alternatives.length)
    return {
      label: "Original input",
      value:
        "The Original is not available in this workspace. Select a ready compatible input or prepare input again.",
    };
  return {
    label: "Original input",
    value:
      "The Original is not available in this workspace, and no compatible file has been prepared. Prepare input again to read a new identity.",
  };
};

const toolFact = (model: EditorViewModel): EditorFactRow => {
  const edit = model.edit;
  if (!edit)
    return {
      label: "Editing tool",
      value: "No editing tool has been started for this workspace yet.",
    };
  if (!execute(edit))
    return {
      label: "Editing tool",
      value:
        "The editing tool cannot accept changes right now. Inspect the tool's own capability before applying a change.",
    };
  return {
    label: "Editing tool",
    value: `The editing tool is ready at revision ${String(edit.revision)}.`,
  };
};

const stateFact = (model: EditorViewModel): EditorFactRow => {
  const edit = model.edit;
  if (!edit)
    return {
      label: "Saved state",
      value:
        "The tool has no saved editing state for this workspace. Unconfirmed settings stay a draft in this browser.",
    };
  if (model.draft.length)
    return {
      label: "Saved state",
      value: `Settings are not confirmed yet. The tool last confirmed revision ${String(edit.revision)}; your changes are still a draft.`,
    };
  return {
    label: "Saved state",
    value: `Settings are confirmed at revision ${String(edit.revision)}.`,
  };
};

const previewFact = (model: EditorViewModel): EditorFactRow => {
  if (model.cameraReference)
    return {
      label: "Preview",
      value:
        "The camera Preview is the Original reference. It is not a workspace result and it does not show your saved settings.",
    };
  if (!model.preview)
    return {
      label: "Preview",
      value: model.edit
        ? "No Preview has been rendered for the confirmed state yet. Request one when you need it."
        : "No Preview is available until an editing tool has been started.",
    };
  const stale =
    model.preview.revision !== model.edit?.revision || model.draft.length > 0;
  return {
    label: "Preview",
    value: stale
      ? "This Preview is from an earlier saved revision. It does not show your current settings."
      : model.previewPending
        ? "This Preview is being updated for the confirmed state."
        : "This Preview shows the confirmed state.",
  };
};

const jobFact = (model: EditorViewModel): EditorFactRow => {
  const jobs = model.jobs;
  if (!jobs.length)
    return {
      label: "Processing jobs",
      value: "No processing job is running for this workspace.",
    };
  const active = jobs.filter(
    (job) => !["succeeded", "failed", "cancelled"].includes(job.status),
  );
  if (active.length)
    return {
      label: "Processing jobs",
      value: `${active.length} job${active.length === 1 ? "" : "s"} in progress. A finished file appears only after its job settles.`,
    };
  const failed = jobs.filter((job) => job.status === "failed");
  if (failed.length)
    return {
      label: "Processing jobs",
      value: `${failed.length} job${failed.length === 1 ? "" : "s"} failed. The saved settings are unaffected; inspect the job's own reason.`,
    };
  return {
    label: "Processing jobs",
    value: "Every job of this workspace has settled.",
  };
};

/**
 * The independent facts of the Edit surface, in presentation order. The
 * Library's own progress and the Original's unavailability never share a
 * label, so a recovering Library cannot look like an unreadable Original.
 */
export function editorFactRows(model: EditorViewModel): EditorFactRow[] {
  return [
    libraryFact(model),
    originalFact(model),
    toolFact(model),
    stateFact(model),
    previewFact(model),
    jobFact(model),
  ];
}

/** The save state of the primary action strip, as its own fact. */
export function saveStateRow(model: EditorViewModel): EditorFactRow {
  if (model.uncertain)
    return {
      label: "Save state",
      value:
        "The last save did not answer. It may have been written. Replay the exact request before applying another change.",
    };
  if (model.blocked)
    return {
      label: "Save state",
      value:
        "Saving stopped because the confirmed state changed under your draft. Inspect the current settings, then reapply or discard the draft.",
    };
  if (model.saving || model.draft.length)
    return {
      label: "Save state",
      value: "Saving your latest settings.",
    };
  if (model.edit)
    return {
      label: "Save state",
      value: `All settings are saved at revision ${String(model.edit.revision)}.`,
    };
  return {
    label: "Save state",
    value: "There is no editing state to save yet.",
  };
}

/** The files disclosure's own unavailable explanation. */
export function filesFactRow(files: readonly WorkspaceFile[]): EditorFactRow {
  const ready = files.filter((file) => file.state === "ready");
  if (!files.length)
    return {
      label: "Files",
      value:
        "No workspace file has been listed yet. Reload the workspace to list them.",
    };
  if (!ready.length)
    return {
      label: "Files",
      value:
        "The workspace lists files, but none is ready yet. Completed files appear when their job settles.",
    };
  return {
    label: "Files",
    value: `${ready.length} file${ready.length === 1 ? "" : "s"} ready in this workspace.`,
  };
}
