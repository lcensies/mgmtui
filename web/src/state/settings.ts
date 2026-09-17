// Persisted web settings: time format, secondary timezone, and keyboard-shortcut overrides.
// Loaded from the server (durable, follows the vault) with a localStorage mirror for instant paint.

import { signal, effect } from "@preact/signals";
import { api } from "../api";
import { setTimeFormat, setWeekStart } from "../lib/time";
import { applyLang, type LangPref } from "../lib/i18n";
import { setNotifierEnabled } from "../lib/notify";
import { meta } from "./meta";

export type WeekStart = "mon" | "sat" | "sun";

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
  /** Sidebar project order (names). Unlisted projects keep their alphabetical position after
   *  these. Lives in the server-persisted blob, so the arrangement follows the user's account
   *  rather than one browser. */
  projectOrder: string[];
  /** Calendar view preferences. Undefined = follow the server config (`calendar:` in config.yaml). */
  weekStart?: WeekStart;
  hideWeekends?: boolean;
  workHours?: [number, number];
  visibleHours?: [number, number];
}

/** Resolved calendar view options: the user's settings, else the server config, else the
 *  built-in defaults. Reads both signals, so components using it re-render on either change. */
export function calOpts(): { weekStart: WeekStart; hideWeekends: boolean; work: [number, number]; visible?: [number, number] } {
  const s = settings.value;
  const c = meta.value?.calendar;
  const range = (v: [number, number] | undefined, cfg: { start: number; end: number } | undefined, fb: [number, number]): [number, number] =>
    v ?? (cfg ? [cfg.start, cfg.end] : fb);
  const visible = range(s.visibleHours, c?.visible_hours, [0, 24]);
  return {
    weekStart: s.weekStart ?? ((c?.week_start as WeekStart) || "mon"),
    hideWeekends: s.hideWeekends ?? c?.hide_weekends ?? false,
    work: range(s.workHours, c?.work_hours, [9, 17]),
    // The whole day means "no explicit window" — the grid then sizes itself to the events.
    visible: visible[0] === 0 && visible[1] === 24 ? undefined : visible,
  };
}

const DEFAULTS: Settings = { timeFormat: "24", secondaryTz: "", lang: "auto", notifications: false, keys: { ...DEFAULT_KEYS }, projectOrder: [] };
const LS = "mgmt:settings";

function hydrate(): Settings {
  try {
    const raw = localStorage.getItem(LS);
    if (raw) return normalize(JSON.parse(raw));
  } catch {
    /* ignore */
  }
  return { ...DEFAULTS, keys: { ...DEFAULT_KEYS }, projectOrder: [] };
}

function hours(v: unknown): [number, number] | undefined {
  if (!Array.isArray(v) || v.length !== 2) return undefined;
  const [a, b] = v.map((x) => Math.min(24, Math.max(0, Math.round(Number(x)))));
  return Number.isFinite(a) && Number.isFinite(b) && b > a ? [a, b] : undefined;
}

function normalize(s: Partial<Settings> | null): Settings {
  return {
    timeFormat: s?.timeFormat === "12" ? "12" : "24",
    secondaryTz: typeof s?.secondaryTz === "string" ? s.secondaryTz : "",
    lang: s?.lang === "en" || s?.lang === "ru" ? s.lang : "auto",
    notifications: s?.notifications === true,
    keys: { ...DEFAULT_KEYS, ...(s?.keys ?? {}) },
    projectOrder: Array.isArray(s?.projectOrder) ? s.projectOrder.filter((x) => typeof x === "string") : [],
    weekStart: s?.weekStart === "sun" || s?.weekStart === "sat" || s?.weekStart === "mon" ? s.weekStart : undefined,
    hideWeekends: typeof s?.hideWeekends === "boolean" ? s.hideWeekends : undefined,
    workHours: hours(s?.workHours),
    visibleHours: hours(s?.visibleHours),
  };
}

export const settings = signal<Settings>(hydrate());

function apply(s: Settings) {
  setTimeFormat(s.timeFormat);
  applyLang(s.lang);
  setNotifierEnabled(s.notifications);
}
apply(settings.value);
// Week start follows settings *or* the server config, whichever resolves — so it also updates
// once /api/meta lands.
effect(() => setWeekStart(calOpts().weekStart));

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
