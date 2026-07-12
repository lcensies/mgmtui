// Time helpers. The app works in the browser's LOCAL timezone: days, grid positioning, labels,
// and form fields are local wall-clock, while the API carries true UTC instants (RFC3339 with Z)
// — user input is parsed as local time and serialized with .toISOString(). One exception:
// all-day events are anchored at UTC midnight of their calendar date (pure date semantics), so
// their day membership is derived with utcDate() below, not local getters.

import { lang } from "./i18n";
import { signal } from "@preact/signals";

/** Live "now", refreshed every 30s so the calendar's now-line + "today" marker advance over time
 *  (and cross midnight) without needing an interaction to force a re-render. Reading `.value`
 *  during render subscribes the component, so it re-renders on each tick. */
export const now = signal(new Date());
if (typeof window !== "undefined") {
  setInterval(() => { now.value = new Date(); }, 30_000);
}
export const MS_DAY = 86400_000;
const pad = (n: number) => String(n).padStart(2, "0");

let timeFormat: "12" | "24" = "24";
export function setTimeFormat(f: "12" | "24") {
  timeFormat = f;
}

/** Format an hour+minute pair per the configured 12/24h preference. */
function fmtHM(h: number, m: number): string {
  if (timeFormat === "24") return `${pad(h)}:${pad(m)}`;
  const ap = h < 12 ? "AM" : "PM";
  const h12 = h % 12 || 12;
  return `${h12}:${pad(m)} ${ap}`;
}

/** Hour-of-day label for the calendar gutter, per the time-format preference. */
export function hourLabel(h: number): string {
  return timeFormat === "24" ? `${pad(h)}:00` : `${(h % 12) || 12}${h < 12 ? "am" : "pm"}`;
}

/** A "day" is a Date at local midnight of the given date's local calendar day. */
export function startOfDay(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

export function startOfToday(): Date {
  return startOfDay(new Date());
}

/** Calendar-date arithmetic (not +n*24h), so stepping days stays correct across DST changes.
 *  Expects (and returns) a day-anchored Date at local midnight. */
export function addDays(d: Date, n: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
}

export function sameDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

/** Monday (local) of the week containing `d`. */
export function startOfWeek(d: Date): Date {
  const x = startOfDay(d);
  const dow = (x.getDay() + 6) % 7; // 0 = Monday
  return addDays(x, -dow);
}

/** Monday of the month grid: the Monday on/before the 1st of `d`'s local month. */
export function startOfMonthGrid(d: Date): Date {
  return startOfWeek(new Date(d.getFullYear(), d.getMonth(), 1));
}

/** ISO-8601 week number of `d`'s local calendar date (computed in UTC space to dodge DST). */
export function isoWeek(d: Date): number {
  const t = new Date(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()));
  const day = (t.getUTCDay() + 6) % 7;
  t.setUTCDate(t.getUTCDate() - day + 3);
  const firstThursday = new Date(Date.UTC(t.getUTCFullYear(), 0, 4));
  const fday = (firstThursday.getUTCDay() + 6) % 7;
  firstThursday.setUTCDate(firstThursday.getUTCDate() - fday + 3);
  return 1 + Math.round((t.getTime() - firstThursday.getTime()) / (7 * MS_DAY));
}

export function snap15(mins: number): number {
  return Math.round(mins / 15) * 15;
}

/** Time-of-day in local wall-clock, formatted per the 12/24h preference. */
export function hhmm(d: Date | string): string {
  const x = typeof d === "string" ? new Date(d) : d;
  return fmtHM(x.getHours(), x.getMinutes());
}

/** What the given local gutter hour reads in a chosen IANA timezone (secondary-time gutter).
 *  Anchored to today so the secondary zone's current DST offset applies. */
export function secondaryHour(localHour: number, tz: string): string | null {
  if (!tz) return null;
  try {
    const base = new Date();
    const d = new Date(base.getFullYear(), base.getMonth(), base.getDate(), localHour, 0);
    return new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", hour12: timeFormat === "12", timeZone: tz }).format(d);
  } catch {
    return null;
  }
}

/** Minutes since local midnight. */
export function minutesOfDay(d: Date): number {
  return d.getHours() * 60 + d.getMinutes();
}

/** RFC3339 UTC instant for a local day + minutes-of-local-day (the Date constructor rolls the
 *  minutes over, so this stays wall-clock-correct across a DST change). */
export function atMinutes(day: Date, minutes: number): string {
  return new Date(day.getFullYear(), day.getMonth(), day.getDate(), 0, minutes).toISOString();
}

/** YYYY-MM-DD (local calendar date). */
export function ymd(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** Local-midnight Date of `d`'s **UTC** calendar date. All-day events are anchored at UTC
 *  midnight of their date (pure date semantics), so which day they belong to must be read with
 *  UTC getters — with local getters a viewer west of UTC would see them on the previous day
 *  (and east of UTC across two days). */
export function utcDate(d: Date): Date {
  return new Date(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
}

/** Locale date formatting in the viewer's local timezone, in the UI language. */
export function fmtDate(d: Date, opts: Intl.DateTimeFormatOptions): string {
  return d.toLocaleDateString(lang.value, opts);
}
