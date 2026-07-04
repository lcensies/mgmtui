// Calendar: month grid + week + day time-block, with navigation and event create/edit.

import { signal } from "@preact/signals";
import { api, type EventItem } from "../api";
import { mutate, resource } from "../lib/cache";
import { addDays, atMinutes, fmtDate, startOfDay, startOfMonthGrid, startOfWeek } from "../lib/time";
import { openModal, searchText } from "../state/ui";
import { MonthGrid } from "../components/calendar/MonthGrid";
import { TimeGrid } from "../components/calendar/TimeGrid";

type View = "month" | "week" | "day";
const view = signal<View>("month");
const anchor = signal<Date>(startOfDay(new Date()));

function range(v: View, a: Date): [Date, Date] {
  if (v === "month") {
    const from = startOfMonthGrid(a);
    return [from, addDays(from, 42)];
  }
  if (v === "week") {
    const from = startOfWeek(a);
    return [from, addDays(from, 7)];
  }
  return [startOfDay(a), addDays(startOfDay(a), 1)];
}

function shift(dir: number) {
  const a = anchor.value;
  if (view.value === "month") anchor.value = new Date(Date.UTC(a.getUTCFullYear(), a.getUTCMonth() + dir, 1));
  else anchor.value = addDays(a, dir * (view.value === "week" ? 7 : 1));
}

function title(): string {
  const a = anchor.value;
  if (view.value === "month") return fmtDate(a, { month: "long", year: "numeric" });
  if (view.value === "week") {
    const s = startOfWeek(a);
    return `${fmtDate(s, { month: "short", day: "numeric" })} – ${fmtDate(addDays(s, 6), { month: "short", day: "numeric" })}`;
  }
  return fmtDate(a, { weekday: "long", month: "long", day: "numeric" });
}

export function Calendar() {
  const v = view.value;
  const a = anchor.value;
  const [from, to] = range(v, a);
  const key = `agenda:${from.toISOString()}:${to.toISOString()}`;
  const res = resource(key, () => api.agenda(from.toISOString(), to.toISOString()));
  const ag = res.data.value ?? { events: [], tasks: [] };

  // Client-side event search (summary/location), matching the TUI's calendar search.
  const q = searchText.value.trim().toLowerCase();
  const events = q
    ? ag.events.filter((e) => e.summary.toLowerCase().includes(q) || (e.location ?? "").toLowerCase().includes(q))
    : ag.events;

  const openEvent = (ev: EventItem) => openModal({ kind: "eventForm", event: ev });
  const days = v === "week" ? Array.from({ length: 7 }, (_, i) => addDays(startOfWeek(a), i)) : [startOfDay(a)];

  // Optimistic drag-reschedule: patch the cached instance immediately, then PUT the whole event.
  const reschedule = (ev: EventItem, startISO: string, endISO: string) => {
    const prev = res.data.value;
    void mutate({
      patch: () => {
        if (!res.data.value) return;
        res.data.value = {
          ...res.data.value,
          events: res.data.value.events.map((e) =>
            e.uid === ev.uid && e.start === ev.start ? { ...e, start: startISO, end: endISO } : e,
          ),
        };
      },
      rollback: () => { res.data.value = prev; },
      request: () => api.updateEvent({ ...ev, start: startISO, end: endISO }),
      after: ["agenda"],
    });
  };

  return (
    <div class="cal">
      <div class="cal-toolbar">
        <button class="icon" onClick={() => shift(-1)}>‹</button>
        <button class="icon" onClick={() => (anchor.value = startOfDay(new Date()))}>Today</button>
        <button class="icon" onClick={() => shift(1)}>›</button>
        <strong class="grow">{title()}</strong>
        <input
          class="search"
          type="search"
          placeholder="Search events…"
          value={searchText.value}
          onInput={(e) => (searchText.value = (e.target as HTMLInputElement).value)}
        />
        <div class="seg">
          {(["month", "week", "day"] as View[]).map((x) => (
            <button class={v === x ? "on" : ""} onClick={() => (view.value = x)}>{x}</button>
          ))}
        </div>
        <button class="primary" title="New event" onClick={() => openModal({ kind: "eventForm", date: a.toISOString() })}>+</button>
      </div>

      {res.error.value && <div class="error">{res.error.value}</div>}

      {v === "month" ? (
        <MonthGrid
          anchor={a}
          events={events}
          onEvent={openEvent}
          onDay={(d) => { anchor.value = d; view.value = "day"; }}
        />
      ) : (
        <TimeGrid
          days={days}
          events={events}
          tasks={ag.tasks}
          onEvent={openEvent}
          onSlot={(day, minutes) => openModal({ kind: "eventForm", date: atMinutes(day, minutes) })}
          onRange={(day, sMin, eMin) => openModal({ kind: "eventForm", date: atMinutes(day, sMin), end: atMinutes(day, eMin) })}
          onReschedule={reschedule}
        />
      )}
    </div>
  );
}
