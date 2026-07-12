// Persisted web settings: time format, secondary timezone, and keyboard-shortcut overrides.
// Loaded from the server (durable, follows the vault) with a localStorage mirror for instant paint.

import { signal } from "@preact/signals";
import { api } from "../api";
import { setTimeFormat } from "../lib/time";
import { applyLang, type LangPref } from "../lib/i18n";
import { setNotifierEnabled } from "../lib/notify";

export type Action = "calendar" | "board" | "tasks" | "focus" | "palette" | "help" | "new" | "undo" | "redo" | "trash";

export const DEFAULT_KEYS: Record<Action, string> = {
  calendar: "1", board: "2", tasks: "3", focus: "4",
  palette: ":", help: "?", new: "n", undo: "u", redo: "U", trash: "g",
};

export interface Settings {
  timeFormat: "12" | "24";
  secondaryTz: string; // IANA zone, "" = none
  lang: LangPref;
  notifications: boolean;
  keys: Record<Action, string>;
}

const DEFAULTS: Settings = { timeFormat: "24", secondaryTz: "", lang: "auto", notifications: false, keys: { ...DEFAULT_KEYS } };
const LS = "mgmt:settings";

function hydrate(): Settings {
  try {
    const raw = localStorage.getItem(LS);
    if (raw) return normalize(JSON.parse(raw));
  } catch {
    /* ignore */
  }
  return { ...DEFAULTS, keys: { ...DEFAULT_KEYS } };
}

function normalize(s: Partial<Settings> | null): Settings {
  return {
    timeFormat: s?.timeFormat === "12" ? "12" : "24",
    secondaryTz: typeof s?.secondaryTz === "string" ? s.secondaryTz : "",
    lang: s?.lang === "en" || s?.lang === "ru" ? s.lang : "auto",
    notifications: s?.notifications === true,
    keys: { ...DEFAULT_KEYS, ...(s?.keys ?? {}) },
  };
}

export const settings = signal<Settings>(hydrate());

function apply(s: Settings) {
  setTimeFormat(s.timeFormat);
  applyLang(s.lang);
  setNotifierEnabled(s.notifications);
}
apply(settings.value);

/** Load from the server and apply (called once the session is known). */
export async function loadSettings() {
  try {
    const remote = await api.getSettings();
    const merged = normalize(remote as Partial<Settings>);
    settings.value = merged;
    localStorage.setItem(LS, JSON.stringify(merged));
    apply(merged);
  } catch {
    /* keep local */
  }
}

/** Patch settings, persist to server + localStorage, and apply immediately. */
export function saveSettings(patch: Partial<Settings>) {
  const next = normalize({ ...settings.value, ...patch });
  settings.value = next;
  localStorage.setItem(LS, JSON.stringify(next));
  apply(next);
  void api.putSettings(next as unknown as Record<string, unknown>).catch(() => {});
}
