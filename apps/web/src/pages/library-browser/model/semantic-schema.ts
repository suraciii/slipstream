export type SemanticSchema = Record<string, unknown>;
export type SemanticPath = readonly (string | number)[];

export function schemaObject(value: unknown): value is SemanticSchema {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function schemaArray(value: unknown): value is unknown[] {
  return Array.isArray(value);
}

export function sameSemanticValue(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (Array.isArray(a) && Array.isArray(b))
    return (
      a.length === b.length && a.every((v, i) => sameSemanticValue(v, b[i]))
    );
  if (schemaObject(a) && schemaObject(b)) {
    const keys = Object.keys(a);
    return (
      keys.length === Object.keys(b).length &&
      keys.every(
        (key) => Object.hasOwn(b, key) && sameSemanticValue(a[key], b[key]),
      )
    );
  }
  return false;
}

/** Copy only the edited path, retaining every unexposed sibling verbatim. */
export function replaceSemanticValue(
  value: unknown,
  path: SemanticPath,
  next: unknown,
  remove = false,
): unknown {
  if (!path.length) return next;
  const [key, ...rest] = path;
  if (typeof key === "number") {
    const result = schemaArray(value) ? [...value] : [];
    if (remove && !rest.length) result.splice(key, 1);
    else result[key] = replaceSemanticValue(result[key], rest, next, remove);
    return result;
  }
  const result = schemaObject(value) ? { ...value } : {};
  if (key === undefined) return next;
  if (remove && !rest.length) delete result[key];
  else
    Object.defineProperty(result, key, {
      value: replaceSemanticValue(result[key], rest, next, remove),
      writable: true,
      enumerable: true,
      configurable: true,
    });
  return result;
}

export function resolveSemanticSchema(
  schema: SemanticSchema,
  root: SemanticSchema,
): SemanticSchema {
  let current = schema;
  for (let depth = 0; current.$ref !== undefined; depth += 1) {
    if (depth >= 40)
      throw new Error(
        "Recursive schema reference exceeds the supported depth.",
      );
    const reference = current.$ref;
    if (typeof reference !== "string" || !reference.startsWith("#/"))
      throw new Error("Only local JSON Schema references are supported.");
    let target: unknown = root;
    for (const token of reference.slice(2).split("/")) {
      const key = token.replace(/~1/g, "/").replace(/~0/g, "~");
      if (!schemaObject(target) || !Object.hasOwn(target, key))
        throw new Error(`Unresolved schema reference: ${reference}`);
      target = target[key];
    }
    if (!schemaObject(target))
      throw new Error(`Invalid schema reference: ${reference}`);
    const siblings = { ...current };
    delete siblings.$ref;
    current = { ...target, ...siblings };
  }
  return current;
}

const supported: Record<string, true> = {
  $schema: true,
  $id: true,
  $defs: true,
  $ref: true,
  title: true,
  description: true,
  default: true,
  examples: true,
  deprecated: true,
  readOnly: true,
  writeOnly: true,
  type: true,
  const: true,
  enum: true,
  oneOf: true,
  anyOf: true,
  properties: true,
  required: true,
  additionalProperties: true,
  items: true,
  prefixItems: true,
  minItems: true,
  maxItems: true,
  uniqueItems: true,
  minimum: true,
  maximum: true,
  exclusiveMinimum: true,
  exclusiveMaximum: true,
  multipleOf: true,
  minLength: true,
  maxLength: true,
  pattern: true,
  unit: true,
  units: true,
  resourceKind: true,
  initializationSchema: true,
  format: true,
  contentMediaType: true,
  contentEncoding: true,
};

export function semanticSchemaError(
  input: SemanticSchema,
  root = input,
  depth = 0,
): string | undefined {
  if (depth > 40) return "Recursive schema exceeds the supported depth.";
  let schema: SemanticSchema;
  try {
    schema = resolveSemanticSchema(input, root);
  } catch (error) {
    return error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : "Schema could not be resolved.";
  }
  for (const key of Object.keys(schema)) {
    if (!Object.hasOwn(supported, key) && !key.startsWith("x-"))
      return `Unsupported schema keyword: ${key}. This value is read-only.`;
  }
  const types = Array.isArray(schema.type)
    ? schema.type
    : schema.type === undefined
      ? []
      : [schema.type];
  if (
    types.some(
      (type) =>
        ![
          "object",
          "array",
          "number",
          "integer",
          "boolean",
          "string",
          "null",
        ].includes(String(type)),
    )
  )
    return "Unsupported schema type.";
  if (schema.type !== undefined && !types.length)
    return "Schema type must not be empty.";
  if (types.filter((type) => type !== "null").length > 1)
    return "Multiple non-null types require oneOf or anyOf controls.";
  if (
    schema.enum !== undefined &&
    (!Array.isArray(schema.enum) || !schema.enum.length)
  )
    return "Schema enum must contain choices.";
  if (
    schema.required !== undefined &&
    (!Array.isArray(schema.required) ||
      schema.required.some((v) => typeof v !== "string"))
  )
    return "Schema required fields must be string names.";
  if (schema.properties !== undefined && !schemaObject(schema.properties))
    return "Schema properties must be an object.";
  for (const key of ["oneOf", "anyOf", "prefixItems"] as const) {
    if (
      schema[key] !== undefined &&
      (!Array.isArray(schema[key]) ||
        (key !== "prefixItems" && !schema[key].length))
    )
      return `Schema ${key} must be an array${key === "prefixItems" ? "" : " of choices"}.`;
  }
  for (const key of [
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
  ])
    if (
      schema[key] !== undefined &&
      (typeof schema[key] !== "number" || !Number.isFinite(schema[key]))
    )
      return `Schema ${key} must be a finite number.`;
  if (typeof schema.multipleOf === "number" && schema.multipleOf <= 0)
    return "Schema multipleOf must be positive.";
  for (const key of ["minItems", "maxItems", "minLength", "maxLength"])
    if (
      typeof schema[key] === "number" &&
      (!Number.isInteger(schema[key]) || schema[key] < 0)
    )
      return `Schema ${key} must be a nonnegative integer.`;
  for (const [lower, upper] of [
    ["minimum", "maximum"],
    ["minItems", "maxItems"],
    ["minLength", "maxLength"],
  ] as const)
    if (
      typeof schema[lower] === "number" &&
      typeof schema[upper] === "number" &&
      schema[lower] > schema[upper]
    )
      return `Schema ${lower} exceeds ${upper}.`;
  for (const key of ["readOnly", "writeOnly", "uniqueItems"])
    if (schema[key] !== undefined && typeof schema[key] !== "boolean")
      return `Schema ${key} must be boolean.`;
  if (schema.pattern !== undefined) {
    if (typeof schema.pattern !== "string")
      return "Schema pattern must be a string.";
    try {
      new RegExp(schema.pattern, "u");
    } catch {
      return "Invalid schema string pattern.";
    }
  }
  const children: unknown[] = [
    ...Object.values(schemaObject(schema.properties) ? schema.properties : {}),
    ...(schemaArray(schema.prefixItems) ? schema.prefixItems : []),
    ...(schemaArray(schema.oneOf) ? schema.oneOf : []),
    ...(schemaArray(schema.anyOf) ? schema.anyOf : []),
  ];
  for (const key of ["items", "additionalProperties"])
    if (schema[key] !== undefined && typeof schema[key] !== "boolean")
      children.push(schema[key]);
  for (const child of children) {
    if (typeof child === "boolean") continue;
    if (!schemaObject(child))
      return "Nested schema must be an object or boolean.";
    const error = semanticSchemaError(child, root, depth + 1);
    if (error) return error;
  }
  return undefined;
}

export function semanticType(schema: SemanticSchema): string | undefined {
  if (Array.isArray(schema.type))
    return (
      schema.type.find(
        (type): type is string => typeof type === "string" && type !== "null",
      ) ?? "null"
    );
  if (typeof schema.type === "string") return schema.type;
  if (schema.properties) return "object";
  if (schema.items !== undefined || schema.prefixItems) return "array";
  return undefined;
}

export function semanticItemSchema(
  schema: SemanticSchema,
  index: number,
): unknown {
  if (Array.isArray(schema.prefixItems) && index < schema.prefixItems.length)
    return schema.prefixItems[index];
  return schema.items ?? true;
}

/** Explicit creation uses only published defaults/constants, never guessed bounds. */
export function newSemanticValue(input: SemanticSchema, root = input): unknown {
  const schema = resolveSemanticSchema(input, root);
  if (Object.hasOwn(schema, "const")) return structuredClone(schema.const);
  if (Object.hasOwn(schema, "default")) return structuredClone(schema.default);
  if (semanticType(schema) === "object") {
    const result: Record<string, unknown> = {};
    if (schemaObject(schema.properties))
      for (const [key, child] of Object.entries(schema.properties)) {
        if (!schemaObject(child)) continue;
        const value = newSemanticValue(child, root);
        if (value !== undefined)
          Object.defineProperty(result, key, {
            value,
            enumerable: true,
            writable: true,
            configurable: true,
          });
      }
    return result;
  }
  if (semanticType(schema) === "array") return [];
  if (semanticType(schema) === "null") return null;
  return undefined;
}

/** Branch selection preserves existing object members and applies explicit discriminators only. */
export function selectSemanticBranch(
  value: unknown,
  branch: SemanticSchema,
  root: SemanticSchema,
): unknown {
  const schema = resolveSemanticSchema(branch, root);
  if (Object.hasOwn(schema, "const")) return structuredClone(schema.const);
  if (semanticType(schema) === "object") {
    let result: unknown = schemaObject(value)
      ? { ...value }
      : newSemanticValue(schema, root);
    if (schemaObject(schema.properties))
      for (const [key, child] of Object.entries(schema.properties)) {
        if (!schemaObject(child)) continue;
        const resolved = resolveSemanticSchema(child, root);
        if (Object.hasOwn(resolved, "const"))
          result = replaceSemanticValue(
            result,
            [key],
            structuredClone(resolved.const),
          );
        else if (
          schemaObject(result) &&
          !Object.hasOwn(result, key) &&
          Object.hasOwn(resolved, "default")
        )
          result = replaceSemanticValue(
            result,
            [key],
            structuredClone(resolved.default),
          );
      }
    return result;
  }
  return validateSemanticValue(schema, value, root).length
    ? newSemanticValue(schema, root)
    : value;
}

export function validateSemanticValue(
  input: SemanticSchema | boolean,
  value: unknown,
  root: SemanticSchema = typeof input === "boolean" ? {} : input,
  depth = 0,
): string[] {
  if (input === true) return [];
  if (input === false) return ["This value is not admitted by the schema."];
  if (depth > 40) return ["Value exceeds the supported schema depth."];
  let schema: SemanticSchema;
  try {
    schema = resolveSemanticSchema(input, root);
  } catch (error) {
    return [
      error instanceof Error
        ? error.message
        : typeof error === "string"
          ? error
          : "Schema could not be resolved.",
    ];
  }
  const errors: string[] = [];
  if (Object.hasOwn(schema, "const") && !sameSemanticValue(value, schema.const))
    errors.push("Value must equal the published constant.");
  if (
    Array.isArray(schema.enum) &&
    !schema.enum.some((choice) => sameSemanticValue(choice, value))
  )
    errors.push("Choose a published value.");
  for (const key of ["oneOf", "anyOf"] as const)
    if (Array.isArray(schema[key])) {
      const matches = schema[key].filter(
        (branch) =>
          (schemaObject(branch) || typeof branch === "boolean") &&
          !validateSemanticValue(branch, value, root, depth + 1).length,
      ).length;
      const branchError = key === "oneOf" ? matches !== 1 : matches === 0;
      if (branchError)
        errors.push(
          key === "oneOf"
            ? "Value must match exactly one branch."
            : "Value must match a branch.",
        );
      return branchError ? errors : [];
    }
  const types = Array.isArray(schema.type)
    ? schema.type
    : schema.type === undefined
      ? []
      : [schema.type];
  const matchesType = (type: unknown) => {
    if (typeof type !== "string") return false;
    const name = type;
    return name === "null"
      ? value === null
      : name === "object"
        ? schemaObject(value)
        : name === "array"
          ? Array.isArray(value)
          : name === "integer"
            ? typeof value === "number" && Number.isInteger(value)
            : name === "number"
              ? typeof value === "number" && Number.isFinite(value)
              : typeof value === name;
  };
  if (types.length && !types.some(matchesType))
    return [...errors, `Expected ${types.join(" or ")}.`];
  if (typeof value === "number") {
    if (!Number.isFinite(value)) errors.push("Enter a finite number.");
    if (typeof schema.minimum === "number" && value < schema.minimum)
      errors.push(`Minimum is ${schema.minimum}.`);
    if (typeof schema.maximum === "number" && value > schema.maximum)
      errors.push(`Maximum is ${schema.maximum}.`);
    if (
      typeof schema.exclusiveMinimum === "number" &&
      value <= schema.exclusiveMinimum
    )
      errors.push(`Value must exceed ${schema.exclusiveMinimum}.`);
    if (
      typeof schema.exclusiveMaximum === "number" &&
      value >= schema.exclusiveMaximum
    )
      errors.push(`Value must be below ${schema.exclusiveMaximum}.`);
    if (
      typeof schema.multipleOf === "number" &&
      Math.abs(
        value / schema.multipleOf - Math.round(value / schema.multipleOf),
      ) > 1e-9
    )
      errors.push(`Value must be a multiple of ${schema.multipleOf}.`);
  }
  if (typeof value === "string") {
    const length = [...value].length;
    if (typeof schema.minLength === "number" && length < schema.minLength)
      errors.push(`Use at least ${schema.minLength} characters.`);
    if (typeof schema.maxLength === "number" && length > schema.maxLength)
      errors.push(`Use at most ${schema.maxLength} characters.`);
    if (
      typeof schema.pattern === "string" &&
      !new RegExp(schema.pattern, "u").test(value)
    )
      errors.push("Value does not match the published pattern.");
  }
  if (schemaObject(value)) {
    const properties = schemaObject(schema.properties) ? schema.properties : {};
    if (Array.isArray(schema.required))
      for (const key of schema.required)
        if (typeof key === "string" && !Object.hasOwn(value, key))
          errors.push(`${key} is required.`);
    for (const [key, child] of Object.entries(properties))
      if (
        Object.hasOwn(value, key) &&
        (schemaObject(child) || typeof child === "boolean")
      )
        errors.push(
          ...validateSemanticValue(child, value[key], root, depth + 1).map(
            (error) => `${key}: ${error}`,
          ),
        );
    // Retained unexposed fields are carried through unchanged; only exposed fields are edited here.
  }
  if (Array.isArray(value)) {
    if (typeof schema.minItems === "number" && value.length < schema.minItems)
      errors.push(`At least ${schema.minItems} nodes are required.`);
    if (typeof schema.maxItems === "number" && value.length > schema.maxItems)
      errors.push(`At most ${schema.maxItems} nodes are permitted.`);
    if (
      schema.uniqueItems === true &&
      value.some((v, i) =>
        value.slice(0, i).some((other) => sameSemanticValue(v, other)),
      )
    )
      errors.push("Nodes must be unique.");
    value.forEach((item, index) => {
      const child = semanticItemSchema(schema, index);
      if (schemaObject(child) || typeof child === "boolean")
        errors.push(
          ...validateSemanticValue(child, item, root, depth + 1).map(
            (error) => `Node ${index + 1}: ${error}`,
          ),
        );
    });
  }
  return errors;
}
