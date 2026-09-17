import { test, expect } from "./fixtures";

/** Today at `hour`:00 local, one hour long. */
function todayAt(hour: number): { start: string; end: string } {
  const d = new Date();
  const start = new Date(d.getFullYear(), d.getMonth(), d.getDate(), hour, 0);
  return { start: start.toISOString(), end: new Date(start.getTime() + 3600_000).toISOString() };
}

test("toggling a weekday chip keeps the rule's other days", async ({ page, app }) => {
  const { start, end } = todayAt(10);
  await page.request.post(app.base + "/api/events", {
    data: {
      uid: "",
      calendar: "default",
      summary: "Weekly sync",
      all_day: false,
      start,
      end,
      rrule: { freq: "Weekly", interval: 1, by_weekday: [{ weekday: "Mon" }, { weekday: "Wed" }] },
    },
  });
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "agenda", exact: true }).click();
  await page.locator(".agenda-row", { hasText: "Weekly sync" }).first().click();

  const modal = page.locator(".modal");
  await expect(modal.getByText("Edit event")).toBeVisible();
  // The server sends BYDAY as objects; both configured days must render selected.
  await expect(modal.locator(".chip.on")).toHaveCount(2);

  await modal.locator(".chip", { hasText: "Fri" }).click();
  await expect(modal.locator(".chip.on")).toHaveCount(3);
  await modal.getByRole("button", { name: "Save" }).click();
  await page.getByRole("button", { name: "All events" }).click();

  await page.reload();
  await page.getByRole("button", { name: "agenda", exact: true }).click();
  await page.locator(".agenda-row", { hasText: "Weekly sync" }).first().click();
  await expect(modal.locator(".chip.on")).toHaveCount(3);
});

test("dragging one occurrence asks for a scope and moves only that instance", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "week", exact: true }).click();
  await page.locator(".tg-col").first().click({ position: { x: 40, y: 140 } });
  const modal = page.locator(".modal");
  await modal.locator("input").first().fill("Daily sync");
  await modal.locator('select:has(option[value="Daily"])').selectOption("Daily");
  await modal.getByRole("button", { name: "Create" }).click();

  const blocks = page.locator(".evblock", { hasText: "Daily sync" });
  await expect(blocks).toHaveCount(7);

  const first = page.locator(".tg-col").first().locator(".evblock", { hasText: "Daily sync" });
  const third = page.locator(".tg-col").nth(2).locator(".evblock", { hasText: "Daily sync" });
  const before = (await third.textContent())!.trim();

  // Drag the third occurrence one hour down (48px = PX_PER_HOUR).
  const box = (await third.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2 + 48, { steps: 10 });
  await page.mouse.up();

  await page.getByRole("button", { name: "This event" }).click();

  await expect(third).not.toHaveText(before);
  await expect(first).toHaveText(before); // the rest of the series stayed put
  await expect(blocks).toHaveCount(7);
});

test("moving a recurring event with scope All shifts every occurrence", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "week", exact: true }).click();
  await page.locator(".tg-col").first().click({ position: { x: 40, y: 140 } });
  const modal = page.locator(".modal");
  await modal.locator("input").first().fill("Daily sync");
  await modal.locator('select:has(option[value="Daily"])').selectOption("Daily");
  await modal.getByRole("button", { name: "Create" }).click();

  const blocks = page.locator(".evblock", { hasText: "Daily sync" });
  await expect(blocks).toHaveCount(7);
  const first = page.locator(".tg-col").first().locator(".evblock", { hasText: "Daily sync" });
  const before = (await first.textContent())!.trim();

  const box = (await first.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2 + 48, { steps: 10 });
  await page.mouse.up();
  await page.getByRole("button", { name: "All events" }).click();

  await expect(blocks).toHaveCount(7);
  const moved = (await first.textContent())!.trim();
  expect(moved).not.toBe(before);
  const times = await blocks.allTextContents();
  expect(times.every((t) => t.trim() === moved)).toBe(true); // the whole series shifted
});
