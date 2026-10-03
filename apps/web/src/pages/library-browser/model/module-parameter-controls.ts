import { isRecord } from "../api/guards.js";

export type ModuleParameterControl = Readonly<{
  path: ReadonlyArray<string | number>;
  label: string;
  value: unknown;
  fixed: boolean;
  kind: "number" | "boolean" | "enum" | "fixed";
  choices?: ReadonlyArray<string | number>;
  minimum?: number;
  maximum?: number;
  step?: number;
  defaultValue?: unknown;
}>;

// Only the module's admitted scalar schema becomes editable. Unknown saved
// intent stays visible without granting an arbitrary engine control.
export function moduleParameterControls(
  schema: unknown,
  tree: unknown,
): ReadonlyArray<ModuleParameterControl> {
  const controls: ModuleParameterControl[] = [];
  const requiresQualification = isRecord(schema) && "x-qualification" in schema;
  const visit = (
    definition: unknown,
    value: unknown,
    path: ReadonlyArray<string | number>,
    fixed = false,
  ): void => {
    let rule: Record<string, unknown> = isRecord(definition) ? definition : {};
    if (isRecord(value) && Array.isArray(rule["allOf"])) {
      for (const branch of rule["allOf"]) {
        if (
          !isRecord(branch) ||
          !isRecord(branch["if"]) ||
          !isRecord(branch["if"]["properties"]) ||
          !isRecord(branch["then"])
        )
          continue;
        const matches = Object.entries(branch["if"]["properties"]).every(
          ([key, condition]) =>
            isRecord(condition) &&
            "const" in condition &&
            JSON.stringify(condition["const"]) === JSON.stringify(value[key]),
        );
        if (matches) {
          const base = isRecord(rule["properties"]) ? rule["properties"] : {};
          const conditional = isRecord(branch["then"]["properties"])
            ? branch["then"]["properties"]
            : {};
          rule = {
            ...rule,
            ...branch["then"],
            properties: { ...base, ...conditional },
          };
        }
      }
    }
    const label =
      typeof rule["title"] === "string"
        ? rule["title"]
        : rule["x-qualification"] === "editable-manual-exposure"
          ? "Exposure (EV)"
          : path.map(String).join(" / ");
    const qualification = rule["x-qualification"];
    const admitted =
      typeof qualification === "string" &&
      qualification.startsWith("editable-");
    if (
      fixed ||
      "const" in rule ||
      qualification === "fixed-qualified-default" ||
      (requiresQualification &&
        !isRecord(value) &&
        !Array.isArray(value) &&
        !admitted)
    ) {
      controls.push({ path, label, value, fixed: true, kind: "fixed" });
      return;
    }
    if (isRecord(value)) {
      const properties = isRecord(rule["properties"]) ? rule["properties"] : {};
      for (const [key, child] of Object.entries(value))
        visit(properties[key], child, [...path, key]);
      return;
    }
    if (Array.isArray(value)) {
      const tuples = Array.isArray(rule["prefixItems"])
        ? rule["prefixItems"]
        : [];
      value.forEach((child, index) => {
        let item: unknown = tuples[index] ?? rule["items"];
        if (isRecord(item) && Array.isArray(item["oneOf"])) {
          item = item["oneOf"].find((candidate) => {
            if (
              !isRecord(candidate) ||
              !isRecord(candidate["properties"]) ||
              !isRecord(child)
            )
              return false;
            return Object.entries(candidate["properties"]).every(
              ([key, field]) =>
                !isRecord(field) ||
                !("const" in field) ||
                JSON.stringify(field["const"]) === JSON.stringify(child[key]),
            );
          });
        }
        visit(item, child, [...path, index]);
      });
      if (value.length === 0)
        controls.push({ path, label, value, fixed: true, kind: "fixed" });
      return;
    }
    const choices =
      Array.isArray(rule["enum"]) &&
      rule["enum"].every(
        (choice) => typeof choice === "string" || typeof choice === "number",
      )
        ? rule["enum"]
        : undefined;
    if (choices && choices.some((choice) => choice === value)) {
      controls.push({
        path,
        label,
        value,
        fixed: false,
        kind: "enum",
        choices,
      });
    } else if (
      (rule["type"] === "number" || rule["type"] === "integer") &&
      typeof value === "number" &&
      Number.isFinite(value)
    ) {
      controls.push({
        path,
        label,
        value,
        fixed: false,
        kind: "number",
        ...(typeof rule["minimum"] === "number"
          ? { minimum: rule["minimum"] }
          : {}),
        ...(typeof rule["maximum"] === "number"
          ? { maximum: rule["maximum"] }
          : {}),
        step:
          typeof rule["multipleOf"] === "number"
            ? rule["multipleOf"]
            : rule["type"] === "integer"
              ? 1
              : 0.01,
        ...("default" in rule ? { defaultValue: rule["default"] } : {}),
      });
    } else if (rule["type"] === "boolean" && typeof value === "boolean") {
      controls.push({ path, label, value, fixed: false, kind: "boolean" });
    } else controls.push({ path, label, value, fixed: true, kind: "fixed" });
  };
  visit(schema, tree, []);
  return controls;
}

export function replaceModuleParameter(
  tree: unknown,
  path: ReadonlyArray<string | number>,
  value: unknown,
): unknown {
  if (path.length === 0) return value;
  const [key, ...rest] = path;
  if (Array.isArray(tree) && typeof key === "number")
    return tree.map((child: unknown, index) =>
      index === key ? replaceModuleParameter(child, rest, value) : child,
    );
  if (isRecord(tree) && typeof key === "string" && key in tree)
    return { ...tree, [key]: replaceModuleParameter(tree[key], rest, value) };
  return tree;
}
