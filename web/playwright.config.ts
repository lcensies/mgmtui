import { defineConfig } from "@playwright/test";

// GUI tests run against a real `mgmt web` server started per-test on a fresh, empty temp vault
// (see tests/fixtures.ts). We use the system Chromium to avoid downloading Playwright's browser.
const CHROMIUM = process.env.MGMT_CHROMIUM || "/etc/profiles/per-user/esc2/bin/chromium";

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
