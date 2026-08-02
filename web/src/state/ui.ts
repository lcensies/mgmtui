// Cross-view UI state: project scope, search text, the active modal, and the multi-select set.

import { signal } from "@preact/signals";
import type { EventItem, Task } from "../api";

/** Project scope shared by Tasks/Board/Calendar. Empty = all projects; the literal `NO_PROJECT`
 *  entry selects tasks that have no project. Persisted per device. */
export const NO_PROJECT = "none";
const SCOPE_KEY = "mgmt:projectScope";

function loadScope(): string[] {
  try {
    const raw = localStorage.getItem(SCOPE_KEY);
    const v = raw ? JSON.parse(raw) : null;
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

export const projectScope = signal<string[]>(loadScope());

export function setScope(next: string[]) {
  projectScope.value = next;
  try {
    localStorage.setItem(SCOPE_KEY, JSON.stringify(next));
  } catch {
    /* private mode — scope just won't survive a reload */
  }
}

/** Add/remove one project from the scope. */
export function toggleScope(name: string) {
  const cur = projectScope.value;
  setScope(cur.includes(name) ? cur.filter((x) => x !== name) : [...cur, name]);
}

/** The `projects=` query value for the active scope (undefined = no constraint). */
export function scopeParam(): string | undefined {
  return projectScope.value.length ? projectScope.value.join(",") : undefined;
}

/** The single project the scope currently names, if it names exactly one. */
export function scopedProject(): string | undefined {
  const s = projectScope.value;
  return s.length === 1 && s[0] !== NO_PROJECT ? s[0] : undefined;
}

/** Quick-add's "file new tasks under the scoped project" toggle. Device-local, like the scope
 *  it follows. On by default so scoped work stays filed without extra clicks. */
const PIN_KEY = "mgmt:pinScope";
export const pinScope = signal<boolean>(localStorage.getItem(PIN_KEY) !== "0");

export function setPinScope(on: boolean) {
  pinScope.value = on;
  try {
    localStorage.setItem(PIN_KEY, on ? "1" : "0");
  } catch {
    /* private mode — the toggle just won't survive a reload */
  }
}

/** The project a newly created task should inherit (honors the toggle). */
export function inheritedProject(): string | undefined {
  return pinScope.value ? scopedProject() : undefined;
}

/** Active search/filter text (Tasks + calendar event search). */
export const searchText = signal<string>("");

/** Tasks-view smart list + sort. Signals (not component state) so they survive tab switches. */
export const taskView = signal<string>("today");
export const taskSort = signal<string>("due");

/** Uid of the most recently created task — drives a transient highlight in the lists. */
export const justCreated = signal<string | null>(null);
export function flashCreated(uid: string) {
  justCreated.value = uid;
  setTimeout(() => {
    if (justCreated.value === uid) justCreated.value = null;
  }, 2500);
}

/** The modal currently open (null = none). A back stack is a single slot for now. */
export type Modal =
  | { kind: "taskForm"; task?: Task; prefill?: Partial<Task> }
  | { kind: "eventForm"; event?: EventItem; date?: string; end?: string }
  | { kind: "projectPicker"; taskUids: string[] }
  | { kind: "palette" }
  | { kind: "trash" }
  | { kind: "help" }
  | { kind: "settings" }
  | { kind: "confirm"; message: string; onConfirm: () => void };

export const modal = signal<Modal | null>(null);
export const openModal = (m: Modal) => (modal.value = m);
export const closeModal = () => (modal.value = null);

/** Selected task uids for bulk (visual/multi-select) operations. */
export const selected = signal<Set<string>>(new Set());
export const clearSelection = () => (selected.value = new Set());
