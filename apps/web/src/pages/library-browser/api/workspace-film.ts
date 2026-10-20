import type { BrowserFetch } from "./browser-fetch.js";
import { isRecord } from "./guards.js";
import { fetchProcessingModules } from "./processing-modules.js";
export { fetchProcessingModules as fetchWorkspaceFilmModules };

export type WorkspaceFilmFile = Readonly<{
  workspaceId: string;
  artifactId: string;
  revision: string;
  imageContract: Record<string, unknown>;
  relativePath: string;
}>;
export type WorkspaceFilmStep = Readonly<{
  stepId: string;
  module: string;
  input: Record<string, unknown>;
  parameters: Readonly<{ schemaVersion: string; tree: unknown }>;
}>;
export type WorkspaceFilmRead = Readonly<{
  sourceRevision: string;
  recipe: Readonly<{
    revision: string;
    sourceRevision: string;
    currentStepId: string | null;
    steps: readonly WorkspaceFilmStep[];
  }> | null;
}>;

export class WorkspaceFilmError extends Error {
  constructor(
    message: string,
    readonly code: string,
    readonly uncertain = false,
  ) {
    super(message);
  }
}
export async function workspaceFilmJson(
  fetcher: BrowserFetch,
  path: string,
  init?: RequestInit,
): Promise<Record<string, unknown>> {
  let response: Response;
  try {
    response = await fetcher(path, init);
  } catch {
    throw new WorkspaceFilmError(
      "The service could not be reached. Reconcile the same request before continuing.",
      "connection",
      Boolean(init?.method),
    );
  }
  const body: unknown = await response.json().catch(() => undefined);
  if (!response.ok) {
    const error = isRecord(body) && isRecord(body.error) ? body.error : {};
    throw new WorkspaceFilmError(
      typeof error.message === "string"
        ? error.message
        : "Film operation failed.",
      typeof error.code === "string" ? error.code : "service",
      response.status >= 500 || error.code === "outcome_unknown",
    );
  }
  if (!isRecord(body))
    throw new WorkspaceFilmError(
      "The Film response could not be identified.",
      "invalid_response",
      Boolean(init?.method),
    );
  return body;
}
export function parseWorkspaceFilmRead(
  value: unknown,
  photoId: string,
): WorkspaceFilmRead {
  if (
    !isRecord(value) ||
    value.photoId !== photoId ||
    typeof value.sourceRevision !== "string"
  )
    throw new WorkspaceFilmError(
      "The Film state belongs to another Photo or is unreadable.",
      "invalid_response",
    );
  if (value.recipe === null)
    return { sourceRevision: value.sourceRevision, recipe: null };
  const recipe = value.recipe;
  if (
    !isRecord(recipe) ||
    recipe.photoId !== photoId ||
    typeof recipe.revision !== "string" ||
    typeof recipe.sourceRevision !== "string" ||
    (recipe.currentStepId !== null &&
      typeof recipe.currentStepId !== "string") ||
    !Array.isArray(recipe.steps)
  )
    throw new WorkspaceFilmError(
      "The saved Film state is unreadable.",
      "invalid_response",
    );
  const steps = recipe.steps.map((step: unknown): WorkspaceFilmStep => {
    if (
      !isRecord(step) ||
      typeof step.stepId !== "string" ||
      typeof step.module !== "string" ||
      !isRecord(step.input) ||
      !isRecord(step.parameters) ||
      typeof step.parameters.schemaVersion !== "string"
    )
      throw new WorkspaceFilmError(
        "A retained processing state is unreadable.",
        "invalid_response",
      );
    return {
      stepId: step.stepId,
      module: step.module,
      input: structuredClone(step.input),
      parameters: structuredClone(
        step.parameters,
      ) as WorkspaceFilmStep["parameters"],
    };
  });
  return {
    sourceRevision: value.sourceRevision,
    recipe: {
      revision: recipe.revision,
      sourceRevision: recipe.sourceRevision,
      currentStepId: recipe.currentStepId,
      steps,
    },
  };
}
export const workspaceFilmRoot = (photoId: string): string =>
  `/api/photos/${encodeURIComponent(photoId)}`;
export const readWorkspaceFilm = async (
  fetcher: BrowserFetch,
  photoId: string,
  signal?: AbortSignal,
): Promise<WorkspaceFilmRead> =>
  parseWorkspaceFilmRead(
    await workspaceFilmJson(
      fetcher,
      `${workspaceFilmRoot(photoId)}/processing-recipe`,
      signal ? { signal } : undefined,
    ),
    photoId,
  );
export const postWorkspaceFilm = (
  fetcher: BrowserFetch,
  path: string,
  body: string,
): Promise<Record<string, unknown>> =>
  workspaceFilmJson(fetcher, path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body,
  });
