// Typed fetch wrapper + full API surface. All calls are same-origin with cookies; a 401 flips
// `authed` off so the app routes to login.

import { signal } from "@preact/signals";

export const authed = signal<boolean>(true);
// True when the server has no admin yet and is waiting for first-run setup.
export const needsSetup = signal<boolean>(false);

// --- domain types (mirror the Rust serde shapes) ---------------------------------------

export type Priority = "None" | "Low" | "Medium" | "High";
export type Weekday = "Mon" | "Tue" | "Wed" | "Thu" | "Fri" | "Sat" | "Sun";
export type Frequency = "Daily" | "Weekly" | "Monthly" | "Yearly";
export type EventStatus = "Confirmed" | "Tentative" | "Cancelled";

export interface RecurrenceRule {
  freq: Frequency;
  interval: number;
  count?: number;
  until?: string; // YYYY-MM-DD
  by_weekday?: Weekday[];
  by_monthday?: number[];
  by_month?: number[];
}

export type AlarmAction = "notify" | "navigate" | { run: { command: string; args: string[] } };
export interface Alarm {
  trigger: { MinutesBefore: number };
  action: AlarmAction;
  description?: string;
}

export interface Task {
  uid: string;
  title: string;
  body?: string;
  status: string;
  priority: Priority;
  project?: string;
  area?: string;
  tags?: string[];
  due?: string;
  scheduled?: string;
  reminders?: string[]; // compact "1d"/"2h"
  completion?: number;
  created?: string;
}

export interface EventItem {
  uid: string;
  calendar: string;
  summary: string;
  description?: string;
  location?: string;
  all_day: boolean;
  start: string;
  end: string;
  project?: string;
  rrule?: RecurrenceRule;
  alarms?: Alarm[];
  status?: EventStatus;
}

export interface Project {
  name: string;
  color?: string;
  description?: string;
}

export interface Column {
  status: string;
  label: string;
  tasks: Task[];
}

export interface Meta {
  data_root: string;
  statuses: { id: string; label: string; kind: string; color: string }[];
  projects: { name: string; color: string }[];
  views: { id: string; label: string }[];
  sorts: { id: string; label: string }[];
  calendar: {
    show_end_time: boolean;
    month_event_lines: number;
    month_panel_style: string;
    event_palette: string[];
  };
  theme: Record<string, string>;
  default_reminders: string[];
  event_alarm_defaults: Alarm[];
}

export interface AppStateInfo {
  dirty: boolean;
  can_undo: boolean;
  can_redo: boolean;
}

export interface AdminUser {
  id: string;
  name: string;
  tokens: string[]; // token labels (never the secrets)
}

export interface PomodoroWire {
  pomodoro?: {
    phase: "focus" | "break";
    running: boolean;
    open: boolean;
    ends_at?: number;
    count_from?: number;
    remaining?: number;
    elapsed?: number;
  };
  next_event?: { summary: string; start: number; all_day: boolean };
}

// --- fetch core ------------------------------------------------------------------------

async function req<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method,
    credentials: "include",
    headers: body !== undefined ? { "content-type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (res.status === 401) {
    authed.value = false;
    throw new Error("unauthorized");
  }
  if (!res.ok) {
    let msg = `HTTP ${res.status}`;
    try {
      msg = (await res.json()).error ?? msg;
    } catch {
      /* ignore */
    }
    throw new Error(msg);
  }
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

const q = (obj: Record<string, string | undefined>) => {
  const p = Object.entries(obj)
    .filter(([, v]) => v !== undefined && v !== "")
    .map(([k, v]) => `${k}=${encodeURIComponent(v as string)}`)
    .join("&");
  return p ? `?${p}` : "";
};

export const api = {
  // auth
  session: () => req<{ enabled: boolean; authenticated: boolean; needs_setup: boolean }>("GET", "/auth/session"),
  login: (password: string, totp?: string) =>
    req<{ ok: boolean }>("POST", "/auth/login", { password, totp: totp || undefined }),
  logout: () => req<{ ok: boolean }>("POST", "/auth/logout"),
  // first-run admin creation (only accepted while no admin exists)
  setup: (password: string, totp_secret?: string) =>
    req<{ ok: boolean }>("POST", "/auth/setup", { password, totp_secret: totp_secret || undefined }),

  // admin: user management (admin session only)
  adminUsers: () => req<{ users: AdminUser[] }>("GET", "/admin/users"),
  adminCreateUser: (id: string, name?: string) => req<{ id: string; name: string }>("POST", "/admin/users", { id, name: name || "" }),
  adminDeleteUser: (id: string) => req<{ ok: boolean }>("DELETE", `/admin/users/${encodeURIComponent(id)}`),
  adminMintToken: (id: string, name?: string) =>
    req<{ name: string; token: string }>("POST", `/admin/users/${encodeURIComponent(id)}/tokens`, { name: name || "" }),
  adminRevokeToken: (id: string, name: string) =>
    req<{ ok: boolean }>("DELETE", `/admin/users/${encodeURIComponent(id)}/tokens/${encodeURIComponent(name)}`),
  adminPairUrl: (id: string, name?: string) =>
    req<{ url: string; token: string }>("POST", `/admin/users/${encodeURIComponent(id)}/pair-url`, { name: name || "" }),

  // reads
  meta: () => req<Meta>("GET", "/meta"),
  state: () => req<AppStateInfo>("GET", "/state"),
  tasks: (params: Record<string, string | undefined> = {}) => req<Task[]>("GET", `/tasks${q(params)}`),
  task: (uid: string) => req<Task>("GET", `/tasks/${encodeURIComponent(uid)}`),
  board: (project?: string) => req<Column[]>("GET", `/board${q({ project })}`),
  agenda: (from: string, to: string) => req<{ events: EventItem[]; tasks: Task[] }>("GET", `/agenda${q({ from, to })}`),
  events: (from: string, to: string) => req<EventItem[]>("GET", `/events${q({ from, to })}`),
  event: (uid: string) => req<EventItem>("GET", `/events/${encodeURIComponent(uid)}`),
  projects: () => req<{ name: string; color: string }[]>("GET", "/projects"),
  trash: () => req<{ tasks: Task[]; projects: Project[]; empty: boolean }>("GET", "/trash"),
  status: () => req<PomodoroWire>("GET", "/status"),

  // task mutations
  createTask: (title: string, project?: string) => req<Task>("POST", "/tasks", { title, project }),
  updateTask: (t: Task) => req<Task>("PUT", `/tasks/${encodeURIComponent(t.uid)}`, t),
  deleteTask: (uid: string) => req<{ ok: boolean }>("DELETE", `/tasks/${encodeURIComponent(uid)}`),
  setStatus: (uid: string, status: string) => req<Task>("POST", `/tasks/${encodeURIComponent(uid)}/status`, { status }),
  toggle: (uid: string) => req<{ status: string }>("POST", `/tasks/${encodeURIComponent(uid)}/toggle`),
  cyclePriority: (uid: string) => req<Task>("POST", `/tasks/${encodeURIComponent(uid)}/priority`),
  setTaskProject: (uid: string, project?: string) =>
    req<Task>("POST", `/tasks/${encodeURIComponent(uid)}/project`, { project }),

  // event mutations
  createEvent: (e: EventItem) => req<EventItem>("POST", "/events", e),
  updateEvent: (e: EventItem) => req<EventItem>("PUT", `/events/${encodeURIComponent(e.uid)}`, e),
  deleteEvent: (uid: string) => req<{ ok: boolean }>("DELETE", `/events/${encodeURIComponent(uid)}`),

  // projects
  createProject: (name: string) => req<{ name: string; color: string }>("POST", "/projects", { name }),
  updateProject: (name: string, edit: { color?: string; description?: string }) =>
    req<{ name: string; color: string }>("PUT", `/projects/${encodeURIComponent(name)}`, edit),
  deleteProject: (name: string) => req<{ ok: boolean }>("DELETE", `/projects/${encodeURIComponent(name)}`),

  // trash
  trashRestore: (kind: "task" | "project", id: string) => req("POST", "/trash/restore", { kind, id }),
  trashPurge: (kind: "task" | "project", id: string) => req("POST", "/trash/purge", { kind, id }),
  trashEmpty: () => req("POST", "/trash/empty"),

  // focus + history
  focus: (action: string, engine?: string) => req<PomodoroWire>("POST", `/focus/${action}${q({ engine })}`),
  undo: () => req<{ undone: boolean }>("POST", "/undo"),
  redo: () => req<{ redone: boolean }>("POST", "/redo"),

  // persisted web settings (opaque JSON blob)
  getSettings: () => req<Record<string, unknown>>("GET", "/settings"),
  putSettings: (s: Record<string, unknown>) => req<Record<string, unknown>>("PUT", "/settings", s),
};
