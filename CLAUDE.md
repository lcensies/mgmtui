# CLAUDE.md

Guidance for working in this repo. `mgmt` is a local-first terminal calendar + markdown-task
+ kanban app, clean-room (referencing `vendor/{calcurse,kanban,vault-tasks}` for patterns
only), built to also embed as a view inside **wng** (`~/repos/wng`, the workmux dashboard).

## Workspace

Dependency-inverted layers (only `mgmt-tui` touches ratatui). `vendor/` is excluded from the
workspace.

```
mgmt-core       errors, Result, Uid newtype, Store trait
mgmt-domain     Event (transp/class/color/url/categories/organizer/attendees, exdates,
                recurrence_id, alarms with relative|absolute triggers), Task, Collection
                (local | caldav | ics subscription), RecurrenceRule (RFC 5545: ByDay ordinals,
                BYSETPOS, WKST, instant UNTIL), occurrence expansion + overrides, Filter,
                SortMode, Workflow/StatusDef, ReminderOffset, SmartView   (pure, no I/O)
mgmt-config     YAML config (~/.config/mgmt/config.yaml): statuses, project colors,
                reminder defaults, theme overrides, smart views, CalDAV accounts/collections,
                local `calendars:` metadata (display name/color), `calendar:` view prefs
                (week_start, hide_weekends, work_hours, visible_hours)
mgmt-ical       iCalendar VEVENT/VTODO/VALARM/RRULE/EXDATE/RECURRENCE-ID <-> domain
                (clean-room parser+writer; one document can hold a whole series)
mgmt-markdown   one task = one .md (YAML frontmatter + body), round-trip
mgmt-store      VaultStore (.md vault) + VdirStore (.ics vdir), atomic writes
mgmt-dav        CalDAV client — blocking facade over `libdav` (owns a tokio runtime); also
                CalDAV auto-discovery (principal → calendar-home → calendars)
mgmt-google     Google OAuth2 (the `oauth2` crate: auth-code + PKCE + refresh) + Meet creation
                (REST), blocking facade; one refresh token — provisioned by the CLI loopback flow
                OR the web "Connect" redirect flow — is shared as the CalDAV bearer, for Meet
                creation, and calendar discovery
mgmt-sync       reconcile over CalDAV (2-way plan_sync) *or* the native mgmt HTTP endpoint
                (HttpRemote, 3-way plan_sync3 + base snapshot, bidirectional); persistent
                Pairings + run_pairing; rustical config/spawn + pre/post hooks; read-only
                ICS/webcal subscription fetch + whole-collection replace (`ics.rs`)
mgmt-service    MgmtContext: load/query/mutate + undo/redo + dirty; occurrence-scoped event
                edit/delete (this | following | all); ICS import/export; pomodoro/flowtime engine
mgmt-backup     tar.zst snapshots of the vault → rclone crypt remote (provider-agnostic, encrypted
                at rest by rclone); pure retention planner; staged restore (never in-place)
mgmt-tui        ratatui views (Calendar/Board/Tasks/Focus) — NEVER owns the terminal
mgmt-web        axum HTTP/JSON API + PWA host; MULTI-USER (per-user vaults under users/<id>, lazy
                MgmtContext registry). Admin UI login via axum-login + tower-sessions (argon2 +
                TOTP + persistent file session store); users = isolated vaults reached over
                /api/sync by scoped bearer tokens; admin user CRUD + mgmt://pair export URLs;
                calendar CRUD + ICS import/export (`calendars.rs`), ICS subscriptions and
                public read-only feed tokens (`feed.rs`, `/api/feed/:token.ics`)
mgmt-cli        bin `mgmt`: tui | add | import | export | sync | pair | serve | daemon | focus |
                backup | restore | web | migrate-tz  (sole terminal owner)
web/            Preact + TypeScript + Vite PWA (agenda/board/tasks/focus); built into web/dist,
                served by mgmt-web via --assets-dir or the `embed-ui` feature (rust-embed)
```

Flow: `cli → {tui, service, sync, web, backup}`; `tui → {service, domain}`;
`web → {service, store, domain}`; `sync → {dav, store, ical, markdown}`;
`service → {store, ical, markdown, domain}`; `store → {domain, ical, markdown}`;
`backup → core`; `dav/ical/markdown → domain → core`.

## Key design decisions

- **Tasks are markdown-first.** `status` is a free-form id (not an enum) resolved against the
  configured `Workflow`; it doubles as the kanban column, so the board is
  `MgmtContext::board()` = `group_by(status)`, with any status id present on a task but absent
  from the workflow appended as a trailing column (no data loss). Scheduled/due tasks surface
  on the calendar; `due` is the deadline and tasks carry `reminders` (offsets like `1d`/`2h`).
- **Projects are markdown too.** One `.md` per project under `<data_root>/projects/`
  (`mgmt-store::ProjectStore`, `mgmt_markdown::{parse,serialize}_project`), so the project list is
  portable like tasks. Frontmatter carries `name`/`color`; the body is a description. A legacy
  newline `projects` *file* is auto-migrated to the directory form on first load.
- **Statuses/projects/views are config-driven.** `mgmt-config` loads one YAML file; the
  `Workflow`, project color overrides, reminder defaults, theme overrides, and the Tasks-view
  smart lists (Inbox/Today/Next7/All) come from it. `MgmtContext` holds the `Config`. Project
  color precedence: the project `.md`'s `color`, then a config override, then `auto_color`. Color
  stays a `String` below `mgmt-tui`; only the TUI parses it (`theme::parse_color`).
- **Times are stored as true UTC instants, entered/displayed in local time.** Naive input (`09:00`,
  a form field) is parsed in the host's local zone and stored as the UTC instant it names; every
  surface renders back in local time (`mgmt-cli::datetime`, `mgmt-tui` via `local_hm`/`local_to_utc`,
  the PWA via `web/src/lib/time.ts`). iCalendar `TZID` params are resolved with `chrono-tz`. All-day
  events are the exception: pure dates anchored at UTC midnight, matched by UTC calendar date so a
  non-UTC viewer doesn't see them bleed into a neighboring day. **Legacy vaults** written under the
  old "UTC wall-clock" convention (a 09:00 event stored `09:00Z`) are converted once by
  `mgmt migrate-tz` (shifts timed events + task due/scheduled by the local offset; idempotent via a
  `.state/tz-migrated` marker). Run it on every node holding a vault copy, or let sync propagate.
- **Editor integration:** `editors/mgmt.nvim` is an nvim-cmp source that completes task
  frontmatter from `mgmt meta --json`. It is inert (registers nothing, no errors) when the
  `mgmt` binary is absent. The NixOS config sources it via a flake input + a one-line spec
  include (`nixos/vim/mgmt-nvim-spec.nix`, deletable to opt out).
- **Events are iCalendar/vdir** for portability. Local sync metadata (`href`/`etag`) has no
  iCalendar home, so it is stored as `X-MGMT-HREF`/`X-MGMT-ETAG` and **stripped before upload**
  (`event_to_ics` is clean; `event_to_ics_local` keeps the X-props). Tasks keep sync meta in
  frontmatter. Forgetting this re-pushes events every sync (412 Precondition Failed).
- **A recurring series is one file.** `<uid>.ics` holds the master VEVENT plus its `RECURRENCE-ID`
  overrides; `VdirStore::upsert` merges a single component into that document instead of dropping
  its siblings (`put_series`/`load_series` handle the whole set). Expansion is RFC 5545 set
  generation (`mgmt-domain::expand`, bounded by `MAX_OCCURRENCES` periods), then EXDATEs are
  removed and overrides substituted (`Event::occurrences_with_overrides`). Occurrences carry
  `occurrence_start` (= `recurrence_id ?? start`), which is the `at` of a scoped edit.
- **Event edits are occurrence-scoped.** `PUT|DELETE /api/events/:uid?at=<rfc3339>&scope=this|
  following|all` (no `at` ⇒ the whole series) maps to `MgmtContext::{update,delete}_occurrence`:
  `this` writes/drops one `RECURRENCE-ID` override (deletes add an EXDATE), `following` bounds the
  old master with `UNTIL = at - 1s` and starts a new series (two writes ⇒ two undo steps), `all`
  rewrites the master. Undo of a series change restores every component (`Snapshot::Series`).
- **Two calendar stores, on purpose.** The collection *directory* under `<vault>/calendars/` is
  authoritative for `Event.calendar`; `config.yaml`'s `calendars:` carries presentation metadata
  (display name, color) for `/api/calendars` CRUD; `<vault>/.state/calendars.yaml` (0600) carries
  what has no iCalendar home — subscription URLs and feed tokens. Subscribed calendars are
  read-only at the service layer (`ensure_writable`), and `/api/calendars` refuses to rename or
  delete them so the sidecar cannot desync.
- **Feed tokens are bearer secrets.** `GET /api/feed/:token.ics` is the only public `/api` route
  (`middleware::is_public`); tokens are 32 OsRng bytes compared in constant time across all
  candidates, and an unknown/revoked one gets a plain 404 (never 401, no id leak). Subscription
  URLs are normalised to http/https/webcal only, fetched with a 30s timeout and an 8 MiB cap.
- **CalDAV sync is remote-wins on etag conflict** (`mgmt-sync/reconcile.rs::plan_sync`, 2-way, pure
  + tested); a `Collection`'s `protocol: caldav|mgmt` selects it. The **native** path is 3-way and
  **bidirectional** (`plan_sync3`, driven by `sync_{tasks,events}_http` over `HttpRemote`, full
  markdown fidelity): a persisted base snapshot at `<vault>/.state/sync/<pairing>/<coll>.json` lets a
  single poller push local edits *and* pull remote edits in one pass and propagate deletes both ways;
  a genuine both-sides edit is resolved last-write-wins by the `modified` stamp. Change detection
  hashes the *clean* serialization, so serialization must stay deterministic (VEVENT `DTSTAMP` is
  anchored to `modified`, not `now()`).
- **Sync setup is a persistent Pairing** (`mgmt-sync::Pairings`, `~/.config/mgmt/sync-pairings.yaml`).
  `mgmt pair import <mgmt://pair/…>` clones a remote user's vault + saves the pairing; the `poll` flag
  says who polls (the `poll:true` node runs the loop inside `mgmt daemon`, the other runs `mgmt web`
  and is polled). `mgmt sync` runs all pairings once. See `docs/sync.md`.
- **CalDAV client is `libdav`** wrapped behind a blocking `CalDavClient` facade (`mgmt-dav`)
  that owns a tokio runtime and `block_on`s; the rest of the app stays synchronous.
- **Backups are rclone-crypt snapshots.** `mgmt backup` tars+zstds the whole data root (+config) with
  a SHA-256 manifest and pushes it (plus a sidecar manifest, so `list`/`verify` never download) to an
  rclone remote; point that remote at an `rclone crypt` wrapper and mgmt never holds the passphrase.
  Retention is a pure bounded planner (`keep_last` AND `keep_days`); `mgmt restore` stages to a
  sibling dir and swaps with two renames (never untars in place), keeping the old tree aside.
- **The web server is multi-user.** `mgmt web` (`mgmt-web`, axum, owns its tokio runtime like
  `mgmt-dav`) migrates the data root to `users/<id>/` on first start and holds a lazily-opened
  per-user `MgmtContext` registry (each `Arc<RwLock>` + its own `notify` watcher). The web **UI** is
  a single admin login on the admin vault; other **users** are isolated vaults reached over
  `/api/sync` by scoped bearer tokens (a request's `Principal` — session→admin, bearer→its owner —
  is stashed by the guard and selects the vault). Sessions/login use `axum-login` + `tower-sessions`
  (argon2 + TOTP, persistent file session store). Credentials/users/tokens live in a **SQLite** DB
  at `<data_root>/.state/web-auth.db` behind a swappable `db::CredRepo` trait (`SqliteRepo`; sqlx,
  bundled) — the auth *logic* in `auth::CredStore` depends only on the trait. `--data-dir` isolates
  the DB; a legacy `web-auth.yaml` is imported once on first start (left as a backup). The rate-limit
  and TOTP-replay state live in the DB too, so they survive restarts. Admin provisions users +
  `mgmt://pair` URLs (`/api/admin/users`, PWA Settings).
  The PWA (`web/`, Preact) is served from `--assets-dir` or the `embed-ui` feature. See
  `docs/web.md`, `docs/sync.md`, `docs/backup.md`.
- **rustical** is the server (not ours): `mgmt serve` generates its TOML and spawns it.
- **Status bars are daemon-driven.** The `mgmt daemon` renders two widgets — a pomodoro/flowtime
  timer and the next event — and pushes them to a desktop bar (`mgmt-cli/statusbar.rs`: `gnome`
  via the bundled `editors/gnome` shell extension over D-Bus, plus `command`/`file` for any other
  bar). The pomodoro is a *shared* session persisted at `<data_root>/.state/pomodoro.json`
  (`mgmt-service::status::PomodoroState`, wall-clock not `Instant`) so the daemon and every
  `mgmt focus {start,toggle,skip,stop}` drive one engine; the daemon owns auto-advance + the
  "phase done" notification. The wire payload (`wire_payload`) is tick-stable (absolute
  `ends_at`), so the GNOME extension animates the countdown locally and the daemon pushes only on
  change. (The TUI Focus view keeps its own in-memory timer — it is not wired to this session.)

## Embeddability contract (standalone now, wng later)

`mgmt-tui` must never own the terminal — **no `enable_raw_mode`, `EnterAlternateScreen`, or
`event::read/poll`** in that crate (only `mgmt-cli` has them). Verify:

```bash
grep -rnE 'enable_raw_mode|EnterAlternateScreen|event::read' crates/mgmt-tui/src   # comments only
```

The embed surface is `MgmtApp`:
- `MgmtApp::new(MgmtContext) -> MgmtApp`, `.with_theme(Theme)`
- `draw(&self, frame: &mut Frame, area: Rect)` — renders into a host-provided rect
- `handle_key(&mut self, KeyEvent) -> Outcome` — `Outcome::{Continue, Quit}`
- `context(&self) -> Context` and `action_for_key(Context, KeyEvent) -> Option<Action>` —
  the same shape as wng's `dashboard/keymap.rs`, so the host can route keys itself.

**To host in wng** (`src/command/dashboard`): add a `Context::Calendar`/`Context::Kanban`
variant, hold a `MgmtApp` in the dashboard app, call `app.draw(frame, area)` from the
dashboard renderer, and forward keys to `app.handle_key`. No terminal/loop is pulled from
`mgmt-tui`. mgmt targets ratatui 0.30 / crossterm 0.29 / edition 2024 to match wng.

## Build & test

```bash
cargo run -p mgmt-cli        # TUI
cargo test --workspace       # all crates (Rust)
just web-build               # build the PWA into web/dist (npm; needs the networked pane)
cargo build --release -p mgmt-cli --features embed-ui   # single binary with the PWA baked in
tests/blackbox/.venv/bin/python -m pytest tests/blackbox -q   # e2e (CLI/TUI/web/backup)
```

Network note: `cargo fetch` and `npm ci` require the networked tmux pane (the sandbox proxy blocks
the crates.io index); build/test run offline with `--offline` once fetched. Web/backup blackbox tests
use a fake `rclone` on `PATH` and spawn `mgmt web` on an ephemeral port — no cloud account needed.

## Conventions

- `thiserror` in libs, `anyhow` in `mgmt-cli`. One error type: `mgmt_core::Error`.
- Pure logic (domain, ical, markdown, reconcile) is unit-tested; stores use `tempfile`; the
  TUI uses ratatui `TestBackend`. CalDAV is validated live against radicale (see git history).
- Keep crates small and layered; do not let ratatui/crossterm leak below `mgmt-tui`.
