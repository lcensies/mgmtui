// Fuzzy command palette (`:`), porting the TUI's command set: navigation, creation, history, trash,
// and selection ops (mark done / delete / set due / set priority).

import { useState } from "preact/hooks";
import { useLocation } from "preact-iso";
import { api, type Priority, type Task } from "../../api";
import { invalidate, mutate, showToast } from "../../lib/cache";
import { fuzzyFilter } from "../../lib/fuzzy";
import { addDays, startOfToday } from "../../lib/time";
import { clearSelection, closeModal, openModal, selected } from "../../state/ui";
import { doRedo, doUndo } from "../../app";
import { Overlay } from "./ModalHost";

interface Cmd {
  id: string;
  label: string;
  run: () => void;
}

function dueDate(kind: string): Date {
  const base = startOfToday();
  if (kind === "today") return base;
  if (kind === "tomorrow") return addDays(base, 1);
  if (kind === "eow") return addDays(base, (5 - (base.getUTCDay() || 7) + 7) % 7 || 0); // Friday
  if (kind === "nw") return addDays(base, ((8 - (base.getUTCDay() || 7)) % 7) || 7); // next Monday
  const days = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
  const target = days.indexOf(kind);
  if (target >= 0) {
    const cur = base.getUTCDay();
    return addDays(base, ((target - cur + 7) % 7) || 7);
  }
  return base;
}

async function editSelected(edit: (t: Task) => Task) {
  const ids = [...selected.value];
  if (!ids.length) {
    showToast("no tasks selected");
    return;
  }
  await mutate({
    request: async () => {
      for (const id of ids) {
        const t = await api.task(id);
        await api.updateTask(edit(t));
      }
    },
    after: ["tasks", "board", "agenda"],
  });
  clearSelection();
}

async function toggleSelected() {
  const ids = [...selected.value];
  if (!ids.length) return showToast("no tasks selected");
  await mutate({ request: () => Promise.all(ids.map((id) => api.toggle(id))), after: ["tasks", "board", "agenda"] });
  clearSelection();
}
async function deleteSelected() {
  const ids = [...selected.value];
  if (!ids.length) return showToast("no tasks selected");
  await mutate({ request: () => Promise.all(ids.map((id) => api.deleteTask(id))), after: ["tasks", "board", "agenda", "trash"] });
  clearSelection();
}

export function CommandPalette() {
  const loc = useLocation();
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState(0);

  const setDue = (kind: string) => editSelected((t) => ({ ...t, due: kind === "none" ? undefined : iso(dueDate(kind)) }));
  const setPrio = (p: Priority) => editSelected((t) => ({ ...t, priority: p }));

  const go = (path: string) => { loc.route(path); closeModal(); };

  const commands: Cmd[] = [
    { id: "cal", label: "Go to Calendar", run: () => go("/") },
    { id: "board", label: "Go to Board", run: () => go("/board") },
    { id: "tasks", label: "Go to Tasks", run: () => go("/tasks") },
    { id: "focus", label: "Go to Focus", run: () => go("/focus") },
    { id: "new-task", label: "New task", run: () => openModal({ kind: "taskForm" }) },
    { id: "new-event", label: "New event", run: () => openModal({ kind: "eventForm" }) },
    { id: "done", label: "Mark selected done", run: () => { toggleSelected(); closeModal(); } },
    { id: "delete", label: "Delete selected", run: () => { deleteSelected(); closeModal(); } },
    { id: "due-today", label: "Due: today", run: () => { setDue("today"); closeModal(); } },
    { id: "due-tomorrow", label: "Due: tomorrow", run: () => { setDue("tomorrow"); closeModal(); } },
    { id: "due-eow", label: "Due: end of week", run: () => { setDue("eow"); closeModal(); } },
    { id: "due-nw", label: "Due: next week", run: () => { setDue("nw"); closeModal(); } },
    { id: "due-none", label: "Due: clear", run: () => { setDue("none"); closeModal(); } },
    { id: "prio-high", label: "Priority: high", run: () => { setPrio("High"); closeModal(); } },
    { id: "prio-medium", label: "Priority: medium", run: () => { setPrio("Medium"); closeModal(); } },
    { id: "prio-low", label: "Priority: low", run: () => { setPrio("Low"); closeModal(); } },
    { id: "prio-none", label: "Priority: none", run: () => { setPrio("None"); closeModal(); } },
    { id: "undo", label: "Undo", run: () => { doUndo(); closeModal(); } },
    { id: "redo", label: "Redo", run: () => { doRedo(); closeModal(); } },
    { id: "trash", label: "Open trash", run: () => openModal({ kind: "trash" }) },
    { id: "empty-trash", label: "Empty trash", run: () => { api.trashEmpty().then(() => invalidate("all")); closeModal(); } },
    { id: "help", label: "Keyboard help", run: () => openModal({ kind: "help" }) },
  ];

  const matches = fuzzyFilter(query, commands, (c) => c.label);
  const active = Math.min(sel, Math.max(0, matches.length - 1));

  function onKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => Math.min(s + 1, matches.length - 1)); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => Math.max(s - 1, 0)); }
    else if (e.key === "Enter") { e.preventDefault(); matches[active]?.run(); }
    else if (e.key === "Escape") closeModal();
  }

  return (
    <Overlay>
      <input
        autofocus
        placeholder="Type a command…"
        value={query}
        onInput={(e) => { setQuery((e.target as HTMLInputElement).value); setSel(0); }}
        onKeyDown={onKey}
      />
      <div style={{ maxHeight: "50vh", overflow: "auto" }}>
        {matches.map((c, i) => (
          <button class={`pickrow ${i === active ? "on" : ""}`} onClick={c.run}>
            {c.label}
          </button>
        ))}
        {matches.length === 0 && <div class="muted">no matching command</div>}
      </div>
    </Overlay>
  );
}

function iso(d: Date): string {
  return new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), 23, 59)).toISOString();
}
