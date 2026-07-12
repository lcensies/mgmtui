// Public invite-acceptance page (/invite?token=…): greets the invited user, asks for a new
// password, and redeems the one-time token. The accept endpoint logs the session in, so on
// success we clear any previous user's cached data and reload into the app.

import { useEffect, useState } from "preact/hooks";
import { api } from "../api";
import { clearCache } from "../lib/cache";
import { t } from "../lib/i18n";

type Info = { valid: boolean; id?: string; email?: string | null; name?: string };

export function Invite() {
  const token = typeof window !== "undefined" ? new URLSearchParams(window.location.search).get("token") ?? "" : "";
  const [info, setInfo] = useState<Info | null>(null);
  const [failed, setFailed] = useState(false);
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!token) {
      setInfo({ valid: false });
      return;
    }
    api
      .inviteInfo(token)
      .then(setInfo)
      .catch(() => setFailed(true));
  }, [token]);

  async function submit(e: Event) {
    e.preventDefault();
    setError("");
    if (password.length < 8) return setError(t("password must be at least 8 characters"));
    if (password !== confirm) return setError(t("passwords do not match"));
    setBusy(true);
    try {
      await api.acceptInvite(token, password);
      // Signed in as the invited user now — drop anything cached for a previous user and reload.
      clearCache();
      window.location.assign("/");
    } catch (err) {
      setError(err instanceof Error ? t(err.message) : t("invite failed"));
      setBusy(false);
    }
  }

  if (failed) return <div class="center"><div class="login"><h1>mgmt</h1><div class="error">{t("Server unreachable — retrying…")}</div></div></div>;
  if (!info) return <div class="center muted">…</div>;

  if (!info.valid) {
    return (
      <div class="center">
        <div class="login">
          <h1>mgmt</h1>
          <p class="error">{t("This invite link is invalid or has already been used. Ask your admin for a new one.")}</p>
        </div>
      </div>
    );
  }

  const who = info.name || info.id || "";
  return (
    <div class="center">
      <form class="login" onSubmit={submit}>
        <h1>mgmt</h1>
        <p class="muted">
          {t("Welcome")}{who ? `, ${who}` : ""}{info.email ? ` (${info.email})` : ""} — {t("set a password to activate your account.")}
        </p>
        <input
          type="password"
          placeholder={t("New password")}
          autocomplete="new-password"
          value={password}
          onInput={(e) => setPassword((e.target as HTMLInputElement).value)}
        />
        <input
          type="password"
          placeholder={t("Confirm password")}
          autocomplete="new-password"
          value={confirm}
          onInput={(e) => setConfirm((e.target as HTMLInputElement).value)}
        />
        <div class="error">{error}</div>
        <button class="primary" type="submit" disabled={busy}>
          {busy ? "…" : t("Set password & sign in")}
        </button>
      </form>
    </div>
  );
}
