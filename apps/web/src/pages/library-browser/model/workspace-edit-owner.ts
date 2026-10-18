import type { BrowserFetch } from "./access-session.js";
import { isRecord } from "../api/guards.js";
import {
  fileBytesUrl,
  parseEdit,
  parseToolJob,
  parseWindow,
  requestIdentity,
  toolCommand,
  toolPath,
  workspacePath,
  workspaceRequest,
  WorkspaceApiError,
  type EditView,
  type JsonObject,
  type ToolJob,
  type Workspace,
  type WorkspaceFile,
} from "../api/workspace-edit.js";
import type { ProcessingModuleDescription } from "../api/processing-modules.js";
import { coalesceChanges } from "./workspace-edit-draft.js";
import { refreshWorkspaceFiles } from "./workspace-edit-files.js";
import { isUnknownEffect, type UncertainRequest } from "./uncertain-request.js";

export type EditChange = JsonObject & { type: string };
export type WorkspaceEditModel = {
  photoId: string;
  workspace?: Workspace;
  files: WorkspaceFile[];
  edit?: EditView;
  modules: JsonObject[];
  descriptions: Record<string, JsonObject>;
  instanceDetails: Record<string, JsonObject>;
  maskDetails: Record<string, JsonObject>;
  resourceKinds: JsonObject[];
  catalogs: Record<string, JsonObject>;
  selectedInput?: string;
  inspectedFile?: WorkspaceFile;
  draft: EditChange[];
  saving: boolean;
  loading: boolean;
  status: string;
  error?: WorkspaceApiError;
  blocked: boolean;
  preview?: {
    url: string;
    revision: string;
    file: WorkspaceFile;
    comparisonUrl?: string;
    identity: string;
  };
  previewPending: boolean;
  comparing: boolean;
  cameraReference: boolean;
  jobs: ToolJob[];
  history: JsonObject[];
  fileCursor: string | null;
  uncertain?: UncertainRequest;
  filmElement?: HTMLElement;
  catalogElement?: HTMLElement;
  maskGraphSchema?: JsonObject;
  filmModules?: readonly ProcessingModuleDescription[];
  conflictInspected?: boolean;
};
export type WorkspaceEditOwner = {
  model: WorkspaceEditModel;
  refresh(): Promise<void>;
  settle(): Promise<boolean>;
  act(action: string, args?: JsonObject): Promise<void>;
  command(
    operation: string,
    args?: JsonObject,
    guarded?: boolean,
  ): Promise<JsonObject | undefined>;
  dispose(): void;
};
export function createWorkspaceEditOwner(
  fetcher: BrowserFetch,
  photoId: string,
  notify: (model: WorkspaceEditModel) => void,
): WorkspaceEditOwner {
  const model: WorkspaceEditModel = {
    photoId,
    files: [],
    modules: [],
    descriptions: {},
    instanceDetails: {},
    maskDetails: {},
    resourceKinds: [],
    catalogs: {},
    draft: [],
    saving: false,
    loading: false,
    status: "Preparing input",
    blocked: false,
    previewPending: false,
    comparing: false,
    cameraReference: false,
    jobs: [],
    history: [],
    fileCursor: null,
  };
  let alive = true;
  let editGeneration = 0;
  let savePromise: Promise<void> | undefined;
  let barrier = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let wantedPreviewRequest: string | undefined;
  let polling = false;
  let previewAfterSave = false;
  let selectingEdit = false;
  const publish = () => {
    if (alive) notify(model);
  };
  const failure = (error: unknown, operation: string) => {
    model.error =
      error instanceof WorkspaceApiError
        ? error
        : new WorkspaceApiError("operation_failed", String(error), "none", {});
    model.status = `${operation} failed`;
    publish();
  };
  const readFiles = async (continuation = false) => {
    await refreshWorkspaceFiles(fetcher, model, continuation);
    publish();
  };
  const readEdit = async (editId: string, generation = editGeneration) => {
    const result = parseEdit(
      await toolCommand(fetcher, photoId, editId, "edit show"),
    );
    if (generation !== editGeneration) return;
    model.edit = result;
    const [modules, kinds, history, exports] = await Promise.all([
      toolCommand(fetcher, photoId, editId, "modules list", { limit: 60 }),
      toolCommand(fetcher, photoId, editId, "resources kinds"),
      toolCommand(fetcher, photoId, editId, "history list", { limit: 60 }),
      toolCommand(fetcher, photoId, editId, "export list", { limit: 60 }),
    ]);
    if (generation !== editGeneration) return;
    model.modules = parseWindow(modules, (value) => {
      if (!isRecord(value)) throw new Error("Invalid module schema");
      return value;
    }).items;
    model.resourceKinds = Array.isArray(kinds.kinds)
      ? kinds.kinds.filter(isRecord)
      : Array.isArray(kinds.items)
        ? kinds.items.filter(isRecord)
        : [];
    model.history = Array.isArray(history.items)
      ? history.items.filter(isRecord)
      : [];
    const retainedJobs = Array.isArray(exports.jobs)
      ? exports.jobs
      : Array.isArray(exports.items)
        ? exports.items
        : [];
    for (const job of retainedJobs)
      if (
        isRecord(job) &&
        typeof job.requestId === "string" &&
        typeof job.status === "string" &&
        !model.jobs.some((existing) => existing.requestId === job.requestId)
      )
        model.jobs.push(
          parseToolJob({
            ...job,
            operation: "export create",
            editId,
            revision: isRecord(job.capture)
              ? job.capture.revision
              : job.revision,
          }),
        );
    if (isRecord(kinds.maskGraphSchema))
      model.maskGraphSchema = kinds.maskGraphSchema;
    const descriptors = await Promise.all(
      result.instances.items.map(async (instance) => {
        const [description, detail] = await Promise.all([
          toolCommand(fetcher, photoId, editId, "modules describe", {
            moduleId: instance.moduleId,
            ifState: result.stateToken,
          }),
          toolCommand(fetcher, photoId, editId, "edit instances show", {
            instanceId: instance.instanceId,
            ifState: result.stateToken,
          }),
        ]);
        return { instance, description, detail };
      }),
    );
    if (generation !== editGeneration) return;
    model.descriptions = {};
    model.instanceDetails = {};
    for (const entry of descriptors) {
      model.descriptions[entry.instance.moduleId] = entry.description;
      model.instanceDetails[entry.instance.instanceId] = entry.detail;
    }
    model.maskDetails = {};
    for (const mask of result.masks.items)
      if (typeof mask.maskId === "string")
        model.maskDetails[mask.maskId] = await toolCommand(
          fetcher,
          photoId,
          editId,
          "edit masks show",
          { maskId: mask.maskId, ifState: result.stateToken },
        );
    result.actions.items = await Promise.all(
      result.actions.items.map((action) =>
        typeof action.actionId === "string"
          ? toolCommand(fetcher, photoId, editId, "edit actions describe", {
              actionId: action.actionId,
              ...(typeof action.instanceId === "string"
                ? { instanceId: action.instanceId }
                : {}),
              ifState: result.stateToken,
            })
          : Promise.resolve(action),
      ),
    );
    model.status = model.draft.length ? "Unsaved settings" : "Ready";
    publish();
  };
  const refresh = async () => {
    model.loading = true;
    delete model.error;
    publish();
    try {
      if (!model.workspace) {
        const workspace = await workspaceRequest(
          fetcher,
          `/api/photos/${encodeURIComponent(photoId)}/workspace`,
          {},
        );
        if (
          typeof workspace.workspaceId !== "string" ||
          workspace.photoId !== photoId
        )
          throw new Error("Invalid workspace identity");
        model.workspace = workspace as Workspace;
      }
      const workspace = await workspaceRequest(
        fetcher,
        workspacePath(model.workspace.workspaceId),
      );
      model.workspace = workspace as Workspace;
      if (!Array.isArray(model.workspace.edits)) model.workspace.edits = [];
      await readFiles();
      if (!model.selectedInput && model.workspace.inputArtifactId)
        model.selectedInput = model.workspace.inputArtifactId;
      const edits = model.workspace.edits.filter(
        (edit) => edit.tool === "darktable",
      );
      if (
        model.edit &&
        !selectingEdit &&
        !model.draft.length &&
        !model.uncertain &&
        !model.saving
      )
        await readEdit(model.edit.editId);
      else if (!selectingEdit && !model.edit && edits.length === 1)
        await readEdit(edits[0]!.editId);
      if (
        model.edit &&
        !model.preview &&
        isRecord(model.edit.latestPreview) &&
        model.workspace
      ) {
        const preview = model.edit.latestPreview;
        const file = model.files.find(
          (item) =>
            item.kind === "preview" &&
            item.state === "ready" &&
            typeof preview.path === "string" &&
            preview.path.endsWith(`/${item.relativePath}`),
        );
        if (file && typeof preview.revision === "string")
          model.preview = {
            file,
            url: fileBytesUrl(model.workspace.workspaceId, file),
            revision: preview.revision,
            identity: String(preview.requestId ?? preview.path),
          };
      }
      if (
        model.jobs.some(
          (job) => !["succeeded", "failed", "cancelled"].includes(job.status),
        )
      )
        void pollJobs();
      model.status = model.edit
        ? model.draft.length
          ? "Unsaved settings"
          : "Ready"
        : edits.length > 1
          ? "Choose a saved edit"
          : model.selectedInput
            ? "Choose an editing tool"
            : "No default input selected";
    } catch (error) {
      failure(error, "Preparing or checking workspace");
    } finally {
      model.loading = false;
      publish();
    }
  };
  const pollJobs = async () => {
    if (!alive || polling) return;
    clearTimeout(timer);
    polling = true;
    let pending = false;
    for (const previous of [...model.jobs]) {
      if (["succeeded", "failed", "cancelled"].includes(previous.status))
        continue;
      try {
        const job = parseToolJob(
          await workspaceRequest(
            fetcher,
            `${toolPath(photoId)}/jobs/${encodeURIComponent(previous.requestId)}`,
          ),
        );
        const index = model.jobs.findIndex(
          (item) => item.requestId === job.requestId,
        );
        if (index >= 0) model.jobs[index] = { ...previous, ...job };
        if (!["succeeded", "failed", "cancelled"].includes(job.status)) {
          pending = true;
          continue;
        }
        try {
          await readFiles();
        } catch (error) {
          if (
            !(error instanceof WorkspaceApiError) ||
            error.code !== "workspace_busy"
          )
            throw error;
          model.jobs[index] = { ...previous, ...job, status: "checking" };
          model.status = "Checking file";
          pending = true;
          continue;
        }
        if (
          job.status === "succeeded" &&
          previous.operation === "export create"
        )
          model.status = "Ready";
        if (job.status === "failed")
          throw new WorkspaceApiError(
            "render_failed",
            typeof job.failure === "string"
              ? job.failure
              : "Rendering failed. Retry preview or export explicitly.",
            "none",
            isRecord(job.failure) ? job.failure : {},
          );
        if (
          job.status === "succeeded" &&
          previous.operation === "preview render" &&
          previous.requestId === wantedPreviewRequest &&
          model.edit &&
          model.edit.editId === previous.editId &&
          model.edit.revision === previous.revision &&
          !model.draft.length
        ) {
          const result = isRecord(job.result) ? job.result : job;
          const artifact = isRecord(job.artifact)
            ? job.artifact
            : isRecord(result.artifact)
              ? result.artifact
              : undefined;
          const file = artifact
            ? model.files.find(
                (item) =>
                  item.artifactId === artifact.artifactId &&
                  item.revision === artifact.revision,
              )
            : model.files.find(
                (item) =>
                  item.relativePath === result.relativePath ||
                  (typeof result.path === "string" &&
                    result.path.endsWith(`/${item.relativePath}`)),
              );
          if (file && file.state === "ready" && model.workspace) {
            const comparison = isRecord(result.comparison)
              ? result.comparison
              : undefined;
            const comparisonFile = comparison
              ? model.files.find(
                  (item) =>
                    item.artifactId === comparison.artifactId ||
                    (typeof comparison.path === "string" &&
                      comparison.path.endsWith(`/${item.relativePath}`)),
                )
              : undefined;
            model.preview = {
              url: fileBytesUrl(model.workspace.workspaceId, file),
              revision: previous.revision,
              file,
              identity: previous.requestId,
              ...(comparisonFile
                ? {
                    comparisonUrl: fileBytesUrl(
                      model.workspace.workspaceId,
                      comparisonFile,
                    ),
                  }
                : {}),
            };
            model.status = "Ready";
          } else {
            model.status = "Checking file";
            pending = true;
            model.jobs[index] = { ...previous, ...job, status: "checking" };
          }
        }
      } catch (error) {
        failure(error, "Rendering");
      }
    }
    model.previewPending = model.jobs.some(
      (job) =>
        job.operation === "preview render" &&
        !["succeeded", "failed", "cancelled"].includes(job.status),
    );
    publish();
    polling = false;
    if (alive) void renderPendingPreview();
    if (alive && pending)
      timer = setTimeout(() => {
        void pollJobs();
      }, 1000);
  };
  const render = async (
    operation: string,
    args: JsonObject = {},
    captured?: EditView,
  ) => {
    const edit = captured ?? model.edit;
    if (
      !edit ||
      model.blocked ||
      (!captured && (model.draft.length || model.saving))
    )
      return;
    if (operation === "preview render") previewAfterSave = false;
    const payload = {
      ...args,
      revision: edit.revision,
      requestId: requestIdentity(),
    };
    if (operation === "preview render")
      wantedPreviewRequest = String(payload.requestId);
    try {
      const result = await toolCommand(
        fetcher,
        photoId,
        edit.editId,
        operation,
        payload,
      );
      if (
        typeof result.requestId !== "string" ||
        typeof result.status !== "string"
      )
        throw new WorkspaceApiError(
          "outcome_unknown",
          "Render admission is unknown. Replay the same request.",
          "unknown",
          {},
        );
      model.jobs.push(
        parseToolJob({
          ...result,
          operation,
          editId: edit.editId,
          revision: edit.revision,
        }),
      );
      model.status =
        operation === "preview render" ? "Rendering preview" : "Exporting";
      if (operation === "preview render") model.previewPending = true;
      publish();
      void pollJobs();
    } catch (error) {
      if (isUnknownEffect(error)) {
        model.uncertain = {
          operation,
          arguments: payload,
          editId: edit.editId,
        };
        model.blocked = true;
      }
      failure(error, operation);
    }
  };
  const renderPendingPreview = async () => {
    if (previewAfterSave && !barrier && !model.previewPending)
      await render("preview render");
  };
  const drain = async (force = false) => {
    if (savePromise) return savePromise;
    savePromise = (async () => {
      while (
        model.draft.length &&
        model.edit &&
        !model.blocked &&
        (!barrier || force)
      ) {
        const generation = editGeneration;
        const edit = model.edit;
        const changes = model.draft;
        model.draft = [];
        const args = {
          input: { changes },
          ifState: edit.stateToken,
          requestId: requestIdentity(),
        };
        model.saving = true;
        model.status = "Saving settings";
        publish();
        let acknowledged = false;
        try {
          const result = await toolCommand(
            fetcher,
            photoId,
            edit.editId,
            "edit apply",
            args,
          );
          if (
            typeof result.stateToken !== "string" ||
            typeof result.revision !== "string"
          )
            throw new WorkspaceApiError(
              "outcome_unknown",
              "Save acknowledgement is invalid. Replay the exact request.",
              "unknown",
              {},
            );
          if (generation !== editGeneration) continue;
          acknowledged = true;
          if (result.revision !== edit.revision) previewAfterSave = true;
          model.edit = {
            ...edit,
            stateToken: result.stateToken,
            revision: result.revision,
          };
          if (
            isRecord(result.current) &&
            result.current.revision !== result.revision
          ) {
            throw new WorkspaceApiError(
              "state_conflict",
              "Your action was saved, but another writer has advanced this edit. Inspect current settings before reapplying the retained draft.",
              "none",
              result.current,
            );
          }
          await readEdit(edit.editId, generation);
        } catch (error) {
          if (generation === editGeneration) {
            if (
              !acknowledged ||
              (error instanceof WorkspaceApiError &&
                error.code === "state_conflict")
            )
              model.draft = [...changes, ...model.draft];
            model.blocked = true;
            model.conflictInspected = false;
            if (!acknowledged && isUnknownEffect(error))
              model.uncertain = {
                operation: "edit apply",
                arguments: args,
                editId: edit.editId,
              };
            failure(
              error,
              acknowledged ? "Checking saved settings" : "Saving settings",
            );
          }
          break;
        } finally {
          model.saving = false;
          publish();
        }
      }
    })();
    try {
      await savePromise;
    } finally {
      savePromise = undefined;
      void renderPendingPreview();
    }
  };
  const change = (next: EditChange) => {
    if (!model.edit || model.blocked) return;
    model.draft = coalesceChanges(model.draft, next);
    model.comparing = false;
    model.status = "Unsaved settings";
    publish();
    void drain();
  };
  const command = async (
    operation: string,
    args: JsonObject = {},
    guarded = false,
  ) => {
    if (!model.edit) return;
    if (guarded) {
      await drain();
      if (model.blocked || model.draft.length) return;
    }
    const edit = model.edit;
    const payload = guarded
      ? { ...args, ifState: edit.stateToken, requestId: requestIdentity() }
      : args;
    try {
      const result = await toolCommand(
        fetcher,
        photoId,
        edit.editId,
        operation,
        payload,
      );
      if (guarded) await readEdit(edit.editId);
      return result;
    } catch (error) {
      if (guarded && isUnknownEffect(error)) {
        model.uncertain = {
          operation,
          arguments: payload,
          editId: edit.editId,
        };
        model.blocked = true;
      }
      failure(error, operation);
    }
  };
  const act = async (action: string, args: JsonObject = {}) => {
    delete model.error;
    try {
      if (action === "change") {
        if (isRecord(args.change) && typeof args.change.type === "string")
          change(args.change as EditChange);
        return;
      }
      if (action === "refresh") {
        await refresh();
        return;
      }
      if (action === "files-more") {
        await readFiles(true);
        return;
      }
      if (action === "window-more" && model.edit) {
        const result = await command(String(args.operation), {
          cursor: args.cursor,
        });
        if (!result) return;
        const window = parseWindow(result, (value) => {
          if (!isRecord(value)) throw new Error("Invalid tool list item");
          return value;
        });
        if (args.operation === "edit instances list") {
          model.edit.instances = {
            ...window,
            items: [...model.edit.instances.items, ...window.items],
          } as EditView["instances"];
          for (const instance of window.items) {
            model.instanceDetails[String(instance.instanceId)] =
              await toolCommand(
                fetcher,
                photoId,
                model.edit.editId,
                "edit instances show",
                {
                  instanceId: instance.instanceId,
                  ifState: model.edit.stateToken,
                },
              );
            model.descriptions[String(instance.moduleId)] = await toolCommand(
              fetcher,
              photoId,
              model.edit.editId,
              "modules describe",
              { moduleId: instance.moduleId, ifState: model.edit.stateToken },
            );
          }
        }
        publish();
        return;
      }
      if (action === "input") {
        model.selectedInput = String(args.artifactId);
        publish();
        return;
      }
      if (action === "edit-select") {
        if (model.saving || model.draft.length || model.uncertain)
          throw new Error(
            "Save or reconcile this draft before choosing another edit.",
          );
        editGeneration++;
        previewAfterSave = false;
        delete model.preview;
        await readEdit(String(args.editId));
        return;
      }
      if (action === "create") {
        const file = model.files.find(
          (item) => item.artifactId === model.selectedInput,
        );
        if (!file) throw new Error("Select a ready input file first.");
        if (selectingEdit) return;
        const generation = ++editGeneration;
        selectingEdit = true;
        try {
          model.status = "Preparing editing tool";
          publish();
          const result = await toolCommand(
            fetcher,
            photoId,
            null,
            "edit create",
            {
              input: {
                input: {
                  kind: "workspace-artifact",
                  artifactId: file.artifactId,
                  revision: file.revision,
                },
              },
              requestId: requestIdentity(),
            },
          );
          if (typeof result.editId !== "string")
            throw new Error("The tool did not return a saved edit identity.");
          if (generation !== editGeneration) return;
          await readEdit(result.editId, generation);
        } finally {
          selectingEdit = false;
        }
        await refresh();
        return;
      }
      if (action === "inspect") {
        const file = model.files.find(
          (item) => item.artifactId === args.artifactId,
        );
        if (!file || !model.workspace) return;
        model.inspectedFile = {
          ...file,
          ...(await workspaceRequest(
            fetcher,
            `${workspacePath(model.workspace.workspaceId)}/files/${encodeURIComponent(file.artifactId)}`,
          )),
        } as WorkspaceFile;
        publish();
        return;
      }
      if (action === "remove") {
        const file = model.inspectedFile;
        if (
          !file ||
          !model.workspace ||
          args.revision !== file.revision ||
          args.artifactId !== file.artifactId
        )
          throw new Error("Review the selected file revision before removal.");
        await workspaceRequest(
          fetcher,
          `${workspacePath(model.workspace.workspaceId)}/files/${encodeURIComponent(file.artifactId)}`,
          { revision: file.revision, requestId: requestIdentity() },
          "DELETE",
        );
        if (model.preview?.file.artifactId === file.artifactId)
          delete model.preview;
        if (model.selectedInput === file.artifactId) delete model.selectedInput;
        delete model.inspectedFile;
        await refresh();
        return;
      }
      if (action === "refresh-input") {
        if (!model.workspace?.observedSourceRevision)
          throw new Error(
            "Recover the Library source before preparing input again.",
          );
        await workspaceRequest(
          fetcher,
          `${workspacePath(model.workspace.workspaceId)}/refresh-input`,
          {
            expectedSourceRevision: model.workspace.observedSourceRevision,
            requestId: requestIdentity(),
          },
        );
        await refresh();
        return;
      }
      if (action === "preview") {
        await drain();
        await render(
          "preview render",
          args.comparison
            ? { input: { comparison: "baseline", region: null } }
            : {},
        );
        return;
      }
      if (action === "current") {
        model.cameraReference = model.comparing = false;
        publish();
        return;
      }
      if (action === "compare") {
        model.cameraReference = false;
        model.comparing =
          Boolean(args.pressed) &&
          Boolean(model.preview?.comparisonUrl) &&
          model.preview?.revision === model.edit?.revision &&
          !model.draft.length;
        publish();
        return;
      }
      if (action === "camera") {
        model.cameraReference = !model.cameraReference;
        publish();
        return;
      }
      if (action === "export") {
        if (barrier) return;
        barrier = true;
        try {
          await drain(true);
          if (model.draft.length && !model.blocked) await drain(true);
          if (model.blocked || model.draft.length || !model.edit) return;
          const captured = model.edit;
          await render("export create", {}, captured);
        } finally {
          barrier = false;
          if (model.draft.length) void drain();
          else void renderPendingPreview();
        }
        return;
      }
      if (action === "job-check") {
        await pollJobs();
        return;
      }
      if (action === "job-cancel") {
        await workspaceRequest(
          fetcher,
          `${toolPath(photoId)}/jobs/${encodeURIComponent(String(args.requestId))}/cancel`,
          { requestId: requestIdentity() },
        );
        await pollJobs();
        return;
      }
      if (action === "job-retry" && model.edit) {
        const result = await toolCommand(
          fetcher,
          photoId,
          model.edit.editId,
          "export retry",
          { id: args.requestId, requestId: requestIdentity() },
        );
        if (
          typeof result.requestId !== "string" ||
          typeof result.status !== "string"
        )
          throw new WorkspaceApiError(
            "outcome_unknown",
            "Retry admission is uncertain. Inspect retained jobs.",
            "unknown",
            {},
          );
        const original = model.jobs.find(
          (job) => job.requestId === args.requestId,
        );
        model.jobs.push(
          parseToolJob({
            ...result,
            operation: original?.operation ?? "export create",
            revision: isRecord(result.capture)
              ? result.capture.revision
              : result.revision,
          }),
        );
        void pollJobs();
        return;
      }
      if (action === "inspect-current") {
        if (model.edit) {
          await readEdit(model.edit.editId);
          model.conflictInspected = true;
          publish();
        }
        return;
      }
      if (action === "discard") {
        if (model.uncertain)
          throw new Error(
            "Reconcile the exact uncertain request before discarding its intent.",
          );
        model.draft = [];
        model.blocked = false;
        model.conflictInspected = false;
        if (model.edit) await readEdit(model.edit.editId);
        return;
      }
      if (action === "reapply") {
        if (model.uncertain)
          throw new Error(
            "Replay the exact request before reapplying new intent.",
          );
        if (!model.conflictInspected)
          throw new Error(
            "Inspect the current saved settings before choosing to reapply your draft.",
          );
        model.blocked = false;
        model.conflictInspected = false;
        await drain();
        return;
      }
      if (action === "replay") {
        const pending = model.uncertain;
        if (!pending) return;
        const result = await toolCommand(
          fetcher,
          photoId,
          pending.editId,
          pending.operation,
          pending.arguments,
        );
        delete model.uncertain;
        if (
          pending.operation === "edit apply" &&
          isRecord(pending.arguments.input) &&
          Array.isArray(pending.arguments.input.changes)
        )
          model.draft = model.draft.slice(
            (pending.arguments.input.changes as unknown[]).length,
          );
        model.blocked = model.draft.length > 0;
        if (typeof result.status === "string") {
          model.jobs.push(
            parseToolJob({
              ...result,
              operation: pending.operation,
              editId: pending.editId,
              revision: pending.arguments.revision,
            }),
          );
          void pollJobs();
        }
        if (model.edit) await readEdit(model.edit.editId);
        publish();
        return;
      }
      if (action === "catalog") {
        const namespace = String(args.namespace);
        const result = await command(`${namespace} list`, {
          limit: 60,
          ...(typeof args.cursor === "string" ? { cursor: args.cursor } : {}),
        });
        if (result) model.catalogs[namespace] = result;
        publish();
        return;
      }
      if (action === "command") {
        await command(
          String(args.operation),
          isRecord(args.arguments) ? args.arguments : {},
          Boolean(args.guarded),
        );
        publish();
        return;
      }
    } catch (error) {
      failure(error, action);
    }
  };
  return {
    model,
    refresh,
    act,
    command,
    async settle() {
      await drain();
      return !model.blocked && !model.draft.length && !model.uncertain;
    },
    dispose() {
      alive = false;
      clearTimeout(timer);
    },
  };
}
