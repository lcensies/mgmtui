// Time-block grid for one or more days (Day view = 1 column, Week view = 7). Shared time window +
// a single hour gutter; greedy lane-packed event blocks; a "now" line on today's column.

import type { EventItem, Task } from "../../api";
import { layoutDay, eventsOnDay, type DayLayout } from "../../lib/cal";
import { contrastText, eventColor, projectColor } from "../../lib/colors";
import { fmtDate, hhmm, hourLabel, minutesOfDay, sameDay, secondaryHour } from "../../lib/time";
import { meta } from "../../state/meta";
import { settings } from "../../state/settings";
import { EventBlock } from "./EventBlock";

const PX_PER_HOUR = 48;

export function TimeGrid({
  days,
  events,
  tasks,
  onEvent,
  onSlot,
  onReschedule,
}: {
  days: Date[];
  events: EventItem[];
  tasks: Task[];
  onEvent: (ev: EventItem) => void;
  onSlot?: (day: Date, minutes: number) => void;
  onReschedule?: (ev: EventItem, startISO: string, endISO: string) => void;
}) {
  const layouts: DayLayout[] = days.map((d) => layoutDay(d, eventsOnDay(d, events)));
  const startHour = Math.min(...layouts.map((l) => l.startHour));
  const endHour = Math.max(...layouts.map((l) => l.endHour));
  const height = (endHour - startHour) * PX_PER_HOUR;
  const now = new Date();

  const tasksOn = (d: Date) =>
    tasks.filter((t) => {
      const cd = t.scheduled ?? t.due;
      return cd && sameDay(new Date(cd), d);
    });

  return (
    <div class="timegrid">
      <div class="tg-head">
        <div class="tg-gutter-head" />
        {days.map((d, i) => (
          <div class={`tg-dayhead ${sameDay(d, now) ? "today" : ""}`} key={i}>
            <div class="tg-dow">
              {fmtDate(d, { weekday: "short" })} {d.getUTCDate()}
            </div>
            <div class="tg-band">
              {layouts[i].allDay.map((ev) => {
                const bg = eventColor(ev, meta.value);
                return (
                  <div class="allday" style={{ background: bg, color: contrastText(bg) }} onClick={() => onEvent(ev)}>
                    {ev.summary}
                  </div>
                );
              })}
              {tasksOn(d).map((t) => (
                <div class="band-task" style={{ color: projectColor(t.project, meta.value) }}>
                  ○ {t.title}
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>

      <div class="tg-body" style={{ height: `${height}px` }}>
        <div class="tg-gutter">
          {Array.from({ length: endHour - startHour }, (_, h) => {
            const hour = startHour + h;
            const sec = secondaryHour(hour, settings.value.secondaryTz);
            return (
              <div class="tg-hour" style={{ top: `${h * PX_PER_HOUR}px` }}>
                {hourLabel(hour)}
                {sec && <span class="tg-hour-2">{sec}</span>}
              </div>
            );
          })}
        </div>
        {days.map((d, i) => (
          <div
            class="tg-col"
            key={i}
            onClick={(e) => {
              // Single click on empty grid space creates an event; clicks on event blocks are ignored.
              if (e.target === e.currentTarget) onSlot?.(d, slotMinutes(e, startHour));
            }}
          >
            {layouts[i].timed.map((p) => (
              <EventBlock
                pos={p}
                lanes={layouts[i].lanes}
                pxPerHour={PX_PER_HOUR}
                day={d}
                startHour={startHour}
                label={`${hhmm(p.ev.start)} ${p.ev.summary}`}
                onClick={() => onEvent(p.ev)}
                onReschedule={onReschedule}
              />
            ))}
            {sameDay(d, now) && minutesOfDay(now) >= startHour * 60 && minutesOfDay(now) <= endHour * 60 && (
              <div class="nowline" style={{ top: `${((minutesOfDay(now) - startHour * 60) / 60) * PX_PER_HOUR}px` }}>
                <span class="nowdot" />
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

function slotMinutes(e: MouseEvent, startHour: number): number {
  const col = (e.currentTarget as HTMLElement);
  const rect = col.getBoundingClientRect();
  const y = e.clientY - rect.top;
  return startHour * 60 + Math.round((y / PX_PER_HOUR) * 60 / 15) * 15;
}
