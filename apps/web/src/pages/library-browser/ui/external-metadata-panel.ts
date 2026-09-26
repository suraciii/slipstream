import {
  readMetadata,
  type MetadataChange,
  type MetadataRead,
  type MetadataSource,
  type MetadataValue,
} from "../api/external-metadata.js";
import type { BrowserFetch } from "../model/access-session.js";

const fields = [
  ["dc:title", "Title", "languages"],
  ["dc:description", "Description", "languages"],
  ["photoshop:Headline", "Headline", "text"],
  ["dc:subject", "Keywords", "list"],
  ["xmp:Label", "Label", "text"],
  ["xmp:Rating", "External rating", "number"],
  ["dc:creator", "Creators (in order)", "list"],
  ["photoshop:AuthorsPosition", "Creator job title", "text"],
  ["photoshop:Credit", "Credit line", "text"],
  ["photoshop:Source", "Source", "text"],
  ["dc:rights", "Copyright notice", "languages"],
  ["xmpRights:UsageTerms", "Rights usage terms", "languages"],
  ["xmpRights:Marked", "Copyright status", "boolean"],
  ["xmpRights:WebStatement", "Rights information URL", "text"],
] as const;
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
function displayValue(
  parent: HTMLElement,
  value: MetadataValue | undefined,
): void {
  if (value === undefined) return;
  if (Array.isArray(value)) {
    const list = element("ol");
    for (const item of value) list.append(element("li", item));
    parent.append(list);
  } else if (typeof value === "object") {
    const list = element("dl");
    for (const [language, text] of Object.entries(value)) {
      list.append(element("dt", language), element("dd", text));
    }
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
  let alive = true;
  let photoId: string | undefined;
  let generation = 0;
  let controller: AbortController | undefined;
  let facts: MetadataRead | undefined;
  let evidence = "";
  let saving = false;
  let needsInspection = false;
  let changes: Record<string, MetadataChange> = {};
  let reviewed = false;
  let updateReview = () => {};
  let status = element("p");
  let saveButton = element("button");
  const errorNodes = new Map<string, HTMLElement>();
  const busyPhotos = new Set<string>();
  const current = (id: string, epoch: number) =>
    alive && photoId === id && generation === epoch;

  const render = () => {
    host.replaceChildren();
    errorNodes.clear();
    host.append(element("h4", "External Metadata"));
    status = element("p");
    status.dataset.externalMetadataStatus = "";
    status.setAttribute("role", "status");
    host.append(status);
    const refresh = button("Refresh inspection", () => {
      if (saving) return;
      if (
        Object.keys(changes).length &&
        !window.confirm(
          "Discard pending Sidecar changes and inspect current metadata?",
        )
      )
        return;
      void inspect();
    });
    refresh.dataset.externalMetadataRefresh = "";
    refresh.disabled = saving;
    host.append(refresh);
    if (!facts) {
      status.textContent = photoId
        ? "Inspecting metadata…"
        : "No Photo selected.";
      return;
    }
    host.append(
      element("p", `Original: ${facts.originalLocation}`),
      element(
        "p",
        `Sidecar association: ${facts.association.state}${facts.association.reason ? ` — ${facts.association.reason}` : ""}`,
      ),
    );
    for (const candidate of facts.association.candidates)
      host.append(element("p", candidate));
    host.append(
      element(
        "p",
        `Library Rating: ${facts.libraryRating} (separate from external rating)`,
      ),
    );
    host.append(
      element(
        "p",
        `Observed Sidecar: ${facts.evidence.sidecar.state}${facts.evidence.sidecar.location ? ` — ${facts.evidence.sidecar.location}` : ""}${facts.evidence.sidecar.reason ? ` — ${facts.evidence.sidecar.reason}` : ""}`,
      ),
    );
    const reason = !facts.saveAvailable
      ? (facts.saveUnavailableReason ??
        "The server cannot safely save this Sidecar. Refresh after resolving server support.")
      : needsInspection
        ? "Fresh inspection is required before a new Save. Refresh inspection and decide again."
        : busyPhotos.has(facts.photoId) && !saving
          ? "An admitted Save for this Photo is still settling. Refresh after it finishes."
          : "";
    if (reason) host.append(element("p", reason));
    const editors = element("div");
    editors.className = "external-metadata-fields";
    for (const [key, label, kind] of fields) {
      const field = facts.fields[key];
      if (!field) continue;
      const section = element("section");
      section.dataset.metadataField = key;
      section.append(element("h5", label), element("p", key));
      displaySource(section, field);
      if (field.inferredValue !== undefined) {
        section.append(element("p", "Inferred, not stored:"));
        displayValue(section, field.inferredValue);
      }
      const sources = element("details");
      sources.append(element("summary", "Inspect source values"));
      for (const source of field.sources) displaySource(sources, source);
      section.append(sources);
      const operationLabel = element("label", "Sidecar operation");
      const operations: [string, string][] = [
        ["unchanged", "Unchanged"],
        ["set", kind === "languages" ? "Edit named languages" : "Set value"],
        ["remove", "Remove Sidecar property"],
      ];
      if (kind === "text" || kind === "list")
        operations.splice(2, 0, ["clear", "Set empty value"]);
      const pending = changes[key];
      const operation = select(operations);
      operation.dataset.metadataOperation = key;
      operation.setAttribute("aria-label", `${label} Sidecar operation`);
      operation.value =
        pending?.op === "setLanguages" ? "set" : (pending?.op ?? "unchanged");
      operation.disabled = !field.writable || saving || Boolean(reason);
      operationLabel.append(operation);
      section.append(operationLabel);
      const controls = element("div");
      controls.hidden = operation.value !== "set";
      section.append(controls);
      const error = element("p");
      error.dataset.metadataError = key;
      error.setAttribute("role", "alert");
      section.append(error);
      errorNodes.set(key, error);
      let value: MetadataValue =
        pending?.op === "set"
          ? pending.value
          : (field.value ??
            (kind === "list"
              ? []
              : kind === "boolean"
                ? false
                : kind === "number"
                  ? 0
                  : ""));
      const languages: Record<string, string | null> = Object.create(
        null,
      ) as Record<string, string | null>;
      const languageRows: {
        name: HTMLInputElement;
        input: HTMLTextAreaElement;
        action: HTMLSelectElement;
      }[] = [];
      const changed = () => {
        reviewed = false;
        error.textContent = "";
        const op = operation.value;
        controls.hidden = op !== "set";
        delete changes[key];
        if (op === "remove" || op === "clear") changes[key] = { op };
        else if (op === "set") {
          if (kind === "languages") {
            for (const language of Object.keys(languages))
              delete languages[language];
            for (const row of languageRows) {
              if (row.action.value === "unchanged") continue;
              const language = row.name.value;
              if (
                !/^(x-default|[A-Za-z]{2,8}(?:-[A-Za-z0-9]{1,8})*)$/.test(
                  language,
                )
              ) {
                error.textContent =
                  "Use a valid named language, such as en, fr, or x-default.";
                break;
              }
              if (
                Object.keys(languages).some(
                  (existing) =>
                    existing.toLowerCase() === language.toLowerCase(),
                )
              ) {
                error.textContent =
                  "Each changed language must be named only once.";
                break;
              }
              languages[language] =
                row.action.value === "remove" ? null : row.input.value;
            }
            if (!error.textContent && !Object.keys(languages).length)
              error.textContent = "Choose explicit language changes.";
            if (!error.textContent)
              changes[key] = {
                op: "setLanguages",
                languages: { ...languages },
              };
          } else if (
            kind === "number" &&
            (typeof value !== "number" ||
              !Number.isFinite(value) ||
              (value !== -1 && (value < 0 || value > 5)))
          )
            error.textContent =
              "Rating must be -1 or a number from 0 through 5.";
          else changes[key] = { op: "set", value };
        }
        updateReview();
      };
      if (kind === "languages") {
        const rows = element("div");
        controls.append(rows);
        const addRow = (language: string, text: string) => {
          const row = element("div");
          row.className = "metadata-language-row";
          const name = element("input");
          name.value = language;
          name.placeholder = "Language (e.g. x-default)";
          name.setAttribute("aria-label", `${label} language`);
          const input = element("textarea");
          input.value = text;
          input.setAttribute("aria-label", `${label} language value`);
          const action = select([
            ["unchanged", "Unchanged"],
            ["change", "Change language"],
            ["remove", "Remove language"],
          ]);
          action.setAttribute("aria-label", `${label} language operation`);
          if (
            pending?.op === "setLanguages" &&
            Object.hasOwn(pending.languages, language)
          ) {
            action.value =
              pending.languages[language] === null ? "remove" : "change";
            input.value = pending.languages[language] ?? "";
            input.disabled = action.value === "remove";
          }
          languageRows.push({ name, input, action });
          const sync = () => {
            input.disabled = action.value === "remove";
            changed();
          };
          name.addEventListener("input", sync);
          input.addEventListener("input", sync);
          action.addEventListener("change", sync);
          row.append(name, action, input);
          rows.append(row);
        };
        if (typeof field.value === "object" && !Array.isArray(field.value))
          for (const [language, text] of Object.entries(field.value))
            addRow(language, text);
        if (pending?.op === "setLanguages")
          for (const language of Object.keys(pending.languages)) {
            if (!languageRows.some((row) => row.name.value === language))
              addRow(language, pending.languages[language] ?? "");
          }
        controls.append(button("Add language", () => addRow("", "")));
      } else if (kind === "boolean") {
        const input = select([
          ["false", "False"],
          ["true", "True"],
        ]);
        input.value = value === true ? "true" : "false";
        input.setAttribute("aria-label", label);
        input.addEventListener("change", () => {
          value = input.value === "true";
          changed();
        });
        controls.append(input);
      } else {
        const input =
          kind === "number" ? element("input") : element("textarea");
        input.setAttribute("aria-label", label);
        input.dataset.metadataValue = key;
        input.value = Array.isArray(value)
          ? value.join("\n")
          : typeof value === "object"
            ? ""
            : String(value);
        if (input instanceof HTMLInputElement) {
          input.type = "number";
          input.min = "-1";
          input.max = "5";
          input.step = "any";
        }
        if (kind === "list")
          controls.append(
            element(
              "p",
              key === "dc:creator"
                ? "Creators, one per line, in order"
                : "Keywords, one per line",
            ),
          );
        input.addEventListener("input", () => {
          value =
            kind === "number"
              ? input.value === ""
                ? NaN
                : Number(input.value)
              : kind === "list"
                ? input.value === ""
                  ? []
                  : key === "dc:subject"
                    ? [...new Set(input.value.split("\n"))]
                    : input.value.split("\n")
                : input.value;
          changed();
        });
        controls.append(input);
      }
      operation.addEventListener("change", changed);
      editors.append(section);
    }
    host.append(editors);
    const capture = element("details");
    capture.append(
      element("summary", "Original capture facts and Sidecar representations"),
    );
    for (const [key, fact] of Object.entries(facts.captureFacts)) {
      const row = element("section");
      row.append(element("h5", `${key} · ${fact.identifier} · ${fact.unit}`));
      displaySource(row, fact);
      capture.append(row);
    }
    for (const [key, field] of Object.entries(facts.fields))
      if (!fields.some(([supported]) => supported === key)) {
        const row = element("section");
        row.append(element("h5", `${key} (read-only representation)`));
        displaySource(row, field);
        for (const source of field.sources) displaySource(row, source);
        capture.append(row);
      }
    host.append(capture);
    const review = element("section");
    review.dataset.externalMetadataReview = "";
    host.append(review);
    const reviewButton = button("Review pending changes", () => {
      reviewed = true;
      updateReview();
    });
    reviewButton.dataset.externalMetadataReviewButton = "";
    host.append(reviewButton);
    saveButton = button("Confirm and save Sidecar", () => void save());
    saveButton.dataset.externalMetadataSave = "";
    host.append(saveButton);
    updateReview = () => {
      review.replaceChildren(element("h4", "Pending Sidecar changes"));
      for (const [key, change] of Object.entries(changes)) {
        review.append(element("h5", `${key}: ${change.op}`));
        if (change.op === "set") displayValue(review, change.value);
        if (change.op === "setLanguages")
          for (const [language, text] of Object.entries(change.languages))
            review.append(
              element(
                "p",
                `${language}: ${text === null ? "Remove alternative" : text || "(empty text)"}`,
              ),
            );
      }
      const invalid = [...errorNodes.values()].some((node) =>
        Boolean(node.textContent),
      );
      reviewButton.disabled =
        saving || invalid || !Object.keys(changes).length || Boolean(reason);
      saveButton.disabled = !reviewed || reviewButton.disabled;
    };
    updateReview();
  };
  const inspect = async () => {
    controller?.abort();
    controller = new AbortController();
    const signal = controller.signal;
    const epoch = ++generation;
    const id = photoId;
    facts = undefined;
    changes = {};
    reviewed = false;
    render();
    if (!id) return;
    try {
      const result = await readMetadata(fetcher, id, signal);
      if (!current(id, epoch) || signal.aborted) return;
      facts = result.facts;
      evidence = result.evidence;
      needsInspection = false;
      render();
      status.textContent =
        "Current metadata inspected. No Sidecar changes selected.";
    } catch (error) {
      if (!current(id, epoch) || signal.aborted) return;
      status.textContent =
        error instanceof Error
          ? error.message
          : "Read Metadata failed. Refresh inspection.";
    }
  };
  const save = async () => {
    if (saving || !facts || !photoId || saveButton.disabled) return;
    const id = photoId;
    const epoch = generation;
    const body = `{"evidence":${evidence},"changes":${JSON.stringify(changes)}}`;
    saving = true;
    busyPhotos.add(id);
    saveButton.disabled = true;
    for (const control of Array.from(
      host.querySelectorAll<
        | HTMLInputElement
        | HTMLButtonElement
        | HTMLSelectElement
        | HTMLTextAreaElement
      >("input,button,select,textarea"),
    ))
      control.disabled = true;
    status.textContent =
      "Saving Sidecar. The admitted save may complete even if you leave this Photo.";
    let message =
      "Save outcome unknown. Refresh inspection before deciding on another Save; nothing has been retried.";
    let success = false;
    let errors: unknown;
    try {
      const response = await fetcher(
        `/api/photos/${encodeURIComponent(id)}/external-metadata`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body,
        },
      );
      const result = (await response.json()) as {
        photoId?: string;
        sidecarLocation?: string;
        affectedFields?: string[];
        error?: { code?: string; message?: string; details?: unknown };
      };
      if (
        response.ok &&
        result.photoId === id &&
        result.sidecarLocation &&
        Array.isArray(result.affectedFields)
      ) {
        success = true;
        message = `Saved Sidecar ${result.sidecarLocation}. Verified fields: ${result.affectedFields.join(", ") || "none"}.`;
      } else if (!response.ok && result.error) {
        message = `${result.error.code ?? "save_failed"}: ${result.error.message ?? "Save failed."} Refresh inspection before a new decision.`;
        errors = result.error.details;
      }
    } catch {
      /* An admitted mutation cannot be described as unchanged. */
    }
    busyPhotos.delete(id);
    if (!current(id, epoch)) {
      if (alive) {
        if (photoId === id) {
          needsInspection = true;
          render();
          status.textContent = message;
        }
        announce(`Metadata Save for ${id}: ${message}`);
      }
      return;
    }
    saving = false;
    needsInspection = true;
    reviewed = false;
    if (success) {
      const inspection = inspect();
      const inspectionEpoch = generation;
      await inspection;
      if (current(id, inspectionEpoch))
        status.textContent = facts
          ? `${message} Fresh inspection establishes current content, not authorship.`
          : `${message} Fresh inspection failed. Refresh inspection before another Save.`;
    } else {
      render();
      status.textContent = message;
      // Display structured server validation detail beside every named affected field.
      if (errors && typeof errors === "object")
        for (const [key, detail] of Object.entries(errors)) {
          const node = errorNodes.get(key);
          if (node)
            node.textContent =
              typeof detail === "string" ? detail : JSON.stringify(detail);
        }
      if (
        errors &&
        typeof errors === "object" &&
        "fields" in errors &&
        Array.isArray(errors.fields)
      ) {
        const languages =
          "languages" in errors &&
          errors.languages &&
          typeof errors.languages === "object"
            ? errors.languages
            : {};
        for (const field of errors.fields)
          if (typeof field === "string") {
            const node = errorNodes.get(field);
            const required = Object.hasOwn(languages, field)
              ? (languages as Record<string, unknown>)[field]
              : undefined;
            if (node)
              node.textContent = required
                ? `${message} Required languages: ${JSON.stringify(required)}`
                : message;
          }
      }
      if (
        errors &&
        typeof errors === "object" &&
        "field" in errors &&
        typeof errors.field === "string"
      ) {
        const node = errorNodes.get(errors.field);
        if (node) node.textContent = message;
      }
      updateReview();
    }
  };
  return {
    show(id: string | undefined) {
      if (id === photoId && (facts || controller)) return;
      if (saving && photoId)
        announce(
          `Metadata Save for ${photoId} is still settling; leaving does not undo it. Refresh inspection before another Save.`,
        );
      saving = false;
      photoId = id;
      void inspect();
    },
    dispose() {
      alive = false;
      generation++;
      controller?.abort();
    },
  };
}
