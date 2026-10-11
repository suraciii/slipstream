import {
  readMetadata,
  type MetadataRead,
  type MetadataSource,
  type MetadataValue,
} from "../api/external-metadata.js";
import type { BrowserFetch } from "../model/access-session.js";
import {
  createMetadataDrafts,
  metadataInputChange,
  metadataFields,
  type MetadataInput,
} from "../model/metadata-drafts.js";

function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  return node;
}
function button(text: string, action: () => void): HTMLButtonElement {
  const node = element("button", text);
  node.type = "button";
  node.addEventListener("click", action);
  return node;
}
function select(
  options: readonly (readonly [string, string])[],
): HTMLSelectElement {
  const node = element("select");
  for (const [value, label] of options) {
    const option = element("option", label);
    option.value = value;
    node.append(option);
  }
  return node;
}
function disclosure(label: string): HTMLDetailsElement {
  const node = element("details");
  node.append(element("summary", label));
  return node;
}
function displayValue(
  parent: HTMLElement,
  value: MetadataValue | undefined,
): void {
  if (value === undefined) return;
  if (Array.isArray(value)) {
    if (!value.length) parent.append(element("p", "(empty list)"));
    const list = element("ol");
    for (const item of value)
      list.append(element("li", item || "(empty text)"));
    parent.append(list);
  } else if (typeof value === "object") {
    const list = element("dl");
    for (const [language, text] of Object.entries(value))
      list.append(
        element("dt", language),
        element("dd", text || "(empty text)"),
      );
    parent.append(list);
  } else parent.append(element("p", String(value) || "(empty text)"));
}
function displaySource(parent: HTMLElement, source: MetadataSource): void {
  parent.append(
    element(
      "p",
      `${source.provenance ?? "External value"}: ${source.state}${source.problem ? ` — ${source.problem}` : ""}`,
    ),
  );
  displayValue(parent, source.value);
  if (source.languageAlternativesAvailable === false)
    parent.append(element("p", "This source has no language alternatives."));
}

export function createMetadataPanel(
  host: HTMLElement,
  fetcher: BrowserFetch,
  announce: (message: string) => void,
) {
  const drafts = createMetadataDrafts();
  const messages = new Map<string, string>();
  let alive = true;
  let photoId: string | undefined;
  let generation = 0;
  let controller: AbortController | undefined;
  let facts: MetadataRead | undefined;
  let evidence = "";
  let status = element("p");
  let updateReview = () => {};
  const current = (id: string, epoch: number) =>
    alive && photoId === id && generation === epoch;
  const saving = () => Boolean(photoId && drafts.isSaving(photoId));

  const render = () => {
    host.replaceChildren();
    host.classList.add("external-metadata-panel");
    host.append(element("h4", "External Metadata"));
    status = element("p");
    status.dataset.externalMetadataStatus = "";
    status.setAttribute("role", "status");
    host.append(status);
    const refresh = button("Refresh inspection", () => void inspect());
    refresh.dataset.externalMetadataRefresh = "";
    refresh.disabled = saving();
    host.append(refresh);
    if (!facts || !photoId) {
      status.textContent = photoId
        ? "Inspecting metadata…"
        : "No Photo selected.";
      return;
    }
    const id = photoId;
    const observed = facts;
    let draft = drafts.get(id);
    const changedEvidence = drafts.changed(id, evidence);
    const busy = drafts.isSaving(id);
    const reason = !observed.saveAvailable
      ? (observed.saveUnavailableReason ??
        "This deployment cannot safely save Sidecars.")
      : busy
        ? "An admitted Save for this Photo is still settling."
        : changedEvidence
          ? "Metadata changed since this draft began. Compare the current sources and retained changes before using this inspection."
          : draft?.needsInspection
            ? "Inspect current metadata and explicitly use that inspection before a new Save."
            : "";
    status.textContent =
      messages.get(id) ??
      (draft
        ? "Unsaved changes retained for this Photo."
        : "Current metadata inspected. No pending Sidecar changes.");
    if (reason) host.append(element("p", reason));
    const discard = button("Discard changes", () => {
      drafts.discard(id);
      messages.delete(id);
      render();
    });
    discard.dataset.externalMetadataDiscard = "";
    discard.hidden = !draft;
    discard.disabled = busy;
    host.append(discard);
    if (draft && (changedEvidence || draft.needsInspection) && !busy) {
      const useInspection = button(
        "Use current inspection for retained changes",
        () => {
          drafts.acceptInspection(id, observed, evidence);
          messages.set(
            id,
            "Current inspection accepted. Review retained changes before confirming Save.",
          );
          render();
        },
      );
      useInspection.dataset.externalMetadataAcceptInspection = "";
      host.append(useInspection);
      const baseline = disclosure("Draft's initial values and sources");
      for (const [key, label] of metadataFields) {
        const original = draft.facts.fields[key];
        if (!original) continue;
        baseline.append(element("h5", label));
        displaySource(baseline, original);
      }
      host.append(baseline);
    }
    const ensureDraft = () => {
      draft = drafts.admit(id, observed, evidence);
      if (!draft) {
        status.textContent =
          "The session holds 32 metadata drafts. Discard or save another Photo's changes before editing this Photo. This input was not retained.";
        return undefined;
      }
      discard.hidden = false;
      draft.reviewed = false;
      return draft;
    };
    const ordinary = element("div");
    ordinary.className = "external-metadata-fields";
    const advanced = disclosure("More fields, attribution, and rights");
    const sourceEvidence = disclosure("Sources and Sidecar evidence");
    sourceEvidence.append(
      element("p", `Original: ${observed.originalLocation}`),
      element(
        "p",
        `Sidecar association: ${observed.association.state}${observed.association.reason ? ` — ${observed.association.reason}` : ""}`,
      ),
      element(
        "p",
        `Library Rating: ${observed.libraryRating} (separate from external rating)`,
      ),
      element(
        "p",
        `Observed Sidecar: ${observed.evidence.sidecar.state}${observed.evidence.sidecar.location ? ` — ${observed.evidence.sidecar.location}` : ""}${observed.evidence.sidecar.reason ? ` — ${observed.evidence.sidecar.reason}` : ""}`,
      ),
    );
    for (const candidate of observed.association.candidates)
      sourceEvidence.append(element("p", candidate));
    for (const [key, label, kind] of metadataFields) {
      const field = observed.fields[key];
      if (!field) continue;
      const section = element("section");
      section.dataset.metadataField = key;
      section.append(element("h5", label));
      const isOrdinary =
        key === "dc:title" || key === "dc:description" || key === "dc:subject";
      (isOrdinary ? ordinary : advanced).append(section);
      const sources = disclosure(`${label}: source values`);
      sources.append(element("p", key));
      displaySource(sources, field);
      for (const source of field.sources) displaySource(sources, source);
      sourceEvidence.append(sources);
      if (field.inferredValue !== undefined) {
        section.append(element("p", "Inferred, not stored:"));
        displayValue(section, field.inferredValue);
      }
      if (!observed.saveAvailable || !field.writable) {
        displaySource(section, field);
        continue;
      }
      section.append(
        element(
          "p",
          `${field.state}${field.provenance ? ` · ${field.provenance}` : ""}${field.problem ? ` — ${field.problem}` : ""}`,
        ),
      );
      const initial = draft?.inputs[key];
      const languageValue = (draft?.facts ?? observed).fields[key]?.value;
      const values =
        languageValue &&
        typeof languageValue === "object" &&
        !Array.isArray(languageValue)
          ? languageValue
          : {};
      const raw: MetadataInput = initial ?? {
        operation: "unchanged",
        text:
          kind === "languages"
            ? typeof languageValue === "string"
              ? languageValue
              : (values["x-default"] ?? "")
            : Array.isArray(field.value)
              ? field.value.join("\n")
              : typeof field.value === "object"
                ? ""
                : String(field.value ?? ""),
        languages: Object.entries(values).map(([name, value]) => ({
          name,
          value,
          action: "unchanged",
        })),
      };
      const error = element("p", draft?.errors[key]);
      error.dataset.metadataError = key;
      error.setAttribute("role", "alert");
      const operation = select([
        ["unchanged", "Unchanged"],
        ["set", "Set value"],
        ...(kind === "text" || kind === "list"
          ? [["clear", "Set empty value"] as const]
          : []),
        ["remove", "Remove Sidecar property"],
      ]);
      operation.value = raw.operation;
      operation.dataset.metadataOperation = key;
      operation.setAttribute("aria-label", `${label} Sidecar operation`);
      const controls = element("div");
      const commit = () => {
        const owner = ensureDraft();
        if (!owner) return;
        owner.inputs[key] = raw;
        delete owner.changes[key];
        delete owner.errors[key];
        const result = metadataInputChange(key, kind, raw);
        if (result.error) owner.errors[key] = result.error;
        if (result.change) owner.changes[key] = result.change;
        error.textContent = owner.errors[key] ?? "";
        controls.hidden = !isOrdinary && raw.operation !== "set";
        updateReview();
      };
      const input =
        kind === "boolean"
          ? select([
              ["", "Choose copyright status"],
              ["false", "False"],
              ["true", "True"],
            ])
          : kind === "number" || key === "dc:title"
            ? element("input")
            : element("textarea");
      input.value = raw.text;
      input.setAttribute("aria-label", label);
      input.dataset.metadataValue = key;
      if (input instanceof HTMLTextAreaElement)
        input.rows = key === "dc:description" ? 3 : 2;
      if (input instanceof HTMLInputElement && kind === "number") {
        input.type = "number";
        input.min = "-1";
        input.max = "5";
        input.step = "any";
      }
      const syncInput = () => {
        raw.text = input.value;
        raw.operation = "set";
        operation.value = "set";
        if (kind === "boolean" && !raw.text) {
          const owner = ensureDraft();
          if (owner) {
            owner.inputs[key] = raw;
            delete owner.changes[key];
            owner.errors[key] = "Choose true or false.";
            error.textContent = owner.errors[key];
            updateReview();
          }
        } else commit();
      };
      input.addEventListener(
        kind === "boolean" ? "change" : "input",
        syncInput,
      );
      controls.append(input);
      if (kind === "list")
        controls.append(
          element(
            "p",
            key === "dc:creator"
              ? "One creator per line; order is preserved."
              : "One keyword per line.",
          ),
        );
      controls.hidden = !isOrdinary && raw.operation !== "set";
      section.append(controls);
      const operations = disclosure(
        kind === "languages"
          ? "Additional languages and removal"
          : "Empty value and removal",
      );
      if (isOrdinary || kind === "languages") section.append(operations);
      else section.append(operation);
      if (isOrdinary || kind === "languages") operations.append(operation);
      if (kind === "languages") {
        operations.append(
          element(
            "p",
            "The ordinary field changes x-default only. Other languages remain unchanged unless selected here. Empty text is a stored value. Removing the property reveals embedded fallback.",
          ),
        );
        const rows = element("div");
        const addRow = (row: MetadataInput["languages"][number]) => {
          const node = element("div");
          node.className = "metadata-language-row";
          const name = element("input");
          name.value = row.name;
          name.placeholder = "Language, e.g. fr";
          name.setAttribute("aria-label", `${label} language`);
          const action = select([
            ["unchanged", "Unchanged"],
            ["change", "Change language"],
            ["remove", "Remove language"],
          ]);
          action.value = row.action;
          action.setAttribute("aria-label", `${label} language operation`);
          const text = element("textarea");
          text.rows = 2;
          text.value = row.value;
          text.setAttribute("aria-label", `${label} language value`);
          const sync = () => {
            row.name = name.value;
            row.value = text.value;
            row.action = action.value;
            text.disabled = busy || action.value === "remove";
            commit();
          };
          name.addEventListener("input", sync);
          text.addEventListener("input", sync);
          action.addEventListener("change", sync);
          text.disabled = busy || action.value === "remove";
          node.append(name, action, text);
          rows.append(node);
        };
        for (const row of raw.languages) addRow(row);
        operations.append(
          rows,
          button("Add language", () => {
            if (!ensureDraft()) return;
            const row = { name: "", value: "", action: "change" };
            raw.languages.push(row);
            addRow(row);
            commit();
          }),
        );
      } else
        operations.append(
          element(
            "p",
            "Empty values suppress embedded fallback. Removal reveals it.",
          ),
        );
      operation.addEventListener("change", () => {
        raw.operation = operation.value;
        commit();
      });
      section.append(error);
      if (busy)
        section
          .querySelectorAll<
            | HTMLInputElement
            | HTMLSelectElement
            | HTMLTextAreaElement
            | HTMLButtonElement
          >("input,select,textarea,button")
          .forEach((control) => {
            control.disabled = true;
          });
    }
    host.append(ordinary, advanced);
    const capture = disclosure(
      "Original capture facts and Sidecar representations",
    );
    for (const [key, fact] of Object.entries(observed.captureFacts)) {
      const row = element("section");
      row.append(element("h5", `${key} · ${fact.identifier} · ${fact.unit}`));
      displaySource(row, fact);
      capture.append(row);
    }
    for (const [key, field] of Object.entries(observed.fields))
      if (!metadataFields.some(([supported]) => supported === key)) {
        const row = element("section");
        row.append(element("h5", `${key} (read-only representation)`));
        displaySource(row, field);
        for (const source of field.sources) displaySource(row, source);
        capture.append(row);
      }
    host.append(capture, sourceEvidence);
    const review = element("section");
    review.dataset.externalMetadataReview = "";
    const reviewButton = button("Review pending changes", () => {
      if (draft) {
        draft.reviewed = true;
        updateReview();
      }
    });
    reviewButton.dataset.externalMetadataReviewButton = "";
    const saveButton = button("Confirm and save Sidecar", () => void save());
    saveButton.dataset.externalMetadataSave = "";
    if (observed.saveAvailable || draft)
      host.append(review, reviewButton, saveButton);
    updateReview = () => {
      review.replaceChildren();
      if (draft && Object.keys(draft.changes).length) {
        review.append(
          element(
            "h4",
            draft.reviewed
              ? "Reviewed Sidecar changes"
              : "Pending Sidecar changes",
          ),
        );
        for (const [key, change] of Object.entries(draft.changes)) {
          const label =
            metadataFields.find(([field]) => field === key)?.[1] ?? key;
          review.append(
            element(
              "h5",
              `${label}: ${change.op === "remove" ? "Remove property" : change.op === "clear" ? "Set empty value" : "Set value"}`,
            ),
          );
          if (change.op === "set") displayValue(review, change.value);
          if (change.op === "setLanguages")
            for (const [language, text] of Object.entries(change.languages))
              review.append(
                element(
                  "p",
                  `${language === "x-default" ? "Default language" : language}: ${text === null ? "Remove alternative" : text || "(empty text)"}`,
                ),
              );
        }
      }
      reviewButton.disabled =
        Boolean(reason) ||
        !draft ||
        !Object.keys(draft.changes).length ||
        Boolean(Object.keys(draft.errors).length);
      saveButton.disabled = reviewButton.disabled || !draft?.reviewed;
    };
    updateReview();
  };
  const inspect = async () => {
    controller?.abort();
    controller = new AbortController();
    const signal = controller.signal;
    const epoch = ++generation;
    const id = photoId;
    const active = document.activeElement;
    const focused =
      active instanceof HTMLElement && host.contains(active)
        ? {
            field: active.closest<HTMLElement>("[data-metadata-field]")?.dataset
              .metadataField,
            label: active.getAttribute("aria-label"),
            refresh: active.hasAttribute("data-external-metadata-refresh"),
            start:
              active instanceof HTMLInputElement ||
              active instanceof HTMLTextAreaElement
                ? active.selectionStart
                : null,
            end:
              active instanceof HTMLInputElement ||
              active instanceof HTMLTextAreaElement
                ? active.selectionEnd
                : null,
          }
        : undefined;
    facts = undefined;
    render();
    if (!id) return;
    try {
      const result = await readMetadata(fetcher, id, signal);
      if (!current(id, epoch) || signal.aborted) return;
      facts = result.facts;
      evidence = result.evidence;
      const draft = drafts.get(id);
      if (draft) draft.reviewed = false;
      render();
      if (focused && document.activeElement === document.body) {
        const scope = focused.field
          ? Array.from(
              host.querySelectorAll<HTMLElement>("[data-metadata-field]"),
            ).find((section) => section.dataset.metadataField === focused.field)
          : host;
        const target = focused.refresh
          ? host.querySelector<HTMLButtonElement>(
              "[data-external-metadata-refresh]",
            )
          : Array.from(
              scope?.querySelectorAll<HTMLElement>("[aria-label]") ?? [],
            ).find((node) => node.getAttribute("aria-label") === focused.label);
        if (target) {
          let ancestor = target.parentElement;
          while (ancestor && ancestor !== host) {
            if (ancestor instanceof HTMLDetailsElement) ancestor.open = true;
            ancestor = ancestor.parentElement;
          }
        }
        target?.focus();
        if (
          ((target instanceof HTMLInputElement && target.type !== "number") ||
            target instanceof HTMLTextAreaElement) &&
          focused.start !== null
        )
          target.setSelectionRange(focused.start, focused.end);
      }
    } catch (error) {
      if (!current(id, epoch) || signal.aborted) return;
      status.textContent = `${error instanceof Error ? error.message : "Read Metadata failed."}${drafts.get(id) ? " Your unsaved draft is retained. Refresh inspection to continue." : ""}`;
    }
  };
  const save = async () => {
    if (
      !photoId ||
      !facts ||
      !facts.saveAvailable ||
      drafts.changed(photoId, evidence)
    )
      return;
    const id = photoId;
    const operation = drafts.beginSave(id);
    if (!operation) return;
    messages.set(
      id,
      "Saving Sidecar. The admitted Save may complete after navigation.",
    );
    render();
    let success = false;
    let message =
      "Save outcome unknown. Refresh inspection before deciding on another Save; nothing has been retried.";
    try {
      const response = await fetcher(
        `/api/photos/${encodeURIComponent(id)}/external-metadata`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: operation.body,
        },
      );
      const result: unknown = await response.json();
      if (
        result &&
        typeof result === "object" &&
        "photoId" in result &&
        result.photoId === id &&
        "sidecarLocation" in result &&
        typeof result.sidecarLocation === "string" &&
        result.sidecarLocation &&
        "affectedFields" in result &&
        Array.isArray(result.affectedFields) &&
        result.affectedFields.every(
          (key: unknown) => typeof key === "string",
        ) &&
        response.ok
      ) {
        success = true;
        const labels = result.affectedFields.map(
          (key: string) =>
            metadataFields.find(([field]) => field === key)?.[1] ?? key,
        );
        message = `Saved Sidecar ${result.sidecarLocation}. Verified fields: ${labels.join(", ") || "none"}.`;
      } else if (
        !response.ok &&
        result &&
        typeof result === "object" &&
        "error" in result &&
        result.error &&
        typeof result.error === "object"
      ) {
        const error = result.error;
        const code =
          "code" in error && typeof error.code === "string"
            ? error.code
            : "save_failed";
        const detail =
          "message" in error && typeof error.message === "string"
            ? error.message
            : "Save failed.";
        message = `${code}: ${detail} Inspect current metadata before a new decision.`;
        if ("details" in error)
          message += ` Required correction: ${JSON.stringify(error.details)}`;
      }
    } catch {
      /* An admitted mutation cannot be described as unchanged. */
    }
    drafts.settle(id, operation.draft, success);
    if (!alive) return;
    messages.set(id, message);
    announce(`Metadata Save for ${id}: ${message}`);
    if (photoId === id) await inspect();
  };
  return {
    show(id: string | undefined) {
      if (id === photoId && (facts || controller)) return;
      photoId = id;
      void inspect();
    },
    refresh() {
      if (!saving()) void inspect();
    },
    dispose() {
      alive = false;
      generation++;
      controller?.abort();
      drafts.clear();
      messages.clear();
    },
  };
}
