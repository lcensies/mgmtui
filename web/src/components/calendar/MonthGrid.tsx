// 6-week month grid starting on the configured week start, with an ISO week-number gutter and
// colored event labels/dots.

import type { EventItem } from "../../api";
import { eventsOnDay, withoutWeekends } from "../../lib/cal";
import { contrastText, eventColor } from "../../lib/colors";
import { startDrag } from "../../lib/drag";
import { t } from "../../lib/i18n";
import { addDays, fmtDate, hhmm, isoWeek, now as nowSig, sameDay, startOfMonthGrid, startOfWeek, ymd } from "../../lib/time";
import { meta } from "../../state/meta";
import { calOpts } from "../../state/settings";
import { dayAt, dragOverDay } from "./EventBlock";

/** Localized weekday abbreviations from the configured week start (fmtDate reads the lang signal,
 *  so a language switch re-renders any component that calls this during render). */
function dowLabels(hideWeekends: boolean): string[] {
  const first = startOfWeek(new Date());
  return withoutWeekends(Array.from({ length: 7 }, (_, i) => addDays(first, i)), hideWeekends).map((d) =>
    fmtDate(d, { weekday: "short" }),
  );
}

export function MonthGrid({
  anchor,
  events,
  onDay,
  onEvent,
  onEventDay,
}: {
  anchor: Date;
  events: EventItem[];
  onDay: (d: Date) => void;
  onEvent: (ev: EventItem) => void;
  onEventDay?: (ev: EventItem, dayKey: string) => void;
}) {
  const gridStart = startOfMonthGrid(anchor);
  const month = anchor.getMonth();
  const now = nowSig.value;
  const { hideWeekends } = calOpts();
  // Narrow screens always get dots: text labels in ~45px cells collide with the day number.
  const narrow = typeof window !== "undefined" && window.matchMedia("(max-width: 560px)").matches;
  const lines = narrow ? 0 : Math.min(3, meta.value?.calendar?.month_event_lines ?? 0);
  const weeks = Array.from({ length: 6 }, (_, w) =>
    withoutWeekends(Array.from({ length: 7 }, (_, d) => addDays(gridStart, w * 7 + d)), hideWeekends),
  );

  return (
    <div class="month" style={`--cols:${weeks[0].length}`}>
      <div class="month-head">
        <div class="wk">{t("Wk")}</div>
        {dowLabels(hideWeekends).map((d) => (
          <div class="dow">{d}</div>
        ))}
      </div>
      {weeks.map((week) => (
        <div class="month-row">
          <div class="wk">{isoWeek(week[0])}</div>
          {week.map((day) => {
            const evs = eventsOnDay(day, events);
            const inMonth = day.getMonth() === month;
            return (
              <div
                class={`daycell ${inMonth ? "" : "adj"} ${sameDay(day, now) ? "today" : ""} ${dragOverDay.value === ymd(day) ? "over" : ""}`}
                data-day={ymd(day)}
                // Tap (not click) so a horizontal swipe across the grid pages the month instead
                // of also opening whichever day the gesture happened to start on.
                onPointerDown={(e) => startDrag(e, { onTap: () => onDay(day) })}
              >
                <div class="daynum">{day.getDate()}</div>
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
                          style={{ background: bg, color: contrastText(bg), touchAction: "none" }}
                          onClick={(ev) => ev.stopPropagation()} // never let a chip gesture open the day
                          onPointerDown={(pe) => {
                            pe.stopPropagation();
                            startDrag(pe, {
                              onMove: (_dx, _dy, m) => { dragOverDay.value = dayAt(m.clientX, m.clientY); },
                              onEnd: (_dx, _dy, m) => {
                                const target = dayAt(m.clientX, m.clientY);
                                dragOverDay.value = null;
                                if (onEventDay && target && target !== ymd(day)) onEventDay(e, target);
                              },
                              onTap: () => onEvent(e),
                            });
                          }}
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
