/// Darktable settings renderer for the Photo editor surface.
///
/// This module owns the DOM structure of the tool settings panel. It retains
/// no state and issues no commands; the controller supplies the target, DOM
/// factories, and change callback.

import { isRecord } from "../api/guards.js";
import type { JsonObject } from "../api/workspace-edit.js";
import type { EditorViewModel } from "../model/editor-controller-contract.js";
import { newSemanticValue } from "../model/semantic-schema.js";
import type { EditChange } from "../model/workspace-edit-model.js";
import { displayScalar } from "./presentation-helpers.js";
import { createSemanticControls } from "./semantic-controls.js";

type EditorControlsDependencies = Readonly<{
  target: HTMLElement;
  button: (
    label: string,
    action: string,
    args?: JsonObject,
    disabled?: boolean,
  ) => HTMLButtonElement;
  paragraph: (text: string) => HTMLParagraphElement;
  select: (
    label: string,
    items: { value: string; label: string }[],
    selected: string | undefined,
    onChange: (value: string) => void,
  ) => HTMLElement;
  change: (value: EditChange) => void;
}>;

export function createEditorControlsRenderer({
  target,
  button,
  paragraph,
  select,
  change,
}: EditorControlsDependencies): (model: EditorViewModel) => void {
  return (model) => {
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
  };
}
