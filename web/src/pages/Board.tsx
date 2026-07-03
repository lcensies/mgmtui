// Kanban board with pointer drag-and-drop between columns (mouse + touch), multi-select, and colors.

import { signal } from "@preact/signals";
import { api, type Column } from "../api";
import { mutate, resource } from "../lib/cache";
import { startDrag } from "../lib/drag";
import { projectColor, statusColor } from "../lib/colors";
import { meta } from "../state/meta";
import { openModal, selected } from "../state/ui";
import * as sel from "../lib/selection";
import { BulkBar } from "../components/BulkBar";

interface Ghost {
  uid: string;
  title: string;
  x: number;
  y: number;
  over: string | null;
}
const ghost = signal<Ghost | null>(null);

export function prioMark(p: string): string {
  return p === "High" ? "!!!" : p === "Medium" ? "!!" : p === "Low" ? "!" : "";
}

/** Column id under the given viewport point (via a data-status hit-test). */
function columnAt(x: number, y: number): string | null {
  const el = document.elementFromPoint(x, y);
  const col = el?.closest("[data-status]") as HTMLElement | null;
  return col?.dataset.status ?? null;
}

export function Board() {
  const res = resource<Column[]>("board:", () => api.board());
  const cols = res.data.value ?? [];
  const allIds = cols.flatMap((c) => c.tasks.map((t) => t.uid));

  async function moveTo(uid: string, status: string) {
    await mutate({ request: () => api.setStatus(uid, status), after: ["board", "tasks", "agenda"] });
  }

  function onCardDown(e: PointerEvent, uid: string, title: string, status: string) {
    startDrag(e, {
      onStart: () => (ghost.value = { uid, title, x: e.clientX, y: e.clientY, over: status }),
      onMove: (_dx, _dy, ev) => {
        if (ghost.value) ghost.value = { ...ghost.value, x: ev.clientX, y: ev.clientY, over: columnAt(ev.clientX, ev.clientY) };
      },
      onEnd: (_dx, _dy, ev) => {
        const target = columnAt(ev.clientX, ev.clientY);
        ghost.value = null;
        if (target && target !== status) void moveTo(uid, target);
      },
      onTap: () => {
        if (selected.value.size > 0) sel.toggle(uid);
        else {
          const t = cols.flatMap((c) => c.tasks).find((x) => x.uid === uid);
          if (t) openModal({ kind: "taskForm", task: t });
        }
      },
      onLongPress: () => sel.toggle(uid),
    });
  }

  const g = ghost.value;

  return (
    <div>
      {res.error.value && <div class="error">{res.error.value}</div>}
      <div class="board">
        {cols.map((col) => {
          const color = statusColor(col.status, meta.value);
          return (
            <div class={`col ${g?.over === col.status ? "over" : ""}`} data-status={col.status} key={col.status}>
              <h3 style={{ color }}>
                {col.label} · {col.tasks.length}
              </h3>
              {col.tasks.map((t) => (
                <div
                  class={`card ${sel.isSelected(t.uid) ? "sel" : ""} ${g?.uid === t.uid ? "dragging" : ""}`}
                  key={t.uid}
                  style={{ borderLeftColor: color, touchAction: "none" }}
                  onPointerDown={(e) => onCardDown(e, t.uid, t.title, col.status)}
                >
                  <div class="title">{t.title}</div>
                  <div class="meta row">
                    {t.project && <span class="pill" style={{ color: projectColor(t.project, meta.value) }}>#{t.project}</span>}
                    {t.priority !== "None" && <span class={`prio-${t.priority}`}>{prioMark(t.priority)}</span>}
                  </div>
                </div>
              ))}
            </div>
          );
        })}
      </div>

      {g && (
        <div class="drag-ghost" style={{ left: `${g.x}px`, top: `${g.y}px` }}>
          {g.title}
        </div>
      )}

      <BulkBar orderedIds={allIds} />
    </div>
  );
}
