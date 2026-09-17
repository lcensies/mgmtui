import { test, expect } from "./fixtures";

/** An event today at 10:00–11:00 local, in the given calendar. */
function todayAt(hour: number): { start: string; end: string } {
  const d = new Date();
  const start = new Date(d.getFullYear(), d.getMonth(), d.getDate(), hour, 0);
  return { start: start.toISOString(), end: new Date(start.getTime() + 3600_000).toISOString() };
}

test("week start Sunday puts Sunday in the first month column", async ({ page, app }) => {
  await page.request.put(app.base + "/api/settings", { data: { weekStart: "sun" } });
  await page.goto(app.base + "/");
  await expect(page.locator(".month-head .dow").first()).toHaveText("Sun");
  // and the grid actually starts on a Sunday
  const firstDay = await page.locator(".month-row .daycell").first().getAttribute("data-day");
  expect(new Date(firstDay + "T12:00:00").getDay()).toBe(0);
});

test("hiding a calendar removes its events from the views", async ({ page, app }) => {
  const { start, end } = todayAt(10);
  await page.request.post(app.base + "/api/events", {
    data: { uid: "", calendar: "work", summary: "Standup", start, end, all_day: false },
  });
  await page.request.post(app.base + "/api/events", {
    data: { uid: "", calendar: "default", summary: "Dentist", start, end, all_day: false },
  });
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "agenda", exact: true }).click();
  await expect(page.locator(".agenda-row", { hasText: "Standup" })).toBeVisible();

  await page.getByRole("checkbox", { name: "work" }).uncheck();
  await expect(page.locator(".agenda-row", { hasText: "Standup" })).toHaveCount(0);
  await expect(page.locator(".agenda-row", { hasText: "Dentist" })).toBeVisible();

  // the choice is device-local and survives a reload
  await page.reload();
  await page.getByRole("button", { name: "agenda", exact: true }).click();
  await expect(page.locator(".agenda-row", { hasText: "Standup" })).toHaveCount(0);
});

test("agenda groups the range into one header per day", async ({ page, app }) => {
  const day = (offset: number, hour: number) => {
    const d = new Date();
    const s = new Date(d.getFullYear(), d.getMonth(), d.getDate() + offset, hour, 0);
    return { start: s.toISOString(), end: new Date(s.getTime() + 3600_000).toISOString() };
  };
  for (const [summary, when] of [["Standup", day(0, 9)], ["Retro", day(0, 11)], ["Review", day(1, 9)]] as const) {
    await page.request.post(app.base + "/api/events", {
      data: { uid: "", calendar: "default", summary, all_day: false, ...when },
    });
  }
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "agenda", exact: true }).click();

  await expect(page.locator(".agenda-day")).toHaveCount(2);
  const today = page.locator(".agenda-day").first();
  await expect(today.locator(".agenda-row")).toHaveCount(2); // both of today's, in start order
  await expect(today.locator(".agenda-row").first()).toContainText("Standup");
  await expect(page.locator(".agenda-day").nth(1).locator(".agenda-row")).toHaveCount(1);
});

test("visible hours clamp the grid and push earlier events to its edge", async ({ page, app }) => {
  await page.request.put(app.base + "/api/settings", { data: { visibleHours: [7, 22] } });
  const d = new Date();
  const start = new Date(d.getFullYear(), d.getMonth(), d.getDate(), 5, 0);
  await page.request.post(app.base + "/api/events", {
    data: {
      uid: "",
      calendar: "default",
      summary: "Early bird",
      all_day: false,
      start: start.toISOString(),
      end: new Date(start.getTime() + 3600_000).toISOString(),
    },
  });
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "day", exact: true }).click();

  await expect(page.locator(".tg-hour")).toHaveCount(15); // 07:00 .. 21:00
  const block = page.locator(".evblock", { hasText: "Early bird" });
  await expect(block).toHaveAttribute("style", /top: 0px/); // clamped to the top edge
});

test("year view shows twelve mini months and marks days with events", async ({ page, app }) => {
  const { start, end } = todayAt(9);
  await page.request.post(app.base + "/api/events", {
    data: { uid: "", calendar: "default", summary: "Kickoff", start, end, all_day: false },
  });
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "year", exact: true }).click();
  await expect(page.locator(".year .mini")).toHaveCount(12);
  await expect(page.locator(".mini-day.has").first()).toBeVisible();
});

test("duplicate opens a new-event form prefilled from the current event", async ({ page, app }) => {
  const { start, end } = todayAt(14);
  await page.request.post(app.base + "/api/events", {
    data: { uid: "", calendar: "default", summary: "Review", start, end, all_day: false },
  });
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "agenda", exact: true }).click();
  await page.locator(".agenda-row", { hasText: "Review" }).click();
  await page.getByRole("button", { name: "Duplicate" }).click();
  const modal = page.locator(".modal");
  await expect(modal.getByText("New event")).toBeVisible();
  await expect(modal.locator("input").first()).toHaveValue("Review");
  await modal.getByRole("button", { name: "Create" }).click();
  await expect(page.locator(".agenda-row", { hasText: "Review" })).toHaveCount(2);
});
