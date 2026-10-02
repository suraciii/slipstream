import { expect, type Page } from "@playwright/test";
import { createHash } from "node:crypto";
import {
  processingBrowserTest as test,
  processingRequestIdFixture,
  sourceRevision,
  moduleName,
  outputContract,
  mockEditor,
  artifact,
  work,
  parameters,
  json,
  routePhoto,
  submit,
  exportRow,
  artifactRow,
  openFirst,
  openEdit,
  navigate,
  setExposure,
} from "./browser-test-support/processing-fixtures.js";

for (const lostResponse of [
  "connection",
  "server-unknown",
  "server-error",
] as const) {
  test(`an uncertain Export (${lostResponse}) replays its exact captured request after navigating`, async ({
    page,
    running,
  }) => {
    const state = await mockEditor(page, running);
    const first = state.photos[0]!;
    const second = state.photos[1]!;
    const requests = new Map<string, string[]>();
    await page.route("**/api/photos/*/processing-exports", async (route) => {
      if (route.request().method() === "GET") return route.fallback();
      const photoId = routePhoto(route);
      const body = route.request().postData()!;
      const captured = requests.get(photoId) ?? [];
      captured.push(body);
      requests.set(photoId, captured);
      const request = JSON.parse(body) as {
        requestId: string;
        stepId: string;
        expectedRecipeRevision: string;
        expectedSourceRevision: string;
      };
      if (photoId === first && captured.length === 1) {
        if (lostResponse === "connection")
          return route.abort("connectionreset");
        return json(
          route,
          {
            error: {
              code:
                lostResponse === "server-unknown"
                  ? "outcome_unknown"
                  : "internal_error",
            },
          },
          500,
        );
      }
      if (photoId === first) {
        const retained = artifact(photoId, Buffer.from("published result"));
        state.artifacts.set(photoId, [retained]);
        return json(route, { artifact: retained, replayed: true }, 201);
      }
      const receipt = work(photoId, request.requestId, "accepted", {
        recipeRevision: request.expectedRecipeRevision,
        sourceRevision: request.expectedSourceRevision,
        stepId: request.stepId,
        parameters: state.recipes.get(photoId)!.steps[0]!.parameters,
      });
      state.exports.set(photoId, [receipt]);
      return json(route, { outcome: "accepted", receipt }, 202);
    });
    await page.setViewportSize({ width: 390, height: 844 });
    await openFirst(page, running.url);
    await expect(submit(page)).toBeEnabled();
    await submit(page).click();
    await expect(
      page.locator("[data-photo-editor-export-retry]"),
    ).toBeVisible();
    await expect(submit(page)).toBeDisabled();
    await navigate(page, "Next");
    await expect(
      page.getByLabel("Exposure (EV)", { exact: true }),
    ).toBeEnabled();
    await setExposure(page, "0.5");
    await expect
      .poll(() => state.recipes.get(second)?.steps[0]?.parameters)
      .toEqual(parameters(0.5));
    await expect(submit(page)).toBeEnabled();
    await submit(page).click();
    await expect(
      exportRow(
        page,
        processingRequestIdFixture(requests.get(second)![0]!),
      ).getByRole("button", { name: "Cancel", exact: true }),
    ).toBeVisible();
    await navigate(page, "Previous");
    await expect(submit(page)).toBeDisabled();
    await page.locator("[data-photo-editor-export-retry]").click();
    await expect(
      artifactRow(page, "a".repeat(64)).getByRole("button", {
        name: "Download",
        exact: true,
      }),
    ).toBeEnabled();
    await expect(submit(page)).toBeEnabled();
    expect(requests.get(first)).toHaveLength(2);
    expect(requests.get(first)![1]).toBe(requests.get(first)![0]);
    expect(JSON.parse(requests.get(first)![0]!)).toMatchObject({
      stepId: "step-1",
      expectedRecipeRevision: "recipe-1",
      expectedSourceRevision: sourceRevision,
    });
    expect(processingRequestIdFixture(requests.get(second)![0]!)).not.toBe(
      processingRequestIdFixture(requests.get(first)![0]!),
    );
  });
}

test("a refused recipe save blocks export until the saved recipe is chosen", async ({
  page,
  running,
}) => {
  const state = await mockEditor(page, running, {
    save: async (route) => {
      await json(
        route,
        {
          error: {
            code: "recipe_conflict",
            message: "The saved edit changed.",
          },
        },
        409,
      );
    },
  });
  await openFirst(page, running.url);
  await setExposure(page, "0.5");
  await expect(page.locator("[data-photo-editor-conflict]")).toBeVisible();
  await expect(submit(page)).toBeDisabled();
  await page.locator("[data-photo-editor-use-saved]").click();
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toHaveValue(
    "0",
  );
  await expect(submit(page)).toBeEnabled();
  expect(state.saves).toHaveLength(1);
});

for (const response of ["connection", "unreadable"] as const) {
  test(`an uncertain recipe save (${response}) blocks export and reconciles its exact body`, async ({
    page,
    running,
  }) => {
    const state = await mockEditor(page, running, {
      save: async (route, fixture, request) => {
        if (fixture.saves.length === 1) {
          if (response === "connection") return route.abort("failed");
          return route.fulfill({
            status: 200,
            contentType: "application/json",
            body: "{",
          });
        }
        const photoId = routePhoto(route);
        const saved = {
          ...fixture.recipes.get(photoId)!,
          revision: "recipe-2",
          currentStepId: request.currentStepId,
          steps: request.steps,
        };
        fixture.recipes.set(photoId, saved);
        await json(route, {
          outcome: "replayed",
          sourceRevision,
          recipeVersion: saved.revision,
          recipe: saved,
        });
      },
    });
    await openFirst(page, running.url);
    await setExposure(page, "0.5");
    await expect.poll(() => state.saves.length).toBe(1);
    await expect(submit(page)).toBeDisabled();
    await expect(
      page.getByLabel("Exposure (EV)", { exact: true }),
    ).toBeDisabled();
    await page.locator("[data-photo-editor-refresh]").click();
    await expect(submit(page)).toBeEnabled();
    await expect(page.getByLabel("Exposure (EV)", { exact: true })).toHaveValue(
      "0.5",
    );
    expect(state.saves).toHaveLength(2);
    expect(state.saves[1]).toEqual(state.saves[0]);
  });
}

test("an unavailable module preserves editable intent but refuses new export", async ({
  page,
  running,
}) => {
  const state = await mockEditor(page, running, { available: false });
  await openFirst(page, running.url);
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toBeEnabled();
  await setExposure(page, "0.5");
  await expect
    .poll(() => state.recipes.get(state.photos[0]!)?.steps[0]?.parameters)
    .toEqual(parameters(0.5));
  await expect(submit(page)).toBeDisabled();
});

test("a recipe bound to an earlier Original retains settings and blocks export", async ({
  page,
  running,
}) => {
  await mockEditor(page, running, {
    read: (photoId, saved) => ({
      photoId,
      sourceRevision,
      currentSourceRevision: "replacement-source",
      sourceAvailable: true,
      recipe: saved,
    }),
  });
  await openFirst(page, running.url);
  await expect(page.locator("[data-photo-editor-conflict]")).toBeVisible();
  await expect(page.getByLabel("Exposure (EV)", { exact: true })).toHaveValue(
    "0",
  );
  await expect(submit(page)).toBeDisabled();
  await expect(page.locator("[data-photo-editor-rebind]")).toBeVisible();
});

test("an explicitly chosen retained artifact becomes the new selected step's export input", async ({
  page,
  running,
}) => {
  const state = await mockEditor(page, running);
  const photoId = state.photos[0]!;
  const retained = artifact(photoId, Buffer.from("retained input"));
  state.artifacts.set(photoId, [retained]);
  let submitted:
    | {
        requestId: string;
        stepId: string;
        expectedRecipeRevision: string;
        expectedSourceRevision: string;
      }
    | undefined;
  await page.route("**/api/photos/*/processing-exports", (route) => {
    if (route.request().method() === "GET") return route.fallback();
    submitted = JSON.parse(route.request().postData()!) as typeof submitted;
    const saved = state.recipes.get(photoId)!;
    const step = saved.steps.find((item) => item.stepId === submitted!.stepId)!;
    const receipt = work(photoId, submitted!.requestId, "accepted", {
      stepId: step.stepId,
      recipeRevision: saved.revision,
      input: step.input,
      parameters: step.parameters,
    });
    state.exports.set(photoId, [receipt]);
    return json(route, { outcome: "accepted", receipt }, 202);
  });
  await openFirst(page, running.url);
  await page
    .locator("[data-photo-editor-new-step-input]")
    .selectOption(`artifact:${retained.artifactId}`);
  await page.locator("[data-photo-editor-composable-add]").click();
  await expect.poll(() => state.recipes.get(photoId)?.steps.length).toBe(2);
  const saved = state.recipes.get(photoId)!;
  const selected = saved.steps.find(
    (step) => step.stepId === saved.currentStepId,
  )!;
  expect(selected.input).toEqual({
    kind: "artifact",
    artifactId: retained.artifactId,
    contract: outputContract,
  });
  expect(selected.stepId).not.toBe("step-1");
  await expect(submit(page)).toBeEnabled();
  await submit(page).click();
  await expect.poll(() => submitted?.stepId).toBe(selected.stepId);
  expect(submitted).toMatchObject({
    expectedRecipeRevision: saved.revision,
    expectedSourceRevision: sourceRevision,
  });
  await expect(
    exportRow(page, submitted!.requestId).getByRole("button", {
      name: "Cancel",
      exact: true,
    }),
  ).toBeEnabled();
});

async function png(page: Page): Promise<Buffer> {
  return Buffer.from(
    await page.evaluate(async () => {
      const canvas = document.createElement("canvas");
      canvas.width = 16;
      canvas.height = 12;
      const context = canvas.getContext("2d")!;
      context.fillStyle = "#607080";
      context.fillRect(0, 0, 16, 12);
      const blob = await new Promise<Blob>((resolve) =>
        canvas.toBlob((value) => resolve(value!), "image/png"),
      );
      return Array.from(new Uint8Array(await blob.arrayBuffer()));
    }),
  );
}

function previewHeaders(
  photoId: string,
  bytes: Buffer,
): Record<string, string> {
  const digest = createHash("sha256").update(bytes).digest("hex");
  return Object.fromEntries(
    Object.entries({
      "photo-id": photoId,
      "step-id": "step-1",
      "source-revision": Buffer.from(sourceRevision).toString("hex"),
      "recipe-revision": "recipe-1",
      sha256: digest,
      width: "16",
      height: "12",
      geometry: "16",
      "bundle-id": "fixture-bundle",
      module: moduleName,
      "adapter-schema-version": "v1",
      "parameter-digest": digest,
      "output-contract": digest,
      "display-conversion": "srgb-v1",
      identity: digest,
      comparison: "current",
      "input-sha256": "b".repeat(64),
      "input-byte-length": "100",
    }).map(([key, value]) => [`slipstream-processing-preview-${key}`, value]),
  );
}

test("Original reference ignores a late selected-step preview", async ({
  page,
  running,
}) => {
  const state = await mockEditor(page, running);
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let settle!: () => void;
  const settled = new Promise<void>((resolve) => {
    settle = resolve;
  });
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toContainText(
    "Ready · 2 Photos",
  );
  const bytes = await png(page);
  await page.route("**/api/photos/*/processing-preview/**", async (route) => {
    await held;
    try {
      await route.fulfill({
        status: 200,
        contentType: "image/png",
        headers: previewHeaders(state.photos[0]!, bytes),
        body: bytes,
      });
    } catch {
      // Switching to the Original may abort the selected-step fetch.
    } finally {
      settle();
    }
  });
  const requested = page.waitForRequest((request) =>
    new URL(request.url()).pathname.includes("/processing-preview/"),
  );
  try {
    await page.locator('[data-photo-index="0"]').click();
    await openEdit(page);
    await requested;
    await expect(page.locator("[data-stage] img")).toBeVisible();
    const originalSrc = await page
      .locator("[data-stage] img")
      .getAttribute("src");
    expect(originalSrc).toBeTruthy();
    await page.locator("[data-photo-editor-camera-reference]").click();
    await expect(
      page.locator("[data-photo-editor-camera-reference]"),
    ).toHaveAttribute("aria-pressed", "true");
    release();
    await settled;
    await expect(
      page.locator("[data-photo-editor-preview-image]"),
    ).toBeHidden();
    await expect(page.locator("[data-stage] img")).toHaveAttribute(
      "src",
      originalSrc!,
    );
  } finally {
    release();
  }
});

test("an admitted selected-step preview can finish after fifteen seconds of polling", async ({
  page,
  running,
}) => {
  await page.clock.install();
  const state = await mockEditor(page, running);
  await page.goto(running.url);
  await expect(page.locator("[data-grid-status]")).toContainText(
    "Ready · 2 Photos",
  );
  const bytes = await png(page);
  let requests = 0;
  await page.route("**/api/photos/*/processing-preview/**", (route) => {
    requests++;
    if (requests <= 21) return json(route, { state: "running" }, 202);
    return route.fulfill({
      status: 200,
      contentType: "image/png",
      headers: previewHeaders(state.photos[0]!, bytes),
      body: bytes,
    });
  });
  await page.locator('[data-photo-index="0"]').click();
  await openEdit(page);
  await expect.poll(() => requests).toBe(1);
  for (let count = 1; count <= 21; count++) {
    await page.clock.runFor(751);
    await expect.poll(() => requests).toBeGreaterThan(count);
  }
  await expect(page.locator("[data-photo-editor-preview-image]")).toBeVisible();
  await expect(
    page.locator("[data-photo-editor-preview-image]"),
  ).toHaveAttribute("src", /^blob:/);
});

test("numeric controls reject incomplete edits and synchronize Reset while focused", async ({
  page,
  running,
}) => {
  const state = await mockEditor(page, running);
  const photoId = state.photos[0]!;
  await openFirst(page, running.url);
  const exposure = page.getByLabel("Exposure (EV)", { exact: true });
  await setExposure(page, "1");
  await expect
    .poll(() => state.recipes.get(photoId)?.steps[0]?.parameters)
    .toEqual(parameters(1));
  await expect(exposure).toHaveValue("1");
  const savedCount = state.saves.length;
  await exposure.fill("");
  await exposure.press("Tab");
  expect(
    await exposure.evaluate(
      (input: HTMLInputElement) => input.validity.valueMissing,
    ),
  ).toBe(true);
  await expect(exposure).toHaveValue("");
  expect(state.saves.length).toBe(savedCount);
  expect(state.recipes.get(photoId)?.steps[0]?.parameters).toEqual(
    parameters(1),
  );
  const reset = exposure
    .locator("..")
    .getByRole("button", { name: "Reset", exact: true });
  await reset.click();
  await expect(exposure).toHaveValue("0");
  await expect(reset).toBeFocused();
  await expect
    .poll(() => state.recipes.get(photoId)?.steps[0]?.parameters)
    .toEqual(parameters(0));
});
