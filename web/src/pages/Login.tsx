import { useState } from "preact/hooks";
import { api, authed, needsSetup } from "../api";

/// The unauthenticated screen: a normal sign-in form, or — when the server has no admin yet — a
/// first-run "create admin account" form (driven by the `needsSetup` signal).
export function Login() {
  const setup = needsSetup.value;
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [totp, setTotp] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: Event) {
    e.preventDefault();
    setError("");
    if (setup) {
      if (password.length < 8) return setError("password must be at least 8 characters");
      if (password !== confirm) return setError("passwords do not match");
    }
    setBusy(true);
    try {
      if (setup) {
        await api.setup(password);
        needsSetup.value = false;
      } else {
        await api.login(password, totp);
      }
      authed.value = true;
    } catch (err) {
      setError(err instanceof Error ? err.message : setup ? "setup failed" : "login failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="center">
      <form class="login" onSubmit={submit}>
        <h1>mgmt</h1>
        {setup && <p class="muted">Create the admin account for this server.</p>}
        <input
          type="password"
          placeholder={setup ? "New admin password" : "Password"}
          autocomplete={setup ? "new-password" : "current-password"}
          value={password}
          onInput={(e) => setPassword((e.target as HTMLInputElement).value)}
        />
        {setup ? (
          <input
            type="password"
            placeholder="Confirm password"
            autocomplete="new-password"
            value={confirm}
            onInput={(e) => setConfirm((e.target as HTMLInputElement).value)}
          />
        ) : (
          <input
            type="text"
            inputMode="numeric"
            placeholder="2FA code (if enabled)"
            value={totp}
            onInput={(e) => setTotp((e.target as HTMLInputElement).value)}
          />
        )}
        <div class="error">{error}</div>
        <button class="primary" type="submit" disabled={busy}>
          {busy ? "…" : setup ? "Create admin" : "Sign in"}
        </button>
      </form>
    </div>
  );
}
