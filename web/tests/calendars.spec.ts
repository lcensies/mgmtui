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
