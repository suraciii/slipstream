import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { startBrowserServer } from "./browser-server.js";

test("production application publishes usable online installation resources", async ({
  page,
}) => {
  const base = await mkdtemp(join(tmpdir(), "slipstream-installation-"));
  const root = join(base, "originals");
  await mkdir(root);
  const server = await startBrowserServer({ base, root });
  try {
    await page.goto(server.url);
    await expect(
      page.getByLabel("Access Token", { exact: true }),
    ).toBeVisible();
    await expect(page.locator("img")).toHaveCount(0);
    const anonymousApi = await page.request.get(`${server.url}/api/overview`);
    expect(anonymousApi.status()).toBe(401);
    expect(anonymousApi.headers()["cache-control"]).toBe("no-store");

    const manifestUrl = await page
      .locator('link[rel="manifest"]')
      .getAttribute("href");
    expect(manifestUrl).toBe("/manifest.webmanifest");
    const response = await page.request.get(`${server.url}${manifestUrl}`);
    expect(response.status()).toBe(200);
    expect(response.headers()["content-type"]).toBe(
      "application/manifest+json",
    );
    expect(response.headers()["cache-control"]).toBe("no-cache");
    const manifest = (await response.json()) as {
      icons: { src: string; sizes: string; type: string; purpose: string }[];
    };
    expect(manifest).toMatchObject({
      id: "/",
      name: "Slipstream",
      short_name: "Slipstream",
      start_url: "/",
      scope: "/",
      display: "standalone",
    });
    expect(manifest.icons).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ sizes: "192x192", purpose: "any" }),
        expect.objectContaining({ sizes: "512x512", purpose: "any" }),
        expect.objectContaining({ sizes: "512x512", purpose: "maskable" }),
      ]),
    );
    const touchIcon = await page
      .locator('link[rel="apple-touch-icon"]')
      .getAttribute("href");
    if (!touchIcon) throw new Error("Missing touch icon");
    for (const icon of [
      ...manifest.icons,
      { src: touchIcon, sizes: "180x180", type: "image/png" },
    ]) {
      const imageResponse = await page.request.get(`${server.url}${icon.src}`);
      expect(imageResponse.status()).toBe(200);
      expect(imageResponse.headers()["content-type"]).toBe(icon.type);
      expect(imageResponse.headers()["cache-control"]).toBe("no-cache");
      const dimensions = await page.evaluate(async (src: string) => {
        const image = new Image();
        image.src = src;
        await image.decode();
        return `${image.naturalWidth}x${image.naturalHeight}`;
      }, icon.src);
      expect(dimensions).toBe(icon.sizes);
    }
    const cdp = await page.context().newCDPSession(page);
    const browserManifest = await cdp.send("Page.getAppManifest");
    expect(browserManifest.errors).toEqual([]);
    expect(browserManifest.url).toBe(`${server.url}/manifest.webmanifest`);
    await cdp.detach();
    expect(
      await page.evaluate(async () =>
        (await navigator.serviceWorker.getRegistrations()).map(
          (item) => item.scope,
        ),
      ),
    ).toEqual([]);

    await page.getByLabel("Access Token", { exact: true }).fill(server.token);
    await page
      .getByRole("button", { name: "Open library", exact: true })
      .click();
    await expect(page.locator("[data-grid-title]")).toHaveText("All Photos");
    const privateApi = await page.evaluate(async () => {
      const response = await fetch("/api/status");
      return {
        status: response.status,
        cacheControl: response.headers.get("Cache-Control"),
      };
    });
    expect(privateApi).toEqual({ status: 200, cacheControl: "no-store" });
  } finally {
    await server.close();
    await rm(base, { recursive: true, force: true });
  }
});
