// Pure calendar layout: split all-day vs timed, compute the visible time window, and greedily
// lane-pack overlapping events into the fewest side-by-side columns (matching the TUI day-grid).

import type { EventItem } from "../api";
import { addDays, minutesOfDay, sameDay } from "./time";

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

/** Lay out one day's events. `day` is a local Date at 00:00. */
export function layoutDay(day: Date, events: EventItem[]): DayLayout {
  const allDay: EventItem[] = [];
  const timed: { ev: EventItem; start: number; end: number }[] = [];

  for (const ev of events) {
    if (ev.all_day) {
      allDay.push(ev);
      continue;
    }
    const s = new Date(ev.start);
    const e = new Date(ev.end);
    // Clamp the (possibly multi-day) instance to this day's minutes.
    const start = sameDay(s, day) ? minutesOfDay(s) : 0;
    const end = sameDay(e, day) ? minutesOfDay(e) : 24 * 60;
    timed.push({ ev, start, end: Math.max(end, start + 15) });
  }

  // Visible window: expand [08,20] to fit all events, clamped to the day.
  let startHour = DEFAULT_START;
  let endHour = DEFAULT_END;
  for (const t of timed) {
    startHour = Math.min(startHour, Math.floor(t.start / 60));
    endHour = Math.max(endHour, Math.ceil(t.end / 60));
  }
  startHour = Math.max(0, startHour);
  endHour = Math.min(24, endHour);

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

/** Events (already expanded instances) that intersect the given UTC day `[day, day+1)`. */
export function eventsOnDay(day: Date, events: EventItem[]): EventItem[] {
  const next = addDays(day, 1);
  return events.filter((ev) => new Date(ev.start) < next && new Date(ev.end) > day);
}
