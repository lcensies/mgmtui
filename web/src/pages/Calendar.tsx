// Calendar: month grid + week + day time-block, with navigation and event create/edit.

import { signal } from "@preact/signals";
import { api, type EventItem, type OccurrenceScope, type Task } from "../api";
import { mutate, resource } from "../lib/cache";
import { startDrag } from "../lib/drag";
import { t } from "../lib/i18n";
import { addDays, atMinutes, fmtDate, startOfDay, startOfMonthGrid, startOfWeek } from "../lib/time";
import { openModal, scopeParam, searchText } from "../state/ui";
import { MonthGrid } from "../components/calendar/MonthGrid";
import { moveToDay } from "../components/calendar/EventBlock";
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
  if (view.value === "month") anchor.value = new Date(a.getFullYear(), a.getMonth() + dir, 1);
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
  const projects = scopeParam();
  const key = `agenda:${from.toISOString()}:${to.toISOString()}:${projects ?? ""}`;
  const res = resource(key, () => api.agenda(from.toISOString(), to.toISOString(), projects));
  const ag = res.data.value ?? { events: [], tasks: [] };

  // Client-side event search (summary/location), matching the TUI's calendar search.
  const q = searchText.value.trim().toLowerCase();
  const events = q
    ? ag.events.filter((e) => e.summary.toLowerCase().includes(q) || (e.location ?? "").toLowerCase().includes(q))
    : ag.events;

  const openEvent = (ev: EventItem) => openModal({ kind: "eventForm", event: ev });
  const days = v === "week" ? Array.from({ length: 7 }, (_, i) => addDays(startOfWeek(a), i)) : [startOfDay(a)];

  // Optimistic drag-reschedule: patch the cached instance immediately, then PUT the whole event.
  // Recurring events need special care: /api/agenda returns expanded occurrences sharing the
  // master uid, so PUTting an occurrence's absolute times would rewrite the series' DTSTART to
  // that day. With scope "all" we apply the drag *delta* to the master's own start/end (rrule
  // kept); "this"/"following" go through the occurrence-scoped API.
  const commit = (ev: EventItem, startISO: string, endISO: string, scope: OccurrenceScope = "all") => {
    const prev = res.data.value;
    const dStart = new Date(startISO).getTime() - new Date(ev.start).getTime();
    const dEnd = new Date(endISO).getTime() - new Date(ev.end).getTime();
    const shiftIso = (iso: string, ms: number) => new Date(new Date(iso).getTime() + ms).toISOString();
    const seriesMove = !!ev.rrule && scope === "all";
    void mutate({
      patch: () => {
        if (!res.data.value) return;
        res.data.value = {
          ...res.data.value,
          events: res.data.value.events.map((e) => {
            if (e.uid !== ev.uid) return e;
            // A recurring master shifts every visible occurrence by the same delta.
            if (seriesMove) return { ...e, start: shiftIso(e.start, dStart), end: shiftIso(e.end, dEnd) };
            return e.start === ev.start ? { ...e, start: startISO, end: endISO } : e;
          }),
        };
      },
      rollback: () => { res.data.value = prev; },
      request: async () => {
        if (seriesMove) {
          const master = await api.event(ev.uid);
          await api.updateEvent({ ...master, start: shiftIso(master.start, dStart), end: shiftIso(master.end, dEnd) });
        } else if (ev.rrule) {
          await api.updateEvent({ ...ev, start: startISO, end: endISO }, { at: ev.occurrence_start ?? ev.start, scope });
        } else {
          await api.updateEvent({ ...ev, start: startISO, end: endISO });
        }
      },
      after: ["agenda"],
    });
  };

  // A dragged occurrence of a recurring event asks which instances the move applies to.
  const reschedule = (ev: EventItem, startISO: string, endISO: string) => {
    if (!ev.rrule) return commit(ev, startISO, endISO);
    openModal({
      kind: "scope",
      message: t("This event repeats — move:"),
      onPick: (scope) => commit(ev, startISO, endISO, scope),
    });
  };

  /** Month-grid drop: shift an event onto another date, keeping time-of-day and duration.
   *  All-day events are pure dates anchored at UTC midnight, so they re-anchor in UTC space. */
  const moveEventToDay = (ev: EventItem, dayKey: string) => {
    const dur = new Date(ev.end).getTime() - new Date(ev.start).getTime();
    let start: Date;
    if (ev.all_day) {
      const [y, m, d] = dayKey.split("-").map(Number);
      start = new Date(Date.UTC(y, m - 1, d));
    } else {
      start = moveToDay(ev.start, dayKey, 0);
    }
    reschedule(ev, start.toISOString(), new Date(start.getTime() + dur).toISOString());
  };

  /** Task drop: move whichever date placed it on the grid (scheduled wins over due). */
  const moveTaskToDay = (task: Task, dayKey: string) => {
    const field = task.scheduled ? "scheduled" : "due";
    const cur = task.scheduled ?? task.due;
    if (!cur) return;
    const next = moveToDay(cur, dayKey, 0).toISOString();
    const prev = res.data.value;
    void mutate({
      patch: () => {
        if (!res.data.value) return;
        res.data.value = {
          ...res.data.value,
          tasks: res.data.value.tasks.map((x) => (x.uid === task.uid ? { ...x, [field]: next } : x)),
        };
      },
      rollback: () => { res.data.value = prev; },
      request: () => api.updateTask({ ...task, [field]: next }),
      after: ["agenda", "tasks", "board"],
    });
  };

  // Horizontal swipe on empty calendar space pages through months/weeks. Chips and the
  // drag-to-create sweep stop propagation, so they never reach this handler.
  const onSwipe = (e: PointerEvent) =>
    startDrag(e, {
      onEnd: (dx, dy) => {
        if (Math.abs(dx) > 60 && Math.abs(dx) > 2 * Math.abs(dy)) shift(dx < 0 ? 1 : -1);
      },
    });

  return (
    <div class="cal">
      <div class="cal-toolbar">
        <button class="icon" aria-label={t("Previous")} onClick={() => shift(-1)}>‹</button>
        <button class="icon" onClick={() => (anchor.value = startOfDay(new Date()))}>{t("Today")}</button>
        <button class="icon" aria-label={t("Next")} onClick={() => shift(1)}>›</button>
        <strong class="grow">{title()}</strong>
        <input
          class="search"
          type="search"
          placeholder={t("Search events…")}
          value={searchText.value}
          onInput={(e) => (searchText.value = (e.target as HTMLInputElement).value)}
        />
        <div class="seg">
          {(["month", "week", "day"] as View[]).map((x) => (
            <button class={v === x ? "on" : ""} onClick={() => (view.value = x)}>{t(x)}</button>
          ))}
        </div>
        <button class="primary" title={t("New event")} onClick={() => openModal({ kind: "eventForm", date: a.toISOString() })}>+</button>
      </div>

      {res.error.value && <div class="error">{res.error.value}</div>}

      <div class="cal-surface" onPointerDown={onSwipe}>
        {v === "month" ? (
          <MonthGrid
            anchor={a}
            events={events}
            onEvent={openEvent}
            onEventDay={moveEventToDay}
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
            onTaskDay={moveTaskToDay}
          />
        )}
      </div>
    </div>
  );
}
