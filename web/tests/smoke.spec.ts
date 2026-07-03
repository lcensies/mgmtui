import { test, expect } from "./fixtures";

test("app loads with an empty vault", async ({ page, app }) => {
  await page.goto(app.base + "/");
  // Nav is present; calendar is the default panel.
  await expect(page.locator("nav")).toBeVisible();
  // Quick-add is present.
  await expect(page.getByPlaceholder(/quick add/i)).toBeVisible();
});

test("quick-add creates a task and it shows in Tasks/All", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await page.getByPlaceholder(/quick add/i).fill("Buy milk");
  await page.getByPlaceholder(/quick add/i).press("Enter");
  await page.goto(app.base + "/tasks");
  await page.getByRole("button", { name: "All", exact: true }).click();
  await expect(page.getByText("Buy milk")).toBeVisible();
});
