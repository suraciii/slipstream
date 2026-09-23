import { test, expect } from "@playwright/test";
import { mkdtemp, mkdir, copyFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  startBrowserServer,
  fixtureFetch,
  type BrowserServer,
} from "./browser-server.js";

let base: string;
let server: BrowserServer;
test.beforeEach(async () => {
  base = await mkdtemp(join(tmpdir(), "slipstream-access-browser-"));
  const root = join(base, "originals");
  await mkdir(root);
  await copyFile(
    "apps/web/test-fixtures/review.jpg",
    join(root, "synthetic.jpg"),
  );
  server = await startBrowserServer({ base, root });
  await expect
    .poll(
      async () =>
        (
          (await (await fixtureFetch(`${server.url}/api/status`)).json()) as {
            state: string;
          }
        ).state,
    )
    .toBe("idle");
});
test.afterEach(async () => {
  await server?.close();
  if (base) await rm(base, { recursive: true, force: true });
});

for (const viewport of [
  { width: 1280, height: 800 },
  { width: 390, height: 540 },
]) {
  test(`HTTPS access, revalidation, cross-tab sign out and history at ${viewport.width}px`, async ({
    page,
    context,
  }) => {
    await page.setViewportSize(viewport);
    const destination = `${server.url}/?selection=selected`;
    await page.goto(destination);
    await expect(
      page.getByLabel("Access Token", { exact: true }),
    ).toBeVisible();
    await expect(page.locator("img")).toHaveCount(0);
    const anonymous = await context.request.get(`${server.url}/api/overview`);
    expect(anonymous.status()).toBe(401);
    expect(anonymous.headers()["cache-control"]).toBe("no-store");
    await page.getByRole("button", { name: "Show Access Token" }).click();
    await expect(
      page.getByLabel("Access Token", { exact: true }),
    ).toHaveAttribute("type", "text");
    await page
      .getByLabel("Access Token", { exact: true })
      .fill("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    await page
      .getByRole("button", { name: "Open library", exact: true })
      .click();
    await expect(page.getByRole("alert")).toContainText(/token/i);
    await page.getByLabel("Access Token", { exact: true }).fill(server.token);
    await page.getByLabel("Access Token", { exact: true }).press("Enter");
    await expect(
      page.getByRole("navigation", { name: "Sources", includeHidden: true }),
    ).toBeAttached();
    expect(new URL(page.url()).origin).toBe(server.url);
    expect(new URL(page.url()).searchParams.get("selection")).toBe("selected");
    expect(page.url()).not.toContain(server.token);
    expect(
      await page.evaluate(() => ({
        local: { ...localStorage },
        session: { ...sessionStorage },
      })),
    ).toEqual({ local: {}, session: {} });
    const cookies = await context.cookies();
    const session = cookies.find(
      (cookie) => cookie.name === "__Host-slipstream",
    );
    expect(session?.secure).toBe(true);
    expect(session?.httpOnly).toBe(true);
    expect(session?.sameSite).toBe("Lax");
    expect(await page.evaluate(() => document.cookie)).not.toContain(
      "__Host-slipstream",
    );
    const second = await context.newPage();
    await second.goto(server.url);
    await expect(
      second.getByRole("navigation", { name: "Sources", includeHidden: true }),
    ).toBeAttached();
    await page.bringToFront();
    // Revalidation of this same session must leave the mounted fetcher usable.
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await expect(
      page.getByRole("navigation", { name: "Sources", includeHidden: true }),
    ).toBeAttached();
    if (viewport.width < 600)
      await page.getByRole("button", { name: /^Sources/ }).click();
    await page.getByRole("button", { name: "Sign out", exact: true }).click();
    await expect(
      page.getByLabel("Access Token", { exact: true }),
    ).toBeVisible();
    await expect(
      second.getByLabel("Access Token", { exact: true }),
    ).toBeVisible();
    await expect(page.locator("img")).toHaveCount(0);
    await expect(second.locator("img")).toHaveCount(0);
    await page.goBack();
    await page.goto(destination);
    await expect(
      page.getByLabel("Access Token", { exact: true }),
    ).toBeVisible();
    expect(
      (await context.request.get(`${server.url}/api/status`)).status(),
    ).toBe(401);
  });
}

test("cookie writes reject missing CSRF and revoked session cannot restore private views", async ({
  page,
  context,
}) => {
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  await expect(
    page.getByRole("navigation", { name: "Sources", includeHidden: true }),
  ).toBeAttached();
  expect(
    (
      await context.request.post(`${server.url}/api/scan`, {
        headers: { Origin: server.url },
      })
    ).status(),
  ).toBe(403);
  const status = (await (
    await context.request.get(`${server.url}/api/access/session`)
  ).json()) as { csrfToken: string };
  expect(
    (
      await context.request.delete(`${server.url}/api/access/session`, {
        headers: { Origin: server.url, "X-CSRF-Token": status.csrfToken },
      })
    ).status(),
  ).toBe(204);
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect(page.getByLabel("Access Token", { exact: true })).toBeVisible();
  await expect(page.locator("img")).toHaveCount(0);
});

test("known expiry hides private content while disconnected", async ({
  page,
  context,
}) => {
  await page.clock.install();
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  await expect(
    page.getByRole("navigation", { name: "Sources" }),
  ).toBeAttached();
  await context.setOffline(true);
  await page.clock.fastForward(7 * 24 * 60 * 60 * 1000 + 1000);
  await expect(page.getByRole("alert")).toContainText("expired");
  await expect(page.getByLabel("Access Token", { exact: true })).toBeVisible();
  await expect(page.locator("img")).toHaveCount(0);
});

test("failed sign out hides Photos without claiming confirmed revocation", async ({
  page,
  context,
}) => {
  await page.goto(server.url);
  await page.getByLabel("Access Token", { exact: true }).fill(server.token);
  await page.getByRole("button", { name: "Open library", exact: true }).click();
  await expect(
    page.getByRole("navigation", { name: "Sources" }),
  ).toBeAttached();
  await context.setOffline(true);
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "could not confirm sign out",
  );
  await expect(page.locator("img")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Retry sign out", exact: true }),
  ).toBeVisible();
});

for (const status of [307, 308]) {
  test(`token exchange refuses cross-origin ${status} redirects`, async ({
    page,
  }) => {
    let externalRequests = 0;
    await page.route("https://redirect.invalid/**", async (route) => {
      externalRequests++;
      await route.fulfill({
        status: 204,
        headers: {
          "Access-Control-Allow-Origin": server.url,
          "Access-Control-Allow-Methods": "POST, OPTIONS",
          "Access-Control-Allow-Headers": "content-type",
        },
      });
    });
    await page.route("**/api/access/session", async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      await route.fulfill({
        status,
        headers: { Location: "https://redirect.invalid/collect" },
      });
    });
    await page.goto(server.url);
    await page.getByLabel("Access Token", { exact: true }).fill(server.token);
    await page
      .getByRole("button", { name: "Open library", exact: true })
      .click();
    await expect(page.getByRole("alert")).toContainText(
      "did not confirm whether access opened",
    );
    expect(externalRequests).toBe(0);
    await expect(page.locator("img")).toHaveCount(0);
  });
}
