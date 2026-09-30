import { afterEach, describe, expect, test } from "bun:test";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkFsdArchitecture } from "./check-fsd-architecture.js";

const roots: string[] = [];

afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
  );
});

async function fixture(files: Record<string, string>): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), "slipstream-fsd-"));
  roots.push(root);
  await mkdir(join(root, "apps/web/src"), { recursive: true });
  await writeFile(
    join(root, "tsconfig.json"),
    JSON.stringify({
      compilerOptions: {
        strict: true,
        module: "ESNext",
        moduleResolution: "Bundler",
        target: "ES2022",
      },
    }),
  );
  await writeFile(
    join(root, "apps/web/tsconfig.json"),
    JSON.stringify({
      extends: "../../tsconfig.json",
      include: ["src/**/*.ts"],
    }),
  );
  await Promise.all(
    Object.entries(files).map(async ([name, contents]) => {
      const path = join(root, "apps/web/src", name);
      await mkdir(join(path, ".."), { recursive: true });
      await writeFile(path, contents);
    }),
  );
  return root;
}

describe("FSD architecture gate", () => {
  test("rejects same-layer imports with a path and line", async () => {
    const root = await fixture({
      "app/index.ts": 'import { mount } from "../pages/other/page"; mount();',
      "pages/other/page.ts":
        'import { value } from "../library/model/value"; export const mount = () => value;',
      "pages/library/model/value.ts": "export const value = 1;",
    });
    const diagnostics = checkFsdArchitecture(root);
    expect(
      diagnostics.some(
        (diagnostic) =>
          diagnostic.message.includes("same-layer slices") &&
          diagnostic.file.endsWith("pages/other/page.ts") &&
          diagnostic.line === 1,
      ),
    ).toBe(true);
  });

  test("accepts a cycle made entirely from type-only edges", async () => {
    const root = await fixture({
      "app/index.ts":
        'import { mount } from "../pages/library-browser/index"; mount();',
      "pages/library-browser/index.ts": 'export { mount } from "./page";',
      "pages/library-browser/page.ts":
        'import type { Other } from "./model/a"; export const mount = () => undefined as Other;',
      "pages/library-browser/model/a.ts":
        'import type { Page } from "../page"; export type Other = Page;',
    });
    expect(checkFsdArchitecture(root)).toEqual([]);
  });

  test("rejects runtime cycles, including dynamic imports and empty named imports", async () => {
    const root = await fixture({
      "app/index.ts":
        'import { mount } from "../pages/library-browser/index"; mount();',
      "pages/library-browser/index.ts": 'export { mount } from "./page";',
      "pages/library-browser/page.ts":
        'import {} from "./model/a"; export const mount = () => import("./model/b");',
      "pages/library-browser/model/a.ts":
        'import { value } from "./b"; export const a = value;',
      "pages/library-browser/model/b.ts":
        'import { a } from "./a"; export const value = a;',
    });
    expect(
      checkFsdArchitecture(root).some((diagnostic) =>
        diagnostic.message.includes("runtime import cycle"),
      ),
    ).toBe(true);
  });

  test("rejects a self runtime cycle", async () => {
    const root = await fixture({
      "app/index.ts":
        'import { mount } from "../pages/library-browser/index"; mount();',
      "pages/library-browser/index.ts": 'export { mount } from "./page";',
      "pages/library-browser/page.ts":
        'import { value } from "./model/self"; export const mount = () => value;',
      "pages/library-browser/model/self.ts":
        'import { value } from "./self"; export const value = value;',
    });
    expect(
      checkFsdArchitecture(root).some((diagnostic) =>
        diagnostic.message.includes("runtime import cycle"),
      ),
    ).toBe(true);
  });

  test("rejects cycles through dynamic imports and re-exports", async () => {
    const root = await fixture({
      "app/index.ts":
        'import { mount } from "../pages/library-browser/index"; mount();',
      "pages/library-browser/index.ts": 'export { mount } from "./page";',
      "pages/library-browser/page.ts":
        'export const mount = () => import("./model/a");',
      "pages/library-browser/model/a.ts": 'export { mount } from "../page";',
    });
    expect(
      checkFsdArchitecture(root).some((diagnostic) =>
        diagnostic.message.includes("runtime import cycle"),
      ),
    ).toBe(true);
  });

  test("rejects page public API bypasses", async () => {
    const root = await fixture({
      "app/index.ts":
        'import { mount } from "../pages/library-browser/page"; mount();',
      "pages/library-browser/index.ts": 'export { mount } from "./page";',
      "pages/library-browser/page.ts":
        'import { mount as publicMount } from "./index"; export const mount = () => publicMount;',
    });
    const diagnostics = checkFsdArchitecture(root);
    expect(
      diagnostics.some(
        (diagnostic) =>
          diagnostic.file.endsWith("app/index.ts") &&
          diagnostic.message.includes("public index.ts API"),
      ),
    ).toBe(true);
    expect(
      diagnostics.some(
        (diagnostic) =>
          diagnostic.file.endsWith("page.ts") &&
          diagnostic.message.includes("own public index.ts"),
      ),
    ).toBe(true);
  });

  test("rejects upward imports and invalid page segments", async () => {
    const root = await fixture({
      "app/index.ts": "export const value = 1;",
      "pages/library-browser/page.ts":
        'import { value } from "../../app/index"; export const mount = () => value;',
      "pages/library-browser/legacy/value.ts": "export const value = 2;",
    });
    const diagnostics = checkFsdArchitecture(root);
    expect(
      diagnostics.some(
        (diagnostic) =>
          diagnostic.file.endsWith("page.ts") &&
          diagnostic.message.includes("pages must not import from app"),
      ),
    ).toBe(true);
    expect(
      diagnostics.some(
        (diagnostic) =>
          diagnostic.file.endsWith("legacy/value.ts") &&
          diagnostic.message.includes("invalid page segment"),
      ),
    ).toBe(true);
  });

  test("rejects unresolved internal edges", async () => {
    const root = await fixture({
      "app/index.ts":
        'import { mount } from "../pages/library-browser/index"; mount();',
      "pages/library-browser/index.ts": 'export { mount } from "./page";',
      "pages/library-browser/page.ts":
        'import { missing } from "./model/missing"; export const mount = () => missing;',
    });
    const diagnostics = checkFsdArchitecture(root);
    expect(
      diagnostics.some(
        (diagnostic) =>
          diagnostic.message.includes("unresolved internal import") &&
          diagnostic.file.endsWith("page.ts"),
      ),
    ).toBe(true);
  });
});
