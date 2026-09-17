// Settings → "Calendars": create / rename / recolor / delete local calendars, upload an .ics into
// one, download one. Backed by /api/calendars (collection directories + config.yaml metadata).

import { useEffect, useState } from "preact/hooks";
import { api, type CalendarInfo } from "../../api";
import { toast } from "../../lib/cache";
import { t } from "../../lib/i18n";

export function LocalCalendarsSection() {
  const [cals, setCals] = useState<CalendarInfo[]>([]);
  const [newId, setNewId] = useState("");
  const [busy, setBusy] = useState(false);

  const refresh = () => api.calendars().then(setCals).catch((e) => (toast.value = msg(e)));
  useEffect(() => { refresh(); }, []);

  async function run(fn: () => Promise<unknown>) {
    setBusy(true);
    try {
      await fn();
      await refresh();
    } catch (e) {
      toast.value = msg(e);
    } finally {
      setBusy(false);
    }
  }

  function rename(c: CalendarInfo) {
    const next = prompt(t("New calendar name"), c.id)?.trim();
    if (next && next !== c.id) void run(() => api.updateCalendar(c.id, { id: next }));
  }

  function remove(c: CalendarInfo) {
    const force = c.events > 0;
    if (force && !confirm(t("Move its events to 'default' and delete this calendar?"))) return;
    void run(() => api.deleteCalendar(c.id, force));
  }

  function upload(c: CalendarInfo, files: FileList | null) {
    const file = files?.[0];
    if (!file) return;
    void run(async () => {
      const { imported } = await api.importCalendar(c.id, await file.text());
      toast.value = `${t("Imported")} ${imported}`;
    });
  }

  return (
    <div class="field">
      <label>{t("Calendars")}</label>
      {cals.map((c) => (
        <div class="km-row" key={c.id}>
          <input
            type="color"
            style={{ width: "32px", padding: 0 }}
            value={c.color || "#888888"}
            disabled={busy}
            onChange={(e) => void run(() => api.updateCalendar(c.id, { color: (e.target as HTMLInputElement).value }))}
          />
          <span class="grow">{c.id} <span class="muted">({c.events})</span></span>
          <button class="km-key" disabled={busy} onClick={() => rename(c)}>{t("Rename")}</button>
          <label class="km-key" style={{ cursor: "pointer" }}>
            {t("Upload")}
            <input
              type="file"
              accept=".ics,text/calendar"
              style={{ display: "none" }}
              onChange={(e) => upload(c, (e.target as HTMLInputElement).files)}
            />
          </label>
          <a class="km-key" href={api.exportCalendarUrl(c.id)} download={`${c.id}.ics`}>{t("Download")}</a>
          <button class="km-key" disabled={busy} onClick={() => remove(c)}>{t("Delete")}</button>
        </div>
      ))}
      <form
        style={{ display: "flex", gap: "6px" }}
        onSubmit={(e) => {
          e.preventDefault();
          const id = newId.trim();
          if (id) void run(() => api.createCalendar(id)).then(() => setNewId(""));
        }}
      >
        <input
          class="grow"
          placeholder={t("New calendar name")}
          value={newId}
          onInput={(e) => setNewId((e.target as HTMLInputElement).value)}
        />
        <button class="primary" type="submit" disabled={busy || !newId.trim()}>{t("Add")}</button>
      </form>
    </div>
  );
}

const msg = (e: unknown) => (e instanceof Error ? t(e.message) : t("request failed"));
