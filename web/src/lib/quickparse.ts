// Quick-add token parser: `#project`, `@date`, `!priority` are lifted out of the typed text and
// become task fields; anything unrecognized stays part of the title (so a literal "@ 5pm" or an
// unknown `#tag` never silently disappears — or worse, creates a typo project).

import type { Priority } from "../api";
import { addDays, startOfDay } from "./time";

export interface QuickAdd {
  title: string;
  project?: string;
  due?: string; // RFC3339 UTC instant, end of the local day (same convention as the task form)
  priority?: Priority;
}

const PRIORITY: Record<string, Priority> = {
  low: "Low",
  med: "Medium",
  medium: "Medium",
  high: "High",
  низкий: "Low",
  средний: "Medium",
  высокий: "High",
};

// 0 = Monday, matching startOfWeek()'s convention.
const WEEKDAY: Record<string, number> = {
  mon: 0, monday: 0, tue: 1, tuesday: 1, wed: 2, wednesday: 2, thu: 3, thursday: 3,
  fri: 4, friday: 4, sat: 5, saturday: 5, sun: 6, sunday: 6,
  пн: 0, вт: 1, ср: 2, чт: 3, пт: 4, сб: 5, вс: 6,
};

const TODAY = ["today", "сегодня"];
const TOMORROW = ["tomorrow", "завтра"];

/** End of the local day as a UTC instant — what the task form stores for `due`. */
function endOfDay(day: Date): string {
  return new Date(day.getFullYear(), day.getMonth(), day.getDate(), 23, 59, 0).toISOString();
}

/** Resolve an `@` token to a local day, or null when it is not a date we understand. */
function resolveDate(word: string, now: Date): Date | null {
  const w = word.toLowerCase();
  if (TODAY.includes(w)) return startOfDay(now);
  if (TOMORROW.includes(w)) return addDays(startOfDay(now), 1);
  const wd = WEEKDAY[w];
  if (wd !== undefined) {
    // Next occurrence, always in the future: "@mon" typed on a Monday means the coming Monday.
    const today = (startOfDay(now).getDay() + 6) % 7;
    return addDays(startOfDay(now), ((wd - today + 7) % 7) || 7);
  }
  const iso = /^(\d{4})-(\d{2})-(\d{2})$/.exec(w);
  if (iso) {
    const d = new Date(Number(iso[1]), Number(iso[2]) - 1, Number(iso[3]));
    return Number.isNaN(d.getTime()) ? null : d;
  }
  return null;
}

/**
 * Split typed quick-add text into a title plus any recognized field tokens. `projects` is the
 * list of known project names — an unknown `#foo` is left in the title rather than inventing a
 * project. Later tokens win over earlier ones.
 */
export function parseQuickAdd(text: string, projects: string[] = [], now: Date = new Date()): QuickAdd {
  const out: QuickAdd = { title: "" };
  const kept: string[] = [];
  for (const word of text.split(/\s+/)) {
    const rest = word.slice(1);
    if (word.startsWith("#") && rest) {
      const match = projects.find((p) => p.toLowerCase() === rest.toLowerCase());
      if (match) {
        out.project = match;
        continue;
      }
    } else if (word.startsWith("@") && rest) {
      const day = resolveDate(rest, now);
      if (day) {
        out.due = endOfDay(day);
        continue;
      }
    } else if (word.startsWith("!") && rest) {
      const p = PRIORITY[rest.toLowerCase()];
      if (p) {
        out.priority = p;
        continue;
      }
    }
    kept.push(word);
  }
  out.title = kept.join(" ").trim();
  return out;
}
