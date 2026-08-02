// Pure unit tests for sidebar project ordering (playwright is just the runner).

import { test, expect } from "@playwright/test";
import { moveProject, orderProjects } from "../src/lib/projectorder";

const p = (...names: string[]) => names.map((name) => ({ name }));
const names = (list: { name: string }[]) => list.map((x) => x.name);

test("arranged projects lead, unarranged keep their incoming order", () => {
  const list = p("alpha", "beta", "gamma", "delta");
  expect(names(orderProjects(list, ["gamma", "alpha"]))).toEqual(["gamma", "alpha", "beta", "delta"]);
});

test("empty order leaves the list untouched", () => {
  const list = p("alpha", "beta");
  expect(names(orderProjects(list, []))).toEqual(["alpha", "beta"]);
});

test("stale names in the order are ignored", () => {
  const list = p("alpha", "beta");
  expect(names(orderProjects(list, ["deleted", "beta"]))).toEqual(["beta", "alpha"]);
});

test("moving a project down and up", () => {
  const order = ["a", "b", "c", "d"];
  expect(moveProject(order, "a", "c")).toEqual(["b", "c", "a", "d"]);
  expect(moveProject(order, "d", "a")).toEqual(["d", "a", "b", "c"]);
});

test("moving onto itself or an unknown name is a no-op", () => {
  const order = ["a", "b"];
  expect(moveProject(order, "a", "a")).toEqual(order);
  expect(moveProject(order, "a", "zzz")).toEqual(order);
  expect(moveProject(order, "zzz", "a")).toEqual(order);
});
