// Settings: theme, time format, secondary timezone, and keyboard-shortcut overrides. Theme is
// device-local; the rest persist server-side (follow the vault) via /api/settings.

import { useEffect, useState } from "preact/hooks";
import { api, type AdminUser } from "../../api";
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

      <div class="actions">
        <button onClick={closeModal}>Close</button>
      </div>
    </Overlay>
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
