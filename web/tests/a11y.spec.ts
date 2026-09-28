// Guards the a11y floor the web-a11y-pass change established: chips are real buttons with state,
// and an open modal is an announced dialog that keeps Tab inside itself.

import { test, expect } from "./fixtures";

test("settings chips are keyboard-operable toggles", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "Settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Settings" });
  await expect(dialog).toBeVisible();

  const light = dialog.getByRole("button", { name: "light", exact: true });
  await expect(light).toHaveAttribute("aria-pressed", "false");
  await light.focus();
  await page.keyboard.press("Enter");

  await expect(light).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
});

test("Tab cannot leave an open dialog", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "Settings" }).click();
  await expect(page.getByRole("dialog", { name: "Settings" })).toBeVisible();

  const inside = () =>
    page.evaluate(() => document.querySelector(".modal")?.contains(document.activeElement) ?? false);

  // Enough presses to walk past the last control in the card and wrap around.
  for (let i = 0; i < 60; i++) await page.keyboard.press("Tab");
  expect(await inside()).toBe(true);

  await page.keyboard.press("Shift+Tab");
  expect(await inside()).toBe(true);
});
