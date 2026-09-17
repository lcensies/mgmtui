// Pure calendar layout: split all-day vs timed, compute the visible time window, and greedily
// lane-pack overlapping events into the fewest side-by-side columns (matching the TUI day-grid).

import { signal } from "@preact/signals";
import type { EventItem } from "../api";
import { addDays, minutesOfDay, sameDay, utcDate } from "./time";

export interface Positioned {
  ev: EventItem;
  topMin: number; // minutes from window start
  durMin: number;
  lane: number;
}

export interface DayLayout {
  timed: Positioned[];
  allDay: EventItem[];
  lanes: number;
  startHour: number;
  endHour: number;
}

const DEFAULT_START = 8;
const DEFAULT_END = 20;

/** Lay out one day's events. `day` is a local Date at 00:00. `vis` is the configured visible hour
 *  window: given, the grid renders exactly those hours and events outside are clamped to the edge;
 *  omitted, the window is [08,20] expanded to fit the day's events. */
export function layoutDay(day: Date, events: EventItem[], vis?: [number, number]): DayLayout {
  const allDay: EventItem[] = [];
  const timed: { ev: EventItem; start: number; end: number }[] = [];
  const [lo, hi] = vis ? [vis[0] * 60, vis[1] * 60] : [0, 24 * 60];

  for (const ev of events) {
    if (ev.all_day) {
      allDay.push(ev);
      continue;
    }
    const s = new Date(ev.start);
    const e = new Date(ev.end);
    // Clamp the (possibly multi-day) instance to this day's minutes, then to the visible window.
    const start = Math.min(Math.max(sameDay(s, day) ? minutesOfDay(s) : 0, lo), hi - 15);
    const end = Math.min(Math.max(sameDay(e, day) ? minutesOfDay(e) : 24 * 60, start + 15), hi);
    timed.push({ ev, start, end });
  }

  // Visible window: as configured, else [08,20] expanded to fit all events (clamped to the day).
  let startHour = vis ? vis[0] : DEFAULT_START;
  let endHour = vis ? vis[1] : DEFAULT_END;
  if (!vis) {
    for (const t of timed) {
      startHour = Math.min(startHour, Math.floor(t.start / 60));
      endHour = Math.max(endHour, Math.ceil(t.end / 60));
    }
    startHour = Math.max(0, startHour);
    endHour = Math.min(24, endHour);
  }

  // Greedy global lane packing (sort by start; first lane whose last end <= start).
  timed.sort((a, b) => a.start - b.start || a.end - b.end);
  const laneEnds: number[] = [];
  const positioned: Positioned[] = timed.map((t) => {
    let lane = laneEnds.findIndex((end) => end <= t.start);
    if (lane === -1) {
      lane = laneEnds.length;
      laneEnds.push(t.end);
    } else {
      laneEnds[lane] = t.end;
    }
    return { ev: t.ev, topMin: t.start - startHour * 60, durMin: t.end - t.start, lane };
  });

  return { timed: positioned, allDay, lanes: Math.max(1, laneEnds.length), startHour, endHour };
}

/** Events (already expanded instances) that intersect the given local day `[day, day+1)`.
 *  All-day events are anchored at UTC midnight of their calendar date (pure date semantics),
 *  so they are compared by calendar date (utcDate) rather than as local instants. */
export function eventsOnDay(day: Date, events: EventItem[]): EventItem[] {
  const next = addDays(day, 1);
  return events.filter((ev) =>
    ev.all_day
      ? utcDate(new Date(ev.start)) < next && utcDate(new Date(ev.end)) > day
      : new Date(ev.start) < next && new Date(ev.end) > day,
  );
}

/** Calendars the user has unchecked in the sidebar. Device-local (a view preference, not data),
 *  so it lives in localStorage rather than the server settings blob. */
const HIDDEN_KEY = "mgmt:hiddenCalendars";

function loadHidden(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(HIDDEN_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

export const hiddenCalendars = signal<string[]>(loadHidden());

export function toggleCalendar(name: string) {
  const cur = hiddenCalendars.value;
  const next = cur.includes(name) ? cur.filter((x) => x !== name) : [...cur, name];
  hiddenCalendars.value = next;
  try {
    localStorage.setItem(HIDDEN_KEY, JSON.stringify(next));
  } catch {
    /* private mode — the toggle just won't survive a reload */
  }
}

/** Drop events belonging to hidden calendars (client-side filter, all views). */
export function visibleEvents(events: EventItem[]): EventItem[] {
  const hidden = hiddenCalendars.value;
  return hidden.length ? events.filter((e) => !hidden.includes(e.calendar)) : events;
}

/** Weekend days removed when "hide weekends" is on. */
export function withoutWeekends(days: Date[], hide: boolean): Date[] {
  return hide ? days.filter((d) => d.getDay() !== 0 && d.getDay() !== 6) : days;
}
