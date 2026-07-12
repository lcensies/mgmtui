import type { ComponentChildren } from "preact";
import { useEffect } from "preact/hooks";
import { signal } from "@preact/signals";
import { useLocation } from "preact-iso";
import { api, authed, needsSetup, serverDown, totpEnrolled } from "./api";
import { clearCache, installRevalidateOnFocus, invalidate, toast } from "./lib/cache";
import { t } from "./lib/i18n";
import { stateRes } from "./state/meta";
import { clearSelection, closeModal, modal, openModal } from "./state/ui";
import { resolvedTheme, toggleTheme } from "./state/theme";
import { loadSettings, settings, type Action } from "./state/settings";
import { Login } from "./pages/Login";
import { QuickAdd } from "./components/QuickAdd";
import { ModalHost } from "./components/modals/ModalHost";
import { Icon } from "./components/Icon";

const ready = signal(false);

const TABS: [string, string, string][] = [
  ["/", "Calendar", "calendar"],
  ["/board", "Board", "board"],
  ["/tasks", "Tasks", "tasks"],
  ["/focus", "Focus", "focus"],
];

export function App({ children }: { children: ComponentChildren }) {
  useEffect(() => {
    installRevalidateOnFocus();
    // The Google OAuth callback bounces back to /?connected=google — surface it and clean the URL.
    if (typeof window !== "undefined" && new URLSearchParams(window.location.search).get("connected") === "google") {
      toast.value = t("Google connected — calendars added (sync to pull them in)");
      window.history.replaceState({}, "", window.location.pathname);
    }
    api
      .session()
      .then((s) => {
        needsSetup.value = s.needs_setup;
        totpEnrolled.value = s.totp === true;
        authed.value = !s.needs_setup && (!s.enabled || s.authenticated);
      })
      .catch(() => (authed.value = false))
      .finally(() => (ready.value = true));
  }, []);

  useEffect(() => {
    if (authed.value) {
      invalidate("all");
      loadSettings();
    } else {
      // Signed out (or session lost): drop every cached resource + mgmt:* localStorage key so the
      // next user on this browser starts clean.
      clearCache();
    }
  }, [authed.value]);

  const loc = useLocation();
  useEffect(() => {
    const run = (a: Action, e: KeyboardEvent) => {
      switch (a) {
        case "calendar": loc.route("/"); break;
        case "board": loc.route("/board"); break;
        case "tasks": loc.route("/tasks"); break;
        case "focus": loc.route("/focus"); break;
        case "palette": e.preventDefault(); openModal({ kind: "palette" }); break;
        case "help": openModal({ kind: "help" }); break;
        case "new": openModal(loc.path === "/" ? { kind: "eventForm" } : { kind: "taskForm" }); break;
        case "undo": doUndo(); break;
        case "redo": doRedo(); break;
        case "trash": openModal({ kind: "trash" }); break;
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (modal.value) {
        if (e.key === "Escape") closeModal();
        return;
      }
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable)) return;
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const keys = settings.value.keys;
      // Match the produced character first, then fall back to the physical key (e.code), so
      // shortcuts keep working under non-Latin layouts (e.g. Russian: physical N produces "т").
      const code = e.code.startsWith("Digit")
        ? e.code.slice(5)
        : e.code.startsWith("Key")
          ? (e.shiftKey ? e.code.slice(3) : e.code.slice(3).toLowerCase())
          : null;
      const action = (Object.keys(keys) as Action[]).find((a) => keys[a] === e.key)
        ?? (code === null ? undefined : (Object.keys(keys) as Action[]).find((a) => keys[a] === code));
      if (action) run(action, e);
      else if (e.key === "Escape") clearSelection();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [loc.path]);

  // While the server is unreachable, ping it so the banner clears (and data refreshes) on its own.
  useEffect(() => {
    if (!serverDown.value) return;
    const timer = setInterval(() => {
      api.session().then(() => invalidate("all")).catch(() => {});
    }, 10_000);
    return () => clearInterval(timer);
  }, [serverDown.value]);

  if (!ready.value) return <div class="center muted">…</div>;
  // The invite page is public (the invitee has no session yet) — render it without the chrome.
  if (loc.path === "/invite") {
    return (
      <>
        {children}
        <Toaster />
      </>
    );
  }
  if (!authed.value) return <Login />;

  const st = stateRes.data.value;
  const light = resolvedTheme.value === "light";

  return (
    <div class="app">
      <header class="topbar">
        <div class="brand"><span class="brand-name">mgmt</span></div>
        <nav class="nav-group">
          {TABS.map(([path, label, icon], i) => (
            <a href={path} class={`navbtn ${loc.path === path ? "active" : ""}`} data-tip={`${t(label)} (${i + 1})`} aria-label={t(label)}>
              <Icon name={icon} />
            </a>
          ))}
        </nav>
        <span class="spacer" />
        <button class="iconbtn" data-tip={`${t("Command palette")} (:)`} aria-label={t("Command palette")} onClick={() => openModal({ kind: "palette" })}><Icon name="command" /></button>
        <button class="iconbtn hide-narrow" aria-label={t("Undo")} disabled={!st?.can_undo} onClick={doUndo}><Icon name="undo" /></button>
        <button class="iconbtn hide-narrow" aria-label={t("Redo")} disabled={!st?.can_redo} onClick={doRedo}><Icon name="redo" /></button>
        {st?.dirty && <span class="dirty" title={t("unsynced changes")} />}
        <button class="iconbtn" aria-label={t("Toggle theme")} onClick={toggleTheme}><Icon name={light ? "moon" : "sun"} /></button>
        <button class="iconbtn" aria-label={t("Settings")} onClick={() => openModal({ kind: "settings" })}><Icon name="settings" /></button>
        <button class="iconbtn hide-narrow" aria-label={t("Sign out")} onClick={() => api.logout().then(() => (authed.value = false))}><Icon name="logout" /></button>
      </header>
      {serverDown.value && <div class="offline-banner">{t("Server unreachable — retrying…")}</div>}
      <div class="quickbar"><QuickAdd /></div>
      <main>{children}</main>
      <HintBar />
      <ModalHost />
      <Toaster />
    </div>
  );
}

export async function doUndo() {
  try { await api.undo(); } finally { invalidate("all"); }
}
export async function doRedo() {
  try { await api.redo(); } finally { invalidate("all"); }
}

function HintBar() {
  return (
    <div class="hintbar">
      <b>1-4</b> {t("panels")} <b>:</b> {t("palette")} <b>?</b> {t("help")} <b>n</b> {t("new")} <b>u/U</b> {t("undo/redo")} · {t("drag events to reschedule")}
    </div>
  );
}

function Toaster() {
  const msg = toast.value;
  useEffect(() => {
    if (!msg) return;
    const t = setTimeout(() => (toast.value = null), 3000);
    return () => clearTimeout(t);
  }, [msg]);
  if (!msg) return null;
  return <div class="toast" role="alert" onClick={() => (toast.value = null)}>{msg}</div>;
}
