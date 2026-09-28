import { test, expect, type Page } from "@playwright/test";
import { copyFile, mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  fixtureFetch,
  startBrowserServer,
  type BrowserServer,
} from "./browser-server.js";

let base: string;
let running: BrowserServer;

test.beforeEach(async ({ context }) => {
  base = await mkdtemp(join(tmpdir(), "slipstream-export-browser-"));
  const root = join(base, "originals");
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
  await expect
    .poll(
      async () =>
        (
          (await (await fixtureFetch(`${running.url}/api/status`)).json()) as {
            state: string;
          }
        ).state,
    )
    .toBe("idle");
});

test.afterEach(async () => {
  await running?.close();
  if (base) await rm(base, { recursive: true, force: true });
});

async function openEdit(page: Page) {
  await page.locator("[data-dock-more]").click();
  await expect(page.locator("[data-photo-tools]")).toBeVisible();
  await page.locator("[data-photo-tools-entry='edit']").click();
  await expect(page.locator("[data-photo-tools-view='edit']")).toBeVisible();
}

async function navigate(page: Page, direction: "Next" | "Previous") {
  await page.locator("[data-photo-tools-close]").click();
  await page.getByRole("button", { name: direction, exact: true }).click();
  await openEdit(page);
}

for (const lostResponse of [
  "connection",
  "server-unknown",
  "server-error",
] as const) {
  test(`an uncertain Export (${lostResponse}) keeps its Photo's identity without blocking another Photo`, async ({
    page,
  }) => {
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
    const [first, second] = photos.map((photo) => photo.id);
    if (!first || !second) throw new Error("Expected two Photos");
    await page.route("**/api/processing/capability", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          state: "ready",
          stages: { develop: "ready", film: "unavailable" },
          profiles: [],
        }),
      }),
    );
    const savedExposure = new Map([
      [first, 0],
      [second, 0],
    ]);
    await page.route("**/api/photos/*/edit-recipe", async (route) => {
      const photoId = new URL(route.request().url()).pathname.split("/")[3];
      if (!photoId || !savedExposure.has(photoId))
        throw new Error("Unexpected Photo");
      if (route.request().method() === "POST") {
        const request = JSON.parse(route.request().postData() ?? "{}") as {
          settings: { exposureEv: number };
        };
        savedExposure.set(photoId, request.settings.exposureEv);
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            outcome: "saved",
            recipeVersion: "recipe-1",
            sourceRevision: "source-1",
          }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          sourceSupport: "supported",
          supportReason: null,
          sourceRevision: "source-1",
          recipe: {
            recipeVersion: "recipe-1",
            exposureEv: savedExposure.get(photoId),
            whiteBalance: { mode: "as-shot" },
          },
          processingAvailable: true,
          controls: {
            exposure: { minimumEv: 0, maximumEv: 1, stepEv: 0.001 },
            whiteBalanceModes: ["as-shot"],
          },
        }),
      });
    });
    const submissions = new Map<
      string,
      Array<{
        requestId: string;
        target: string;
        expectedRecipeVersion: string;
        expectedSourceRevision: string;
      }>
    >();
    await page.route("**/api/photos/*/exports", async (route) => {
      const photoId = new URL(route.request().url()).pathname.split("/")[3];
      if (!photoId) throw new Error("Export has no Photo");
      if (route.request().method() === "GET") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ exports: [] }),
        });
        return;
      }
      const body = JSON.parse(route.request().postData() ?? "{}") as {
        requestId: string;
        target: string;
        expectedRecipeVersion: string;
        expectedSourceRevision: string;
      };
      const requests = submissions.get(photoId) ?? [];
      requests.push(body);
      submissions.set(photoId, requests);
      if (photoId === first && requests.length === 1) {
        if (lostResponse === "connection") {
          await route.abort("connectionreset");
        } else {
          await route.fulfill({
            status: 500,
            contentType: "application/json",
            body: JSON.stringify({
              error: {
                code:
                  lostResponse === "server-unknown"
                    ? "outcome_unknown"
                    : "internal_error",
              },
            }),
          });
        }
        return;
      }
      await route.fulfill({
        status: requests.length === 1 ? 201 : 200,
        contentType: "application/json",
        body: JSON.stringify({
          exportId: `export-${photoId}`,
          target: body.target,
          recipeVersion: body.expectedRecipeVersion,
          sourceRevision: body.expectedSourceRevision,
          state: "succeeded",
        }),
      });
    });
    await page.route("**/api/exports/*", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          exportId: new URL(route.request().url()).pathname.split("/").at(-1),
          target: "development-tiff",
          state: "succeeded",
          artifact: null,
        }),
      }),
    );

    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(running.url);
    await expect(page.locator("[data-grid-status]")).toContainText(
      "Ready · 2 Photos",
    );
    await page.locator('[data-photo-index="0"]').click();
    await expect(page.locator("[data-review]")).toBeVisible();
    await openEdit(page);
    const submit = page.locator("[data-photo-editor-export-submit]");
    await expect(submit).toBeEnabled();
    await submit.click();
    await expect(
      page.locator("[data-photo-editor-export-state]"),
    ).toContainText("outcome is unknown");
    await navigate(page, "Next");
    await expect(page.getByText("2 / 2")).toBeVisible();
    const exposure = page.locator("[data-photo-editor-exposure]");
    await expect(exposure).toBeEnabled();
    await exposure.evaluate((element: HTMLInputElement) => {
      element.value = "0.5";
      element.dispatchEvent(new Event("input", { bubbles: true }));
      element.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await expect.poll(() => savedExposure.get(second)).toBe(0.5);
    await expect(submit).toBeEnabled();
    await submit.click();
    await expect(
      page.locator("[data-photo-editor-export-state]"),
    ).toContainText("succeeded");
    await navigate(page, "Previous");
    await expect(submit).toBeDisabled();
    const reconcile = page.locator("[data-photo-editor-export-retry]");
    await expect(reconcile).toHaveText("Reconcile");
    await reconcile.click();
    await expect(
      page.locator("[data-photo-editor-export-state]"),
    ).toContainText("succeeded");
    expect(submissions.get(first)).toHaveLength(2);
    expect(submissions.get(first)?.[1]).toEqual(submissions.get(first)?.[0]);
    expect(submissions.get(second)).toHaveLength(1);
    expect(submissions.get(second)?.[0]?.requestId).not.toBe(
      submissions.get(first)?.[0]?.requestId,
    );
  });
}
