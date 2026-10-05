import type { BrowserFetch } from "../model/access-session.js";
import type { ComposableRecipeInput } from "./composable-recipe.js";
import { isRecord } from "./guards.js";

export type EditStateRead = Readonly<{
  photoId: string;
  editRevision: string | null;
  sourceRevision: string | null;
  currentSourceRevision: string | null;
  sourceAvailable: boolean;
  requiresRebind: boolean;
  canSave: boolean;
  canPreview: boolean;
  canExport: boolean;
  current: Readonly<{
    engine: string;
    input: ComposableRecipeInput;
    exposureEv: number | null;
  }> | null;
}>;

export function parseEditState(value: unknown): EditStateRead | undefined {
  if (!isRecord(value) || typeof value["photoId"] !== "string") return;
  for (const key of ["editRevision", "sourceRevision", "currentSourceRevision"])
    if (value[key] !== null && typeof value[key] !== "string") return;
  for (const key of [
    "sourceAvailable",
    "requiresRebind",
    "canSave",
    "canPreview",
    "canExport",
  ])
    if (typeof value[key] !== "boolean") return;
  let current: EditStateRead["current"] = null;
  if (value["current"] !== null) {
    const projection = value["current"];
    if (
      !isRecord(projection) ||
      typeof projection["engine"] !== "string" ||
      !isRecord(projection["input"])
    )
      return;
    const input = projection["input"];
    let binding: ComposableRecipeInput;
    if (
      input["kind"] === "original" &&
      typeof input["photoId"] === "string" &&
      typeof input["sourceRevision"] === "string"
    )
      binding = {
        kind: "original",
        photoId: input["photoId"],
        sourceRevision: input["sourceRevision"],
      };
    else if (
      input["kind"] === "artifact" &&
      typeof input["artifactId"] === "string" &&
      isRecord(input["contract"])
    )
      binding = {
        kind: "artifact",
        artifactId: input["artifactId"],
        contract: input["contract"],
      };
    else return;
    const controls = projection["controls"];
    const exposure = isRecord(controls) ? controls["exposure"] : undefined;
    const ev = isRecord(exposure) ? exposure["ev"] : undefined;
    current = {
      engine: projection["engine"],
      input: binding,
      exposureEv: typeof ev === "number" && Number.isFinite(ev) ? ev : null,
    };
  }
  return {
    photoId: value["photoId"],
    editRevision: value["editRevision"] as string | null,
    sourceRevision: value["sourceRevision"] as string | null,
    currentSourceRevision: value["currentSourceRevision"] as string | null,
    sourceAvailable: value["sourceAvailable"] as boolean,
    requiresRebind: value["requiresRebind"] as boolean,
    canSave: value["canSave"] as boolean,
    canPreview: value["canPreview"] as boolean,
    canExport: value["canExport"] as boolean,
    current,
  };
}

export async function fetchEditState(
  fetcher: BrowserFetch,
  photoId: string,
): Promise<EditStateRead | undefined> {
  const response = await fetcher(
    `/api/photos/${encodeURIComponent(photoId)}/edit`,
    { priority: "low" },
  );
  if (!response.ok) return;
  const state = parseEditState(await response.json());
  return state?.photoId === photoId ? state : undefined;
}
