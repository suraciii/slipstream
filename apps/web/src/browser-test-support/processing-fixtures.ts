import {
  test as browserTest,
  expect,
  type Page,
  type Route,
} from "@playwright/test";
import { createHash } from "node:crypto";
import { copyFile, mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  fixtureFetch,
  startBrowserServer,
  type BrowserServer,
} from "../browser-server.js";

// Shared processing wire records, browser setup, and editor routing fixtures.
export type ProcessingParametersFixture = {
  schemaVersion: string;
  tree: unknown;
};
export type ProcessingInputFixture =
  | { kind: "original"; photoId: string; sourceRevision: string }
  | { kind: "artifact"; artifactId: string; contract: Record<string, unknown> };
export type ProcessingStepFixture = {
  stepId: string;
  module: string;
  input: ProcessingInputFixture;
  parameters: ProcessingParametersFixture;
};
export type ProcessingRecipeFixture = {
  photoId: string;
  revision: string;
  sourceRevision: string;
  currentStepId: string | null;
  steps: ProcessingStepFixture[];
};
export type ProcessingExportFixture = ProcessingStepFixture & {
  photoId: string;
  requestId: string;
  recipeRevision: string;
  sourceRevision: string;
  state: "accepted" | "executing" | "succeeded" | "failed" | "cancelled";
  artifactId: string | null;
  failureReason: string | null;
  acceptedAt: number;
  terminalAt: number | null;
  retainUntil: number;
  bundleId: string;
};
export type ProcessingArtifactFixture = Omit<ProcessingStepFixture, "input"> & {
  artifactId: string;
  photoId: string;
  adapterSchemaVersion: string;
  input: {
    binding: ProcessingInputFixture;
    sha256: string;
    byteLength: number;
  };
  outputContract: {
    format: string;
    precision: string;
    colorSpace: string;
    transfer: string;
    geometry: { width: number; height: number };
    encoding: string;
  };
  bundleId: string;
  sha256: string;
  byteLength: number;
  filename: string;
  publishedAt: string;
  expiresAt: string;
  orientation: string;
  iccEmbedded: boolean;
  sampleFormat: string;
};
export type HistoricalExportFixture = {
  exportId: string;
  target: string;
  state: "succeeded";
  failureReason: string;
  createdAt: string;
  artifact: {
    exportId: string;
    target: string;
    stage: string;
    contentType: string;
    width: number;
    height: number;
    profileIdentity: string;
    byteLength: number;
    sha256: string;
    expiresAt: string;
    filename: string;
    orientation: string;
    sampleFormat: string;
    colorSpace: string;
    iccEmbedded: boolean;
  };
};

export function processingModuleDefaultsFixture(value: unknown, name: string) {
  if (
    typeof value !== "object" ||
    value === null ||
    !("contractVersion" in value) ||
    typeof value.contractVersion !== "string" ||
    !("modules" in value) ||
    !Array.isArray(value.modules)
  )
    throw new Error("The processing fixture requires a module listing");
  const modules: unknown[] = value.modules;
  const names = modules.map((entry) => {
    if (
      typeof entry !== "object" ||
      entry === null ||
      !("id" in entry) ||
      typeof entry.id !== "object" ||
      entry.id === null ||
      !("name" in entry.id) ||
      typeof entry.id.name !== "string"
    )
      throw new Error("The processing fixture requires module names");
    return entry.id.name;
  });
  const module = modules[names.indexOf(name)];
  if (
    typeof module !== "object" ||
    module === null ||
    !("availability" in module) ||
    typeof module.availability !== "object" ||
    module.availability === null ||
    !("state" in module.availability) ||
    module.availability.state !== "ready" ||
    !("parameterVersions" in module) ||
    !Array.isArray(module.parameterVersions) ||
    typeof module.parameterVersions[0] !== "string" ||
    !module.parameterVersions[0] ||
    !("parameterSchema" in module) ||
    typeof module.parameterSchema !== "object" ||
    module.parameterSchema === null ||
    !("type" in module.parameterSchema) ||
    module.parameterSchema.type !== "object" ||
    !("default" in module.parameterSchema) ||
    typeof module.parameterSchema.default !== "object" ||
    module.parameterSchema.default === null ||
    Array.isArray(module.parameterSchema.default)
  )
    throw new Error(
      `The real-processing fixture requires a ready ${name} module with published defaults`,
    );
  return {
    names,
    schemaVersion: module.parameterVersions[0],
    defaultTree: module.parameterSchema.default,
  };
}

export function processingRequestIdFixture(body: string): string {
  const value: unknown = JSON.parse(body);
  if (
    typeof value !== "object" ||
    value === null ||
    !("requestId" in value) ||
    typeof value.requestId !== "string"
  )
    throw new Error("The processing fixture requires a requestId");
  return value.requestId;
}

export const processingBrowserTest = browserTest.extend<{
  running: BrowserServer;
}>({
  running: async ({ context }, use) => {
    const base = await mkdtemp(join(tmpdir(), "slipstream-export-browser-"));
    const root = join(base, "originals");
    let running: BrowserServer | undefined;
    try {
      await mkdir(root);
      for (const name of ["001.jpg", "002.jpg"])
        await copyFile("apps/web/test-fixtures/review.jpg", join(root, name));
      running = await startBrowserServer({ base, root });
      const login = await context.request.post(
        `${running.url}/api/access/session`,
        {
          headers: { Origin: running.url },
          data: { token: running.token },
        },
      );
      expect(login.status()).toBe(204);
      const server = running;
      await expect
        .poll(
          async () =>
            (
              (await (
                await fixtureFetch(`${server.url}/api/status`)
              ).json()) as {
                state: string;
              }
            ).state,
        )
        .toBe("idle");
      await use(running);
    } finally {
      await running?.close();
      await rm(base, { recursive: true, force: true });
    }
  },
});

export const sourceRevision = "opaque\u0000source-1";
export const moduleName = "photo";
const parameterTree = (exposure = 0) => ({
  stack: [{ op: "exposure", params: { exposure } }],
});
export const parameters = (exposure = 0) => ({
  schemaVersion: "v1",
  tree: parameterTree(exposure),
});

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function recipeExposure(recipe: ProcessingRecipeFixture): number {
  const step = recipe.steps.find(
    (candidate) => candidate.stepId === recipe.currentStepId,
  );
  const tree = step?.parameters.tree;
  const stackValue: unknown = isRecord(tree) ? tree["stack"] : undefined;
  if (!Array.isArray(stackValue)) return 0;
  const stack: unknown[] = stackValue;
  const entry = stack.find(
    (candidate) => isRecord(candidate) && candidate["op"] === "exposure",
  );
  const params = isRecord(entry) ? entry["params"] : undefined;
  const exposure = isRecord(params) ? params["exposure"] : undefined;
  return typeof exposure === "number" && Number.isFinite(exposure)
    ? exposure
    : 0;
}
export const outputContract = {
  format: "jpeg",
  precision: "uint8",
  colorSpace: "srgb",
  transfer: "srgb",
  geometry: { width: 16, height: 12 },
  encoding: "jpeg",
};
const moduleOutputContract = {
  format: outputContract.format,
  colorSpace: outputContract.colorSpace,
  transferFunction: outputContract.transfer,
  precisionBits: 8,
  width: outputContract.geometry.width,
  height: outputContract.geometry.height,
};

function recipe(photoId: string): ProcessingRecipeFixture {
  return {
    photoId,
    revision: "recipe-1",
    sourceRevision,
    currentStepId: "step-1",
    steps: [
      {
        stepId: "step-1",
        module: moduleName,
        input: { kind: "original", photoId, sourceRevision },
        parameters: parameters(),
      },
    ],
  };
}

export function work(
  photoId: string,
  requestId: string,
  state: ProcessingExportFixture["state"],
  overrides: Partial<ProcessingExportFixture> = {},
): ProcessingExportFixture {
  return {
    photoId,
    requestId,
    stepId: "step-1",
    module: moduleName,
    recipeRevision: "recipe-1",
    sourceRevision,
    state,
    artifactId: null,
    failureReason:
      state === "failed"
        ? "The processing allowance is insufficient for this output."
        : null,
    acceptedAt: 1,
    terminalAt: state === "accepted" || state === "executing" ? null : 2,
    retainUntil: 4_070_908_800_000,
    parameters: parameters(),
    input: { kind: "original", photoId, sourceRevision },
    bundleId: "fixture-bundle",
    ...overrides,
  };
}

export function artifact(
  photoId: string,
  bytes: Buffer,
  artifactId = "a".repeat(64),
): ProcessingArtifactFixture {
  return {
    artifactId,
    photoId,
    stepId: "step-1",
    module: moduleName,
    adapterSchemaVersion: "v1",
    parameters: parameters(),
    input: {
      binding: { kind: "original", photoId, sourceRevision },
      sha256: "b".repeat(64),
      byteLength: 100,
    },
    outputContract,
    bundleId: "fixture-bundle",
    sha256: createHash("sha256").update(bytes).digest("hex"),
    byteLength: bytes.length,
    filename: "retained-result.jpg",
    publishedAt: "2026-10-01T00:00:00Z",
    expiresAt: "2099-01-01T00:00:00Z",
    orientation: "top-left",
    iccEmbedded: true,
    sampleFormat: "uint8",
  };
}

export function artifactHeaders(
  record: ProcessingArtifactFixture,
): Record<string, string> {
  return {
    "slipstream-artifact-id": record.artifactId,
    "slipstream-artifact-photo-id": record.photoId,
    "slipstream-artifact-step-id": record.stepId,
    "slipstream-artifact-module": record.module,
    "slipstream-artifact-adapter-schema-version": record.adapterSchemaVersion,
    "slipstream-artifact-bundle-id": record.bundleId,
    "slipstream-artifact-width": String(record.outputContract.geometry.width),
    "slipstream-artifact-height": String(record.outputContract.geometry.height),
    "slipstream-artifact-byte-length": String(record.byteLength),
    "slipstream-artifact-sha256": record.sha256,
    "slipstream-artifact-filename": record.filename,
    "slipstream-artifact-published-at": record.publishedAt,
    "slipstream-artifact-expires-at": record.expiresAt,
    "slipstream-artifact-orientation": record.orientation,
    "slipstream-artifact-icc-embedded": String(record.iccEmbedded),
    "slipstream-artifact-sample-format": record.sampleFormat,
  };
}

export const json = (route: Route, body: unknown, status = 200) =>
  route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
export const routePhoto = (route: Route) =>
  new URL(route.request().url()).pathname.split("/")[3]!;
export const submit = (page: Page) =>
  page.locator("[data-photo-editor-export-submit]");
export const exportRow = (page: Page, requestId: string) =>
  page.locator(
    `[data-photo-editor-export-list] [data-request-id="${requestId}"]`,
  );
export const artifactRow = (page: Page, artifactId: string) =>
  page.locator(
    `[data-photo-editor-composable-artifact-list] [data-artifact-id="${artifactId}"]`,
  );

type SaveRequest = {
  requestId: string;
  expectedRecipeRevision: string | null;
  expectedSourceRevision: string;
  currentStepId: string | null;
  steps: ProcessingStepFixture[];
};
type Fixture = {
  photos: string[];
  recipes: Map<string, ProcessingRecipeFixture>;
  exports: Map<string, ProcessingExportFixture[]>;
  artifacts: Map<string, ProcessingArtifactFixture[]>;
  historicalExports: Map<string, HistoricalExportFixture[]>;
  saves: Array<{ photoId: string; body: string }>;
};

export async function mockEditor(
  page: Page,
  running: BrowserServer,
  options: {
    available?: boolean;
    save?: (
      route: Route,
      state: Fixture,
      request: SaveRequest,
    ) => Promise<void>;
    read?: (
      photoId: string,
      saved: ProcessingRecipeFixture,
    ) => Record<string, unknown>;
  } = {},
): Promise<Fixture> {
  const browse = await fixtureFetch(`${running.url}/api/browse`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Origin: running.url },
    body: JSON.stringify({ source: "library" }),
  });
  const opened = (await browse.json()) as { token: string };
  const listing = await fixtureFetch(
    `${running.url}/api/browse/${opened.token}?start=0&limit=60`,
  );
  const { photos } = (await listing.json()) as {
    photos: Array<{ id: string }>;
  };
  const state: Fixture = {
    photos: photos.map((photo) => photo.id),
    recipes: new Map(photos.map((photo) => [photo.id, recipe(photo.id)])),
    exports: new Map(photos.map((photo) => [photo.id, []])),
    artifacts: new Map(photos.map((photo) => [photo.id, []])),
    saves: [],
    historicalExports: new Map(photos.map((photo) => [photo.id, []])),
  };
  expect(state.photos).toHaveLength(2);
  await page.route("**/api/processing/modules", (route) =>
    json(route, {
      contractVersion: "v1",
      modules: [
        {
          id: { name: moduleName, adapterVersion: "v1" },
          parameterVersions: ["v1"],
          parameterSchema: {
            type: "object",
            "x-qualification": "qualified-module",
            default: parameterTree(),
            properties: {
              stack: {
                type: "array",
                items: {
                  type: "object",
                  properties: {
                    op: { const: "exposure" },
                    params: {
                      type: "object",
                      properties: {
                        exposure: {
                          type: "number",
                          minimum: -3,
                          maximum: 3,
                          multipleOf: 0.001,
                          default: 0,
                          "x-qualification": "editable-manual-exposure",
                        },
                      },
                    },
                  },
                },
              },
            },
          },
          admittedInputs: [
            {
              format: "jpeg",
              colorSpace: "camera-native",
              transferFunction: "srgb",
              precisionBits: 8,
              width: 16,
              height: 12,
            },
            moduleOutputContract,
          ],
          admittedOutputs: [moduleOutputContract],
          limits: {},
          availability: {
            state: options.available === false ? "unavailable" : "ready",
            refusalReasons:
              options.available === false
                ? ["Processing bundle unavailable"]
                : [],
          },
        },
      ],
    }),
  );
  await page.route("**/api/photos/*/edit", (route) => {
    const photoId = routePhoto(route);
    const saved = state.recipes.get(photoId)!;
    const step = saved.steps.find(
      (item) => item.stepId === saved.currentStepId,
    );
    return json(route, {
      photoId,
      editRevision: saved.revision,
      sourceRevision: saved.sourceRevision,
      currentSourceRevision: sourceRevision,
      sourceAvailable: true,
      requiresRebind: false,
      canSave: Boolean(step),
      canPreview: Boolean(step) && options.available !== false,
      canExport: Boolean(step) && options.available !== false,
      current: step
        ? {
            engine: step.module,
            input: step.input,
            controls: { exposure: { ev: recipeExposure(saved) } },
          }
        : null,
    });
  });

  const updateExposure = async (route: Route, reset: boolean) => {
    if (route.request().method() !== "POST") return route.fallback();
    const photoId = routePhoto(route);
    const saved = state.recipes.get(photoId)!;
    const raw: unknown = JSON.parse(route.request().postData() ?? "{}");
    const value = reset ? 0 : isRecord(raw) ? raw["value"] : undefined;
    if (typeof value !== "number" || !Number.isFinite(value)) {
      return json(
        route,
        { error: { code: "invalid_value", message: "Invalid exposure" } },
        422,
      );
    }
    const next = {
      ...saved,
      revision: `${saved.revision}-stateful`,
      steps: saved.steps.map((step) =>
        step.stepId === saved.currentStepId
          ? { ...step, parameters: parameters(value) }
          : step,
      ),
    };
    state.recipes.set(photoId, next);
    await json(route, { outcome: "saved" });
  };
  await page.route("**/api/photos/*/edit/set", (route) =>
    updateExposure(route, false),
  );
  await page.route("**/api/photos/*/edit/reset", (route) =>
    updateExposure(route, true),
  );
  await page.route("**/api/photos/*/processing-recipe", async (route) => {
    const photoId = routePhoto(route);
    const saved = state.recipes.get(photoId)!;
    if (route.request().method() === "POST") {
      const body = route.request().postData()!;
      state.saves.push({ photoId, body });
      const request = JSON.parse(body) as SaveRequest;
      if (options.save) return options.save(route, state, request);
      const next = {
        ...saved,
        revision: `recipe-${state.saves.length + 1}`,
        currentStepId: request.currentStepId,
        steps: request.steps,
      };
      state.recipes.set(photoId, next);
      await json(route, {
        outcome: "saved",
        sourceRevision,
        recipeVersion: next.revision,
        recipe: next,
      });
      return;
    }
    await json(
      route,
      options.read?.(photoId, saved) ?? {
        photoId,
        sourceRevision,
        currentSourceRevision: sourceRevision,
        sourceAvailable: true,
        recipe: saved,
      },
    );
  });
  await page.route("**/api/photos/*/processing-exports", (route) => {
    const photoId = routePhoto(route);
    if (route.request().method() !== "GET")
      throw new Error("Unexpected export submission");
    return json(route, {
      photoId,
      exports: state.exports.get(photoId),
      artifacts: state.artifacts.get(photoId),
      historicalExports: state.historicalExports.get(photoId),
    });
  });
  await page.route("**/api/photos/*/processing-preview/**", (route) =>
    json(
      route,
      {
        error: { code: "module_parameters_unavailable" },
      },
      503,
    ),
  );
  await page.route("**/api/photos/*/edit/preview", (route) =>
    json(
      route,
      {
        error: { code: "module_parameters_unavailable" },
      },
      503,
    ),
  );
  return state;
}

export async function openEdit(page: Page) {
  await page.locator("[data-dock-more]").click();
  await page.locator("[data-photo-tools-entry='edit']").click();
  await expect(page.locator("[data-photo-tools-view='edit']")).toBeVisible();
}

/** Opens the advanced compatibility controls for tests that exercise the
 * legacy complete snapshot surface. */
export async function openAdvancedCompatibility(page: Page) {
  const details = page.locator("[data-photo-editor-advanced]");
  await expect(details).toBeVisible();
  if ((await details.getAttribute("open")) === null)
    await details.locator(":scope > summary").click();
  await expect(details).toHaveAttribute("open", "");
}

export async function openFirst(page: Page, url: string) {
  await page.goto(url);
  await expect(page.locator("[data-grid-status]")).toContainText(
    "Ready · 2 Photos",
  );
  await page.locator('[data-photo-index="0"]').click();
  await openEdit(page);
  await openAdvancedCompatibility(page);
  await expect(page.locator("[data-photo-editor-exposure]")).toBeVisible();
}

export async function navigate(page: Page, direction: "Next" | "Previous") {
  await page.locator("[data-photo-tools-close]").click();
  await page.locator("[data-dock-more]").click();
  await page
    .locator(`[data-photo-tools-entry='${direction.toLowerCase()}']`)
    .click();
  await openEdit(page);
}

export async function setExposure(page: Page, value: string) {
  const input = page.locator("[data-photo-editor-exposure]");
  await expect(input).toBeVisible();
  await input.fill(value);
  await input.press("Tab");
}

export async function setAdvancedExposure(page: Page, value: string) {
  await openAdvancedCompatibility(page);
  const input = page
    .locator(
      "[data-photo-editor-composable-editor] [data-photo-editor-module-controls]",
    )
    .getByLabel("Exposure (EV)", { exact: true });
  await expect(input).toBeVisible();
  await input.fill(value);
  await input.press("Tab");
}
