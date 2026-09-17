import { test, expect } from "./fixtures";

// A timed event with independent start/end dates must be storable and must render in every day
// column it covers. The first week column is used so "the next day" is always still in view.
test("an overnight event spans both day columns and cancelling strikes it through", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "week", exact: true }).click();
  await page.locator(".tg-col").first().click({ position: { x: 40, y: 140 } });

  const modal = page.locator(".modal");
  await expect(modal.getByText("New event")).toBeVisible();
  await modal.locator("input").first().fill("Overnight");

  const dates = modal.locator('input[type="date"]');
  const startDate = await dates.first().inputValue();
  const next = new Date(`${startDate}T12:00:00`);
  next.setDate(next.getDate() + 1);
  const nextDate = `${next.getFullYear()}-${String(next.getMonth() + 1).padStart(2, "0")}-${String(next.getDate()).padStart(2, "0")}`;

  await modal.locator('input[type="time"]').first().fill("22:00");
  await dates.nth(1).fill(nextDate);
  await modal.locator('input[type="time"]').nth(1).fill("02:00");
  await modal.getByRole("button", { name: "Create" }).click();

  const blocks = page.locator(".evblock", { hasText: "Overnight" });
  await expect(blocks).toHaveCount(2);

  // Status → Cancelled renders struck-through.
  await blocks.first().click();
  await expect(modal.getByText("Edit event")).toBeVisible();
  // The status select is found by its options, not its position (the form keeps growing).
  await modal.locator("select", { has: page.locator('option[value="Cancelled"]') }).selectOption("Cancelled");
  await modal.getByRole("button", { name: "Save" }).click();
  await expect(page.locator(".evblock.cancelled", { hasText: "Overnight" }).first()).toBeVisible();
});
