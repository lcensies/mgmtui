import { useState } from "preact/hooks";
import { api, authed } from "../api";

export function Login() {
  const [password, setPassword] = useState("");
  const [totp, setTotp] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: Event) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.login(password, totp);
      authed.value = true;
    } catch (err) {
      setError(err instanceof Error ? err.message : "login failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="center">
      <form class="login" onSubmit={submit}>
        <h1>mgmt</h1>
        <input
          type="password"
          placeholder="Password"
          autocomplete="current-password"
          value={password}
          onInput={(e) => setPassword((e.target as HTMLInputElement).value)}
        />
        <input
          type="text"
          inputMode="numeric"
          placeholder="2FA code (if enabled)"
          value={totp}
          onInput={(e) => setTotp((e.target as HTMLInputElement).value)}
        />
        <div class="error">{error}</div>
        <button class="primary" type="submit" disabled={busy}>
          {busy ? "…" : "Sign in"}
        </button>
      </form>
    </div>
  );
}
