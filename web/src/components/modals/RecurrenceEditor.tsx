// Full recurrence editor: frequency, interval, weekday chips (weekly), and a mutually-exclusive
// "ends" control (never / after N / on date). Emits a RecurrenceRule matching the Rust serde shape.

import type { Frequency, RecurrenceRule, Weekday } from "../../api";
import { t } from "../../lib/i18n";
import { ymd } from "../../lib/time";

const FREQS: Frequency[] = ["Daily", "Weekly", "Monthly", "Yearly"];
const WEEKDAYS: Weekday[] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

export function RecurrenceEditor({
  value,
  onChange,
}: {
  value?: RecurrenceRule;
  onChange: (r: RecurrenceRule | undefined) => void;
}) {
  const r = value;
  const endsMode: "never" | "count" | "until" = r?.count ? "count" : r?.until ? "until" : "never";

  const set = (patch: Partial<RecurrenceRule>) => {
    if (!r) return;
    onChange({ ...r, ...patch });
  };

  const toggleWeekday = (d: Weekday) => {
    const cur = new Set(r?.by_weekday ?? []);
    if (cur.has(d)) cur.delete(d);
    else cur.add(d);
    set({ by_weekday: WEEKDAYS.filter((w) => cur.has(w)) });
  };

  return (
    <div class="field">
      <label>{t("Repeats")}</label>
      <select
        value={r?.freq ?? ""}
        onChange={(e) => {
          const f = (e.target as HTMLSelectElement).value as Frequency | "";
          onChange(f ? { freq: f, interval: r?.interval ?? 1, by_weekday: r?.by_weekday } : undefined);
        }}
      >
        <option value="">{t("Does not repeat")}</option>
        {FREQS.map((f) => <option value={f}>{t(f)}</option>)}
      </select>

      {r && (
        <div class="recur">
          <div class="row">
            <span class="muted" style={{ padding: 0 }}>{t("every")}</span>
            <input
              type="number"
              min={1}
              style={{ width: "64px" }}
              value={r.interval}
              onInput={(e) => set({ interval: Math.max(1, Number((e.target as HTMLInputElement).value) || 1) })}
            />
            <span class="muted" style={{ padding: 0 }}>{unit(r.freq, r.interval)}</span>
          </div>

          {r.freq === "Weekly" && (
            <div class="chips">
              {WEEKDAYS.map((d) => (
                <span class={`chip ${r.by_weekday?.includes(d) ? "on" : ""}`} onClick={() => toggleWeekday(d)}>
                  {t(d)}
                </span>
              ))}
            </div>
          )}

          <div class="row" style={{ gap: "6px", flexWrap: "wrap" }}>
            <span class="muted" style={{ padding: 0 }}>{t("ends")}</span>
            <label class="row" style={{ gap: "4px" }}>
              <input type="radio" checked={endsMode === "never"} style={{ width: "auto" }}
                onChange={() => onChange({ freq: r.freq, interval: r.interval, by_weekday: r.by_weekday })} />
              {t("never")}
            </label>
            <label class="row" style={{ gap: "4px" }}>
              <input type="radio" checked={endsMode === "count"} style={{ width: "auto" }}
                onChange={() => set({ count: r.count ?? 10, until: undefined })} />
              {t("after")}
            </label>
            {endsMode === "count" && (
              <input type="number" min={1} style={{ width: "64px" }} value={r.count}
                onInput={(e) => set({ count: Math.max(1, Number((e.target as HTMLInputElement).value) || 1), until: undefined })} />
            )}
            <label class="row" style={{ gap: "4px" }}>
              <input type="radio" checked={endsMode === "until"} style={{ width: "auto" }}
                onChange={() => set({ until: r.until ?? ymd(new Date()), count: undefined })} />
              {t("on")}
            </label>
            {/* The API returns UNTIL as an instant ("…T23:59:59Z"); a date input wants the date. */}
            {endsMode === "until" && (
              <input type="date" value={r.until?.slice(0, 10)}
                onInput={(e) => set({ until: (e.target as HTMLInputElement).value, count: undefined })} />
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function unit(freq: Frequency, n: number): string {
  const base = { Daily: "day", Weekly: "week", Monthly: "month", Yearly: "year" }[freq];
  return n === 1 ? t(base) : t(`${base}s`);
}
