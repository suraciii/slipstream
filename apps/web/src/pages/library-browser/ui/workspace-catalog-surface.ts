import type { BrowserFetch } from "../api/browser-fetch.js";
import {
  toolCommand,
  toolPath,
  workspaceRequest,
  parseEdit,
  type EditView,
  type EditInstance,
  type JsonObject,
} from "../api/workspace-edit.js";
import { isRecord } from "../api/guards.js";
import {
  createCatalogMutation,
  catalogMapping,
} from "../model/workspace-catalog.js";
import {
  validateSemanticValue,
  resolveSemanticSchema,
  replaceSemanticValue,
  schemaArray,
} from "../model/semantic-schema.js";
import {
  objects,
  object,
  text,
  node,
  choice,
  check,
} from "./workspace-catalog-controls.js";

export interface WorkspaceCatalogSurface {
  element: HTMLElement;
  bind(
    edit: EditView,
    instances: EditInstance[],
    details: Record<string, JsonObject>,
    resourceKinds: JsonObject[],
  ): void;
  dispose(): void;
}
type Family = "resources" | "presets" | "styles";

export function createWorkspaceCatalogSurface(
  fetcher: BrowserFetch,
  photoId: string,
  onMutation: () => Promise<void>,
  beforeMutation?: () => Promise<boolean>,
): WorkspaceCatalogSurface {
  const element = node("section");
  element.className = "workspace-catalog";
  const heading = node("h3", "Resources, presets and styles"),
    status = node("p"),
    disclosure = node("p"),
    toolbar = node("div"),
    list = node("div"),
    inspector = node("div"),
    capture = node("div"),
    recovery = node("div");
  status.setAttribute("role", "status");
  const family = choice(
    "Collection",
    [
      ["resources", "Resources"],
      ["presets", "Presets"],
      ["styles", "Styles"],
    ],
    "presets",
  );
  const scope = choice(
    "Catalog",
    [
      ["local", "Edit local"],
      ["global", "Global on execution host"],
    ],
    "local",
  );
  toolbar.append(family.wrapper, scope.wrapper);
  element.append(
    heading,
    toolbar,
    disclosure,
    status,
    recovery,
    list,
    inspector,
    capture,
  );
  let alive = true,
    edit: EditView | undefined,
    instances: EditInstance[] = [],
    details: Record<string, JsonObject> = {},
    kinds: JsonObject[] = [];
  let readGeneration = 0,
    preparing = false,
    conflictInspected = false,
    nextCursor: string | null = null,
    records: JsonObject[] = [],
    shown: JsonObject | undefined,
    boundId = "";
  const mutation = createCatalogMutation(fetcher, photoId, async () => {
    if (alive) await onMutation();
  });
  const collection = () => family.input.value as Family;
  const scopedEdit = () =>
    scope.input.value === "local" ? (edit?.editId ?? null) : null;
  const idKey = () =>
    collection() === "resources"
      ? "resourceId"
      : collection() === "presets"
        ? "presetId"
        : "styleId";
  function button(
    label: string,
    action: () => void | Promise<void>,
    parent: HTMLElement,
    write = false,
  ) {
    const result = node("button", label);
    result.type = "button";
    result.disabled =
      write &&
      (!edit?.availability.write ||
        !!mutation.state().pending ||
        mutation.state().busy);
    result.addEventListener("click", () => {
      void Promise.resolve()
        .then(action)
        .catch((error) => {
          if (alive) {
            status.textContent = String(
              error instanceof Error ? error.message : error,
            );
            renderRecovery();
          }
        });
    });
    result.addEventListener(
      "click",
      (event) => {
        if (!alive) event.stopImmediatePropagation();
      },
      { capture: true },
    );
    parent.append(result);
    return result;
  }
  function describeCatalog(value: unknown) {
    const catalog = object(value);
    disclosure.textContent = `Selected ${scope.input.value} catalog: ${text(catalog.catalogId) || "not yet inspected"}; scope ${text(catalog.scope) || "unknown"}; execution host ${text(catalog.executionHost) || "unknown"}. Global records belong to the executing host; local records belong to this edit.`;
  }
  function renderRecovery() {
    recovery.replaceChildren();
    const retained = mutation.state();
    const locked = preparing || !!retained.pending || retained.busy;
    family.input.disabled = locked;
    scope.input.disabled = locked;
    list.querySelectorAll<HTMLButtonElement>("button").forEach((control) => {
      control.disabled = locked;
    });
    for (const region of [list, inspector, capture])
      region
        .querySelectorAll<HTMLButtonElement>("button[data-mutation]")
        .forEach((control) => {
          control.disabled = locked || !edit?.availability.write;
        });
    for (const region of [inspector, capture])
      region
        .querySelectorAll<HTMLInputElement | HTMLSelectElement>("input, select")
        .forEach((control) => {
          control.disabled = locked || control.dataset.unavailable === "true";
        });
    if (!retained.pending) return;
    recovery.append(
      node(
        "p",
        `Retained request ${text(retained.pending.arguments.requestId)} against state ${text(retained.pending.arguments.ifState) || "catalog receipt"}. Your choices are retained.`,
      ),
    );
    button(
      "Replay exact request",
      async () => {
        await mutation.replay();
        if (alive) {
          renderRecovery();
          await load();
        }
      },
      recovery,
    ).disabled = retained.busy;
    if (retained.error?.effect !== "unknown" && !retained.busy) {
      button(
        "Inspect current state",
        async () => {
          if (!edit) return;
          const current = parseEdit(
            await toolCommand(fetcher, photoId, edit.editId, "edit show"),
          );
          const refreshed = await Promise.all(
            current.instances.items.map(async (instance) => ({
              instanceId: instance.instanceId,
              detail: await toolCommand(
                fetcher,
                photoId,
                current.editId,
                "edit instances show",
                {
                  instanceId: instance.instanceId,
                  ifState: current.stateToken,
                },
              ),
            })),
          );
          if (alive) {
            edit = current;
            instances = current.instances.items;
            for (const item of refreshed)
              details[item.instanceId] = {
                ...item.detail,
                descriptor: details[item.instanceId]?.descriptor,
              };
            status.textContent = `Current state ${current.stateToken} inspected. Draft retained; review before explicit reapply.`;
            conflictInspected = true;
            renderRecovery();
          }
        },
        recovery,
      );
      button(
        "Keep draft and use newly inspected state",
        () => {
          mutation.discard();
          conflictInspected = false;
          status.textContent =
            "Draft retained. Review the current state, then submit explicitly.";
          renderRecovery();
        },
        recovery,
      ).disabled = !conflictInspected;
    }
  }
  function mutationButton(
    label: string,
    action: () => Promise<void>,
    parent: HTMLElement,
  ) {
    const result = button(label, action, parent, true);
    result.dataset.mutation = "true";
    return result;
  }
  async function mutate(
    operation: string,
    args: () => JsonObject,
    catalogOnly = false,
  ) {
    if (preparing || mutation.state().pending || mutation.state().busy)
      throw new Error("Resolve the retained request first.");
    conflictInspected = false;
    preparing = true;
    renderRecovery();
    try {
      if (beforeMutation && !(await beforeMutation()))
        throw new Error(
          "Resolve pending editor changes before changing the catalog.",
        );
      if (!alive || !edit) return;
      const arguments_ = args();
      if (!catalogOnly) {
        arguments_.ifState = edit.stateToken;
        arguments_.catalog = scope.input.value;
      }
      status.textContent = "Submitting catalog action…";
      const promise = mutation.submit({
        editId: catalogOnly ? scopedEdit() : edit.editId,
        operation,
        arguments: arguments_,
        global: catalogOnly && scope.input.value === "global",
      });
      preparing = false;
      renderRecovery();
      const result = await promise;
      if (alive) {
        status.textContent = `Confirmed ${text(result?.outcome)}.`;
        renderRecovery();
        await load();
      }
    } finally {
      preparing = false;
      if (alive) renderRecovery();
    }
  }
  async function read(operation: string, args: JsonObject = {}) {
    if (scope.input.value === "global")
      return workspaceRequest(fetcher, "/api/darktable/commands", {
        operation,
        editId: null,
        arguments: args,
      });
    return toolCommand(fetcher, photoId, scopedEdit(), operation, args);
  }
  async function load(continuation = false) {
    if (!edit) return;
    const generation = ++readGeneration,
      selectedFamily = collection();
    {
      const result = await read(
        `${selectedFamily} list`,
        continuation && nextCursor ? { cursor: nextCursor } : { limit: 50 },
      );
      if (!alive || generation !== readGeneration) return;
      records = continuation
        ? [...records, ...objects(result.items)]
        : objects(result.items);
      nextCursor =
        typeof result.nextCursor === "string" ? result.nextCursor : null;
      describeCatalog(result.catalog);
      renderList();
      status.textContent = `${records.length} of ${String(result.total)} records loaded.`;
    }
  }
  function renderList() {
    list.replaceChildren();
    for (const record of records) {
      const row = node("div");
      row.append(
        node(
          "span",
          `${text(record.name)} · ${text(record.id)} · ${text(record.version)} · ${text(record.kind)} · ${text(object(record.availability).reason) || text(object(record.availability).status)}`,
        ),
      );
      button(
        "Show",
        async () => {
          const generation = ++readGeneration;
          const result = await read(`${collection()} show`, {
            [idKey()]: record.id,
            version: record.version,
          });
          if (!alive || generation !== readGeneration) return;
          shown = object(result.record);
          describeCatalog(result.catalog);
          renderInspector();
        },
        row,
      );
      list.append(row);
    }
    if (nextCursor) button("Load more", () => load(true), list);
  }
  function summary(value: unknown, parent: HTMLElement) {
    if (Array.isArray(value)) {
      const ul = node("ul");
      for (const entry of value) {
        const li = node("li");
        summary(entry, li);
        ul.append(li);
      }
      parent.append(ul);
    } else if (isRecord(value)) {
      const dl = node("dl");
      for (const [key, item] of Object.entries(value)) {
        dl.append(node("dt", key));
        const dd = node("dd");
        summary(item, dd);
        dl.append(dd);
      }
      parent.append(dl);
    } else
      parent.append(
        node(
          "span",
          value === null
            ? "None"
            : typeof value === "string" ||
                typeof value === "number" ||
                typeof value === "boolean" ||
                typeof value === "undefined"
              ? String(value)
              : "Unsupported value",
        ),
      );
  }
  function renderInspector() {
    inspector.replaceChildren();
    if (!shown) return;
    const record = shown,
      family_ = collection(),
      identity = { [idKey()]: record.id, version: record.version };
    inspector.append(
      node(
        "h4",
        `${text(record.name)} — exact version ${text(record.version)}`,
      ),
    );
    const facts = node("details");
    facts.append(node("summary", "All saved fields, entries and dependencies"));
    summary(record, facts);
    inspector.append(facts);
    if (family_ === "resources") renderResource(record);
    else {
      const mode = choice(
        "Apply mode",
        family_ === "presets"
          ? [
              ["update", "Update selected values"],
              ["replace", "Reset target then replace"],
            ]
          : [
              ["merge", "Merge mapped entries"],
              ["replace", "Reset mapped existing targets then replace"],
            ],
        family_ === "presets" ? "update" : "merge",
      );
      const init = choice(
        "Initialization",
        [
          ["literal", "Literal captured values"],
          ["target-camera", "Target camera directives"],
        ],
        text(record.initialization),
      );
      const masks = choice(
        "Mask policy",
        [
          ["omit", "Omit captured masks; preserve target relationships"],
          [
            "compatible",
            "Clone compatible masks; reject incompatible geometry",
          ],
        ],
        text(record.maskPolicy),
      );
      inspector.append(mode.wrapper, init.wrapper, masks.wrapper);
      const mapping: Record<string, string> = {};
      let target: HTMLSelectElement | undefined;
      if (family_ === "presets") {
        const moduleId = object(record.settings).moduleId;
        const select = choice("Exact preset target", [
          ["", "Choose existing instance"],
          ...instances
            .filter((instance) => instance.moduleId === moduleId)
            .map(
              (instance) =>
                [
                  instance.instanceId,
                  `${instance.name} · ${instance.instanceId}`,
                ] as [string, string],
            ),
        ]);
        target = select.input;
        inspector.append(select.wrapper);
      } else {
        for (const entry of objects(record.entries)) {
          const select = choice(
            `Entry ${text(entry.entryId)} · ${text(entry.moduleId)}`,
            [
              ["", "Choose explicitly"],
              ["@new", "Create new instance"],
              ...instances
                .filter((instance) => instance.moduleId === entry.moduleId)
                .map(
                  (instance) =>
                    [
                      instance.instanceId,
                      `${instance.name} · ${instance.instanceId}`,
                    ] as [string, string],
                ),
            ],
          );
          select.input.addEventListener("change", () => {
            mapping[text(entry.entryId)] = select.input.value;
          });
          inspector.append(select.wrapper);
        }
      }
      const output = check("Apply saved output", false);
      if (family_ === "styles") {
        output.input.dataset.unavailable = String(record.output === null);
        output.input.disabled = record.output === null;
        inspector.append(output.wrapper);
      }
      mutationButton(
        "Apply exact version",
        () =>
          mutate(`${family_} apply`, () => {
            if (init.input.value !== record.initialization)
              throw new Error(
                "Initialization must match the saved definition representation.",
              );
            if (
              family_ === "styles" &&
              output.input.checked &&
              record.output === null
            )
              throw new Error("This style contains no saved output.");
            const application = {
              mode: mode.input.value,
              initialization: init.input.value,
              maskPolicy: masks.input.value,
            };
            if (family_ === "presets") {
              if (!target?.value)
                throw new Error("Choose an exact compatible existing target.");
              return { ...identity, ...application, instanceId: target.value };
            }
            return {
              ...identity,
              input: {
                ...application,
                mapping: catalogMapping(objects(record.entries), mapping),
                includeOutput: output.input.checked,
              },
            };
          }),
        inspector,
      );
      button(
        "Export definition",
        async () => {
          const result = await read(`${family_} export`, {
            ...identity,
            file: {
              filename: `${text(record.id)}-${text(record.version)}.json`,
            },
          });
          if (!alive) return;
          if (typeof result.transferId !== "string")
            throw new Error("Export returned no authorized transfer.");
          const a = node(
            "a",
            `Download ${text(result.filename)} (${String(result.byteLength)} bytes; SHA-256 ${text(result.sha256)})`,
          );
          a.href = `${scope.input.value === "global" ? "/api/darktable" : toolPath(photoId)}/files/${encodeURIComponent(result.transferId)}/bytes`;
          a.download = text(result.filename);
          inspector.append(a);
        },
        inspector,
      );
    }
    const confirm = check(
      "Remove this exact version from the disclosed catalog",
    );
    inspector.append(confirm.wrapper);
    mutationButton(
      "Remove version",
      () => {
        if (!confirm.input.checked)
          throw new Error("Confirm the exact version removal first.");
        return mutate(`${family_} remove`, () => identity, true);
      },
      inspector,
    );
  }
  function renderResource(record: JsonObject) {
    const reference = { resourceId: record.id, version: record.version };
    inspector.append(
      node(
        "p",
        `Immutable reference: ${text(record.id)} / ${text(record.version)}. ${String(record.byteLength)} bytes; SHA-256 ${text(record.sha256)}. Removal refuses retained references.`,
      ),
    );
    const candidates: { label: string; change: () => JsonObject }[] = [];
    function admittedReferences(
      schema: JsonObject,
      value: unknown,
      root: JsonObject,
      path: (string | number)[] = [],
      depth = 0,
    ): (string | number)[][] {
      if (depth > 32) return [];
      const resolved = resolveSemanticSchema(schema, root),
        paths: (string | number)[][] = [];
      const props = object(resolved.properties);
      if (
        ((Object.hasOwn(props, "resourceId") &&
          Object.hasOwn(props, "version")) ||
          Array.isArray(resolved.oneOf) ||
          Array.isArray(resolved.anyOf)) &&
        !validateSemanticValue(schema, reference, root).length
      )
        paths.push(path);
      for (const [key, child] of Object.entries(props))
        if (isRecord(child))
          paths.push(
            ...admittedReferences(
              child,
              object(value)[key],
              root,
              [...path, key],
              depth + 1,
            ),
          );
      if (Array.isArray(value) && isRecord(resolved.items))
        value.forEach((item, index) =>
          paths.push(
            ...admittedReferences(
              resolved.items as JsonObject,
              item,
              root,
              [...path, index],
              depth + 1,
            ),
          ),
        );
      for (const branch of [
        ...(schemaArray(resolved.oneOf) ? resolved.oneOf : []),
        ...(schemaArray(resolved.anyOf) ? resolved.anyOf : []),
      ])
        if (isRecord(branch))
          paths.push(
            ...admittedReferences(branch, value, root, path, depth + 1).filter(
              (candidate) =>
                !paths.some(
                  (existing) => existing.join(".") === candidate.join("."),
                ),
            ),
          );
      return paths;
    }
    for (const instance of instances) {
      const descriptor = object(details[instance.instanceId]?.descriptor),
        properties = object(object(descriptor.parameters).properties);
      for (const [parameterId, schema] of Object.entries(properties))
        if (isRecord(schema)) {
          const current = object(details[instance.instanceId]?.parameters)[
            parameterId
          ];
          for (const path of admittedReferences(
            schema,
            current,
            object(descriptor.parameters),
          ))
            candidates.push({
              label: `${instance.name} · ${instance.instanceId} · ${parameterId}${path.length ? `.${path.join(".")}` : ""}`,
              change: () => {
                const next = replaceSemanticValue(
                  object(details[instance.instanceId]?.parameters)[parameterId],
                  path,
                  reference,
                );
                if (
                  validateSemanticValue(
                    schema,
                    next,
                    object(descriptor.parameters),
                  ).length
                )
                  throw new Error(
                    "Updated parameter no longer admits this resource. Inspect the field again.",
                  );
                return {
                  type: "set",
                  instanceId: instance.instanceId,
                  parameterId,
                  value: next,
                };
              },
            });
        }
      if (isRecord(descriptor.blendSchema)) {
        const current = details[instance.instanceId]?.blend;
        for (const path of admittedReferences(
          descriptor.blendSchema,
          current,
          descriptor.blendSchema,
        ))
          candidates.push({
            label: `${instance.name} · ${instance.instanceId} · Blend ${path.join(".")}`,
            change: () => {
              const next = replaceSemanticValue(
                details[instance.instanceId]?.blend,
                path,
                reference,
              );
              if (
                validateSemanticValue(object(descriptor.blendSchema), next)
                  .length
              )
                throw new Error(
                  "Updated blend no longer admits this resource. Inspect the field again.",
                );
              return {
                type: "blend-set",
                instanceId: instance.instanceId,
                value: next,
              };
            },
          });
      }
    }
    for (const [fieldId, schema] of Object.entries(
      object(edit?.output.schema.properties),
    ))
      if (isRecord(schema) && edit) {
        const current = edit.output.settings[fieldId];
        for (const path of admittedReferences(
          schema,
          current,
          edit.output.schema,
        ))
          candidates.push({
            label: `Output · ${fieldId}${path.length ? `.${path.join(".")}` : ""}`,
            change: () => {
              const next = replaceSemanticValue(
                edit?.output.settings[fieldId],
                path,
                reference,
              );
              if (
                validateSemanticValue(schema, next, edit?.output.schema).length
              )
                throw new Error(
                  "Updated output no longer admits this resource. Inspect the field again.",
                );
              return { type: "output-set", fieldId, value: next };
            },
          });
      }
    const field = choice("Use resource in exact field", [
      ["", "Choose schema-admitted destination"],
      ...candidates.map(
        (candidate, index) =>
          [String(index), candidate.label] as [string, string],
      ),
    ]);
    inspector.append(field.wrapper);
    mutationButton(
      "Apply resource reference",
      () =>
        mutate("edit apply", () => {
          const selected = candidates[Number(field.input.value)];
          if (field.input.value === "" || !selected)
            throw new Error("Choose a schema-admitted destination.");
          return { input: { changes: [selected.change()] } };
        }),
      inspector,
    );
    if (!candidates.length)
      inspector.append(
        node(
          "p",
          "No current parameter, blend or output field admits this complete resource reference. Inspect a compatible operation before applying.",
        ),
      );
  }
  function renderCapture() {
    capture.replaceChildren();
    if (!edit) return;
    const family_ = collection();
    const file = node("input");
    file.type = "file";
    file.setAttribute("aria-label", "Import file bytes");
    capture.append(
      node(
        "h4",
        family_ === "resources"
          ? "Import typed resource bytes"
          : "Import definition",
      ),
      file,
    );
    const kind = choice("Native discovered resource kind", [
      ["", "Choose kind"],
      ...kinds
        .filter(
          (item) =>
            typeof item.kind === "string" &&
            object(item.availability).status !== "unavailable",
        )
        .map(
          (item) =>
            [
              text(item.kind),
              `${text(item.kind)} · ${Array.isArray(item.mediaTypes) ? item.mediaTypes.join(", ") : "native media type"} · maximum ${String(item.maximumBytes)} bytes`,
            ] as [string, string],
        ),
    ]);
    if (family_ === "resources") capture.append(kind.wrapper);
    mutationButton(
      "Import selected file",
      async () => {
        if (preparing || mutation.state().pending || mutation.state().busy)
          throw new Error("Resolve the retained request first.");
        preparing = true;
        renderRecovery();
        const selectedKind = kind.input.value;
        try {
          const selected = file.files?.[0];
          if (!selected) throw new Error("Choose a local file to transfer.");
          const descriptor = kinds.find(
            (item) => item.kind === kind.input.value,
          );
          if (family_ === "resources" && !descriptor)
            throw new Error("Choose a discovered resource kind.");
          const maximum =
            family_ === "resources"
              ? Number(descriptor?.maximumBytes)
              : 8 * 1024 * 1024;
          if (Number.isFinite(maximum) && selected.size > maximum)
            throw new Error(`File exceeds ${maximum} bytes.`);
          const bytes = await selected.arrayBuffer(),
            digest = await crypto.subtle.digest("SHA-256", bytes);
          const sha256 = Array.from(new Uint8Array(digest), (byte) =>
            byte.toString(16).padStart(2, "0"),
          ).join("");
          const response = await fetcher(
            `${scope.input.value === "global" ? "/api/darktable" : toolPath(photoId)}/files`,
            {
              method: "POST",
              headers: {
                "Content-Type": "application/octet-stream",
                "X-Filename": selected.name,
                "X-Sha256": sha256,
              },
              body: bytes,
            },
          );
          const transfer: unknown = await response.json();
          if (
            !response.ok ||
            !isRecord(transfer) ||
            typeof transfer.transferId !== "string"
          )
            throw new Error(
              "Byte transfer failed; no catalog mutation was submitted.",
            );
          preparing = false;
          await mutate(
            `${family_} import`,
            () => ({
              file: { transferId: transfer.transferId },
              ...(family_ === "resources" ? { kind: selectedKind } : {}),
            }),
            true,
          );
        } finally {
          preparing = false;
          if (alive) renderRecovery();
        }
      },
      capture,
    );
    if (family_ === "resources") return;
    capture.append(
      node(
        "h4",
        `Save ${family_ === "presets" ? "preset from one exact instance" : "style from explicit selections"}`,
      ),
    );
    const nameLabel = node("label", "Name"),
      name = node("input");
    name.type = "text";
    nameLabel.append(name);
    capture.append(nameLabel);
    const init = choice(
        "Capture initialization",
        [
          ["literal", "Literal values"],
          ["target-camera", "Target camera directives"],
        ],
        "literal",
      ),
      masks = choice(
        "Capture masks",
        [
          ["omit", "Omit masks"],
          ["compatible", "Capture complete compatible closure"],
        ],
        "omit",
      );
    capture.append(init.wrapper, masks.wrapper);
    const selected: {
      instanceId: string;
      include: HTMLInputElement;
      blend: HTMLInputElement;
      parameters: { id: string; input: HTMLInputElement }[];
    }[] = [];
    for (const instance of instances) {
      const fieldset = node("fieldset"),
        include = check(
          `Include ${instance.name} · ${instance.instanceId} · ${instance.moduleId}`,
        ),
        blend = check("Include blend");
      fieldset.append(include.wrapper, blend.wrapper);
      const descriptor = object(details[instance.instanceId]?.descriptor),
        properties = object(object(descriptor.parameters).properties);
      const parameters = Object.entries(properties).map(([id, schema]) => {
        const control = check(`${text(object(schema).title) || id} · ${id}`);
        fieldset.append(control.wrapper);
        return { id, input: control.input };
      });
      selected.push({
        instanceId: instance.instanceId,
        include: include.input,
        blend: blend.input,
        parameters,
      });
      capture.append(fieldset);
    }
    const output = check("Include complete saved output");
    if (family_ === "styles") capture.append(output.wrapper);
    mutationButton(
      "Save selected settings",
      () =>
        mutate(`${family_} save`, () => {
          if (!name.value.trim()) throw new Error("Enter a definition name.");
          const selections = selected
            .filter((item) => item.include.checked)
            .map((item) => ({
              instanceId: item.instanceId,
              parameterIds: item.parameters
                .filter((parameter) => parameter.input.checked)
                .map((parameter) => parameter.id),
              includeBlend: item.blend.checked,
            }));
          if (
            !selections.length ||
            selections.some(
              (selection) =>
                !selection.parameterIds.length && !selection.includeBlend,
            )
          )
            throw new Error(
              "Select instances and at least one parameter or blend for each.",
            );
          const common = {
            name: name.value,
            initialization: init.input.value,
            maskPolicy: masks.input.value,
          };
          if (family_ === "presets") {
            if (selections.length !== 1)
              throw new Error("A preset requires exactly one source instance.");
            const source = selections[0]!;
            return {
              instanceId: source.instanceId,
              input: {
                ...common,
                parameterIds: source.parameterIds,
                includeBlend: source.includeBlend,
              },
            };
          }
          return {
            input: {
              ...common,
              instances: selections,
              includeOutput: output.input.checked,
            },
          };
        }),
      capture,
    );
  }
  async function switchCatalog() {
    ++readGeneration;
    records = [];
    shown = undefined;
    nextCursor = null;
    inspector.replaceChildren();
    list.replaceChildren();
    renderCapture();
    describeCatalog({});
    await load();
  }
  family.input.addEventListener("change", () => {
    void switchCatalog().catch((error) => {
      status.textContent = String(error);
    });
  });
  scope.input.addEventListener("change", () => {
    void switchCatalog().catch((error) => {
      status.textContent = String(error);
    });
  });
  button("Refresh collection", () => load(), toolbar);
  return {
    element,
    bind(next, nextInstances, nextDetails, resourceKinds) {
      if (!alive) return;
      edit = next;
      instances = nextInstances;
      details = nextDetails;
      kinds = resourceKinds;
      if (boundId !== next.editId) {
        boundId = next.editId;
        void switchCatalog().catch((error) => {
          if (alive) status.textContent = String(error);
        });
      } else if (!capture.childElementCount) renderCapture();
      renderRecovery();
    },
    dispose() {
      alive = false;
      ++readGeneration;
      element.remove();
    },
  };
}
