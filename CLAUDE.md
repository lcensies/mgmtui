# CLAUDE.md

Guidance for working in this repo. `mgmt` is a local-first terminal calendar + markdown-task
+ kanban app, clean-room (referencing `vendor/{calcurse,kanban,vault-tasks}` for patterns
only), built to also embed as a view inside **wng** (`~/repos/wng`, the workmux dashboard).

## Workspace

Dependency-inverted layers (only `mgmt-tui` touches ratatui). `vendor/` is excluded from the
workspace.

```
mgmt-core       errors, Result, Uid newtype, Store trait
mgmt-domain     Event, Task, Collection, RecurrenceRule, Filter, SortMode,
                Workflow/StatusDef (dynamic statuses), ReminderOffset, SmartView   (pure, no I/O)
mgmt-config     YAML config (~/.config/mgmt/config.yaml): statuses, project colors,
                reminder defaults, theme overrides, smart views, CalDAV accounts/collections
mgmt-ical       iCalendar VEVENT/VTODO/VALARM/RRULE <-> domain (clean-room parser+writer)
mgmt-markdown   one task = one .md (YAML frontmatter + body), round-trip
mgmt-store      VaultStore (.md vault) + VdirStore (.ics vdir), atomic writes
mgmt-dav        CalDAV client — blocking facade over `libdav` (owns a tokio runtime)
mgmt-sync       two-way reconcile (plan_sync) over CalDAV *or* the native mgmt HTTP endpoint
                (HttpRemote); rustical config/spawn + pre/post hooks
mgmt-service    MgmtContext: load/query/mutate + undo/redo + dirty; pomodoro/flowtime engine
mgmt-backup     tar.zst snapshots of the vault → rclone crypt remote (provider-agnostic, encrypted
                at rest by rclone); pure retention planner; staged restore (never in-place)
mgmt-tui        ratatui views (Calendar/Board/Tasks/Focus) — NEVER owns the terminal
mgmt-web        axum HTTP/JSON API + PWA host over MgmtContext (Arc<RwLock> + notify reload); auth =
                argon2 password + TOTP + cookie sessions + bearer tokens; native /api/sync protocol
mgmt-cli        bin `mgmt`: tui | add | import | export | sync | serve | daemon | focus |
                backup | restore | web  (sole terminal owner)
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
- **Editor integration:** `editors/mgmt.nvim` is an nvim-cmp source that completes task
  frontmatter from `mgmt meta --json`. It is inert (registers nothing, no errors) when the
  `mgmt` binary is absent. The NixOS config sources it via a flake input + a one-line spec
  include (`nixos/vim/mgmt-nvim-spec.nix`, deletable to opt out).
- **Events are iCalendar/vdir** for portability. Local sync metadata (`href`/`etag`) has no
  iCalendar home, so it is stored as `X-MGMT-HREF`/`X-MGMT-ETAG` and **stripped before upload**
  (`event_to_ics` is clean; `event_to_ics_local` keeps the X-props). Tasks keep sync meta in
  frontmatter. Forgetting this re-pushes events every sync (412 Precondition Failed).
- **Sync is remote-wins on etag conflict** (`mgmt-sync/reconcile.rs::plan_sync`, pure + tested).
  `plan_sync` is protocol-agnostic: the CalDAV path (`sync_events/sync_tasks`, VTODO) and the
  native path (`sync_events_http/sync_tasks_http`, raw `.md`/`.ics` via `HttpRemote`) both drive it.
  A `Collection`'s `protocol: caldav|mgmt` selects which; native ships full markdown fidelity.
- **CalDAV client is `libdav`** wrapped behind a blocking `CalDavClient` facade (`mgmt-dav`)
  that owns a tokio runtime and `block_on`s; the rest of the app stays synchronous.
- **Backups are rclone-crypt snapshots.** `mgmt backup` tars+zstds the whole data root (+config) with
  a SHA-256 manifest and pushes it (plus a sidecar manifest, so `list`/`verify` never download) to an
  rclone remote; point that remote at an `rclone crypt` wrapper and mgmt never holds the passphrase.
  Retention is a pure bounded planner (`keep_last` AND `keep_days`); `mgmt restore` stages to a
  sibling dir and swaps with two renames (never untars in place), keeping the old tree aside.
- **The web server owns the vault.** `mgmt web` (`mgmt-web`, axum, owns its tokio runtime like
  `mgmt-dav`) wraps one `MgmtContext` in `Arc<RwLock>`; a `notify` watcher flags it stale so reads
  reload after external CLI/cron/sync edits. Domain types are the wire types (all `Serialize`). Auth
  (`argon2` password + hand-rolled RFC-6238 TOTP + hashed cookie sessions + bearer tokens) lives in a
  separate `web-auth.yaml`, not `config.yaml`. The PWA (`web/`, Preact) is served from `--assets-dir`
  or embedded via the `embed-ui` feature (`rust-embed`). See `docs/web.md`, `docs/backup.md`.
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
