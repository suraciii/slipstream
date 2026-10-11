import { describe, expect, test } from "bun:test";
import {
  newSemanticValue,
  replaceSemanticValue,
  selectSemanticBranch,
  semanticSchemaError,
  validateSemanticValue,
} from "./semantic-schema.js";

const nativeParameterSchema = {
  $schema: "https://json-schema.org/draft/2020-12/schema",
  type: "object",
  additionalProperties: false,
  properties: {
    exposure: { type: "number", minimum: -3, maximum: 3, unit: "EV" },
    enabled: { type: "boolean" },
    mode: { type: "string", enum: ["linear", "filmic"] },
  },
  required: ["exposure", "enabled", "mode"],
};

describe("semantic schema model", () => {
  test("validates the native output settings schema", () => {
    const schema = {
      type: "object",
      additionalProperties: false,
      required: [
        "format",
        "precision",
        "colorSpace",
        "transfer",
        "profile",
        "width",
        "height",
        "quality",
        "compression",
      ],
      properties: {
        format: { enum: ["tiff", "jpeg", "png"] },
        precision: { enum: ["float32", "uint8", "uint16"] },
        colorSpace: { enum: ["ProPhoto RGB", "sRGB"] },
        transfer: { enum: ["linear", "srgb"] },
        profile: {
          type: "object",
          additionalProperties: false,
          required: ["id", "version"],
          properties: {
            id: { enum: ["linear-prophoto", "srgb"] },
            version: { const: "1" },
          },
        },
        width: { type: "integer", minimum: 0, maximum: 32768 },
        height: { type: "integer", minimum: 0, maximum: 32768 },
        quality: { type: "integer", minimum: 1, maximum: 100 },
        compression: { enum: ["deflate", "none"] },
      },
    };
    const settings = {
      format: "tiff",
      precision: "float32",
      colorSpace: "ProPhoto RGB",
      transfer: "linear",
      profile: { id: "linear-prophoto", version: "1" },
      width: 0,
      height: 0,
      quality: 90,
      compression: "deflate",
    };
    expect(semanticSchemaError(schema)).toBeUndefined();
    expect(validateSemanticValue(schema, settings)).toEqual([]);
    expect(
      validateSemanticValue(schema, { ...settings, quality: 101 }),
    ).toEqual(["quality: Maximum is 100."]);
  });

  test("validates native generated scalar, enum, object and required constraints", () => {
    expect(semanticSchemaError(nativeParameterSchema)).toBeUndefined();
    expect(
      validateSemanticValue(nativeParameterSchema, {
        exposure: 2,
        enabled: false,
        mode: "linear",
        internal: 7,
      }),
    ).toEqual([]);
    expect(
      validateSemanticValue(nativeParameterSchema, {
        exposure: 4,
        enabled: true,
      }),
    ).toEqual(["mode is required.", "exposure: Maximum is 3."]);
    expect(
      validateSemanticValue(nativeParameterSchema, {
        exposure: 1,
        enabled: true,
        mode: "other",
      }),
    ).toEqual(["mode: Choose a published value."]);
  });

  test("replacement preserves unknown and unexposed fields without mutating the input", () => {
    const original = {
      stack: [
        {
          operation: "tone",
          parameters: { strength: 2, hidden: [1, 2] },
          internal: "engine",
        },
      ],
      output: {
        format: "tiff",
        pins: { resourceId: "profile-1", version: "1" },
      },
    };
    const changed = replaceSemanticValue(
      original,
      ["stack", 0, "parameters", "strength"],
      0.5,
    );
    expect(changed).toEqual({
      stack: [
        {
          operation: "tone",
          parameters: { strength: 0.5, hidden: [1, 2] },
          internal: "engine",
        },
      ],
      output: {
        format: "tiff",
        pins: { resourceId: "profile-1", version: "1" },
      },
    });
    expect(original.stack[0]?.parameters.strength).toBe(2);
    expect(
      replaceSemanticValue(original, ["stack", 0], undefined, true),
    ).toEqual({
      stack: [],
      output: {
        format: "tiff",
        pins: { resourceId: "profile-1", version: "1" },
      },
    });
  });

  test("bounds are reported without clamping and integers remain exact", () => {
    const schema = { type: "integer", minimum: 0, maximum: 10, multipleOf: 2 };
    expect(validateSemanticValue(schema, 12)).toEqual(["Maximum is 10."]);
    expect(validateSemanticValue(schema, 3)).toEqual([
      "Value must be a multiple of 2.",
    ]);
    expect(
      validateSemanticValue({ type: "number", exclusiveMinimum: 0 }, -0),
    ).toEqual(["Value must exceed 0."]);
  });

  test("nullable and resource reference objects admit null and exact identifier pairs", () => {
    const nullableProfile = {
      type: ["object", "null"],
      properties: {
        id: { type: "string", minLength: 1 },
        version: { const: "1" },
      },
      required: ["id", "version"],
    };
    expect(validateSemanticValue(nullableProfile, null)).toEqual([]);
    expect(
      validateSemanticValue(nullableProfile, {
        id: "linear-prophoto",
        version: "1",
      }),
    ).toEqual([]);
    expect(
      validateSemanticValue(nullableProfile, { id: "linear-prophoto" }),
    ).toEqual(["version is required."]);
    expect(newSemanticValue(nullableProfile)).toEqual({ version: "1" });
    const anyOfNullable = { anyOf: [nullableProfile, { type: "null" }] };
    expect(validateSemanticValue(anyOfNullable, null)).toEqual([]);
    expect(
      validateSemanticValue(anyOfNullable, { id: "srgb", version: "1" }),
    ).toEqual([]);
    expect(
      selectSemanticBranch(
        { id: "srgb", version: "1" },
        { type: "null" },
        anyOfNullable,
      ),
    ).toBeNull();
    expect(
      semanticSchemaError({
        ...nullableProfile,
        resourceKind: "icc-profile",
        initializationSchema: {
          type: "object",
          properties: { mode: { const: "target-camera" } },
          required: ["mode"],
        },
      }),
    ).toBeUndefined();
  });

  test("arrays and tuples admit nodes only within published bounds and order", () => {
    const curve = {
      type: "array",
      minItems: 2,
      maxItems: 5,
      items: {
        prefixItems: [
          { type: "number", minimum: 0, maximum: 1 },
          { type: "number", minimum: 0, maximum: 1 },
        ],
        minItems: 2,
        maxItems: 2,
      },
    };
    expect(
      validateSemanticValue(curve, [
        [0, 0.5],
        [1, 1],
      ]),
    ).toEqual([]);
    expect(validateSemanticValue(curve, [[0, 0.5]])).toEqual([
      "At least 2 nodes are required.",
    ]);
    expect(
      validateSemanticValue(curve, [
        [0, 0.5],
        [2, 1],
      ]),
    ).toEqual(["Node 2: Node 1: Maximum is 1."]);
  });

  test("oneOf and anyOf matching are decisive", () => {
    const one = { oneOf: [{ type: "string" }, { type: "integer" }] };
    expect(validateSemanticValue(one, "sRGB")).toEqual([]);
    expect(validateSemanticValue(one, 4)).toEqual([]);
    expect(validateSemanticValue(one, true)).toEqual([
      "Value must match exactly one branch.",
    ]);
    const overlap = { oneOf: [{ type: "integer" }, { type: "number" }] };
    expect(validateSemanticValue(overlap, 1)).toEqual([
      "Value must match exactly one branch.",
    ]);
    expect(
      validateSemanticValue(
        { anyOf: [{ type: "integer" }, { type: "number" }] },
        1.5,
      ),
    ).toEqual([]);
  });

  test("branch selection applies published constants/defaults and keeps preserved object fields", () => {
    const branch = {
      type: "object",
      properties: {
        kind: { const: "curve" },
        points: { type: "array", default: [] },
      },
      required: ["kind", "points"],
    };
    expect(selectSemanticBranch({ legacy: true }, branch, branch)).toEqual({
      legacy: true,
      kind: "curve",
      points: [],
    });
    expect(
      selectSemanticBranch({ kind: "other", legacy: true }, branch, branch),
    ).toEqual({ kind: "curve", legacy: true, points: [] });
    expect(
      selectSemanticBranch(
        { kind: "other", points: [{ x: 1 }] },
        branch,
        branch,
      ),
    ).toEqual({ kind: "curve", points: [{ x: 1 }] });
  });

  test("local references resolve and remote references are explicit schema errors", () => {
    const schema = {
      $defs: { point: { type: "number" } },
      type: "object",
      properties: { x: { $ref: "#/$defs/point" } },
    };
    expect(validateSemanticValue(schema, { x: 4 })).toEqual([]);
    expect(validateSemanticValue(schema, { x: "4" })).toContain(
      "x: Expected number.",
    );
    const chained = {
      $defs: {
        text: { type: "string", minLength: 2 },
        alias: { $ref: "#/$defs/text" },
      },
      $ref: "#/$defs/alias",
    };
    expect(validateSemanticValue(chained, "ok")).toEqual([]);
    expect(validateSemanticValue(chained, "x")).toContain(
      "Use at least 2 characters.",
    );
    expect(validateSemanticValue(chained, 4)).toContain("Expected string.");
    expect(semanticSchemaError(schema)).toBeUndefined();
    expect(
      semanticSchemaError({
        $defs: { self: { $ref: "#/$defs/self" } },
        $ref: "#/$defs/self",
      }),
    ).toContain("Recursive schema reference");
    expect(
      semanticSchemaError({ $ref: "https://example.test/schema.json" }),
    ).toContain("Only local JSON Schema references");
  });

  test("unsupported keywords and malformed constraints produce explicit read-only errors", () => {
    expect(semanticSchemaError({ type: "number", not: {} })).toContain(
      "Unsupported schema keyword: not",
    );
    expect(
      semanticSchemaError({ type: "number", minimum: 5, maximum: 1 }),
    ).toContain("Schema minimum exceeds maximum");
    expect(semanticSchemaError({ type: "string", pattern: "[" })).toBe(
      "Invalid schema string pattern.",
    );
    expect(semanticSchemaError({ type: "array", maxItems: 1.5 })).toContain(
      "Schema maxItems must be a nonnegative integer",
    );
    expect(semanticSchemaError({ type: ["string", "number"] })).toContain(
      "Multiple non-null types",
    );
  });

  test("schema defaults create complete structures without inventing scalar defaults", () => {
    const schema = {
      type: "object",
      properties: {
        curve: {
          type: "array",
          items: {
            type: "object",
            properties: { x: { default: 0 }, y: { type: "number" } },
          },
        },
        mode: { const: "linear" },
        amount: { type: "number" },
      },
    };
    expect(newSemanticValue(schema)).toEqual({ curve: [], mode: "linear" });
  });
});
