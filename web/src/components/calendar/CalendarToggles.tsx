// Per-calendar visibility checkboxes. The list comes from /api/calendars; the hidden set is
// device-local (lib/cal.ts) and filters events in every view.

import { hiddenCalendars, toggleCalendar } from "../../lib/cal";
import { resource } from "../../lib/cache";
import { t } from "../../lib/i18n";
import { api } from "../../api";

export function CalendarToggles() {
  const cals = resource("calendars", api.calendars).data.value ?? [];
  const hidden = hiddenCalendars.value;
  if (cals.length < 2) return null;
  return (
    <div class="side-group">
      <div class="muted" style={{ fontSize: "11px", textAlign: "left", padding: 0 }}>{t("Calendars")}</div>
      <div class="cal-toggles">
        {cals.map((c) => (
          <label>
            <input type="checkbox" checked={!hidden.includes(c.name)} onChange={() => toggleCalendar(c.name)} />
            <span>{c.name}</span>
          </label>
        ))}
      </div>
    </div>
  );
}
