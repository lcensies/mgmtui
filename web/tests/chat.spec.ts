import { test, expect } from "./fixtures";

// The fixture's server opens a fresh vault with no config.yaml, so `GET /api/chat/config`
// reports `{enabled: false}` — chat must be fully invisible: no tab, and a direct hit on
// `/chat` bounces to the calendar instead of showing a dead composer.

test("chat tab is hidden when disabled", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await expect(page.getByRole("link", { name: "Chat" })).toHaveCount(0);
});

test("/chat redirects to the calendar when disabled", async ({ page, app }) => {
  await page.goto(app.base + "/chat");
  await expect(page).toHaveURL(app.base + "/");
  await expect(page.getByRole("link", { name: "Chat" })).toHaveCount(0);
  // and no composer was ever shown en route
  await expect(page.locator(".composer")).toHaveCount(0);
});
