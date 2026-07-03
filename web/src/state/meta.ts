// Cached taxonomy (meta) + app state flags, plus live theme application.

import { effect } from "@preact/signals";
import { api, type AppStateInfo, type Meta } from "../api";
import { resource } from "../lib/cache";
import { themeVars } from "../lib/colors";

export const metaRes = resource<Meta>("meta", api.meta);
export const stateRes = resource<AppStateInfo>("state", api.state);

/** Convenience signal for the current meta (may be null before first load / hydrated from cache). */
export const meta = metaRes.data;

// Apply theme CSS custom properties whenever meta changes.
effect(() => {
  const m = meta.value;
  if (!m || typeof document === "undefined") return;
  const root = document.documentElement;
  for (const [k, v] of Object.entries(themeVars(m))) root.style.setProperty(k, v);
});
