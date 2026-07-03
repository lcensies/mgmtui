import { useState } from "preact/hooks";
import { api, type EventItem, type Frequency, type RecurrenceRule } from "../../api";
import { invalidate } from "../../lib/cache";
import { meta } from "../../state/meta";
import { closeModal } from "../../state/ui";
import { Overlay } from "./ModalHost";
import { RecurrenceEditor } from "./RecurrenceEditor";

// Times are UTC wall-clock (see lib/time.ts), so read/write with UTC components.
function localParts(rfc?: string) {
  const d = rfc ? new Date(rfc) : new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return {
    date: `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())}`,
    time: `${p(d.getUTCHours())}:${p(d.getUTCMinutes())}`,
  };
}

function combine(date: string, time: string): string {
  const [y, m, d] = date.split("-").map(Number);
  const [hh, mm] = time.split(":").map(Number);
  return new Date(Date.UTC(y, m - 1, d, hh, mm, 0)).toISOString();
}

export function EventForm({ event, date }: { event?: EventItem; date?: string }) {
  const editing = !!event;
  const startInit = localParts(event?.start ?? date);
  const endInit = localParts(event?.end ?? (date ? new Date(new Date(date).getTime() + 30 * 60000).toISOString() : undefined));

  const [summary, setSummary] = useState(event?.summary ?? "");
  const [calendar, setCalendar] = useState(event?.calendar ?? "default");
  const [allDay, setAllDay] = useState(event?.all_day ?? false);
  const [dateV, setDateV] = useState(startInit.date);
  const [start, setStart] = useState(startInit.time);
  const [end, setEnd] = useState(endInit.time);
  const [endTouched, setEndTouched] = useState(editing);
  const [location, setLocation] = useState(event?.location ?? "");
  const [project, setProject] = useState(event?.project ?? "");
  const [description, setDescription] = useState(event?.description ?? "");
  const [rrule, setRrule] = useState<RecurrenceRule | undefined>(event?.rrule);
  const [busy, setBusy] = useState(false);

  function onStart(v: string) {
    setStart(v);
    if (!endTouched) {
      const [h, m] = v.split(":").map(Number);
      const d = new Date(0, 0, 0, h, m + 30);
      setEnd(`${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`);
    }
  }

  async function submit(e: Event) {
    e.preventDefault();
    if (!summary.trim() || busy) return;
    setBusy(true);
    try {
      const [y, m, d] = dateV.split("-").map(Number);
      const startISO = allDay ? new Date(Date.UTC(y, m - 1, d)).toISOString() : combine(dateV, start);
      const endISO = allDay ? new Date(Date.UTC(y, m - 1, d + 1)).toISOString() : combine(dateV, end);
      const body: EventItem = {
        uid: event?.uid ?? "",
        calendar: calendar.trim() || "default",
        summary: summary.trim(),
        all_day: allDay,
        start: startISO,
        end: endISO,
        location: location.trim() || undefined,
        project: project.trim() || undefined,
        description: description.trim() || undefined,
        rrule,
        alarms: event?.alarms,
        status: event?.status,
      };
      if (editing) await api.updateEvent(body);
      else await api.createEvent(body);
      invalidate("all");
      closeModal();
    } catch {
      setBusy(false);
    }
  }

  async function remove() {
    if (!event) return;
    await api.deleteEvent(event.uid);
    invalidate("all");
    closeModal();
  }

  return (
    <Overlay>
      <form onSubmit={submit} style={{ display: "flex", flexDirection: "column", gap: "10px" }}>
        <h2>{editing ? "Edit event" : "New event"}</h2>
        <div class="field">
          <label>Title</label>
          <input autofocus value={summary} onInput={(e) => setSummary((e.target as HTMLInputElement).value)} />
        </div>
        <label class="row" style={{ gap: "6px" }}>
          <input type="checkbox" checked={allDay} style={{ width: "auto" }} onChange={(e) => setAllDay((e.target as HTMLInputElement).checked)} />
          All day
        </label>
        <div class="row">
          <div class="field grow">
            <label>Date</label>
            <input type="date" value={dateV} onInput={(e) => setDateV((e.target as HTMLInputElement).value)} />
          </div>
          {!allDay && (
            <>
              <div class="field">
                <label>Start</label>
                <input type="time" value={start} onInput={(e) => onStart((e.target as HTMLInputElement).value)} />
              </div>
              <div class="field">
                <label>End</label>
                <input type="time" value={end} onInput={(e) => { setEndTouched(true); setEnd((e.target as HTMLInputElement).value); }} />
              </div>
            </>
          )}
        </div>
        <div class="row">
          <div class="field grow">
            <label>Project</label>
            <input list="projects" value={project} onInput={(e) => setProject((e.target as HTMLInputElement).value)} />
            <datalist id="projects">
              {(meta.value?.projects ?? []).map((p) => <option value={p.name} />)}
            </datalist>
          </div>
          <div class="field grow">
            <label>Calendar</label>
            <input value={calendar} onInput={(e) => setCalendar((e.target as HTMLInputElement).value)} />
          </div>
        </div>
        <div class="field">
          <label>Location</label>
          <input value={location} onInput={(e) => setLocation((e.target as HTMLInputElement).value)} />
        </div>
        <RecurrenceEditor value={rrule} onChange={setRrule} />
        <div class="field">
          <label>Description</label>
          <textarea rows={3} value={description} onInput={(e) => setDescription((e.target as HTMLTextAreaElement).value)} />
        </div>
        <div class="actions">
          {editing && <button type="button" style={{ marginRight: "auto", color: "var(--red)" }} onClick={remove}>Delete</button>}
          <button type="button" onClick={closeModal}>Cancel</button>
          <button class="primary" type="submit" disabled={busy}>{editing ? "Save" : "Create"}</button>
        </div>
      </form>
    </Overlay>
  );
}

export type { Frequency };
