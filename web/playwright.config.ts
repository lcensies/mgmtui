import { defineConfig } from "@playwright/test";

// GUI tests run against a real `mgmt web` server started per-test on a fresh, empty temp vault
// (see tests/fixtures.ts). MGMT_CHROMIUM points at a system Chromium (must not be firejailed);
// unset, Playwright's own download is used.
const CHROMIUM = process.env.MGMT_CHROMIUM || undefined;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  workers: 3,
  timeout: 30_000,
  expect: { timeout: 7_000 },
  reporter: [["list"]],
  use: {
    headless: true,
    launchOptions: { executablePath: CHROMIUM, args: ["--no-sandbox"] },
    trace: "off",
  },
});
