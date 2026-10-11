import type { MetadataChange, MetadataRead } from "../api/external-metadata.js";

export const metadataFields = [
  ["dc:title", "Title", "languages"],
  ["dc:description", "Description", "languages"],
  ["dc:subject", "Keywords", "list"],
  ["photoshop:Headline", "Headline", "text"],
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

export function defaultLanguageChange(
  value: string,
): Extract<MetadataChange, { op: "setLanguages" }> {
  return { op: "setLanguages", languages: { "x-default": value } };
}

export interface MetadataLanguageInput {
  name: string;
  value: string;
  action: string;
}
export interface MetadataInput {
  operation: string;
  text: string;
  languages: MetadataLanguageInput[];
}

export function metadataInputChange(
  key: string,
  kind: string,
  input: MetadataInput,
): { change?: MetadataChange; error?: string } {
  if (input.operation === "clear" || input.operation === "remove")
    return { change: { op: input.operation } };
  if (kind === "languages") {
    const languages = Object.create(null) as Record<string, string | null>;
    if (input.operation === "set") languages["x-default"] = input.text;
    for (const row of input.languages) {
      if (row.action === "unchanged") continue;
      if (
        !row.name ||
        Object.keys(languages).some(
          (name) => name.toLowerCase() === row.name.toLowerCase(),
        )
      )
        return {
          error:
            "Name each changed language once. Change x-default either in the ordinary field or its named row.",
        };
      languages[row.name] = row.action === "remove" ? null : row.value;
    }
    return Object.keys(languages).length
      ? { change: { op: "setLanguages", languages } }
      : {};
  }
  if (input.operation !== "set") return {};
  if (kind === "number") {
    const value = input.text === "" ? NaN : Number(input.text);
    return !Number.isFinite(value) || (value !== -1 && (value < 0 || value > 5))
      ? { error: "Rating must be -1 or a number from 0 through 5." }
      : { change: { op: "set", value } };
  }
  if (kind === "boolean" && !["true", "false"].includes(input.text))
    return { error: "Choose true or false." };
  const value =
    kind === "boolean"
      ? input.text === "true"
      : kind === "list"
        ? input.text === ""
          ? []
          : key === "dc:subject"
            ? [...new Set(input.text.split("\n"))]
            : input.text.split("\n")
        : input.text;
  return { change: { op: "set", value } };
}
export interface MetadataDraft {
  facts: MetadataRead;
  evidence: string;
  changes: Record<string, MetadataChange>;
  inputs: Record<string, MetadataInput>;
  errors: Record<string, string>;
  reviewed: boolean;
  needsInspection: boolean;
}

// Session-local input is never evicted to make room for another Photo.
export function createMetadataDrafts(capacity = 32) {
  const drafts = new Map<string, MetadataDraft>();
  const pending = new Map<string, MetadataDraft>();
  return {
    get: (id: string) => drafts.get(id),
    isSaving: (id: string) => pending.has(id),
    admit(
      id: string,
      facts: MetadataRead,
      evidence: string,
    ): MetadataDraft | undefined {
      const existing = drafts.get(id);
      if (existing) return existing;
      if (drafts.size >= capacity) return undefined;
      const draft: MetadataDraft = {
        facts,
        evidence,
        changes: {},
        inputs: {},
        errors: {},
        reviewed: false,
        needsInspection: false,
      };
      drafts.set(id, draft);
      return draft;
    },
    discard(id: string) {
      if (pending.has(id)) return false;
      return drafts.delete(id);
    },
    changed(id: string, evidence: string) {
      const draft = drafts.get(id);
      return Boolean(draft && draft.evidence !== evidence);
    },
    acceptInspection(id: string, facts: MetadataRead, evidence: string) {
      const draft = drafts.get(id);
      if (!draft || pending.has(id)) return;
      draft.facts = facts;
      draft.evidence = evidence;
      draft.reviewed = false;
      draft.needsInspection = false;
    },
    beginSave(id: string) {
      const draft = drafts.get(id);
      if (
        !draft ||
        pending.has(id) ||
        !draft.reviewed ||
        draft.needsInspection ||
        Object.keys(draft.errors).length ||
        !Object.keys(draft.changes).length
      )
        return;
      pending.set(id, draft);
      draft.reviewed = false;
      return {
        draft,
        body: `{"evidence":${draft.evidence},"changes":${JSON.stringify(draft.changes)}}`,
      };
    },
    settle(id: string, draft: MetadataDraft, success: boolean) {
      if (pending.get(id) !== draft) return;
      pending.delete(id);
      if (drafts.get(id) !== draft) return;
      if (success) drafts.delete(id);
      else {
        draft.needsInspection = true;
        draft.reviewed = false;
      }
    },
    clear() {
      drafts.clear();
      pending.clear();
    },
  };
}
