// Cross-view UI state: project scope, search text, the active modal, and the multi-select set.

import { signal } from "@preact/signals";
import type { EventItem, Task } from "../api";

/** `[`/`]` project scope (and Tasks sidebar selection). null = all projects. */
export const projectScope = signal<string | null>(null);

/** Active search/filter text (Tasks + calendar event search). */
export const searchText = signal<string>("");

/** The modal currently open (null = none). A back stack is a single slot for now. */
export type Modal =
  | { kind: "taskForm"; task?: Task }
  | { kind: "eventForm"; event?: EventItem; date?: string; end?: string }
  | { kind: "projectPicker"; taskUids: string[] }
  | { kind: "eventProjectPicker"; eventUid: string }
  | { kind: "palette" }
  | { kind: "trash" }
  | { kind: "help" }
  | { kind: "settings" }
  | { kind: "confirm"; message: string; onConfirm: () => void }
  | { kind: "jumpDate" };

export const modal = signal<Modal | null>(null);
export const openModal = (m: Modal) => (modal.value = m);
export const closeModal = () => (modal.value = null);

/** Selected task uids for bulk (visual/multi-select) operations. */
export const selected = signal<Set<string>>(new Set());
export const clearSelection = () => (selected.value = new Set());
