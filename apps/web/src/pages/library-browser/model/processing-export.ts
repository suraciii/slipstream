import { isRecord } from "../api/guards.js";
import type { ComposableRecipeInput } from "../api/composable-recipe.js";

export type ProcessingExportWork = Readonly<{
  photoId: string;
  requestId: string;
  stepId: string;
  module: string;
  recipeRevision: string;
  sourceRevision: string;
  state: "accepted" | "executing" | "succeeded" | "failed" | "cancelled";
  artifactId: string | null;
  failureReason: string | null;
  acceptedAt: number | null;
  terminalAt: number | null;
  retainUntil: number | null;
  parameters: Readonly<{ schemaVersion: string; tree: unknown }> | null;
  input: ComposableRecipeInput | null;
  bundleId: string;
}>;

export function parseProcessingExportWork(
  value: unknown,
): ProcessingExportWork | undefined {
  if (!isRecord(value)) return;
  for (const key of [
    "photoId",
    "requestId",
    "stepId",
    "module",
    "recipeRevision",
    "sourceRevision",
    "bundleId",
  ])
    if (typeof value[key] !== "string" || !value[key]) return;
  if (
    (value["sourceRevision"] as string).length > 16_384 ||
    (value["recipeRevision"] as string).length > 128 ||
    (value["requestId"] as string).length > 128
  )
    return;
  if (
    !["accepted", "executing", "succeeded", "failed", "cancelled"].includes(
      String(value["state"]),
    )
  )
    return;
  const string = (key: string) =>
    typeof value[key] === "string" ? value[key] : "";
  const count = (key: string) =>
    typeof value[key] === "number" && Number.isFinite(value[key])
      ? value[key]
      : null;
  const parameters = value["parameters"];
  const input = value["input"];
  if (
    !isRecord(parameters) ||
    typeof parameters["schemaVersion"] !== "string" ||
    !parameters["schemaVersion"] ||
    !("tree" in parameters)
  )
    return;
  if (
    !isRecord(input) ||
    !(
      (input["kind"] === "original" &&
        typeof input["photoId"] === "string" &&
        typeof input["sourceRevision"] === "string") ||
      (input["kind"] === "artifact" &&
        typeof input["artifactId"] === "string" &&
        isRecord(input["contract"]))
    )
  )
    return;
  if (
    value["state"] === "succeeded" &&
    (typeof value["artifactId"] !== "string" || !value["artifactId"])
  )
    return;
  return Object.freeze({
    photoId: string("photoId"),
    requestId: string("requestId"),
    stepId: string("stepId"),
    module: string("module"),
    recipeRevision: string("recipeRevision"),
    sourceRevision: string("sourceRevision"),
    state: value["state"] as ProcessingExportWork["state"],
    artifactId: string("artifactId") || null,
    failureReason: string("failureReason") || null,
    acceptedAt: count("acceptedAt"),
    terminalAt: count("terminalAt"),
    retainUntil: count("retainUntil"),
    parameters:
      isRecord(parameters) && typeof parameters["schemaVersion"] === "string"
        ? Object.freeze({
            schemaVersion: parameters["schemaVersion"],
            tree: parameters["tree"],
          })
        : null,
    input:
      isRecord(input) &&
      (input["kind"] === "original" || input["kind"] === "artifact")
        ? (input as ComposableRecipeInput)
        : null,
    bundleId: string("bundleId"),
  });
}
