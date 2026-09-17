// Settings: theme, time format, secondary timezone, and keyboard-shortcut overrides. Theme is
// device-local; the rest persist server-side (follow the vault) via /api/settings.

import { useEffect, useState } from "preact/hooks";
import { api, authed, totpEnrolled, type AdminUser, type CalDavAccount, type CalDavCollection, type DiscoveredCalendar } from "../../api";
import { toast } from "../../lib/cache";
import { t, type LangPref } from "../../lib/i18n";
import { notificationsDenied, notificationsSupported, requestNotifications, setNotifierEnabled } from "../../lib/notify";
import { setTheme, themePref, type ThemePref } from "../../state/theme";
import { DEFAULT_KEYS, saveSettings, settings, type Action } from "../../state/settings";
import { closeModal } from "../../state/ui";
import { Overlay } from "./ModalHost";
import { SubscriptionsSection } from "../settings/SubscriptionsSection";

const THEMES: ThemePref[] = ["light", "dark", "system"];
const LANGS: [LangPref, string][] = [["auto", "Auto"], ["en", "English"], ["ru", "Русский"]];
const ZONES = ["", "UTC", "America/Los_Angeles", "America/New_York", "Europe/London", "Europe/Berlin", "Europe/Moscow", "Asia/Kolkata", "Asia/Tokyo", "Australia/Sydney"];
const ACTIONS: [Action, string][] = [
  ["calendar", "Calendar panel"], ["board", "Board panel"], ["tasks", "Tasks panel"], ["focus", "Focus panel"],
  ["palette", "Command palette"], ["new", "New item"], ["help", "Help"], ["trash", "Trash"],
  ["undo", "Undo"], ["redo", "Redo"],
];

export function Settings() {
  const s = settings.value;
  const [recording, setRecording] = useState<Action | null>(null);

  async function toggleNotifications() {
    if (s.notifications) {
      saveSettings({ notifications: false });
      setNotifierEnabled(false);
      return;
    }
    if (!(await requestNotifications())) {
      toast.value = t("Notifications are blocked by the browser — allow them in site settings.");
      return;
    }
    saveSettings({ notifications: true });
  }

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
      <h2>{t("Settings")}</h2>

      <div class="field">
        <label>{t("Theme")}</label>
        <div class="chips">
          {THEMES.map((th) => (
            <span class={`chip ${themePref.value === th ? "on" : ""}`} onClick={() => setTheme(th)}>{t(th)}</span>
          ))}
        </div>
      </div>

      <div class="field">
        <label>{t("Language")}</label>
        <div class="chips">
          {LANGS.map(([l, label]) => (
            <span class={`chip ${s.lang === l ? "on" : ""}`} onClick={() => saveSettings({ lang: l })}>
              {l === "auto" ? t("Auto") : label}
            </span>
          ))}
        </div>
      </div>

      <div class="field">
        <label>{t("Time format")}</label>
        <div class="chips">
          {(["24", "12"] as const).map((f) => (
            <span class={`chip ${s.timeFormat === f ? "on" : ""}`} onClick={() => saveSettings({ timeFormat: f })}>
              {f === "24" ? t("24-hour") : t("12-hour")}
            </span>
          ))}
        </div>
      </div>

      {notificationsSupported() && (
        <div class="field">
          <label>{t("Notifications")}</label>
          <label class="km-row" style={{ gap: "8px", cursor: "pointer" }}>
            <input
              type="checkbox"
              style={{ width: "auto" }}
              checked={s.notifications}
              disabled={notificationsDenied() && !s.notifications}
              onChange={() => void toggleNotifications()}
            />
            <span class="grow">{t("Enable notifications")}</span>
          </label>
          <div class="muted" style={{ fontSize: "11px", textAlign: "left", padding: 0 }}>
            {notificationsDenied()
              ? t("Notifications are blocked by the browser — allow them in site settings.")
              : t("Notify for event/task reminders and pomodoro phases while the app is open.")}
          </div>
        </div>
      )}

      <div class="field">
        <label>{t("Secondary timezone (calendar gutter)")}</label>
        <select value={s.secondaryTz} onChange={(e) => saveSettings({ secondaryTz: (e.target as HTMLSelectElement).value })}>
          {ZONES.map((z) => <option value={z}>{z || t("None")}</option>)}
        </select>
      </div>

      <div class="field">
        <label>{t("Keyboard shortcuts")}</label>
        <div class="keymap-edit">
          {ACTIONS.map(([a, label]) => (
            <div class="km-row">
              <span class="grow">{t(label)}</span>
              <button class="km-key" onClick={() => setRecording(a)}>
                {recording === a ? t("press a key…") : keyLabel(s.keys[a])}
              </button>
            </div>
          ))}
        </div>
        <button class="km-reset" onClick={() => saveSettings({ keys: { ...DEFAULT_KEYS } })}>{t("Reset shortcuts")}</button>
      </div>

      <PasswordSection />
      <UsersSection />
      <CalDavSection />
      <SubscriptionsSection />

      <div class="field">
        <label>{t("Account")}</label>
        <button
          class="km-reset"
          onClick={() => api.logout().then(() => { closeModal(); authed.value = false; }).catch((e) => (toast.value = e instanceof Error ? e.message : t("sign out failed")))}
        >
          {t("Sign out")}
        </button>
      </div>

      <div class="actions">
        <button onClick={closeModal}>{t("Close")}</button>
      </div>
    </Overlay>
  );
}

/// Change the signed-in user's password (session-only; the TOTP field appears when enrolled).
function PasswordSection() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [totp, setTotp] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: Event) {
    e.preventDefault();
    if (next.length < 8) {
      toast.value = t("password must be at least 8 characters");
      return;
    }
    if (next !== confirm) {
      toast.value = t("passwords do not match");
      return;
    }
    setBusy(true);
    try {
      await api.changePassword(current, next, totp);
      setCurrent(""); setNext(""); setConfirm(""); setTotp("");
      toast.value = t("password changed");
    } catch (err) {
      toast.value = err instanceof Error ? t(err.message) : t("password change failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="field">
      <label>{t("Change password")}</label>
      <form onSubmit={submit} style={{ display: "flex", flexDirection: "column", gap: "6px" }}>
        <input
          type="password"
          placeholder={t("Current password")}
          autocomplete="current-password"
          value={current}
          onInput={(e) => setCurrent((e.target as HTMLInputElement).value)}
        />
        <input
          type="password"
          placeholder={t("New password")}
          autocomplete="new-password"
          value={next}
          onInput={(e) => setNext((e.target as HTMLInputElement).value)}
        />
        <input
          type="password"
          placeholder={t("Confirm password")}
          autocomplete="new-password"
          value={confirm}
          onInput={(e) => setConfirm((e.target as HTMLInputElement).value)}
        />
        {totpEnrolled.value && (
          <input
            type="text"
            inputMode="numeric"
            placeholder={t("2FA code")}
            value={totp}
            onInput={(e) => setTotp((e.target as HTMLInputElement).value)}
          />
        )}
        <button class="primary" type="submit" disabled={busy || !current || !next}>{t("Change password")}</button>
      </form>
    </div>
  );
}

/// A sync provider: either the OAuth path (Google) or a CalDAV endpoint with basic/bearer auth.
/// `secretLabel`/`hint` tailor the credential field + guidance so, e.g., Yandex tells you to use
/// an app password rather than your account password.
interface Provider {
  label: string;
  url: string;
  auth: "basic" | "bearer";
  oauth?: boolean;
  secretLabel?: string; // i18n key for the password/token field label
  hint?: string; // i18n key for provider-specific guidance
}

const PROVIDERS: Provider[] = [
  { label: "Custom", url: "", auth: "basic" },
  { label: "Google", url: "", auth: "bearer", oauth: true, hint: "Google uses a one-click sign-in — no password needed." },
  {
    label: "Yandex", url: "https://caldav.yandex.ru/", auth: "basic", secretLabel: "App password",
    hint: "Yandex: username is your login; the password is an app password for “Calendar CalDAV” (Yandex ID → Security → App passwords), not your account password.",
  },
  {
    label: "Fastmail", url: "https://caldav.fastmail.com/", auth: "basic", secretLabel: "App password",
    hint: "Fastmail: create an app password under Settings → Privacy & Security → App passwords.",
  },
  {
    label: "iCloud", url: "https://caldav.icloud.com/", auth: "basic", secretLabel: "App-specific password",
    hint: "iCloud: generate an app-specific password at appleid.apple.com → Sign-In and Security.",
  },
  { label: "Radicale / self-hosted", url: "", auth: "basic" },
];

/// Admin-only calendar sync setup. Pick a provider first; Google runs a one-click OAuth flow,
/// every other provider takes a server URL + credentials, discovers calendars, and saves them to
/// caldav.yaml. Auto-hides on a 403 (non-admin session).
function CalDavSection() {
  const [hidden, setHidden] = useState(false);
  const [accounts, setAccounts] = useState<CalDavAccount[]>([]);
  const [collections, setCollections] = useState<CalDavCollection[]>([]);

  // Discovery form.
  const [providerLabel, setProviderLabel] = useState("Custom");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [auth, setAuth] = useState<"basic" | "bearer">("basic");
  const [username, setUsername] = useState("");
  const [secret, setSecret] = useState(""); // password or token depending on auth
  const [found, setFound] = useState<DiscoveredCalendar[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);

  const provider = PROVIDERS.find((p) => p.label === providerLabel) ?? PROVIDERS[0];

  const refresh = () =>
    api.caldavConfig().then((r) => { setAccounts(r.accounts); setCollections(r.collections); }).catch(() => setHidden(true));
  useEffect(() => { refresh(); }, []);
  if (hidden) return null;

  function selectProvider(label: string) {
    const p = PROVIDERS.find((x) => x.label === label) ?? PROVIDERS[0];
    setProviderLabel(label);
    setUrl(p.url);
    setAuth(p.auth);
    setFound(null);
    setSecret("");
  }

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
      toast.value = err instanceof Error ? err.message : t("discovery failed");
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
      toast.value = t("CalDAV account saved — sync with `mgmt sync` or the daemon");
      setFound(null); setName(""); setUrl(""); setUsername(""); setSecret("");
      await refresh();
    } catch (err) {
      toast.value = err instanceof Error ? err.message : t("save failed");
    }
  }

  async function removeAccount(a: CalDavAccount) {
    if (!window.confirm(t('Remove CalDAV account "{name}" and its collections?').replace("{name}", a.name))) return;
    try { await api.caldavDeleteAccount(a.name); await refresh(); }
    catch (err) { toast.value = err instanceof Error ? err.message : t("remove failed"); }
  }

  const secretLabel = auth === "bearer" ? t("Token") : t(provider.secretLabel ?? "Password");

  return (
    <div class="field">
      <label>{t("Calendar sync")}</label>
      <div class="user-list">
        {accounts.map((a) => (
          <div class="km-row">
            <span class="grow">{a.name} <span class="muted">({a.auth}{a.username ? ` · ${a.username}` : ""})</span></span>
            <span class="muted" style={{ fontSize: "11px" }}>
              {collections.filter((c) => c.account === a.name).length} {t("cal")}
            </span>
            <button onClick={() => removeAccount(a)} data-tip={t("Remove account")}>✕</button>
          </div>
        ))}
        {accounts.length === 0 && <div class="muted">{t("No calendar accounts yet.")}</div>}
      </div>

      {/* Provider picker — one clear choice, Google included (no separate section). */}
      <div class="muted" style={{ fontSize: "12px", marginTop: "10px", textAlign: "left" }}>{t("Add an account")}</div>
      <div class="chips">
        {PROVIDERS.map((p) => (
          <span class={`chip ${providerLabel === p.label ? "on" : ""}`} onClick={() => selectProvider(p.label)}>{t(p.label)}</span>
        ))}
      </div>

      {provider.oauth ? (
        <GoogleConnect />
      ) : (
        <form class="caldav-add" onSubmit={discover} style={{ display: "flex", flexDirection: "column", gap: "8px", marginTop: "8px" }}>
          <label class="sub">{t("Server URL")}</label>
          <input placeholder="https://caldav.example.com/" value={url} onInput={(e) => setUrl((e.target as HTMLInputElement).value)} />

          <label class="sub">{t("Sign-in method")}</label>
          <div class="chips">
            {(["basic", "bearer"] as const).map((a) => (
              <span class={`chip ${auth === a ? "on" : ""}`} onClick={() => setAuth(a)}>
                {a === "basic" ? t("Username & password") : t("Bearer token")}
              </span>
            ))}
          </div>

          {auth === "basic" && (
            <>
              <label class="sub">{t("Username")}</label>
              <input placeholder={t("username / email")} value={username} onInput={(e) => setUsername((e.target as HTMLInputElement).value)} />
            </>
          )}
          <label class="sub">{secretLabel}</label>
          <input type="password" placeholder={secretLabel} value={secret} onInput={(e) => setSecret((e.target as HTMLInputElement).value)} />

          {provider.hint && <div class="muted" style={{ fontSize: "11px", textAlign: "left" }}>{t(provider.hint)}</div>}
          <button class="primary" type="submit" disabled={busy || !url.trim()}>{busy ? t("Discovering…") : t("Discover calendars")}</button>
        </form>
      )}

      {found && (
        <div style={{ marginTop: "8px" }}>
          {found.length === 0 && <div class="muted">{t("No calendars found.")}</div>}
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
              <span class="grow">{c.name} <span class="muted">{c.supports_events ? t("events") : ""}{c.supports_events && c.supports_tasks ? "+" : ""}{c.supports_tasks ? t("tasks") : ""}</span></span>
            </label>
          ))}
          {found.length > 0 && (
            <div style={{ display: "flex", flexDirection: "column", gap: "6px", marginTop: "6px" }}>
              <input placeholder={t("account name (optional)")} value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
              <button class="primary" onClick={save} disabled={picked.size === 0}>{t("Save {n} calendar(s)").replace("{n}", String(picked.size))}</button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/// Google Calendar via one-click OAuth, shown inline when the Google provider is picked. Set the
/// OAuth client id/secret once, then "Connect" runs the browser consent flow and auto-provisions
/// the account + its calendars.
function GoogleConnect() {
  const [configured, setConfigured] = useState(false);
  const [redirectUri, setRedirectUri] = useState<string | undefined>(undefined);
  const [available, setAvailable] = useState(true);
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [account, setAccount] = useState("google");
  const [editing, setEditing] = useState(false);

  const refresh = () =>
    api
      .googleOauthStatus()
      .then((s) => {
        setConfigured(s.configured);
        setRedirectUri(s.redirect_uri);
        setEditing(!s.configured);
      })
      .catch(() => setAvailable(false));
  useEffect(() => {
    refresh();
  }, []);
  if (!available) return <div class="muted" style={{ fontSize: "11px", marginTop: "8px" }}>{t("Google sync isn't available on this server.")}</div>;

  async function saveClient(e: Event) {
    e.preventDefault();
    try {
      await api.googleOauthSet(clientId.trim(), clientSecret.trim());
      setClientId("");
      setClientSecret("");
      toast.value = t("Google OAuth client saved");
      await refresh();
    } catch (err) {
      toast.value = err instanceof Error ? err.message : t("save failed");
    }
  }

  async function connect() {
    try {
      const { url } = await api.googleConnectUrl(account.trim() || "google");
      window.location.href = url; // Google consent → callback provisions the account
    } catch (err) {
      toast.value = err instanceof Error ? err.message : t("could not start Google connect");
    }
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "8px", marginTop: "8px" }}>
      {redirectUri === undefined && (
        <div class="muted" style={{ fontSize: "11px", textAlign: "left" }}>{t("Set web.public_origin to enable the Google connect flow.")}</div>
      )}
      {editing ? (
        <form class="caldav-add" onSubmit={saveClient} style={{ display: "flex", flexDirection: "column", gap: "6px" }}>
          <div class="muted" style={{ fontSize: "11px", textAlign: "left" }}>
            {t("Create an OAuth client (Web application) in Google Cloud, enable the Calendar API, and add this redirect URI:")}
            <br /><code>{redirectUri ?? t("<set public_origin first>")}</code>
          </div>
          <label class="sub">{t("client id")}</label>
          <input placeholder={t("client id")} value={clientId} onInput={(e) => setClientId((e.target as HTMLInputElement).value)} />
          <label class="sub">{t("client secret")}</label>
          <input type="password" placeholder={t("client secret")} value={clientSecret} onInput={(e) => setClientSecret((e.target as HTMLInputElement).value)} />
          <button class="primary" type="submit">{t("Save OAuth client")}</button>
        </form>
      ) : (
        <>
          <label class="sub">{t("Account name")}</label>
          <input placeholder={t("account name")} value={account} onInput={(e) => setAccount((e.target as HTMLInputElement).value)} />
          <button class="primary" onClick={connect} disabled={!configured || !redirectUri}>{t("Connect with Google")}</button>
          <button class="km-reset" onClick={() => setEditing(true)}>{t("Change OAuth client")}</button>
        </>
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
  const [email, setEmail] = useState("");
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
      await api.adminCreateUser(id.trim(), name.trim(), email.trim());
      setId("");
      setName("");
      setEmail("");
      await refresh();
      toast.value = t("user created");
    } catch (err) {
      toast.value = err instanceof Error ? err.message : t("create failed");
    } finally {
      setBusy(false);
    }
  }

  async function syncUrl(u: AdminUser) {
    try {
      const { url } = await api.adminPairUrl(u.id);
      try {
        await navigator.clipboard.writeText(url);
        toast.value = t("sync URL copied to clipboard");
      } catch {
        window.prompt(t("Sync URL for {id} (copy it now — the token is shown once):").replace("{id}", u.id), url);
      }
    } catch (err) {
      toast.value = err instanceof Error ? err.message : t("could not generate URL");
    }
  }

  async function invite(u: AdminUser) {
    try {
      const { token, url } = await api.adminInvite(u.id);
      const link = url || token;
      try {
        await navigator.clipboard.writeText(link);
        toast.value = t("invite link copied to clipboard");
      } catch {
        window.prompt(t("Invite link for {id} (copy it now — it is shown once):").replace("{id}", u.id), link);
      }
      await refresh(); // pick up pending_invite
    } catch (err) {
      toast.value = err instanceof Error ? err.message : t("could not generate invite");
    }
  }

  async function remove(u: AdminUser) {
    if (!window.confirm(t('Delete user "{id}"? Their vault files are left on disk.').replace("{id}", u.id))) return;
    try {
      await api.adminDeleteUser(u.id);
      await refresh();
    } catch (err) {
      toast.value = err instanceof Error ? err.message : t("delete failed");
    }
  }

  return (
    <div class="field">
      <label>{t("Users")}</label>
      <div class="user-list">
        {users?.map((u) => (
          <div class="km-row">
            <span class="grow">
              {u.name || u.id} <span class="muted">({u.id}{u.email ? ` · ${u.email}` : ""})</span>
              {u.pending_invite && <span class="muted"> · {t("invite pending")}</span>}
            </span>
            {u.email && <button onClick={() => invite(u)} data-tip={t("Generate a one-time invite link")}>{t("Invite")}</button>}
            <button onClick={() => syncUrl(u)} data-tip={t("Copy an import/sync URL")}>{t("Sync URL")}</button>
            <button onClick={() => remove(u)} data-tip={t("Delete user")}>✕</button>
          </div>
        ))}
        {users?.length === 0 && <div class="muted">{t("No additional users yet.")}</div>}
      </div>
      <form class="user-add" onSubmit={add}>
        <input placeholder={t("id (a-z, 0-9, - _)")} value={id} onInput={(e) => setId((e.target as HTMLInputElement).value)} />
        <input placeholder={t("display name (optional)")} value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
        <input type="email" placeholder={t("email (for invites, optional)")} value={email} onInput={(e) => setEmail((e.target as HTMLInputElement).value)} />
        <button class="primary" type="submit" disabled={busy}>{t("Add user")}</button>
      </form>
    </div>
  );
}

function keyLabel(k: string): string {
  if (k === " ") return "Space";
  return k;
}
