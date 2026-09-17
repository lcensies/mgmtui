// Compact month: the sidebar navigator (with ‹ › header) and one cell of the year view.

import type { EventItem } from "../../api";
import { addDays, fmtDate, now as nowSig, sameDay, startOfMonthGrid, startOfWeek, ymd } from "../../lib/time";

/** Days that carry at least one event, keyed by local ymd.
 *  ponytail: a multi-day event only marks its start day — enough for a dot; per-day membership
 *  would mean running eventsOnDay over 42 cells × 12 months in the year view. */
export function eventDays(events: EventItem[]): Set<string> {
  return new Set(events.map((e) => ymd(new Date(e.start))));
}

export function MiniMonth({
  month,
  marks,
  selected,
  onDay,
  onShift,
}: {
  month: Date;
  marks?: Set<string>;
  selected?: Date;
  onDay: (d: Date) => void;
  onShift?: (dir: number) => void;
}) {
  const first = startOfWeek(new Date());
  const start = startOfMonthGrid(month);
  const now = nowSig.value;
  const cells = Array.from({ length: 42 }, (_, i) => addDays(start, i));

  return (
    <div class="mini">
      <div class="mini-head">
        {onShift && <button class="icon" aria-label="Previous month" onClick={() => onShift(-1)}>‹</button>}
        <strong>{fmtDate(month, { month: "long", year: "numeric" })}</strong>
        {onShift && <button class="icon" aria-label="Next month" onClick={() => onShift(1)}>›</button>}
      </div>
      <div class="mini-grid">
        {Array.from({ length: 7 }, (_, i) => (
          <div class="mini-dow">{fmtDate(addDays(first, i), { weekday: "narrow" })}</div>
        ))}
        {cells.map((d) => (
          <div
            class={`mini-day ${d.getMonth() === month.getMonth() ? "" : "adj"} ${sameDay(d, selected ?? now) ? "today" : ""} ${marks?.has(ymd(d)) ? "has" : ""}`}
            onClick={() => onDay(d)}
          >
            {d.getDate()}
          </div>
        ))}
      </div>
    </div>
  );
}
