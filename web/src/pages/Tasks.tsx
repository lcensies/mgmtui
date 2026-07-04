// Tasks: smart-view + project sidebar, undone/done panes, sort cycle, multi-select bulk ops, forms.

import { useState } from "preact/hooks";
import { api, type Task } from "../api";
import { mutate, resource } from "../lib/cache";
import { projectColor } from "../lib/colors";
import { meta } from "../state/meta";
import { projectScope, openModal, searchText } from "../state/ui";
import * as sel from "../lib/selection";
import { startDrag } from "../lib/drag";
import { prioMark } from "./Board";
import { BulkBar } from "../components/BulkBar";

export function Tasks() {
  const [view, setView] = useState("today");
  const [sort, setSort] = useState("due");
  const views = meta.value?.views ?? [{ id: "today", label: "Today" }];
  const sorts = meta.value?.sorts ?? [{ id: "due", label: "due date" }];
  const project = projectScope.value ?? undefined;
  const text = searchText.value.trim() || undefined;

  const key = `tasks:${view}:${project ?? ""}:${sort}:${text ?? ""}`;
  const res = resource<Task[]>(key, () => api.tasks({ view, project, sort, text }));
  const all = res.data.value ?? [];

  const kindOf = (t: Task) => meta.value?.statuses.find((s) => s.id === t.status)?.kind ?? "open";
  const undone = all.filter((t) => kindOf(t) !== "done" && kindOf(t) !== "cancelled");
  const done = all.filter((t) => kindOf(t) === "done" || kindOf(t) === "cancelled");
  const orderedIds = [...undone, ...done].map((t) => t.uid);

  function cycleSort() {
    const i = sorts.findIndex((s) => s.id === sort);
    setSort(sorts[(i + 1) % sorts.length].id);
  }

  return (
    <div class="tasks-layout">
      <aside class="sidebar">
        <div class="side-group">
          {views.map((v) => (
            <button class={`side-row ${v.id === view ? "active" : ""}`} onClick={() => setView(v.id)}>
              {v.label}
            </button>
          ))}
        </div>
        <div class="side-group">
          <button class={`side-row ${!project ? "active" : ""}`} onClick={() => (projectScope.value = null)}>
            All projects
          </button>
          {(meta.value?.projects ?? []).map((p) => (
            <button
              class={`side-row ${project === p.name ? "active" : ""}`}
              onClick={() => (projectScope.value = p.name)}
            >
              <span style={{ color: projectColor(p.name, meta.value) }}>●</span> {p.name}
            </button>
          ))}
        </div>
      </aside>

      <div class="list">
        <div class="row" style={{ marginBottom: "8px", gap: "8px" }}>
          <input
            class="search grow"
            type="search"
            placeholder="Search tasks…"
            value={searchText.value}
            onInput={(e) => (searchText.value = (e.target as HTMLInputElement).value)}
          />
          <button class="icon" title="Sort" onClick={cycleSort}>
            ↓ {sorts.find((s) => s.id === sort)?.label ?? sort}
          </button>
        </div>
        <div class="muted" style={{ marginBottom: "6px", fontSize: "12px" }}>{undone.length} open</div>

        {res.error.value && <div class="error">{res.error.value}</div>}

        {undone.map((t) => (
          <TaskRow key={t.uid} task={t} kind={kindOf(t)} />
        ))}

        {done.length > 0 && <div class="day-head">Done</div>}
        {done.map((t) => (
          <TaskRow key={t.uid} task={t} kind={kindOf(t)} />
        ))}

        {all.length === 0 && !res.error.value && <div class="muted">No tasks in this view.</div>}
      </div>

      <BulkBar orderedIds={orderedIds} />
    </div>
  );
}

function TaskRow({ task, kind }: { task: Task; kind: string }) {
  const done = kind === "done" || kind === "cancelled";
  const selectedRow = sel.isSelected(task.uid);
  const overdue = task.due && new Date(task.due) < new Date() && !done;

  async function toggle() {
    await mutate({ request: () => api.toggle(task.uid), after: ["tasks", "board", "agenda"] });
  }

  // Leftmost checkbox = done (the universal expectation). Tap the title to edit; long-press it
  // (or tap while a selection is active) to multi-select.
  function titleDown(e: PointerEvent) {
    startDrag(e, {
      onTap: () => (sel.count() > 0 ? sel.toggle(task.uid) : openModal({ kind: "taskForm", task })),
      onLongPress: () => sel.toggle(task.uid),
    });
  }

  return (
    <div class={`card ${done ? "done" : ""} ${selectedRow ? "sel" : ""}`}>
      <div class="row">
        <input type="checkbox" title="Done" checked={done} style={{ width: "auto" }} onChange={toggle} />
        <span class="title grow" style={{ touchAction: "none" }} onPointerDown={titleDown}>
          {task.title}
        </span>
      </div>
      <div class="meta">
        {task.project && (
          <span class="pill" style={{ color: projectColor(task.project, meta.value) }}>#{task.project}</span>
        )}
        {task.priority !== "None" && <span class={`prio-${task.priority}`}>{prioMark(task.priority)}</span>}
        {task.due && <span style={overdue ? { color: "var(--red)" } : undefined}>due {new Date(task.due).toLocaleDateString()}</span>}
      </div>
    </div>
  );
}
