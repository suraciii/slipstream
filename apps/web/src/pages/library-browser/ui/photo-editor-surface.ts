import { createSemanticControls } from "./semantic-controls.js";
import { isRecord } from "../api/guards.js";
import {
  fileBytesUrl,
  type JsonObject,
  type WorkspaceFile,
} from "../api/workspace-edit.js";
import type { EditChange } from "../model/workspace-edit-owner.js";
import type {
  EditorViewModel,
  LibraryBrowserIntent,
} from "./library-browser-view.js";
import "./workspace-editor.css";
import { newSemanticValue } from "../model/semantic-schema.js";
import { canContinueFilm } from "../model/workspace-film.js";
type EditorIntent = Extract<LibraryBrowserIntent, { kind: `editor-${string}` }>;
export interface PhotoEditorSurfaceController {
  open(photoId: string): void;
  visible(): boolean;
  render(model: EditorViewModel): void;
  presentPreview(url: string): void;
  clearPreview(): void;
  dispose(): void;
}

function displayScalar(value: unknown, fallback = ""): string {
  return typeof value === "string" ||
    typeof value === "number" ||
    typeof value === "boolean"
    ? String(value)
    : fallback;
}
export function createPhotoEditorSurfaceController({
  root,
  send,
  isPhotoVisible,
  isEditorSurfaceVisible,
  openEditorSurface,
}: {
  root: ParentNode;
  send: (intent: EditorIntent) => void;
  isPhotoVisible: () => boolean;
  isEditorSurfaceVisible: () => boolean;
  openEditorSurface: () => void;
}): PhotoEditorSurfaceController {
  let alive = true;
  let photoId: string | undefined;
  let current: EditorViewModel | undefined;
  let controlsIdentity = "";
  const element = <T extends HTMLElement>(selector: string): T => {
    const found = root.querySelector<T>(selector);
    if (!found) throw new Error(`Missing editor element ${selector}`);
    return found;
  };
  const image = element<HTMLImageElement>("[data-photo-editor-preview-image]");
  const referenceImage = element<HTMLImageElement>(
    "[data-photo-editor-reference-image]",
  );
  const empty = element<HTMLElement>("[data-photo-editor-image-empty]");
  const act = (action: string, args: JsonObject = {}) => {
    if (alive && photoId)
      send({
        kind: "editor-workspace-action",
        photoId,
        action,
        arguments: args,
      });
  };
  const viewListeners = new AbortController();
  const currentButton = element<HTMLButtonElement>(
    "[data-photo-editor-current]",
  );
  const cameraButton = element<HTMLButtonElement>(
    "[data-photo-editor-camera-reference]",
  );
  const compareButton = element<HTMLButtonElement>(
    "[data-photo-editor-compare]",
  );
  currentButton.addEventListener("click", () => act("current"), {
    signal: viewListeners.signal,
  });
  cameraButton.addEventListener("click", () => act("camera"), {
    signal: viewListeners.signal,
  });
  compareButton.addEventListener(
    "click",
    () => act("compare", { pressed: !current?.comparing }),
    { signal: viewListeners.signal },
  );
  const button = (
    label: string,
    action: string,
    args: JsonObject = {},
    disabled = false,
  ) => {
    const control = document.createElement("button");
    control.type = "button";
    control.textContent = label;
    control.disabled = disabled;
    control.addEventListener("click", () => act(action, args));
    return control;
  };
  const paragraph = (text: string) => {
    const node = document.createElement("p");
    node.textContent = text;
    return node;
  };
  const change = (value: EditChange) => {
    act("change", { change: value });
  };
  const select = (
    label: string,
    items: { value: string; label: string }[],
    selected: string | undefined,
    onChange: (value: string) => void,
  ) => {
    const wrapper = document.createElement("label");
    wrapper.append(label);
    const control = document.createElement("select");
    control.setAttribute("aria-label", label);
    control.append(new Option("Choose…", ""));
    for (const item of items)
      control.append(new Option(item.label, item.value));
    control.value = selected ?? "";
    control.addEventListener("change", () => {
      if (control.value) onChange(control.value);
    });
    wrapper.append(control);
    return wrapper;
  };
  const fileCard = (file: WorkspaceFile, model: EditorViewModel) => {
    const card = document.createElement("article");
    card.dataset.artifactId = file.artifactId;
    card.append(
      paragraph(
        `${file.relativePath} · ${file.kind} · ${file.byteLength === null ? "Unknown size" : `${file.byteLength.toLocaleString()} bytes`} · ${file.state === "checking" ? "Checking file" : file.state}`,
      ),
    );
    if (isRecord(file.imageContract)) {
      const geometry = isRecord(file.imageContract.geometry)
        ? file.imageContract.geometry
        : file.imageContract;
      card.append(
        paragraph(
          `${displayScalar(geometry.width, "?")} × ${displayScalar(geometry.height, "?")} · ${displayScalar(file.imageContract.format)} · ${displayScalar(file.imageContract.precision)} · ${displayScalar(file.imageContract.transfer)}`,
        ),
      );
    }
    if (model.workspace && file.state === "ready") {
      const download = document.createElement("a");
      download.textContent = "Download";
      download.href = fileBytesUrl(model.workspace.workspaceId, file);
      download.download = file.relativePath.split("/").at(-1) ?? "image";
      card.append(download);
      const format = isRecord(file.imageContract)
        ? displayScalar(file.imageContract.format).toLowerCase()
        : "";
      if (["png", "jpeg", "jpg", "webp"].includes(format)) {
        const thumbnail = document.createElement("img");
        thumbnail.loading = "lazy";
        thumbnail.alt = file.relativePath;
        thumbnail.src = download.href;
        thumbnail.className = "workspace-file-thumbnail";
        card.prepend(thumbnail);
      }
      if (file.kind === "output") {
        const compatible =
          isRecord(file.imageContract) &&
          model.filmModules?.some((module) =>
            canContinueFilm(file.imageContract!, module),
          );
        if (compatible)
          card.append(
            button("Continue processing with Film", "continue-film", {
              artifactId: file.artifactId,
              revision: file.revision,
            }),
          );
        else
          card.append(
            paragraph(
              model.filmModules
                ? "No available compatible next tool. Download remains available."
                : "Checking compatible processing tools…",
            ),
          );
      }
    }
    card.append(
      button("Inspect / Remove…", "inspect", { artifactId: file.artifactId }),
    );
    return card;
  };
  const renderControls = (model: EditorViewModel) => {
    const target = element("[data-workspace-controls]");
    target.replaceChildren();
    const edit = model.edit;
    if (!edit) return;
    const blocked = model.loading || model.blocked || !edit.availability.write;
    if (!edit.availability.write)
      target.append(
        paragraph(
          `${edit.availability.reason ?? "Settings are read-only"}. ${edit.availability.recovery ?? "Inspect tool availability."}`,
        ),
      );
    const heading = document.createElement("h4");
    heading.textContent = "darktable settings";
    target.append(heading);
    target.append(
      button(
        "Reset this edit to initialized settings",
        "change",
        { change: { type: "reset", scope: "all" } },
        blocked,
      ),
    );
    const moduleChoice = select(
      "Add operation",
      model.modules.map((module) => ({
        value: displayScalar(module.moduleId),
        label: displayScalar(module.name, displayScalar(module.moduleId)),
      })),
      undefined,
      (moduleId) =>
        change({
          type: "instance-add",
          moduleId,
          ref: `new_${Date.now()}`,
          name: null,
        }),
    );
    target.append(moduleChoice);
    const instances = [...edit.instances.items].sort(
      (a, b) => a.order - b.order,
    );
    for (const [position, instance] of instances.entries()) {
      const section = document.createElement("details");
      section.open = position === 0;
      section.dataset.instanceId = instance.instanceId;
      const summary = document.createElement("summary");
      summary.textContent = `${instance.name} · ${instance.moduleId}`;
      section.append(summary);
      const detail = model.instanceDetails[instance.instanceId] ?? instance;
      const descriptor = model.descriptions[instance.moduleId];
      const enabled = document.createElement("input");
      enabled.type = "checkbox";
      enabled.checked = instance.enabled;
      enabled.disabled = blocked;
      enabled.addEventListener("change", () =>
        change({
          type: "instance-enable",
          instanceId: instance.instanceId,
          enabled: enabled.checked,
        }),
      );
      const enabledLabel = document.createElement("label");
      enabledLabel.append(enabled, "Enabled");
      section.append(enabledLabel);
      section.append(
        button(
          "Duplicate",
          "change",
          {
            change: {
              type: "instance-clone",
              instanceId: instance.instanceId,
              ref: `clone_${position}`,
              name: null,
            },
          },
          blocked,
        ),
        button(
          "Reset operation",
          "change",
          {
            change: {
              type: "reset",
              scope: "instance",
              instanceId: instance.instanceId,
            },
          },
          blocked,
        ),
        button(
          "Remove operation",
          "change",
          {
            change: {
              type: "instance-remove",
              instanceId: instance.instanceId,
            },
          },
          blocked,
        ),
      );
      for (const [label, offset] of [
        ["Move earlier", -1],
        ["Move later", 1],
      ] as const) {
        const ids = instances.map((item) => item.instanceId);
        const next = position + offset;
        if (next >= 0 && next < ids.length)
          [ids[position], ids[next]] = [ids[next]!, ids[position]!];
        section.append(
          button(
            label,
            "change",
            { change: { type: "instance-reorder", instanceIds: ids } },
            blocked || next < 0 || next >= ids.length,
          ),
        );
      }
      const parameters = isRecord(detail.parameters) ? detail.parameters : {};
      const described: JsonObject[] =
        descriptor &&
        isRecord(descriptor.parameters) &&
        isRecord(descriptor.parameters.properties)
          ? Object.entries(descriptor.parameters.properties).map(
              ([id, schema]) => ({
                parameterId: id,
                name:
                  isRecord(schema) && typeof schema.title === "string"
                    ? schema.title
                    : id,
                valueSchema: schema,
              }),
            )
          : [];
      if (!descriptor || !described.length)
        section.append(
          paragraph(
            "Schema unavailable: inspect the installed module description. Saved parameters remain preserved.",
          ),
        );
      for (const parameter of described) {
        const id = displayScalar(parameter.parameterId ?? parameter.id);
        const schema = isRecord(parameter.valueSchema)
          ? parameter.valueSchema
          : isRecord(parameter.schema)
            ? parameter.schema
            : parameter;
        let draft: EditChange | undefined;
        for (let index = model.draft.length - 1; index >= 0; index--) {
          const item = model.draft[index];
          if (
            item?.type === "set" &&
            item.instanceId === instance.instanceId &&
            item.parameterId === id
          ) {
            draft = item;
            break;
          }
        }
        const writable = isRecord(parameter.availability)
          ? parameter.availability.write === true
          : true;
        const control = createSemanticControls(
          schema,
          draft ? draft.value : parameters[id],
          (value) =>
            change({
              type: "set",
              instanceId: instance.instanceId,
              parameterId: id,
              value,
            }),
          {
            disabled: blocked || !writable,
            label: displayScalar(parameter.name, id),
          },
        );
        control.dataset.parameterId = id;
        section.append(
          control,
          button(
            `Reset ${displayScalar(parameter.name, id)}`,
            "change",
            {
              change: {
                type: "reset",
                scope: "parameter",
                instanceId: instance.instanceId,
                parameterId: id,
              },
            },
            blocked || !writable,
          ),
        );
      }
      if (
        descriptor &&
        isRecord(descriptor.blendSchema) &&
        isRecord(detail.blend)
      )
        section.append(
          createSemanticControls(
            descriptor.blendSchema,
            detail.blend,
            (value) =>
              change({
                type: "blend-set",
                instanceId: instance.instanceId,
                value,
              }),
            { label: "Blending and mask references", disabled: blocked },
          ),
        );
      target.append(section);
    }
    if (edit.instances.nextCursor)
      target.append(
        button("More operations", "window-more", {
          operation: "edit instances list",
          cursor: edit.instances.nextCursor,
        }),
      );
    const masks = document.createElement("details");
    masks.append(
      Object.assign(document.createElement("summary"), {
        textContent: "Masks",
      }),
    );
    const graphSchema =
      model.maskGraphSchema ??
      Object.values(model.maskDetails).find((detail) =>
        isRecord(detail.graphSchema),
      )?.graphSchema;
    const graphProperties =
      isRecord(graphSchema) && isRecord(graphSchema.properties)
        ? graphSchema.properties
        : {};
    const maskArray = isRecord(graphProperties.masks)
      ? graphProperties.masks
      : {};
    const maskItems = isRecord(maskArray.items) ? maskArray.items : {};
    const maskBranches = Array.isArray(maskItems.oneOf)
      ? maskItems.oneOf.filter(isRecord)
      : [];
    for (const mask of edit.masks.items) {
      const detail = model.maskDetails[displayScalar(mask.maskId)];
      masks.append(
        paragraph(
          `${displayScalar(mask.kind)} · ${displayScalar(mask.frameId)} · ${displayScalar(mask.maskId)}`,
        ),
      );
      const branch = maskBranches.find(
        (item) =>
          isRecord(item.properties) &&
          isRecord(item.properties.kind) &&
          item.properties.kind.const === mask.kind,
      );
      const schema =
        detail && isRecord(detail.schema)
          ? detail.schema
          : branch &&
              isRecord(branch.properties) &&
              isRecord(branch.properties.value)
            ? branch.properties.value
            : undefined;
      if (detail && schema)
        masks.append(
          createSemanticControls(
            schema,
            detail.value,
            (value) => change({ type: "mask-set", maskId: mask.maskId, value }),
            { label: "Mask shape", disabled: blocked },
          ),
        );
      else
        masks.append(
          paragraph("Mask schema unavailable. Saved mask remains preserved."),
        );
      masks.append(
        button(
          "Remove mask",
          "change",
          { change: { type: "mask-remove", maskId: mask.maskId } },
          blocked,
        ),
      );
    }
    for (const branch of maskBranches) {
      const properties = isRecord(branch.properties) ? branch.properties : {};
      const kind = isRecord(properties.kind)
        ? properties.kind.const
        : undefined;
      const valueSchema = isRecord(properties.value)
        ? properties.value
        : undefined;
      if (typeof kind !== "string" || !valueSchema) continue;
      let frameId =
        typeof edit.frames[0]?.frameId === "string"
          ? edit.frames[0].frameId
          : undefined;
      let maskValue = newSemanticValue(valueSchema);
      const form = createSemanticControls(
        valueSchema,
        maskValue,
        (value) => {
          maskValue = value;
        },
        { label: `New ${kind}`, disabled: blocked },
      );
      const frames = select(
        "Mask coordinate frame",
        edit.frames.map((frame) => ({
          value: displayScalar(frame.frameId),
          label: `${displayScalar(frame.frameId)} · ${displayScalar(frame.width, "?")} × ${displayScalar(frame.height, "?")}`,
        })),
        frameId,
        (value) => {
          frameId = value;
        },
      );
      const add = document.createElement("button");
      add.type = "button";
      add.textContent = `Create ${kind} mask`;
      add.disabled = blocked || !frameId;
      add.addEventListener("click", () =>
        change({
          type: "mask-create",
          ref: `mask_${Date.now()}`,
          kind,
          frameId,
          value: maskValue,
        }),
      );
      masks.append(frames, form, add);
    }
    target.append(masks);
    for (const action of edit.actions.items) {
      const wrapper = document.createElement("details");
      wrapper.append(
        Object.assign(document.createElement("summary"), {
          textContent: displayScalar(
            action.name,
            displayScalar(action.actionId),
          ),
        }),
      );
      let input: unknown = {};
      if (isRecord(action.inputSchema))
        wrapper.append(
          createSemanticControls(
            action.inputSchema,
            {},
            (value) => {
              input = value;
            },
            { label: "Action inputs", disabled: blocked },
          ),
        );
      const run = document.createElement("button");
      run.type = "button";
      run.textContent = "Run action";
      run.disabled =
        blocked ||
        !isRecord(action.availability) ||
        action.availability.execute !== true;
      run.addEventListener("click", () =>
        change({
          type: "action",
          actionId: action.actionId,
          instanceId: action.instanceId ?? null,
          input,
        }),
      );
      wrapper.append(run);
      target.append(wrapper);
    }
    const output = document.createElement("details");
    output.append(
      Object.assign(document.createElement("summary"), {
        textContent: "Output format, color and dimensions",
      }),
    );
    const properties = isRecord(edit.output.schema.properties)
      ? edit.output.schema.properties
      : {};
    for (const [fieldId, schema] of Object.entries(properties))
      if (isRecord(schema))
        output.append(
          createSemanticControls(
            schema,
            edit.output.settings[fieldId],
            (value) => change({ type: "output-set", fieldId, value }),
            {
              label: typeof schema.title === "string" ? schema.title : fieldId,
              disabled: blocked || !edit.output.availability.write,
            },
          ),
        );
    if (!Object.keys(properties).length)
      output.append(
        paragraph(
          "Output schema has no supported fields. Inspect the tool output capability.",
        ),
      );
    target.append(output);
    const history = document.createElement("details");
    history.append(
      Object.assign(document.createElement("summary"), {
        textContent: "Saved history / Undo",
      }),
    );
    for (const entry of model.history)
      history.append(
        button(
          `Restore ${displayScalar(entry.revision, "Unknown revision")} ${displayScalar(entry.createdAt)}`,
          "command",
          {
            operation: "history restore",
            arguments: { revision: entry.revision },
            guarded: true,
          },
          blocked,
        ),
      );
    target.append(history);
  };
  return {
    open(id) {
      if (!alive || !id || !isPhotoVisible()) return;
      photoId = id;
      current = undefined;
      controlsIdentity = "";
      image.hidden = true;
      image.removeAttribute("src");
      referenceImage.hidden = true;
      referenceImage.removeAttribute("src");
      empty.hidden = false;
      openEditorSurface();
      send({ kind: "editor-open", photoId: id });
    },
    visible: () => alive && isPhotoVisible() && isEditorSurfaceVisible(),
    render(model) {
      if (!alive || photoId !== model.photoId) return;
      current = model;
      element("[data-photo-editor-status]").textContent = model.status;
      const input = model.files.find(
        (file) => file.artifactId === model.selectedInput,
      );
      element("[data-photo-editor-input]").textContent = input
        ? `Input: ${input.relativePath}`
        : "No default input selected. Select an existing compatible file or prepare input again.";
      const stale = Boolean(
        model.preview &&
          (model.preview.revision !== model.edit?.revision ||
            model.draft.length),
      );
      element("[data-photo-editor-render-status]").textContent =
        model.cameraReference
          ? "Camera Preview · Original reference"
          : model.preview
            ? `${model.comparing ? "Initialized baseline comparison" : "Edit preview"} · ${stale ? "Previous saved revision · out of date" : model.previewPending ? "Updating" : "Ready"}`
            : model.edit
              ? "Edit preview has not been rendered"
              : "Start editing · no Edit State yet";
      currentButton.disabled = !model.preview;
      cameraButton.setAttribute("aria-pressed", String(model.cameraReference));
      compareButton.disabled = !model.preview?.comparisonUrl || stale;
      compareButton.setAttribute("aria-pressed", String(model.comparing));
      const actions = element("[data-workspace-editor-actions]");
      actions.replaceChildren(
        button("Reload workspace", "refresh", {}, model.loading),
        button(
          "Preview",
          "preview",
          {},
          !model.edit?.availability.execute ||
            model.blocked ||
            model.previewPending,
        ),
        button(
          "Render baseline comparison",
          "preview",
          { comparison: true },
          !model.edit?.availability.execute ||
            model.blocked ||
            model.previewPending,
        ),
        button(
          "Export",
          "export",
          {},
          !model.edit?.availability.execute || model.blocked,
        ),
      );
      const errors = element("[data-workspace-error]");
      errors.replaceChildren();
      errors.hidden = !model.error && !model.blocked;
      if (model.error) {
        errors.append(
          paragraph(`${model.error.message} (${model.error.code})`),
        );
        const blockers = model.error.details.blockers;
        if (Array.isArray(blockers))
          for (const blocker of blockers)
            if (isRecord(blocker))
              errors.append(
                paragraph(
                  `${displayScalar(blocker.name, "Unknown blocker")}: ${displayScalar(blocker.action, "Inspect blocker details")}`,
                ),
              );
      }
      if (model.blocked)
        errors.append(
          button("Inspect current saved settings", "inspect-current"),
          button(
            "Reapply retained draft to inspected state",
            "reapply",
            {},
            Boolean(model.uncertain) || !model.conflictInspected,
          ),
          button("Discard draft", "discard", {}, Boolean(model.uncertain)),
        );
      if (model.uncertain)
        errors.append(button("Replay exact request", "replay"));
      const selection = element("[data-workspace-selection]");
      selection.replaceChildren(
        select(
          "Workspace input",
          model.files
            .filter((file) => file.state === "ready" && file.kind !== "preview")
            .map((file) => ({
              value: file.artifactId,
              label: file.relativePath,
            })),
          model.selectedInput,
          (artifactId) => act("input", { artifactId }),
        ),
      );
      const edits =
        model.workspace?.edits.filter((edit) => edit.tool === "darktable") ??
        [];
      if (edits.length)
        selection.append(
          select(
            "Saved edit",
            edits.map((edit) => ({ value: edit.editId, label: edit.name })),
            model.edit?.editId,
            (editId) => act("edit-select", { editId }),
          ),
        );
      selection.append(
        button(
          model.edit ? "Start separate darktable edit" : "Edit with darktable",
          "create",
          {},
          !model.selectedInput ||
            model.loading ||
            model.saving ||
            model.draft.length > 0,
        ),
        button(
          "Prepare input again",
          "refresh-input",
          {},
          !model.workspace?.observedSourceRevision,
        ),
      );
      const identity = JSON.stringify([
        model.edit?.editId,
        model.edit?.revision,
        model.blocked,
        model.loading,
        model.catalogs,
        model.draft,
        model.instanceDetails,
        model.maskDetails,
      ]);
      if (identity !== controlsIdentity) {
        controlsIdentity = identity;
        renderControls(model);
      }
      const jobs = element("[data-workspace-jobs]");
      jobs.replaceChildren();
      for (const job of model.jobs) {
        const row = document.createElement("article");
        row.dataset.requestId = job.requestId;
        row.append(
          paragraph(
            `${String(job.operation)} · ${job.status} · saved revision ${String(job.revision)}`,
          ),
          button("Check status", "job-check"),
          button(
            "Cancel",
            "job-cancel",
            { requestId: job.requestId },
            ["succeeded", "failed", "cancelled"].includes(job.status),
          ),
        );
        if (["failed", "cancelled"].includes(job.status))
          row.append(
            button("Retry captured settings", "job-retry", {
              requestId: job.requestId,
            }),
          );
        if (isRecord(job.result) && isRecord(job.result.error))
          row.append(
            paragraph(
              displayScalar(
                job.result.error.message,
                "Processing failed. Inspect job details.",
              ),
            ),
          );
        jobs.append(row);
      }
      const files = element("[data-workspace-file-groups]");
      files.replaceChildren();
      for (const [kind, name] of [
        ["input", "Inputs"],
        ["preview", "Previews"],
        ["output", "Outputs"],
        ["file", "Other files"],
        ["resource", "Resources"],
      ]) {
        const group = document.createElement("section");
        group.append(
          Object.assign(document.createElement("h4"), { textContent: name }),
        );
        for (const file of model.files.filter((file) => file.kind === kind))
          group.append(fileCard(file, model));
        files.append(group);
      }
      if (model.fileCursor) files.append(button("More files", "files-more"));
      const inspection = element("[data-workspace-file-inspection]");
      inspection.replaceChildren();
      if (model.inspectedFile) {
        const file = model.inspectedFile;
        inspection.append(
          paragraph(
            `Reviewed for removal: ${file.relativePath}. Removal preserves the Library Original. Dependent work may refuse removal.`,
          ),
          button("Remove this reviewed file", "remove", {
            artifactId: file.artifactId,
            revision: file.revision,
          }),
        );
        const details = document.createElement("pre");
        details.textContent = JSON.stringify(file, null, 2);
        inspection.append(details);
      }
      element("[data-workspace-details]").textContent = JSON.stringify(
        {
          workspace: model.workspace,
          input: model.edit?.input,
          revision: model.edit?.revision,
          bundleId: model.edit?.bundleId,
          schemaVersion: model.edit?.schemaVersion,
        },
        null,
        2,
      );
      if (model.filmElement)
        element("[data-workspace-film]").replaceChildren(model.filmElement);
      if (model.catalogElement)
        element("[data-workspace-catalog]").replaceChildren(
          model.catalogElement,
        );
      image.hidden = true;
      referenceImage.hidden = true;
      empty.hidden = Boolean(model.preview || model.cameraReference);
      if (model.preview) {
        image.src = model.preview.url;
        image.alt = model.comparing
          ? "Initialized tool baseline comparison"
          : "Selected workspace edit preview";
        image.hidden = model.cameraReference;
        empty.hidden = true;
      } else image.removeAttribute("src");
      if (model.cameraReference) {
        const reference =
          root.querySelector<HTMLImageElement>("[data-stage] img");
        if (reference?.src) {
          referenceImage.src = reference.src;
          referenceImage.alt = "Camera Preview · Original reference";
          referenceImage.hidden = false;
          empty.hidden = true;
        } else {
          referenceImage.removeAttribute("src");
          empty.textContent =
            "The camera Preview for this Photo is unavailable. No image has been substituted.";
          empty.hidden = false;
        }
      } else {
        empty.textContent = model.preview
          ? ""
          : model.edit
            ? "No Edit Preview is available yet. Refresh the preview when processing is ready."
            : "No Edit State yet. Choose an available Processing Engine to start, or inspect the Original reference.";
      }
    },
    presentPreview(url) {
      if (alive && current && !current.cameraReference) {
        image.src = url;
        image.hidden = false;
        empty.hidden = true;
      }
    },
    clearPreview() {
      image.hidden = true;
      image.removeAttribute("src");
      referenceImage.hidden = true;
      referenceImage.removeAttribute("src");
      empty.hidden = false;
    },
    dispose() {
      viewListeners.abort();
      alive = false;
      image.removeAttribute("src");
      referenceImage.removeAttribute("src");
    },
  };
}
