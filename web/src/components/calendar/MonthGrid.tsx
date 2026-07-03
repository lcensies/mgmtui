// 6-week Monday-first month grid with an ISO week-number gutter and colored event labels/dots.

import type { EventItem } from "../../api";
import { eventsOnDay } from "../../lib/cal";
import { contrastText, eventColor } from "../../lib/colors";
import { addDays, hhmm, isoWeek, sameDay, startOfMonthGrid } from "../../lib/time";
import { meta } from "../../state/meta";

const DOW = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

export function MonthGrid({
  anchor,
  events,
  onDay,
  onEvent,
}: {
  anchor: Date;
  events: EventItem[];
  onDay: (d: Date) => void;
  onEvent: (ev: EventItem) => void;
}) {
  const gridStart = startOfMonthGrid(anchor);
  const month = anchor.getUTCMonth();
  const now = new Date();
  const lines = Math.min(3, meta.value?.calendar?.month_event_lines ?? 0);
  const weeks = Array.from({ length: 6 }, (_, w) => Array.from({ length: 7 }, (_, d) => addDays(gridStart, w * 7 + d)));

  return (
    <div class="month">
      <div class="month-head">
        <div class="wk">Wk</div>
        {DOW.map((d) => (
          <div class="dow">{d}</div>
        ))}
      </div>
      {weeks.map((week) => (
        <div class="month-row">
          <div class="wk">{isoWeek(week[0])}</div>
          {week.map((day) => {
            const evs = eventsOnDay(day, events);
            const inMonth = day.getUTCMonth() === month;
            return (
              <div
                class={`daycell ${inMonth ? "" : "adj"} ${sameDay(day, now) ? "today" : ""}`}
                onClick={() => onDay(day)}
              >
                <div class="daynum">{day.getUTCDate()}</div>
                {lines === 0 ? (
                  evs.length > 0 && <div class="dots">{evs.slice(0, 4).map((e) => (
                    <span class="dot" style={{ background: eventColor(e, meta.value) }} />
                  ))}</div>
                ) : (
                  <div class="daylabels">
                    {evs.slice(0, lines).map((e) => {
                      const bg = eventColor(e, meta.value);
                      return (
                        <div
                          class="daylabel"
                          style={{ background: bg, color: contrastText(bg) }}
                          onClick={(ev) => { ev.stopPropagation(); onEvent(e); }}
                        >
                          {e.all_day ? e.summary : `${hhmm(e.start)} ${e.summary}`}
                        </div>
                      );
                    })}
                    {evs.length > lines && <div class="more">+{evs.length - lines}</div>}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
}
