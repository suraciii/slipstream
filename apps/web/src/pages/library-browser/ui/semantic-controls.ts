import {
  newSemanticValue,
  replaceSemanticValue,
  resolveSemanticSchema,
  sameSemanticValue,
  schemaArray,
  schemaObject,
  selectSemanticBranch,
  semanticItemSchema,
  semanticSchemaError,
  semanticType,
  validateSemanticValue,
  type SemanticPath,
  type SemanticSchema,
} from "../model/semantic-schema.js";

let nextControlId = 0;

/** Native schema controls keep a local draft until a complete change is admitted. */
export function createSemanticControls(
  schema: Record<string, unknown>,
  value: unknown,
  onCommit: (value: unknown) => void,
  options?: { disabled?: boolean; label?: string },
): HTMLElement {
  const root = document.createElement("div");
  root.className = "semantic-controls";
  root.style.cssText =
    "display:grid;gap:.6rem;min-width:0;max-width:100%;overflow-wrap:anywhere";
  let draft = value;
  let committed = value;
  const selectedBranches: Record<string, number> = {};
  const schemaError = semanticSchemaError(schema);
  const status = document.createElement("p");
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  status.style.margin = "0";
  const content = document.createElement("div");
  content.style.cssText = "display:grid;gap:.6rem;min-width:0";
  root.append(content, status);

  function update(
    path: SemanticPath,
    next: unknown,
    remove = false,
    redraw = false,
  ) {
    draft = replaceSemanticValue(draft, path, next, remove);
    const errors = validateSemanticValue(schema, draft);
    status.textContent = errors.length
      ? `Changes are not saved: ${errors.join(" ")}`
      : "";
    if (!errors.length && !sameSemanticValue(draft, committed)) {
      committed = draft;
      onCommit(draft);
    }
    if (redraw) render();
  }

  function renderNode(
    input: SemanticSchema | boolean,
    current: unknown,
    path: SemanticPath,
    label: string,
    required: boolean,
    inheritedDisabled: boolean,
  ): HTMLElement {
    const group = document.createElement("fieldset");
    group.style.cssText =
      "min-width:0;margin:0;padding:.5rem;border:1px solid currentColor;border-radius:.25rem;display:grid;gap:.4rem";
    const legend = document.createElement("legend");
    legend.textContent = `${label}${required ? " (required)" : ""}`;
    group.append(legend);
    if (typeof input === "boolean") {
      const note = document.createElement("p");
      note.textContent = `${input ? "No editable value shape is published" : "Schema does not permit this value"}. Read-only: ${JSON.stringify(current) ?? "absent"}`;
      group.append(note);
      return group;
    }
    const node = resolveSemanticSchema(input, schema);
    const disabled =
      inheritedDisabled ||
      node.readOnly === true ||
      Object.hasOwn(node, "const");
    group.disabled = disabled;
    const title = typeof node.title === "string" ? node.title : label;
    legend.textContent = `${title}${required ? " (required)" : ""}`;
    const id = `semantic-control-${++nextControlId}`;
    const hint = document.createElement("p");
    hint.id = `${id}-hint`;
    hint.style.cssText = "margin:0;font-size:.85em";
    const unit = node.unit ?? node.units ?? node["x-unit"];
    const details: string[] = [];
    if (typeof node.description === "string") details.push(node.description);
    if (typeof unit === "string") details.push(`Unit: ${unit}.`);
    if (typeof node.resourceKind === "string")
      details.push(`Resource: ${node.resourceKind}.`);
    const initialization = node.initializationSchema;
    if (
      schemaObject(initialization) &&
      schemaObject(initialization.properties)
    ) {
      const mode = initialization.properties.mode;
      if (schemaObject(mode) && mode.const === "target-camera")
        details.push("Initializes from the target camera.");
    }
    for (const key of [
      "minimum",
      "maximum",
      "exclusiveMinimum",
      "exclusiveMaximum",
      "minItems",
      "maxItems",
      "minLength",
      "maxLength",
    ])
      if (typeof node[key] === "number") details.push(`${key}: ${node[key]}.`);
    hint.textContent = details.join(" ");
    if (details.length) group.append(hint);

    function button(text: string, action: () => void): HTMLButtonElement {
      const element = document.createElement("button");
      element.type = "button";
      element.textContent = text;
      element.setAttribute("aria-label", `${text}: ${title}`);
      element.disabled = disabled;
      element.addEventListener("click", action);
      return element;
    }

    const nullable = Array.isArray(node.type) && node.type.includes("null");
    if (nullable) {
      if (node.const === null) {
        const note = document.createElement("p");
        note.textContent = `${title} is fixed to null (read-only).`;
        group.append(note);
        return group;
      }
      const presenceLabel = document.createElement("label");
      const presence = document.createElement("input");
      presence.type = "checkbox";
      presence.checked = current !== null && current !== undefined;
      presence.disabled = disabled;
      const creationSchema: SemanticSchema = {
        ...node,
        type: semanticType(node),
      };
      if (creationSchema.default === null) delete creationSchema.default;
      presence.addEventListener("change", () =>
        update(
          path,
          presence.checked ? newSemanticValue(creationSchema, schema) : null,
          false,
          true,
        ),
      );
      presenceLabel.append(presence, document.createTextNode(` Use ${title}`));
      group.append(presenceLabel);
      if (current === null) return group;
    }

    const branches = schemaArray(node.oneOf)
      ? node.oneOf
      : schemaArray(node.anyOf)
        ? node.anyOf
        : undefined;
    if (branches) {
      const branchLabel = document.createElement("label");
      branchLabel.textContent = `${title} variant`;
      branchLabel.htmlFor = id;
      const select = document.createElement("select");
      select.id = id;
      select.style.cssText = "width:100%;min-width:0;max-width:100%";
      select.disabled = disabled;
      select.required = required;
      if (details.length) select.setAttribute("aria-describedby", hint.id);
      const matches = branches
        .map((branch, index) =>
          (schemaObject(branch) || typeof branch === "boolean") &&
          !validateSemanticValue(branch, current, schema).length
            ? index
            : -1,
        )
        .filter((index) => index >= 0);
      const key = JSON.stringify(path);
      const selected =
        selectedBranches[key] ??
        (matches.length === 1 || node.anyOf ? matches[0] : undefined);
      const placeholder = document.createElement("option");
      placeholder.value = "";
      placeholder.textContent =
        selected === undefined
          ? "Choose a schema variant"
          : "Choose another variant";
      select.append(placeholder);
      branches.forEach((branch, index) => {
        const option = document.createElement("option");
        option.value = String(index);
        option.textContent =
          schemaObject(branch) && typeof branch.title === "string"
            ? branch.title
            : `Variant ${index + 1}`;
        select.append(option);
      });
      select.value = selected === undefined ? "" : String(selected);
      select.addEventListener("change", () => {
        if (select.value === "") return;
        const index = Number(select.value);
        const branch = branches[index];
        if (!schemaObject(branch)) return;
        selectedBranches[key] = index;
        update(
          path,
          selectSemanticBranch(current, branch, schema),
          false,
          true,
        );
      });
      group.append(branchLabel, select);
      if (selected !== undefined) {
        const branch = branches[selected];
        if (schemaObject(branch)) {
          const base = { ...node };
          delete base.oneOf;
          delete base.anyOf;
          const resolved = resolveSemanticSchema(branch, schema);
          const combined = { ...base, ...resolved };
          if (
            schemaObject(base.properties) &&
            schemaObject(resolved.properties)
          )
            combined.properties = {
              ...base.properties,
              ...resolved.properties,
            };
          if (schemaArray(base.required) || schemaArray(resolved.required))
            combined.required = [
              ...(schemaArray(base.required) ? base.required : []),
              ...(schemaArray(resolved.required) ? resolved.required : []),
            ];
          group.append(
            renderNode(combined, current, path, title, required, disabled),
          );
        }
      }
      return group;
    }

    if (Object.hasOwn(node, "const")) {
      const fixed = document.createElement("output");
      fixed.textContent = JSON.stringify(current) ?? "Absent";
      fixed.setAttribute("aria-label", `${title} (read-only)`);
      group.append(fixed);
      return group;
    }
    if (Array.isArray(node.enum)) {
      const select = document.createElement("select");
      select.id = id;
      select.setAttribute("aria-label", title);
      if (details.length) select.setAttribute("aria-describedby", hint.id);
      select.style.cssText = "width:100%;min-width:0;max-width:100%";
      select.disabled = disabled;
      select.required = required;
      const absent = document.createElement("option");
      absent.value = "";
      absent.textContent =
        current === undefined
          ? "Choose a value"
          : `Current: ${JSON.stringify(current)}`;
      select.append(absent);
      const choices = node.enum;
      choices.forEach((choice, index) => {
        const option = document.createElement("option");
        option.value = String(index);
        option.textContent =
          typeof choice === "string" ? choice : JSON.stringify(choice);
        select.append(option);
      });
      const selected = choices.findIndex((choice) =>
        sameSemanticValue(choice, current),
      );
      select.value = selected < 0 ? "" : String(selected);
      select.addEventListener("change", () => {
        if (select.value !== "")
          update(path, structuredClone(choices[Number(select.value)]));
      });
      group.append(select);
      return group;
    }

    const type = semanticType(node);
    if (type === "object") {
      if (!schemaObject(current)) {
        const note = document.createElement("p");
        note.textContent =
          current === undefined
            ? "Object is absent."
            : `Current value is not an object and is preserved read-only: ${JSON.stringify(current)}`;
        group.append(note);
        if (current === undefined)
          group.append(
            button("Create object", () =>
              update(path, newSemanticValue(node, schema), false, true),
            ),
          );
        return group;
      }
      const properties = schemaObject(node.properties) ? node.properties : {};
      const requiredFields = Array.isArray(node.required) ? node.required : [];
      for (const [key, child] of Object.entries(properties)) {
        if (!schemaObject(child) && typeof child !== "boolean") continue;
        const isRequired = requiredFields.includes(key);
        const childGroup = renderNode(
          child,
          current[key],
          [...path, key],
          key,
          isRequired,
          disabled,
        );
        if (!isRequired && Object.hasOwn(current, key))
          childGroup.append(
            button(`Remove ${key}`, () =>
              update([...path, key], undefined, true, true),
            ),
          );
        group.append(childGroup);
      }
      for (const [key, retained] of Object.entries(current))
        if (!Object.hasOwn(properties, key)) {
          const note = document.createElement("p");
          note.textContent = `${key} (unexposed, preserved): ${JSON.stringify(retained)}`;
          group.append(note);
        }
      if (!Object.keys(properties).length) {
        const note = document.createElement("p");
        note.textContent =
          "No editable object fields are published. Existing fields are preserved.";
        group.append(note);
      }
      return group;
    }
    if (type === "array") {
      if (!schemaArray(current)) {
        const note = document.createElement("p");
        note.textContent =
          current === undefined
            ? "Array is absent."
            : `Current value is not an array and is preserved read-only: ${JSON.stringify(current)}`;
        group.append(note);
        if (current === undefined)
          group.append(
            button("Create array", () =>
              update(path, newSemanticValue(node, schema), false, true),
            ),
          );
        return group;
      }
      current.forEach((item, index) => {
        const child = semanticItemSchema(node, index);
        if (!schemaObject(child) && typeof child !== "boolean") return;
        const itemGroup = renderNode(
          child,
          item,
          [...path, index],
          `Node ${index + 1}`,
          true,
          disabled,
        );
        const removable =
          current.length >
            (typeof node.minItems === "number" ? node.minItems : 0) &&
          (!Array.isArray(node.prefixItems) ||
            index >= node.prefixItems.length ||
            index === current.length - 1);
        if (removable)
          itemGroup.append(
            button(`Remove node ${index + 1}`, () =>
              update([...path, index], undefined, true, true),
            ),
          );
        group.append(itemGroup);
      });
      const child = semanticItemSchema(node, current.length);
      if (
        schemaObject(child) &&
        (typeof node.maxItems !== "number" || current.length < node.maxItems)
      )
        group.append(
          button("Add node", () =>
            update(
              path,
              [...current, newSemanticValue(child, schema)],
              false,
              true,
            ),
          ),
        );
      return group;
    }
    if (["number", "integer", "string", "boolean"].includes(type ?? "")) {
      const fieldLabel = document.createElement("label");
      fieldLabel.htmlFor = id;
      fieldLabel.textContent = title;
      const input = document.createElement("input");
      input.id = id;
      input.disabled = disabled;
      input.required = required;
      if (details.length) input.setAttribute("aria-describedby", hint.id);
      input.style.cssText =
        "box-sizing:border-box;min-width:0;max-width:100%;width:100%";
      const booleanChoices = type === "boolean" && typeof current !== "boolean";
      if (booleanChoices) {
        const select = document.createElement("select");
        select.id = id;
        select.disabled = disabled;
        select.required = required;
        if (details.length) select.setAttribute("aria-describedby", hint.id);
        select.style.cssText = "min-width:0;max-width:100%;width:100%";
        for (const [key, text] of [
          ["", "Choose true or false"],
          ["true", "True"],
          ["false", "False"],
        ]) {
          const option = document.createElement("option");
          option.value = key ?? "";
          option.textContent = text ?? "";
          select.append(option);
        }
        select.addEventListener("change", () => {
          if (select.value) update(path, select.value === "true", false, true);
        });
        group.append(fieldLabel, select);
        return group;
      }
      input.type =
        type === "boolean" ? "checkbox" : type === "string" ? "text" : "number";
      if (type === "boolean") {
        input.checked = current === true;
        input.style.width = "auto";
      } else
        input.value =
          typeof current === "string" || typeof current === "number"
            ? String(current)
            : "";
      if (type === "number" || type === "integer") {
        input.step =
          typeof node.multipleOf === "number"
            ? String(node.multipleOf)
            : type === "integer"
              ? "1"
              : "any";
        if (typeof node.minimum === "number") input.min = String(node.minimum);
        if (typeof node.maximum === "number") input.max = String(node.maximum);
      }
      if (type === "string") {
        if (typeof node.minLength === "number")
          input.minLength = node.minLength;
        if (typeof node.maxLength === "number")
          input.maxLength = node.maxLength;
      }
      input.addEventListener("change", () => {
        let next: unknown = type === "boolean" ? input.checked : input.value;
        if (type === "number" || type === "integer") {
          if (!input.value.trim() || !Number.isFinite(input.valueAsNumber)) {
            input.setAttribute("aria-invalid", "true");
            update(path, undefined, !required);
            if (required)
              status.textContent =
                "Changes are not saved: enter a finite number.";
            return;
          }
          next = input.valueAsNumber;
        }
        const errors = validateSemanticValue(node, next, schema);
        input.setAttribute("aria-invalid", errors.length ? "true" : "false");
        update(path, next);
      });
      if (
        current !== undefined &&
        validateSemanticValue(node, current, schema).length
      )
        input.setAttribute("aria-invalid", "true");
      group.append(fieldLabel, input);
      return group;
    }
    const note = document.createElement("p");
    note.textContent =
      type === "null"
        ? "Null value."
        : `Unsupported or unexposed value shape (read-only): ${JSON.stringify(current) ?? "absent"}`;
    group.append(note);
    return group;
  }

  function render() {
    if (schemaError) {
      const note = document.createElement("p");
      note.textContent = `Schema error: ${schemaError}`;
      note.setAttribute("role", "alert");
      const retained = document.createElement("pre");
      retained.style.cssText =
        "white-space:pre-wrap;overflow-wrap:anywhere;max-width:100%";
      retained.textContent = JSON.stringify(value, null, 2) ?? "Absent";
      content.replaceChildren(note, retained);
      return;
    }
    content.replaceChildren(
      renderNode(
        schema,
        draft,
        [],
        options?.label ?? "Settings",
        true,
        options?.disabled === true,
      ),
    );
  }
  render();
  if (!schemaError) {
    const errors = validateSemanticValue(schema, draft);
    status.textContent = errors.length
      ? `Current value: ${errors.join(" ")} Values are preserved until you edit them.`
      : "";
  }
  return root;
}
