// A positioned, colored event block with pointer drag-to-reschedule (body) and resize (top/bottom
// handles). Snaps to 15 minutes, previews locally, and commits an optimistic PUT on drop. A tap
// (no drag) opens the editor.

import { useState } from "preact/hooks";
import type { EventItem } from "../../api";
import type { Positioned } from "../../lib/cal";
import { contrastText, eventColor } from "../../lib/colors";
import { startDrag } from "../../lib/drag";
import { hhmm, snap15 } from "../../lib/time";
import { meta } from "../../state/meta";

type Zone = "move" | "start" | "end";

export function EventBlock({
  pos,
  lanes,
  pxPerHour,
  label,
  onClick,
  onReschedule,
}: {
  pos: Positioned;
  lanes: number;
  pxPerHour: number;
  day: Date;
  startHour: number;
  label: string;
  onClick: () => void;
  onReschedule?: (ev: EventItem, startISO: string, endISO: string) => void;
}) {
  const bg = eventColor(pos.ev, meta.value);
  const pxPerMin = pxPerHour / 60;
  const [delta, setDelta] = useState<{ top: number; dur: number } | null>(null);

  const baseTop = (pos.topMin / 60) * pxPerHour;
  const baseHeight = Math.max(18, (pos.durMin / 60) * pxPerHour - 2);
  const previewTop = baseTop + (delta ? delta.top : 0) * pxPerMin;
  const previewHeight = Math.max(12, baseHeight + (delta ? delta.dur : 0) * pxPerMin);
  const width = `calc(${100 / lanes}% - 3px)`;
  const left = `calc(${(pos.lane * 100) / lanes}%)`;
  const resizable = baseHeight >= 20 && !!onReschedule;

  function down(e: PointerEvent) {
    if (!onReschedule) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const offY = e.clientY - rect.top;
    const edge = Math.min(10, rect.height / 3); // generous grab zone for the resize handles
    const zone: Zone = resizable && offY < edge ? "start" : resizable && offY > rect.height - edge ? "end" : "move";
    startDrag(e, {
      onMove: (_dx, dy) => {
        const dMin = snap15(dy / pxPerMin);
        if (zone === "move") setDelta({ top: dMin, dur: 0 });
        else if (zone === "start") setDelta({ top: dMin, dur: -dMin });
        else setDelta({ top: 0, dur: dMin });
      },
      onEnd: (_dx, dy) => {
        const dMin = snap15(dy / pxPerMin);
        setDelta(null);
        if (dMin === 0) return;
        const start = new Date(pos.ev.start).getTime();
        const end = new Date(pos.ev.end).getTime();
        let ns = start;
        let ne = end;
        if (zone === "move") { ns = start + dMin * 60000; ne = end + dMin * 60000; }
        else if (zone === "start") { ns = Math.min(start + dMin * 60000, end - 15 * 60000); }
        else { ne = Math.max(end + dMin * 60000, start + 15 * 60000); }
        onReschedule!(pos.ev, new Date(ns).toISOString(), new Date(ne).toISOString());
      },
      onTap: onClick,
    });
  }

  const liveLabel = delta ? `${hhmm(new Date(new Date(pos.ev.start).getTime() + (delta.top) * 60000))} ${pos.ev.summary}` : label;

  return (
    <div
      class={`evblock ${delta ? "dragging" : ""}`}
      style={{ top: `${previewTop}px`, height: `${previewHeight}px`, width, left, background: bg, color: contrastText(bg), touchAction: "none" }}
      onPointerDown={down}
      title={label}
    >
      {resizable && <span class="ev-handle top" />}
      {liveLabel}
      {resizable && <span class="ev-handle bottom" />}
    </div>
  );
}
