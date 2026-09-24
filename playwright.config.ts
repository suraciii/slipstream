import { defineConfig } from "@playwright/test";

export default defineConfig({
  // Most scenarios live in one file. Without full parallelism Playwright runs
  // that file on a single worker, so three of the runner's four CPUs idle.
  // Every scenario provisions its own Library, server, and browser context, so
  // scenarios do not share state and can run in any order.
  fullyParallel: true,
  // CI runners are dedicated to this job; use all of their CPUs instead of the
  // half-of-available default. Local runs keep the default.
  workers: process.env.CI ? "100%" : undefined,
  testDir: "apps/web/src",
  testMatch: "**/*.browser-test.ts",
  use: {
    browserName: "chromium",
    headless: true,
    ignoreHTTPSErrors: true,
    ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
      ? {
          launchOptions: {
            executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE,
          },
        }
      : {}),
  },
});
