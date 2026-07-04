// Settings: theme, time format, secondary timezone, and keyboard-shortcut overrides. Theme is
// device-local; the rest persist server-side (follow the vault) via /api/settings.

import { useEffect, useState } from "preact/hooks";
import { api, type AdminUser, type CalDavAccount, type CalDavCollection, type DiscoveredCalendar } from "../../api";
import { toast } from "../../lib/cache";
import { setTheme, themePref, type ThemePref } from "../../state/theme";
import { DEFAULT_KEYS, saveSettings, settings, type Action } from "../../state/settings";
import { closeModal } from "../../state/ui";
import { Overlay } from "./ModalHost";

const THEMES: ThemePref[] = ["light", "dark", "system"];
const ZONES = ["", "UTC", "America/Los_Angeles", "America/New_York", "Europe/London", "Europe/Berlin", "Europe/Moscow", "Asia/Kolkata", "Asia/Tokyo", "Australia/Sydney"];
const ACTIONS: [Action, string][] = [
  ["calendar", "Calendar panel"], ["board", "Board panel"], ["tasks", "Tasks panel"], ["focus", "Focus panel"],
  ["palette", "Command palette"], ["new", "New item"], ["help", "Help"], ["trash", "Trash"],
  ["undo", "Undo"], ["redo", "Redo"],
];

export function Settings() {
  const s = settings.value;
  const [recording, setRecording] = useState<Action | null>(null);

  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key !== "Escape" && e.key.length >= 1) {
        saveSettings({ keys: { ...settings.value.keys, [recording]: e.key } });
      }
      setRecording(null);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [recording]);

  return (
    <Overlay>
      <h2>Settings</h2>

      <div class="field">
        <label>Theme</label>
        <div class="chips">
          {THEMES.map((t) => (
            <span class={`chip ${themePref.value === t ? "on" : ""}`} onClick={() => setTheme(t)}>{t}</span>
          ))}
        </div>
      </div>

      <div class="field">
        <label>Time format</label>
        <div class="chips">
          {(["24", "12"] as const).map((f) => (
            <span class={`chip ${s.timeFormat === f ? "on" : ""}`} onClick={() => saveSettings({ timeFormat: f })}>
              {f === "24" ? "24-hour" : "12-hour"}
            </span>
          ))}
        </div>
      </div>

      <div class="field">
        <label>Secondary timezone (calendar gutter)</label>
        <select value={s.secondaryTz} onChange={(e) => saveSettings({ secondaryTz: (e.target as HTMLSelectElement).value })}>
          {ZONES.map((z) => <option value={z}>{z || "None"}</option>)}
        </select>
      </div>

      <div class="field">
        <label>Keyboard shortcuts</label>
        <div class="keymap-edit">
          {ACTIONS.map(([a, label]) => (
            <div class="km-row">
              <span class="grow">{label}</span>
              <button class="km-key" onClick={() => setRecording(a)}>
                {recording === a ? "press a key…" : keyLabel(s.keys[a])}
              </button>
            </div>
          ))}
        </div>
        <button class="km-reset" onClick={() => saveSettings({ keys: { ...DEFAULT_KEYS } })}>Reset shortcuts</button>
      </div>

      <UsersSection />
      <CalDavSection />

      <div class="actions">
        <button onClick={closeModal}>Close</button>
      </div>
    </Overlay>
  );
}

const PROVIDER_PRESETS: { label: string; url: string; auth: string }[] = [
  { label: "Custom", url: "", auth: "basic" },
  { label: "Fastmail", url: "https://caldav.fastmail.com/", auth: "basic" },
  { label: "iCloud", url: "https://caldav.icloud.com/", auth: "basic" },
  { label: "Yandex", url: "https://caldav.yandex.ru/", auth: "basic" },
  { label: "Google", url: "https://apidata.googleusercontent.com/caldav/v2/", auth: "bearer" },
  { label: "Radicale/self-hosted", url: "", auth: "basic" },
];

/// Admin-only CalDAV account management with auto-discovery: enter server + credentials, discover
/// the calendars, tick which to sync, and save them to caldav.yaml. Auto-hides on a 403.
function CalDavSection() {
  const [hidden, setHidden] = useState(false);
  const [accounts, setAccounts] = useState<CalDavAccount[]>([]);
  const [collections, setCollections] = useState<CalDavCollection[]>([]);

  // Discovery form.
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [auth, setAuth] = useState("basic");
  const [username, setUsername] = useState("");
  const [secret, setSecret] = useState(""); // password or token depending on auth
  const [found, setFound] = useState<DiscoveredCalendar[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);

  const refresh = () =>
    api.caldavConfig().then((r) => { setAccounts(r.accounts); setCollections(r.collections); }).catch(() => setHidden(true));
  useEffect(() => { refresh(); }, []);
  if (hidden) return null;

  const usePreset = (label: string) => {
    const p = PROVIDER_PRESETS.find((x) => x.label === label);
    if (p) { setUrl(p.url); setAuth(p.auth); }
  };

  async function discover(e: Event) {
    e.preventDefault();
    setBusy(true);
    setFound(null);
    try {
      const body = {
        server_url: url.trim(),
        auth,
        username: auth === "basic" ? username.trim() : undefined,
        password: auth === "basic" ? secret : undefined,
        token: auth === "bearer" ? secret : undefined,
      };
      const r = await api.caldavDiscover(body);
      setFound(r.calendars);
      setPicked(new Set(r.calendars.map((c) => c.url))); // default: all
    } catch (err) {
      toast.value = err instanceof Error ? err.message : "discovery failed";
    } finally {
      setBusy(false);
    }
  }

  async function save() {
    if (!found) return;
    const acctName = name.trim() || url.replace(/^https?:\/\//, "").split("/")[0] || "caldav";
    // Each picked calendar becomes one collection per supported component kind.
    const colls: { name: string; kind: string; url: string }[] = [];
    for (const c of found.filter((c) => picked.has(c.url))) {
      const base = c.name.replace(/[^a-zA-Z0-9_-]+/g, "-").toLowerCase();
      if (c.supports_events) colls.push({ name: base, kind: "events", url: c.url });
      if (c.supports_tasks) colls.push({ name: `${base}-tasks`, kind: "tasks", url: c.url });
    }
    try {
      await api.caldavSaveAccount(
        {
          name: acctName,
          auth,
          username: auth === "basic" ? username.trim() : undefined,
          password: auth === "basic" ? secret || undefined : undefined,
          token: auth === "bearer" ? secret || undefined : undefined,
        },
        colls,
      );
      toast.value = "CalDAV account saved — sync with `mgmt sync` or the daemon";
      setFound(null); setName(""); setUrl(""); setUsername(""); setSecret("");
      await refresh();
    } catch (err) {
      toast.value = err instanceof Error ? err.message : "save failed";
    }
  }

  async function removeAccount(a: CalDavAccount) {
    if (!window.confirm(`Remove CalDAV account "${a.name}" and its collections?`)) return;
    try { await api.caldavDeleteAccount(a.name); await refresh(); }
    catch (err) { toast.value = err instanceof Error ? err.message : "remove failed"; }
  }

  return (
    <div class="field">
      <label>CalDAV accounts</label>
      <div class="user-list">
        {accounts.map((a) => (
          <div class="km-row">
            <span class="grow">{a.name} <span class="muted">({a.auth}{a.username ? ` · ${a.username}` : ""})</span></span>
            <span class="muted" style={{ fontSize: "11px" }}>
              {collections.filter((c) => c.account === a.name).length} cal
            </span>
            <button onClick={() => removeAccount(a)} data-tip="Remove account">✕</button>
          </div>
        ))}
        {accounts.length === 0 && <div class="muted">No CalDAV accounts yet.</div>}
      </div>

      <form class="caldav-add" onSubmit={discover} style={{ display: "flex", flexDirection: "column", gap: "6px", marginTop: "8px" }}>
        <div class="row" style={{ gap: "6px" }}>
          <select onChange={(e) => usePreset((e.target as HTMLSelectElement).value)}>
            {PROVIDER_PRESETS.map((p) => <option value={p.label}>{p.label}</option>)}
          </select>
          <input class="grow" placeholder="server URL" value={url} onInput={(e) => setUrl((e.target as HTMLInputElement).value)} />
        </div>
        <div class="row" style={{ gap: "6px" }}>
          <select value={auth} onChange={(e) => setAuth((e.target as HTMLSelectElement).value)}>
            <option value="basic">basic</option>
            <option value="bearer">bearer</option>
          </select>
          {auth === "basic" && (
            <input class="grow" placeholder="username" value={username} onInput={(e) => setUsername((e.target as HTMLInputElement).value)} />
          )}
          <input class="grow" type="password" placeholder={auth === "bearer" ? "token" : "app password"} value={secret} onInput={(e) => setSecret((e.target as HTMLInputElement).value)} />
        </div>
        <button class="primary" type="submit" disabled={busy}>{busy ? "Discovering…" : "Discover calendars"}</button>
        {url.includes("yandex") && (
          <div class="muted" style={{ fontSize: "11px" }}>
            Yandex: use your login as the username and an <b>app password</b> for “Calendar CalDAV”
            (Yandex ID → Security → App passwords), not your main password.
          </div>
        )}
      </form>

      {found && (
        <div style={{ marginTop: "8px" }}>
          {found.length === 0 && <div class="muted">No calendars found.</div>}
          {found.map((c) => (
            <label class="km-row" style={{ gap: "6px" }}>
              <input
                type="checkbox"
                style={{ width: "auto" }}
                checked={picked.has(c.url)}
                onChange={(e) => {
                  const next = new Set(picked);
                  if ((e.target as HTMLInputElement).checked) next.add(c.url); else next.delete(c.url);
                  setPicked(next);
                }}
              />
              <span class="grow">{c.name} <span class="muted">{c.supports_events ? "events" : ""}{c.supports_events && c.supports_tasks ? "+" : ""}{c.supports_tasks ? "tasks" : ""}</span></span>
            </label>
          ))}
          {found.length > 0 && (
            <div class="row" style={{ gap: "6px", marginTop: "6px" }}>
              <input class="grow" placeholder="account name (optional)" value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
              <button class="primary" onClick={save} disabled={picked.size === 0}>Save {picked.size} calendar(s)</button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/// Admin-only user management: create users (each an isolated vault) and generate the sync/pair URL
/// another device imports. Hidden automatically when the API rejects it (non-admin sessions).
function UsersSection() {
  const [users, setUsers] = useState<AdminUser[] | null>(null);
  const [hidden, setHidden] = useState(false);
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);

  const refresh = () =>
    api
      .adminUsers()
      .then((r) => setUsers(r.users))
      .catch(() => setHidden(true)); // e.g. 403 for a non-admin session

  useEffect(() => {
    refresh();
  }, []);

  if (hidden) return null;

  async function add(e: Event) {
    e.preventDefault();
    if (!id.trim()) return;
    setBusy(true);
    try {
      await api.adminCreateUser(id.trim(), name.trim());
      setId("");
      setName("");
      await refresh();
      toast.value = "user created";
    } catch (err) {
      toast.value = err instanceof Error ? err.message : "create failed";
    } finally {
      setBusy(false);
    }
  }

  async function syncUrl(u: AdminUser) {
    try {
      const { url } = await api.adminPairUrl(u.id);
      try {
        await navigator.clipboard.writeText(url);
        toast.value = "sync URL copied to clipboard";
      } catch {
        window.prompt(`Sync URL for ${u.id} (copy it now — the token is shown once):`, url);
      }
    } catch (err) {
      toast.value = err instanceof Error ? err.message : "could not generate URL";
    }
  }

  async function remove(u: AdminUser) {
    if (!window.confirm(`Delete user "${u.id}"? Their vault files are left on disk.`)) return;
    try {
      await api.adminDeleteUser(u.id);
      await refresh();
    } catch (err) {
      toast.value = err instanceof Error ? err.message : "delete failed";
    }
  }

  return (
    <div class="field">
      <label>Users</label>
      <div class="user-list">
        {users?.map((u) => (
          <div class="km-row">
            <span class="grow">{u.name || u.id} <span class="muted">({u.id})</span></span>
            <button onClick={() => syncUrl(u)} data-tip="Copy an import/sync URL">Sync URL</button>
            <button onClick={() => remove(u)} data-tip="Delete user">✕</button>
          </div>
        ))}
        {users?.length === 0 && <div class="muted">No additional users yet.</div>}
      </div>
      <form class="user-add" onSubmit={add}>
        <input placeholder="id (a-z, 0-9, - _)" value={id} onInput={(e) => setId((e.target as HTMLInputElement).value)} />
        <input placeholder="display name (optional)" value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
        <button class="primary" type="submit" disabled={busy}>Add user</button>
      </form>
    </div>
  );
}

function keyLabel(k: string): string {
  if (k === " ") return "Space";
  return k;
}
