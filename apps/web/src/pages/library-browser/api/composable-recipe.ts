import type { BrowserFetch } from "../model/access-session.js";
import { isRecord } from "./guards.js";

export type ComposableRecipeInput = Readonly<
  | { kind: "original"; photoId: string; sourceRevision: string }
  | {
      kind: "artifact";
      artifactId: string;
      contract: Record<string, unknown>;
    }
>;

export type ComposableProcessingStep = Readonly<{
  stepId: string;
  module: string;
  input: ComposableRecipeInput;
  parameters: Readonly<{ schemaVersion: string; tree: unknown }>;
}>;

export type ComposableRecipe = Readonly<{
  photoId: string;
  revision: string;
  sourceRevision: string;
  currentStepId: string | null;
  executionRefusals?: ReadonlyArray<
    Readonly<{ stepId: string; code: string; message: string }>
  >;
  steps: ReadonlyArray<ComposableProcessingStep>;
}>;

export type ComposableRecipeRead = Readonly<{
  sourceRevision: string;
  currentSourceRevision?: string | null;
  sourceAvailable?: boolean;
  recipe: ComposableRecipe | null;
}>;

const readInput = (value: unknown): ComposableRecipeInput | undefined => {
  if (!isRecord(value) || typeof value["kind"] !== "string") return undefined;
  if (
    value["kind"] === "original" &&
    typeof value["photoId"] === "string" &&
    typeof value["sourceRevision"] === "string"
  )
    return Object.freeze({
      kind: "original",
      photoId: value["photoId"],
      sourceRevision: value["sourceRevision"],
    });
  if (
    value["kind"] === "artifact" &&
    typeof value["artifactId"] === "string" &&
    isRecord(value["contract"])
  )
    return Object.freeze({
      kind: "artifact",
      artifactId: value["artifactId"],
      contract: Object.freeze({ ...value["contract"] }),
    });
  return undefined;
};

const readRecipe = (value: unknown): ComposableRecipe | undefined => {
  if (
    !isRecord(value) ||
    typeof value["photoId"] !== "string" ||
    typeof value["revision"] !== "string" ||
    typeof value["sourceRevision"] !== "string" ||
    (value["currentStepId"] !== null &&
      typeof value["currentStepId"] !== "string") ||
    !Array.isArray(value["steps"])
  )
    return undefined;
  const steps = value["steps"].map(
    (step): ComposableProcessingStep | undefined => {
      if (
        !isRecord(step) ||
        typeof step["stepId"] !== "string" ||
        typeof step["module"] !== "string" ||
        !isRecord(step["parameters"]) ||
        typeof step["parameters"]["schemaVersion"] !== "string"
      )
        return undefined;
      const input = readInput(step["input"]);
      return input
        ? Object.freeze({
            stepId: step["stepId"],
            module: step["module"],
            input,
            parameters: Object.freeze({
              schemaVersion: step["parameters"]["schemaVersion"],
              tree: step["parameters"]["tree"],
            }),
          })
        : undefined;
    },
  );
  return steps.every(
    (step): step is ComposableProcessingStep => step !== undefined,
  )
    ? Object.freeze({
        photoId: value["photoId"],
        revision: value["revision"],
        sourceRevision: value["sourceRevision"],
        currentStepId: value["currentStepId"],
        ...(Array.isArray(value["executionRefusals"])
          ? {
              executionRefusals: value["executionRefusals"].filter(
                (
                  item,
                ): item is { stepId: string; code: string; message: string } =>
                  isRecord(item) &&
                  typeof item["stepId"] === "string" &&
                  typeof item["code"] === "string" &&
                  typeof item["message"] === "string",
              ),
            }
          : {}),
        steps: Object.freeze(steps),
      })
    : undefined;
};

export const parseComposableRecipe = (
  value: unknown,
  photoId: string,
): ComposableRecipeRead | undefined => {
  if (
    !isRecord(value) ||
    value["photoId"] !== photoId ||
    typeof value["sourceRevision"] !== "string"
  )
    return undefined;
  const rawRecipe = value["recipe"];
  if (rawRecipe === null)
    return Object.freeze({
      sourceRevision: value["sourceRevision"],
      ...(typeof value["currentSourceRevision"] === "string" ||
      value["currentSourceRevision"] === null
        ? { currentSourceRevision: value["currentSourceRevision"] }
        : {}),
      ...(typeof value["sourceAvailable"] === "boolean"
        ? { sourceAvailable: value["sourceAvailable"] }
        : {}),
      recipe: null,
    });
  const recipe = readRecipe(rawRecipe);
  return recipe && recipe.photoId === photoId
    ? Object.freeze({
        sourceRevision: value["sourceRevision"],
        recipe,
        ...(typeof value["currentSourceRevision"] === "string" ||
        value["currentSourceRevision"] === null
          ? { currentSourceRevision: value["currentSourceRevision"] }
          : {}),
        ...(typeof value["sourceAvailable"] === "boolean"
          ? { sourceAvailable: value["sourceAvailable"] }
          : {}),
      })
    : undefined;
};

export const fetchComposableRecipe = async (
  fetcher: BrowserFetch,
  photoId: string,
  signal: AbortSignal,
): Promise<ComposableRecipeRead | undefined> => {
  try {
    const response = await fetcher(
      `/api/photos/${encodeURIComponent(photoId)}/processing-recipe`,
      { signal, priority: "high" },
    );
    if (!response.ok) return undefined;
    return parseComposableRecipe(await response.json(), photoId);
  } catch {
    return undefined;
  }
};

export const saveComposableRecipe = async (
  fetcher: BrowserFetch,
  photoId: string,
  request: Readonly<{
    requestId: string;
    expectedRecipeRevision: string | null;
    expectedSourceRevision: string;
    currentStepId: string | null;
    steps: ReadonlyArray<ComposableProcessingStep>;
  }>,
  signal?: AbortSignal,
): Promise<Response> =>
  fetcher(`/api/photos/${encodeURIComponent(photoId)}/processing-recipe`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    ...(signal ? { signal } : {}),
    body: JSON.stringify({
      requestId: request.requestId,
      expectedRecipeRevision: request.expectedRecipeRevision,
      expectedSourceRevision: request.expectedSourceRevision,
      currentStepId: request.currentStepId,
      steps: request.steps,
    }),
  });

/** Reconciliation must replay the captured bytes, including its request identity. */
export const saveComposableRecipeBody = (
  fetcher: BrowserFetch,
  photoId: string,
  body: string,
  rebind = false,
): Promise<Response> =>
  fetcher(
    `/api/photos/${encodeURIComponent(photoId)}/processing-recipe${rebind ? "/rebind" : ""}`,
    { method: "POST", headers: { "Content-Type": "application/json" }, body },
  );

export const rebindComposableRecipe = (
  fetcher: BrowserFetch,
  photoId: string,
  request: Readonly<{
    requestId: string;
    expectedRecipeRevision: string;
    newSourceRevision: string;
  }>,
): Promise<Response> =>
  saveComposableRecipeBody(fetcher, photoId, JSON.stringify(request), true);
