import { useEffect, useState } from "preact/hooks";
import { api, type Alarm, type EventItem, type Frequency, type RecurrenceRule } from "../../api";
import { invalidate, resource, showToast } from "../../lib/cache";
import { t } from "../../lib/i18n";
import { minutesToOffset, offsetToMinutes, parseOffsetList } from "../../lib/notify";
import { meta } from "../../state/meta";
import { closeModal, openModal } from "../../state/ui";
import { Overlay } from "./ModalHost";
import { RecurrenceEditor } from "./RecurrenceEditor";

// The form fields are LOCAL wall-clock; the API carries true UTC instants (see lib/time.ts).
// All-day events are the exception: they are anchored at UTC midnight of their calendar date
// (pure date semantics), so their date is read/written with UTC components.
function localParts(rfc?: string, utcDateOnly = false) {
  const d = rfc ? new Date(rfc) : new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  if (utcDateOnly) {
    return {
      date: `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())}`,
      time: `${p(d.getUTCHours())}:${p(d.getUTCMinutes())}`,
    };
  }
  return {
    date: `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`,
    time: `${p(d.getHours())}:${p(d.getMinutes())}`,
  };
}

function combine(date: string, time: string): string {
  const [y, m, d] = date.split("-").map(Number);
  const [hh, mm] = time.split(":").map(Number);
  return new Date(y, m - 1, d, hh, mm, 0).toISOString(); // local wall-clock → UTC instant
}

function alarmsToText(alarms?: Alarm[]): string {
  return (alarms ?? []).map((a) => minutesToOffset(a.trigger.MinutesBefore)).join(", ");
}

export function EventForm({ event, date, end: endProp }: { event?: EventItem; date?: string; end?: string }) {
  // A duplicate arrives as a full event with an empty uid — prefilled, but still a *new* event.
  const editing = !!event?.uid;
  const startInit = localParts(event?.start ?? date, event?.all_day ?? false);
  const endInit = localParts(
    event?.end ?? endProp ?? (date ? new Date(new Date(date).getTime() + 30 * 60000).toISOString() : undefined),
    event?.all_day ?? false,
  );

  const [summary, setSummary] = useState(event?.summary ?? "");
  const [calendar, setCalendar] = useState(event?.calendar ?? "default");
  const [allDay, setAllDay] = useState(event?.all_day ?? false);
  const [dateV, setDateV] = useState(startInit.date);
  const [start, setStart] = useState(startInit.time);
  const [end, setEnd] = useState(endInit.time);
  const [endTouched, setEndTouched] = useState(editing || !!endProp);
  const [location, setLocation] = useState(event?.location ?? "");
  const [conference, setConference] = useState(event?.conference_url ?? "");
  const [project, setProject] = useState(event?.project ?? "");
  const [description, setDescription] = useState(event?.description ?? "");
  const [rrule, setRrule] = useState<RecurrenceRule | undefined>(event?.rrule);
  // New events start with the configured alarm defaults; edits show the event's own alarms.
  const [alarmsText, setAlarmsText] = useState(
    editing ? alarmsToText(event?.alarms) : alarmsToText(meta.value?.event_alarm_defaults),
  );
  // The master event backing an agenda occurrence (occurrences share the master uid, so a re-save
  // must go through the master to keep rrule/alarms/description — and its real start/end — intact).
  const [master, setMaster] = useState<EventItem | undefined>(event);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!event?.uid) return;
    api
      .event(event.uid)
      .then((m) => {
        setMaster(m);
        setSummary(m.summary);
        setCalendar(m.calendar);
        setAllDay(m.all_day);
        const s = localParts(m.start, m.all_day);
        const e = localParts(m.end, m.all_day);
        setDateV(s.date);
        setStart(s.time);
        setEnd(e.time);
        setLocation(m.location ?? "");
        setConference(m.conference_url ?? "");
        setProject(m.project ?? "");
        setDescription(m.description ?? "");
        setRrule(m.rrule);
        setAlarmsText(alarmsToText(m.alarms));
      })
      .catch(() => {
        /* keep the occurrence's values; save still targets the same uid */
      });
  }, []);

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
    setError("");
    if (!allDay && end <= start) {
      setError(t("End must be after start"));
      return;
    }
    const offsets = parseOffsetList(alarmsText);
    if (offsets === null) {
      setError(t("Reminders must be offsets like 15m, 1h, 1d"));
      return;
    }
    // Rebuild alarms from the offsets, keeping any existing alarm object with the same trigger
    // (so custom actions/descriptions survive an unrelated edit).
    const existing = master?.alarms ?? [];
    const alarms: Alarm[] = offsets.map((o) => {
      const min = offsetToMinutes(o) ?? 0;
      return existing.find((a) => a.trigger.MinutesBefore === min) ?? { trigger: { MinutesBefore: min }, action: "notify" };
    });
    setBusy(true);
    try {
      const [y, m, d] = dateV.split("-").map(Number);
      // All-day events stay anchored at UTC midnight of the calendar date (pure date semantics).
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
        conference_url: conference.trim() || undefined,
        project: project.trim() || undefined,
        description: description.trim() || undefined,
        rrule,
        alarms: alarms.length ? alarms : undefined,
        status: master?.status,
      };
      if (editing) await api.updateEvent(body);
      else await api.createEvent(body);
      invalidate("all");
      closeModal();
    } catch (err) {
      showToast(err instanceof Error ? err.message : t("save failed"));
      setBusy(false);
    }
  }

  function duplicate() {
    // Reopen the form on a copy with no uid: saving creates a fresh event (the server assigns
    // the new UID) and leaves the original untouched.
    openModal({ kind: "eventForm", event: { ...(master ?? event)!, uid: "" } });
  }

  async function remove() {
    if (!event) return;
    try {
      await api.deleteEvent(event.uid);
      invalidate("all");
      closeModal();
    } catch (err) {
      showToast(err instanceof Error ? err.message : t("delete failed"));
    }
  }

  return (
    <Overlay>
      <form onSubmit={submit} style={{ display: "flex", flexDirection: "column", gap: "10px" }}>
        <h2>{editing ? t("Edit event") : t("New event")}</h2>
        <div class="field">
          <label>{t("Title")}</label>
          <input autofocus value={summary} onInput={(e) => setSummary((e.target as HTMLInputElement).value)} />
        </div>
        <label class="row" style={{ gap: "6px" }}>
          <input type="checkbox" checked={allDay} style={{ width: "auto" }} onChange={(e) => setAllDay((e.target as HTMLInputElement).checked)} />
          {t("All day")}
        </label>
        <div class="row">
          <div class="field grow">
            <label>{t("Date")}</label>
            <input type="date" value={dateV} onInput={(e) => setDateV((e.target as HTMLInputElement).value)} />
          </div>
          {!allDay && (
            <>
              <div class="field">
                <label>{t("Start")}</label>
                <input type="time" value={start} onInput={(e) => onStart((e.target as HTMLInputElement).value)} />
              </div>
              <div class="field">
                <label>{t("End")}</label>
                <input type="time" value={end} onInput={(e) => { setEndTouched(true); setEnd((e.target as HTMLInputElement).value); }} />
              </div>
            </>
          )}
        </div>
        <div class="row">
          <div class="field grow">
            <label>{t("Project")}</label>
            <input list="projects" value={project} onInput={(e) => setProject((e.target as HTMLInputElement).value)} />
            <datalist id="projects">
              {(meta.value?.projects ?? []).map((p) => <option value={p.name} />)}
            </datalist>
          </div>
          <div class="field grow">
            <label>{t("Calendar")}</label>
            <select value={calendar} onChange={(e) => setCalendar((e.target as HTMLSelectElement).value)}>
              {calendarNames(calendar).map((c) => <option value={c}>{c}</option>)}
            </select>
          </div>
        </div>
        <div class="field">
          <label>{t("Location")}</label>
          <input value={location} onInput={(e) => setLocation((e.target as HTMLInputElement).value)} />
        </div>
        <div class="field">
          <label>{t("Conference / video call")}{conference.trim() && (
            <> · <a href={conference.trim()} target="_blank" rel="noreferrer">{t("Join")} ↗</a></>
          )}</label>
          <input
            type="url"
            placeholder="https://telemost.yandex.ru/… or https://meet.google.com/…"
            value={conference}
            onInput={(e) => setConference((e.target as HTMLInputElement).value)}
          />
        </div>
        <RecurrenceEditor value={rrule} onChange={setRrule} />
        <div class="field">
          <label>{t("Reminders")}</label>
          <input
            placeholder={t("e.g. 15m, 1h, 1d — empty for none")}
            value={alarmsText}
            onInput={(e) => setAlarmsText((e.target as HTMLInputElement).value)}
          />
        </div>
        <div class="field">
          <label>{t("Description")}</label>
          <textarea rows={3} value={description} onInput={(e) => setDescription((e.target as HTMLTextAreaElement).value)} />
        </div>
        {error && <div class="error">{error}</div>}
        <div class="actions">
          {editing && <button type="button" style={{ marginRight: "auto", color: "var(--red)" }} onClick={remove}>{t("Delete")}</button>}
          {editing && <button type="button" onClick={duplicate}>{t("Duplicate")}</button>}
          <button type="button" onClick={closeModal}>{t("Cancel")}</button>
          <button class="primary" type="submit" disabled={busy}>{editing ? t("Save") : t("Create")}</button>
        </div>
      </form>
    </Overlay>
  );
}

export type { Frequency };

/** Calendars offered by the picker: the server's list plus whatever the event already names. */
function calendarNames(current: string): string[] {
  const names = (resource("calendars", api.calendars).data.value ?? []).map((c) => c.name);
  return names.includes(current) ? names : [...names, current].filter(Boolean);
}
