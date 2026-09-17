// Calendar subscriptions (read-only ICS/webcal mirrors) and per-calendar feed URLs.
// Talks to /api/calendar-feeds + /api/subscriptions directly — this section owns its own
// endpoints, so it stays out of the shared api.ts surface.

import { useEffect, useState } from "preact/hooks";
import { toast } from "../../lib/cache";
import { t } from "../../lib/i18n";

interface CalendarFeed {
  id: string;
  display_name: string;
  read_only: boolean;
  subscription: { url: string; refresh_minutes: number } | null;
  feed_url: string | null;
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method,
    credentials: "include",
    headers: body !== undefined ? { "content-type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error((data as { error?: string }).error ?? `HTTP ${res.status}`);
  return data as T;
}

export function SubscriptionsSection() {
  const [cals, setCals] = useState<CalendarFeed[]>([]);
  const [url, setUrl] = useState("");
  const [name, setName] = useState("");
  const [every, setEvery] = useState(60);
  const [busy, setBusy] = useState(false);

  async function refresh() {
    try {
      setCals((await call<{ calendars: CalendarFeed[] }>("GET", "/calendar-feeds")).calendars);
    } catch {
      /* not an admin session — leave the section empty */
    }
  }

  useEffect(() => {
    refresh();
  }, []);

  function fail(e: unknown) {
    toast.value = e instanceof Error ? e.message : t("request failed");
  }

  async function run(fn: () => Promise<unknown>, ok?: string) {
    setBusy(true);
    try {
      await fn();
      await refresh();
      if (ok) toast.value = ok;
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  }

  const subscribe = (e: Event) => {
    e.preventDefault();
    run(async () => {
      const r = await call<{ events: number }>("POST", "/subscriptions", {
        url: url.trim(),
        name: name.trim() || undefined,
        refresh_minutes: every,
      });
      setUrl("");
      setName("");
      toast.value = `${t("Subscribed")} — ${r.events} ${t("events")}`;
    });
  };

  const copy = (feedUrl: string) => {
    const abs = feedUrl.startsWith("http") ? feedUrl : location.origin + feedUrl;
    navigator.clipboard?.writeText(abs).then(
      () => (toast.value = t("Feed URL copied")),
      () => (toast.value = abs),
    );
  };

  return (
    <div class="field">
      <label>{t("Calendar subscriptions & feeds")}</label>

      <div class="list">
        {cals.map((c) => (
          <div class="row" key={c.id}>
            <span class="grow">
              {c.display_name}
              {c.subscription && (
                <span class="muted" style={{ fontSize: "11px" }}>
                  {" "}
                  {t("subscribed")} · {t("every")} {c.subscription.refresh_minutes} {t("min")}
                </span>
              )}
            </span>
            {c.subscription && (
              <>
                <button disabled={busy} data-tip={t("Refresh now")} onClick={() => run(() => call("POST", `/subscriptions/${encodeURIComponent(c.id)}/refresh`), t("Refreshed"))}>↻</button>
                <button
                  disabled={busy}
                  data-tip={t("Unsubscribe")}
                  onClick={() => {
                    if (window.confirm(t("Unsubscribe and delete the mirrored events?"))) {
                      run(() => call("DELETE", `/subscriptions/${encodeURIComponent(c.id)}`));
                    }
                  }}
                >
                  ✕
                </button>
              </>
            )}
            {c.feed_url ? (
              <>
                <button disabled={busy} data-tip={t("Copy feed URL")} onClick={() => copy(c.feed_url!)}>{t("Copy")}</button>
                <button disabled={busy} data-tip={t("Revoke feed URL")} onClick={() => run(() => call("DELETE", `/calendar-feeds/${encodeURIComponent(c.id)}`), t("Feed URL revoked"))}>{t("Revoke")}</button>
              </>
            ) : (
              <button disabled={busy} data-tip={t("Create a public feed URL")} onClick={() => run(() => call("POST", `/calendar-feeds/${encodeURIComponent(c.id)}`), t("Feed URL created"))}>{t("Feed")}</button>
            )}
          </div>
        ))}
        {cals.length === 0 && <div class="muted">{t("No calendars yet.")}</div>}
      </div>

      <form onSubmit={subscribe} style={{ marginTop: "10px" }}>
        <label class="sub">{t("Subscribe to a calendar URL")}</label>
        <input placeholder="webcal://example.org/holidays.ics" value={url} onInput={(e) => setUrl((e.target as HTMLInputElement).value)} />
        <input placeholder={t("Name (optional)")} value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
        <label class="sub">{t("Refresh every (minutes)")}</label>
        <input type="number" min={5} max={1440} value={every} onInput={(e) => setEvery(Number((e.target as HTMLInputElement).value) || 60)} />
        <button class="primary" type="submit" disabled={busy || !url.trim()}>{busy ? t("Working…") : t("Subscribe")}</button>
      </form>
    </div>
  );
}
