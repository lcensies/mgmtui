import { test, expect } from "./fixtures";

test("settings: add a local calendar, then upload an .ics into it", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "Settings" }).click();

  await page.getByPlaceholder("New calendar name").fill("work");
  await page.getByRole("button", { name: "Add", exact: true }).click();

  const row = page.locator(".km-row", { hasText: "work" });
  await expect(row).toBeVisible();

  // Upload two VEVENTs; the row's event count reflects them.
  const ics =
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n" +
    [1, 2]
      .map((i) => `BEGIN:VEVENT\r\nUID:u${i}\r\nSUMMARY:Ev ${i}\r\nDTSTART:20260618T090000Z\r\nDTEND:20260618T100000Z\r\nEND:VEVENT\r\n`)
      .join("") +
    "END:VCALENDAR\r\n";
  await row.locator("input[type=file]").setInputFiles({ name: "cal.ics", mimeType: "text/calendar", buffer: Buffer.from(ics) });
  await expect(row.locator(".muted", { hasText: "(2)" })).toBeVisible();
});

// A subscription mirror is refetched wholesale, so its events must not be editable here.
test("events of a read-only calendar open in a disabled form", async ({ page, app }) => {
  const d = new Date();
  const start = new Date(d.getFullYear(), d.getMonth(), d.getDate(), 10, 0);
  await page.request.post(app.base + "/api/events", {
    data: {
      uid: "",
      calendar: "default",
      summary: "Bank holiday",
      all_day: false,
      start: start.toISOString(),
      end: new Date(start.getTime() + 3600_000).toISOString(),
    },
  });
  // Subscribing needs a reachable feed, so the flag is stubbed at the API boundary.
  await page.route("**/api/calendars", (route) =>
    route.fulfill({ json: [{ id: "default", display_name: "default", events: 1, read_only: true }] }),
  );

  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "agenda", exact: true }).click();
  await page.locator(".agenda-row", { hasText: "Bank holiday" }).click();

  const modal = page.locator(".modal");
  await expect(modal.getByText("This calendar is read-only")).toBeVisible();
  await expect(modal.locator("input").first()).toBeDisabled();
  await expect(modal.getByRole("button", { name: "Save" })).toHaveCount(0);
  await expect(modal.getByRole("button", { name: "Delete" })).toHaveCount(0);
});
