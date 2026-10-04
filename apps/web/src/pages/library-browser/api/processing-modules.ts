import type { BrowserFetch } from "../model/access-session.js";
import { isRecord } from "./guards.js";

export type ProcessingModuleAvailability = Readonly<{
  state: "ready" | "unavailable";
  refusalReasons: ReadonlyArray<string>;
}>;

export type AutomaticAdjustmentDescription = Readonly<{
  operation: string;
  label: string;
  multiPriority: number;
  instruction: Record<string, unknown>;
}>;

export type ProcessingModuleDescription = Readonly<{
  id: Readonly<{ name: string; adapterVersion: string }>;
  parameterVersions: ReadonlyArray<string>;
  parameterSchema: unknown;
  automaticAdjustments: ReadonlyArray<AutomaticAdjustmentDescription>;
  admittedInputs: ReadonlyArray<Record<string, unknown>>;
  admittedOutputs: ReadonlyArray<Record<string, unknown>>;
  limits: Readonly<Record<string, number>>;
  availability: ProcessingModuleAvailability;
}>;

const isStringArray = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((item) => typeof item === "string");

const readDescription = (
  value: unknown,
): ProcessingModuleDescription | undefined => {
  if (!isRecord(value) || !isRecord(value.id) || !isRecord(value.availability))
    return undefined;
  const name = value.id["name"];
  const adapterVersion = value.id["adapterVersion"];
  const parameterVersions = value["parameterVersions"];
  const state = value.availability["state"];
  const refusalReasons = value.availability["refusalReasons"];
  const limits = value["limits"];
  if (
    typeof name !== "string" ||
    typeof adapterVersion !== "string" ||
    !isStringArray(parameterVersions) ||
    (state !== "ready" && state !== "unavailable") ||
    !isStringArray(refusalReasons) ||
    !isRecord(limits) ||
    !Object.values(limits).every(
      (item) => typeof item === "number" && Number.isFinite(item),
    )
  )
    return undefined;
  const readContracts = (
    item: unknown,
  ): ReadonlyArray<Record<string, unknown>> =>
    Array.isArray(item) && item.every(isRecord) ? item : [];
  const readAutomaticAdjustments = (
    item: unknown,
  ): ReadonlyArray<AutomaticAdjustmentDescription> =>
    Array.isArray(item)
      ? item.flatMap((entry) => {
          if (
            !isRecord(entry) ||
            typeof entry["operation"] !== "string" ||
            typeof entry["label"] !== "string" ||
            typeof entry["multiPriority"] !== "number" ||
            !Number.isFinite(entry["multiPriority"]) ||
            !isRecord(entry["instruction"])
          )
            return [];
          return [
            Object.freeze({
              operation: entry["operation"],
              label: entry["label"],
              multiPriority: entry["multiPriority"],
              instruction: Object.freeze({ ...entry["instruction"] }),
            }),
          ];
        })
      : [];
  const parameterSchema = value["parameterSchema"];
  const automaticAdjustments = isRecord(parameterSchema)
    ? readAutomaticAdjustments(parameterSchema["x-automatic-adjustments"])
    : [];
  return Object.freeze({
    id: Object.freeze({ name, adapterVersion }),
    parameterVersions: Object.freeze([...parameterVersions]),
    parameterSchema: value["parameterSchema"],
    automaticAdjustments: Object.freeze([...automaticAdjustments]),
    admittedInputs: Object.freeze(readContracts(value["admittedInputs"])),
    admittedOutputs: Object.freeze(readContracts(value["admittedOutputs"])),
    limits: Object.freeze({ ...limits } as Record<string, number>),
    availability: Object.freeze({
      state,
      refusalReasons: Object.freeze([...refusalReasons]),
    }),
  });
};

export const parseProcessingModules = (
  value: unknown,
): ReadonlyArray<ProcessingModuleDescription> | undefined => {
  if (!isRecord(value) || typeof value["contractVersion"] !== "string")
    return undefined;
  const modules = value["modules"];
  if (!Array.isArray(modules)) return undefined;
  const parsed = modules.map(readDescription);
  return parsed.every(
    (item): item is ProcessingModuleDescription => item !== undefined,
  )
    ? Object.freeze(parsed)
    : undefined;
};

export const fetchProcessingModules = async (
  fetcher: BrowserFetch,
  signal: AbortSignal,
): Promise<ReadonlyArray<ProcessingModuleDescription> | undefined> => {
  try {
    const response = await fetcher("/api/processing/modules", {
      signal,
      priority: "high",
    });
    if (!response.ok) return undefined;
    return parseProcessingModules(await response.json());
  } catch {
    return undefined;
  }
};
