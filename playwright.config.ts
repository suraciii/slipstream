import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "apps/server/src",
  testMatch: "**/*.browser-test.ts",
  use: {
    browserName: "chromium",
    headless: true,
    ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
      ? {
          launchOptions: {
            executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE,
          },
        }
      : {}),
  },
});
