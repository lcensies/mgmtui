// Tasks: smart-view + project sidebar, undone/done panes, sort cycle, multi-select bulk ops, forms.

import { useEffect, useRef } from "preact/hooks";
import { api, type Task } from "../api";
import { mutate, resource, showToast } from "../lib/cache";
import { projectColor } from "../lib/colors";
import { t } from "../lib/i18n";
import { fmtDate } from "../lib/time";
import { meta } from "../state/meta";
import { justCreated, NO_PROJECT, openModal, projectScope, inheritedProject, scopeParam, searchText, setScope, taskSort, taskView, toggleScope } from "../state/ui";
import { moveProject, orderProjects } from "../lib/projectorder";
import { saveSettings, settings } from "../state/settings";
import * as sel from "../lib/selection";
import { startDrag } from "../lib/drag";
import { prioMark } from "./Board";
import { BulkBar } from "../components/BulkBar";
import { signal } from "@preact/signals";

/** Sidebar row currently under a reorder drag (drives the drop highlight). */
const dragOverProject = signal<string | null>(null);

/** Which project row sits under a viewport point. */
function projectAt(x: number, y: number): string | null {
  const el = document.elementFromPoint(x, y);
  return (el?.closest("[data-project]") as HTMLElement | null)?.dataset.project ?? null;
}

export function Tasks() {
  const view = taskView.value;
  const sort = taskSort.value;
  const views = meta.value?.views ?? [{ id: "today", label: "Today" }];
  const sorts = meta.value?.sorts ?? [{ id: "due", label: "due date" }];
  const scope = projectScope.value;
  const projects = scopeParam();
  const text = searchText.value.trim() || undefined;

  const key = `tasks:${view}:${projects ?? ""}:${sort}:${text ?? ""}`;
  const res = resource<Task[]>(key, () => api.tasks({ view, projects, sort, text }));
  const all = res.data.value ?? [];

  const kindOf = (t: Task) => meta.value?.statuses.find((s) => s.id === t.status)?.kind ?? "open";
  const undone = all.filter((t) => kindOf(t) !== "done" && kindOf(t) !== "cancelled");
  const done = all.filter((t) => kindOf(t) === "done" || kindOf(t) === "cancelled");
  const orderedIds = [...undone, ...done].map((t) => t.uid);

  // A task created while a narrow smart view is active (e.g. "Today" + no due date) would just
  // vanish — which reads as "the list didn't refresh". Say where it went instead.
  const reported = useRef<string | null>(null);
  const created = justCreated.value;
  useEffect(() => {
    if (!created || res.loading.value || reported.current === created) return;
    reported.current = created;
    if (!all.some((t) => t.uid === created)) {
      showToast(t("Task added — not in this view. Check “All”."));
    }
  }, [created, all, res.loading.value]);

  function cycleSort() {
    const i = sorts.findIndex((s) => s.id === sort);
    taskSort.value = sorts[(i + 1) % sorts.length].id;
  }

  // Sidebar order is a saved preference; drag a row onto another to rearrange.
  const ordered = orderProjects(meta.value?.projects ?? [], settings.value.projectOrder);
  function reorder(name: string, target: string) {
    const next = moveProject(ordered.map((p) => p.name), name, target);
    if (next !== undefined) saveSettings({ projectOrder: next });
  }
  function rowDown(e: PointerEvent, name: string) {
    startDrag(e, {
      onMove: (_dx, _dy, ev) => { dragOverProject.value = projectAt(ev.clientX, ev.clientY); },
      onEnd: (_dx, _dy, ev) => {
        const target = projectAt(ev.clientX, ev.clientY);
        dragOverProject.value = null;
        if (target && target !== name) reorder(name, target);
      },
      onTap: () => toggleScope(name),
    });
  }

  const newTask = () => openModal({ kind: "taskForm", prefill: { project: inheritedProject() } });

  return (
    <div class="tasks-layout">
      <aside class="sidebar">
        <div class="side-group">
          {views.map((v) => (
            <button class={`side-row ${v.id === view ? "active" : ""}`} onClick={() => (taskView.value = v.id)}>
              {v.label}
            </button>
          ))}
        </div>
        <div class="side-group">
          {/* Empty scope = everything; each row toggles its project in or out of the set. */}
          <button class={`side-row ${scope.length === 0 ? "active" : ""}`} onClick={() => setScope([])}>
            {t("All projects")}
          </button>
          {ordered.map((p) => (
            <button
              key={p.name}
              class={`side-row draggable ${scope.includes(p.name) ? "active" : ""} ${dragOverProject.value === p.name ? "over" : ""}`}
              data-project={p.name}
              title={t("Reorder projects")}
              style={{ touchAction: "none" }}
              onPointerDown={(e) => rowDown(e, p.name)}
            >
              <span style={{ color: projectColor(p.name, meta.value) }}>●</span> {p.name}
              {p.open !== undefined && <span class="side-count">{p.open}</span>}
            </button>
          ))}
          <button
            class={`side-row ${scope.includes(NO_PROJECT) ? "active" : ""}`}
            onClick={() => toggleScope(NO_PROJECT)}
          >
            <span class="muted">○</span> {t("No project")}
            {meta.value?.no_project_open !== undefined && <span class="side-count">{meta.value.no_project_open}</span>}
          </button>
          {scope.length > 0 && (
            <button class="side-row" onClick={() => setScope([])}>✕ {t("Clear filter")}</button>
          )}
        </div>
      </aside>

      <div class="list">
        <div class="row" style={{ marginBottom: "8px", gap: "8px" }}>
          <input
            class="search grow"
            type="search"
            placeholder={t("Search tasks…")}
            value={searchText.value}
            onInput={(e) => (searchText.value = (e.target as HTMLInputElement).value)}
          />
          <button class="icon" title={t("Sort")} onClick={cycleSort}>
            ↓ {sorts.find((s) => s.id === sort)?.label ?? sort}
          </button>
          <button class="primary" onClick={newTask}>+ {t("New task")}</button>
        </div>
        <div class="muted" style={{ marginBottom: "6px", fontSize: "12px" }}>{undone.length} {t("open")}</div>

        {res.error.value && <div class="error">{res.error.value}</div>}

        {undone.map((t) => (
          <TaskRow key={t.uid} task={t} kind={kindOf(t)} />
        ))}

        {done.length > 0 && <div class="day-head">{t("Done")}</div>}
        {done.map((t) => (
          <TaskRow key={t.uid} task={t} kind={kindOf(t)} />
        ))}

        {all.length === 0 && !res.error.value && (
          <div class="empty">
            <div class="muted">{t("No tasks in this view.")}</div>
            <button class="primary" onClick={newTask}>+ {t("New task")}</button>
          </div>
        )}
      </div>

      <BulkBar orderedIds={orderedIds} />
    </div>
  );
}

function TaskRow({ task, kind }: { task: Task; kind: string }) {
  const done = kind === "done" || kind === "cancelled";
  const selectedRow = sel.isSelected(task.uid);
  const overdue = task.due && new Date(task.due) < new Date() && !done;
  const fresh = justCreated.value === task.uid;

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
    <div
      class={`card ${done ? "done" : ""} ${selectedRow ? "sel" : ""} ${fresh ? "fresh" : ""}`}
      ref={(el) => {
        if (fresh) el?.scrollIntoView({ block: "nearest" });
      }}
    >
      <div class="row">
        <input type="checkbox" title={t("Done")} checked={done} style={{ width: "auto" }} onChange={toggle} />
        <span class="title grow" style={{ touchAction: "pan-y" }} onPointerDown={titleDown}>
          {task.title}
        </span>
      </div>
      <div class="meta">
        {task.project && (
          <span class="pill" style={{ color: projectColor(task.project, meta.value) }}>#{task.project}</span>
        )}
        {task.priority !== "None" && <span class={`prio-${task.priority}`}>{prioMark(task.priority)}</span>}
        {task.due && <span style={overdue ? { color: "var(--red)" } : undefined}>{t("due")} {fmtDate(new Date(task.due), { day: "numeric", month: "short", year: "numeric" })}</span>}
        {(task.tags ?? []).map((tag) => <span class="pill muted">{tag}</span>)}
      </div>
    </div>
  );
}
