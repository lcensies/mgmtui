# mgmt

Local-first calendar + markdown tasks + kanban, in the terminal. A clean-room Rust/ratatui
take on calcurse, with markdown-first tasks (à la vault-tasks), a kanban board that is just a
view over those tasks, pomodoro/flowtime time management, and two-way CalDAV sync (events as
`VEVENT`, tasks as `VTODO`) — built so the TUI can later be **embedded as a view in wng**.

## Quick start

```bash
cargo run -p mgmt-cli            # launch the TUI
cargo run -p mgmt-cli -- add "Buy oat milk" --project home
cargo run -p mgmt-cli -- import meetings.ics --calendar work
cargo run -p mgmt-cli -- sync work          # two-way CalDAV sync
cargo run -p mgmt-cli -- serve              # run bundled rustical for phone sync
```

The release binary is named `mgmt`.

## Views & keys

Tabs: **Calendar · Board · Tasks · Focus** (`Tab`/`Shift-Tab` to switch, `?` for help).

| View     | Keys |
|----------|------|
| Calendar | `h/l` day · `j/k` week/day · `v` month/week/day · `Enter` focus agenda · `J/K` move event ±15m · `a` new · `e` edit · `t` today · `d` delete |
| Board    | `h/l` column · `j/k` card · `H/L` move card · `space` done · `a` add · `e` edit · `p` project · `P` priority · `[ ]` project scope · `/` search · `d` delete |
| Tasks    | `j/k` select · `space` done · `a` add · `e` edit · `p` project · `P` priority · `[ ]` project scope · `/` search · `d` delete |
| Focus    | `space` start/pause · `s` skip phase |
| Global   | `u` undo · `Ctrl-r` redo · `q` quit |

A context **hint bar** at the bottom always shows the useful keys for the current view.

- **Calendar** has month / week / day views (`v`). `Enter` toggles focus between the date grid
  and the day's agenda; in the agenda `j/k` select an event and `J/K` reschedule it. Tasks with
  a due/scheduled date are mixed into the agenda (Google-Calendar style).
- **New / edit events** (`a` / `e` on Calendar) open an in-TUI form — summary, date, start, end,
  location, and **recurrence** (daily/weekly/monthly/yearly, `←/→` to change). Recurring events
  expand across the calendar (marked `↻`). Tasks (`e`) open in `$EDITOR` since their markdown is
  already human-readable; mgmt reloads on exit.
- **Projects**: `p` assigns/creates a project (a picker); `[`/`]` scope the board & task list to
  one project (per-project boards). Projects persist in `<data>/projects`.
- **Search**: `/` text-filters the task list.

### CLI

Beyond the TUI, full-field scriptable CRUD (used by the `mgmt-schedule` skill):

```bash
mgmt event add "Standup" --start "2026-06-19 09:00" --duration 30 --rrule "FREQ=WEEKLY"
mgmt event list --from 2026-06-18 --to 2026-06-25
mgmt task add "Write report" --project work --due "2026-06-19 17:00" --priority high
mgmt task edit <uid-prefix> --status doing
```

Times are UTC (`YYYY-MM-DD`, `YYYY-MM-DD HH:MM`, or RFC 3339); items are addressed by UID prefix.


## Data model

- **Tasks** are the source of truth as markdown files (`<data>/tasks/<uid>.md`): YAML
  frontmatter (`status`, `project`, `area`, `tags`, `due`, `scheduled`, `priority`, sync
  `href`/`etag`) + a free-form notes body. The **kanban board is `group_by(status)`** over
  these tasks — one store, many views.
- **Events** are iCalendar `.ics` files in a vdir tree
  (`<data>/calendars/<collection>/<uid>.ics`), with recurrence (`RRULE`) and alarms. Local
  sync metadata is stashed as `X-MGMT-*` properties and stripped before upload.

Data root defaults to `$XDG_DATA_HOME/mgmt`; override with `--data-dir`.

## Sync

`mgmt` is a CalDAV **client** (wrapping [`libdav`](https://git.sr.ht/~whynothugo/libdav)). It
syncs each local collection two-way with a remote CalDAV collection (remote wins on etag
conflict). Run a bundled **rustical** server (`mgmt serve`) as the local hub that serves your
phone (DAVx5 WebDAV-Push) and aggregates remotes like Google.

Configure accounts/collections in `$XDG_CONFIG_HOME/mgmt/config.toml` — see
[`config.example.toml`](config.example.toml). Pre/post-sync hooks live in
`$XDG_CONFIG_HOME/mgmt/hooks/{pre-sync,post-sync}`.

## Architecture

A Cargo workspace with dependency-inverted layers; only `mgmt-tui` touches ratatui, and it
never owns the terminal (see [CLAUDE.md](CLAUDE.md)).

```
mgmt-core · mgmt-domain · mgmt-ical · mgmt-markdown · mgmt-store
mgmt-dav · mgmt-sync · mgmt-service · mgmt-tui · mgmt-cli(bin: mgmt)
```

## Tests

```bash
cargo test --workspace      # 72 unit/integration tests
```
