import type { BrowserFetch } from "../model/access-session.js";
import { isRecord } from "./editor.js";

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
  steps: ReadonlyArray<ComposableProcessingStep>;
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
        steps: Object.freeze(steps),
      })
    : undefined;
};

export const parseComposableRecipe = (
  value: unknown,
  photoId: string,
):
  | Readonly<{ sourceRevision: string; recipe: ComposableRecipe | null }>
  | undefined => {
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
      recipe: null,
    });
  const recipe = readRecipe(rawRecipe);
  return recipe
    ? Object.freeze({ sourceRevision: value["sourceRevision"], recipe })
    : undefined;
};

export const fetchComposableRecipe = async (
  fetcher: BrowserFetch,
  photoId: string,
  signal: AbortSignal,
): Promise<
  | Readonly<{ sourceRevision: string; recipe: ComposableRecipe | null }>
  | undefined
> => {
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
