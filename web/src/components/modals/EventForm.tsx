import { useEffect, useState } from "preact/hooks";
import {
  api,
  type Alarm,
  type Attendee,
  type Classification,
  type EventItem,
  type EventStatus,
  type Frequency,
  type OccurrenceScope,
  type RecurrenceRule,
  type Transparency,
} from "../../api";
import { invalidate, resource, showToast } from "../../lib/cache";
import { t } from "../../lib/i18n";
import { alarmsToText, parseAlarmList } from "../../lib/notify";
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

/** The end DATE to show for an event. All-day ends are exclusive (UTC midnight of the next day). */
function endDateOf(rfc: string | undefined, allDay: boolean, fallback: string): string {
  if (!rfc) return fallback;
  const d = new Date(rfc);
  if (allDay) d.setUTCDate(d.getUTCDate() - 1);
  return localParts(d.toISOString(), allDay).date;
}

/** "ACCEPTED" → "Accepted"; an absent PARTSTAT means the invitee has not answered. */
function partstatLabel(p?: string): string {
  const s = (p ?? "NEEDS-ACTION").replace(/-/g, " ").toLowerCase();
  return t(s.charAt(0).toUpperCase() + s.slice(1));
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
  const [endDate, setEndDate] = useState(endDateOf(event?.end ?? endProp, event?.all_day ?? false, startInit.date));
  const [start, setStart] = useState(startInit.time);
  const [end, setEnd] = useState(endInit.time);
  const [endTouched, setEndTouched] = useState(editing || !!endProp);
  const [location, setLocation] = useState(event?.location ?? "");
  const [conference, setConference] = useState(event?.conference_url ?? "");
  const [project, setProject] = useState(event?.project ?? "");
  const [description, setDescription] = useState(event?.description ?? "");
  const [rrule, setRrule] = useState<RecurrenceRule | undefined>(event?.rrule);
  const [status, setStatus] = useState<EventStatus>(event?.status ?? "Confirmed");
  const [transp, setTransp] = useState<Transparency>(event?.transp ?? "Opaque");
  const [klass, setKlass] = useState<Classification>(event?.class ?? "Public");
  const [color, setColor] = useState(event?.color ?? "");
  const [url, setUrl] = useState(event?.url ?? "");
  // ponytail: categories are a comma-separated text field, not a chip widget — same data, no
  // keyboard/focus handling to own. Swap in chips if tag entry ever needs autocomplete.
  const [categories, setCategories] = useState((event?.categories ?? []).join(", "));
  const [attendees, setAttendees] = useState<Attendee[]>(event?.attendees ?? []);
  // New events start with the configured alarm defaults; edits show the event's own alarms.
  const [alarmsText, setAlarmsText] = useState(
    editing ? alarmsToText(event?.alarms) : alarmsToText(meta.value?.event_alarm_defaults),
  );
  // The master event backing an agenda occurrence (occurrences share the master uid, so a re-save
  // must go through the master to keep rrule/alarms/description — and its real start/end — intact).
  const [master, setMaster] = useState<EventItem | undefined>(event);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  // Occurrence identity for scoped saves/deletes: `at` is the instance's slot in its series.
  const occAt = event?.occurrence_start ?? event?.start;
  const recurring = !!(master?.rrule ?? event?.rrule);

  const finish = async (p: Promise<unknown>, failed = t("save failed")) => {
    try {
      await p;
      invalidate("all");
      closeModal();
    } catch (err) {
      showToast(err instanceof Error ? err.message : failed);
      setBusy(false);
    }
  };

  useEffect(() => {
    if (!event?.uid) return;
    api
      .event(event.uid)
      .then((m) => {
        setMaster(m);
        setSummary(m.summary);
        setCalendar(m.calendar);
        setAllDay(m.all_day);
        // A recurring occurrence keeps the clicked instance's date/time (the save scope decides
        // which instances it applies to); a plain event follows the master.
        if (!m.rrule) {
          const s = localParts(m.start, m.all_day);
          const e = localParts(m.end, m.all_day);
          setDateV(s.date);
          setStart(s.time);
          setEnd(e.time);
          setEndDate(endDateOf(m.end, m.all_day, s.date));
        }
        setStatus(m.status ?? "Confirmed");
        setTransp(m.transp ?? "Opaque");
        setKlass(m.class ?? "Public");
        setColor(m.color ?? "");
        setUrl(m.url ?? "");
        setCategories((m.categories ?? []).join(", "));
        setAttendees(m.attendees ?? []);
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

  function onDate(v: string) {
    if (endDate === dateV) setEndDate(v); // a same-day event stays same-day when the date moves
    setDateV(v);
  }

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
    const triggers = parseAlarmList(alarmsText);
    if (triggers === null) {
      setError(t("Reminders must be offsets like 15m, 1h, end-5m, @2026-09-20T09:00"));
      return;
    }
    // Rebuild alarms from the parsed triggers, keeping any existing alarm object with the same
    // trigger (so custom actions/descriptions survive an unrelated edit).
    const existing = master?.alarms ?? [];
    const alarms: Alarm[] = triggers.map(
      (tr) => existing.find((a) => JSON.stringify(a.trigger) === JSON.stringify(tr)) ?? { trigger: tr, action: "notify" },
    );
    const [y, m, d] = dateV.split("-").map(Number);
    const [ey, em, ed] = endDate.split("-").map(Number);
    // All-day events stay anchored at UTC midnight of the calendar date (pure date semantics),
    // with an exclusive end, so a one-day event ends at the next midnight.
    const startISO = allDay ? new Date(Date.UTC(y, m - 1, d)).toISOString() : combine(dateV, start);
    const endISO = allDay ? new Date(Date.UTC(ey, em - 1, ed + 1)).toISOString() : combine(endDate, end);
    if (Date.parse(endISO) <= Date.parse(startISO)) {
      setError(t("End must be after start"));
      return;
    }
    setBusy(true);
    try {
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
        status,
        transp,
        class: klass,
        color: color || undefined,
        url: url.trim() || undefined,
        categories: categories.split(",").map((c) => c.trim()).filter(Boolean),
        organizer: master?.organizer,
        attendees: attendees.filter((a) => a.email.trim()),
      };
      const write = (scope: OccurrenceScope) => {
        if (!editing) return api.createEvent(body);
        if (!recurring || !occAt) return api.updateEvent(body);
        if (scope !== "all") return api.updateEvent(body, { at: occAt, scope });
        // "All events": shift the series by the delta the user applied to this occurrence, so the
        // other instances keep their own dates.
        const shift = (iso: string, ms: number) => new Date(new Date(iso).getTime() + ms).toISOString();
        const base = master ?? event!;
        const dStart = new Date(startISO).getTime() - new Date(occAt).getTime();
        const dEnd = new Date(endISO).getTime() - new Date(event?.end ?? endISO).getTime();
        return api.updateEvent({ ...body, start: shift(base.start, dStart), end: shift(base.end, dEnd) });
      };
      if (editing && recurring && occAt) {
        setBusy(false);
        openModal({
          kind: "scope",
          message: t("This event repeats — save changes to:"),
          onPick: (scope) => void finish(write(scope)),
        });
        return;
      }
      await finish(write("all"));
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

  function remove() {
    if (!event) return;
    if (recurring && occAt) {
      openModal({
        kind: "scope",
        message: t("This event repeats — delete:"),
        onPick: (scope) => void finish(api.deleteEvent(event.uid, { at: occAt, scope }), t("delete failed")),
      });
      return;
    }
    void finish(api.deleteEvent(event.uid), t("delete failed"));
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
            <input type="date" value={dateV} onInput={(e) => onDate((e.target as HTMLInputElement).value)} />
          </div>
          {!allDay && (
            <div class="field">
              <label>{t("Start")}</label>
              <input type="time" value={start} onInput={(e) => onStart((e.target as HTMLInputElement).value)} />
            </div>
          )}
          <div class="field grow">
            <label>{t("End date")}</label>
            <input type="date" value={endDate} min={dateV} onInput={(e) => setEndDate((e.target as HTMLInputElement).value)} />
          </div>
          {!allDay && (
            <div class="field">
              <label>{t("End")}</label>
              <input type="time" value={end} onInput={(e) => { setEndTouched(true); setEnd((e.target as HTMLInputElement).value); }} />
            </div>
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
        <div class="row">
          <div class="field grow">
            <label>{t("Status")}</label>
            <select value={status} onChange={(e) => setStatus((e.target as HTMLSelectElement).value as EventStatus)}>
              <option value="Confirmed">{t("Confirmed")}</option>
              <option value="Tentative">{t("Tentative")}</option>
              <option value="Cancelled">{t("Cancelled")}</option>
            </select>
          </div>
          <div class="field grow">
            <label>{t("Shows as")}</label>
            <select value={transp} onChange={(e) => setTransp((e.target as HTMLSelectElement).value as Transparency)}>
              <option value="Opaque">{t("Busy")}</option>
              <option value="Transparent">{t("Free")}</option>
            </select>
          </div>
          <div class="field grow">
            <label>{t("Visibility")}</label>
            <select value={klass} onChange={(e) => setKlass((e.target as HTMLSelectElement).value as Classification)}>
              <option value="Public">{t("Public")}</option>
              <option value="Private">{t("Private")}</option>
              <option value="Confidential">{t("Confidential")}</option>
            </select>
          </div>
          <div class="field">
            <label>{t("Color")}</label>
            <div class="row" style={{ gap: "4px" }}>
              <input type="color" value={color || "#4285f4"} onInput={(e) => setColor((e.target as HTMLInputElement).value)} />
              {color && <button type="button" title={t("Use calendar color")} onClick={() => setColor("")}>×</button>}
            </div>
          </div>
        </div>
        <div class="row">
          <div class="field grow">
            <label>{t("URL")}</label>
            <input type="url" value={url} onInput={(e) => setUrl((e.target as HTMLInputElement).value)} />
          </div>
          <div class="field grow">
            <label>{t("Categories")}</label>
            <input placeholder={t("comma separated")} value={categories} onInput={(e) => setCategories((e.target as HTMLInputElement).value)} />
          </div>
        </div>
        <div class="field">
          <label>{t("Attendees")}</label>
          {attendees.map((a, i) => (
            <div class="row" style={{ gap: "4px" }} key={i}>
              <input
                type="email"
                placeholder={t("email")}
                value={a.email}
                onInput={(e) => setAttendees(attendees.map((x, j) => (j === i ? { ...x, email: (e.target as HTMLInputElement).value } : x)))}
              />
              <input
                placeholder={t("Name")}
                value={a.name ?? ""}
                onInput={(e) => setAttendees(attendees.map((x, j) => (j === i ? { ...x, name: (e.target as HTMLInputElement).value || undefined } : x)))}
              />
              {/* PARTSTAT is what the other side answered — mgmt never sends invitations, so it is read-only here. */}
              <span class="dim" style={{ whiteSpace: "nowrap" }}>{partstatLabel(a.partstat)}</span>
              <button type="button" onClick={() => setAttendees(attendees.filter((_, j) => j !== i))}>×</button>
            </div>
          ))}
          <button type="button" onClick={() => setAttendees([...attendees, { email: "" }])}>{t("Add attendee")}</button>
        </div>
        <div class="field">
          <label>{t("Reminders")}</label>
          <input
            placeholder={t("e.g. 15m, 1h, end-5m, @2026-09-20T09:00 — empty for none")}
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
