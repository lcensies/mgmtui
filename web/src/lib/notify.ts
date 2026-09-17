// Web notifications for reminders + pomodoro phases. Uses the service-worker registration's
// showNotification when available (required on Android; a plain `new Notification` throws
// there), falling back to the constructor. Scheduling is client-side while the app is open:
// a 1-minute tick scans the next two days of agenda reminders (agenda cached for 5 minutes),
// and the running pomodoro's phase end gets a precise timeout. (Background push with the app
// closed would need a server-side push service — out of scope.)

import { api, type Alarm, type AlarmTrigger, type EventItem, type PomodoroWire, type Task } from "../api";
import { hhmm } from "./time";
import { t } from "./i18n";

const FIRED_LS = "mgmt:notified";
const TICK_MS = 60_000;
const AGENDA_TTL_MS = 5 * 60_000;
// Fire anything that came due in the last 10 minutes (covers sleep/tab-inactive gaps).
const LATE_MS = 10 * 60_000;

export function notificationsSupported(): boolean {
  return typeof window !== "undefined" && "Notification" in window;
}

export function notificationsGranted(): boolean {
  return notificationsSupported() && Notification.permission === "granted";
}

export function notificationsDenied(): boolean {
  return notificationsSupported() && Notification.permission === "denied";
}

/** Ask for permission (must be called from a user gesture, e.g. the Settings toggle). */
export async function requestNotifications(): Promise<boolean> {
  if (!notificationsSupported()) return false;
  return (await Notification.requestPermission()) === "granted";
}

export async function showNotification(title: string, body?: string): Promise<void> {
  if (!notificationsGranted()) return;
  const opts: NotificationOptions = { body, icon: "icons/icon-192.png", badge: "icons/icon-192.png" };
  try {
    const reg = await navigator.serviceWorker?.getRegistration();
    if (reg) {
      await reg.showNotification(title, opts);
      return;
    }
  } catch {
    /* fall through to the constructor */
  }
  try {
    new Notification(title, opts);
  } catch {
    /* constructor is unavailable on Android outside a SW — nothing more we can do */
  }
}

// ---- de-dup ledger (uid+timestamp keys, pruned to the last 24h) -----------------------

function loadFired(): Record<string, number> {
  try {
    return JSON.parse(localStorage.getItem(FIRED_LS) ?? "{}") as Record<string, number>;
  } catch {
    return {};
  }
}

function markFired(key: string) {
  const fired = loadFired();
  const cutoff = Date.now() - 24 * 3600_000;
  for (const [k, at] of Object.entries(fired)) if (at < cutoff) delete fired[k];
  fired[key] = Date.now();
  try {
    localStorage.setItem(FIRED_LS, JSON.stringify(fired));
  } catch {
    /* ignore */
  }
}

function fireOnce(key: string, title: string, body?: string) {
  if (key in loadFired()) return;
  markFired(key);
  void showNotification(title, body);
}

// ---- reminder computation --------------------------------------------------------------

/** "1d" / "2h" / "30m" → milliseconds. */
function offsetMs(s: string): number | null {
  const ms = offsetToMinutes(s);
  return ms === null ? null : ms * 60_000;
}

/** "1d" / "2h" / "30m" → minutes (null when malformed). Shared by the reminder form fields. */
export function offsetToMinutes(s: string): number | null {
  const m = /^(\d+)\s*([dhm])$/i.exec(s.trim());
  if (!m) return null;
  const n = Number(m[1]);
  return n * (m[2].toLowerCase() === "d" ? 1440 : m[2].toLowerCase() === "h" ? 60 : 1);
}

/** Minutes → the most compact "Nd"/"Nh"/"Nm" offset. */
export function minutesToOffset(min: number): string {
  if (min > 0 && min % 1440 === 0) return `${min / 1440}d`;
  if (min > 0 && min % 60 === 0) return `${min / 60}h`;
  return `${min}m`;
}

/**
 * Parse a comma-separated reminder-offset list ("15m, 1h, 1d") into normalized tokens.
 * Returns null when any token is malformed; an empty/blank input yields [].
 */
export function parseOffsetList(s: string): string[] | null {
  const out: string[] = [];
  for (const part of s.split(",").map((x) => x.trim()).filter(Boolean)) {
    const min = offsetToMinutes(part);
    if (min === null) return null;
    out.push(minutesToOffset(min));
  }
  return out;
}

/**
 * Parse one alarm token. Grammar (mirrors `mgmt_domain::AlarmTrigger`):
 *   `15m`/`1h`/`1d`  before the start
 *   `-10m`           after the start
 *   `end-5m`         before the end (`end+5m` = after it)
 *   `@2026-09-20T09:00`  absolute local wall-clock
 */
export function parseAlarm(token: string): AlarmTrigger | null {
  const s = token.trim();
  if (s.startsWith("@")) {
    const at = new Date(s.slice(1));
    return Number.isNaN(at.getTime()) ? null : { At: at.toISOString() };
  }
  const end = /^end\s*([+-])\s*(.+)$/i.exec(s);
  if (end) {
    const min = offsetToMinutes(end[2]);
    return min === null ? null : { MinutesBeforeEnd: end[1] === "-" ? min : -min };
  }
  if (s.startsWith("-")) {
    const min = offsetToMinutes(s.slice(1));
    return min === null ? null : { MinutesAfterStart: min };
  }
  const min = offsetToMinutes(s);
  return min === null ? null : { MinutesBefore: min };
}

/** Render a trigger back into the grammar [`parseAlarm`] accepts. */
export function alarmToText(tr: AlarmTrigger): string {
  if ("At" in tr) {
    const d = new Date(tr.At);
    const p = (n: number) => String(n).padStart(2, "0");
    return `@${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
  }
  if ("MinutesBeforeEnd" in tr) {
    const m = tr.MinutesBeforeEnd;
    return m >= 0 ? `end-${minutesToOffset(m)}` : `end+${minutesToOffset(-m)}`;
  }
  if ("MinutesAfterStart" in tr) return `-${minutesToOffset(tr.MinutesAfterStart)}`;
  return minutesToOffset(tr.MinutesBefore);
}

/** Comma-separated alarm list → triggers (null when any token is malformed). */
export function parseAlarmList(s: string): AlarmTrigger[] | null {
  const out: AlarmTrigger[] = [];
  for (const part of s.split(",").map((x) => x.trim()).filter(Boolean)) {
    const tr = parseAlarm(part);
    if (tr === null) return null;
    out.push(tr);
  }
  return out;
}

export function alarmsToText(alarms?: Alarm[]): string {
  return (alarms ?? []).map((a) => alarmToText(a.trigger)).join(", ");
}

/** When an alarm fires for an event (mirrors `mgmt_domain::Alarm::fire_at`). */
export function alarmFireAt(tr: AlarmTrigger, ev: EventItem): number {
  if ("At" in tr) return Date.parse(tr.At);
  if ("MinutesBeforeEnd" in tr) return Date.parse(ev.end) - tr.MinutesBeforeEnd * 60_000;
  if ("MinutesAfterStart" in tr) return Date.parse(ev.start) + tr.MinutesAfterStart * 60_000;
  return Date.parse(ev.start) - tr.MinutesBefore * 60_000;
}

interface Due {
  key: string;
  at: number;
  title: string;
  body: string;
}

function eventReminders(ev: EventItem): Due[] {
  if (Number.isNaN(Date.parse(ev.start)) || ev.status === "Cancelled") return [];
  return (ev.alarms ?? [])
    .map((a: Alarm) => alarmFireAt(a.trigger, ev))
    .filter((at) => !Number.isNaN(at))
    .map((at) => ({ key: `${ev.uid}@${at}`, at, title: ev.summary, body: hhmm(ev.start) }));
}

function taskReminders(task: Task): Due[] {
  if (!task.due) return [];
  const due = Date.parse(task.due);
  if (Number.isNaN(due)) return [];
  return (task.reminders ?? [])
    .map(offsetMs)
    .filter((ms): ms is number => ms !== null)
    .map((ms) => ({
      key: `${task.uid}@${due - ms}`,
      at: due - ms,
      title: task.title,
      body: `${t("due")} ${hhmm(task.due!)}`,
    }));
}

// ---- the notifier loop -----------------------------------------------------------------

let timer: ReturnType<typeof setInterval> | null = null;
let pomodoroTimeout: ReturnType<typeof setTimeout> | null = null;
let agendaCache: { due: Due[]; at: number } | null = null;

async function dueSoon(now: number): Promise<Due[]> {
  if (agendaCache && now - agendaCache.at < AGENDA_TTL_MS) return agendaCache.due;
  const ag = await api.agenda(new Date(now - LATE_MS).toISOString(), new Date(now + 86400_000 * 2).toISOString());
  const due = [...ag.events.flatMap(eventReminders), ...ag.tasks.flatMap(taskReminders)];
  agendaCache = { due, at: now };
  return due;
}

async function tick() {
  if (!notificationsGranted()) return;
  const now = Date.now();

  try {
    for (const d of await dueSoon(now)) {
      if (d.at <= now && d.at > now - LATE_MS) fireOnce(d.key, d.title, d.body);
    }
  } catch {
    /* server unreachable — the offline banner handles messaging */
  }

  // Pomodoro: arm a precise timeout for the running phase's end.
  try {
    const w: PomodoroWire = await api.status();
    const p = w.pomodoro;
    if (pomodoroTimeout) {
      clearTimeout(pomodoroTimeout);
      pomodoroTimeout = null;
    }
    if (p?.running && p.ends_at !== undefined) {
      const { phase, ends_at } = p;
      const msg = phase === "focus" ? t("Focus finished — time for a break") : t("Break finished — back to focus");
      const delay = ends_at * 1000 - now;
      if (delay > -LATE_MS) {
        pomodoroTimeout = setTimeout(() => fireOnce(`pomodoro@${ends_at}`, msg), Math.max(0, delay));
      }
    }
  } catch {
    /* ignore */
  }
}

/** Notify immediately when a local mutation changed the agenda (drops the cache). */
export function invalidateNotifier() {
  agendaCache = null;
}

/** Start (or stop) the notifier per the user's setting. Safe to call repeatedly. */
export function setNotifierEnabled(on: boolean) {
  if (on && !timer && notificationsGranted()) {
    timer = setInterval(() => void tick(), TICK_MS);
    void tick();
  } else if (!on && timer) {
    clearInterval(timer);
    timer = null;
    if (pomodoroTimeout) {
      clearTimeout(pomodoroTimeout);
      pomodoroTimeout = null;
    }
  }
}
