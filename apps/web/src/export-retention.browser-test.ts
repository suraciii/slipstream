import { expect } from "@playwright/test";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import {
  processingBrowserTest as test,
  mockEditor,
  artifact,
  artifactHeaders,
  work,
  parameters,
  json,
  submit,
  exportRow,
  artifactRow,
  openFirst,
  openEdit,
  navigate,
  setExposure,
  openAdvancedCompatibility,
  type ProcessingExportFixture as ProcessingExportWork,
} from "./browser-test-support/processing-fixtures.js";

test("retained tasks and artifacts restore after a browser reload and Photo navigation", async ({
  page,
  running,
}) => {
  const state = await mockEditor(page, running);
  const photoId = state.photos[0]!;
  const bytes = Buffer.from("retained immutable result");
  const retained = artifact(photoId, bytes);
  const expired = {
    ...artifact(photoId, bytes, "c".repeat(64)),
    filename: "expired-result.jpg",
    expiresAt: "2020-01-01T00:00:00Z",
  };
  state.exports.set(photoId, [
    work(photoId, "failed-request", "failed"),
    work(photoId, "cancelled-request", "cancelled"),
    work(photoId, "succeeded-request", "succeeded", {
      artifactId: retained.artifactId,
    }),
  ]);
  state.artifacts.set(photoId, [retained, expired]);
  await openFirst(page, running.url);
  await openAdvancedCompatibility(page);
  await expect(exportRow(page, "failed-request")).toContainText(
    "The processing allowance is insufficient for this output.",
  );
  await expect(
    exportRow(page, "failed-request").getByRole("button", {
      name: "Retry captured settings",
    }),
  ).toBeEnabled();
  await expect(
    exportRow(page, "cancelled-request").getByRole("button", {
      name: "Retry captured settings",
    }),
  ).toBeEnabled();
  await expect(
    artifactRow(page, retained.artifactId).getByRole("button", {
      name: "Download",
      exact: true,
    }),
  ).toBeEnabled();
  await expect(
    artifactRow(page, expired.artifactId).getByRole("button", {
      name: "Download",
      exact: true,
    }),
  ).toBeDisabled();
  await page.reload();
  await expect(page.locator("[data-review]")).toBeVisible();
  await openEdit(page);
  await openAdvancedCompatibility(page);
  await expect(exportRow(page, "succeeded-request")).toBeVisible();
  await navigate(page, "Next");
  await expect(page.locator("[data-photo-editor-export-list] li")).toHaveCount(
    0,
  );
  await expect(
    page.locator("[data-photo-editor-composable-artifact-list] li"),
  ).toHaveCount(0);
  await navigate(page, "Previous");
  await expect(page.locator("[data-photo-editor-export-list] li")).toHaveCount(
    3,
  );
  await expect(artifactRow(page, retained.artifactId)).toBeVisible();
  await setExposure(page, "0.75");
  await expect
    .poll(() => state.recipes.get(photoId)?.steps[0]?.parameters)
    .toEqual(parameters(0.75));
  await expect(artifactRow(page, retained.artifactId)).toContainText(
    "Based on earlier settings",
  );
  await expect(
    artifactRow(page, retained.artifactId).getByRole("button", {
      name: "Download",
      exact: true,
    }),
  ).toBeEnabled();
  await artifactRow(page, retained.artifactId)
    .getByRole("button", { name: "Use as service input" })
    .click();
  await expect
    .poll(() => state.recipes.get(photoId)?.steps[0]?.input)
    .toMatchObject({ kind: "artifact", artifactId: retained.artifactId });
});

test("retry uses captured settings despite newer edits and cancellation is available on acceptance", async ({
  page,
  running,
}) => {
  const state = await mockEditor(page, running);
  const photoId = state.photos[0]!;
  const captured = work(photoId, "old-request", "failed", {
    stepId: "removed-step",
    parameters: parameters(-1),
  });
  state.exports.set(photoId, [captured]);
  let retryBody: string | undefined;
  let retried: ProcessingExportWork | undefined;
  let cancelRequests = 0;
  await page.route(
    "**/api/photos/*/processing-exports/old-request/retry",
    async (route) => {
      retryBody = route.request().postData()!;
      const { requestId } = JSON.parse(retryBody) as { requestId: string };
      retried = {
        ...captured,
        requestId,
        state: "accepted",
        failureReason: null,
        terminalAt: null,
      };
      state.exports.set(photoId, [retried, captured]);
      await json(route, { outcome: "accepted", receipt: retried }, 202);
    },
  );
  await page.route(
    "**/api/photos/*/processing-exports/*/cancel",
    async (route) => {
      cancelRequests += 1;
      expect(route.request().method()).toBe("POST");
      retried = { ...retried!, state: "cancelled", terminalAt: 3 };
      state.exports.set(photoId, [retried, captured]);
      await json(route, { outcome: "cancelled", receipt: retried });
    },
  );
  await page.route("**/api/photos/*/processing-exports/*", (route) => {
    const requestId = new URL(route.request().url()).pathname.split("/").at(-1);
    return json(
      route,
      state.exports.get(photoId)!.find((item) => item.requestId === requestId),
    );
  });
  await openFirst(page, running.url);
  await setExposure(page, "0.8");
  await expect
    .poll(() => state.recipes.get(photoId)?.steps[0]?.parameters)
    .toEqual(parameters(0.8));
  await exportRow(page, captured.requestId)
    .getByRole("button", { name: "Retry captured settings" })
    .click();
  await expect.poll(() => retried?.requestId).toBeTruthy();
  const row = exportRow(page, retried!.requestId);
  await expect(row).toContainText("removed-step");
  const cancel = row.getByRole("button", { name: "Cancel", exact: true });
  await expect(cancel).toBeEnabled();
  expect(JSON.parse(retryBody!)).toEqual({ requestId: retried!.requestId });
  expect(retried!.requestId).not.toBe(captured.requestId);
  await cancel.click();
  await expect(
    row.getByRole("button", { name: "Retry captured settings" }),
  ).toBeVisible();
  await expect(
    row.getByRole("button", { name: "Cancel", exact: true }),
  ).toHaveCount(0);
  expect(cancelRequests).toBe(1);
});

test("a failed submission receipt retains its reason and captured retry action", async ({
  page,
  running,
}) => {
  const state = await mockEditor(page, running);
  const photoId = state.photos[0]!;
  let requestId = "";
  await page.route("**/api/photos/*/edit/export", (route) => {
    if (route.request().method() === "GET") return route.fallback();
    const request = JSON.parse(route.request().postData()!) as {
      requestId: string;
    };
    requestId = request.requestId;
    const receipt = work(photoId, requestId, "failed");
    state.exports.set(photoId, [receipt]);
    return json(
      route,
      { error: { code: "export_failed", details: { receipt } } },
      409,
    );
  });
  await openFirst(page, running.url);
  await expect(submit(page)).toBeEnabled();
  await submit(page).click();
  await expect.poll(() => requestId).not.toBe("");
  await expect(exportRow(page, requestId)).toContainText(
    "The processing allowance is insufficient for this output.",
  );
  await expect(
    exportRow(page, requestId).getByRole("button", {
      name: "Retry captured settings",
    }),
  ).toBeEnabled();
});

for (const digestPath of ["native", "worker"] as const) {
  test(`a retained artifact validates and downloads using ${digestPath} hashing`, async ({
    page,
    running,
  }) => {
    if (digestPath === "worker")
      await page.addInitScript(() =>
        Object.defineProperty(crypto, "subtle", { value: undefined }),
      );
    const state = await mockEditor(page, running, { available: false });
    const photoId = state.photos[0]!;
    const bytes = Buffer.alloc(1024 * 1024 + 67);
    for (let index = 0; index < bytes.length; index++)
      bytes[index] = index % 251;
    const retained = artifact(photoId, bytes);
    state.artifacts.set(photoId, [retained]);
    state.exports.set(photoId, [
      work(photoId, "retained-request", "succeeded", {
        artifactId: retained.artifactId,
      }),
    ]);
    await page.route(
      `**/api/processing-artifacts/${retained.artifactId}/bytes`,
      (route) =>
        route.fulfill({
          status: 200,
          contentType: "image/jpeg",
          headers: artifactHeaders(retained),
          body: bytes,
        }),
    );
    await openFirst(page, running.url);
    await expect(submit(page)).toBeDisabled();
    const download = page.waitForEvent("download");
    await artifactRow(page, retained.artifactId)
      .getByRole("button", { name: "Download", exact: true })
      .click();
    const file = await download;
    expect(file.suggestedFilename()).toBe(retained.filename);
    expect(await readFile(await file.path())).toEqual(bytes);
  });
}

test("historical retained outputs restore and download while module execution is unavailable", async ({
  page,
  running,
}) => {
  const state = await mockEditor(page, running, { available: false });
  const photoId = state.photos[0]!;
  const bytes = await readFile("apps/web/test-fixtures/review.jpg");
  const retained = {
    exportId: "historical-result",
    target: "film-jpeg",
    stage: "film",
    contentType: "image/jpeg",
    width: 16,
    height: 12,
    profileIdentity: "historical-profile",
    byteLength: bytes.length,
    sha256: createHash("sha256").update(bytes).digest("hex"),
    expiresAt: "2099-01-01T00:00:00Z",
    filename: "historical-result.jpg",
    orientation: "landscape",
    sampleFormat: "uint8",
    colorSpace: "sRGB",
    iccEmbedded: true,
  };
  state.historicalExports.set(photoId, [
    {
      exportId: retained.exportId,
      target: retained.target,
      state: "succeeded",
      failureReason: "",
      createdAt: "2026-09-01T00:00:00Z",
      artifact: retained,
    },
  ]);
  const headers = Object.fromEntries(
    Object.entries({
      "export-id": retained.exportId,
      target: retained.target,
      stage: retained.stage,
      "content-type": retained.contentType,
      width: String(retained.width),
      height: String(retained.height),
      "profile-identity": retained.profileIdentity,
      "byte-length": String(retained.byteLength),
      sha256: retained.sha256,
      "expires-at": retained.expiresAt,
      filename: retained.filename,
      orientation: retained.orientation,
      "sample-format": retained.sampleFormat,
      "color-space": retained.colorSpace,
      "icc-embedded": String(retained.iccEmbedded),
    }).map(([name, value]) => [`slipstream-artifact-${name}`, value]),
  );
  await page.route("**/api/exports/historical-result/artifact", (route) =>
    route.fulfill({
      status: 200,
      contentType: retained.contentType,
      headers,
      body: bytes,
    }),
  );
  await openFirst(page, running.url);
  await expect(submit(page)).toBeDisabled();
  const card = page.locator('[data-historical-export-id="historical-result"]');
  await expect(card).toContainText(retained.filename);
  await navigate(page, "Next");
  await expect(card).toHaveCount(0);
  await navigate(page, "Previous");
  const download = page.waitForEvent("download");
  await card
    .getByRole("button", { name: "Download historical output" })
    .click();
  const file = await download;
  expect(file.suggestedFilename()).toBe(retained.filename);
  expect(await readFile(await file.path())).toEqual(bytes);
});

for (const mismatch of ["header", "digest"] as const) {
  test(`a retained artifact with mismatched ${mismatch} is refused before download`, async ({
    page,
    running,
  }) => {
    const state = await mockEditor(page, running);
    const bytes = Buffer.from("validated published bytes");
    const retained = artifact(state.photos[0]!, bytes);
    state.artifacts.set(retained.photoId, [retained]);
    let downloads = 0;
    page.on("download", () => downloads++);
    await page.route(
      `**/api/processing-artifacts/${retained.artifactId}/bytes`,
      (route) =>
        route.fulfill({
          status: 200,
          contentType: "image/jpeg",
          headers: {
            ...artifactHeaders(retained),
            ...(mismatch === "header"
              ? { "slipstream-artifact-step-id": "another-step" }
              : {}),
          },
          body: mismatch === "digest" ? Buffer.alloc(bytes.length, 0) : bytes,
        }),
    );
    await openFirst(page, running.url);
    await artifactRow(page, retained.artifactId)
      .getByRole("button", { name: "Download", exact: true })
      .click();
    await expect(
      page.locator("[data-photo-editor-export-state]"),
    ).toContainText("discarded");
    expect(downloads).toBe(0);
  });
}
