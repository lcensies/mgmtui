// Theme preference: light / dark / system, persisted to localStorage and applied via the
// `data-theme` attribute on <html>. Reacts to system changes while in "system" mode.

import { effect, signal } from "@preact/signals";

export type ThemePref = "light" | "dark" | "system";

const KEY = "mgmt:theme";
const media = typeof matchMedia !== "undefined" ? matchMedia("(prefers-color-scheme: light)") : null;

export const themePref = signal<ThemePref>((localStorage.getItem(KEY) as ThemePref) || "system");

function resolve(p: ThemePref): "light" | "dark" {
  if (p === "system") return media?.matches ? "light" : "dark";
  return p;
}

/** The concrete theme currently applied. */
export const resolvedTheme = signal<"light" | "dark">(resolve(themePref.value));

effect(() => {
  const p = themePref.value;
  localStorage.setItem(KEY, p);
  const r = resolve(p);
  resolvedTheme.value = r;
  document.documentElement.setAttribute("data-theme", r);
});

media?.addEventListener("change", () => {
  if (themePref.value === "system") {
    const r = resolve("system");
    resolvedTheme.value = r;
    document.documentElement.setAttribute("data-theme", r);
  }
});

/** Header toggle: flip between the two concrete themes. */
export function toggleTheme() {
  themePref.value = resolvedTheme.value === "dark" ? "light" : "dark";
}

export function setTheme(p: ThemePref) {
  themePref.value = p;
}
