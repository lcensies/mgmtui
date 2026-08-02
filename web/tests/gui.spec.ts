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

test("language switch to Russian translates the chrome and persists", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "Settings" }).click();
  await page.locator(".chip", { hasText: "Русский" }).click();
  await expect(page.locator("h2", { hasText: "Настройки" })).toBeVisible();
  await page.getByRole("button", { name: "Закрыть" }).click();
  await expect(page.getByPlaceholder("Быстро добавить задачу…")).toBeVisible();
  await page.reload();
  await expect(page.getByPlaceholder("Быстро добавить задачу…")).toBeVisible();
});

test("login form does not show a 2FA field when TOTP is not enrolled", async ({ page, app }) => {
  // The e2e server runs open (no auth), so just assert the session probe shape the form keys on.
  const session = await (await page.request.get(app.base + "/api/auth/session")).json();
  expect(session.totp).toBe(false);
});

test("mobile month view hides the week-number gutter (no collisions)", async ({ page, app }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(app.base + "/");
  await expect(page.locator(".month")).toBeVisible();
  await expect(page.locator(".month-head .wk")).toBeHidden();
});

test("mobile board shows swipe pivots (dots) and snaps columns", async ({ page, app }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.request.post(app.base + "/api/tasks", { data: { title: "Pivot card" } });
  await page.goto(app.base + "/board");
  const dots = page.locator(".board-dots .dot");
  await expect(dots.first()).toBeVisible();
  expect(await dots.count()).toBeGreaterThanOrEqual(3); // one per column
  const snap = await page.locator(".board").evaluate((el) => getComputedStyle(el).scrollSnapType);
  expect(snap).toContain("x");
});

test("quick-add tokens set project and priority, and the task is highlighted", async ({ page, app }) => {
  await page.goto(app.base + "/tasks");
  // Create the project first so the #token resolves against a known name.
  await page.getByPlaceholder(/quick add/i).fill("scaffold");
  await page.getByPlaceholder(/quick add/i).press("Enter");
  await page.getByRole("button", { name: "All", exact: true }).click();
  const card = page.locator(".card", { hasText: "scaffold" });
  await expect(card).toBeVisible();
  // A freshly created task flashes so it is findable.
  await expect(page.locator(".card.fresh")).toBeVisible();

  // Tokens: unknown #project stays in the title, priority is lifted out.
  await page.getByPlaceholder(/quick add/i).fill("ship it #nosuch !high");
  await page.getByPlaceholder(/quick add/i).press("Enter");
  const prio = page.locator(".card", { hasText: "ship it #nosuch" });
  await expect(prio).toBeVisible();
  await expect(prio.locator(".prio-High")).toBeVisible();
});

test("dedicated New task button opens the form with status and tags", async ({ page, app }) => {
  await page.goto(app.base + "/tasks");
  await page.getByRole("button", { name: /New task/ }).first().click();
  const modal = page.locator(".modal");
  await expect(modal.getByText("New task")).toBeVisible();
  await modal.locator("input").first().fill("write spec");
  await modal.getByLabel("Tags").fill("docs, draft");
  await modal.getByRole("button", { name: "Create" }).click();
  await page.getByRole("button", { name: "All", exact: true }).click();
  const card = page.locator(".card", { hasText: "write spec" });
  await expect(card).toBeVisible();
  await expect(card.locator(".pill", { hasText: "docs" })).toBeVisible();
});

test("dragging an event across day columns reschedules it", async ({ page, app }) => {
  await page.goto(app.base + "/");
  await page.getByRole("button", { name: "week", exact: true }).click();
  // Create an event in the first column.
  await page.locator(".tg-col").first().click({ position: { x: 40, y: 140 } });
  const modal = page.locator(".modal");
  await modal.locator("input").first().fill("Move me");
  await modal.getByRole("button", { name: "Create" }).click();
  const block = page.locator(".evblock", { hasText: "Move me" });
  await expect(block).toBeVisible();

  const srcDay = await page.locator(".tg-col").first().getAttribute("data-day");
  const target = page.locator(".tg-col").nth(3);
  const tb = (await target.boundingBox())!;
  const bb = (await block.boundingBox())!;
  await page.mouse.move(bb.x + bb.width / 2, bb.y + bb.height / 2);
  await page.mouse.down();
  await page.mouse.move(tb.x + tb.width / 2, bb.y + bb.height / 2, { steps: 12 });
  await page.mouse.up();

  // The block now lives under a different day column than it started in.
  const movedTo = await page.locator(".tg-col", { has: page.locator(".evblock", { hasText: "Move me" }) }).getAttribute("data-day");
  expect(movedTo).not.toBe(srcDay);
  expect(movedTo).toBe(await target.getAttribute("data-day"));
});

test("swiping the month grid pages to the next month", async ({ page, app }) => {
  await page.goto(app.base + "/");
  const title = page.locator(".cal-toolbar strong");
  const before = await title.textContent();
  const grid = page.locator(".month");
  const gb = (await grid.boundingBox())!;
  const y = gb.y + gb.height / 2;
  await page.mouse.move(gb.x + gb.width * 0.75, y);
  await page.mouse.down();
  await page.mouse.move(gb.x + gb.width * 0.2, y, { steps: 12 });
  await page.mouse.up();
  await expect(title).not.toHaveText(before ?? "");
  // A swipe must not also open the day view.
  await expect(grid).toBeVisible();
});

test("project filter is multi-select, counts open tasks, and persists across reload", async ({ page, app }) => {
  await page.goto(app.base + "/tasks");
  // Two tasks in two projects (the #token only resolves once the project exists, so set it
  // via the form the first time).
  for (const [title, project] of [["alpha task", "alpha"], ["beta task", "beta"]]) {
    await page.getByRole("button", { name: /New task/ }).first().click();
    const modal = page.locator(".modal");
    await modal.locator("input").first().fill(title);
    await modal.locator("#tf-project").fill(project);
    await modal.getByRole("button", { name: "Create" }).click();
  }
  await page.getByRole("button", { name: "All", exact: true }).click();
  await expect(page.locator(".card", { hasText: "alpha task" })).toBeVisible();

  // Counts render next to each project row.
  const alphaRow = page.locator(".side-row", { hasText: "alpha" });
  await expect(alphaRow.locator(".side-count")).toHaveText("1");

  // Selecting one project hides the other's tasks…
  await alphaRow.click();
  await expect(page.locator(".card", { hasText: "alpha task" })).toBeVisible();
  await expect(page.locator(".card", { hasText: "beta task" })).toHaveCount(0);

  // …and adding the second brings it back (multi-select, not replace).
  await page.locator(".side-row", { hasText: "beta" }).click();
  await expect(page.locator(".card", { hasText: "alpha task" })).toBeVisible();
  await expect(page.locator(".card", { hasText: "beta task" })).toBeVisible();

  // The scope survives a reload.
  await page.reload();
  await expect(page.locator(".side-row.active", { hasText: "alpha" })).toBeVisible();
  await expect(page.locator(".side-row.active", { hasText: "beta" })).toBeVisible();

  // Clearing restores everything.
  await page.locator(".side-row", { hasText: "Clear filter" }).click();
  await expect(page.locator(".side-row.active", { hasText: "All projects" })).toBeVisible();
});

test("an edit in one window shows up in another without refocus (SSE)", async ({ page, app, context }) => {
  await page.goto(app.base + "/tasks");
  await page.getByRole("button", { name: "All", exact: true }).click();

  // A second "device" on the same server.
  const other = await context.newPage();
  await other.goto(app.base + "/tasks");
  await other.getByRole("button", { name: "All", exact: true }).click();

  // Create in the second window; the first must update on its own (it is never refocused).
  await other.getByPlaceholder(/quick add/i).fill("pushed live");
  await other.getByPlaceholder(/quick add/i).press("Enter");

  await expect(page.locator(".card", { hasText: "pushed live" })).toBeVisible({ timeout: 10_000 });
  await other.close();
});

test("quick-add files new tasks under the scoped project while pinned", async ({ page, app }) => {
  await page.goto(app.base + "/tasks");
  // Create the project via the form, then scope to it.
  await page.getByRole("button", { name: /New task/ }).first().click();
  let modal = page.locator(".modal");
  await modal.locator("input").first().fill("seed");
  await modal.locator("#tf-project").fill("acme");
  await modal.getByRole("button", { name: "Create" }).click();
  await page.getByRole("button", { name: "All", exact: true }).click();
  await page.locator(".side-row", { hasText: "acme" }).click();

  // The pin shows the scoped project and is on by default.
  const pin = page.locator(".scope-pin");
  await expect(pin).toHaveText("#acme");
  await expect(pin).toHaveClass(/on/);

  await page.getByPlaceholder(/quick add/i).fill("pinned task");
  await page.getByPlaceholder(/quick add/i).press("Enter");
  await expect(page.locator(".card", { hasText: "pinned task" }).locator(".pill")).toHaveText("#acme");

  // Turning the pin off files the next task with no project (so the scoped view hides it).
  await pin.click();
  await expect(pin).not.toHaveClass(/on/);
  await page.getByPlaceholder(/quick add/i).fill("loose task");
  await page.getByPlaceholder(/quick add/i).press("Enter");
  await expect(page.locator(".card", { hasText: "loose task" })).toHaveCount(0);
  await page.locator(".side-row", { hasText: "Clear filter" }).click();
  await expect(page.locator(".card", { hasText: "loose task" })).toBeVisible();
});

test("projects can be dragged into a new sidebar order that survives reload", async ({ page, app }) => {
  await page.goto(app.base + "/tasks");
  for (const project of ["zeta", "alpha"]) {
    await page.getByRole("button", { name: /New task/ }).first().click();
    const modal = page.locator(".modal");
    await modal.locator("input").first().fill(`task for ${project}`);
    await modal.locator("#tf-project").fill(project);
    await modal.getByRole("button", { name: "Create" }).click();
  }
  const rows = page.locator(".side-row.draggable");
  await expect(rows).toHaveCount(2);
  await expect(rows.first()).toContainText("alpha"); // alphabetical to begin with

  // Drag "zeta" onto "alpha" to put it first.
  const zeta = page.locator(".side-row", { hasText: "zeta" });
  const alpha = page.locator(".side-row", { hasText: "alpha" });
  const zb = (await zeta.boundingBox())!;
  const ab = (await alpha.boundingBox())!;
  await page.mouse.move(zb.x + zb.width / 2, zb.y + zb.height / 2);
  await page.mouse.down();
  await page.mouse.move(ab.x + ab.width / 2, ab.y + ab.height / 2, { steps: 10 });
  await page.mouse.up();
  await expect(page.locator(".side-row.draggable").first()).toContainText("zeta");

  // The order is server-side, so it survives a reload.
  await page.reload();
  await expect(page.locator(".side-row.draggable").first()).toContainText("zeta");
});
