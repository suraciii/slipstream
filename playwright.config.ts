import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "apps/web/src",
  testMatch: "**/*.browser-test.ts",
  // Each scenario provisions its own server, state directory, and browser
  // context, so scenarios inside one file are independent. The smoke suite
  // stays small enough for the 4-vCPU runner to admit one worker per core.
  fullyParallel: true,
  workers: process.env.CI ? 4 : undefined,
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
