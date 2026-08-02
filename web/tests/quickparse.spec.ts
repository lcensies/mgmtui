// Pure unit test for the quick-add token parser (no browser needed — playwright is just the runner).

import { test, expect } from "@playwright/test";
import { parseQuickAdd } from "../src/lib/quickparse";

const PROJECTS = ["home", "Work"];
const NOW = new Date(2026, 0, 7, 10, 0); // Wednesday 2026-01-07, local

const localDate = (iso?: string) => (iso ? new Date(iso) : null);

test("lifts project and due tokens out of the title", () => {
  const r = parseQuickAdd("buy milk #home @tomorrow", PROJECTS, NOW);
  expect(r.title).toBe("buy milk");
  expect(r.project).toBe("home");
  const due = localDate(r.due)!;
  expect(due.getDate()).toBe(8);
  expect(due.getHours()).toBe(23);
});

test("unknown tokens stay in the title", () => {
  const r = parseQuickAdd("email @ 5pm draft #nope", PROJECTS, NOW);
  expect(r.title).toBe("email @ 5pm draft #nope");
  expect(r.due).toBeUndefined();
  expect(r.project).toBeUndefined();
});

test("project match is case-insensitive but returns the canonical name", () => {
  expect(parseQuickAdd("ship it #work", PROJECTS, NOW).project).toBe("Work");
});

test("priority tokens", () => {
  expect(parseQuickAdd("fix bug !high", PROJECTS, NOW).priority).toBe("High");
  expect(parseQuickAdd("fix bug !med", PROJECTS, NOW).priority).toBe("Medium");
  expect(parseQuickAdd("fix bug !nope", PROJECTS, NOW).title).toBe("fix bug !nope");
});

test("weekday resolves to the next future occurrence", () => {
  // NOW is a Wednesday; @wed must mean next week, not today.
  expect(localDate(parseQuickAdd("standup @wed", PROJECTS, NOW).due)!.getDate()).toBe(14);
  expect(localDate(parseQuickAdd("standup @fri", PROJECTS, NOW).due)!.getDate()).toBe(9);
});

test("ISO date and russian aliases", () => {
  expect(localDate(parseQuickAdd("pay rent @2026-03-01", PROJECTS, NOW).due)!.getMonth()).toBe(2);
  expect(localDate(parseQuickAdd("отчёт @завтра", PROJECTS, NOW).due)!.getDate()).toBe(8);
});

test("last token wins", () => {
  const r = parseQuickAdd("task @today @tomorrow !low !high", PROJECTS, NOW);
  expect(localDate(r.due)!.getDate()).toBe(8);
  expect(r.priority).toBe("High");
});
