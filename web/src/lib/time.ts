// Time helpers. mgmt treats all times as UTC wall-clock (a 09:00 event is stored 09:00Z and the
// TUI shows "09:00"), so the web app also works in UTC everywhere — days, positioning, and form
// times are UTC — to stay consistent with the TUI/CLI regardless of the viewer's timezone.

export const MS_DAY = 86400_000;
const pad = (n: number) => String(n).padStart(2, "0");

let timeFormat: "12" | "24" = "24";
export function setTimeFormat(f: "12" | "24") {
  timeFormat = f;
}

/** Format UTC hour+minute per the configured 12/24h preference. */
function fmtHM(h: number, m: number): string {
  if (timeFormat === "24") return `${pad(h)}:${pad(m)}`;
  const ap = h < 12 ? "AM" : "PM";
  const h12 = h % 12 || 12;
  return `${h12}:${pad(m)} ${ap}`;
}

/** Hour-of-day label for the calendar gutter (UTC), per the time-format preference. */
export function hourLabel(h: number): string {
  return timeFormat === "24" ? `${pad(h)}:00` : `${(h % 12) || 12}${h < 12 ? "am" : "pm"}`;
}

/** A "day" is a Date at UTC midnight of the given date's UTC calendar day. */
export function startOfDay(d: Date): Date {
  return new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate()));
}

export function startOfToday(): Date {
  return startOfDay(new Date());
}

export function addDays(d: Date, n: number): Date {
  return new Date(d.getTime() + n * MS_DAY);
}

export function sameDay(a: Date, b: Date): boolean {
  return (
    a.getUTCFullYear() === b.getUTCFullYear() &&
    a.getUTCMonth() === b.getUTCMonth() &&
    a.getUTCDate() === b.getUTCDate()
  );
}

/** Monday (UTC) of the week containing `d`. */
export function startOfWeek(d: Date): Date {
  const x = startOfDay(d);
  const dow = (x.getUTCDay() + 6) % 7; // 0 = Monday
  return addDays(x, -dow);
}

/** Monday of the month grid: the Monday on/before the 1st of `d`'s UTC month. */
export function startOfMonthGrid(d: Date): Date {
  return startOfWeek(new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), 1)));
}

/** ISO-8601 week number (UTC). */
export function isoWeek(d: Date): number {
  const t = new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate()));
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

/** Time-of-day in UTC wall-clock, formatted per the 12/24h preference. */
export function hhmm(d: Date | string): string {
  const x = typeof d === "string" ? new Date(d) : d;
  return fmtHM(x.getUTCHours(), x.getUTCMinutes());
}

/** The UTC hour shown in a chosen IANA timezone (for a secondary-time gutter). */
export function secondaryHour(utcHour: number, tz: string): string | null {
  if (!tz) return null;
  try {
    const d = new Date(Date.UTC(2020, 0, 1, utcHour, 0));
    return new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", hour12: timeFormat === "12", timeZone: tz }).format(d);
  } catch {
    return null;
  }
}

/** Minutes since UTC midnight. */
export function minutesOfDay(d: Date): number {
  return d.getUTCHours() * 60 + d.getUTCMinutes();
}

/** RFC3339 for a UTC day + minutes-of-day. */
export function atMinutes(day: Date, minutes: number): string {
  return new Date(startOfDay(day).getTime() + minutes * 60000).toISOString();
}

/** YYYY-MM-DD (UTC). */
export function ymd(d: Date): string {
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`;
}

/** Locale date formatting pinned to UTC (so labels match the UTC wall-clock). */
export function fmtDate(d: Date, opts: Intl.DateTimeFormatOptions): string {
  return d.toLocaleDateString(undefined, { ...opts, timeZone: "UTC" });
}
