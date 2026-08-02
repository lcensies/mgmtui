// Kanban board with pointer drag-and-drop between columns (mouse + touch), multi-select, and colors.
// On mobile the columns scroll-snap one per swipe, with a dot indicator ("pivots").

import { signal } from "@preact/signals";
import { useRef, useState } from "preact/hooks";
import { api, type Column, type Task } from "../api";
import { mutate, resource } from "../lib/cache";
import { startDrag } from "../lib/drag";
import { projectColor, statusColor } from "../lib/colors";
import { meta } from "../state/meta";
import { justCreated, openModal, scopeParam, scopedProject, selected } from "../state/ui";
import * as sel from "../lib/selection";
import { t } from "../lib/i18n";
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
  const projects = scopeParam();
  const res = resource<Column[]>(`board:${projects ?? ""}`, () => api.board(projects));
  const cols = res.data.value ?? [];
  const allIds = cols.flatMap((c) => c.tasks.map((t) => t.uid));

  // Which column is centered in the (mobile) snap scroller — drives the dot indicator.
  const scroller = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState(0);
  function onScroll() {
    const el = scroller.current;
    if (!el || el.scrollWidth <= el.clientWidth) return;
    const kids = Array.from(el.querySelectorAll<HTMLElement>("[data-status]"));
    const mid = el.scrollLeft + el.clientWidth / 2;
    let best = 0;
    kids.forEach((k, i) => {
      if (Math.abs(k.offsetLeft + k.offsetWidth / 2 - mid) < Math.abs(kids[best].offsetLeft + kids[best].offsetWidth / 2 - mid)) best = i;
    });
    setActive(best);
  }
  function jumpTo(i: number) {
    const el = scroller.current;
    const k = el?.querySelectorAll<HTMLElement>("[data-status]")[i];
    if (el && k) el.scrollTo({ left: k.offsetLeft + k.offsetWidth / 2 - el.clientWidth / 2, behavior: "smooth" });
  }

  async function moveTo(uid: string, status: string) {
    const prev = res.data.value;
    await mutate({
      // Optimistic: move the card into its target column immediately, roll back on failure.
      patch: () => {
        const cur = res.data.value;
        if (!cur) return;
        let moved: Task | undefined;
        const stripped = cur.map((c) => {
          const found = c.tasks.find((t) => t.uid === uid);
          if (found) moved = { ...found, status };
          return { ...c, tasks: c.tasks.filter((t) => t.uid !== uid) };
        });
        if (!moved) return;
        res.data.value = stripped.map((c) => (c.status === status ? { ...c, tasks: [...c.tasks, moved!] } : c));
      },
      rollback: () => { res.data.value = prev; },
      request: () => api.setStatus(uid, status),
      after: ["board", "tasks", "agenda"],
    });
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
      <div class="board-dots">
        {cols.map((c, i) => (
          <span key={c.status} class={`dot ${i === active ? "on" : ""}`} onClick={() => jumpTo(i)} />
        ))}
      </div>
      <div class="board" ref={scroller} onScroll={onScroll}>
        {cols.map((col) => {
          const color = statusColor(col.status, meta.value);
          return (
            <div class={`col ${g?.over === col.status ? "over" : ""}`} data-status={col.status} key={col.status}>
              <h3 style={{ color }}>
                {col.label} · {col.tasks.length}
                <button
                  class="icon col-add"
                  title={t("New task")}
                  aria-label={t("New task")}
                  onClick={() => openModal({ kind: "taskForm", prefill: { status: col.status, project: scopedProject() } })}
                >
                  +
                </button>
              </h3>
              {col.tasks.map((t) => (
                <div
                  class={`card ${sel.isSelected(t.uid) ? "sel" : ""} ${g?.uid === t.uid ? "dragging" : ""} ${justCreated.value === t.uid ? "fresh" : ""}`}
                  key={t.uid}
                  style={{ borderLeftColor: color, touchAction: "pan-y" }}
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
