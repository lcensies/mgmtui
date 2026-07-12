import { useState } from "preact/hooks";
import { api, authed, needsSetup, totpEnrolled } from "../api";
import { t } from "../lib/i18n";

/// The unauthenticated screen: a normal sign-in form, or — when the server has no admin yet — a
/// first-run "create admin account" form (driven by the `needsSetup` signal). The 2FA input only
/// appears when the server reports an enrolled TOTP secret (2FA is opt-in).
/** Translate the server's known login errors (incl. the parametric lockout message). */
function friendlyError(msg: string): string {
  const locked = /^too many attempts, locked for (\d+)s$/.exec(msg);
  if (locked) return t("too many attempts, locked for {s}s").replace("{s}", locked[1]);
  return t(msg); // "invalid credentials" / "unauthorized" / … fall back to the raw message
}

export function Login() {
  const setup = needsSetup.value;
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [totp, setTotp] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: Event) {
    e.preventDefault();
    setError("");
    if (setup) {
      if (password.length < 8) return setError(t("password must be at least 8 characters"));
      if (password !== confirm) return setError(t("passwords do not match"));
    }
    setBusy(true);
    try {
      if (setup) {
        await api.setup(password);
        needsSetup.value = false;
      } else {
        await api.login(password, totp, email.trim());
      }
      authed.value = true;
    } catch (err) {
      setError(err instanceof Error ? friendlyError(err.message) : t(setup ? "setup failed" : "login failed"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="center">
      <form class="login" onSubmit={submit}>
        <h1>mgmt</h1>
        {setup && <p class="muted">{t("Create the admin account for this server.")}</p>}
        {!setup && (
          <input
            type="email"
            placeholder={t("Email (leave empty for admin)")}
            autocomplete="username"
            value={email}
            onInput={(e) => setEmail((e.target as HTMLInputElement).value)}
          />
        )}
        <input
          type="password"
          placeholder={setup ? t("New admin password") : t("Password")}
          autocomplete={setup ? "new-password" : "current-password"}
          value={password}
          onInput={(e) => setPassword((e.target as HTMLInputElement).value)}
        />
        {setup && (
          <input
            type="password"
            placeholder={t("Confirm password")}
            autocomplete="new-password"
            value={confirm}
            onInput={(e) => setConfirm((e.target as HTMLInputElement).value)}
          />
        )}
        {!setup && totpEnrolled.value && (
          <input
            type="text"
            inputMode="numeric"
            placeholder={t("2FA code")}
            value={totp}
            onInput={(e) => setTotp((e.target as HTMLInputElement).value)}
          />
        )}
        <div class="error">{error}</div>
        <button class="primary" type="submit" disabled={busy}>
          {busy ? "…" : setup ? t("Create admin") : t("Sign in")}
        </button>
      </form>
    </div>
  );
}
