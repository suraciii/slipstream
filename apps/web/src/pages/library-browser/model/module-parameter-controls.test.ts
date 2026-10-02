import { describe, expect, test } from "bun:test";
import {
  moduleParameterControls,
  replaceModuleParameter,
} from "./module-parameter-controls.js";

describe("module-owned editor controls", () => {
  test("only admitted stack controls are editable and unsupported intent survives", () => {
    const tree = {
      stack: [
        {
          operation: "exposure",
          parameters: { exposure: 1, temperature: 7200 },
        },
      ],
      output: { format: "tiff" },
    };
    const schema = {
      type: "object",
      properties: {
        stack: {
          type: "array",
          items: {
            type: "object",
            properties: {
              operation: { const: "exposure" },
              parameters: {
                type: "object",
                properties: {
                  exposure: { type: "number", minimum: -3, maximum: 3 },
                },
              },
            },
          },
        },
        output: { const: { format: "tiff" } },
      },
    };
    const controls = moduleParameterControls(schema, tree);
    expect(
      controls
        .filter((control) => !control.fixed)
        .map((control) => control.path),
    ).toEqual([["stack", 0, "parameters", "exposure"]]);
    const changed = replaceModuleParameter(
      tree,
      ["stack", 0, "parameters", "exposure"],
      2,
    );
    expect(changed).toEqual({
      stack: [
        {
          operation: "exposure",
          parameters: { exposure: 2, temperature: 7200 },
        },
      ],
      output: { format: "tiff" },
    });
    expect(tree.stack[0]?.parameters.exposure).toBe(1);
  });
  test("pinned Film groups never become editable scalar controls", () => {
    const group = { grain: 0.5, enabled: true };
    expect(
      moduleParameterControls(
        { type: "object", properties: { effects: { const: group } } },
        { effects: group },
      ),
    ).toEqual([
      {
        path: ["effects"],
        label: "effects",
        value: group,
        fixed: true,
        kind: "fixed",
      },
    ]);
  });
  test("discriminated stack entries select their own admitted schema", () => {
    const schema = {
      type: "object",
      properties: {
        stack: {
          type: "array",
          items: {
            oneOf: [
              {
                properties: {
                  operation: { const: "exposure" },
                  value: { type: "number" },
                },
              },
              {
                properties: {
                  operation: { const: "unsupported" },
                  value: { const: 3 },
                },
              },
            ],
          },
        },
      },
    };
    const controls = moduleParameterControls(schema, {
      stack: [
        { operation: "unsupported", value: 3 },
        { operation: "exposure", value: 1 },
      ],
    });
    expect(
      controls
        .filter((control) => !control.fixed)
        .map((control) => control.path),
    ).toEqual([["stack", 1, "value"]]);
  });
  test("conditional exposure schema keeps native defaults fixed", () => {
    const schema = {
      "x-qualification": "manual exposure only",
      type: "object",
      properties: {
        stack: {
          type: "array",
          items: {
            properties: {
              operation: { type: "string" },
              params: { type: "object" },
            },
            allOf: [
              {
                if: { properties: { operation: { const: "exposure" } } },
                then: {
                  properties: {
                    params: {
                      type: "object",
                      properties: {
                        exposure: {
                          type: "number",
                          "x-qualification": "editable-manual-exposure",
                        },
                        black: {
                          type: "number",
                          "x-qualification": "fixed-qualified-default",
                        },
                      },
                    },
                  },
                },
              },
            ],
          },
        },
      },
    };
    const controls = moduleParameterControls(schema, {
      stack: [
        { operation: "exposure", params: { exposure: 1, black: 0 } },
        { operation: "temperature", params: { temperatureKelvin: 7200 } },
      ],
    });
    expect(
      controls
        .filter((control) => !control.fixed)
        .map((control) => control.path),
    ).toEqual([["stack", 0, "params", "exposure"]]);
  });
});
