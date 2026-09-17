import { test, expect } from "./fixtures";

/** `YYYYMMDD` of today, for ICS date-times that must land inside the agenda range. */
function icsToday(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}`;
}

// A Google-style invite: the form lists every ATTENDEE with the status they answered.
test("an imported invite lists its attendees and their answers", async ({ page, app }) => {
  const day = icsToday();
  const ics =
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:invite1\r\nSUMMARY:Design review\r\n" +
    `DTSTART:${day}T090000Z\r\nDTEND:${day}T100000Z\r\n` +
    "ORGANIZER;CN=Ann:mailto:ann@ex.org\r\n" +
    "ATTENDEE;CN=Ann;PARTSTAT=ACCEPTED:mailto:ann@ex.org\r\n" +
    "ATTENDEE;CN=Bob;PARTSTAT=ACCEPTED:mailto:bob@ex.org\r\n" +
    "ATTENDEE;CN=Cid;PARTSTAT=ACCEPTED:mailto:cid@ex.org\r\n" +
    "END:VEVENT\r\nEND:VCALENDAR\r\n";
  await page.request.post(app.base + "/api/calendars/default/import", {
    headers: { "content-type": "text/calendar" },
    data: ics,
  });

  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "agenda", exact: true }).click();
  await page.locator(".agenda-row", { hasText: "Design review" }).click();

  const modal = page.locator(".modal");
  await expect(modal.locator('input[type="email"]')).toHaveCount(3);
  await expect(modal.locator('input[type="email"]').first()).toHaveValue("ann@ex.org");
  await expect(modal.getByText("Accepted")).toHaveCount(3);
});

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
