# Web app (`mgmt web`)

`mgmt web` serves an HTTP/JSON API over the vault plus an installable PWA, so you can view and edit
your calendar, tasks, and kanban board from a phone. It wraps the same `MgmtContext` the TUI and CLI
use — one source of truth, no logic duplicated. The server owns the vault on the machine it runs on;
other machines reconcile to it with `mgmt sync` (see [native sync](#desktop-sync) below).

## Architecture

- **Backend** — `mgmt-web` (axum): reads/mutations over `MgmtContext` behind an async `RwLock`, with
  a `notify` watcher that reloads when the vault changes underneath it (a CLI edit, a cron run).
- **Frontend** — a Preact + TypeScript + Vite PWA in `web/`, feature-complete vs the TUI (~25 KB
  gzipped, zero UI libraries): a month/week/**day time-block calendar** (colorable, single-click to
  create, drag-to-reschedule/resize), kanban **drag-and-drop**, tasks with sidebar/multi-select/sort,
  a fuzzy **command palette** (`:`), a full **event form with recurrence editor**, project picker,
  trash, and help. Minimal flat design with a Google-Calendar-blue accent and **light + dark themes**
  (SVG icons, system font). A stale-while-revalidate cache (localStorage) makes it open instantly and
  mutations are optimistic. Number keys `1`–`4` switch panels (rebindable); touch gestures (tap /
  long-press / drag) on mobile.
- **Settings** — a settings sheet (⚙) persists theme, 12/24-hour time format, a secondary-timezone
  calendar gutter, and keyboard-shortcut overrides. Non-theme settings are stored server-side
  (`GET/PUT /api/settings` → `<data_root>/.state/web-settings.json`) so they follow the vault; the
  theme is device-local.
- **GUI tests** — Playwright specs in `web/tests/` drive the real server on a fresh, EMPTY temp vault
  per test (never your data). Run with `just web-e2e` (uses the system Chromium; no browser download).
- **Time model** — mgmt stores all times as **UTC wall-clock** (a 09:00 event is `09:00Z` and the
  TUI shows "09:00"), so the web app renders and edits in UTC too, staying identical to the TUI
  regardless of the viewer's timezone.
- **Auth** — Argon2id password + optional TOTP guarding cookie sessions; static bearer tokens for the
  desktop sync client. Credentials live in `web-auth.yaml` (never in the shared `config.yaml`).

## Build the PWA

```bash
just web-build          # npm ci + vite build → web/dist
```

Then either serve it from disk or bake it into the binary:

```bash
mgmt web serve --assets-dir web/dist                 # serve web/dist from disk
cargo build --release -p mgmt-cli --features embed-ui # bake web/dist into `mgmt`
```

With `--features embed-ui` the SPA is embedded, so a single `mgmt` binary serves everything.

## Set credentials

```bash
mgmt web setpass          # reads $MGMT_WEB_PASSWORD, else prompts
mgmt web totp-enroll      # optional 2FA: prints an otpauth:// URI / secret for your app
mgmt web token-new laptop # a bearer token for the desktop sync client (printed once)
mgmt web token-list
mgmt web token-revoke laptop
```

Credentials are written to `~/.config/mgmt/web-auth.yaml` (mode 0600).

Auth modes, in order:

- **Open (dev)** — loopback bind with no password, or explicit `--no-auth`: no login at all
  (`just web-dev` uses this, so dev never prompts).
- **First-run setup** — non-loopback bind with no password: every route except
  `/api/auth/setup|session` and `/api/health` is locked until the web UI's "create admin" form
  claims the account. Claiming is atomic and **once per deployment** — after it, `/auth/setup`
  is 401/409 forever (change the password with `mgmt web setpass`).
- **Enforced** — password set: all `/api` routes require the admin session cookie or a bearer
  token; 2FA is opt-in and the login form only shows the TOTP field when a secret is enrolled.

## Run

```bash
mgmt web serve            # binds config `web.bind` (default 127.0.0.1:8321)
```

Config (`~/.config/mgmt/config.yaml`):

```yaml
web:
  bind: "127.0.0.1:8321"
  public_origin: "https://mgmt.example.com"   # enables cookie Secure + the CSRF Origin check
  session_ttl_days: 30
```

## Deploy on a public VPS

Two processes run on the VPS: `mgmt web` and the `mgmt-backup.timer`. Terminate TLS with Caddy.

```bash
# 1) credentials
mgmt web setpass && mgmt web totp-enroll
# 2) service
cp contrib/systemd/mgmt-web.service ~/.config/systemd/user/
systemctl --user enable --now mgmt-web
# 3) TLS reverse proxy (edit the hostname first)
caddy run --config contrib/Caddyfile
```

Caddyfile:

```
mgmt.example.com {
    reverse_proxy 127.0.0.1:8321
}
```

Open `https://mgmt.example.com` on your phone and "Add to Home Screen" to install the PWA.

To ship an update, `just web-deploy` (fresh PWA build + release binary with it embedded) and
restart the service. Installed PWAs poll for a new service worker hourly and on tab focus and
auto-reload; while the server is down the cached shell shows a "server unreachable" banner and
retries instead of failing silently.

## Language & notifications

- The UI is bilingual (English/Русский): Settings → Language, default follows the browser.
- Settings → Notifications enables Web Notifications for event/task reminders and pomodoro
  phase ends **while the app is open** (delivered via the service worker, so they work on
  Android). Background push with the app closed would need a push server and is not implemented;
  on iOS notifications require the installed (home-screen) PWA.

## Security notes

- Session cookie is `HttpOnly; SameSite=Lax` (+ `Secure` when `public_origin` is https).
- Mutating requests authenticated by cookie must carry a matching `Origin` (CSRF defense); bearer
  clients are exempt.
- `/api/auth/login` is rate-limited per client IP: the TCP peer address, with `X-Forwarded-For`
  honored only when the peer is loopback (i.e. a reverse proxy on the same host) — a remote
  client can't spoof its way into a fresh rate-limit bucket. TOTP codes are single-use per
  time step (replay-protected).
- Response headers: `Content-Security-Policy: default-src 'self'`, `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: no-referrer`, `Cache-Control: no-store` on the API.

## API surface (selected)

| Method & path | Purpose |
|---|---|
| `POST /api/auth/login` `{email?, password, totp?}` · `POST /api/auth/logout` · `GET /api/auth/session` | auth (email absent/`admin` → the deployment admin) |
| `POST /api/auth/password` `{current_password, new_password, totp?}` | rotate the logged-in user's password (other sessions are invalidated) |
| `GET /api/auth/invite?token=` · `POST /api/auth/invite/accept` `{token, password}` | validate / redeem a one-time invite (public; logs the invitee in) |
| `POST /api/admin/users/{id}/invite` | mint an invite token + URL for a managed user (admin) |
| `GET /api/meta` | statuses, projects+colors, smart views, sort modes |
| `GET /api/tasks?view=&project=&status=&tag=&text=&sort=` · `POST /api/tasks` | list / quick-add |
| `GET/PUT/DELETE /api/tasks/{uid}` · `POST /api/tasks/{uid}/status` · `.../toggle` | task CRUD + board moves |
| `GET /api/board?project=` | kanban columns |
| `GET /api/agenda?from=&to=` | `{events, tasks}` for a calendar range (one round-trip) |
| `GET /api/events?from=&to=` (RFC 3339) · CRUD `/api/events[/{uid}]` | recurrence-expanded events |
| `POST /api/tasks/{uid}/priority` · `.../project` | cycle priority / reassign project |
| `GET/POST/PUT/DELETE /api/projects[/{name}]` | projects (PUT sets color/description) |
| `GET /api/trash` · `POST /api/trash/{restore,purge,empty}` | trash browser |
| `GET /api/state` | `{dirty, can_undo, can_redo}` |
| `GET /api/status` · `POST /api/focus/{start\|toggle\|skip\|stop}` | shared pomodoro session |
| `POST /api/undo` · `/api/redo` · `/api/reload` | context ops |
| `/api/sync/...` | native sync protocol (bearer auth) — see below |

## Desktop sync

The desktop keeps its own local vault and reconciles to the server with the native sync protocol
(full markdown fidelity, no VTODO round-trip). Unlike CalDAV's remote-wins, this path is a 3-way
bidirectional merge against a persisted base snapshot: edits and deletes propagate both ways, and
a genuine both-sides edit resolves last-write-wins by the `modified` stamp (see docs/sync.md):

```yaml
accounts:
  - { name: vps, auth: bearer, token: "<from `mgmt web token-new`>" }
collections:
  - { name: tasks, kind: tasks, protocol: mgmt, url: "https://mgmt.example.com/api/sync", account: vps }
  - { name: personal, kind: events, protocol: mgmt, url: "https://mgmt.example.com/api/sync", account: vps }
```

Then `mgmt sync` on the desktop pushes/pulls against the server over HTTPS.
