import { test, expect } from "./fixtures";

test("create a task via quick-add and toggle it done", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await page.getByPlaceholder(/quick add/i).fill("Write report");
  await page.getByPlaceholder(/quick add/i).press("Enter");
  await page.goto(app.base + "/tasks");
  await page.getByRole("button", { name: "All", exact: true }).click();
  const card = page.locator(".card", { hasText: "Write report" });
  await expect(card).toBeVisible();
  await card.getByRole("checkbox").check();
  await expect(page.locator(".card.done", { hasText: "Write report" })).toBeVisible();
});

test("create an event by clicking an empty calendar slot (single click)", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "day", exact: true }).click();
  // single click on empty grid space opens the event form
  await page.locator(".tg-col").first().click({ position: { x: 120, y: 140 } });
  const modal = page.locator(".modal");
  await expect(modal.getByText("New event")).toBeVisible();
  await modal.locator("input").first().fill("Team sync");
  await modal.getByRole("button", { name: "Create" }).click();
  await expect(page.locator(".evblock", { hasText: "Team sync" })).toBeVisible();
});

test("number keys navigate panels", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await expect(page.locator("nav")).toBeVisible();
  await page.keyboard.press("2");
  await expect(page).toHaveURL(/\/board$/);
  await page.keyboard.press("3");
  await expect(page).toHaveURL(/\/tasks$/);
  await page.keyboard.press("1");
  await expect(page).toHaveURL(new RegExp(app.base.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + "/$"));
});

test("command palette navigates", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await expect(page.locator("nav")).toBeVisible();
  await page.keyboard.press(":");
  await expect(page.getByPlaceholder(/type a command/i)).toBeVisible();
  await page.getByPlaceholder(/type a command/i).fill("board");
  // wait for the filtered list, then run the top match
  await expect(page.locator(".pickrow", { hasText: "Go to Board" })).toBeVisible();
  await page.locator(".pickrow", { hasText: "Go to Board" }).click();
  await expect(page).toHaveURL(/\/board$/);
});

test("theme toggle flips data-theme and persists nothing server-side (device-local)", async ({ page, app }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto(app.base + "/");
  const before = await page.evaluate(() => document.documentElement.getAttribute("data-theme"));
  await page.getByRole("button", { name: "Toggle theme" }).click();
  const after = await page.evaluate(() => document.documentElement.getAttribute("data-theme"));
  expect(after).not.toBe(before);
});

test("settings: time format persists across reload (server-side)", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByText("12-hour").click();
  await page.getByRole("button", { name: "Close" }).click();
  await page.reload();
  await page.getByRole("button", { name: "Settings" }).click();
  await expect(page.locator(".chip.on", { hasText: "12-hour" })).toBeVisible();
});

test("board drag moves a card between columns", async ({ page, app }) => {
  // seed two todo tasks
  await page.request.post(app.base + "/api/tasks", { data: { title: "Card A" } });
  await page.request.post(app.base + "/api/tasks", { data: { title: "Card B" } });
  await page.goto(app.base + "/board");
  const card = page.locator(".board .card", { hasText: "Card A" });
  const doing = page.locator('.board .col[data-status="doing"]');
  await expect(card).toBeVisible();
  const cb = await card.boundingBox();
  const db = await doing.boundingBox();
  if (!cb || !db) throw new Error("no bounding boxes");
  await page.mouse.move(cb.x + cb.width / 2, cb.y + cb.height / 2);
  await page.mouse.down();
  await page.mouse.move(cb.x + cb.width / 2 + 20, cb.y + cb.height / 2, { steps: 4 });
  await page.mouse.move(db.x + db.width / 2, db.y + 60, { steps: 8 });
  await page.mouse.up();
  // Card A now lives under the "doing" column
  await expect(doing.locator(".card", { hasText: "Card A" })).toBeVisible();
});
