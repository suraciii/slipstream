import type { BrowserFetch } from "../model/access-session.js";

export type MetadataValue =
  | string
  | number
  | boolean
  | string[]
  | Record<string, string>;
export interface MetadataSource {
  state: string;
  provenance?: string;
  value?: MetadataValue;
  problem?: string;
  languageAlternativesAvailable?: boolean;
}
export interface MetadataField extends MetadataSource {
  writable: boolean;
  inferredValue?: MetadataValue;
  sources: MetadataSource[];
}
export interface MetadataRead {
  photoId: string;
  originalLocation: string;
  association: { state: string; candidates: string[]; reason?: string };
  evidence: { sidecar: { state: string; location?: string; reason?: string } };
  fields: Record<string, MetadataField>;
  captureFacts: Record<
    string,
    MetadataSource & { identifier: string; unit: string }
  >;
  libraryRating: number;
  saveAvailable: boolean;
  saveUnavailableReason?: string;
}
export type MetadataChange =
  | { op: "set"; value: MetadataValue }
  | { op: "clear" | "remove" }
  | { op: "setLanguages"; languages: Record<string, string | null> };

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function validSource(value: unknown): boolean {
  if (
    !record(value) ||
    !["present", "absent", "invalid", "unavailable", "resource_limit"].includes(
      String(value.state),
    )
  )
    return false;
  const item = value.value;
  return (
    item === undefined ||
    typeof item === "string" ||
    typeof item === "boolean" ||
    (typeof item === "number" && Number.isFinite(item)) ||
    (Array.isArray(item)
      ? item.every((entry) => typeof entry === "string")
      : record(item) &&
        Object.values(item).every((entry) => typeof entry === "string"))
  );
}

// Keep the evidence JSON verbatim: filesystem u64 facts can exceed JS integer precision.
export function evidenceJson(text: string): string {
  let depth = 0;
  let start = -1;
  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (char === '"') {
      const begin = i++;
      for (; i < text.length; i++) {
        if (text[i] === "\\") i++;
        else if (text[i] === '"') break;
      }
      if (depth === 1 && JSON.parse(text.slice(begin, i + 1)) === "evidence") {
        let next = i + 1;
        while (/\s/.test(text[next] ?? "")) next++;
        if (text[next] !== ":") continue;
        next++;
        while (/\s/.test(text[next] ?? "")) next++;
        if (text[next] === "{") start = next;
      }
    } else if (char === "{" || char === "[") depth++;
    else if (char === "}" || char === "]") {
      depth--;
      if (start >= 0 && depth === 1) return text.slice(start, i + 1);
    }
  }
  throw new Error("Read Metadata did not return checked-save evidence.");
}

export async function readMetadata(
  fetcher: BrowserFetch,
  photoId: string,
  signal: AbortSignal,
): Promise<{ facts: MetadataRead; evidence: string }> {
  const response = await fetcher(
    `/api/photos/${encodeURIComponent(photoId)}/external-metadata`,
    { signal, cache: "no-store" },
  );
  const text = await response.text();
  const parsed: unknown = JSON.parse(text);
  if (!response.ok) {
    const error =
      record(parsed) && record(parsed.error) ? parsed.error : undefined;
    throw new Error(
      typeof error?.message === "string"
        ? error.message
        : `Read Metadata failed (${response.status}).`,
    );
  }
  if (
    !record(parsed) ||
    parsed.photoId !== photoId ||
    typeof parsed.originalLocation !== "string" ||
    typeof parsed.saveAvailable !== "boolean" ||
    !record(parsed.fields) ||
    !record(parsed.captureFacts) ||
    !record(parsed.association) ||
    !Array.isArray(parsed.association.candidates) ||
    !parsed.association.candidates.every(
      (candidate) => typeof candidate === "string",
    ) ||
    !record(parsed.evidence) ||
    !record(parsed.evidence.sidecar) ||
    !Object.values(parsed.fields).every(
      (field) =>
        validSource(field) &&
        record(field) &&
        typeof field.writable === "boolean" &&
        Array.isArray(field.sources) &&
        field.sources.every(validSource),
    ) ||
    !Object.values(parsed.captureFacts).every(
      (fact) =>
        validSource(fact) &&
        record(fact) &&
        typeof fact.identifier === "string" &&
        typeof fact.unit === "string",
    )
  )
    throw new Error(
      "Read Metadata returned an unexpected Photo or incomplete facts.",
    );
  const body = parsed as unknown as MetadataRead;
  return { facts: body, evidence: evidenceJson(text) };
}
