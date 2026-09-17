// Agenda: the events of the loaded range as a day-grouped list (one header per day with events).

import type { EventItem } from "../../api";
import { eventColor } from "../../lib/colors";
import { t } from "../../lib/i18n";
import { fmtDate, hhmm, utcDate, ymd } from "../../lib/time";
import { meta } from "../../state/meta";

export function Agenda({ events, onEvent }: { events: EventItem[]; onEvent: (ev: EventItem) => void }) {
  const byDay = new Map<string, EventItem[]>();
  for (const ev of [...events].sort((a, b) => a.start.localeCompare(b.start))) {
    // All-day events are pure dates anchored at UTC midnight; timed ones group by local day.
    const key = ymd(ev.all_day ? utcDate(new Date(ev.start)) : new Date(ev.start));
    const list = byDay.get(key);
    if (list) list.push(ev);
    else byDay.set(key, [ev]);
  }

  if (byDay.size === 0) return <div class="muted">{t("Nothing scheduled")}</div>;

  return (
    <div class="agenda">
      {[...byDay.entries()].map(([key, evs]) => {
        const [y, m, d] = key.split("-").map(Number);
        return (
          <div class="agenda-day" data-day={key}>
            <h4>{fmtDate(new Date(y, m - 1, d), { weekday: "long", month: "long", day: "numeric" })}</h4>
            {evs.map((ev) => (
              <div class="agenda-row" onClick={() => onEvent(ev)}>
                <span class="when">{ev.all_day ? t("All day") : hhmm(ev.start)}</span>
                <span class="swatch" style={{ background: eventColor(ev, meta.value) }} />
                <span class="grow">{ev.summary}</span>
              </div>
            ))}
          </div>
        );
      })}
    </div>
  );
}
