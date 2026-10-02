import { describe, expect, test } from "bun:test";
import { parseProcessingModules } from "./processing-modules.js";

describe("processing module discovery", () => {
  test("preserves peer availability and module-owned schema trees", () => {
    const modules = parseProcessingModules({
      contractVersion: "slipstream-module-contract-1",
      modules: [
        {
          id: { name: "darktable", adapterVersion: "darktable-adapter-1" },
          parameterVersions: ["darktable-params-1"],
          parameterSchema: { operations: [{ operation: "exposure" }] },
          admittedInputs: [],
          admittedOutputs: [],
          limits: {
            maxInputBytes: 10,
            maxParameterBytes: 11,
            maxOutputPixels: 12,
            deadlineMillis: 13,
          },
          availability: { state: "ready", refusalReasons: [] },
        },
        {
          id: { name: "spektrafilm", adapterVersion: "spektrafilm-adapter-1" },
          parameterVersions: ["spektrafilm-params-1"],
          parameterSchema: {
            groups: { scanner: { grain: { type: "number" } } },
          },
          admittedInputs: [],
          admittedOutputs: [],
          limits: {
            maxInputBytes: 20,
            maxParameterBytes: 21,
            maxOutputPixels: 22,
            deadlineMillis: 23,
          },
          availability: {
            state: "unavailable",
            refusalReasons: ["runtime-missing"],
          },
        },
      ],
    });
    expect(modules?.map((module) => module.id.name)).toEqual([
      "darktable",
      "spektrafilm",
    ]);
    expect(modules?.[0]?.availability.state).toBe("ready");
    expect(modules?.[1]?.availability.refusalReasons).toEqual([
      "runtime-missing",
    ]);
    expect(modules?.[1]?.parameterSchema).toEqual({
      groups: { scanner: { grain: { type: "number" } } },
    });
  });

  test("refuses an incomplete discovery envelope", () => {
    expect(
      parseProcessingModules({ contractVersion: "x", modules: [{}] }),
    ).toBeUndefined();
    expect(parseProcessingModules({ modules: [] })).toBeUndefined();
  });
});
